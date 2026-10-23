use super::*;

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::testserver::scratch_dir;

const STATUSES: &[&str] = &["in_progress", "blocked", "open", "open"];

fn home(label: &str) -> PathBuf {
    let home = scratch_dir(label);
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    home
}

fn env(home: &Path) -> Env {
    Env {
        home: Some(home.to_path_buf()),
        enable_todo_tools: Some("1".into()),
        ..Env::default()
    }
}

fn payload(home: &Path) -> Value {
    json!({
        "hook_event_name": "SessionStart",
        "session_id": "s-1",
        "transcript_path": home.join(".claude/projects/p/s-1.jsonl"),
        "cwd": "/somewhere"
    })
}

fn line(home: &Path, scope: Option<&str>, env: &Env) -> Option<String> {
    session_line(Some(&payload(home)), Scope::parse(scope), STATUSES, env)
}

#[test]
fn a_harness_that_is_not_claude_code_adds_no_line() {
    // This is what keeps every existing banner byte-identical.
    let home = home("adapter-generic");
    let env = env(&home);
    let codex = json!({"session_id": "s", "transcript_path": "/home/x/.codex/s.jsonl"});
    assert_eq!(
        session_line(Some(&codex), Scope::parse(None), STATUSES, &env),
        None
    );
    assert_eq!(
        session_line(
            Some(&json!({"cwd": "/x"})),
            Scope::parse(None),
            STATUSES,
            &env
        ),
        None
    );
    assert_eq!(session_line(None, Scope::parse(None), STATUSES, &env), None);
}

#[test]
fn a_ready_list_reports_a_dry_run_count_for_the_scope() {
    let home = home("adapter-ready");
    let env = env(&home);
    let all = line(&home, None, &env).expect("a line");
    assert!(all.contains("would sync 4"), "{all}");
    assert!(all.contains("all-open"), "{all}");
    assert!(all.contains("nothing is written"), "{all}");
    let active = line(&home, Some("active"), &env).expect("a line");
    assert!(active.contains("would sync 2"), "{active}");
    assert!(!home.join(".claude/tasks").exists(), "a dry run wrote");
}

#[test]
fn scope_none_and_the_kill_switch_each_say_the_sync_is_off() {
    let home = home("adapter-off");
    let env = env(&home);
    let none = line(&home, Some("none"), &env).expect("a line");
    assert!(
        none.contains("off") && none.contains("harness.scope"),
        "{none}"
    );
    let switched = Env {
        sync_switch: Some("off".into()),
        ..env.clone()
    };
    let off = line(&home, None, &switched).expect("a line");
    assert!(off.contains("off") && off.contains(SYNC_SWITCH), "{off}");
}

#[test]
fn an_unknown_scope_falls_back_to_the_banner_and_says_so() {
    let home = home("adapter-bad-scope");
    let text = line(&home, Some("everything"), &env(&home)).expect("a line");
    assert!(text.starts_with("TaskList not synced"), "{text}");
    assert!(text.contains("everything"), "{text}");
}

#[test]
fn no_tools_says_how_to_turn_them_on() {
    let home = home("adapter-no-tools");
    let env = Env {
        enable_todo_tools: None,
        ..env(&home)
    };
    let text = line(&home, None, &env).expect("a line");
    assert!(text.starts_with("TaskList not synced"), "{text}");
    assert!(text.contains("CLAUDE_CODE_ENABLE_TODO_TOOLS"), "{text}");
}

#[test]
fn a_probe_mismatch_writes_nothing_and_warns_with_the_entry() {
    let home = home("adapter-mismatch");
    let env = env(&home);
    let dir = home.join(".claude/tasks/s-1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    let text = line(&home, None, &env).expect("a line");
    assert!(text.starts_with("TaskList not synced"), "{text}");
    assert!(text.contains("notes.txt"), "{text}");
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert_eq!(entries.len(), 1, "the probe wrote into the list");
}

#[test]
fn a_bad_list_id_is_refused_and_named_safely() {
    let home = home("adapter-bad-id");
    let env = Env {
        task_list_id: Some("../x\nforged line".into()),
        ..env(&home)
    };
    let text = line(&home, None, &env).expect("a line");
    assert!(text.starts_with("TaskList not synced"), "{text}");
    assert!(!text.contains('\n'), "{text}");
}

#[test]
fn a_missing_config_dir_is_said_and_not_created() {
    let home = scratch_dir("adapter-no-store");
    let env = Env {
        claude_config_dir: Some(home.join("cfg")),
        ..env(&home)
    };
    let p = json!({"session_id": "s-1", "transcript_path": home.join("cfg/projects/x.jsonl")});
    let text = session_line(Some(&p), Scope::parse(None), STATUSES, &env).expect("a line");
    assert!(text.starts_with("TaskList not synced"), "{text}");
    assert!(!home.join("cfg").exists());
}

use super::*;

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::testserver::scratch_dir;

fn tasks() -> Vec<Task> {
    [(1, "in_progress"), (2, "blocked"), (3, "open"), (4, "open")]
        .into_iter()
        .map(|(n, status)| Task {
            urn: Some(format!("yadgar:task:{n}")),
            number: n,
            title: format!("task {n}"),
            status: status.into(),
            version: Some(1),
        })
        .collect()
}

fn home(label: &str) -> PathBuf {
    let home = scratch_dir(label);
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    home
}

fn env(home: &Path) -> Env {
    Env {
        home: Some(home.to_path_buf()),
        state_home: Some(home.join("state")),
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

fn report_for<'a>(scope: Option<&str>, project: &'a str, tasks: &'a [Task]) -> Report<'a> {
    Report {
        scope: Scope::parse(scope),
        project,
        tasks,
        complete: true,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    }
}

fn line(home: &Path, scope: Option<&str>, env: &Env) -> Option<String> {
    let tasks = tasks();
    session_line(
        Some(&payload(home)),
        &report_for(scope, "yadgarhq/docs", &tasks),
        env,
    )
}

fn list(home: &Path) -> PathBuf {
    home.join(".claude/tasks/s-1")
}

#[test]
fn a_harness_that_is_not_claude_code_adds_no_line_and_writes_nothing() {
    // This is what keeps every existing banner byte-identical.
    let home = home("adapter-generic");
    let env = env(&home);
    let tasks = tasks();
    let report = report_for(None, "yadgarhq/docs", &tasks);
    let codex = json!({"session_id": "s", "transcript_path": "/home/x/.codex/s.jsonl"});
    assert_eq!(session_line(Some(&codex), &report, &env), None);
    assert_eq!(
        session_line(Some(&json!({"cwd": "/x"})), &report, &env),
        None
    );
    assert_eq!(session_line(None, &report, &env), None);
    assert!(!home.join(".claude/tasks").exists());
}

#[test]
fn a_ready_list_is_synced_and_the_line_says_how_many() {
    let home = home("adapter-sync");
    let env = env(&home);
    let all = line(&home, None, &env).expect("a line");
    assert!(all.starts_with("TaskList holds 4 yaadgaar tasks"), "{all}");
    assert!(all.contains("4 new"), "{all}");
    assert_eq!(std::fs::read_dir(list(&home)).unwrap().count(), 4);
    let again = line(&home, None, &env).expect("a line");
    assert!(again.contains("0 new, 0 updated, 0 removed"), "{again}");
    let active = line(&home, Some("active"), &env).expect("a line");
    assert!(active.starts_with("TaskList holds 2"), "{active}");
    assert!(active.contains("2 removed"), "{active}");
}

#[test]
fn scope_none_and_the_kill_switch_each_say_the_sync_is_off_and_write_nothing() {
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
    assert!(!list(&home).exists());
}

#[test]
fn scope_none_and_the_kill_switch_each_remove_the_mirror_so_none_stays_writable() {
    for (label, scope, switch) in [
        ("adapter-off-removes-none", Some("none"), None),
        ("adapter-off-removes-switch", None, Some("off")),
    ] {
        let home = home(label);
        let env = env(&home);
        line(&home, None, &env).expect("a first, real pass");
        let foreign = list(&home).join("9.json");
        let body = r#"{"id":"9","subject":"mine","description":"","status":"pending","blocks":[],"blockedBy":[]}"#;
        std::fs::write(&foreign, body).unwrap();
        let off_env = Env {
            sync_switch: switch.map(str::to_string),
            ..env.clone()
        };
        let off = line(&home, scope, &off_env).expect("a line");
        assert!(off.starts_with("TaskList sync is off"), "{label}: {off}");
        assert!(off.contains("4 mirrored tasks removed"), "{label}: {off}");
        let left: Vec<_> = std::fs::read_dir(list(&home))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json"))
            .collect();
        assert_eq!(
            left,
            ["9.json"],
            "{label}: only the file it never wrote stays"
        );
        assert_eq!(std::fs::read_to_string(&foreign).unwrap(), body);
    }
}

#[test]
fn an_unknown_scope_falls_back_to_the_banner_and_says_so() {
    let home = home("adapter-bad-scope");
    let text = line(&home, Some("everything"), &env(&home)).expect("a line");
    assert!(text.starts_with(NOT_SYNCED), "{text}");
    assert!(text.contains("everything"), "{text}");
    assert!(!list(&home).exists());
}

#[test]
fn no_tools_says_how_to_turn_them_on_and_writes_nothing() {
    let home = home("adapter-no-tools");
    let env = Env {
        enable_todo_tools: None,
        ..env(&home)
    };
    let text = line(&home, None, &env).expect("a line");
    assert!(text.starts_with(NOT_SYNCED), "{text}");
    assert!(text.contains("CLAUDE_CODE_ENABLE_TODO_TOOLS"), "{text}");
    assert!(!list(&home).exists());
}

#[test]
fn a_probe_mismatch_writes_nothing_and_warns_with_the_entry() {
    let home = home("adapter-mismatch");
    let env = env(&home);
    let dir = list(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    let text = line(&home, None, &env).expect("a line");
    assert!(text.starts_with(NOT_SYNCED), "{text}");
    assert!(text.contains("notes.txt"), "{text}");
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert_eq!(entries.len(), 1, "something was written into the list");
    assert!(
        !home.join("state").exists(),
        "state was written for a mismatch"
    );
}

#[test]
fn a_bad_list_id_is_refused_and_named_safely() {
    let home = home("adapter-bad-id");
    let env = Env {
        task_list_id: Some("../x\nforged line".into()),
        ..env(&home)
    };
    let text = line(&home, None, &env).expect("a line");
    assert!(text.starts_with(NOT_SYNCED), "{text}");
    assert!(!text.contains('\n'), "{text}");
}

#[test]
fn a_missing_config_dir_is_said_and_not_created() {
    let home = scratch_dir("adapter-no-store");
    let env = Env {
        claude_config_dir: Some(home.join("cfg")),
        ..env(&home)
    };
    let tasks = tasks();
    let p = json!({"session_id": "s-1", "transcript_path": home.join("cfg/projects/x.jsonl")});
    let text = session_line(Some(&p), &report_for(None, "p", &tasks), &env).expect("a line");
    assert!(text.starts_with(NOT_SYNCED), "{text}");
    assert!(!home.join("cfg").exists());
}

#[test]
fn a_sync_that_refuses_keeps_the_line_and_names_the_reason() {
    let home = home("adapter-refused");
    let env = env(&home);
    let tasks = tasks();
    session_line(
        Some(&payload(&home)),
        &report_for(None, "a/one", &tasks),
        &env,
    )
    .unwrap();
    let text = session_line(
        Some(&payload(&home)),
        &report_for(None, "b/two", &tasks),
        &env,
    )
    .expect("a line");
    assert!(
        text.starts_with(NOT_SYNCED) && text.contains("a/one"),
        "{text}"
    );
}

#[test]
fn a_panic_inside_the_sync_is_contained_to_its_line() {
    let home = home("adapter-panic");
    let env = env(&home);
    let tasks = tasks();
    let report = report_for(None, super::sync::PANIC_PROJECT, &tasks);
    let text = session_line(Some(&payload(&home)), &report, &env).expect("a line");
    assert!(
        text.starts_with(NOT_SYNCED) && text.contains("internal error"),
        "{text}"
    );
}

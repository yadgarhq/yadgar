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

#[test]
fn a_pass_that_wrote_entries_asks_for_one_no_op_task_update_on_the_lowest_id() {
    let home = home("adapter-panel-nudge");
    let env = env(&home);
    let first = line(&home, None, &env).expect("a line");
    let (_, nudge) = first.split_once('\n').expect("a second line");
    assert!(!nudge.contains('\n'), "{nudge}");
    assert!(nudge.contains("TaskUpdate"), "{nudge}");
    assert!(nudge.contains("taskId \"1\""), "{nudge}");
    assert!(nudge.contains("sends nothing to yaadgaar"), "{nudge}");
    assert!(nudge.contains("task panel"), "{nudge}");
}

#[test]
fn a_pass_that_wrote_nothing_adds_no_task_update_request() {
    let home = home("adapter-panel-quiet");
    let env = env(&home);
    line(&home, None, &env).expect("a line");
    let again = line(&home, None, &env).expect("a line");
    assert!(again.contains("0 new, 0 updated"), "{again}");
    assert!(!again.contains("TaskUpdate"), "{again}");
    assert!(!again.contains('\n'), "{again}");
}

#[test]
fn a_pass_that_only_removed_entries_adds_no_task_update_request() {
    let home = home("adapter-panel-removed");
    let env = env(&home);
    line(&home, None, &env).expect("a line");
    let active = line(&home, Some("active"), &env).expect("a line");
    assert!(active.contains("2 removed"), "{active}");
    assert!(!active.contains("TaskUpdate"), "{active}");
}

#[test]
fn a_sync_that_is_off_or_refused_adds_no_task_update_request() {
    let home = home("adapter-panel-off");
    let off = Env {
        sync_switch: Some("off".into()),
        ..env(&home)
    };
    let l = line(&home, None, &off).expect("a line");
    assert!(!l.contains("TaskUpdate"), "{l}");
    let l = line(&home, Some("none"), &env(&home)).expect("a line");
    assert!(!l.contains("TaskUpdate"), "{l}");
    let no_tools = Env {
        enable_todo_tools: None,
        ..env(&home)
    };
    let l = line(&home, None, &no_tools).expect("a line");
    assert!(!l.contains("TaskUpdate"), "{l}");
}

#[test]
fn a_pass_that_only_updated_entries_still_asks_for_the_task_update() {
    let home = home("adapter-panel-updated");
    let env = env(&home);
    line(&home, None, &env).expect("a line");
    let mut moved = tasks();
    for t in &mut moved {
        t.title = format!("{} renamed", t.title);
        t.version = Some(2);
    }
    let l = session_line(
        Some(&payload(&home)),
        &report_for(None, "yadgarhq/docs", &moved),
        &env,
    )
    .expect("a line");
    assert!(l.contains("0 new, 4 updated"), "{l}");
    assert!(l.contains("TaskUpdate once on taskId"), "{l}");
}

/// The taskId and status the TaskUpdate request names, read from the line.
fn requested(line: &str) -> (String, String) {
    let (_, rest) = line.split_once("taskId \"").expect("a taskId");
    let (id, rest) = rest.split_once('"').expect("a closing quote");
    let (_, rest) = rest.split_once("with status \"").expect("a named status");
    let (status, _) = rest.split_once('"').expect("a closing quote");
    (id.to_string(), status.to_string())
}

/// Run the TaskUpdate the line asks for through the write-back's own
/// [`decide`](crate::harness::write_back::decide), against the state and the
/// file the pass left: what yaadgaar would be told.
fn ask_and_decide(home: &Path, env: &Env, line: &str) -> crate::harness::write_back::Change {
    use crate::harness::{state, write_back};
    let (id, status) = requested(line);
    let spath = state::path(&state::state_dir(env).unwrap(), "s-1");
    let st = state::load(&spath).unwrap().expect("a state");
    let entry = st.entries.values().find(|e| e.harness_id.to_string() == id);
    let entry = entry.expect("the named id is mirrored");
    let file: Value =
        serde_json::from_slice(&std::fs::read(list(home).join(format!("{id}.json"))).unwrap())
            .unwrap();
    let input = json!({"taskId": id, "status": status});
    match write_back::decide(entry, &file, &input) {
        write_back::Verdict::Apply(c) => c,
        write_back::Verdict::Refuse(r) => panic!("refused: {r}"),
    }
}

fn first_is(status: &str) -> Vec<Task> {
    let mut t = tasks();
    t[0].status = status.into();
    t
}

#[test]
fn the_task_update_the_line_asks_for_sends_nothing_to_yaadgaar_for_every_status() {
    for status in ["open", "blocked", "in_progress"] {
        let home = home(&format!("adapter-panel-noop-{status}"));
        let env = env(&home);
        let t = first_is(status);
        let report = report_for(None, "yadgarhq/docs", &t);
        let l = session_line(Some(&payload(&home)), &report, &env).expect("a line");
        let c = ask_and_decide(&home, &env, &l);
        assert_eq!(
            (c.title, c.to, c.complete),
            (None, None, false),
            "{status}: {l}"
        );
    }
}

#[test]
fn the_task_update_the_line_asks_for_sends_nothing_when_the_file_diverged() {
    // The lowest file is edited past the hooks (open in yaadgaar, in_progress
    // on disk); a rename of another task makes the next pass write. The line
    // must name the state's status, not the file's.
    let home = home("adapter-panel-noop-diverged");
    let env = env(&home);
    let t = first_is("open");
    let report = report_for(None, "yadgarhq/docs", &t);
    session_line(Some(&payload(&home)), &report, &env).expect("a line");
    let path = list(&home).join("1.json");
    let mut file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file["status"] = json!("in_progress");
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let mut t = t;
    t[1].title = "task 2 renamed".into();
    t[1].version = Some(2);
    let report = report_for(None, "yadgarhq/docs", &t);
    let l = session_line(Some(&payload(&home)), &report, &env).expect("a line");
    assert!(l.contains("1 edited past the write-back hooks"), "{l}");
    let c = ask_and_decide(&home, &env, &l);
    assert_eq!((c.title, c.to, c.complete), (None, None, false), "{l}");
}

#[test]
fn the_task_update_request_names_the_status_to_send() {
    for (yaadgaar, harness) in [
        ("open", "pending"),
        ("blocked", "pending"),
        ("in_progress", "in_progress"),
    ] {
        let home = home(&format!("adapter-panel-named-{yaadgaar}"));
        let env = env(&home);
        let t = first_is(yaadgaar);
        let report = report_for(None, "yadgarhq/docs", &t);
        let l = session_line(Some(&payload(&home)), &report, &env).expect("a line");
        assert!(
            l.contains(&format!("taskId \"1\" with status \"{harness}\"")),
            "{yaadgaar}: {l}"
        );
    }
}

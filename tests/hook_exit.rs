//! The exit status of `yaadgaar hook`, read off the built binary (ledger 1576).
//!
//! Exit 2 blocks the tool call. Exit 101 (a panic) and exit 0 are both read by
//! Claude Code as "let it run", so each path that could end in either is pinned
//! here against the real process.

#![cfg(unix)]

use std::io::Write as _;
use std::process::{Command, Output, Stdio};

fn hook(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let home = std::env::temp_dir().join(format!("yaadgaar-hook-exit-{}", std::process::id()));
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .arg("hook")
        .args(args)
        .env("HOME", &home)
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#;
    let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
    child.wait_with_output().unwrap()
}

#[test]
fn a_hook_invocation_with_bad_arguments_exits_2_not_101() {
    let out = hook(&[], &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

/// The fault is compiled in only under `debug_assertions`, so a release build
/// never reads the variable. A cargo feature was the alternative; a test gated
/// behind one runs only if CI passes the feature, and silently runs nothing if
/// it does not (ledger 837).
#[cfg(debug_assertions)]
#[test]
fn a_panic_before_dispatch_exits_2() {
    let out = hook(
        &["pre-tool-guard"],
        &[("YAADGAAR_TEST_PANIC_IN_SETUP", "1")],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("planted panic in the hook's setup"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked at"), "{stderr}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

/// A scratch home holding ONE mirrored task (harness id 4, yaadgaar #7) in
/// list `s-1`, with its state file, and the payload fields that name it.
fn mirrored_home(label: &str) -> (std::path::PathBuf, serde_json::Value) {
    let home = std::env::temp_dir().join(format!("yaadgaar-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let list = home.join(".claude/tasks/s-1");
    std::fs::create_dir_all(&list).unwrap();
    let urn = "yadgar:task:01a1";
    let record = serde_json::json!({
        "id": "4", "subject": "#7 Cut over", "status": "in_progress",
        "description": format!("A yaadgaar task.\nyaadgaar: {urn}"),
        "blocks": [], "blockedBy": [],
        "metadata": {"yaadgaar": {"urn": urn, "number": 7}}
    });
    std::fs::write(list.join("4.json"), record.to_string()).unwrap();
    let sdir = home.join("state/yaadgaar/harness/claude-code");
    std::fs::create_dir_all(&sdir).unwrap();
    let state = serde_json::json!({
        "format": 1, "project": "sentinel-org/no-such-repo", "dir": list,
        "entries": {urn: {"harness_id": 4, "number": 7, "version": 3,
            "status": "in_progress", "title": "Cut over", "written": record}}
    });
    std::fs::write(sdir.join("s-1.json"), state.to_string()).unwrap();
    let payload = serde_json::json!({
        "session_id": "s-1",
        "transcript_path": home.join(".claude/projects/p/s-1.jsonl"),
        "cwd": home,
        "prompt_id": "p-1",
        "hook_event_name": "TaskCompleted",
        "task_id": "4",
        "task_subject": "#7 Cut over"
    });
    (home, payload)
}

/// `yaadgaar hook <name>` with *payload* on stdin, against the scratch *home*
/// only: no real config, no real state, no real `~/.claude`.
fn hook_in(home: &std::path::Path, name: &str, payload: &str, envs: &[(&str, &str)]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(["hook", name])
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("YAADGAAR_CONFIG_DIR", home.join("cfg"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CLAUDE_CODE_TASK_LIST_ID")
        .env_remove("YAADGAAR_HARNESS_SYNC")
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
    child.wait_with_output().unwrap()
}

#[test]
fn task_completed_with_no_marker_exits_2_with_the_reason_on_stderr() {
    // The teammate case: no TaskUpdate of this session marked the task.
    let (home, payload) = mirrored_home("task-completed-exit");
    let out = hook_in(&home, "task-completed", &payload.to_string(), &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("yaadgaar task-completed:"), "{stderr}");
    assert!(stderr.contains("mirrors yaadgaar #7"), "{stderr}");
    assert!(!stderr.contains("No such file or directory"), "{stderr}");
    assert!(out.stdout.is_empty(), "{out:?}");
    // A task the state does not list completes untouched.
    let mut foreign = payload.clone();
    foreign["task_id"] = serde_json::json!("5");
    let out = hook_in(&home, "task-completed", &foreign.to_string(), &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_mirrored_delete_exits_2_and_a_bad_payload_or_a_panic_exits_0() {
    let (home, mut payload) = mirrored_home("task-update-exit");
    payload["hook_event_name"] = serde_json::json!("PreToolUse");
    payload["tool_name"] = serde_json::json!("TaskUpdate");
    payload["tool_input"] = serde_json::json!({"taskId": "4", "status": "deleted"});
    let out = hook_in(&home, "task-update", &payload.to_string(), &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be deleted"));
    // The deliberate fail-open: unreadable, and a panic in the setup.
    for name in ["task-update", "task-completed"] {
        let out = hook_in(&home, name, "not json", &[]);
        assert_eq!(out.status.code(), Some(0), "{name}: {out:?}");
    }
    #[cfg(debug_assertions)]
    for name in ["task-update", "task-completed"] {
        let planted = [("YAADGAAR_TEST_PANIC_IN_SETUP", "1")];
        let out = hook_in(&home, name, &payload.to_string(), &planted);
        assert_eq!(out.status.code(), Some(0), "{name}: {out:?}");
    }
    std::fs::remove_dir_all(&home).ok();
}

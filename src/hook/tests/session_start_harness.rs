//! `session-start` with the harness adapter wired in (car 1 of
//! `plans/harness-task-sync.md`): the banner is unchanged for every harness,
//! and a Claude Code session gets ONE more line saying what a sync would do.
//! Nothing is written to the harness list in this car.
//!
//! Every input is a parameter: the gateway is the test server, the home and
//! the config directory are scratch directories, and the environment is an
//! [`Env`] value, never the process's own.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::*;
use crate::harness::adapter::NOT_SYNCED;
use crate::harness::Env;

const PROJECT: &str = "sentinel-org/no-such-repo";

fn workspace(label: &str) -> PathBuf {
    let dir = crate::testserver::scratch_dir(label);
    std::fs::create_dir_all(dir.join(".yadgar")).unwrap();
    std::fs::write(dir.join(".yadgar/project-id"), PROJECT).unwrap();
    dir
}

fn tasks() -> Value {
    json!([
        {"id":"yadgar:task:a","number":3,"title":"Write the runbook","status":"open","version":1},
        {"id":"yadgar:task:b","number":7,"title":"Cut over","status":"blocked","version":2},
        {"id":"yadgar:task:c","number":9,"title":"The hook","status":"in_progress","version":5}
    ])
}

/// The banner exactly as the listing renders it, before any harness line.
fn banner() -> String {
    let rows = vec![
        Row {
            number: 3,
            status: "open".into(),
            title: "Write the runbook".into(),
        },
        Row {
            number: 7,
            status: "blocked".into(),
            title: "Cut over".into(),
        },
        Row {
            number: 9,
            status: "in_progress".into(),
            title: "The hook".into(),
        },
    ];
    render(
        PROJECT,
        &Listing {
            rows,
            complete: true,
        },
    )
}

fn claude_payload(dir: &Path, home: &Path) -> Value {
    json!({
        "hook_event_name": "SessionStart",
        "cwd": dir.display().to_string(),
        "session_id": "s-1",
        "transcript_path": home.join(".claude/projects/p/s-1.jsonl"),
        "source": "startup"
    })
}

/// Run the handler against a one-shot gateway answering [`tasks`].
fn run(payload: Value, config_dir: &Path, env: &Env) -> String {
    let body: &'static str = Box::leak(
        json!({"jsonrpc":"2.0","id":1,"result":{
            "content":[{"type":"text","text":"…"}],
            "structuredContent":{"tasks":tasks(),"next_page_token":""}
        }})
        .to_string()
        .into_boxed_str(),
    );
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (addr, served) = rt.block_on(crate::testserver::answer_once("200 OK", body));
    let config = Config::new(config_dir, format!("http://{addr}/"), "tok".into());
    let text = context_text_in(Ok(payload), Ok(config), None, DEADLINE, env).expect("a banner");
    rt.block_on(served).unwrap();
    text
}

fn home(label: &str) -> PathBuf {
    let home = crate::testserver::scratch_dir(label);
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

#[test]
fn a_session_that_is_not_claude_code_gets_exactly_todays_banner() {
    let dir = workspace("ssh-generic");
    let home = home("ssh-generic-home");
    let payload = json!({"hook_event_name": "SessionStart", "cwd": dir.display().to_string()});
    assert_eq!(run(payload, &dir, &env(&home)), banner());
}

#[test]
fn a_claude_code_session_gets_the_banner_then_one_dry_run_line_and_nothing_is_written() {
    let dir = workspace("ssh-cc");
    let home = home("ssh-cc-home");
    let text = run(claude_payload(&dir, &home), &dir, &env(&home));
    let (head, last) = text.rsplit_once('\n').expect("two parts");
    assert_eq!(head, banner());
    assert!(last.contains("would sync 3"), "{last}");
    assert!(!home.join(".claude/tasks").exists(), "car 1 wrote the list");
}

#[test]
fn the_settings_file_narrows_the_dry_run_count() {
    let dir = workspace("ssh-cc-active");
    let home = home("ssh-cc-active-home");
    std::fs::write(
        dir.join(crate::harness::scope::FILE),
        r#"{"scope":"active"}"#,
    )
    .unwrap();
    let text = run(claude_payload(&dir, &home), &dir, &env(&home));
    assert!(
        text.ends_with('.') && text.contains("would sync 2"),
        "{text}"
    );
}

#[test]
fn a_list_off_the_recorded_shape_keeps_the_banner_and_says_why() {
    let dir = workspace("ssh-cc-mismatch");
    let home = home("ssh-cc-mismatch-home");
    let list = home.join(".claude/tasks/s-1");
    std::fs::create_dir_all(&list).unwrap();
    std::fs::write(list.join("1.json"), "not json").unwrap();
    let text = run(claude_payload(&dir, &home), &dir, &env(&home));
    let (head, last) = text.rsplit_once('\n').expect("two parts");
    assert_eq!(head, banner());
    assert!(last.starts_with(NOT_SYNCED), "{last}");
    assert_eq!(
        std::fs::read_to_string(list.join("1.json")).unwrap(),
        "not json"
    );
}

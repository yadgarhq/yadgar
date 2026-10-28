//! `session-start` with the harness sync wired in (cars 1 and 2 of
//! `plans/harness-task-sync.md`): the banner is unchanged for every harness,
//! and a Claude Code session gets ONE more line, after the sync ran.
//!
//! Every input is a parameter: the gateway is the test server, the home, the
//! state directory and the config directory are scratch directories, and the
//! environment is an [`Env`] value, never the process's own.

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
    let row = |number, status: &str, title: &str| Row {
        number,
        status: status.into(),
        title: title.into(),
        ..Row::default()
    };
    let rows = vec![
        row(3, "open", "Write the runbook"),
        row(7, "blocked", "Cut over"),
        row(9, "in_progress", "The hook"),
    ];
    render(
        PROJECT,
        &Listing {
            rows,
            complete: true,
        },
    )
}

/// The banner when the harness mirror holds *n* of the tasks: the count line
/// and the mirror count, with no titles (yaadgaar #398).
fn mirrored_banner(n: usize) -> String {
    format!(
        "Tasks for {PROJECT} live in yaadgaar (mcp__yaadgaar__*).\n\
         3 open tasks: 1 in_progress, 1 blocked, 1 open. {n} mirrored into the task list."
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

fn page(tasks: Value, next: &str) -> &'static str {
    Box::leak(
        json!({"jsonrpc":"2.0","id":1,"result":{
            "content":[{"type":"text","text":"…"}],
            "structuredContent":{"tasks":tasks,"next_page_token":next}
        }})
        .to_string()
        .into_boxed_str(),
    )
}

/// Run the handler against a gateway answering *pages* in order.
fn run_pages(pages: Vec<&'static str>, payload: Value, config_dir: &Path, env: &Env) -> String {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let answers = pages.into_iter().map(|p| ("200 OK", p)).collect();
    let (addr, served) = rt.block_on(crate::testserver::answer_each(answers));
    let config = Config::new(config_dir, format!("http://{addr}/"), "tok".into());
    let text = context_text_in(Ok(payload), Ok(config), None, DEADLINE, env).expect("a banner");
    rt.block_on(served).unwrap();
    text
}

fn run(payload: Value, config_dir: &Path, env: &Env) -> String {
    run_pages(vec![page(tasks(), "")], payload, config_dir, env)
}

fn home(label: &str) -> PathBuf {
    let home = crate::testserver::scratch_dir(label);
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

/// The URNs the list's task files carry, sorted.
fn mirrored(home: &Path) -> Vec<String> {
    let dir = home.join(".claude/tasks/s-1");
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            crate::harness::record::owner(&v).map(str::to_string)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_session_that_is_not_claude_code_gets_exactly_todays_banner_and_no_list() {
    let dir = workspace("ssh-generic");
    let home = home("ssh-generic-home");
    let payload = json!({"hook_event_name": "SessionStart", "cwd": dir.display().to_string()});
    assert_eq!(run(payload, &dir, &env(&home)), banner());
    assert!(!home.join(".claude/tasks").exists());
}

#[test]
fn a_claude_code_session_gets_the_banner_then_one_line_and_the_tasks_are_mirrored() {
    let dir = workspace("ssh-cc");
    let home = home("ssh-cc-home");
    let text = run(claude_payload(&dir, &home), &dir, &env(&home));
    let (head, rest) = text.split_once("\nTaskList holds").expect("two parts");
    assert_eq!(head, mirrored_banner(3));
    assert!(rest.starts_with(" 3 yaadgaar tasks"), "{rest}");
    // The first sync wrote entries, so the harness part gains the panel line.
    let (_, nudge) = rest.rsplit_once('\n').expect("the panel line");
    assert!(nudge.contains("TaskUpdate once on taskId"), "{nudge}");
    assert_eq!(
        mirrored(&home),
        ["yadgar:task:a", "yadgar:task:b", "yadgar:task:c"]
    );
}

#[test]
fn every_page_of_the_listing_is_mirrored() {
    let dir = workspace("ssh-cc-pages");
    let home = home("ssh-cc-pages-home");
    let first = page(
        json!([{"id":"yadgar:task:p1","number":1,"title":"one","status":"open","version":1}]),
        "page-2",
    );
    let second = page(
        json!([{"id":"yadgar:task:p2","number":2,"title":"two","status":"open","version":1}]),
        "",
    );
    let text = run_pages(
        vec![first, second],
        claude_payload(&dir, &home),
        &dir,
        &env(&home),
    );
    assert!(text.contains("TaskList holds 2"), "{text}");
    assert_eq!(mirrored(&home), ["yadgar:task:p1", "yadgar:task:p2"]);
}

#[test]
fn the_settings_file_narrows_what_is_mirrored() {
    let dir = workspace("ssh-cc-active");
    let home = home("ssh-cc-active-home");
    std::fs::write(
        dir.join(crate::harness::scope::FILE),
        r#"{"scope":"active"}"#,
    )
    .unwrap();
    let text = run(claude_payload(&dir, &home), &dir, &env(&home));
    assert!(text.contains("TaskList holds 2"), "{text}");
    assert_eq!(mirrored(&home), ["yadgar:task:b", "yadgar:task:c"]);
}

#[test]
fn a_list_off_the_recorded_shape_keeps_the_banner_says_why_and_writes_nothing() {
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
    assert_eq!(std::fs::read_dir(&list).unwrap().count(), 1);
}

#[test]
fn a_row_with_no_usable_id_or_version_still_renders_the_full_banner() {
    // The gateway's golden answer shape in the older tests: ids that are not
    // URNs and a version that is not a number. The banner must not degrade
    // into the "could not be loaded" notice because the sync cannot use them.
    let dir = workspace("ssh-cc-loose");
    let home = home("ssh-cc-loose-home");
    let loose = page(
        json!([{"id":"a","number":3,"title":"Write the runbook","status":"open","version":"v"}]),
        "",
    );
    let text = run_pages(vec![loose], claude_payload(&dir, &home), &dir, &env(&home));
    assert!(
        text.starts_with("Tasks for sentinel-org/no-such-repo live in yaadgaar"),
        "{text}"
    );
    assert!(text.contains("- #3 [open] Write the runbook"), "{text}");
    assert!(text.contains("1 skipped, no task id"), "{text}");
    assert!(mirrored(&home).is_empty());
}

#[test]
fn a_mirrored_banner_carries_no_titles_and_no_framing() {
    let dir = workspace("ssh-cc-compact");
    let home = home("ssh-cc-compact-home");
    let text = run(claude_payload(&dir, &home), &dir, &env(&home));
    for title in ["Write the runbook", "Cut over", "The hook"] {
        assert!(!text.contains(title), "{text}");
    }
    assert!(!text.contains(FRAMING), "{text}");
    assert!(text.contains("3 mirrored into the task list"), "{text}");
}

#[test]
fn a_banner_keeps_its_titles_when_the_mirror_is_off() {
    let dir = workspace("ssh-cc-off");
    let home = home("ssh-cc-off-home");
    let off = Env {
        sync_switch: Some("off".into()),
        ..env(&home)
    };
    let text = run(claude_payload(&dir, &home), &dir, &off);
    assert!(text.contains("- #3 [open] Write the runbook"), "{text}");
    assert!(!text.contains("mirrored into the task list"), "{text}");
}

/// A legacy seed record of task *number* at *urn*, as `seed_tasks.py` wrote it.
fn legacy_seed(list: &Path, id: u64, number: i64, urn: &str) {
    let body = json!({
        "id": id.to_string(),
        "subject": format!("#{number} closed"),
        "description": format!("yaadgaar #{number} · {urn} · done"),
        "status": "completed",
        "blocks": [],
        "blockedBy": []
    });
    std::fs::write(list.join(format!("{id}.json")), body.to_string()).unwrap();
}

#[test]
fn a_copied_record_of_a_done_task_is_deleted_after_one_read_of_the_closed_tasks() {
    // yaadgaar #460, end to end: the second request reads the done and
    // dropped tasks, on a client of its own. Only a listed URN whose row is
    // really done or dropped counts.
    let dir = workspace("ssh-cc-stale");
    let home = home("ssh-cc-stale-home");
    let list = home.join(".claude/tasks/s-1");
    std::fs::create_dir_all(&list).unwrap();
    legacy_seed(&list, 1, 12, "yadgar:task:z");
    legacy_seed(&list, 2, 13, "yadgar:task:y");
    legacy_seed(&list, 3, 14, "yadgar:task:x");
    let closed = page(
        json!([
            {"id":"yadgar:task:z","number":12,"title":"closed","status":"done","version":2},
            {"id":"yadgar:task:y","number":13,"title":"open again","status":"in_progress","version":3}
        ]),
        "",
    );
    let rt = tokio::runtime::Runtime::new().unwrap();
    let answers = vec![("200 OK", page(tasks(), "")), ("200 OK", closed)];
    let (addr, served) = rt.block_on(crate::testserver::answer_each(answers));
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());
    let text = context_text_in(
        Ok(claude_payload(&dir, &home)),
        Ok(config),
        None,
        DEADLINE,
        &env(&home),
    )
    .expect("a banner");
    let requests = rt.block_on(served).unwrap();
    assert!(
        text.contains("1 copy of done or dropped tasks removed"),
        "{text}"
    );
    assert!(!list.join("1.json").exists(), "a done task's copy");
    assert!(
        list.join("2.json").exists(),
        "a row that is not done or dropped deletes nothing"
    );
    assert!(list.join("3.json").exists(), "an unlisted task is kept");
    assert!(
        requests[1].contains(r#""statuses":["done","dropped"]"#),
        "{}",
        requests[1]
    );
}

/// The closed read runs while the sync holds its list lock (#460), so a
/// failed read must keep the record, say so, and cost no more than
/// [`CLOSED_DEADLINE`].
fn a_failed_closed_read(label: &str, addr: std::net::SocketAddr) {
    let dir = workspace(label);
    let home = home(&format!("{label}-home"));
    let list = home.join(".claude/tasks/s-1");
    std::fs::create_dir_all(&list).unwrap();
    legacy_seed(&list, 1, 12, "yadgar:task:z");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());
    let started = std::time::Instant::now();
    let text = context_text_in(
        Ok(claude_payload(&dir, &home)),
        Ok(config),
        None,
        DEADLINE,
        &env(&home),
    )
    .expect("a banner");
    let took = started.elapsed();
    assert!(
        text.contains("1 copy of tasks out of scope kept, their status could not be read"),
        "{text}"
    );
    assert!(
        list.join("1.json").exists(),
        "a failed read keeps the record"
    );
    assert!(
        took < CLOSED_DEADLINE + std::time::Duration::from_secs(2),
        "the hook took {took:?}"
    );
}

#[test]
fn a_closed_read_that_stalls_keeps_the_record_within_its_own_deadline() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let answers = vec![("200 OK", page(tasks(), ""))];
    let addr = rt.block_on(crate::testserver::answer_each_then_hold(answers));
    a_failed_closed_read("ssh-cc-stale-stall", addr);
}

#[test]
fn a_closed_read_that_fails_keeps_the_record() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let answers = vec![
        ("200 OK", page(tasks(), "")),
        ("500 Internal Server Error", "{}"),
    ];
    let (addr, _served) = rt.block_on(crate::testserver::answer_each(answers));
    a_failed_closed_read("ssh-cc-stale-5xx", addr);
}

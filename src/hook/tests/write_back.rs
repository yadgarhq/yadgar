//! The write-back hooks (car 3), against a scratch home, a scratch state
//! directory and the test server. Never the real `~/.claude`, the real state
//! or the live gateway.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Value};

use super::*;
use crate::harness::record::{self, Desired};

const URN: &str = "yadgar:task:01a1";
const PROMPT: &str = "a0a2a4a7-ed25-4b02-b091-b6f0188fe639";

struct Home {
    root: PathBuf,
    /// The clock the handlers read (`Inputs::now`).
    now: std::cell::Cell<SystemTime>,
}

/// An arbitrary fixed instant, so marker ages are exact.
const T0: Duration = Duration::from_secs(1_800_000_000);

impl Home {
    /// A scratch home holding ONE mirrored task, harness id 4, yaadgaar #7 at
    /// version 3 with *status*.
    fn mirrored(label: &str, status: &str) -> Self {
        let root = crate::testserver::scratch_dir(label);
        let home = Self {
            root,
            now: std::cell::Cell::new(SystemTime::UNIX_EPOCH + T0),
        };
        let d = desired(status, 3, "Cut over");
        std::fs::create_dir_all(home.list()).unwrap();
        home.write_file(4, &record::record(4, &d));
        let mut st = State::new("sentinel-org/no-such-repo", &home.list());
        st.entries.insert(URN.into(), sync::entry(4, &d));
        let sdir = state::state_dir(&home.env()).unwrap();
        std::fs::create_dir_all(&sdir).unwrap();
        state::save(&state::path(&sdir, "s-1"), &st).unwrap();
        home
    }

    fn list(&self) -> PathBuf {
        self.root.join(".claude/tasks/s-1")
    }

    fn env(&self) -> Env {
        Env {
            // A named list: every session that sets it shares list `s-1`.
            task_list_id: Some("s-1".into()),
            home: Some(self.root.clone()),
            state_home: Some(self.root.join("state")),
            ..Env::default()
        }
    }

    fn file(&self, id: u64) -> Value {
        sync::read_json(&self.list().join(format!("{id}.json"))).unwrap()
    }

    fn write_file(&self, id: u64, v: &Value) {
        std::fs::write(self.list().join(format!("{id}.json")), v.to_string()).unwrap();
    }

    fn state(&self) -> State {
        let sdir = state::state_dir(&self.env()).unwrap();
        state::load(&state::path(&sdir, "s-1")).unwrap().unwrap()
    }

    fn entry(&self) -> Entry {
        self.state().entries[URN].clone()
    }

    fn base(&self, event: &str) -> Value {
        json!({
            "session_id": "s-1",
            "transcript_path": self.root.join(".claude/projects/p/s-1.jsonl"),
            "cwd": self.root.display().to_string(),
            "prompt_id": PROMPT,
            "hook_event_name": event,
        })
    }

    fn update(&self, input: Value) -> Value {
        let mut p = self.base("PreToolUse");
        p["tool_name"] = json!("TaskUpdate");
        p["tool_input"] = input;
        p["tool_use_id"] = json!("toolu_1");
        p
    }

    fn completed(&self, task_id: &str) -> Value {
        let mut p = self.base("TaskCompleted");
        p["task_id"] = json!(task_id);
        p["task_subject"] = json!("#7 Cut over");
        p
    }

    /// Run *name* with the gateway at *gateway* (`None`: not logged in).
    fn run(&self, name: &str, payload: &Value, gateway: Option<String>) -> Decision {
        self.run_counting(name, payload, gateway).0
    }

    /// [`Home::run`], and how many times the handler read the config.
    fn run_counting(
        &self,
        name: &str,
        payload: &Value,
        gateway: Option<String>,
    ) -> (Decision, usize) {
        let env = self.env();
        let reads = std::cell::Cell::new(0);
        let config = || {
            reads.set(reads.get() + 1);
            match &gateway {
                Some(url) => Ok(Config::new(
                    &self.root.join("cfg"),
                    url.clone(),
                    "tok".into(),
                )),
                None => Err(ConfigError::Missing),
            }
        };
        let decision = handle(
            name,
            payload,
            Inputs {
                env: &env,
                config: &config,
                config_dir: &self.root.join("cfg"),
                deadline: Duration::from_secs(5),
                now: self.now.get(),
            },
        );
        (decision, reads.get())
    }

    /// Run *name* against a gateway answering *answers* in order, and return
    /// the decision and the JSON body of every request it received.
    fn converse(
        &self,
        name: &str,
        payload: &Value,
        answers: Vec<&'static str>,
    ) -> (Decision, Vec<Value>) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let answers = answers.into_iter().map(|a| ("200 OK", a)).collect();
        let (addr, served) = rt.block_on(crate::testserver::answer_each(answers));
        let decision = self.run(name, payload, Some(format!("http://{addr}/")));
        let requests = rt.block_on(served).unwrap();
        (decision, requests.iter().map(|r| body(r)).collect())
    }

    /// [`Home::converse`], returning every raw request, headers included.
    fn converse_raw(
        &self,
        name: &str,
        payload: &Value,
        answers: Vec<&'static str>,
    ) -> (Decision, Vec<String>) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let answers = answers.into_iter().map(|a| ("200 OK", a)).collect();
        let (addr, served) = rt.block_on(crate::testserver::answer_each(answers));
        let decision = self.run(name, payload, Some(format!("http://{addr}/")));
        (decision, rt.block_on(served).unwrap())
    }

    fn later(&self, by: Duration) {
        self.now.set(self.now.get() + by);
    }

    fn harness_json(&self, body: &str) {
        let cfg = self.root.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("harness.json"), body).unwrap();
    }

    fn mark_completion(&self) {
        let pre = self.update(json!({"taskId": "4", "status": "completed"}));
        assert_eq!(self.run(TASK_UPDATE, &pre, None), Decision::Allow);
        assert!(self.state().pending_completions.contains_key("4"));
    }
}

fn desired(status: &str, version: u64, title: &str) -> Desired {
    Desired {
        urn: URN.into(),
        number: 7,
        title: title.into(),
        status: status.into(),
        version,
    }
}

/// A request's JSON body.
fn body(request: &str) -> Value {
    let (_, body) = request.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap()
}

fn args(request: &Value) -> (&str, &Value) {
    (
        request["params"]["name"].as_str().unwrap(),
        &request["params"]["arguments"],
    )
}

fn ok(content: Value) -> &'static str {
    Box::leak(
        json!({"jsonrpc":"2.0","id":1,"result":{
            "content":[{"type":"text","text":"…"}],
            "structuredContent": content
        }})
        .to_string()
        .into_boxed_str(),
    )
}

fn refused(text: &str) -> &'static str {
    Box::leak(
        json!({"jsonrpc":"2.0","id":1,"result":{
            "isError": true,
            "content":[{"type":"text","text": text}]
        }})
        .to_string()
        .into_boxed_str(),
    )
}

/// A loopback address nothing listens on.
fn closed_port() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/")
}

fn denied(d: Decision) -> String {
    match d {
        Decision::Deny(reason) => reason,
        Decision::Allow => panic!("allowed"),
    }
}

#[test]
fn a_subject_edit_is_written_back_with_expect_version_and_the_prefix_stripped() {
    let home = Home::mirrored("wb-subject", "open");
    let payload = home.update(json!({"taskId": "4", "subject": "#7 Cut over at noon"}));
    let (decision, requests) = home.converse(
        TASK_UPDATE,
        &payload,
        vec![ok(json!({"id": URN, "version": 4}))],
    );
    assert_eq!(decision, Decision::Allow);
    assert_eq!(requests.len(), 1);
    let (tool, a) = args(&requests[0]);
    assert_eq!(tool, "edit_task");
    assert_eq!(a["id"], URN);
    assert_eq!(a["expect_version"], 3);
    assert_eq!(a["title"], "Cut over at noon");
    assert!(a.get("status").is_none(), "{a}");
    let e = home.entry();
    assert_eq!((e.version, e.title.as_str()), (4, "Cut over at noon"));
    // `written` is the file as the harness will write it, so the next sync
    // does not read the allowed edit as one that bypassed the hooks.
    assert_eq!(e.written["subject"], "#7 Cut over at noon");
}

#[test]
fn in_progress_sends_a_transition_and_records_the_new_status() {
    let home = Home::mirrored("wb-progress", "open");
    let payload = home.update(json!({"taskId": "4", "status": "in_progress"}));
    let (decision, requests) = home.converse(
        TASK_UPDATE,
        &payload,
        vec![ok(json!({"id": URN, "version": 4, "from": "open"}))],
    );
    assert_eq!(decision, Decision::Allow);
    let (tool, a) = args(&requests[0]);
    assert_eq!(
        (tool, &a["to"], &a["expect_version"]),
        ("transition_task", &json!("in_progress"), &json!(3))
    );
    let e = home.entry();
    assert_eq!((e.version, e.status.as_str()), (4, "in_progress"));
    assert_eq!(e.written["status"], "in_progress");
}

#[test]
fn a_subject_and_a_status_in_one_call_chain_the_returned_version() {
    let home = Home::mirrored("wb-both", "open");
    let payload = home.update(json!({"taskId": "4", "subject": "New", "status": "in_progress"}));
    let (decision, requests) = home.converse(
        TASK_UPDATE,
        &payload,
        vec![
            ok(json!({"id": URN, "version": 4})),
            ok(json!({"id": URN, "version": 5, "from": "open"})),
        ],
    );
    assert_eq!(decision, Decision::Allow);
    assert_eq!(args(&requests[0]).0, "edit_task");
    assert_eq!(args(&requests[1]).0, "transition_task");
    assert_eq!(args(&requests[1]).1["expect_version"], 4);
    assert_eq!(home.entry().version, 5);
}

#[test]
fn pending_on_a_blocked_task_sends_nothing() {
    let home = Home::mirrored("wb-blocked", "blocked");
    let payload = home.update(json!({"taskId": "4", "status": "pending"}));
    // A closed port: any call would be refused as unreachable.
    assert_eq!(
        home.run(TASK_UPDATE, &payload, Some(closed_port())),
        Decision::Allow
    );
    assert_eq!(home.entry().status, "blocked");
}

#[test]
fn a_completion_is_written_back_only_through_the_marker() {
    let home = Home::mirrored("wb-complete", "in_progress");
    let pre = home.update(json!({"taskId": "4", "status": "completed"}));
    // The PreToolUse half sends nothing: no gateway at all.
    assert_eq!(home.run(TASK_UPDATE, &pre, None), Decision::Allow);
    let marker = &home.state().pending_completions["4"];
    assert_eq!(marker.prompt_id, PROMPT);

    let (decision, requests) = home.converse(
        TASK_COMPLETED,
        &home.completed("4"),
        vec![ok(json!({"id": URN, "version": 4, "from": "in_progress"}))],
    );
    assert_eq!(decision, Decision::Allow);
    let (tool, a) = args(&requests[0]);
    assert_eq!(tool, "transition_task");
    assert_eq!(
        (&a["to"], &a["expect_version"], &a["id"]),
        (&json!("done"), &json!(3), &json!(URN))
    );
    let st = home.state();
    assert!(
        st.pending_completions.is_empty(),
        "the marker was not consumed"
    );
    assert_eq!(st.entries[URN].status, "done");
}

#[test]
fn task_completed_with_no_marker_or_another_prompts_marker_refuses_and_sends_nothing() {
    // The teammate case: TaskCompleted fired without this client's TaskUpdate.
    let home = Home::mirrored("wb-no-marker", "in_progress");
    let reason = denied(home.run(TASK_COMPLETED, &home.completed("4"), Some(closed_port())));
    assert!(reason.contains("yaadgaar #7"), "{reason}");
    assert!(reason.contains("transition_task"), "{reason}");

    let pre = home.update(json!({"taskId": "4", "status": "completed"}));
    assert_eq!(home.run(TASK_UPDATE, &pre, None), Decision::Allow);
    let mut other = home.completed("4");
    other["prompt_id"] = json!("another-prompt");
    assert!(matches!(
        home.run(TASK_COMPLETED, &other, Some(closed_port())),
        Decision::Deny(_)
    ));
    let mut absent = home.completed("4");
    absent.as_object_mut().unwrap().remove("prompt_id");
    assert!(matches!(
        home.run(TASK_COMPLETED, &absent, Some(closed_port())),
        Decision::Deny(_)
    ));
    // Still there for the matching TaskCompleted.
    assert!(home.state().pending_completions.contains_key("4"));
    assert_eq!(home.entry().status, "in_progress");
}

#[test]
fn a_conflict_refuses_and_refreshes_the_file_from_yaadgaar() {
    let home = Home::mirrored("wb-conflict", "open");
    let payload = home.update(json!({"taskId": "4", "subject": "Mine"}));
    let fresh =
        json!({"id": URN, "number": 7, "title": "Theirs", "status": "in_progress", "version": 9});
    let (decision, requests) = home.converse(
        TASK_UPDATE,
        &payload,
        vec![refused("version mismatch: expected 3, have 9"), ok(fresh)],
    );
    let reason = denied(decision);
    assert!(reason.contains("version mismatch"), "{reason}");
    assert!(reason.contains("refreshed from yaadgaar"), "{reason}");
    assert_eq!(args(&requests[1]).0, "read_task");
    let file = home.file(4);
    assert_eq!(file["subject"], "#7 Theirs");
    assert_eq!(file["status"], "in_progress");
    let e = home.entry();
    assert_eq!((e.version, e.title.as_str()), (9, "Theirs"));
    assert_eq!(e.written, file);
}

#[test]
fn a_refused_completion_exits_with_the_reason_and_refreshes() {
    let home = Home::mirrored("wb-complete-conflict", "in_progress");
    let pre = home.update(json!({"taskId": "4", "status": "completed"}));
    assert_eq!(home.run(TASK_UPDATE, &pre, None), Decision::Allow);
    let fresh =
        json!({"id": URN, "number": 7, "title": "Cut over", "status": "blocked", "version": 6});
    let (decision, _) = home.converse(
        TASK_COMPLETED,
        &home.completed("4"),
        vec![refused("version mismatch"), ok(fresh)],
    );
    assert!(denied(decision).contains("version mismatch"));
    assert_eq!(home.file(4)["subject"], "#7 [blocked] Cut over");
    assert_eq!(home.entry().version, 6);
}

#[test]
fn a_refresh_to_a_terminal_task_removes_the_mirror() {
    let home = Home::mirrored("wb-refresh-done", "open");
    let payload = home.update(json!({"taskId": "4", "status": "in_progress"}));
    let fresh =
        json!({"id": URN, "number": 7, "title": "Cut over", "status": "done", "version": 8});
    let (decision, _) = home.converse(
        TASK_UPDATE,
        &payload,
        vec![refused("illegal move"), ok(fresh)],
    );
    assert!(matches!(decision, Decision::Deny(_)));
    assert!(!home.list().join("4.json").exists());
    assert!(!home.state().entries.contains_key(URN));
    let hwm = std::fs::read_to_string(home.list().join(".highwatermark")).unwrap();
    assert_eq!(hwm, "4");
}

#[test]
fn an_unreachable_gateway_or_no_login_refuses_and_changes_nothing() {
    let home = Home::mirrored("wb-down", "open");
    let before = (home.file(4), home.state());
    let payload = home.update(json!({"taskId": "4", "subject": "New"}));
    let reason = denied(home.run(TASK_UPDATE, &payload, Some(closed_port())));
    assert!(reason.contains("could not reach yaadgaar"), "{reason}");
    assert!(reason.contains("not made"), "{reason}");
    let reason = denied(home.run(TASK_UPDATE, &payload, None));
    assert!(reason.contains("not logged in"), "{reason}");
    assert_eq!((home.file(4), home.state()), before);

    let pre = home.update(json!({"taskId": "4", "status": "completed"}));
    assert_eq!(home.run(TASK_UPDATE, &pre, None), Decision::Allow);
    let reason = denied(home.run(TASK_COMPLETED, &home.completed("4"), Some(closed_port())));
    assert!(reason.contains("could not reach yaadgaar"), "{reason}");
}

#[test]
fn a_title_over_128_characters_is_refused_before_any_call() {
    let home = Home::mirrored("wb-long", "open");
    let payload = home.update(json!({"taskId": "4", "subject": "x".repeat(129)}));
    let reason = denied(home.run(TASK_UPDATE, &payload, Some(closed_port())));
    assert_eq!(reason, crate::harness::write_back::too_long(7, 129));
}

#[test]
fn deleting_a_mirrored_entry_is_refused() {
    let home = Home::mirrored("wb-delete", "open");
    let payload = home.update(json!({"taskId": "4", "status": "deleted"}));
    let reason = denied(home.run(TASK_UPDATE, &payload, None));
    assert!(reason.contains("cannot be deleted"), "{reason}");
    assert!(home.list().join("4.json").exists());
}

#[test]
fn a_foreign_task_passes_through_untouched() {
    let home = Home::mirrored("wb-foreign", "open");
    let foreign = json!({"id":"5","subject":"mine","description":"","status":"pending","blocks":[],"blockedBy":[]});
    home.write_file(5, &foreign);
    let before = home.state();
    // Allowed, and the saved login never even read.
    let passes = |name: &str, payload: &Value| {
        let (decision, reads) = home.run_counting(name, payload, Some(closed_port()));
        assert_eq!(decision, Decision::Allow, "{payload}");
        assert_eq!(reads, 0, "the config was read for {payload}");
    };
    // A task the state does not list: even a delete passes, with no gateway.
    for input in [
        json!({"taskId": "5", "status": "deleted"}),
        json!({"taskId": "5", "subject": "x".repeat(300)}),
        json!({"taskId": "99", "status": "completed"}),
    ] {
        passes(TASK_UPDATE, &home.update(input));
    }
    passes(TASK_COMPLETED, &home.completed("5"));
    // A mirrored id whose file lost its URN is no longer ours.
    let mut disowned = home.file(4);
    disowned["description"] = json!("rewritten by someone else");
    home.write_file(4, &disowned);
    passes(
        TASK_UPDATE,
        &home.update(json!({"taskId": "4", "status": "deleted"})),
    );
    passes(TASK_COMPLETED, &home.completed("4"));
    // Another tool.
    let mut bash = home.update(json!({"taskId": "4"}));
    bash["tool_name"] = json!("Bash");
    passes(TASK_UPDATE, &bash);
    assert_eq!(home.state(), before);
    assert_eq!(home.file(5), foreign);
}

#[test]
fn the_kill_switch_and_scope_none_pass_a_mirrored_task_through() {
    let home = Home::mirrored("wb-off", "open");
    let delete = home.update(json!({"taskId": "4", "status": "deleted"}));
    let mut env = home.env();
    env.sync_switch = Some("off".into());
    let off = handle(
        TASK_UPDATE,
        &delete,
        Inputs {
            env: &env,
            config: &|| Err(ConfigError::Missing),
            config_dir: &home.root.join("cfg"),
            deadline: Duration::from_secs(1),
            now: SystemTime::now(),
        },
    );
    assert_eq!(off, Decision::Allow);
    let cfg = home.root.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("harness.json"), r#"{"scope":"none"}"#).unwrap();
    assert_eq!(home.run(TASK_UPDATE, &delete, None), Decision::Allow);
}

#[test]
fn a_refusal_never_carries_the_text_claude_code_reads_as_a_missing_hook() {
    let io = std::io::Error::from(std::io::ErrorKind::NotFound);
    let raw = format!("x: {}", std::io::Error::from_raw_os_error(2));
    assert!(
        raw.contains(MISSING_FILE),
        "the OS text changed: {raw} ({io})"
    );
    assert!(!scrub(&raw).contains(MISSING_FILE));
    // An unknown handler name allows, with nothing read.
    let d = handle(
        "no-such-handler",
        &json!({}),
        Inputs {
            env: &Env::default(),
            config: &|| Err(ConfigError::Missing),
            config_dir: Path::new("/nonexistent"),
            deadline: Duration::from_secs(1),
            now: SystemTime::now(),
        },
    );
    assert_eq!(d, Decision::Allow);
}

#[test]
fn no_state_file_passes_a_task_through() {
    let home = Home::mirrored("wb-no-state", "open");
    std::fs::remove_dir_all(home.root.join("state")).unwrap();
    let payload = home.update(json!({"taskId": "4", "status": "deleted"}));
    assert_eq!(home.run(TASK_UPDATE, &payload, None), Decision::Allow);
}

// ---- review fixes: the project on the wire, the marker, the fail-closed reads

#[test]
fn the_write_carries_the_state_s_project_not_the_one_the_cwd_derives() {
    let home = Home::mirrored("wb-wire-project", "open");
    // The agent `cd`ed into another repository: its cwd derives project B.
    let b = home.root.join("repo-b");
    std::fs::create_dir_all(b.join(".yadgar")).unwrap();
    std::fs::write(b.join(".yadgar/project-id"), "other-org/project-b\n").unwrap();
    assert_eq!(
        crate::project::derive(&b).as_deref(),
        Some("other-org/project-b")
    );
    let mut payload = home.update(json!({"taskId": "4", "subject": "#7 Renamed"}));
    payload["cwd"] = json!(b.display().to_string());
    let (decision, raw) = home.converse_raw(
        TASK_UPDATE,
        &payload,
        vec![ok(json!({"id": URN, "version": 4}))],
    );
    assert_eq!(decision, Decision::Allow);
    let sent = raw[0].to_ascii_lowercase();
    assert!(
        sent.contains("x-yadgar-project: sentinel-org/no-such-repo\r\n"),
        "{sent}"
    );
    assert!(!sent.contains("project-b"), "{sent}");
}

#[test]
fn a_marker_from_another_session_does_not_complete() {
    let home = Home::mirrored("wb-marker-session", "in_progress");
    home.mark_completion();
    let mut other = home.completed("4");
    other["session_id"] = json!("s-2");
    let reason = denied(home.run(TASK_COMPLETED, &other, Some(closed_port())));
    assert!(reason.contains("yaadgaar #7"), "{reason}");
}

#[test]
fn a_stale_marker_does_not_complete_and_the_ttl_is_configurable() {
    let home = Home::mirrored("wb-marker-stale", "in_progress");
    home.mark_completion();
    home.later(crate::harness::scope::COMPLETION_TTL + Duration::from_secs(1));
    let reason = denied(home.run(TASK_COMPLETED, &home.completed("4"), Some(closed_port())));
    assert!(reason.contains("yaadgaar #7"), "{reason}");

    // A TTL from harness.json: 5 s. Six seconds on, the marker is stale.
    let home = Home::mirrored("wb-marker-ttl", "in_progress");
    home.harness_json(r#"{"completion_ttl_secs": 5}"#);
    home.mark_completion();
    home.later(Duration::from_secs(6));
    // The no-marker refusal, not the unreachable-gateway one: a stale
    // marker must not get as far as the send.
    let reason = denied(home.run(TASK_COMPLETED, &home.completed("4"), Some(closed_port())));
    assert!(
        reason.contains("is closed only by an explicit TaskUpdate to completed"),
        "{reason}"
    );
    // Within the TTL it completes.
    let home = Home::mirrored("wb-marker-fresh", "in_progress");
    home.harness_json(r#"{"completion_ttl_secs": 5}"#);
    home.mark_completion();
    home.later(Duration::from_secs(4));
    let (decision, _) = home.converse(
        TASK_COMPLETED,
        &home.completed("4"),
        vec![ok(json!({"id": URN, "version": 4, "from": "in_progress"}))],
    );
    assert_eq!(decision, Decision::Allow);
}

#[test]
fn a_non_completing_task_update_clears_the_marker() {
    let home = Home::mirrored("wb-marker-cleared", "in_progress");
    home.mark_completion();
    // The agent then moves the same task back to pending instead.
    let back = home.update(json!({"taskId": "4", "status": "pending"}));
    let (decision, _) = home.converse(
        TASK_UPDATE,
        &back,
        vec![ok(json!({"id": URN, "version": 4, "from": "in_progress"}))],
    );
    assert_eq!(decision, Decision::Allow);
    assert!(home.state().pending_completions.is_empty());
    assert!(matches!(
        home.run(TASK_COMPLETED, &home.completed("4"), Some(closed_port())),
        Decision::Deny(_)
    ));
    // A REFUSED TaskUpdate clears it too: the completion it marked is not
    // the call that follows.
    home.mark_completion();
    let delete = home.update(json!({"taskId": "4", "status": "deleted"}));
    assert!(matches!(
        home.run(TASK_UPDATE, &delete, None),
        Decision::Deny(_)
    ));
    assert!(home.state().pending_completions.is_empty());
}

#[test]
fn a_completing_task_update_with_no_prompt_id_is_refused_and_marks_nothing() {
    let home = Home::mirrored("wb-no-prompt", "in_progress");
    let mut pre = home.update(json!({"taskId": "4", "status": "completed"}));
    pre.as_object_mut().unwrap().remove("prompt_id");
    let reason = denied(home.run(TASK_UPDATE, &pre, None));
    assert!(reason.contains("prompt_id"), "{reason}");
    assert!(home.state().pending_completions.is_empty());
}

#[test]
fn an_unreadable_state_or_settings_refuses_an_edit_to_a_file_that_carries_a_urn() {
    // A corrupt state.
    let home = Home::mirrored("wb-corrupt-state", "open");
    let sdir = state::state_dir(&home.env()).unwrap();
    std::fs::write(state::path(&sdir, "s-1"), "{not json").unwrap();
    let payload = home.update(json!({"taskId": "4", "status": "deleted"}));
    let reason = denied(home.run(TASK_UPDATE, &payload, None));
    assert!(reason.contains("yaadgaar"), "{reason}");
    assert!(reason.contains("not made"), "{reason}");
    assert!(matches!(
        home.run(TASK_COMPLETED, &home.completed("4"), None),
        Decision::Deny(_)
    ));
    // A file with no URN still passes through: it is not a mirror.
    let foreign = json!({"id":"5","subject":"mine","description":"","status":"pending","blocks":[],"blockedBy":[]});
    home.write_file(5, &foreign);
    let mine = home.update(json!({"taskId": "5", "status": "deleted"}));
    assert_eq!(home.run(TASK_UPDATE, &mine, None), Decision::Allow);

    // A malformed harness.json.
    let home = Home::mirrored("wb-bad-settings", "open");
    home.harness_json(r#"{"scop": "none"}"#);
    let payload = home.update(json!({"taskId": "4", "status": "deleted"}));
    let reason = denied(home.run(TASK_UPDATE, &payload, None));
    assert!(reason.contains("harness.json"), "{reason}");
    assert!(home.list().join("4.json").exists());
}

#[test]
fn a_state_for_another_config_dir_passes_the_task_through() {
    let home = Home::mirrored("wb-other-dir", "open");
    let sdir = state::state_dir(&home.env()).unwrap();
    let mut st = home.state();
    st.dir = home.root.join("other-claude/tasks/s-1");
    state::save(&state::path(&sdir, "s-1"), &st).unwrap();
    let payload = home.update(json!({"taskId": "4", "status": "deleted"}));
    assert_eq!(home.run(TASK_UPDATE, &payload, None), Decision::Allow);
}

#[test]
fn a_failed_refresh_after_a_partial_write_leaves_the_version_stale_so_the_next_sync_rewrites() {
    let home = Home::mirrored("wb-partial", "open");
    let payload = home.update(json!({"taskId": "4", "subject": "New", "status": "in_progress"}));
    let (decision, requests) = home.converse(
        TASK_UPDATE,
        &payload,
        vec![
            ok(json!({"id": URN, "version": 4})),
            refused("illegal move"),
            "not json at all",
        ],
    );
    let reason = denied(decision);
    assert!(reason.contains("could not be refreshed"), "{reason}");
    assert_eq!(args(&requests[2]).0, "read_task");
    assert_eq!(
        home.entry().version,
        3,
        "the state must not claim version 4"
    );
    // The agent edits the file past the hooks. yaadgaar holds version 4, so
    // the next pass sees yaadgaar ahead of the state and rewrites the file.
    let mut edited = home.file(4);
    edited["subject"] = json!("#7 Something local");
    home.write_file(4, &edited);
    let tasks = [sync::Task {
        urn: Some(URN.into()),
        number: 7,
        title: "New".into(),
        status: "open".into(),
        version: Some(4),
    }];
    let input = sync::Input {
        project: "sentinel-org/no-such-repo",
        tasks: &tasks,
        complete: true,
        scope: crate::harness::scope::Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
        closed: None,
    };
    let listing =
        crate::harness::shape::check_dir(&home.list(), &crate::harness::shape::recorded().unwrap())
            .unwrap();
    let out = sync::run(&home.list(), "s-1", &listing, &home.env(), &input).unwrap();
    assert_eq!(out.refreshed, 1, "{out:?}");
    assert_eq!(home.file(4)["subject"], "#7 New");
}

//! `yaadgaar hook session-start`, read off the built binary (ADR-0992).
//!
//! The session must start whatever happens: every path here exits 0 and
//! prints ONE `hookSpecificOutput` object on stdout telling the agent the tasks
//! could not be loaded and that it must ask the user. The credential never
//! appears on stdout or stderr.
//!
//! `#![cfg(unix)]` for `tests/cli.rs`'s reason: `HOME` only redirects the home
//! directory on Unix. Nothing here writes into it, but a test that LOOKS
//! hermetic on Windows and is not is worse than one that does not run there.

#![cfg(unix)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const SENTINEL_TOKEN: &str = "sentinel-token-must-never-print";

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "yaadgaar-session-start-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn hook(config_dir: &Path, payload: &str, envs: &[(&str, &str)]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(["hook", "session-start"])
        .env("HOME", config_dir)
        .env("YAADGAAR_CONFIG_DIR", config_dir)
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
    child.wait_with_output().unwrap()
}

/// The additional context, after checking the envelope and the exit status.
fn context(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: Value = serde_json::from_str(stdout.trim()).expect("one JSON object");
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let text = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("could not be loaded"), "{text}");
    assert!(text.contains("Ask the user"), "{text}");
    assert!(text.contains("Do not record tasks in any other ledger"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stdout.contains(SENTINEL_TOKEN), "{stdout}");
    assert!(!stderr.contains(SENTINEL_TOKEN), "{stderr}");
    text
}

#[test]
fn no_saved_login_starts_the_session_with_a_notice() {
    let dir = scratch("no-login");
    let text = context(&hook(&dir, "{}", &[]));
    assert!(text.contains("not logged in"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unreadable_payload_still_starts_the_session_with_a_notice() {
    let dir = scratch("bad-payload");
    context(&hook(&dir, "not json", &[]));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unreachable_gateway_starts_the_session_with_a_notice_and_no_token() {
    // Port 1 passes the https check at load and is refused at connect.
    let dir = scratch("unreachable");
    std::fs::write(
        dir.join("config.json"),
        format!(r#"{{"gateway":"https://127.0.0.1:1/","token":"{SENTINEL_TOKEN}"}}"#),
    )
    .unwrap();
    let work = dir.join("work");
    std::fs::create_dir_all(work.join(".yadgar")).unwrap();
    std::fs::write(work.join(".yadgar/project-id"), "sentinel-org/no-such-repo").unwrap();
    let payload = format!(r#"{{"cwd":"{}"}}"#, work.display());

    let text = context(&hook(&dir, &payload, &[]));

    assert!(text.contains("could not be reached"), "{text}");
    assert!(
        text.starts_with("Tasks for sentinel-org/no-such-repo live in yaadgaar"),
        "{text}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Compiled in only under `debug_assertions`; see `tests/hook_exit.rs`.
#[cfg(debug_assertions)]
#[test]
fn a_panic_in_setup_starts_the_session_with_a_notice_not_a_refusal() {
    let dir = scratch("panic");
    let out = hook(&dir, "{}", &[("YAADGAAR_TEST_PANIC_IN_SETUP", "1")]);
    let text = context(&out);
    assert!(text.contains("planted panic in the hook's setup"), "{text}");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("panicked at"),
        "{out:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

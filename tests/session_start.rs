//! `yaadgaar hook session-start`, read off the built binary (ADR-0992).
//!
//! The session must start whatever happens: every path here exits 0. Inside a
//! project, a failure prints ONE `hookSpecificOutput` object on stdout with the
//! neutral notice; a directory that names no project prints nothing at all.
//! The credential never appears on stdout or stderr.
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

/// A directory under *dir* whose `.yadgar/project-id` names a project.
fn project_dir(dir: &Path) -> PathBuf {
    let work = dir.join("work");
    std::fs::create_dir_all(work.join(".yadgar")).unwrap();
    std::fs::write(work.join(".yadgar/project-id"), "sentinel-org/no-such-repo").unwrap();
    work
}

/// The hook, run from *cwd* — the PROCESS's directory, which a payload without
/// a `cwd` falls back to. Never this repository's: it names a project.
fn hook(config_dir: &Path, cwd: &Path, payload: &str, envs: &[(&str, &str)]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(["hook", "session-start"])
        .current_dir(cwd)
        .env("HOME", config_dir)
        .env("YAADGAAR_CONFIG_DIR", config_dir)
        // The registry record and the sync state follow these; unset, they
        // land under the scratch HOME rather than the developer's own.
        .env_remove("XDG_STATE_HOME")
        .env_remove("YAADGAAR_STATE_DIR")
        .env_remove("CLAUDE_CONFIG_DIR")
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
    assert!(
        text.starts_with("Open tasks could not be loaded from yaadgaar: "),
        "{text}"
    );
    assert!(
        text.ends_with(
            "If this project's tasks are tracked in yaadgaar, ask the user before \
             tracking work; do not move them to another ledger."
        ),
        "{text}"
    );
    assert!(!text.contains("Tasks for"), "{text}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stdout.contains(SENTINEL_TOKEN), "{stdout}");
    assert!(!stderr.contains(SENTINEL_TOKEN), "{stderr}");
    text
}

#[test]
fn no_saved_login_starts_the_session_with_a_notice() {
    let dir = scratch("no-login");
    let work = project_dir(&dir);
    let text = context(&hook(&dir, &work, "{}", &[]));
    assert!(text.contains("not logged in"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unreadable_payload_still_starts_the_session_with_a_notice() {
    let dir = scratch("bad-payload");
    let work = project_dir(&dir);
    context(&hook(&dir, &work, "not json", &[]));
    std::fs::remove_dir_all(&dir).ok();
}

/// Out of scope: no project, so not one byte on stdout — not even a newline —
/// logged in or not.
#[test]
fn a_directory_naming_no_project_prints_nothing() {
    let dir = scratch("no-project");
    let payload = format!(r#"{{"cwd":"{}"}}"#, dir.display());
    for logged_in in [false, true] {
        if logged_in {
            std::fs::write(
                dir.join("config.json"),
                format!(r#"{{"gateway":"https://127.0.0.1:1/","token":"{SENTINEL_TOKEN}"}}"#),
            )
            .unwrap();
        }
        let out = hook(&dir, &dir, &payload, &[]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(out.stdout.is_empty(), "logged in {logged_in}: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains(SENTINEL_TOKEN), "{stderr}");
    }
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
    let work = project_dir(&dir);
    let payload = format!(r#"{{"cwd":"{}"}}"#, work.display());

    let text = context(&hook(&dir, &dir, &payload, &[]));

    assert!(text.contains("the gateway could not be reached"), "{text}");
    assert!(!text.contains("https://"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Compiled in only under `debug_assertions`; see `tests/hook_exit.rs`.
#[cfg(debug_assertions)]
#[test]
fn a_panic_in_setup_starts_the_session_with_a_notice_not_a_refusal() {
    let dir = scratch("panic");
    let work = project_dir(&dir);
    let out = hook(&dir, &work, "{}", &[("YAADGAAR_TEST_PANIC_IN_SETUP", "1")]);
    let text = context(&out);
    assert!(text.contains("planted panic in the hook's setup"), "{text}");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("panicked at"),
        "{out:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

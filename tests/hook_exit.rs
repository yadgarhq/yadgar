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

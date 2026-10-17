//! The built binary beside the yadgar memory engine: the guard's exceptions file,
//! stale hook entries from 0.1.4, and the engine's `~/.config/yadgar`.
//!
//! Process tests, for the reason `cli.rs` gives: only the process proves that
//! `main` reads the right path. `HOME` is a scratch directory and
//! `XDG_CONFIG_HOME` is REMOVED, because on Linux `dirs::config_dir()` prefers
//! it over `$HOME/.config` — a developer with it set would otherwise point
//! these tests at their real config directory.
//!
//! `#![cfg(unix)]` for the same reason as `cli.rs`: on Windows the home
//! directory does not come from `HOME`.

#![cfg(unix)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("yaadgaar-coexist-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).expect("scratch home");
    dir
}

fn yaadgaar(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("YAADGAAR_CONFIG_DIR")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary under test did not run");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn bash_payload(command: &str) -> String {
    serde_json::json!({"tool_name": "Bash", "tool_input": {"command": command}}).to_string()
}

const BYPASS: &str = "git commit --no-verify -m x";

fn exceptions(home: &Path) -> PathBuf {
    home.join(".claude").join("yadgar-hook-exceptions.json")
}

#[test]
fn the_guard_reads_disabled_guards_from_the_home_it_runs_in() {
    let home = scratch("guard-disabled");
    std::fs::write(
        exceptions(&home),
        r#"{"version":1,"push_default_allowlist":[],"disabled_guards":["git_commit_flags"]}"#,
    )
    .unwrap();
    let before = std::fs::read(exceptions(&home)).unwrap();

    let out = yaadgaar(&home, &["hook", "pre-tool-guard"], &bash_payload(BYPASS));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");

    // Still read-only.
    assert_eq!(std::fs::read(exceptions(&home)).unwrap(), before);
}

#[test]
fn with_no_exceptions_file_the_guard_refuses_and_says_nothing_on_stderr() {
    let home = scratch("guard-absent");
    let out = yaadgaar(&home, &["hook", "pre-tool-guard"], &bash_payload(BYPASS));
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("\"deny\""),
        "{out:?}"
    );
    // Missing is the normal state on a machine without the engine: no warning.
    assert!(out.stderr.is_empty(), "{out:?}");
    assert!(!exceptions(&home).exists(), "the guard created the file");
}

#[test]
fn a_malformed_exceptions_file_keeps_the_guard_on_and_logs_a_notice() {
    let home = scratch("guard-malformed");
    std::fs::write(
        exceptions(&home),
        r#"{"disabled_guards":["git_commit_flags""#,
    )
    .unwrap();
    let out = yaadgaar(&home, &["hook", "pre-tool-guard"], &bash_payload(BYPASS));
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("yadgar-hook-exceptions.json") && stderr.contains("every guard stays on"),
        "{stderr}"
    );
    // The notice is on stderr only; stdout carries exactly the refusal.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_ok(),
        "{stdout}"
    );
}

#[test]
fn a_hook_entry_left_by_an_older_install_exits_cleanly_and_silently() {
    // 0.1.4 registered eleven more handlers. Until the next `install` strips
    // them they keep firing, and each must be a silent no-op.
    let home = scratch("stale-entry");
    for name in ["post-tool-capture", "stop-checkpoint", "block-reflect"] {
        let out = yaadgaar(&home, &["hook", name], &bash_payload(BYPASS));
        assert_eq!(out.status.code(), Some(0), "{name}: {out:?}");
        assert!(out.stdout.is_empty(), "{name}: {out:?}");
    }
}

/// The engine's files in `~/.config/yadgar`, plus this client's 0.1.4 config.
fn seed_old_config(home: &Path) -> PathBuf {
    let legacy = home.join(".config").join("yadgar");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("secrets.env"), "YADGAR_MCP_AUTH_TOKEN=fake\n").unwrap();
    std::fs::write(legacy.join("config.yaml"), "engine: true\n").unwrap();
    std::fs::write(
        legacy.join("config.json"),
        r#"{"gateway":"https://gw.example","token":"fake"}"#,
    )
    .unwrap();
    legacy
}

#[cfg(unix)]
fn stamps(dir: &Path) -> Vec<(PathBuf, Vec<u8>, u64, u32, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.push(dir.to_path_buf());
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let m = std::fs::metadata(&p).unwrap();
            let bytes = if m.is_file() {
                std::fs::read(&p).unwrap()
            } else {
                Vec::new()
            };
            (p, bytes, m.ino(), m.mode(), m.mtime(), m.mtime_nsec())
        })
        .collect()
}

#[test]
fn verify_hints_at_an_old_config_and_leaves_the_engines_directory_alone() {
    let home = scratch("old-config-verify");
    let legacy = seed_old_config(&home);
    let before = stamps(&legacy);

    let out = yaadgaar(&home, &["verify"], "");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(&legacy.join("config.json").display().to_string())
            && said.contains("yaadgaar login"),
        "{said}"
    );
    assert_eq!(stamps(&legacy), before);
    assert!(!home.join(".config").join("yaadgaar").exists());
}

#[test]
fn serve_with_only_an_old_config_names_it_on_stderr_and_not_on_stdout() {
    // stdout is the MCP transport under `serve`; a hint there is a frame the
    // agent cannot parse.
    let home = scratch("old-config-serve");
    let legacy = seed_old_config(&home);
    let before = stamps(&legacy);

    let out = yaadgaar(&home, &["serve"], "");
    assert!(!out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("older yaadgaar"), "{stderr}");
    assert!(stderr.contains("not logged in"), "{stderr}");
    assert_eq!(stamps(&legacy), before);
}

#[test]
fn no_hint_when_the_engines_directory_holds_only_engine_files() {
    let home = scratch("engine-only");
    let legacy = seed_old_config(&home);
    std::fs::remove_file(legacy.join("config.json")).unwrap();
    let out = yaadgaar(&home, &["verify"], "");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("older yaadgaar"),
        "{out:?}"
    );
}

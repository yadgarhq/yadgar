//! `yaadgaar harness check-shape`, read off the built binary.
//!
//! The canary (`make canary`) runs a real `claude`, then hands the list
//! directory it wrote to this subcommand. So the exit status IS the canary's
//! verdict on the file shape, and it is pinned here against the real process
//! rather than only through the unit tests of the check behind it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn check(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(["harness", "check-shape"])
        .arg(dir)
        .output()
        .expect("the built binary runs")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("yaadgaar-check-shape-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn the_directory_a_real_claude_code_run_wrote_exits_0_and_says_what_it_holds() {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-code/list-2.1.296");
    let out = check(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert!(stdout.contains("1 task"), "{stdout}");
    assert!(stdout.contains("highwatermark 2"), "{stdout}");
}

#[test]
fn a_directory_off_the_recorded_shape_exits_non_zero_and_names_the_entry() {
    let dir = scratch("bad");
    std::fs::write(
        dir.join("1.json"),
        r#"{"id":"1","subject":"s","description":"d","status":"pending","blocks":[],"blockedBy":[],"priority":1}"#,
    )
    .unwrap();
    let out = check(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr.contains("1.json") && stderr.contains("priority"),
        "{stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_directory_that_does_not_exist_exits_non_zero() {
    let dir = scratch("absent").join("nothing-here");
    let out = check(&dir);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!out.stderr.is_empty(), "{out:?}");
}

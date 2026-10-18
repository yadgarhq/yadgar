//! What the exceptions file can switch off, and what it never can.

use super::*;

const ALL_OFF: Policy = Policy {
    terraform_family: false,
    git_commit_flags: false,
    hook_config_tamper: false,
};

#[test]
fn each_shared_id_switches_off_its_own_guard_and_no_other() {
    for (id, expected) in [
        (
            "terraform_family",
            Policy {
                terraform_family: false,
                ..Policy::ALL_ON
            },
        ),
        (
            "git_commit_flags",
            Policy {
                git_commit_flags: false,
                ..Policy::ALL_ON
            },
        ),
        (
            "hook_config_tamper",
            Policy {
                hook_config_tamper: false,
                ..Policy::ALL_ON
            },
        ),
    ] {
        let text = format!(r#"{{"version":1,"disabled_guards":["{id}"]}}"#);
        assert_eq!(Policy::parse(&text), Ok(expected), "{id}");
    }
}

#[test]
fn all_three_ids_together_switch_off_all_three() {
    let text =
        r#"{"disabled_guards":["hook_config_tamper","terraform_family","git_commit_flags"]}"#;
    assert_eq!(Policy::parse(text), Ok(ALL_OFF));
}

#[test]
fn ids_this_client_does_not_implement_change_nothing() {
    // The engine's own two other ids, a misspelling and a different case.
    // None of them names a refusal this guard makes. (A non-string item makes
    // the whole list malformed; see the all-or-nothing test.)
    let text = r#"{"disabled_guards":["git_push_default","db_lockdown","terraform","Terraform_Family","TERRAFORM_FAMILY"," git_commit_flags"]}"#;
    assert_eq!(Policy::parse(text), Ok(Policy::ALL_ON));
}

#[test]
fn a_file_the_engine_seeded_disables_nothing() {
    // What the engine writes on a fresh machine.
    let text =
        r#"{"version":1,"push_default_allowlist":["nix","ledger","ostad"],"disabled_guards":[]}"#;
    assert_eq!(Policy::parse(text), Ok(Policy::ALL_ON));
    // And a file that does not mention the key at all.
    assert_eq!(Policy::parse(r#"{"version":1}"#), Ok(Policy::ALL_ON));
}

#[test]
fn malformed_text_is_an_error_and_never_a_partial_policy() {
    for text in [
        "",
        "{",
        r#"{"disabled_guards":["terraform_family""#,
        r#"["terraform_family"]"#,
        r#""terraform_family""#,
        "null",
        // A bare string is not a one-element list.
        r#"{"disabled_guards":"terraform_family"}"#,
        r#"{"disabled_guards":{"terraform_family":true}}"#,
        r#"{"disabled_guards":null}"#,
    ] {
        assert!(Policy::parse(text).is_err(), "{text:?} parsed");
    }
}

#[test]
fn a_file_that_cannot_be_used_leaves_every_guard_on() {
    let dir = scratch("unusable");
    let missing = dir.join("absent.json");
    assert_eq!(Policy::load(Some(&missing)), Policy::ALL_ON);
    assert_eq!(Policy::load(None), Policy::ALL_ON);

    let bad = dir.join(EXCEPTIONS_FILE);
    std::fs::write(&bad, r#"{"disabled_guards":["terraform_family""#).unwrap();
    assert_eq!(Policy::load(Some(&bad)), Policy::ALL_ON);

    // A directory where the file should be: unreadable as a file.
    let a_dir = dir.join("is-a-directory");
    std::fs::create_dir_all(&a_dir).unwrap();
    assert_eq!(Policy::load(Some(&a_dir)), Policy::ALL_ON);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_readable_file_is_honoured_and_never_written() {
    let dir = scratch("honoured");
    let path = dir.join(EXCEPTIONS_FILE);
    let body = r#"{"disabled_guards":["git_commit_flags"]}"#;
    std::fs::write(&path, body).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    assert_eq!(
        Policy::load(Some(&path)),
        Policy {
            git_commit_flags: false,
            ..Policy::ALL_ON
        }
    );

    assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `Policy::load` on *path*, or `None` if it did not return within 5 seconds.
///
/// A hang IS the defect: Claude Code times a hook out and a timed-out hook is
/// not a deny, so every guarded call would then run unguarded.
fn load_within_timeout(path: std::path::PathBuf) -> Option<Policy> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Policy::load(Some(&path)));
    });
    rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
}

#[cfg(unix)]
#[test]
fn a_fifo_in_place_of_the_file_returns_at_once_with_every_guard_on() {
    let dir = scratch("fifo");
    let path = dir.join(EXCEPTIONS_FILE);
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo failed");
    assert_eq!(
        load_within_timeout(path),
        Some(Policy::ALL_ON),
        "the guard hung on a FIFO, or switched a guard off"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_symlink_to_an_endless_device_returns_at_once_with_every_guard_on() {
    let dir = scratch("dev-zero");
    let path = dir.join(EXCEPTIONS_FILE);
    std::os::unix::fs::symlink("/dev/zero", &path).unwrap();
    assert_eq!(
        load_within_timeout(path),
        Some(Policy::ALL_ON),
        "the guard hung on /dev/zero, or switched a guard off"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_file_over_the_size_cap_is_malformed_even_when_it_would_parse() {
    // Valid JSON disabling a guard, padded past 64 KiB with whitespace. Over
    // the cap is malformed, so the padding cannot buy a switched-off guard.
    let dir = scratch("oversize");
    let path = dir.join(EXCEPTIONS_FILE);
    let body = format!(
        "{}{{\"disabled_guards\":[\"git_commit_flags\"]}}",
        " ".repeat(64 * 1024)
    );
    std::fs::write(&path, body).unwrap();
    assert_eq!(load_within_timeout(path.clone()), Some(Policy::ALL_ON));

    // And one just under the cap is still read.
    let fits = format!(
        "{}{{\"disabled_guards\":[\"git_commit_flags\"]}}",
        " ".repeat(1024)
    );
    std::fs::write(&path, fits).unwrap();
    assert_eq!(
        load_within_timeout(path),
        Some(Policy {
            git_commit_flags: false,
            ..Policy::ALL_ON
        })
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_push_allowlist_that_is_not_a_list_of_strings_voids_the_whole_file() {
    // Parity with the engine's router (m-agahi/yadgar#77): this client does
    // not use the key, but a file the engine ignores must not switch anything
    // off here either.
    for text in [
        r#"{"disabled_guards":["git_commit_flags"],"push_default_allowlist":"x"}"#,
        r#"{"disabled_guards":["git_commit_flags"],"push_default_allowlist":[1]}"#,
    ] {
        assert!(Policy::parse(text).is_err(), "{text}");
    }
    let fine = r#"{"disabled_guards":["git_commit_flags"],"push_default_allowlist":["main"]}"#;
    assert!(!Policy::parse(fine).unwrap().git_commit_flags);
}

#[test]
fn a_list_with_any_entry_that_is_not_a_string_is_malformed_as_a_whole() {
    // All-or-nothing, as the engine's router reads it (m-agahi/yadgar#77):
    // `["git_commit_flags", 1]` used to switch git_commit_flags off and skip
    // the 1. A list the engine ignores must not switch anything off here.
    for text in [
        r#"{"disabled_guards":["git_commit_flags",1]}"#,
        r#"{"disabled_guards":["git_commit_flags",null]}"#,
        r#"{"disabled_guards":[["git_commit_flags"]]}"#,
        r#"{"disabled_guards":["git_commit_flags",{"id":"x"}]}"#,
    ] {
        assert!(Policy::parse(text).is_err(), "{text}");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_in_place_of_the_file_is_refused_even_to_a_valid_file() {
    // O_NOFOLLOW, as the engine opens it: the two guards read one file and
    // must reach one decision, and the engine ignores a symlinked file.
    let dir = scratch("symlink-valid");
    let real = dir.join("real.json");
    std::fs::write(&real, r#"{"disabled_guards":["git_commit_flags"]}"#).unwrap();
    let link = dir.join(EXCEPTIONS_FILE);
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(Policy::load(Some(&link)), Policy::ALL_ON);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_file_name_is_the_one_the_engine_reads() {
    assert_eq!(EXCEPTIONS_FILE, "yadgar-hook-exceptions.json");
}

fn scratch(name: &str) -> std::path::PathBuf {
    crate::testserver::scratch_dir(&format!("policy-{name}"))
}

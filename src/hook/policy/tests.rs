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
    // The engine's own two other ids, a misspelling, a different case and a
    // non-string item. None of them names a refusal this guard makes.
    let text = r#"{"disabled_guards":["git_push_default","db_lockdown","terraform","Terraform_Family","TERRAFORM_FAMILY"," git_commit_flags",7,null,{"id":"terraform_family"}]}"#;
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

#[test]
fn the_file_name_is_the_one_the_engine_reads() {
    assert_eq!(EXCEPTIONS_FILE, "yadgar-hook-exceptions.json");
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "yaadgaar-policy-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

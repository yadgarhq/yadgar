//! The refusal matrix, both directions, keyed on independent literals.

use super::*;
use serde_json::json;
fn bash_call(command: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": command}})
}

fn denies(command: &str) -> bool {
    matches!(pre_tool_guard(&bash_call(command)), Decision::Deny(_))
}

/// Refused today, and each one is something a person is entitled to do.
///
/// Keyed on the command, not on which rule over-matched: the substring
/// matcher this replaces refused all three, and one of them is the remedy
/// the standing rule itself prescribes.
const MUST_BE_ALLOWED: &[&str] = &[
    // A read of the exceptions file. Reading a human's decisions is how an
    // agent honours them.
    "cat ~/.claude/yadgar-hook-exceptions.json",
    // Searching the documentation for the word.
    "grep -r 'terraform ' docs/",
    "cat docs/terraform-notes.md",
    "rg --files-with-matches 'tofu' .",
    // Ordinary work.
    "cargo test",
    "git commit -m 'a change'",
    "git log -n 5",
    // `-n` is `--dry-run` for push, and refusing it would be the guard
    // reading a letter without reading the subcommand it belongs to.
    "git push -n origin main",
    // `-m` takes the rest of the token as its value, so this is a commit
    // message that happens to be the letter n.
    "git commit -mn x",
    "git commit -am 'a change'",
    // Named after the word, and running none of it.
    "ls terraforming/",
    "gh pr comment 12 --body 'see the digger notes'",
    "grep digger docs/",
    // Naming the command without posting it. Nothing here reaches a forge,
    // and refusing this refuses reading the rule that forbids it.
    "grep 'digger apply' docs/",
    "cat MIGRATION_NOTES.md",
];

/// Allowed today, and each one does the thing the rules forbid.
const MUST_BE_DENIED: &[&str] = &[
    // `-n` IS `--no-verify` for commit.
    "git commit -n -m x",
    "git commit -nm x",
    "git commit --no-verify -m x",
    "git push --no-verify",
    // A config override for one invocation: nothing in the repository
    // records that the checks were turned off.
    "git -c core.hooksPath=/dev/null commit",
    "git -c commit.gpgsign=false commit -m x",
    // A separator away from the start of the line.
    "cd infra && terraform apply",
    "cd infra; terraform",
    // No trailing space, which is what the substring "terraform " needed.
    "terraform",
    // The plan container, and the wrappers that reach the same binary.
    "docker exec tf-local-plan tofu",
    "podman exec tf-local-plan-7 tfp plan",
    "docker run hashicorp/terraform plan",
    "nix run nixpkgs#terraform -- apply",
    "sudo terraform destroy",
    "TF_LOG=trace terraform apply",
    "sh -c 'terraform apply'",
    // The rule names five verbs; the guard listed three.
    r#"gh pr comment 12 --body "digger unlock""#,
    r#"gh pr comment 12 --body "digger lock""#,
    r#"gh pr comment 12 --body "digger apply""#,
    "digger plan",
    // Writing the exceptions file, as opposed to reading it.
    "echo '{}' > ~/.claude/yadgar-hook-exceptions.json",
    "tee ~/.claude/yadgar-hook-exceptions.json",
];

#[test]
fn everything_a_person_is_entitled_to_do_is_allowed() {
    for command in MUST_BE_ALLOWED {
        assert!(
            !denies(command),
            "refused a command that must be allowed: {command}"
        );
    }
}

#[test]
fn everything_the_rules_forbid_is_refused() {
    for command in MUST_BE_DENIED {
        assert!(
            denies(command),
            "allowed a command that must be refused: {command}"
        );
    }
}

#[test]
fn the_two_directions_are_pinned_on_separate_inputs() {
    // Neither list may be satisfied by answering everything the same way,
    // and neither list may quietly become empty.
    assert!(MUST_BE_ALLOWED.len() >= 10 && MUST_BE_DENIED.len() >= 10);
    for command in MUST_BE_ALLOWED {
        assert!(!MUST_BE_DENIED.contains(command), "{command} is in both");
    }
}

#[test]
fn writing_a_digger_command_into_a_file_is_the_remedy_and_not_the_offence() {
    // THE ONE THAT MATTERS MOST. The standing rule says: refuse to post the
    // command, write it into MIGRATION_NOTES.md, and hand it to a person.
    // The guard refused exactly that, so an agent following the rule tripped
    // the hook that enforces it.
    let write = json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/x/repo/MIGRATION_NOTES.md",
            "content": "Post this on the pull request yourself:\n\n    digger apply\n"
        }
    });
    assert_eq!(pre_tool_guard(&write), Decision::Allow);
}

#[test]
fn an_edit_is_judged_on_its_path_and_not_on_its_text() {
    // An edit executes nothing. Reading its content is what made the guard
    // refuse the case above.
    let edit = json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "/home/x/repo/runbook.md",
            "new_string": "run `terraform apply` on the bastion, then `digger unlock`"
        }
    });
    assert_eq!(pre_tool_guard(&edit), Decision::Allow);

    // And the same for the exceptions file BY NAME. Only the path decides;
    // a note that mentions the file is a note.
    let note = json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/x/repo/notes.md",
            "content": "the allowlist lives in ~/.claude/yadgar-hook-exceptions.json"
        }
    });
    assert_eq!(pre_tool_guard(&note), Decision::Allow);
}

#[test]
fn the_exceptions_file_is_protected_from_edit_not_just_bash() {
    // The incident: an agent used Edit, not Bash, to add itself to the
    // allowlist, pushed to a protected branch, then reverted the file to
    // conceal it — and a Bash-only matcher never routed the call here.
    for tool in ["Edit", "Write", "NotebookEdit"] {
        let call = json!({
            "tool_name": tool,
            "tool_input": {
                "file_path": "/home/x/.claude/yadgar-hook-exceptions.json",
                "new_string": "{\"push_default_allowlist\": [\"everything\"]}"
            }
        });
        assert!(
            matches!(pre_tool_guard(&call), Decision::Deny(_)),
            "{tool} was allowed to write the exceptions file"
        );
    }
}

#[test]
fn an_unreadable_payload_allows_rather_than_denying() {
    // Fail OPEN. A guard that denied on a payload it could not parse would
    // block the session on its own bug.
    assert_eq!(pre_tool_guard(&Value::Null), Decision::Allow);
    assert_eq!(
        pre_tool_guard(&json!({"tool_name": "Bash"})),
        Decision::Allow
    );
}

#[test]
fn a_tool_the_guard_does_not_cover_is_allowed() {
    let read = json!({"tool_name": "Read", "tool_input": {"file_path": "/etc/passwd"}});
    assert_eq!(pre_tool_guard(&read), Decision::Allow);
}

#[test]
fn a_nested_script_is_followed_only_so_far() {
    // Followed, because `sh -c 'terraform apply'` runs terraform. Bounded,
    // because a payload can nest as deeply as it likes and a guard that
    // recurses on it hangs the session it exists to protect.
    assert!(denies(r#"sh -c "sh -c 'terraform apply'""#));
    let deep = r#"sh -c "sh -c \"sh -c 'terraform apply'\"""#;
    assert!(!denies(deep), "the recursion is unbounded");
}

#[test]
fn what_a_command_names_is_not_what_it_runs() {
    // `names_terraform` splits on the separators an image reference, a flake
    // reference and a path use — so the binary is matched and the prose is
    // not.
    assert!(names_terraform("hashicorp/terraform:1.5"));
    assert!(names_terraform("nixpkgs#terraform"));
    assert!(!names_terraform("docs/terraform-notes.md"));
    assert!(!names_terraform("terraforming"));
}

#[test]
fn a_short_option_cluster_is_read_up_to_its_first_value() {
    // The pair that makes the scan falsifiable: without both halves, a scan
    // that returned `true` for any cluster containing an `n` would pass.
    assert!(carries_no_verify("-n"));
    assert!(carries_no_verify("-an"));
    assert!(carries_no_verify("-nm"));
    assert!(!carries_no_verify("-mn"));
    assert!(!carries_no_verify("-am"));
    assert!(!carries_no_verify("--no-edit"));
}

#[test]
fn a_wrapper_does_not_change_what_runs() {
    for line in [
        "sudo -u ops terraform apply",
        "env -i terraform apply",
        "nohup terraform apply",
        "/usr/local/bin/terraform apply",
    ] {
        assert!(denies(line), "a wrapper hid terraform: {line}");
    }
}

#[test]
fn a_refusal_says_what_to_do_instead() {
    // A guard that only says no is a guard somebody switches off. Every
    // reason names either the remedy or the reason the rule exists.
    for reason in [HOOK_BYPASS, TERRAFORM, DIGGER, EXCEPTIONS] {
        assert!(reason.len() > 60, "a one-line refusal explains nothing");
    }
}

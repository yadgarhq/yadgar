//! A guard a human switched off in the exceptions file allows; the others still refuse.

use super::*;

fn with(policy: Policy, call: &Value) -> Decision {
    pre_tool_guard(call, &policy)
}

fn edit_of_the_exceptions_file(tool: &str) -> Value {
    json!({
        "tool_name": tool,
        "tool_input": {
            "file_path": "/home/x/.claude/yadgar-hook-exceptions.json",
            "new_string": "{}"
        }
    })
}

const TERRAFORM_CALLS: &[&str] = &[
    "terraform plan",
    "cd infra && tofu apply",
    "docker run hashicorp/terraform:1.5 plan",
    "sh -c 'terraform apply'",
    // Digger-on-PR is filed under terraform_family by the engine too.
    r#"gh pr comment 12 --body "digger apply""#,
];

const HOOK_BYPASS_CALLS: &[&str] = &[
    "git commit --no-verify -m x",
    "git commit -nm x",
    "git -c core.hooksPath=/dev/null commit -m x",
];

const TAMPER_CALLS: &[&str] = &[
    "tee ~/.claude/yadgar-hook-exceptions.json",
    "echo '{}' > ~/.claude/yadgar-hook-exceptions.json",
];

#[test]
fn terraform_family_switched_off_allows_terraform_and_digger_and_nothing_else() {
    let policy = Policy {
        terraform_family: false,
        ..Policy::ALL_ON
    };
    for cmd in TERRAFORM_CALLS {
        assert_eq!(with(policy, &bash_call(cmd)), Decision::Allow, "{cmd}");
    }
    for cmd in HOOK_BYPASS_CALLS.iter().chain(TAMPER_CALLS) {
        assert!(
            matches!(with(policy, &bash_call(cmd)), Decision::Deny(_)),
            "{cmd}"
        );
    }
    assert!(matches!(
        with(policy, &edit_of_the_exceptions_file("Edit")),
        Decision::Deny(_)
    ));
}

#[test]
fn git_commit_flags_switched_off_allows_a_hook_bypass_and_nothing_else() {
    let policy = Policy {
        git_commit_flags: false,
        ..Policy::ALL_ON
    };
    for cmd in HOOK_BYPASS_CALLS {
        assert_eq!(with(policy, &bash_call(cmd)), Decision::Allow, "{cmd}");
    }
    for cmd in TERRAFORM_CALLS.iter().chain(TAMPER_CALLS) {
        assert!(
            matches!(with(policy, &bash_call(cmd)), Decision::Deny(_)),
            "{cmd}"
        );
    }
}

#[test]
fn hook_config_tamper_switched_off_allows_both_routes_to_the_file_and_nothing_else() {
    // BOTH routes: the Bash writers AND Edit/Write/NotebookEdit on the path.
    // The engine switches off both halves of its G5 with this one id.
    let policy = Policy {
        hook_config_tamper: false,
        ..Policy::ALL_ON
    };
    for cmd in TAMPER_CALLS {
        assert_eq!(with(policy, &bash_call(cmd)), Decision::Allow, "{cmd}");
    }
    for tool in ["Edit", "Write", "NotebookEdit"] {
        assert_eq!(
            with(policy, &edit_of_the_exceptions_file(tool)),
            Decision::Allow,
            "{tool}"
        );
    }
    for cmd in TERRAFORM_CALLS.iter().chain(HOOK_BYPASS_CALLS) {
        assert!(
            matches!(with(policy, &bash_call(cmd)), Decision::Deny(_)),
            "{cmd}"
        );
    }
}

#[test]
fn a_policy_parsed_from_unknown_ids_still_refuses_everything() {
    let policy = Policy::parse(r#"{"disabled_guards":["git_push_default","db_lockdown"]}"#)
        .expect("valid file");
    for cmd in TERRAFORM_CALLS
        .iter()
        .chain(HOOK_BYPASS_CALLS)
        .chain(TAMPER_CALLS)
    {
        assert!(
            matches!(with(policy, &bash_call(cmd)), Decision::Deny(_)),
            "{cmd}"
        );
    }
}

#[test]
fn a_nested_script_obeys_the_same_policy() {
    // The `-c` recursion carries the policy, or a switched-off guard would
    // still refuse one level down.
    let off = Policy {
        git_commit_flags: false,
        ..Policy::ALL_ON
    };
    assert_eq!(
        with(off, &bash_call("bash -c 'git commit --no-verify -m x'")),
        Decision::Allow
    );
    assert!(matches!(
        with(off, &bash_call("bash -c 'terraform apply'")),
        Decision::Deny(_)
    ));
}

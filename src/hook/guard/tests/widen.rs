//! The guard's widening under ADR-1005: terraform behind wrappers and three
//! shells deep, MultiEdit, and a refusal past the nesting bound rather than
//! an allowance (yaadgaar #229, #227).

use super::*;

/// Each runs terraform, tofu or tfp. Allowed before ADR-1005: a wrapper with
/// a positional operand (`timeout 5`), a wrapper that reads its command from
/// its operands (`xargs`), an option cluster that hides a value (`-iu ops`),
/// and a third nested shell.
const TERRAFORM_BEHIND_A_WRAPPER: &[&str] = &[
    "timeout 5 terraform apply",
    "timeout -s KILL 5 terraform",
    "timeout --kill-after=3 10 tofu plan",
    "timeout --signal TERM 5s tfp",
    "timeout --foreground 5 terraform plan",
    "xargs terraform",
    "echo plan | xargs terraform",
    "xargs -I{} terraform {}",
    "xargs -I {} terraform {}",
    "xargs -n1 tofu",
    "xargs -0 -P 4 tfp plan",
    "xargs --max-args 1 terraform",
    "xargs sh -c 'terraform apply'",
    "exec -a x terraform",
    "exec terraform apply",
    "env -u FOO terraform",
    "env -i PATH=/bin terraform",
    "nice -n 10 terraform",
    "nice -10 terraform",
    "nice terraform",
    "nohup tofu apply",
    "command terraform",
    "sudo -iu ops terraform apply",
    "sudo --user ops terraform",
    "sudo -- terraform",
    "timeout 5 sudo -u ops xargs terraform",
    // Review of #86: a lone `-` is `env -i`, and main skipped it as an
    // option; `--max-lines` takes its value only after `=`; `-S` may close
    // an option cluster; a shell's `-c` may sit in a cluster or before other
    // options.
    "env - terraform",
    "env -i - terraform plan",
    "sh -c 'env - terraform'",
    "xargs --max-lines terraform",
    "env -iS 'terraform plan'",
    "env -vS'terraform plan'",
    "env -iu X -S 'terraform plan'",
    "bash -lc 'terraform plan'",
    "bash -ec 'terraform plan'",
    "bash -c -e 'terraform plan'",
    // getopt_long accepts any unambiguous prefix of a long option.
    "xargs --max-a 1 terraform",
    "timeout --sig KILL 5 terraform",
    "sudo --us ops terraform",
    "env --un X terraform",
    "env --split 'terraform plan'",
    // Depth 3: the bound was 2, and deeper allowed.
    r#"sh -c "sh -c 'sh -c terraform'""#,
    "eval eval eval terraform",
    r#"echo "$(echo "$(echo "$(terraform)")")""#,
];

#[test]
fn terraform_behind_a_wrapper_or_three_shells_deep_is_refused() {
    let wrong: Vec<(&str, String)> = TERRAFORM_BEHIND_A_WRAPPER
        .iter()
        .filter_map(|c| match guard(&bash_call(c)) {
            Decision::Deny(r) if r == TERRAFORM => None,
            other => Some((*c, format!("{other:?}"))),
        })
        .collect();
    assert!(wrong.is_empty(), "not refused as terraform: {wrong:#?}");
}

#[test]
fn a_wrapper_also_exposes_a_commit_that_skips_hooks() {
    for line in [
        "timeout 60 git commit -n -m x",
        "xargs git commit --no-verify -m",
        "nice -n 5 git commit -nm x",
        "env - git commit --no-verify -m x",
    ] {
        assert_eq!(reason_for(&bash_call(line)), HOOK_BYPASS, "{line}");
    }
}

/// The wrapper runs a harmless program; terraform is only its argument.
const WRAPPED_BUT_HARMLESS: &[&str] = &[
    "timeout 5 cargo test",
    "timeout 30 git log -n 5",
    "xargs grep terraform",
    "git ls-files | xargs -I{} grep -l tofu {}",
    "echo terraform | xargs echo",
    "nice -n 5 make",
    "nohup cargo build",
    "eval eval eval true",
    "env - cargo test",
    "env -iS 'cargo test'",
    "xargs --max-lines echo",
    "bash -lc 'cargo test'",
    "bash -c 'echo $1' x terraform-notes.md",
];

#[test]
fn a_wrapper_around_a_harmless_program_is_allowed() {
    let refused: Vec<&&str> = WRAPPED_BUT_HARMLESS.iter().filter(|c| denies(c)).collect();
    assert!(refused.is_empty(), "refused: {refused:#?}");
}

#[test]
fn past_the_nesting_bound_refuses_rather_than_allows() {
    // ONE LEVEL PAST THE BOUND is refused under its own reason, whatever the
    // innermost command is: what the guard did not read, it cannot allow.
    for line in [
        "eval eval eval eval true",
        "echo $(echo $(echo $(echo $(true))))",
        r#"sh -c "sh -c 'sh -c \"sh -c true\"'""#,
    ] {
        assert_eq!(reason_for(&bash_call(line)), TOO_DEEP, "{line}");
    }
}

#[test]
fn multiedit_is_judged_like_edit() {
    let call = |path: &str| {
        json!({
            "tool_name": "MultiEdit",
            "tool_input": {"file_path": path, "edits": [{"old_string": "a", "new_string": "b"}]}
        })
    };
    assert_eq!(
        reason_for(&call("/home/x/.claude/yadgar-hook-exceptions.json")),
        EXCEPTIONS
    );
    assert_eq!(guard(&call("/home/x/repo/src/main.rs")), Decision::Allow);
    // No path: refused, as for Edit.
    let unplaced = json!({"tool_name": "MultiEdit", "tool_input": {"edits": []}});
    assert_eq!(reason_for(&unplaced), UNPLACED);
}

#[test]
fn every_refusal_names_its_rule_and_adr() {
    for reason in ALL_REASONS {
        assert!(reason.contains("ADR-1005"), "{reason}");
        assert!(reason.contains(" rule"), "{reason}");
    }
    assert!(EXCEPTIONS.contains("ADR-0971"));
}

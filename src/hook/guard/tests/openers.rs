//! A compound command's opener is not its program (ADR-1005): the scanner
//! keeps `do`, `then`, `if`, `!`, `{` as a command's first word, and the
//! base rules used to read that word as the program and skip the rest.

use super::*;

/// Each line runs the forbidden thing inside a loop, a branch or a group.
const BEHIND_AN_OPENER: &[(&str, &str)] = &[
    ("for i in 1; do terraform apply; done", TERRAFORM),
    ("{ terraform apply; }", TERRAFORM),
    ("if true; then terraform apply; fi", TERRAFORM),
    ("if false; then :; else terraform apply; fi", TERRAFORM),
    (
        "if false; then :; elif terraform plan; then :; fi",
        TERRAFORM,
    ),
    ("if terraform plan; then :; fi", TERRAFORM),
    ("while true; do tofu apply; done", TERRAFORM),
    ("until false; do tfp; done", TERRAFORM),
    ("while terraform plan; do :; done", TERRAFORM),
    ("until tofu apply; do :; done", TERRAFORM),
    ("! terraform apply", TERRAFORM),
    ("for i in 1; do sudo terraform apply; done", TERRAFORM),
    (
        "for i in 1; do docker run hashicorp/terraform apply; done",
        TERRAFORM,
    ),
    ("if true; then { terraform apply; }; fi", TERRAFORM),
    (
        "for f in a; do git commit --no-verify -m x; done",
        HOOK_BYPASS,
    ),
    ("if true; then git commit -n -m x; fi", HOOK_BYPASS),
    (
        "{ git -c core.hooksPath=/dev/null commit -m x; }",
        HOOK_BYPASS,
    ),
    ("! git commit --no-gpg-sign -m x", HOOK_BYPASS),
    (
        r#"for i in 1; do gh pr comment 1 --body "digger apply"; done"#,
        DIGGER,
    ),
];

#[test]
fn a_rule_is_not_skipped_behind_a_compound_command_opener() {
    let wrong: Vec<(&str, String)> = BEHIND_AN_OPENER
        .iter()
        .filter_map(|(line, want)| match guard(&bash_call(line)) {
            Decision::Deny(r) if r == *want => None,
            other => Some((*line, format!("{other:?}"))),
        })
        .collect();
    assert!(wrong.is_empty(), "not refused under its rule: {wrong:#?}");
}

/// Loops and branches that run nothing a rule forbids.
const ORDINARY_COMPOUNDS: &[&str] = &[
    "for f in *.tf; do echo $f; done",
    "if [ -f main.tf ]; then cat main.tf; fi",
    "for i in 1 2; do git commit -m x; done",
    "if git diff --quiet; then echo clean; else git status; fi",
    "{ echo do; echo then; }",
    "echo if then else do done",
    "while read l; do echo $l; done < terraform.log",
    "grep -r 'do terraform' docs/",
    "for r in a b; do gh api repos/$r/tags; done",
    "! git diff --quiet",
    "if true; then git log -n 5; fi",
];

#[test]
fn ordinary_loops_and_branches_are_not_refused() {
    let refused: Vec<(&str, String)> = ORDINARY_COMPOUNDS
        .iter()
        .filter_map(|line| match guard(&bash_call(line)) {
            Decision::Allow => None,
            other => Some((*line, format!("{other:?}"))),
        })
        .collect();
    assert!(refused.is_empty(), "over-refused: {refused:#?}");
}

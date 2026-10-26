//! Protected mode under a SAFE sudo verdict (ADR-1028, ruling B on yaadgaar
//! #439 item 5).
//!
//! When a root-written attestation says sudo cannot be reused without a
//! person (`hook/mode/attest.rs`), the shapes refused only because they might
//! hide an escalation are allowed. A line that names an escalator, or the
//! protected path, still refuses. Not SAFE, unknown or unverified: the strict
//! rules, exactly as before. Default mode: unchanged.

use super::retired::{ESCALATION, FAIL_CLOSED, INLINE_CODE, PROTECTED_LITERAL, PS_FAIL_CLOSED};
use super::*;

fn strict() -> GuardEnv {
    GuardEnv {
        exceptions_protected: true,
        ..GuardEnv::default()
    }
}

fn relaxed() -> GuardEnv {
    GuardEnv {
        exceptions_protected: true,
        sudo_safe: true,
        ..GuardEnv::default()
    }
}

fn decide(command: &str, env: &GuardEnv) -> Decision {
    pre_tool_guard(&bash_call(command), &Policy::ALL_ON, env)
}

fn decide_ps(command: &str, env: &GuardEnv) -> Decision {
    let call = json!({"tool_name": "PowerShell", "tool_input": {"command": command}});
    pre_tool_guard(&call, &Policy::ALL_ON, env)
}

/// Each shape ruling B allows under SAFE, one per sub-rule it relaxes.
const RELAXED: &[&str] = &[
    // inline code
    "python3 -c 'print(1)'",
    "sh -c 'ls | wc -l'",
    "bash -c ls",
    "bash -xc 'ls'",
    "node -e 1",
    "perl -e 'print 1'",
    "perl -E 'say 1'",
    "ruby -e 'p 1'",
    "awk '{print $1}' f",
    "gawk 'BEGIN{print 1}'",
    "sh -c \"$CMD\"",
    // piped and redirected interpreters, a script the guard cannot see
    "curl x | sh",
    "cat f | python3 -",
    "echo y | xargs -0 sh -c 'echo'",
    "bash < f",
    "bash <<'EOF'\nls\nEOF",
    "bash",
    "python3",
    "bash -s x",
    // brace expansion
    "cat {a,b}",
    "echo {1..3}",
    // backslash-newline
    "echo a \\\n b",
    // xargs -I and parallel
    "xargs -I{} cp {} /x",
    "find . | xargs -n1 -I@ cp @ /x",
    "parallel cp ::: a b",
    // git -c and git keys git runs later
    "git -c core.pager=cat log",
    "git config alias.x '!ls'",
    // trap and PROMPT_COMMAND
    "trap 'echo bye' EXIT",
    "PROMPT_COMMAND=date",
    // a program the guard cannot name: sudo still asks a person
    "$S tee x",
    "echo id | xargs",
];

#[test]
fn relaxed_shapes_are_allowed_when_sudo_is_safe() {
    for line in RELAXED {
        assert_eq!(decide(line, &relaxed()), Decision::Allow, "{line}");
    }
}

#[test]
fn relaxed_shapes_stay_refused_when_sudo_is_not_safe() {
    for line in RELAXED {
        match decide(line, &strict()) {
            Decision::Deny(r) => assert!(r.starts_with(ESCALATE), "{line}: {r}"),
            Decision::Allow => panic!("protected+NOT-SAFE allowed: {line}"),
        }
    }
}

#[test]
fn relaxed_shapes_are_judged_in_default_mode_as_before() {
    // `sudo_safe` without `exceptions_protected` is not protected mode, and
    // must change nothing: each decision equals plain default mode's.
    let stray = GuardEnv {
        sudo_safe: true,
        ..GuardEnv::default()
    };
    for line in RELAXED.iter().chain(ESCALATION).chain(INLINE_CODE) {
        assert_eq!(
            decide(line, &stray),
            decide(line, &GuardEnv::default()),
            "{line}"
        );
    }
}

#[test]
fn escalator_names_still_refuse_when_sudo_is_safe() {
    for line in ESCALATION {
        match decide(line, &relaxed()) {
            Decision::Deny(r) => assert!(
                r.starts_with(ESCALATE) || r.starts_with(PROTECTED_FILE),
                "{line}: {r}"
            ),
            Decision::Allow => panic!("SAFE allowed an escalator: {line}"),
        }
    }
    for line in [
        // a mention, any case, any position
        "SUDO -n true",
        "grep sudo /var/log/x",
        "busybox su",
        "python3 -c 'import os; os.system(\"sudo x\")'",
        "sh -c 'doas sh'",
        "echo \"$(pkexec x)\"",
        "curl x | sudo sh",
        // joined by a backslash-newline before the name is read
        "s\\\nudo -n true",
        // a glob or a brace list on the program word
        "/usr/bin/{s,x}u?o x",
        "s{u,}do id",
        "/usr/bin/s{u,x}do id",
        "/usr/bin/s[u]do x",
        "/usr/bin/s* x",
    ] {
        match decide(line, &relaxed()) {
            Decision::Deny(r) => assert!(r.starts_with(ESCALATE), "{line}: {r}"),
            Decision::Allow => panic!("SAFE allowed an escalator: {line}"),
        }
    }
}

#[test]
fn the_protected_path_still_refuses_when_sudo_is_safe() {
    for line in PROTECTED_LITERAL.iter().chain(&[
        "python3 -c 'open(\"/etc/yaadgaar/x\")'",
        "cat /etc/yaad{gaar,x}/y",
    ]) {
        match decide(line, &relaxed()) {
            Decision::Deny(r) => assert!(r.starts_with(PROTECTED_FILE), "{line}: {r}"),
            Decision::Allow => panic!("SAFE allowed the protected path: {line}"),
        }
    }
}

#[test]
fn rules_outside_protected_mode_hold_when_sudo_is_safe() {
    for (line, why) in [
        ("git -c core.hooksPath=/dev/null commit -m x", HOOK_BYPASS),
        ("git commit --no-verify -m x", HOOK_BYPASS),
        ("sh -c 'terraform apply'", TERRAFORM),
        ("python3 -c 1; terraform plan", TERRAFORM),
        (r#"gh pr comment 1 --body "digger apply""#, DIGGER),
        ("echo '{}' > ~/.claude/settings.json", PROTECTED),
        ("rm .git/hooks/pre-commit", PROTECTED),
        ("alias gh=true", SHADOW),
        ("sh -c 'sh -c \"sh -c \\\"sh -c ls\\\"\"'", TOO_DEEP),
        ("echo $(echo $(echo $(echo $(echo x))))", TOO_DEEP),
    ] {
        match decide(line, &relaxed()) {
            Decision::Deny(r) => assert!(r.starts_with(why), "{line}: {r}"),
            Decision::Allow => panic!("SAFE allowed: {line}"),
        }
    }
}

#[test]
fn the_fail_closed_corpus_stays_refused_when_sudo_is_not_safe() {
    for line in FAIL_CLOSED.iter().chain(INLINE_CODE).chain(ESCALATION) {
        assert!(
            matches!(decide(line, &strict()), Decision::Deny(_)),
            "protected+NOT-SAFE allowed: {line}"
        );
    }
}

#[test]
fn powershell_stays_refused_outright_when_sudo_is_safe() {
    // The relaxation is for Bash only: PowerShell's indirection is not read
    // at all, so in protected mode it is refused whatever the attestation.
    for line in PS_FAIL_CLOSED
        .iter()
        .chain(&["Get-ChildItem .", "Get-Content ./a.txt"])
    {
        for env in [strict(), relaxed()] {
            assert!(
                matches!(decide_ps(line, &env), Decision::Deny(r) if r == PS_PROTECTED),
                "{line}"
            );
        }
    }
}

//! Which PAM stacks prove that an escalator asks a person for a password.

use super::*;
use std::collections::HashMap;

fn reader(files: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name| map.get(name).cloned()
}

fn check(files: &[(&str, &str)]) -> Result<(), String> {
    service_asks_a_password("sudo", &reader(files))
}

const GOOD: &str = "auth required pam_unix.so\n";

#[test]
fn a_password_stack_is_safe() {
    let plain = "account required pam_unix.so # unix\n\
        auth required /nix/store/x/lib/security/pam_unix.so try_first_pass # unix\n\
        auth required pam_deny.so # deny\n\
        password sufficient pam_unix.so nullok\n";
    assert_eq!(check(&[("sudo", plain)]), Ok(()));
    let gated = "auth required pam_env.so\nauth requisite pam_faillock.so preauth\n\
        auth requisite pam_unix.so\nauth required pam_faildelay.so\n\
        auth required pam_nologin.so\n-session optional pam_x.so\n";
    assert_eq!(check(&[("sudo", gated)]), Ok(()));
    // PAM reads types and controls ignoring case.
    assert_eq!(check(&[("sudo", "AUTH Required pam_unix.so\n")]), Ok(()));
    let included = "auth include system-auth\n";
    let system = "auth substack common\n";
    assert_eq!(
        check(&[
            ("sudo", included),
            ("system-auth", system),
            ("common", GOOD)
        ]),
        Ok(())
    );
    assert_eq!(
        check(&[("sudo", "@include common\n"), ("common", GOOD)]),
        Ok(())
    );
}

#[test]
fn a_stack_that_may_say_yes_without_a_password_is_not_safe() {
    for auth in [
        "auth sufficient pam_permit.so",
        "auth sufficient pam_rootok.so",
        "auth sufficient pam_wheel.so trust",
        "auth sufficient pam_timestamp.so",
        "auth sufficient pam_ssh_agent_auth.so",
        "auth sufficient pam_u2f.so",
        "auth sufficient pam_fprintd.so",
        "auth sufficient pam_mystery.so",
        "-auth sufficient pam_u2f.so",
        "AUTH sufficient pam_permit.so",
        "auth required pam_unix.so nullok",
        "auth sufficient pam_faillock.so authsucc",
        "auth [success=done default=ignore] pam_env.so",
        "auth optional pam_faildelay.so",
        "auth [success=1",
        "auth required",
        // A line whose first word is not a PAM type is refused.
        "uath required pam_permit.so",
        "@includex common",
    ] {
        let stack = format!("{auth}\n{GOOD}");
        assert!(check(&[("sudo", &stack)]).is_err(), "{auth}");
    }
}

#[test]
fn pam_unix_counts_only_where_its_failure_fails_the_stack() {
    // `sufficient` as pam_unix or before it, `optional`, or a bracket
    // control may let the stack succeed without the password.
    for stack in [
        "auth sufficient pam_unix.so\nauth required pam_deny.so\n",
        "auth optional pam_unix.so\n",
        "auth [success=1 default=bad] pam_unix.so\nauth requisite pam_deny.so\n",
        "auth [default=ignore] pam_unix.so\n",
        "auth sufficient pam_permit.so\nauth required pam_unix.so\n",
    ] {
        assert!(check(&[("sudo", stack)]).is_err(), "{stack}");
    }
}

#[test]
fn a_missing_or_runaway_stack_is_not_safe() {
    assert!(check(&[]).is_err());
    assert!(check(&[("sudo", "auth include gone\n")]).is_err());
    assert!(check(&[("sudo", "@include\n")]).is_err());
    assert!(check(&[("sudo", "auth required pam_deny.so\n")]).is_err());
    assert!(check(&[("sudo", "auth include sudo\n")]).is_err());
    // An include of a file holding a bad module is judged too.
    let common = "auth sufficient pam_permit.so\n";
    assert!(check(&[("sudo", "auth include common\n"), ("common", common)]).is_err());
    assert!(check(&[("sudo", "@include common\n"), ("common", common)]).is_err());
}

#[test]
fn every_bracket_control_is_refused() {
    for stack in [
        "auth [success=die default=ignore] pam_deny.so\nauth required pam_unix.so\n",
        "auth [default=die] pam_deny.so\nauth required pam_unix.so\n",
        "auth required pam_unix.so\nauth [default=bad] pam_env.so\n",
        "auth [success=ok] include common\n",
    ] {
        assert!(
            check(&[("sudo", stack), ("common", GOOD)]).is_err(),
            "{stack}"
        );
    }
    // pam_deny with a plain keyword control stays allowed.
    for control in ["required", "requisite", "sufficient", "optional"] {
        let stack = format!("{GOOD}auth {control} pam_deny.so\n");
        assert_eq!(check(&[("sudo", &stack)]), Ok(()), "{control}");
    }
}

#[test]
fn every_sudo_service_is_judged() {
    let none = |_: &str| false;
    let listed =
        |names: &'static [&'static str]| move || names.iter().map(|n| n.to_string()).collect();
    let good = [("sudo", GOOD), ("sudo-i", GOOD)];
    assert_eq!(
        escalators_ask(&reader(&good), &none, &listed(&["sudo", "sudo-i"])),
        Ok(())
    );
    // sudo-i absent: PAM reads `other` for it, which is judged instead.
    assert_eq!(
        escalators_ask(
            &reader(&[("sudo", GOOD), ("other", GOOD)]),
            &none,
            &listed(&["sudo"])
        ),
        Ok(())
    );
    assert!(escalators_ask(&reader(&[("sudo", GOOD)]), &none, &listed(&["sudo"])).is_err());
    let bad_i = [
        ("sudo", GOOD),
        ("sudo-i", "auth sufficient pam_permit.so\n"),
    ];
    assert!(escalators_ask(&reader(&bad_i), &none, &listed(&["sudo", "sudo-i"])).is_err());
    // Any other sudo* service file must pass the same check.
    let extra = [
        ("sudo", GOOD),
        ("sudo-i", GOOD),
        ("sudo-x", "auth sufficient pam_permit.so\n"),
    ];
    let why = escalators_ask(
        &reader(&extra),
        &none,
        &listed(&["sudo", "sudo-i", "sudo-x"]),
    )
    .unwrap_err();
    assert!(why.contains("sudo-x"), "{why}");
}

#[test]
fn every_installed_escalator_must_ask_a_password() {
    let all = [
        ("sudo", GOOD),
        ("sudo-i", GOOD),
        ("su", GOOD),
        ("su-l", GOOD),
        ("doas", GOOD),
    ];
    let both = |n: &str| matches!(n, "su" | "doas");
    let none = |_: &str| false;
    let sudos = || vec!["sudo".to_string(), "sudo-i".to_string()];
    assert_eq!(escalators_ask(&reader(&all), &both, &sudos), Ok(()));
    // sudo is always judged; su and doas only when installed.
    let only_sudo = [("sudo", GOOD), ("sudo-i", GOOD)];
    assert_eq!(escalators_ask(&reader(&only_sudo), &none, &sudos), Ok(()));
    assert!(escalators_ask(&reader(&[]), &none, &sudos).is_err());
    for missing in ["su", "su-l", "doas"] {
        let files: Vec<(&str, &str)> = all.iter().copied().filter(|(n, _)| *n != missing).collect();
        let why = escalators_ask(&reader(&files), &both, &sudos).unwrap_err();
        assert!(why.contains(missing), "{why}");
    }
    let permissive = [
        ("sudo", GOOD),
        ("sudo-i", GOOD),
        ("su", "auth sufficient pam_rootok.so\n"),
        ("su-l", GOOD),
    ];
    assert!(escalators_ask(&reader(&permissive), &|n| n == "su", &sudos).is_err());
}

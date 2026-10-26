//! `yaadgaar guard lock`: who may run it, whose file it installs, and that
//! what it leaves passes the guard's own verification. Run unprivileged
//! against a scratch tree owned by this test's uid.

use super::*;

#[test]
fn lock_refuses_unless_root() {
    for euid in [None, Some(1000)] {
        let err = preconditions(euid, Some("alice".into()), Some("1000".into())).unwrap_err();
        assert!(err.contains("must run as root"), "{err}");
    }
}

#[test]
fn lock_refuses_when_the_source_user_is_ambiguous() {
    let cases = [
        (None, Some("1000")),
        (Some(""), Some("1000")),
        (Some("alice"), None),
        (Some("alice"), Some("x")),
        (Some("root"), Some("0")),
        (Some("alice"), Some("0")),
    ];
    for (user, uid) in cases {
        let err = preconditions(Some(0), user.map(Into::into), uid.map(Into::into)).unwrap_err();
        assert!(err.contains("SUDO_"), "case {user:?}");
    }
    assert_eq!(
        preconditions(Some(0), Some("alice".into()), Some("1000".into())),
        Ok(("alice".to_string(), 1000))
    );
}

const PASSWD: &str = "root:x:0:0:System administrator:/root:/bin/sh\n\
    alice:x:1000:100::/home/alice:/bin/zsh\n\
    dup:x:1001:100::/home/a:/bin/sh\n\
    dup:x:1001:100::/home/b:/bin/sh\n\
    rel:x:1002:100::home/rel:/bin/sh\n";

#[test]
fn the_home_comes_from_the_password_database_and_must_match_the_uid() {
    assert_eq!(
        passwd_home(PASSWD, "alice", 1000),
        Ok(PathBuf::from("/home/alice"))
    );
    for (name, uid) in [("alice", 1001), ("nobody", 9), ("dup", 1001), ("rel", 1002)] {
        assert!(passwd_home(PASSWD, name, uid).is_err(), "{name}");
    }
}

mod sudo_check {
    use crate::hook::lock::sudo::{verdict, ONLY_AS_STRONG};

    fn first(sudoers: Option<&str>) -> String {
        verdict(sudoers, None)[0].clone()
    }

    #[test]
    fn only_timestamp_timeout_0_without_nopasswd_is_ok() {
        assert!(
            first(Some("Defaults timestamp_timeout=0\nroot ALL=(ALL) ALL")).starts_with("sudo: OK")
        );
        assert!(first(Some("Defaults env_reset, timestamp_timeout = 0")).starts_with("sudo: OK"));
        assert!(first(Some("Defaults timestamp_timeout=10")).starts_with("sudo: WARNING"));
    }

    #[test]
    fn a_parse_that_is_not_complete_and_unambiguous_is_unknown() {
        // Fail closed (eighth review): scoped Defaults, includes the guard
        // did not read, and timestamp options it does not model.
        for text in [
            "Defaults:alice timestamp_timeout=0",
            "Defaults@host timestamp_timeout=0",
            "Defaults>root timestamp_timeout=0",
            "Defaults!/bin/ls timestamp_timeout=0",
            "Defaults timestamp_timeout=0\nDefaults:alice !authenticate",
            "Defaults timestamp_timeout=0\n@include /etc/sudoers.local",
            "Defaults timestamp_timeout=0\n#include /etc/sudoers.local",
            "Defaults timestamp_timeout=0\n@includedir /etc/other.d",
            "Defaults timestamp_timeout=0, timestampowner=alice",
        ] {
            assert!(first(Some(text)).starts_with("sudo: UNKNOWN"), "{text}");
        }
        // The directory the guard reads itself is followed, not unknown.
        let read =
            "Defaults timestamp_timeout=0\n#includedir /etc/sudoers.d\n@includedir /etc/sudoers.d";
        assert!(first(Some(read)).starts_with("sudo: OK"));
    }

    #[test]
    fn the_last_value_of_a_repeated_option_wins() {
        let later_loose = "Defaults timestamp_timeout=0\nDefaults timestamp_timeout=15";
        assert!(first(Some(later_loose)).starts_with("sudo: WARNING"));
        let later_tight = "Defaults timestamp_timeout=15\nDefaults timestamp_timeout=0";
        assert!(first(Some(later_tight)).starts_with("sudo: OK"));
        let type_flip = "Defaults timestamp_type=tty\nDefaults timestamp_type=global";
        assert!(first(Some(type_flip)).contains("global"));
        assert!(first(Some("Defaults !tty_tickets")).contains("global"));
    }

    #[test]
    fn no_authentication_is_degraded() {
        let text = "Defaults timestamp_timeout=0\nDefaults !authenticate";
        assert!(first(Some(text)).starts_with("sudo: WARNING"));
        assert!(first(Some(text)).contains("!authenticate"));
        let rule = "Defaults timestamp_timeout=0\nmax ALL=(ALL) NOPASSWD:ALL";
        assert!(first(Some(rule)).starts_with("sudo: WARNING"));
    }

    #[test]
    fn root_equivalent_groups_are_named_and_degrade() {
        use crate::hook::lock::sudo::{group_warnings, protected_headline};
        let passwd = "alice:x:1000:100::/home/alice:/bin/sh\nbob:x:1001:998::/home/bob:/bin/sh\n";
        let group =
            "users:x:100:\ndocker:x:998:\nlibvirtd:x:997:alice\nwheel:x:1:alice\ndisk:x:6:bob,alice\n";
        let alice = group_warnings(passwd, group, 1000);
        assert!(
            alice
                .iter()
                .any(|l| l.contains("libvirtd") && l.contains("host disks")),
            "{alice:?}"
        );
        assert!(alice.iter().any(|l| l.contains("`disk`")), "{alice:?}");
        assert!(!alice.iter().any(|l| l.contains("wheel")), "{alice:?}");
        // A primary group counts.
        let bob = group_warnings(passwd, group, 1001);
        assert!(bob.iter().any(|l| l.contains("`docker`")), "{bob:?}");
        assert!(group_warnings(passwd, "users:x:100:\n", 1000).is_empty());
        let mut lines = verdict(Some("Defaults timestamp_timeout=0"), None);
        lines.extend(alice);
        assert!(protected_headline(&lines)[0].contains("DEGRADED"));
    }

    #[test]
    fn no_trusted_sudo_binary_is_unknown_and_degrades() {
        use crate::hook::lock::sudo::{pick_sudo, protected_headline, SUDO_CANDIDATES};
        // The candidates are fixed absolute paths, never PATH.
        assert!(SUDO_CANDIDATES.iter().all(|c| c.starts_with('/')));
        assert_eq!(SUDO_CANDIDATES[0], "/run/wrappers/bin/sudo");
        let none = pick_sudo(&["/a/sudo", "/b/sudo"], |_| Err("loose".into()));
        assert!(
            matches!(&none, Err(l) if l.starts_with("sudo -l: UNKNOWN")),
            "{none:?}"
        );
        let second = pick_sudo(&["/a/sudo", "/b/sudo"], |p| {
            if p.ends_with("b/sudo") {
                Ok(p.into())
            } else {
                Err("loose".into())
            }
        });
        assert_eq!(second, Ok(std::path::PathBuf::from("/b/sudo")));
        let mut lines = verdict(Some("Defaults timestamp_timeout=0"), None);
        lines.push(none.unwrap_err());
        assert!(protected_headline(&lines)[0].contains("DEGRADED"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_sudoers_d_is_unknown_and_a_missing_one_is_not() {
        use crate::hook::lock::sudo::read_sudoers_from;
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testserver::scratch_dir("sudoers-d");
        let main = dir.join("sudoers");
        std::fs::write(&main, "Defaults timestamp_timeout=0\n").unwrap();
        assert!(read_sudoers_from(&main, &dir.join("absent")).is_some());
        let d = dir.join("sudoers.d");
        std::fs::create_dir(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o000)).unwrap();
        // As root the mode does not stop the read; assert only as a user.
        let root = crate::hook::mode::effective_uid() == Some(0);
        assert!(root || read_sudoers_from(&main, &d).is_none());
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sudo_list_output_adds_what_the_parse_missed() {
        use crate::hook::lock::sudo::sudo_list_warnings;
        let out = "User alice may run the following commands on h:\n    (ALL) NOPASSWD: ALL\n";
        assert_eq!(sudo_list_warnings(out).len(), 1);
        assert!(sudo_list_warnings("    (ALL) ALL\n").is_empty());
        assert_eq!(
            sudo_list_warnings("Matching Defaults entries:\n    !authenticate\n").len(),
            1
        );
    }

    #[test]
    fn nopasswd_global_and_unset_warn_and_tty_is_a_caution() {
        let nopass = "Defaults timestamp_timeout=0\nmax ALL=(ALL) NOPASSWD: ALL";
        assert!(first(Some(nopass)).contains("NOPASSWD"));
        assert!(first(Some(nopass)).starts_with("sudo: WARNING"));
        let global = first(Some("Defaults timestamp_type=global"));
        assert!(global.starts_with("sudo: WARNING") && global.contains("global"));
        assert!(first(Some("Defaults timestamp_type=tty")).starts_with("sudo: CAUTION"));
        assert!(first(Some("root ALL=(ALL) ALL")).starts_with("sudo: WARNING"));
        // A commented-out setting is not a setting.
        assert!(first(Some("# Defaults timestamp_timeout=0")).starts_with("sudo: WARNING"));
        assert!(first(Some(
            "# alice ALL=(ALL) NOPASSWD: ALL\nDefaults timestamp_timeout=0"
        ))
        .starts_with("sudo: OK"));
    }

    #[test]
    fn unreadable_sudoers_is_unknown_and_every_verdict_says_how_strong_it_is() {
        let lines = verdict(None, None);
        assert!(lines[0].starts_with("sudo: UNKNOWN"));
        assert_eq!(lines.last().map(String::as_str), Some(ONLY_AS_STRONG));
    }

    #[test]
    fn protected_is_reported_only_when_sudo_is_safe_for_agents() {
        use crate::hook::lock::sudo::{protected_headline, FIX};
        let head = |s: Option<&str>, d: Option<&str>| protected_headline(&verdict(s, d));
        assert_eq!(
            head(Some("Defaults timestamp_timeout=0"), None),
            ["mode: protected"]
        );
        // tty is NOT safe: an agent harness runs in the terminal that holds
        // the ticket (review of #94).
        assert_eq!(
            head(Some("Defaults timestamp_type=tty"), None)[0],
            "mode: PROTECTED (DEGRADED: sudo reusable)"
        );
        for (s, d) in [
            (Some("root ALL=(ALL) ALL"), None),
            (Some("Defaults timestamp_type=global"), None),
            (
                Some("Defaults timestamp_timeout=0\nmax ALL=(ALL) NOPASSWD: ALL"),
                None,
            ),
            (
                Some("Defaults timestamp_timeout=0"),
                Some("permit persist alice"),
            ),
        ] {
            let lines = head(s, d);
            assert_eq!(
                lines[0], "mode: PROTECTED (DEGRADED: sudo reusable)",
                "{s:?}"
            );
            assert_eq!(lines[1], FIX);
        }
        assert_eq!(
            head(None, None)[0],
            "mode: PROTECTED (DEGRADED: sudo settings not verified)"
        );
    }

    #[test]
    fn status_says_protected_only_with_safe_sudo_and_never_in_default_mode() {
        use crate::hook::mode::{Loaded, Mode};
        use crate::hook::Policy;
        let loaded = |mode| Loaded {
            mode,
            policy: Policy::ALL_ON,
        };
        let unsafe_sudo = verdict(Some("root ALL=(ALL) ALL"), None);
        let safe_sudo = verdict(Some("Defaults timestamp_timeout=0"), None);
        let run = |mode, v: &Vec<String>| {
            crate::hook::lock::status_lines(
                &loaded(mode),
                None,
                "guard binary: x".into(),
                v.clone(),
            )
        };
        assert_eq!(run(Mode::Protected, &safe_sudo)[0], "mode: protected");
        let degraded = run(Mode::Protected, &unsafe_sudo);
        assert!(degraded[0].contains("DEGRADED"), "{degraded:?}");
        assert!(degraded.iter().any(|l| l.starts_with("fix: ")));
        assert_eq!(run(Mode::Default, &unsafe_sudo)[0], "mode: default");
        assert!(run(Mode::Default, &unsafe_sudo)
            .iter()
            .any(|l| l == "guard binary: x"));
    }

    #[test]
    fn doas_nopass_and_persist_warn() {
        let lines = verdict(
            Some("Defaults timestamp_timeout=0"),
            Some("permit persist alice as root\npermit nopass alice"),
        );
        assert!(lines.iter().any(|l| l.contains("`persist`")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("`nopass`")), "{lines:?}");
        let quiet = verdict(
            Some("Defaults timestamp_timeout=0"),
            Some("# permit persist alice\npermit alice"),
        );
        assert_eq!(quiet.len(), 2, "{quiet:?}");
    }
}

#[cfg(unix)]
mod attestation_verdict {
    use crate::hook::lock::sudo::verdict;
    use crate::hook::lock::{attestation, attestation_line, Facts};
    use std::path::Path;

    fn ok() -> Vec<String> {
        verdict(Some("Defaults timestamp_timeout=0"), None)
    }

    #[test]
    fn the_attestation_is_safe_only_for_a_verdict_status_calls_safe() {
        let none: &[&Path] = &[Path::new("/nonexistent-yaadgaar-input")];
        let mut cases: Vec<Vec<String>> = [
            None,
            Some("Defaults timestamp_type=tty"),
            Some("Defaults timestamp_timeout=0\nmax ALL=(ALL) NOPASSWD:ALL"),
            Some("Defaults timestamp_timeout=0,!authenticate"),
            Some("@include /x"),
        ]
        .iter()
        .map(|s| verdict(*s, None))
        .collect();
        for doas in ["permit nopass max", "permit persist max"] {
            cases.push(verdict(Some("Defaults timestamp_timeout=0"), Some(doas)));
        }
        for extra in [
            "sudo -l: WARNING — sudo lists a NOPASSWD rule for this user",
            "sudo -l: UNKNOWN — no root-owned sudo at a fixed path verifies (x)",
            // A root-equivalent group keeps the strict rules too.
            "groups: WARNING — member of `libvirtd`, which x: root by another name",
        ] {
            let mut lines = ok();
            lines.push(extra.into());
            cases.push(lines);
        }
        for lines in cases {
            let text = attestation(&lines, &facts(), none);
            assert!(text.contains("verdict: NOT SAFE"), "{lines:?}: {text}");
        }
    }

    #[test]
    fn an_unknown_group_set_keeps_the_strict_rules() {
        let none: &[&Path] = &[Path::new("/nonexistent-yaadgaar-input")];
        let text = attestation(
            &ok(),
            &Facts {
                gids: None,
                ..facts()
            },
            none,
        );
        assert!(text.contains("verdict: NOT SAFE"), "{text}");
        let text = attestation(&ok(), &facts(), none);
        assert!(text.contains("user gids: 100 1000\n"), "{text}");
    }

    fn facts() -> Facts<'static> {
        Facts {
            nsswitch: Ok(None),
            sudo_conf: Ok(None),
            polkit_escalators: Vec::new(),
            polkit_service: Vec::new(),
            pam: Ok(()),
            gids: Some(&[1000, 100]),
        }
    }

    #[test]
    fn polkit_reachable_over_d_bus_keeps_the_strict_rules() {
        let none: &[&Path] = &[Path::new("/nonexistent-yaadgaar-input")];
        for found in [
            "/usr/share/dbus-1/system-services/org.freedesktop.PolicyKit1.service",
            "a running polkitd",
        ] {
            let text = attestation(
                &ok(),
                &Facts {
                    polkit_service: vec![found.into()],
                    ..facts()
                },
                none,
            );
            assert!(
                text.contains("verdict: NOT SAFE") && text.contains(found),
                "{text}"
            );
        }
    }

    #[test]
    fn a_pam_stack_that_may_not_ask_a_password_keeps_the_strict_rules() {
        let none: &[&Path] = &[Path::new("/nonexistent-yaadgaar-input")];
        let text = attestation(
            &ok(),
            &Facts {
                pam: Err("PAM module pam_permit may authenticate without a password".into()),
                ..facts()
            },
            none,
        );
        assert!(
            text.contains("verdict: NOT SAFE") && text.contains("pam_permit"),
            "{text}"
        );
    }

    #[test]
    fn status_says_whether_the_relaxed_rules_are_on() {
        use crate::hook::lock::relaxed_line;
        assert!(relaxed_line(true).starts_with("relaxed rules: ON"));
        assert!(relaxed_line(false).starts_with("relaxed rules: OFF"));
    }

    #[test]
    fn lock_attests_safe_only_for_a_safe_verdict_read_from_files() {
        let none: &[&Path] = &[Path::new("/nonexistent-yaadgaar-input")];
        let safe_one = attestation(&ok(), &facts(), none);
        assert!(safe_one.contains("verdict: SAFE"), "{safe_one}");
        assert!(attestation_line(&safe_one, true).contains("SAFE —"));
        assert!(attestation_line(&safe_one, false).contains("strict rules"));
        let unsafe_for = |lines: Vec<String>, f: Facts| {
            let text = attestation(&lines, &f, none);
            assert!(text.contains("verdict: NOT SAFE"), "{text}");
            assert!(attestation_line(&text, true).contains("strict rules"));
        };
        unsafe_for(verdict(None, None), facts());
        unsafe_for(
            ok(),
            Facts {
                nsswitch: Ok(Some("sudoers: files sss\n")),
                ..facts()
            },
        );
        unsafe_for(
            ok(),
            Facts {
                nsswitch: Err("denied".into()),
                ..facts()
            },
        );
    }

    #[test]
    fn a_polkit_escalator_on_the_system_keeps_the_strict_rules() {
        // polkit may keep an authorisation (`auth_admin_keep`), and its JS
        // rules cannot be modelled: any of its escalators present is NOT SAFE.
        let none: &[&Path] = &[Path::new("/nonexistent-yaadgaar-input")];
        let text = attestation(
            &ok(),
            &Facts {
                polkit_escalators: vec!["/usr/bin/run0".into()],
                ..facts()
            },
            none,
        );
        assert!(text.contains("verdict: NOT SAFE"), "{text}");
        assert!(text.contains("/usr/bin/run0"), "{text}");
    }

    #[test]
    fn a_sudo_conf_policy_directive_the_verdict_does_not_model_is_not_safe() {
        let none: &[&Path] = &[Path::new("/nonexistent-yaadgaar-input")];
        for conf in [
            "Plugin sudoers_policy sudoers.so sudoers_file=/opt/x\n",
            "Plugin my_policy my.so\n",
            "Path askpass /opt/x\n",
        ] {
            let text = attestation(
                &ok(),
                &Facts {
                    sudo_conf: Ok(Some(conf)),
                    ..facts()
                },
                none,
            );
            assert!(text.contains("verdict: NOT SAFE"), "{conf}: {text}");
        }
        for conf in [
            "# Plugin x\n",
            "Set disable_coredump false\n",
            "Debug sudo /var/log/x all\n",
        ] {
            let text = attestation(
                &ok(),
                &Facts {
                    sudo_conf: Ok(Some(conf)),
                    ..facts()
                },
                none,
            );
            assert!(text.contains("verdict: SAFE"), "{conf}: {text}");
        }
        let text = attestation(
            &ok(),
            &Facts {
                sudo_conf: Err("denied".into()),
                ..facts()
            },
            none,
        );
        assert!(text.contains("verdict: NOT SAFE"), "{text}");
    }
}

#[cfg(unix)]
mod review {
    use super::*;
    use crate::hook::lock::install::Plan;
    use crate::hook::lock::review::{confirms, consent, review_lines, switched_off, Consent};

    const OFF: &str = r#"{"disabled_guards":["git_commit_flags","terraform_family"]}"#;
    const NONE_OFF: &str = r#"{"disabled_guards":[]}"#;

    fn plan(content: &str, current: Option<Result<String, String>>) -> Plan {
        Plan {
            source: PathBuf::from("/home/u/copy.json"),
            content: content.into(),
            current,
        }
    }

    #[test]
    fn the_review_shows_the_content_the_change_and_every_guard_switched_off() {
        let lines = review_lines(&plan(OFF, Some(Ok(NONE_OFF.into()))), "/p");
        let text = lines.join("\n");
        assert!(text.contains(&format!("    {OFF}")), "{text}");
        assert!(text.contains(&format!("  - {NONE_OFF}")), "{text}");
        assert!(text.contains(&format!("  + {OFF}")), "{text}");
        assert!(text.contains("git_commit_flags: "), "{text}");
        assert!(text.contains("terraform_family: "), "{text}");
        assert!(!text.contains("hook_config_tamper: "), "{text}");
        let same = review_lines(&plan(OFF, Some(Ok(OFF.into()))), "/p").join("\n");
        assert!(same.contains("already holds exactly"), "{same}");
        let none = review_lines(&plan(NONE_OFF, None), "/p").join("\n");
        assert!(none.contains("none — every guard stays on"), "{none}");
    }

    #[test]
    fn switched_off_names_each_disabled_id() {
        assert_eq!(switched_off(NONE_OFF).len(), 0);
        assert_eq!(switched_off(OFF).len(), 2);
        let all =
            r#"{"disabled_guards":["terraform_family","git_commit_flags","hook_config_tamper"]}"#;
        assert_eq!(switched_off(all).len(), 3);
    }

    #[test]
    fn without_a_terminal_only_harmless_content_with_yes_proceeds() {
        let switching = plan(OFF, Some(Ok(NONE_OFF.into())));
        assert!(
            matches!(consent(false, false, &switching), Consent::Refuse(r) if r.contains("not a terminal"))
        );
        assert!(
            matches!(consent(false, true, &switching), Consent::Refuse(r) if r.contains("--yes"))
        );
        assert_eq!(
            consent(false, true, &plan(OFF, Some(Ok(OFF.into())))),
            Consent::Proceed
        );
        assert_eq!(
            consent(false, true, &plan(NONE_OFF, None)),
            Consent::Proceed
        );
        assert!(matches!(
            consent(false, false, &plan(NONE_OFF, None)),
            Consent::Refuse(_)
        ));
        // An unverifiable current file is not "what it already holds".
        let broken = plan(OFF, Some(Err("loose".into())));
        assert!(matches!(consent(false, true, &broken), Consent::Refuse(_)));
    }

    #[test]
    fn at_a_terminal_the_person_is_asked_unless_yes_covers_it() {
        assert_eq!(consent(true, false, &plan(OFF, None)), Consent::Ask);
        assert_eq!(consent(true, true, &plan(OFF, None)), Consent::Ask);
        assert_eq!(consent(true, false, &plan(NONE_OFF, None)), Consent::Ask);
        assert_eq!(consent(true, true, &plan(NONE_OFF, None)), Consent::Proceed);
    }

    #[test]
    fn only_a_yes_confirms() {
        for a in ["y", "yes\n", " YES "] {
            assert!(confirms(a), "{a:?}");
        }
        for a in ["", "n", "no", "yess", "sure"] {
            assert!(!confirms(a), "{a:?}");
        }
    }
}

#[cfg(unix)]
mod tree {
    use super::*;
    use crate::hook::mode::fixture::{chmod, tree};
    use crate::hook::mode::verify::tree as verified;
    use crate::hook::Policy;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::path::Path;

    const SOURCE: &str = r#"{"disabled_guards":["git_commit_flags"]}"#;

    /// A tree with `etc` but no protected directory yet, a user copy, and
    /// the owner the lock installs as (this test's uid and gid).
    fn fresh(label: &str) -> (Location, PathBuf, Owner) {
        let loc = tree(label, "{}");
        std::fs::remove_dir_all(loc.dir()).unwrap();
        let source = loc.anchor.join("user-copy.json");
        std::fs::write(&source, SOURCE).unwrap();
        let meta = std::fs::metadata(&loc.anchor).unwrap();
        let owner = Owner {
            uid: meta.uid(),
            gid: meta.gid(),
        };
        (loc, source, owner)
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }

    #[test]
    fn a_first_lock_creates_the_directory_and_file_and_they_verify() {
        let (loc, source, owner) = fresh("lock-first");
        let lines = lock_into(&loc, &source, owner.uid, owner).unwrap();
        assert!(lines.iter().any(|l| l.starts_with("created")), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.starts_with("installed")),
            "{lines:?}"
        );
        assert_eq!(verified(&loc), Ok(SOURCE.to_string()));
        assert_eq!(mode_of(&loc.dir()), 0o755);
        assert_eq!(mode_of(&loc.path()), 0o644);
        // The user's copy is copied, never moved or changed.
        assert_eq!(std::fs::read_to_string(&source).unwrap(), SOURCE);
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn the_attestation_is_written_root_owned_beside_the_file_and_verifies() {
        use crate::hook::mode::attest::{holds, safe_text, unsafe_text, ATTESTATION_FILE};
        let (loc, source, owner) = fresh("lock-attest");
        lock_into(&loc, &source, owner.uid, owner).unwrap();
        let input = loc.anchor.join("sudoers");
        std::fs::write(&input, "x").unwrap();
        let inputs = [input.as_path()];
        install::write_attestation(&loc, &safe_text(&inputs, &[]).unwrap(), owner).unwrap();
        assert_eq!(mode_of(&loc.dir().join(ATTESTATION_FILE)), 0o644);
        assert!(holds(&loc, &inputs, &[]));
        // A later lock replaces it, a NOT SAFE one included.
        install::write_attestation(&loc, &unsafe_text("sudo reusable"), owner).unwrap();
        assert!(!holds(&loc, &inputs, &[]));
        // The exceptions file is untouched by either.
        assert_eq!(verified(&loc), Ok(SOURCE.to_string()));
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn a_second_lock_changes_nothing() {
        let (loc, source, owner) = fresh("lock-twice");
        lock_into(&loc, &source, owner.uid, owner).unwrap();
        let before = std::fs::metadata(loc.path()).unwrap().ino();
        let lines = lock_into(&loc, &source, owner.uid, owner).unwrap();
        assert_eq!(lines, vec![NOTHING.to_string()]);
        assert_eq!(std::fs::metadata(loc.path()).unwrap().ino(), before);
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn a_changed_source_replaces_the_file() {
        let (loc, source, owner) = fresh("lock-replace");
        lock_into(&loc, &source, owner.uid, owner).unwrap();
        std::fs::write(&source, "{}").unwrap();
        let lines = lock_into(&loc, &source, owner.uid, owner).unwrap();
        assert!(lines.iter().any(|l| l.starts_with("replaced")), "{lines:?}");
        assert_eq!(verified(&loc), Ok("{}".to_string()));
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn no_user_copy_installs_a_file_that_switches_nothing_off() {
        let (loc, source, owner) = fresh("lock-nosource");
        std::fs::remove_file(&source).unwrap();
        lock_into(&loc, &source, owner.uid, owner).unwrap();
        let text = verified(&loc).unwrap();
        assert_eq!(Policy::parse(&text), Ok(Policy::ALL_ON));
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn a_hostile_source_is_refused_and_nothing_is_written() {
        // A symlink (root would copy whatever it names), a file another uid
        // owns, and a malformed decision.
        let (loc, source, owner) = fresh("lock-hostile");
        let elsewhere = loc.anchor.join("elsewhere.json");
        std::fs::rename(&source, &elsewhere).unwrap();
        symlink(&elsewhere, &source).unwrap();
        assert!(lock_into(&loc, &source, owner.uid, owner)
            .unwrap_err()
            .contains("symlink"));
        std::fs::remove_file(&source).unwrap();
        std::fs::write(&source, SOURCE).unwrap();
        let err = lock_into(&loc, &source, owner.uid + 1, owner).unwrap_err();
        assert!(err.contains("owned by uid"), "{err}");
        std::fs::write(&source, r#"{"disabled_guards":"x"}"#).unwrap();
        assert!(lock_into(&loc, &source, owner.uid, owner)
            .unwrap_err()
            .contains("not usable"));
        assert!(!loc.dir().exists(), "a refused lock created the directory");
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn a_loosened_protected_directory_is_repaired() {
        let (loc, source, owner) = fresh("lock-repair");
        std::fs::create_dir(loc.dir()).unwrap();
        chmod(&loc.dir(), 0o777);
        let lines = lock_into(&loc, &source, owner.uid, owner).unwrap();
        assert!(lines.iter().any(|l| l.starts_with("repaired")), "{lines:?}");
        assert_eq!(mode_of(&loc.dir()), 0o755);
        assert_eq!(verified(&loc), Ok(SOURCE.to_string()));
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn a_symlinked_protected_directory_or_a_loose_ancestor_is_refused() {
        let (loc, source, owner) = fresh("lock-link");
        let real = loc.anchor.join("real");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, loc.dir()).unwrap();
        assert!(lock_into(&loc, &source, owner.uid, owner)
            .unwrap_err()
            .contains("symlink"));
        std::fs::remove_file(loc.dir()).unwrap();
        chmod(&loc.anchor.join("etc"), 0o777);
        assert!(lock_into(&loc, &source, owner.uid, owner)
            .unwrap_err()
            .contains("writable"));
        assert!(!loc.dir().exists());
        std::fs::remove_dir_all(&loc.anchor).ok();
    }
}

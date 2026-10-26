//! The sudo attestation: what holds, what goes stale, what is NOT SAFE.

use super::*;
use crate::hook::mode::fixture::{chmod, tree};

#[test]
fn su_and_doas_candidates_are_fingerprinted() {
    let all = super::inputs();
    for want in [
        "/usr/bin/su",
        "/bin/su",
        "/run/wrappers/bin/su",
        "/usr/bin/doas",
    ] {
        assert!(all.iter().any(|p| p == Path::new(want)), "{want}");
    }
}

/// Scratch inputs: a file, a directory with one entry, and an absent one.
fn inputs(loc: &Location) -> Vec<std::path::PathBuf> {
    let base = loc.anchor.join("inputs");
    std::fs::create_dir_all(base.join("d")).unwrap();
    std::fs::write(base.join("sudoers"), "Defaults timestamp_timeout=0\n").unwrap();
    std::fs::write(base.join("d").join("x"), "").unwrap();
    vec![base.join("sudoers"), base.join("d"), base.join("absent")]
}

fn attest(loc: &Location, text: &str) {
    let path = loc.dir().join(ATTESTATION_FILE);
    std::fs::write(&path, text).unwrap();
    chmod(&path, 0o644);
}

fn refs(paths: &[std::path::PathBuf]) -> Vec<&Path> {
    paths.iter().map(|p| p.as_path()).collect()
}

#[test]
fn a_verified_safe_attestation_for_todays_inputs_holds() {
    let loc = tree("attest-holds", "{}");
    let ins = inputs(&loc);
    attest(&loc, &safe_text(&refs(&ins), &[]).unwrap());
    assert!(holds(&loc, &refs(&ins), &[]));
    std::fs::remove_dir_all(&loc.anchor).ok();
}

#[test]
fn anything_but_a_verified_safe_attestation_for_todays_inputs_does_not_hold() {
    let loc = tree("attest-not", "{}");
    let ins = inputs(&loc);
    // No attestation at all.
    assert!(!holds(&loc, &refs(&ins), &[]));
    // NOT SAFE.
    attest(&loc, &unsafe_text("sudo reusable"));
    assert!(!holds(&loc, &refs(&ins), &[]));
    // A format this build does not know.
    let safe = safe_text(&refs(&ins), &[]).unwrap();
    attest(&loc, &safe.replace("v1", "v2"));
    assert!(!holds(&loc, &refs(&ins), &[]));
    // No inputs is no evidence.
    attest(&loc, &safe);
    assert!(!holds(&loc, &[], &[]));
    // Writable by others: fails the ownership walk.
    chmod(&loc.dir().join(ATTESTATION_FILE), 0o666);
    assert!(!holds(&loc, &refs(&ins), &[]));
    chmod(&loc.dir().join(ATTESTATION_FILE), 0o644);
    assert!(holds(&loc, &refs(&ins), &[]));
    // A symlink in its place.
    let path = loc.dir().join(ATTESTATION_FILE);
    let elsewhere = loc.anchor.join("elsewhere");
    std::fs::write(&elsewhere, &safe).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert!(!holds(&loc, &refs(&ins), &[]));
    std::fs::remove_dir_all(&loc.anchor).ok();
}

#[test]
fn a_changed_input_makes_the_attestation_stale() {
    let loc = tree("attest-stale", "{}");
    let ins = inputs(&loc);
    attest(&loc, &safe_text(&refs(&ins), &[]).unwrap());
    // A rule added to a file, an entry added to a directory, and an
    // absent input that appears: each one alone.
    std::fs::write(&ins[0], "max ALL=(ALL) NOPASSWD:ALL\n").unwrap();
    assert!(!holds(&loc, &refs(&ins), &[]));
    attest(&loc, &safe_text(&refs(&ins), &[]).unwrap());
    std::fs::write(ins[1].join("y"), "").unwrap();
    assert!(!holds(&loc, &refs(&ins), &[]));
    attest(&loc, &safe_text(&refs(&ins), &[]).unwrap());
    // An entry edited in place leaves the directory's own stat alone.
    std::fs::write(ins[1].join("x"), "max ALL=(ALL) NOPASSWD:ALL\n").unwrap();
    assert!(!holds(&loc, &refs(&ins), &[]));
    attest(&loc, &safe_text(&refs(&ins), &[]).unwrap());
    std::fs::write(&ins[2], "permit nopass max\n").unwrap();
    assert!(!holds(&loc, &refs(&ins), &[]));
    std::fs::remove_dir_all(&loc.anchor).ok();
}

#[test]
fn an_unlistable_input_directory_cannot_be_fingerprinted() {
    let loc = tree("attest-unlistable", "{}");
    let ins = inputs(&loc);
    chmod(&ins[1], 0o000);
    let listed = std::fs::read_dir(&ins[1]).is_ok();
    // Running as root, the mode does not stop a listing: nothing to test.
    if !listed {
        assert!(safe_text(&refs(&ins), &[]).is_err());
    }
    chmod(&ins[1], 0o755);
    std::fs::remove_dir_all(&loc.anchor).ok();
}

#[test]
fn every_input_the_review_named_is_fingerprinted() {
    let all = super::inputs();
    for want in [
        "/etc/sudoers",
        "/etc/sudoers.d",
        "/etc/sudo.conf",
        "/etc/doas.conf",
        "/etc/nsswitch.conf",
        "/etc/pam.d",
        "/usr/lib/pam.d",
        "/usr/etc/pam.d",
        "/etc/security",
        "/etc/polkit-1/rules.d",
        "/usr/share/polkit-1/rules.d",
        "/usr/bin/run0",
        "/usr/bin/pkexec",
        "/usr/bin/systemd-run",
        "/usr/bin/machinectl",
        "/run/current-system/sw/bin/run0",
        "/run/wrappers/bin/pkexec",
    ] {
        assert!(all.iter().any(|p| p == Path::new(want)), "{want}");
    }
}

#[test]
fn polkit_escalators_are_found_by_fixed_directory_and_name() {
    let loc = tree("attest-polkit", "{}");
    let bin = loc.anchor.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    assert!(polkit_escalators(&[bin.as_path()]).is_empty());
    std::fs::write(bin.join("pkexec"), "").unwrap();
    // A dangling symlink counts: present is present.
    std::os::unix::fs::symlink("/nonexistent", bin.join("run0")).unwrap();
    let found = polkit_escalators(&[bin.as_path()]);
    assert_eq!(found, vec![bin.join("pkexec"), bin.join("run0")]);
    std::fs::remove_dir_all(&loc.anchor).ok();
}

#[test]
fn the_users_group_set_must_match_exactly() {
    let loc = tree("attest-gids", "{}");
    let ins = inputs(&loc);
    attest(&loc, &safe_text(&refs(&ins), &[100, 1000]).unwrap());
    // Order and repeats do not matter; the set does.
    assert!(holds(&loc, &refs(&ins), &[1000, 100, 100]));
    assert!(!holds(&loc, &refs(&ins), &[100]));
    assert!(!holds(&loc, &refs(&ins), &[100, 1000, 998]));
    assert!(!holds(&loc, &refs(&ins), &[]));
    std::fs::remove_dir_all(&loc.anchor).ok();
}

#[test]
fn the_group_set_comes_from_the_kernel_and_the_account_databases() {
    let status = "Name:\tx\nUid:\t1000\t1000\t1000\t1000\nGid:\t100\t101\t100\t100\n\
        Groups:\t1 998 \n";
    assert_eq!(gids_from_status(status), Some(vec![101, 1, 998]));
    assert_eq!(gids_from_status("Gid:\t1\t1\t1\t1\n"), None);
    assert_eq!(
        gids_from_status("Gid:\t1\t1\t1\t1\nGroups:\n"),
        Some(vec![1])
    );
    let passwd = "root:x:0:0::/root:/bin/sh\nalice:x:1000:100::/home/alice:/bin/sh\n";
    let group = "users:x:100:\nwheel:x:1:alice,bob\ndocker:x:998:bob\n";
    assert_eq!(gids_of(passwd, group, 1000), Some(vec![100, 1]));
    assert_eq!(gids_of(passwd, group, 1001), None);
}

#[test]
fn the_account_and_group_databases_are_inputs() {
    for want in ["/etc/passwd", "/etc/group", "/etc/gshadow"] {
        assert!(
            super::inputs().iter().any(|p| p == Path::new(want)),
            "{want}"
        );
    }
}

#[test]
fn nsswitch_passwd_and_group_must_be_files_only() {
    assert!(nsswitch_files_only(Some("passwd: files\ngroup: files\n")));
    assert!(nsswitch_files_only(Some("PASSWD : files\nnetgroup: nis\n")));
    for conf in [
        "passwd: files systemd\n",
        "group: files sss\n",
        "Group:ldap\n",
        "passwd:\n",
    ] {
        assert!(!nsswitch_files_only(Some(conf)), "{conf}");
    }
}

#[test]
fn polkit_is_present_by_its_service_file_daemon_or_process() {
    let loc = tree("attest-polkitd", "{}");
    let service = loc.anchor.join("org.freedesktop.PolicyKit1.service");
    let proc = loc.anchor.join("proc");
    std::fs::create_dir_all(proc.join("1")).unwrap();
    std::fs::write(proc.join("1").join("comm"), "systemd\n").unwrap();
    assert!(polkit_present(&[service.as_path()], &proc).is_empty());
    std::fs::write(&service, "").unwrap();
    assert_eq!(polkit_present(&[service.as_path()], &proc).len(), 1);
    std::fs::remove_file(&service).unwrap();
    std::fs::create_dir_all(proc.join("42")).unwrap();
    std::fs::write(proc.join("42").join("comm"), "polkitd\n").unwrap();
    assert_eq!(
        polkit_present(&[service.as_path()], &proc),
        vec!["a running polkitd"]
    );
    // Each service path is an input, so polkit installed later is stale.
    for p in POLKIT_SERVICE {
        assert!(super::inputs().iter().any(|i| i == Path::new(p)), "{p}");
    }
    std::fs::remove_dir_all(&loc.anchor).ok();
}

#[test]
fn the_nsswitch_sudoers_key_is_matched_as_sudo_matches_it() {
    assert!(nsswitch_files_only(Some("SUDOERS : files\n")));
    assert!(nsswitch_files_only(Some("Sudoers:files\n")));
    assert!(!nsswitch_files_only(Some("SUDOERS: files ldap\n")));
    assert!(!nsswitch_files_only(Some("sudoers ldap\n")));
    assert!(!nsswitch_files_only(Some("sudoers:\n")));
    assert!(!nsswitch_files_only(Some("sudoers_x: files\n")));
    assert!(!nsswitch_files_only(Some(
        "sudoers: files [NOTFOUND=return] sss\n"
    )));
}

#[test]
fn a_sudo_conf_is_modelled_only_without_plugin_or_path_directives() {
    assert!(sudo_conf_modelled(None));
    assert!(sudo_conf_modelled(Some(
        "# Plugin x\nSet disable_coredump false\n"
    )));
    assert!(!sudo_conf_modelled(Some(
        "Plugin sudoers_policy sudoers.so\n"
    )));
    assert!(!sudo_conf_modelled(Some("  Path noexec /x\n")));
}

#[test]
fn only_a_files_sudoers_source_is_attestable() {
    assert!(nsswitch_files_only(None));
    assert!(nsswitch_files_only(Some("passwd: files\n")));
    assert!(nsswitch_files_only(Some("sudoers:   files\n")));
    assert!(nsswitch_files_only(Some("sudoers: files # local\n")));
    assert!(!nsswitch_files_only(Some("sudoers: files sss\n")));
    assert!(!nsswitch_files_only(Some("sudoers: ldap\n")));
}

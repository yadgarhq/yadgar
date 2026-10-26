//! Protected mode: each way a tree can fail, and the one way it passes.
//!
//! Run unprivileged: the tree is built in a scratch directory, and the
//! "trusted" uid is this test's own uid while the "current" uid is another.
//! The real-uid tests at the bottom judge the real `/etc` with uid 0.

use super::*;

#[test]
fn the_production_location_is_fixed_and_root_owned() {
    // Kills the wrong-anchor, wrong-directory and wrong-owner mutants.
    #[cfg(unix)]
    {
        let loc = Location::system().expect("unix has protected mode");
        assert_eq!(loc.anchor, PathBuf::from("/"));
        assert_eq!(loc.trusted_uid, 0);
        assert_eq!(loc.file, "yadgar-hook-exceptions.json");
        #[cfg(target_os = "linux")]
        assert_eq!(
            loc.path(),
            PathBuf::from("/etc/yaadgaar/yadgar-hook-exceptions.json")
        );
        assert_eq!(loc.dir(), PathBuf::from(PROTECTED_DIR));
    }
    #[cfg(not(unix))]
    assert_eq!(Location::system(), None);
}

#[test]
fn no_location_is_default_mode_reading_the_users_copy() {
    let loaded = load(None, None);
    assert_eq!(loaded.mode, Mode::Default);
    assert_eq!(loaded.policy, Policy::ALL_ON);
}

#[test]
fn status_names_the_mode_and_never_the_policy() {
    let loc = None;
    let default = status_lines(
        &Loaded {
            mode: Mode::Default,
            policy: Policy::ALL_ON,
        },
        loc,
    );
    assert_eq!(default[0], "mode: default");
    assert!(default.iter().any(|l| l.contains("ADVISORY")));
    let invalid = status_lines(
        &Loaded {
            mode: Mode::Invalid("a reason".into()),
            policy: Policy::ALL_ON,
        },
        loc,
    );
    assert!(invalid[0].contains("INVALID"));
    assert!(invalid.iter().any(|l| l == "reason: a reason"));
    let protected = status_lines(
        &Loaded {
            mode: Mode::Protected,
            policy: Policy::ALL_ON,
        },
        loc,
    );
    assert_eq!(protected[0], "mode: protected");
    for line in default.iter().chain(&invalid).chain(&protected) {
        assert!(!line.contains("disabled_guards"), "{line}");
    }
}

#[test]
fn an_invalid_note_carries_the_reason() {
    assert!(invalid_note("owned by uid 5").contains("owned by uid 5"));
}

#[cfg(unix)]
mod tree {
    use super::*;
    use crate::hook::mode::fixture::chmod;
    use std::os::unix::fs::{symlink, MetadataExt};

    const BODY: &str = r#"{"disabled_guards":["git_commit_flags"]}"#;
    const USER_BODY: &str =
        r#"{"disabled_guards":["terraform_family","git_commit_flags","hook_config_tamper"]}"#;

    fn tree(label: &str) -> Location {
        crate::hook::mode::fixture::tree(label, BODY)
    }

    /// The user's copy, switching EVERY guard off: a test that sees any of
    /// it honoured has read the wrong file.
    fn user_copy(loc: &Location) -> PathBuf {
        let path = loc.anchor.join("home-copy.json");
        std::fs::write(&path, USER_BODY).unwrap();
        path
    }

    fn invalid_reason(loc: &Location) -> String {
        match verify(loc) {
            Verified::Invalid(why) => why,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    fn done(loc: &Location) {
        std::fs::remove_dir_all(&loc.anchor).ok();
    }

    #[test]
    fn a_valid_tree_is_protected_mode_and_the_users_copy_is_ignored() {
        let loc = tree("valid");
        assert_eq!(verify(&loc), Verified::Valid(BODY.into()));
        let user = user_copy(&loc);
        let loaded = load(Some(&loc), Some(&user));
        assert_eq!(loaded.mode, Mode::Protected);
        assert_eq!(
            loaded.policy,
            Policy {
                git_commit_flags: false,
                ..Policy::ALL_ON
            }
        );
        done(&loc);
    }

    #[test]
    fn a_group_writable_file_is_invalid() {
        let loc = tree("file-g-w");
        chmod(&loc.path(), 0o664);
        assert!(invalid_reason(&loc).contains("writable"));
        done(&loc);
    }

    #[test]
    fn an_other_writable_file_is_invalid() {
        let loc = tree("file-o-w");
        chmod(&loc.path(), 0o646);
        assert!(invalid_reason(&loc).contains("writable"));
        done(&loc);
    }

    #[test]
    fn a_group_writable_ancestor_is_invalid() {
        let loc = tree("dir-g-w");
        chmod(&loc.dir(), 0o775);
        let why = invalid_reason(&loc);
        assert!(why.contains("writable"));
        assert!(why.contains("yaadgaar"));
        done(&loc);
    }

    #[test]
    fn an_other_writable_anchor_is_invalid_even_when_sticky() {
        // /tmp's shape: 1777. Refused: a non-root user can create there.
        let loc = tree("anchor-sticky");
        chmod(&loc.anchor, 0o1777);
        assert!(invalid_reason(&loc).contains("writable"));
        done(&loc);
    }

    #[test]
    fn a_component_owned_by_another_uid_is_invalid() {
        let mut loc = tree("foreign-owner");
        let me = loc.trusted_uid;
        loc.trusted_uid = me + 7;
        loc.current_uid = Some(me + 8);
        let why = invalid_reason(&loc);
        assert!(why.contains(&format!("owned by uid {me}")));
        loc.trusted_uid = me;
        done(&loc);
    }

    #[test]
    fn the_file_and_dir_judge_reads_owner_and_mode() {
        // The pure judge: an owner that is not the trusted uid, on a file
        // whose ancestors passed, is refused by the same check.
        let p = Path::new("/x");
        assert!(verify::judge(p, 5, 0o100644, 0)
            .unwrap_err()
            .contains("owned by uid 5"));
        assert!(verify::judge(p, 0, 0o100664, 0)
            .unwrap_err()
            .contains("writable"));
        assert!(verify::judge(p, 0, 0o100646, 0)
            .unwrap_err()
            .contains("writable"));
        assert_eq!(verify::judge(p, 0, 0o100644, 0), Ok(()));
        assert_eq!(verify::judge(p, 0, 0o100755, 0), Ok(()));
    }

    #[test]
    fn a_symlinked_file_is_invalid_even_to_a_valid_file() {
        let loc = tree("file-link");
        let real = loc.dir().join("real.json");
        std::fs::rename(loc.path(), &real).unwrap();
        symlink(&real, loc.path()).unwrap();
        assert!(invalid_reason(&loc).contains("symlink"));
        done(&loc);
    }

    #[test]
    fn a_symlinked_ancestor_is_invalid_even_to_a_valid_directory() {
        let loc = tree("dir-link");
        let real = loc.anchor.join("etc").join("real-yaadgaar");
        std::fs::rename(loc.dir(), &real).unwrap();
        symlink(&real, loc.dir()).unwrap();
        assert!(invalid_reason(&loc).contains("symlink"));
        done(&loc);
    }

    #[test]
    fn a_fifo_in_the_files_place_is_invalid_and_does_not_hang() {
        let loc = tree("fifo");
        std::fs::remove_file(loc.path()).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(loc.path())
            .status()
            .unwrap();
        assert!(made.success());
        chmod(&loc.path(), 0o644);
        assert!(invalid_reason(&loc).contains("regular file"));
        done(&loc);
    }

    #[test]
    fn a_missing_directory_is_default_mode_and_honours_the_users_copy() {
        let loc = tree("absent");
        std::fs::remove_dir_all(loc.dir()).unwrap();
        assert_eq!(verify(&loc), Verified::Absent);
        let user = user_copy(&loc);
        let loaded = load(Some(&loc), Some(&user));
        assert_eq!(loaded.mode, Mode::Default);
        assert_eq!(loaded.policy, Policy::parse(USER_BODY).unwrap());
        done(&loc);
    }

    #[test]
    fn a_dangling_symlink_in_the_directorys_place_is_invalid_not_absent() {
        // Review of #94: only NotFound is absence. A symlink, even to
        // nothing, is something root put there, and it does not verify.
        let loc = tree("dangling");
        std::fs::remove_dir_all(loc.dir()).unwrap();
        symlink(loc.anchor.join("nowhere"), loc.dir()).unwrap();
        assert!(matches!(verify(&loc), Verified::Invalid(_)));
        assert!(crate::hook::mode::present(&loc.dir()));
        std::fs::remove_file(loc.dir()).unwrap();
        assert!(!crate::hook::mode::present(&loc.dir()));
        done(&loc);
    }

    #[test]
    fn a_sticky_root_owned_ancestor_is_accepted_only_for_the_binary_check() {
        use crate::hook::mode::verify::{judge_ancestor, owned_below};
        let p = Path::new("/nix/store");
        // /nix/store's shape: 1775 root:nixbld.
        assert_eq!(judge_ancestor(p, 0, 0o41775, 0, true), Ok(()));
        assert!(judge_ancestor(p, 0, 0o41775, 0, false).is_err());
        assert!(
            judge_ancestor(p, 0, 0o40775, 0, true).is_err(),
            "not sticky"
        );
        assert!(
            judge_ancestor(p, 5, 0o41775, 0, true).is_err(),
            "not root's"
        );
        // On a real tree: a sticky group-writable dir, a strict leaf below.
        let loc = tree("sticky");
        let me = loc.trusted_uid;
        let store = loc.anchor.join("store");
        std::fs::create_dir(&store).unwrap();
        chmod(&store, 0o1777);
        let bin = store.join("bin");
        std::fs::write(&bin, "").unwrap();
        chmod(&bin, 0o755);
        let rel = |p: &Path| p.strip_prefix(&loc.anchor).unwrap().to_path_buf();
        assert_eq!(owned_below(&loc.anchor, &rel(&bin), me, true), Ok(()));
        assert!(owned_below(&loc.anchor, &rel(&bin), me, false).is_err());
        chmod(&bin, 0o775);
        assert!(
            owned_below(&loc.anchor, &rel(&bin), me, true).is_err(),
            "leaf loose"
        );
        chmod(&bin, 0o755);
        chmod(&store, 0o775);
        assert!(
            owned_below(&loc.anchor, &rel(&bin), me, true).is_err(),
            "not sticky"
        );
        chmod(&store, 0o755);
        done(&loc);
    }

    #[test]
    fn a_directory_without_the_file_is_invalid_not_default() {
        // Deleting the file must not downgrade to the user's copy.
        let loc = tree("no-file");
        std::fs::remove_file(loc.path()).unwrap();
        assert!(invalid_reason(&loc).contains("no exceptions file"));
        done(&loc);
    }

    #[test]
    fn an_invalid_tree_honours_no_exception_at_all() {
        let loc = tree("zero");
        chmod(&loc.path(), 0o666);
        let user = user_copy(&loc);
        let loaded = load(Some(&loc), Some(&user));
        assert!(matches!(loaded.mode, Mode::Invalid(_)), "{loaded:?}");
        assert_eq!(loaded.policy, Policy::ALL_ON);
        done(&loc);
    }

    #[test]
    fn a_malformed_protected_file_is_invalid_with_every_guard_on() {
        let loc = tree("malformed");
        std::fs::write(loc.path(), r#"{"disabled_guards":"git_commit_flags"}"#).unwrap();
        let loaded = load(Some(&loc), Some(&user_copy(&loc)));
        assert!(matches!(&loaded.mode, Mode::Invalid(w) if w.contains("not usable")));
        assert_eq!(loaded.policy, Policy::ALL_ON);
        done(&loc);
    }

    #[test]
    fn a_guard_running_as_the_owner_is_invalid() {
        let mut loc = tree("as-owner");
        loc.current_uid = Some(loc.trusted_uid);
        assert!(invalid_reason(&loc).contains("runs as uid"));
        loc.current_uid = None;
        assert!(invalid_reason(&loc).contains("cannot tell"));
        done(&loc);
    }

    #[test]
    fn the_effective_uid_is_the_owner_of_a_file_this_process_creates() {
        let dir = crate::testserver::scratch_dir("mode-euid");
        let probe = dir.join("probe");
        std::fs::write(&probe, "").unwrap();
        let owner = std::fs::metadata(&probe).unwrap().uid();
        assert_eq!(effective_uid(), Some(owner));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_real_etc_passes_the_walk_with_uid_0() {
        // REAL UIDS: `/`, `/etc` and `/etc/passwd` are root-owned and not
        // group- or other-writable on every unix this runs on. Asserted on
        // both branches, not skipped: as root, the guard is refused instead.
        let me = effective_uid().unwrap();
        let loc = Location {
            anchor: PathBuf::from("/"),
            dirs: vec!["etc".into()],
            file: "passwd".into(),
            trusted_uid: 0,
            current_uid: Some(me),
        };
        match verify(&loc) {
            Verified::Valid(text) if me != 0 => assert!(text.contains("root")),
            Verified::Invalid(why) if me == 0 => assert!(why.contains("runs as uid 0")),
            _ => panic!("the real /etc did not give the expected verdict"),
        }
    }

    #[test]
    fn a_tree_this_user_owns_fails_the_real_uid_0_check() {
        let mut loc = tree("real-uid");
        loc.trusted_uid = 0;
        loc.current_uid = effective_uid();
        let me = std::fs::metadata(&loc.anchor).unwrap().uid();
        match verify(&loc) {
            Verified::Invalid(why) if me != 0 => assert!(why.contains("owned by uid")),
            Verified::Invalid(why) => assert!(why.contains("runs as uid 0")),
            _ => panic!("a user-owned tree passed the uid 0 check"),
        }
        done(&loc);
    }
}

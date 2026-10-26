//! Bypasses of the ADR-1020 checks found in review of yadgar#91, and the
//! mutants that survived its suite.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::shadow::{bash_calls, edit, not_refused_as, refused, Machine};
use super::*;

// ── gh shadowing ─────────────────────────────────────────────────────────────

const SHADOWING: &[&str] = &[
    "builtin alias gh=x",
    "functions+=([gh]=x)",
    "aliases+=(gh x)",
    "aliases=(gh x)",
    "commands+=([gh]=/tmp/evil)",
    "mapfile PATH",
    "readarray PATH",
    "mapfile -t path < /tmp/dirs",
    "declare -n R=PATH",
    "typeset -n R=path",
    "local -n R=PATH",
    "tee /tmp/bin/{g,}h",
    "cp /tmp/e /tmp/bin/{gh,y}",
    "cp /tmp/e /tmp/bin/g{h,x}",
    "cp /tmp/e /tmp/{x,bin/gh}",
];

#[test]
fn the_reviewed_shadowing_shapes_are_refused() {
    let wrong = not_refused_as(&bash_calls(SHADOWING), SHADOW, guard);
    assert!(wrong.is_empty(), "not refused as shadowing: {wrong:#?}");
}

const NOT_SHADOWING: &[&str] = &[
    "declare -n R=OTHER",
    "declare R=PATH",
    "mapfile -t lines < /tmp/f",
    "builtin echo hi",
    "builtin cd /tmp",
    "cp /tmp/x /tmp/{a,b}",
];

#[test]
fn near_misses_of_those_shapes_are_not_refused() {
    let wrong = refused(&bash_calls(NOT_SHADOWING), guard);
    assert!(wrong.is_empty(), "over-refused: {wrong:#?}");
}

/// A `}` with no list to close is a literal to the shell. The brace arm of
/// `names_a_gh_file` recursed on such a word forever, and the stack
/// overflow aborted the hook: an exit that is not 2, so Claude Code ran
/// the call unchecked, and a line that also shadowed gh went through.
const LONE_BRACES: &[&str] = &[
    "cp x y}",
    "touch }",
    "mv a b}",
    "tee y{a,b}} < /dev/null",
    "cp x /tmp/{a,b}}",
];

#[test]
fn a_lone_close_brace_is_judged_rather_than_crashed_on() {
    let over = refused(&bash_calls(LONE_BRACES), guard);
    assert!(over.is_empty(), "over-refused: {over:#?}");
    let hidden = bash_calls(&[
        "cp x y}; alias gh=x",
        "touch }; PATH=/tmp/evil:$PATH",
        "cp x /tmp/{a,b}}; tee /tmp/bin/{g,}h",
        // A list-free `{x}` stays literal, so the name is still gh.
        "cp e /tmp/{x}/{g,}h",
    ]);
    let wrong = not_refused_as(&hidden, SHADOW, guard);
    assert!(wrong.is_empty(), "not refused as shadowing: {wrong:#?}");
    let m = Machine::new("lonebrace", Some(0o755));
    let plant = format!("touch }}; cp /tmp/e {}/t", m.dir("home/.local/bin"));
    let wrong = not_refused_as(&bash_calls(&[&plant]), SHADOW, |c| m.judge(c));
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
}

// ── PATH directories before gh ───────────────────────────────────────────────

#[test]
fn a_home_spelled_another_way_still_reaches_the_directory() {
    let m = Machine::new("homeforms", Some(0o755));
    let writes = bash_calls(&[
        "cp /tmp/x ${HOME%/}/.local/bin/t",
        "cp /tmp/x ${HOME:-/nope}/.local/bin/t",
        "cp /tmp/x $(echo ~)/.local/bin/t",
        r#"cp /tmp/x "$(echo ~)/.local/bin/t""#,
        "cp /tmp/x `echo ~`/.local/bin/t",
        "cp /tmp/x ~someone/.local/bin/t",
    ]);
    let wrong = not_refused_as(&writes, SHADOW, |c| m.judge(c));
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
    let fine = bash_calls(&[
        "cp /tmp/x ${HOME%/}/.local/share/t",
        "cp /tmp/x $(pwd)/target/t",
        "cp /tmp/x ~someone/notes.md",
        "ls $(echo ~)/.local/bin",
    ]);
    let over = refused(&fine, |c| m.judge(c));
    assert!(over.is_empty(), "over-refused: {over:#?}");
}

/// A fresh scratch directory under the temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "yaadgaar-guard-bypass-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn dir(&self, name: &str) -> PathBuf {
        let d = self.0.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tool(&self, dir: &Path, name: &str) {
        let f = dir.join(name);
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_unset_or_empty_path_fails_closed() {
    for path in [None, Some(OsStr::new("")), Some(OsStr::new("relative:bin"))] {
        let env = GuardEnv::resolve(None, Some(PathBuf::from("/home/x")), path);
        let judge = |c: &Value| pre_tool_guard(c, &Policy::ALL_ON, &env);
        for line in ["cp /tmp/x /tmp/y", "echo x > /opt/y"] {
            assert_eq!(
                judge(&bash_call(line)),
                Decision::Deny(SHADOW.into()),
                "{path:?} {line}"
            );
        }
        assert_eq!(
            judge(&edit("Write", "/tmp/y")),
            Decision::Deny(SHADOW.into())
        );
        for line in ["cat /tmp/y", "cp x y", "ls /", "git status"] {
            assert_eq!(judge(&bash_call(line)), Decision::Allow, "{path:?} {line}");
        }
    }
}

#[test]
fn the_directories_before_gh_personal_are_protected_too() {
    let s = Scratch::new("personal");
    let (a, b, c, d) = (s.dir("a"), s.dir("b"), s.dir("c"), s.dir("d"));
    s.tool(&b, "gh");
    s.tool(&c, "gh-personal");
    let path = std::env::join_paths([&a, &b, &c, &d]).unwrap();
    let env = GuardEnv::resolve(None, None, Some(&path));
    for protected in [&a, &b] {
        assert!(
            env.before_gh.contains(protected),
            "{protected:?}: {:?}",
            env.before_gh
        );
    }
    for open in [&c, &d] {
        assert!(
            !env.before_gh.contains(open),
            "{open:?}: {:?}",
            env.before_gh
        );
    }
    // No gh-personal on PATH: only the directories before gh.
    let path = std::env::join_paths([&a, &b, &d]).unwrap();
    let env = GuardEnv::resolve(None, None, Some(&path));
    assert_eq!(env.before_gh, vec![a.clone()]);
}

/// Mutant env_canon: a PATH directory that is a symlink is protected under
/// its target too.
#[cfg(unix)]
#[test]
fn a_symlinked_path_directory_is_protected_under_its_target() {
    let s = Scratch::new("canon");
    let (target, real) = (s.dir("target"), s.dir("real"));
    s.tool(&real, "gh");
    let link = s.0.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let path = std::env::join_paths([&link, &real]).unwrap();
    let env = GuardEnv::resolve(None, None, Some(&path));
    let canon = target.canonicalize().unwrap();
    let line = format!("cp /tmp/x {}/tool", canon.display());
    assert_eq!(
        pre_tool_guard(&bash_call(&line), &Policy::ALL_ON, &env),
        Decision::Deny(SHADOW.into())
    );
}

// ── plugin hook files ────────────────────────────────────────────────────────

#[test]
fn plugin_hook_scripts_and_the_installed_list_are_protected() {
    for path in [
        "/home/x/.claude/plugins/cache/m/p/1/hooks/run.sh",
        "/home/x/.claude/plugins/marketplaces/m/plugins/p/hooks/lib/x.py",
        "/home/x/.claude/plugins/installed_plugins.json",
    ] {
        assert_eq!(
            guard(&edit("Write", path)),
            Decision::Deny(PROTECTED.into()),
            "{path}"
        );
    }
    let cfg = GuardEnv::with_config_dir(Some(Path::new("/home/x/.config/claude-work")));
    assert_eq!(
        pre_tool_guard(
            &edit(
                "Write",
                "/home/x/.config/claude-work/plugins/installed_plugins.json"
            ),
            &Policy::ALL_ON,
            &cfg
        ),
        Decision::Deny(PROTECTED.into())
    );
    let writes = bash_calls(&[
        "cp /tmp/x ~/.claude/plugins/cache/m/p/1/hooks/run.sh",
        "echo '{}' > ~/.claude/plugins/installed_plugins.json",
    ]);
    let wrong = not_refused_as(&writes, PROTECTED, guard);
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
    for path in [
        "/home/x/.claude/plugins/cache/m/p/1/README.md",
        "/home/x/.claude/plugins/known_marketplaces.json",
        "/home/x/repo/plugins/p/hooks/run.sh",
    ] {
        assert_eq!(guard(&edit("Write", path)), Decision::Allow, "{path}");
    }
    let reads = bash_calls(&[
        "cat ~/.claude/plugins/installed_plugins.json",
        "ls ~/.claude/plugins/cache/m/p/1/hooks",
    ]);
    let over = refused(&reads, guard);
    assert!(over.is_empty(), "over-refused: {over:#?}");
}

// ── the surviving mutants ────────────────────────────────────────────────────

/// Mutants glob_more, more_substr_sub and more_substr_plug: a word only the
/// glob reading or only the substring reading catches.
#[test]
fn each_reading_of_a_hook_path_is_needed() {
    let writes = bash_calls(&[
        // Only the glob reading: the substring and exact readings miss `.c*`.
        "cp /tmp/x ~/.c*/plugins/p/hooks/hooks.json",
        "rm .g?t/modules/sub/hooks/pre-commit",
        // Only the substring reading: a path inside an interpreter's code.
        r#"python3 -c "open('.git/modules/sub/hooks/pre-commit','w')""#,
        r#"python3 -c "open('/h/.claude/plugins/p/hooks/hooks.json','w')""#,
        r#"python3 -c "open('.claude/plugins/p/hooks/run.sh','w')""#,
    ]);
    let wrong = not_refused_as(&writes, PROTECTED, guard);
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
}

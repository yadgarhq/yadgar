//! What a variable the command line does not set holds in the agent's shell.
//!
//! The hook runs as a child of the agent client, so it receives the client's
//! environment. The Bash tool's shell receives the same environment, then
//! changes it, measured on 2026-10-20 by comparing the client process's
//! environment with the tool shell's:
//!
//! * the shell sets or changes `PWD`, `OLDPWD`, `SHLVL`, `_`, `PATH` and
//!   `LD_LIBRARY_PATH`;
//! * the client adds about twenty variables to the tool shell only, most of
//!   them named `CLAUDE…`, and one of them (`CLAUDE_CODE_EXECPATH`) names
//!   `.claude`;
//! * the shell sources a snapshot of the user's profile, which exports
//!   variables of its own.
//!
//! So a variable resolves to its real value only when it is set in the hook's
//! environment and none of those can change it. Every other one, and every
//! variable at all when no environment is given, stays UNKNOWN: it may hold
//! anything, `.claude` included, and the caller refuses accordingly. `PWD`
//! is one of them: the payload's `cwd` is not proven to follow a `cd` made
//! in an earlier call.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Names the agent's shell sets or changes itself.
const SHELL_SET: &[&str] = &[
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "PATH",
    "LD_LIBRARY_PATH",
    "IFS",
    "RANDOM",
    "SECONDS",
    "LINENO",
    "PPID",
    "REPLY",
    "OPTARG",
    "OPTIND",
];

/// Where the guard looks up a variable the command line does not set.
pub struct Environment {
    /// The hook process's own environment. Empty when nothing resolves.
    vars: HashMap<String, String>,
    /// Names the profile snapshots set, export or unset in any way: their
    /// value in the agent's shell is the snapshot's, which this does not
    /// evaluate.
    snapshot: HashSet<String>,
}

impl Environment {
    /// No variable resolves: every one the line does not set may be anything.
    pub fn unknown() -> Self {
        Self {
            vars: HashMap::new(),
            snapshot: HashSet::new(),
        }
    }

    /// The hook process's environment, less every name the profile snapshots
    /// under *snapshots* (a directory of `*.sh` files) touch.
    ///
    /// FAILS CLOSED: with no directory, or one that cannot be read in full,
    /// nothing resolves, because what the snapshot would change is unknown.
    pub fn from_process(snapshots: Option<&Path>) -> Self {
        match snapshots.and_then(snapshot_names) {
            Some(snapshot) => Self {
                vars: std::env::vars().collect(),
                snapshot,
            },
            None => Self::unknown(),
        }
    }

    /// An environment of exactly *vars*, for a test.
    #[cfg(test)]
    pub fn of(vars: &[(&str, &str)]) -> Self {
        Self {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            snapshot: HashSet::new(),
        }
    }

    /// The value *name* holds in the agent's shell, or `None` when it is
    /// UNKNOWN.
    pub(super) fn value(&self, name: &str) -> Option<String> {
        if SHELL_SET.contains(&name)
            || name.to_ascii_uppercase().starts_with("CLAUDE")
            || self.snapshot.contains(name)
        {
            return None;
        }
        // UNSET IS UNKNOWN, not empty: the tool shell has variables the
        // hook's environment lacks.
        self.vars.get(name).cloned()
    }
}

/// Every name the `*.sh` files in *dir* may set or unset: each `NAME=`
/// anywhere in them (a plain assignment is exported under `set -a`), and
/// each operand of `export`, `unset`, `declare`, `typeset`, `local` and
/// `readonly`. `None` when the directory, or any file in it, cannot be read,
/// or when it holds no snapshot at all: then the snapshot may live elsewhere.
fn snapshot_names(dir: &Path) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    let mut seen = false;
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        if path.extension().is_none_or(|e| e != "sh") {
            continue;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        seen = true;
        for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '=')) {
            if let Some((name, _)) = word.split_once('=') {
                if is_name(name) {
                    out.insert(name.to_string());
                }
            }
        }
        for line in text.lines() {
            let mut words = line.split_whitespace();
            if matches!(
                words.next(),
                Some("export" | "unset" | "declare" | "typeset" | "local" | "readonly")
            ) {
                out.extend(
                    words
                        .map(|w| w.split('=').next().unwrap_or(""))
                        .filter(|n| is_name(n))
                        .map(str::to_string),
                );
            }
        }
    }
    seen.then_some(out)
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_variable_the_shell_cannot_change_resolves() {
        let env = Environment::of(&[
            ("SRC", "/tmp/a"),
            ("PATH", "/bin"),
            ("CLAUDE_X", "/x"),
            ("PWD", "/w"),
        ]);
        assert_eq!(env.value("SRC").as_deref(), Some("/tmp/a"));
        assert_eq!(env.value("UNSET"), None, "unset is unknown");
        assert_eq!(env.value("PATH"), None, "the shell changes it");
        assert_eq!(env.value("CLAUDE_X"), None, "the client adds them");
        assert_eq!(env.value("PWD"), None, "the shell's own directory");
        assert_eq!(Environment::unknown().value("SRC"), None);
    }

    #[test]
    fn with_no_readable_snapshot_nothing_resolves() {
        std::env::set_var("YAADGAAR_TEST_SRC", "/tmp/a");
        let missing = std::env::temp_dir().join("yaadgaar-no-such-snapshots");
        for env in [
            Environment::from_process(None),
            Environment::from_process(Some(&missing)),
        ] {
            assert_eq!(env.value("YAADGAAR_TEST_SRC"), None);
        }
    }

    #[test]
    fn a_snapshot_directory_with_no_snapshot_resolves_nothing() {
        std::env::set_var("YAADGAAR_TEST_SRC2", "/tmp/a");
        let dir = std::env::temp_dir().join(format!("yaadgaar-snap0-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let env = Environment::from_process(Some(&dir));
        assert_eq!(env.value("YAADGAAR_TEST_SRC2"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_the_snapshot_sets_or_unsets_in_any_way_is_unknown() {
        let dir = std::env::temp_dir().join(format!("yaadgaar-snap2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("snapshot-zsh-2.sh"),
            "set -a\nA=/x\nunset B\nf() { C=1; }\n",
        )
        .unwrap();
        let mut env = Environment::from_process(Some(&dir));
        for name in ["A", "B", "C"] {
            env.vars.insert(name.into(), "/tmp".into());
            assert_eq!(env.value(name), None, "{name}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_the_snapshot_exports_is_unknown() {
        let dir = std::env::temp_dir().join(format!("yaadgaar-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("snapshot-zsh-1.sh"), "export D=/x\nalias a=b\n").unwrap();
        let mut env = Environment::from_process(Some(&dir));
        env.vars.insert("D".into(), "/tmp".into());
        assert_eq!(env.value("D"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

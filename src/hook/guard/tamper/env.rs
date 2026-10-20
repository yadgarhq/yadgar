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
use std::path::{Path, PathBuf};

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
    /// under *snapshots* (a directory of `*.sh` files) or the shells' startup
    /// files spell (see [`Environment::from_vars`]).
    pub fn from_process(snapshots: Option<&Path>) -> Self {
        let vars: HashMap<String, String> = std::env::vars().collect();
        let home = vars.get("HOME").map(PathBuf::from);
        let zdotdir = vars
            .get("ZDOTDIR")
            .map(PathBuf::from)
            .or_else(|| home.clone());
        // The snapshot's name says which shell the tool runs; with no
        // snapshot of either kind, both shells' files are read.
        let kinds = snapshots.map(snapshot_kinds).unwrap_or_default();
        let (zsh, bash) = match (kinds.contains("zsh"), kinds.contains("bash")) {
            (false, false) => (true, true),
            other => other,
        };
        let mut startup = Vec::new();
        if zsh {
            startup.extend(["/etc/zshenv", "/etc/zprofile", "/etc/zlogin"].map(PathBuf::from));
            if let Some(z) = &zdotdir {
                startup.extend([".zshenv", ".zprofile", ".zlogin"].map(|f| z.join(f)));
            }
            if let Some(h) = &home {
                startup.push(h.join(".zshenv"));
            }
        }
        if bash {
            startup.extend(["/etc/profile", "/etc/bash.bashrc"].map(PathBuf::from));
            if let Some(h) = &home {
                startup.extend([".profile", ".bash_profile", ".bash_login"].map(|f| h.join(f)));
            }
        }
        Self::from_vars(vars, snapshots, &startup)
    }

    /// *vars*, less every name the snapshots under *snapshots* or the
    /// *startup* files (and the files they source) spell anywhere: a name
    /// such a file assigns, exports, unsets, reads or prints into may differ
    /// in the agent's shell, however the file does it.
    ///
    /// FAILS CLOSED, resolving nothing, when: there is no snapshot directory,
    /// it holds no snapshot, or any file in it cannot be read; a startup file
    /// that exists cannot be read, or sources a path that is not a plain
    /// absolute one; or the shell would run code of its own on starting or on
    /// a command name (`BASH_ENV`, `ENV`, an exported `BASH_FUNC_…`).
    pub fn from_vars(
        vars: HashMap<String, String>,
        snapshots: Option<&Path>,
        startup: &[PathBuf],
    ) -> Self {
        if vars
            .keys()
            .any(|k| k == "BASH_ENV" || k == "ENV" || k.starts_with("BASH_FUNC_"))
        {
            return Self::unknown();
        }
        let Some(mut snapshot) = snapshots.and_then(snapshot_names) else {
            return Self::unknown();
        };
        for file in startup {
            match file_names(file, MAX_SOURCE_DEPTH, &mut HashSet::new()) {
                Some(names) => snapshot.extend(names),
                None => return Self::unknown(),
            }
        }
        Self { vars, snapshot }
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

/// The shells the snapshots under *dir* were taken from: `snapshot-zsh-…`
/// and `snapshot-bash-…`.
fn snapshot_kinds(dir: &Path) -> HashSet<&'static str> {
    let mut out = HashSet::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        for kind in ["zsh", "bash"] {
            if name.contains(&format!("-{kind}-")) {
                out.insert(kind);
            }
        }
    }
    out
}

/// How deep `.` / `source` is followed from a startup file.
const MAX_SOURCE_DEPTH: usize = 4; // ADR-0569-EXCEPTION(CB): bound on following untrusted files, not a per-install knob.

/// Every name the `*.sh` files in *dir* spell. `None` when the directory,
/// or any file in it, cannot be read, or when it holds no snapshot at all:
/// then the snapshot may live elsewhere.
fn snapshot_names(dir: &Path) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    let mut seen = false;
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        if path.extension().is_none_or(|e| e != "sh") {
            continue;
        }
        out.extend(names_in(&std::fs::read_to_string(&path).ok()?));
        seen = true;
    }
    seen.then_some(out)
}

/// Every name *path* spells, and the files it sources, to *depth*. An absent
/// file spells nothing; `None` when one cannot be read or sources a path
/// that is not a plain absolute one.
fn file_names(path: &Path, depth: usize, seen: &mut HashSet<PathBuf>) -> Option<HashSet<String>> {
    // A file already read adds nothing: `/etc/bashrc` and `/etc/profile`
    // source each other.
    if !path.exists() || !seen.insert(path.to_path_buf()) {
        return Some(HashSet::new());
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut out = names_in(&text);
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if !matches!(words.next(), Some("." | "source")) {
            continue;
        }
        let target = words.next()?.trim_matches(['"', '\'']);
        if depth == 0 || !target.starts_with('/') || target.contains(['$', '`', '*', '?', '~']) {
            return None;
        }
        out.extend(file_names(Path::new(target), depth - 1, seen)?);
    }
    Some(out)
}

/// Every identifier in *text*.
fn names_in(text: &str) -> HashSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| is_name(w))
        .map(str::to_string)
        .collect()
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
        let missing = std::env::temp_dir().join("yaadgaar-no-such-snapshots");
        for env in [
            Environment::from_vars(vars(&[("SRC", "/tmp/a")]), None, &[]),
            Environment::from_vars(vars(&[("SRC", "/tmp/a")]), Some(&missing), &[]),
        ] {
            assert_eq!(env.value("SRC"), None);
        }
    }

    #[test]
    fn a_snapshot_directory_with_no_snapshot_resolves_nothing() {
        let dir = std::env::temp_dir().join(format!("yaadgaar-snap0-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let env = Environment::from_vars(vars(&[("SRC", "/tmp/a")]), Some(&dir), &[]);
        assert_eq!(env.value("SRC"), None);
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
        let mut env = Environment::from_vars(HashMap::new(), Some(&dir), &[]);
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
        let mut env = Environment::from_vars(HashMap::new(), Some(&dir), &[]);
        env.vars.insert("D".into(), "/tmp".into());
        assert_eq!(env.value("D"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("yaadgaar-env-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("snap")).unwrap();
        std::fs::write(dir.join("snap/snapshot-zsh-1.sh"), "f() { read X; }\n").unwrap();
        dir
    }

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_shell_that_runs_code_of_its_own_resolves_nothing() {
        let dir = scratch("bashenv");
        for extra in [
            ("BASH_ENV", "/x"),
            ("ENV", "/x"),
            ("BASH_FUNC_ls%%", "() { :; }"),
        ] {
            let env =
                Environment::from_vars(vars(&[("D", "/tmp"), extra]), Some(&dir.join("snap")), &[]);
            assert_eq!(env.value("D"), None, "{extra:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn any_name_a_snapshot_or_startup_file_spells_is_unknown() {
        let dir = scratch("startup");
        std::fs::write(dir.join("sourced"), "Y=1\n").unwrap();
        std::fs::write(
            dir.join("zshenv"),
            format!("print -v Z x\n. '{}'\n", dir.join("sourced").display()),
        )
        .unwrap();
        let env = Environment::from_vars(
            vars(&[("X", "/t"), ("Y", "/t"), ("Z", "/t"), ("D", "/t")]),
            Some(&dir.join("snap")),
            &[dir.join("zshenv"), dir.join("absent")],
        );
        for name in ["X", "Y", "Z"] {
            assert_eq!(env.value(name), None, "{name}");
        }
        assert_eq!(env.value("D").as_deref(), Some("/t"));
        std::fs::write(dir.join("zshenv"), ". $HOME/x\n").unwrap();
        let env = Environment::from_vars(
            vars(&[("D", "/t")]),
            Some(&dir.join("snap")),
            &[dir.join("zshenv")],
        );
        assert_eq!(env.value("D"), None, "a source the guard cannot follow");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

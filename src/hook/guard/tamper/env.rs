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
//! anything, `.claude` included, and the caller refuses accordingly. `PWD` is
//! the payload's working directory, unless the line runs `cd`.

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
    /// Names the profile snapshot exports: their value in the agent's shell
    /// is the snapshot's, which this does not parse.
    snapshot: HashSet<String>,
    /// The payload's working directory, which is `$PWD` in the tool shell.
    cwd: Option<String>,
}

impl Environment {
    /// No variable resolves: every one the line does not set may be anything.
    pub fn unknown() -> Self {
        Self {
            vars: HashMap::new(),
            snapshot: HashSet::new(),
            cwd: None,
        }
    }

    /// The hook process's environment, less the names in the profile
    /// snapshots under *snapshots* (a directory of `*.sh` files), with *cwd*
    /// as `$PWD`.
    pub fn from_process(snapshots: Option<&Path>, cwd: Option<&str>) -> Self {
        Self {
            vars: std::env::vars().collect(),
            snapshot: snapshots.map(exported_names).unwrap_or_default(),
            cwd: cwd.map(str::to_string),
        }
    }

    /// An environment of exactly *vars*, for a test.
    #[cfg(test)]
    pub fn of(vars: &[(&str, &str)], cwd: Option<&str>) -> Self {
        Self {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            snapshot: HashSet::new(),
            cwd: cwd.map(str::to_string),
        }
    }

    /// The value *name* holds in the agent's shell, or `None` when it is
    /// UNKNOWN. *cd* says the line changes directory, which changes `$PWD`.
    pub(super) fn value(&self, name: &str, cd: bool) -> Option<String> {
        if name == "PWD" {
            return self.cwd.clone().filter(|_| !cd);
        }
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

/// Every name a `*.sh` file in *dir* exports: `export NAME=`, `declare -x`,
/// `typeset -x`. Unreadable files add nothing; the names they would add stay
/// resolvable only if the hook's environment has them, as for any variable.
fn exported_names(dir: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sh") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let first = words.next();
            let rest: Vec<&str> = match first {
                Some("export") => words.collect(),
                Some("declare" | "typeset" | "local" | "readonly") => {
                    words.filter(|w| !w.starts_with('-')).collect()
                }
                _ => continue,
            };
            for word in rest {
                let name = word.split('=').next().unwrap_or("");
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.insert(name.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_variable_the_shell_cannot_change_resolves() {
        let env = Environment::of(
            &[("SRC", "/tmp/a"), ("PATH", "/bin"), ("CLAUDE_X", "/x")],
            Some("/w"),
        );
        assert_eq!(env.value("SRC", false).as_deref(), Some("/tmp/a"));
        assert_eq!(env.value("UNSET", false), None, "unset is unknown");
        assert_eq!(env.value("PATH", false), None, "the shell changes it");
        assert_eq!(env.value("CLAUDE_X", false), None, "the client adds them");
        assert_eq!(env.value("PWD", false).as_deref(), Some("/w"));
        assert_eq!(env.value("PWD", true), None, "a cd changes it");
        assert_eq!(Environment::unknown().value("SRC", false), None);
    }

    #[test]
    fn a_name_the_snapshot_exports_is_unknown() {
        let dir = std::env::temp_dir().join(format!("yaadgaar-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("snapshot-zsh-1.sh"), "export D=/x\nalias a=b\n").unwrap();
        let mut env = Environment::from_process(Some(&dir), None);
        env.vars.insert("D".into(), "/tmp".into());
        assert_eq!(env.value("D", false), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

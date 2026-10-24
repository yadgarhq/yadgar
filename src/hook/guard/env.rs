//! What the guard knows about the machine it runs on (ADR-1005, ADR-1020).
//!
//! Read ONCE, by the hook process, and handed in as a value: every rule
//! that depends on it is then reachable from a test with a temporary home
//! and a made-up `PATH`, and no test reads the live environment.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The parts of the hook's environment the guard's rules depend on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardEnv {
    /// `CLAUDE_CONFIG_DIR`, whose settings files are protected like
    /// `~/.claude`'s.
    pub config_dir: Option<PathBuf>,
    /// `HOME`, which `~` and `$HOME` in a word expand to.
    pub home: Option<PathBuf>,
    /// The `PATH` directories that come BEFORE the real `gh` (ADR-1020): a
    /// file written into one of them is found before gh is.
    pub before_gh: Vec<PathBuf>,
}

impl GuardEnv {
    /// The guard's environment with only *config_dir* set: no home, and no
    /// `PATH` directory protected. For tests.
    #[cfg(test)]
    pub fn with_config_dir(config_dir: Option<&Path>) -> Self {
        Self {
            config_dir: config_dir.map(Path::to_path_buf),
            ..Self::default()
        }
    }

    /// The environment this process received.
    pub fn from_process() -> Self {
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let home = dirs::home_dir();
        Self::resolve(config_dir, home, std::env::var_os("PATH").as_deref())
    }

    /// The environment for *config_dir*, *home* and the `PATH` value *path*,
    /// with the directories before `gh` resolved NOW, against the
    /// filesystem.
    pub fn resolve(
        config_dir: Option<PathBuf>,
        home: Option<PathBuf>,
        path: Option<&OsStr>,
    ) -> Self {
        Self {
            config_dir,
            home,
            before_gh: path.map(dirs_before_gh).unwrap_or_default(),
        }
    }
}

/// The absolute directories of *path* that come before the first one
/// holding an executable `gh`, each also as its canonical path when that
/// differs, so a write through a symlink's target is seen too.
///
/// NO `gh` ON PATH PROTECTS EVERY DIRECTORY: then any of them could be
/// where a planted one is found first. A RELATIVE entry is not resolved
/// against a working directory the hook does not share; a file named `gh`
/// is refused anywhere, which covers it.
fn dirs_before_gh(path: &OsStr) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in std::env::split_paths(path) {
        if !dir.is_absolute() {
            continue;
        }
        if holds_gh(&dir) {
            break;
        }
        if let Ok(real) = dir.canonicalize() {
            if real != dir && !out.contains(&real) {
                out.push(real);
            }
        }
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

/// Whether *dir* holds an executable file named `gh` (or `gh.exe`).
fn holds_gh(dir: &Path) -> bool {
    ["gh", "gh.exe"]
        .iter()
        .any(|name| std::fs::metadata(dir.join(name)).is_ok_and(|m| m.is_file() && executable(&m)))
}

#[cfg(unix)]
fn executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    true
}

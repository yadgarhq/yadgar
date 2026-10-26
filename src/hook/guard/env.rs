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
    /// The `PATH` directories that come BEFORE the real `gh`, and before
    /// the real `gh-personal` when there is one (ADR-1020): a file written
    /// into one of them is found before the one the guard trusts.
    pub before_gh: Vec<PathBuf>,
    /// The hook's `PATH` was unset, empty, or held no absolute directory,
    /// so nothing says where a planted gh would be found: EVERY absolute
    /// directory is protected (fail closed).
    pub path_unknown: bool,
    /// Where the protected exceptions file is (ADR-1028): `None` off unix
    /// and in a test that names none. Read by the hook, not by the guard.
    pub protected_file: Option<crate::hook::mode::Location>,
    /// The exceptions file in force is the protected one, verified, so the
    /// Bash text rule for it is retired (ADR-1028). Set by the hook from
    /// what it loaded, never from the process environment.
    pub exceptions_protected: bool,
    /// A root-written attestation says sudo cannot be reused without a
    /// person (ADR-1028, ruling B; `hook/mode/attest.rs`), so protected mode
    /// refuses only escalator names and the protected path. Set by the hook,
    /// in protected mode only, never from the process environment.
    pub sudo_safe: bool,
    /// The inputs the attestation is checked against: `None` is the
    /// production list (`attest::inputs`). A test names scratch files.
    pub sudo_inputs: Option<Vec<PathBuf>>,
    /// The group set the attestation must match: `None` is this process's
    /// own (`getgroups` and the effective gid). A test names its own.
    pub sudo_gids: Option<Vec<u32>>,
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
        // ONLY WHEN SOMETHING IS AT THE PROTECTED DIRECTORY, not followed:
        // absent (NotFound only), default mode costs one `lstat` and no uid
        // lookup. A symlink or an unreadable path is present, and verified.
        let protected = crate::hook::mode::present(Path::new(crate::hook::mode::PROTECTED_DIR));
        Self {
            protected_file: protected
                .then(crate::hook::mode::Location::system)
                .flatten(),
            ..Self::resolve(config_dir, home, std::env::var_os("PATH").as_deref())
        }
    }

    /// The environment for *config_dir*, *home* and the `PATH` value *path*,
    /// with the directories before `gh` resolved NOW, against the
    /// filesystem.
    pub fn resolve(
        config_dir: Option<PathBuf>,
        home: Option<PathBuf>,
        path: Option<&OsStr>,
    ) -> Self {
        let absolute = path.is_some_and(|p| std::env::split_paths(p).any(|d| d.is_absolute()));
        let before_gh = match path {
            Some(path) if absolute => {
                let (mut dirs, _) = dirs_before(path, "gh");
                // gh-personal is trusted as gh, so a directory before the real
                // one is protected too. Absent, it adds nothing: a file NAMED
                // gh-personal is refused anywhere.
                if let (personal, true) = dirs_before(path, "gh-personal") {
                    for dir in personal {
                        if !dirs.contains(&dir) {
                            dirs.push(dir);
                        }
                    }
                }
                dirs
            }
            _ => Vec::new(),
        };
        Self {
            config_dir,
            home,
            before_gh,
            path_unknown: !absolute,
            protected_file: None,
            exceptions_protected: false,
            sudo_safe: false,
            sudo_inputs: None,
            sudo_gids: None,
        }
    }
}

/// The absolute directories of *path* that come before the first one
/// holding an executable *name*, each also as its canonical path when that
/// differs, so a write through a symlink's target is seen too; and whether
/// *name* was found.
///
/// NO *name* ON PATH LISTS EVERY DIRECTORY: then any of them could be
/// where a planted one is found first. A RELATIVE entry is not resolved
/// against a working directory the hook does not share; a file named `gh`
/// is refused anywhere, which covers it.
fn dirs_before(path: &OsStr, name: &str) -> (Vec<PathBuf>, bool) {
    let mut out = Vec::new();
    for dir in std::env::split_paths(path) {
        if !dir.is_absolute() {
            continue;
        }
        if holds(&dir, name) {
            return (out, true);
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
    (out, false)
}

/// Whether *dir* holds an executable file named *name* (or `name.exe`).
fn holds(dir: &Path, name: &str) -> bool {
    [name.to_string(), format!("{name}.exe")]
        .iter()
        .any(|n| std::fs::metadata(dir.join(n)).is_ok_and(|m| m.is_file() && executable(&m)))
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

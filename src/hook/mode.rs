//! Protected mode: the exceptions file is trusted only when the operating
//! system protects it (ADR-1028, yaadgaar #434).
//!
//! Agents run as the user, so nothing running as the user can stop an agent
//! from writing a file in the user's home. The guard's only defence of that
//! file was a TEXT rule that refuses any Bash command naming it, and a text
//! rule cannot evaluate `$S/$d` (#431). Ownership is enforced by the kernel
//! for every harness and every spelling of a path, so it replaces the text
//! rule when it can be proven.
//!
//! ## Three modes
//!
//! * **Default** — no protected directory. Today's behaviour, unchanged: the
//!   copy in the user's config is read and the text rule stays on. Tamper
//!   protection is ADVISORY: any process running as this user can change the
//!   file. `yaadgaar guard status` says so.
//! * **Protected** — the file at [`Location::system`] and every directory
//!   above it pass [`verify`]. That file is read, the user's copy is ignored,
//!   and the Bash text rule for the file is retired.
//! * **Invalid** — the protected directory exists but does not pass. NO
//!   exception is honoured (every guard on), the text rule stays on, and each
//!   refusal says why. A protected directory that cannot be trusted must not
//!   switch anything off, and must not fall back to the user's copy either:
//!   that fallback is the downgrade an agent would aim for.
//!
//! ## What is verified, and how
//!
//! From the anchor (`/`) down, each directory is opened with
//! `O_DIRECTORY | O_NOFOLLOW`, and the file with `O_NOFOLLOW | O_NONBLOCK`.
//! Each is judged by `fstat` ON THE OPEN HANDLE: owned by the trusted uid
//! (0), and not writable by its group or by others. The file must be a
//! regular file and is read from that same handle.
//!
//! **Opening by path re-resolves the earlier components**, and that is sound
//! only because of the order: each earlier prefix has already been shown to be
//! root-owned and not group- or other-writable, so no other user can replace
//! anything in it between one open and the next. A symlink at any component
//! refuses, `O_NOFOLLOW` being checked at every level.
//!
//! **POSIX ACLs are covered by the group bits.** With an access ACL, the mode's
//! group bits are the ACL mask, which caps every named user and group entry: a
//! mask without write lets no entry but the owner's write. macOS ACLs are not
//! reflected in the mode and are not read (see the revisit trigger of
//! ADR-1028).
//!
//! The guard itself must also not run as the trusted uid: root can write the
//! file, so a guard running as root has nothing protecting it from its agent.
//!
//! Not unix (Windows): there is no protected mode, only the default.

use std::path::{Path, PathBuf};

use super::policy::Policy;
#[cfg(unix)]
use super::policy::EXCEPTIONS_FILE;

#[cfg(unix)]
pub(crate) mod verify;
#[cfg(unix)]
pub use verify::{effective_uid, verify};

/// The protected directory. FIXED, and that is the point: a path read from
/// the environment or a config file is a path an agent can point elsewhere.
/// macOS's `/etc` is a symlink, which the walk refuses, so its real path is
/// used there.
#[cfg(target_os = "macos")]
pub const PROTECTED_DIR: &str = "/private/etc/yaadgaar"; // ADR-0569-EXCEPTION(CB): ADR-1028's fixed root-owned location; a configurable one is one an agent could redirect.
#[cfg(not(target_os = "macos"))]
pub const PROTECTED_DIR: &str = "/etc/yaadgaar"; // ADR-0569-EXCEPTION(CB): ADR-1028's fixed root-owned location; a configurable one is one an agent could redirect.

/// The only owner the protected tree may have: root.
#[cfg_attr(not(unix), allow(dead_code))]
pub const TRUSTED_UID: u32 = 0; // ADR-0569-EXCEPTION(CB): ADR-1028 — root is the one owner an agent running as the user cannot be.

/// Where the protected file is, and whom to trust. A VALUE, so a test can
/// build a tree in a scratch directory and name its own uid as trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The first directory judged; every directory below it is judged too.
    pub anchor: PathBuf,
    /// The directories between the anchor and the file, top down.
    pub dirs: Vec<String>,
    /// The file's name.
    pub file: String,
    /// The uid that must own every component.
    pub trusted_uid: u32,
    /// The uid the guard runs as; `None` when it could not be found.
    pub current_uid: Option<u32>,
}

impl Location {
    /// The production location: [`PROTECTED_DIR`] under `/`, owned by root,
    /// judged for the uid this process runs as. `None` off unix.
    pub fn system() -> Option<Self> {
        #[cfg(unix)]
        {
            let relative = PROTECTED_DIR.trim_start_matches('/');
            Some(Self {
                anchor: PathBuf::from("/"),
                dirs: relative.split('/').map(str::to_string).collect(),
                file: EXCEPTIONS_FILE.to_string(),
                trusted_uid: TRUSTED_UID,
                current_uid: effective_uid(),
            })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// The directory that holds the file.
    pub fn dir(&self) -> PathBuf {
        let mut path = self.anchor.clone();
        path.extend(&self.dirs);
        path
    }

    /// The file's full path.
    pub fn path(&self) -> PathBuf {
        self.dir().join(&self.file)
    }
}

/// Whether anything is at *dir*, a symlink included and NOT followed:
/// only NotFound is absence (review of #94). The hook builds the protected
/// location only when this holds, so default mode costs one `lstat`.
pub fn present(dir: &Path) -> bool {
    !matches!(std::fs::symlink_metadata(dir), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// What [`verify`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verified {
    /// No protected directory: default mode.
    Absent,
    /// A protected directory that cannot be trusted, and why.
    Invalid(String),
    /// The file's text, read through the verified handle.
    Valid(String),
}

/// Which mode the guard is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Default,
    Protected,
    Invalid(String),
}

/// The mode, and the policy it gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub mode: Mode,
    pub policy: Policy,
}

/// The mode and policy for *location* (`None`: no protected mode on this
/// platform, or in this test) and the user's copy at *user*.
///
/// The user's copy is read ONLY in default mode. Protected mode reads the
/// protected file; an invalid one reads nothing and switches nothing off.
pub fn load(location: Option<&Location>, user: Option<&Path>) -> Loaded {
    let default = || Loaded {
        mode: Mode::Default,
        policy: Policy::load(user),
    };
    let Some(location) = location else {
        return default();
    };
    match check(location) {
        Verified::Absent => default(),
        Verified::Invalid(why) => invalid(why),
        Verified::Valid(text) => match Policy::parse(&text) {
            Ok(policy) => Loaded {
                mode: Mode::Protected,
                policy,
            },
            Err(why) => invalid(format!(
                "{} is not usable ({why})",
                location.path().display()
            )),
        },
    }
}

/// [`verify`], or default mode where there is no protected mode.
fn check(location: &Location) -> Verified {
    #[cfg(unix)]
    {
        verify(location)
    }
    #[cfg(not(unix))]
    {
        let _ = location;
        Verified::Absent
    }
}

fn invalid(why: String) -> Loaded {
    tracing::warn!("protected exceptions file not trusted ({why}); every guard stays on");
    Loaded {
        mode: Mode::Invalid(why),
        policy: Policy::ALL_ON,
    }
}

/// What a refusal adds in invalid mode, so the person learns why no
/// exception applied.
pub fn invalid_note(why: &str) -> String {
    format!(
        "(Protected mode is broken, so no exception is honoured: {why}. \
         Run `yaadgaar guard status`; `sudo yaadgaar guard lock` repairs it.)"
    )
}

/// The lines `yaadgaar guard status` prints for *loaded* at *location*.
/// No file content: only the mode, the path and the reason.
pub fn status_lines(loaded: &Loaded, location: Option<&Location>) -> Vec<String> {
    let path = location.map_or_else(
        || "(none on this platform)".to_string(),
        |l| l.path().display().to_string(),
    );
    match &loaded.mode {
        Mode::Protected => vec![
            "mode: protected".to_string(),
            format!("exceptions file: {path} (root-owned, verified)"),
            "the user's copy is ignored; the exceptions-file text rule is retired".to_string(),
        ],
        Mode::Invalid(why) => vec![
            "mode: INVALID — no exception is honoured".to_string(),
            format!("exceptions file: {path}"),
            format!("reason: {why}"),
            "repair: sudo yaadgaar guard lock".to_string(),
        ],
        Mode::Default => vec![
            "mode: default".to_string(),
            format!("protected exceptions file: {path} (absent)"),
            ADVISORY.to_string(),
        ],
    }
}

/// The one advisory line default mode carries.
pub const ADVISORY: &str = "tamper protection is ADVISORY: the exceptions file is in the \
     user's config, and any process running as this user can change it. Run \
     `sudo yaadgaar guard lock` for protected mode (ADR-1028).";

#[cfg(test)]
mod tests;

/// A protected tree in a scratch directory, for the tests here and in
/// `hook.rs`.
#[cfg(all(test, unix))]
pub(crate) mod fixture {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    pub(crate) fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// A valid tree holding *body*, trusted to this test's uid and judged
    /// for a different "current" uid.
    pub(crate) fn tree(label: &str, body: &str) -> Location {
        let root = crate::testserver::scratch_dir(&format!("mode-{label}"));
        let me = std::fs::metadata(&root).unwrap().uid();
        let dir = root.join("etc").join("yaadgaar");
        std::fs::create_dir_all(&dir).unwrap();
        for d in [&root, &root.join("etc"), &dir] {
            chmod(d, 0o755);
        }
        let file = dir.join(EXCEPTIONS_FILE);
        std::fs::write(&file, body).unwrap();
        chmod(&file, 0o644);
        Location {
            anchor: root,
            dirs: vec!["etc".into(), "yaadgaar".into()],
            file: EXCEPTIONS_FILE.into(),
            trusted_uid: me,
            current_uid: Some(me + 1),
        }
    }
}

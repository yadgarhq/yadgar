//! The walk that decides whether the protected file can be trusted.
//!
//! See `mode.rs` for what is judged and why opening by path is sound here.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::{Location, Verified};
use crate::hook::policy::read_open;

/// The mode bits that let anyone but the owner write.
const GROUP_OR_OTHER_WRITE: u32 = 0o022;

/// Whether the file at *location* can be trusted, and its text if so.
///
/// ABSENT only when the protected DIRECTORY does not exist. A directory
/// without the file is invalid: deleting the file must not be a way back to
/// the user's copy.
pub fn verify(location: &Location) -> Verified {
    let dir = location.dir();
    // NOT FOLLOWED: a symlink in the directory's place, even a dangling
    // one, is present and must verify (review of #94). Only NotFound is
    // absence; any other error is invalid.
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Verified::Absent,
        Err(e) => return Verified::Invalid(format!("cannot inspect {}: {e}", dir.display())),
        Ok(_) => {}
    }
    match walk(location) {
        Ok(text) => Verified::Valid(text),
        Err(why) => Verified::Invalid(why),
    }
}

/// The current uid's check, then [`tree`].
fn walk(location: &Location) -> Result<String, String> {
    let trusted = location.trusted_uid;
    match location.current_uid {
        None => return Err("the guard cannot tell which user it runs as".into()),
        Some(uid) if uid == trusted => {
            return Err(format!(
                "the guard runs as uid {uid}, which owns the protected tree and can write it"
            ))
        }
        Some(_) => {}
    }
    tree(location)
}

/// Every component from the anchor down, judged in order, then the file's
/// text. Whoever runs this is not judged: `guard lock` runs it as root.
pub(crate) fn tree(location: &Location) -> Result<String, String> {
    let trusted = location.trusted_uid;
    let mut path = location.anchor.clone();
    check_dir(&path, trusted)?;
    for dir in &location.dirs {
        path.push(dir);
        check_dir(&path, trusted)?;
    }
    path.push(&location.file);
    // NON-BLOCKING, as the user's copy is opened: a FIFO with no writer must
    // not hang the open. `read_open` then refuses anything not a regular file.
    let file = open(&path, libc::O_NOFOLLOW | libc::O_NONBLOCK).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            format!(
                "{} exists but holds no exceptions file",
                location.dir().display()
            )
        } else {
            refused_open(&path, &e)
        }
    })?;
    let meta = file
        .metadata()
        .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    judge(&path, meta.uid(), meta.mode(), trusted)?;
    read_open(file).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether every component of the absolute *path*, itself included, is
/// owned by *trusted* and not group- or other-writable. Nothing is read.
/// *sticky_ok*: see [`judge_ancestor`]; only for a binary's path.
pub(crate) fn owned_path(path: &Path, trusted: u32, sticky_ok: bool) -> Result<(), String> {
    let rel = path.strip_prefix("/").map_err(|_| "not absolute")?;
    owned_below(Path::new("/"), rel, trusted, sticky_ok)
}

/// [`owned_path`] from *anchor* (judged too) down through *rel*. The leaf
/// is always judged strictly.
pub(crate) fn owned_below(
    anchor: &Path,
    rel: &Path,
    trusted: u32,
    sticky_ok: bool,
) -> Result<(), String> {
    let mut prefix = anchor.to_path_buf();
    dir_meta(&prefix).and_then(|(u, m)| judge_ancestor(&prefix, u, m, trusted, sticky_ok))?;
    let parts: Vec<_> = rel.iter().collect();
    let Some((leaf, dirs)) = parts.split_last() else {
        return Ok(());
    };
    for dir in dirs {
        prefix.push(dir);
        dir_meta(&prefix).and_then(|(u, m)| judge_ancestor(&prefix, u, m, trusted, sticky_ok))?;
    }
    prefix.push(leaf);
    let file = open(&prefix, libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .map_err(|e| refused_open(&prefix, &e))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("cannot stat {}: {e}", prefix.display()))?;
    judge(&prefix, meta.uid(), meta.mode(), trusted)
}

/// [`judge`] for a directory ABOVE a binary: a sticky directory owned by
/// *trusted* may be group- or other-writable (`/nix/store` is 1775
/// root:nixbld), because the sticky bit stops anyone but an entry's owner
/// renaming or removing it, and the entry below is judged strictly. Never
/// used for the protected exceptions tree.
pub(crate) fn judge_ancestor(
    what: &Path,
    uid: u32,
    mode: u32,
    trusted: u32,
    sticky_ok: bool,
) -> Result<(), String> {
    if sticky_ok && uid == trusted && mode & 0o1000 != 0 {
        return Ok(());
    }
    judge(what, uid, mode, trusted)
}

/// Open *path* as a directory, refusing a symlink; its owner and mode.
fn dir_meta(path: &Path) -> Result<(u32, u32), String> {
    let dir =
        open(path, libc::O_DIRECTORY | libc::O_NOFOLLOW).map_err(|e| refused_open(path, &e))?;
    let meta = dir
        .metadata()
        .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    Ok((meta.uid(), meta.mode()))
}

/// Open *path* as a directory, refusing a symlink, and judge the handle.
pub(crate) fn check_dir(path: &Path, trusted: u32) -> Result<(), String> {
    let (uid, mode) = dir_meta(path)?;
    judge(path, uid, mode, trusted)
}

fn open(path: &Path, flags: libc::c_int) -> std::io::Result<File> {
    OpenOptions::new().read(true).custom_flags(flags).open(path)
}

/// Why *path* could not be opened. `O_NOFOLLOW` on a symlink fails with
/// `ELOOP` (`EMLINK` on FreeBSD); `O_DIRECTORY` on a file with `ENOTDIR`.
fn refused_open(path: &Path, e: &std::io::Error) -> String {
    let link = matches!(
        e.raw_os_error(),
        Some(libc::ELOOP | libc::EMLINK | libc::ENOTDIR)
    );
    if link {
        format!(
            "{} is a symlink, or not what it should be ({e})",
            path.display()
        )
    } else {
        format!("cannot open {}: {e}", path.display())
    }
}

/// The judgement on one component: owned by *trusted*, and not writable by
/// its group or by others. PURE, so an owner no unprivileged test can create
/// is still a test.
pub(super) fn judge(what: &Path, uid: u32, mode: u32, trusted: u32) -> Result<(), String> {
    if uid != trusted {
        return Err(format!(
            "{} is owned by uid {uid}, not uid {trusted}",
            what.display()
        ));
    }
    if mode & GROUP_OR_OTHER_WRITE != 0 {
        return Err(format!(
            "{} is writable by its group or by others (mode {:04o})",
            what.display(),
            mode & 0o7777
        ));
    }
    Ok(())
}

/// The uid this process runs as (its effective uid).
///
/// `geteuid()` is `unsafe` in `libc`, which `unsafe_code = "forbid"` rules
/// out. Linux states it in `/proc/self/status`; elsewhere, or without
/// `/proc`, it is the owner of a file this process creates, as
/// `config/private.rs` probes it.
pub fn effective_uid() -> Option<u32> {
    proc_status_euid().or_else(probe_euid)
}

fn proc_status_euid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    // `Uid:` real, effective, saved, filesystem.
    line.split_whitespace().nth(2)?.parse().ok()
}

fn probe_euid() -> Option<u32> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .subsec_nanos();
    let probe = std::env::temp_dir().join(format!(".yaadgaar-euid-{}-{nanos}", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&probe)
        .ok()?;
    let uid = file.metadata().ok().map(|m| m.uid());
    let _ = std::fs::remove_file(&probe);
    uid
}

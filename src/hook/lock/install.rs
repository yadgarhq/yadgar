//! What `guard lock` writes, in the order that keeps a refusal harmless:
//! the ancestors are judged and the source is read and checked BEFORE
//! anything is created, so a refused lock leaves the system as it was.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::{fchown, DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{Owner, EMPTY, NOTHING};
use crate::hook::mode::verify::{check_dir, tree};
use crate::hook::mode::Location;
use crate::hook::Policy;

const DIR_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o644;

/// What `lock` would install, read and checked, before anything is written:
/// so a person can review it (`lock/review.rs`) and a refusal changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Where the content came from.
    pub source: PathBuf,
    /// The exact bytes that would be installed.
    pub content: String,
    /// The protected file now: `None` absent, else its verified text or
    /// why it does not verify.
    pub current: Option<Result<String, String>>,
}

/// Read and check the exceptions at *source*, which uid *source_uid* must
/// own, for *location*; and judge the directories above the protected one.
pub fn plan(location: &Location, source: &Path, source_uid: u32) -> Result<Plan, String> {
    system_dirs(location)?;
    let content = read_source(source, source_uid)?;
    let current = std::fs::symlink_metadata(location.path())
        .is_ok()
        .then(|| tree(location));
    Ok(Plan {
        source: source.to_path_buf(),
        content,
        current,
    })
}

/// [`plan`] then [`apply`]: for the tests, which consent by calling it.
#[cfg(test)]
pub fn lock_into(
    location: &Location,
    source: &Path,
    source_uid: u32,
    owner: Owner,
) -> Result<Vec<String>, String> {
    apply(location, &plan(location, source, source_uid)?, owner)
}

/// The directories ABOVE the protected one are the system's: judged, never
/// repaired. The path they lead to is returned.
fn system_dirs(location: &Location) -> Result<PathBuf, String> {
    let mut path = location.anchor.clone();
    check_dir(&path, location.trusted_uid)?;
    let Some((protected, above)) = location.dirs.split_last() else {
        return Err("the protected location names no directory".into());
    };
    for dir in above {
        path.push(dir);
        check_dir(&path, location.trusted_uid)?;
    }
    path.push(protected);
    Ok(path)
}

/// Install *plan* as *location*'s protected file owned by *owner*. The lines
/// say what changed.
pub fn apply(location: &Location, plan: &Plan, owner: Owner) -> Result<Vec<String>, String> {
    let path = system_dirs(location)?;
    let content = &plan.content;
    let mut lines = Vec::new();
    ensure_dir(&path, owner, &mut lines)?;
    let target = location.path();
    if lines.is_empty() && tree(location).as_deref() == Ok(content.as_str()) {
        return Ok(vec![NOTHING.to_string()]);
    }
    let existed = std::fs::symlink_metadata(&target).is_ok();
    write_file(&path, &target, content, owner)?;
    let verb = if existed { "replaced" } else { "installed" };
    lines.push(format!(
        "{verb} {} ({} {FILE_MODE:04o}) from {}",
        target.display(),
        owner_name(owner),
        plan.source.display()
    ));
    match tree(location) {
        Ok(text) if &text == content => {
            lines.push(format!(
                "verified: {} passes the guard's check",
                target.display()
            ));
            Ok(lines)
        }
        Ok(_) => Err(format!(
            "{} changed while it was installed",
            target.display()
        )),
        Err(why) => Err(format!("the installed file does not verify: {why}")),
    }
}

/// The source's text: a regular file *uid* owns, not a symlink, holding a
/// usable decision. No file at all is a decision that switches nothing off.
fn read_source(source: &Path, uid: u32) -> Result<String, String> {
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source);
    let file = match opened {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(EMPTY.to_string()),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(format!(
                "{} is a symlink; refusing to follow it as root",
                source.display()
            ))
        }
        Err(e) => return Err(format!("cannot open {}: {e}", source.display())),
    };
    let owned_by = file
        .metadata()
        .map_err(|e| format!("cannot stat {}: {e}", source.display()))?
        .uid();
    if owned_by != uid {
        return Err(format!(
            "{} is owned by uid {owned_by}, not by uid {uid} (SUDO_UID)",
            source.display()
        ));
    }
    let text =
        crate::hook::policy::read_open(file).map_err(|e| format!("{}: {e}", source.display()))?;
    Policy::parse(&text).map_err(|why| {
        format!(
            "{} is not usable ({why}); fix it and run again",
            source.display()
        )
    })?;
    Ok(text)
}

/// Create *dir* owned by *owner* and mode 0755, or bring an existing one
/// to that. A symlink, or anything not a directory, is refused.
fn ensure_dir(dir: &Path, owner: Owner, lines: &mut Vec<String>) -> Result<(), String> {
    let shown = dir.display();
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .mode(DIR_MODE)
                .create(dir)
                .map_err(|e| format!("cannot create {shown}: {e}"))?;
            set_owner_and_mode(&open_dir(dir)?, owner, DIR_MODE, dir)?;
            lines.push(format!(
                "created {shown} ({} {DIR_MODE:04o})",
                owner_name(owner)
            ));
        }
        Err(e) => return Err(format!("cannot inspect {shown}: {e}")),
        Ok(meta) if !meta.file_type().is_dir() => {
            return Err(format!(
                "{shown} is a symlink, or not a directory; remove it and run again"
            ))
        }
        Ok(meta) => {
            let right = meta.uid() == owner.uid
                && meta.gid() == owner.gid
                && meta.mode() & 0o7777 == DIR_MODE;
            if !right {
                set_owner_and_mode(&open_dir(dir)?, owner, DIR_MODE, dir)?;
                lines.push(format!(
                    "repaired {shown} to {} {DIR_MODE:04o}",
                    owner_name(owner)
                ));
            }
        }
    }
    Ok(())
}

fn open_dir(dir: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(dir)
        .map_err(|e| format!("cannot open {}: {e}", dir.display()))
}

/// Owner and mode, set through the open handle (`fchown`, `fchmod`), so
/// nothing is followed between a check and the change.
fn set_owner_and_mode(file: &File, owner: Owner, mode: u32, shown: &Path) -> Result<(), String> {
    fchown(file, Some(owner.uid), Some(owner.gid))
        .and_then(|()| file.set_permissions(std::fs::Permissions::from_mode(mode)))
        .map_err(|e| format!("cannot set the owner and mode of {}: {e}", shown.display()))
}

/// Write *content* beside *target*, then rename it into place: a reader
/// sees the old file or the new one, never half of one.
fn write_file(dir: &Path, target: &Path, content: &str, owner: Owner) -> Result<(), String> {
    let temp = dir.join(format!(
        ".{}.lock-{}",
        crate::hook::EXCEPTIONS_FILE,
        std::process::id()
    ));
    if std::fs::symlink_metadata(&temp).is_ok() {
        std::fs::remove_file(&temp)
            .map_err(|e| format!("cannot remove {}: {e}", temp.display()))?;
    }
    let failed = |e: std::io::Error| format!("cannot write {}: {e}", temp.display());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .map_err(failed)?;
    file.write_all(content.as_bytes()).map_err(failed)?;
    set_owner_and_mode(&file, owner, FILE_MODE, &temp)?;
    file.sync_all().map_err(failed)?;
    drop(file);
    std::fs::rename(&temp, target)
        .map_err(|e| format!("cannot move {} into place: {e}", temp.display()))?;
    // The rename is durable once the directory is.
    open_dir(dir)?
        .sync_all()
        .map_err(|e| format!("cannot sync {}: {e}", dir.display()))
}

fn owner_name(owner: Owner) -> String {
    if owner.uid == 0 && owner.gid == 0 {
        "root:root".into()
    } else {
        format!("{}:{}", owner.uid, owner.gid)
    }
}

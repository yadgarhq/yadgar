//! Writing a file that holds a credential: owner-only, atomic, never through
//! a link, and never into a directory another account owns (ledgers 1547,
//! 1553).
//!
//! Split out of `config.rs` at its size ceiling. The seam is the filesystem:
//! `config.rs` decides WHAT is stored, this file decides HOW it reaches disk.

use std::path::{Path, PathBuf};

/// Write *body* to `dir/name` atomically, owner-only, never through a link.
///
/// THE TEMPORARY FILE IS CREATED, NEVER OPENED (ledger 1553). `fs::write`
/// follows a symlink: a link planted at the old fixed `.config.json.tmp` sent
/// the bearer token into whatever it named, and the rename then made
/// `config.json` that link. The file is now made with `O_CREAT|O_EXCL` plus
/// `O_NOFOLLOW` at mode 0600: anything already at the path fails the create
/// rather than redirecting it, and the credential is never readable by anyone
/// else, not even for the instant before a chmod.
///
/// A NAME UNIQUE TO THIS WRITE (see [`temp_names`]), not a fixed one.
/// Several `serve` processes write the tool cache at once; with one shared
/// name, one would unlink or rename another's half-written file.
///
/// The fresh file is also the OWNER PROBE: it belongs to whoever this process
/// runs as, so a directory with a different owner is refused before a byte is
/// written or a mode changed. See [`refuse_foreign_owner`].
pub(crate) fn write_private(
    dir: &Path,
    name: &str,
    body: &[u8],
) -> Result<(), (PathBuf, std::io::Error)> {
    write_private_at(dir, name, body, temp_names(dir, &format!(".{name}")))
}

/// [`write_private`], trying the temporary names *candidates* in order.
///
/// Separate so a test can plant something at EXACTLY the name tried first.
pub(super) fn write_private_at(
    dir: &Path,
    name: &str,
    body: &[u8],
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<(), (PathBuf, std::io::Error)> {
    use std::io::Write as _;
    create_private_dir(dir).map_err(|e| (dir.to_path_buf(), e))?;
    let (tmp, mut file) = create_first_free(candidates).map_err(|e| (dir.to_path_buf(), e))?;
    if let Err(e) = check_owner(dir, &file).and_then(|()| tighten(dir)) {
        let _ = std::fs::remove_file(&tmp);
        return Err((dir.to_path_buf(), e));
    }
    if let Err(e) = file.write_all(body).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&tmp);
        return Err((tmp, e));
    }
    drop(file);
    let path = dir.join(name);
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        (path, e)
    })
}

/// How many temporary names one write tries before giving up.
///
/// Each name already taken is a crash leftover or a planted file; sixteen in a
/// row is not an accident to retry past.
const TEMP_ATTEMPTS: usize = 16; // ADR-0569-EXCEPTION(CB): bound on a retry loop over temporary names, not a per-install knob.

/// `<stem>.<pid>.<n>.tmp` in *dir*, [`TEMP_ATTEMPTS`] of them, lazily.
///
/// The pid keeps processes apart and the counter keeps one process's writes
/// apart. Neither is enough on its own after a crash: a container restart or a
/// short CLI run can reuse a pid, and the counter restarts at zero, so the
/// first name can already be taken by a leftover. [`create_first_free`] moves
/// past it.
pub(crate) fn temp_names(dir: &Path, stem: &str) -> impl Iterator<Item = PathBuf> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = dir.to_path_buf();
    let stem = stem.to_string();
    let pid = std::process::id();
    (0..TEMP_ATTEMPTS).map(move |_| {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        dir.join(format!("{stem}.{pid}.{n}.tmp"))
    })
}

/// Create the first of *candidates* that nothing occupies, through
/// [`create_exclusive`].
///
/// ONLY `AlreadyExists` moves on — a leftover file or a planted link, either
/// of which is left exactly as it was. Any other error is returned at once.
pub(crate) fn create_first_free(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    let mut last = None;
    for path in candidates {
        match create_exclusive(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "no temporary name was free",
        )
    }))
}

/// Make *dir* exist, owner-only (ledger 1547).
///
/// The file inside is 0600, and that protects nothing while the DIRECTORY is
/// group-writable: anyone who can write the directory can replace the file. It
/// followed the umask before — 0775 under umask 002.
///
/// **A DIRECTORY THIS CREATES is made 0700 whatever it is called**: nobody
/// else's permissions can be changed by creating it. An existing one is left
/// to [`tighten`]. Only the leaf: the parents are created with the default
/// mode, because a missing `~/.config` made 0700 here would be a surprise to
/// every other program that uses it.
#[cfg(unix)]
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if !dir.exists() {
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            // Lost a race with another `serve`: [`tighten`] runs next anyway.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Tighten an existing *dir* to 0700 — only when its leaf is `yaadgaar`.
///
/// That is the leaf [`super::base_dir`] builds, which covers the `./yaadgaar` fallback
/// for an OS that names no config directory and a `YAADGAAR_CONFIG_DIR` that
/// happens to end in `yaadgaar`. An override under any other name keeps the
/// permissions somebody gave it — and that now includes `~/.config/yadgar`, the
/// memory engine's directory. **A SYMLINK IS NEVER TIGHTENED**:
/// `set_permissions` follows the link, and its target belongs to whatever
/// declared it (home-manager, for one).
#[cfg(unix)]
fn tighten(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if dir.file_name().is_some_and(|n| n == super::DIR) {
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() {
            return Ok(());
        }
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Create *path* new, 0600, refusing an existing file or a symlink.
#[cfg(unix)]
pub(crate) fn create_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Refuse *dir* unless this process's user owns it, judged by *probe*: a file
/// this process just created there, and so owned by its effective uid.
///
/// A probe rather than `geteuid()`, which `libc` offers only as `unsafe` and
/// `unsafe_code = "forbid"` rules out. The directory's owner is read THROUGH a
/// symlink: what matters is who can replace files in the directory the
/// credential actually lands in.
#[cfg(unix)]
fn check_owner(dir: &Path, probe: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    refuse_foreign_owner(dir, std::fs::metadata(dir)?.uid(), probe.metadata()?.uid())
}

/// The decision [`check_owner`] makes, apart from the filesystem.
///
/// An existing directory KEPT ITS OWNER before (ledger 1553): one another
/// account created can have its files replaced by that account whatever mode
/// this sets, and running as root would chmod it for them.
#[cfg(unix)]
pub(super) fn refuse_foreign_owner(dir: &Path, owner: u32, me: u32) -> std::io::Result<()> {
    if owner == me {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!(
            "{} is owned by uid {owner}, not by this user (uid {me}); refusing to \
             store a credential where another account can replace it",
            dir.display()
        ),
    ))
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// No mode bits and no uid on Windows, and that is a real gap rather than a
/// shrug.
///
/// There is no chmod; the equivalent is an ACL, which needs a Windows-specific
/// crate this client does not carry. The file lands under the user's profile
/// directory, which is not world-readable by default — so the property holds by
/// where it sits rather than by what this sets, and that is a weaker guarantee.
/// Stated here so nobody later reads the empty functions as "nothing to do".
/// `create_new` still refuses an existing file at the temporary path.
#[cfg(not(unix))]
pub(crate) fn create_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(not(unix))]
fn tighten(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn check_owner(_dir: &Path, _probe: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

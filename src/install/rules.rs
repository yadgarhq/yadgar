//! The rules file yadgar owns, and the single line it writes into one it does not.
//!
//! Updating the instructions is a whole-file replace of a file with exactly one
//! owner. There are no markers and no section splice, because the Python version
//! spliced into a shared file and accumulated THREE marker conventions that
//! disagree with each other — `## Yadgar`, `## Memory System — Yadgar`, and a
//! legacy `<!-- YADGAR-RULES-BEGIN -->` — every one of them a way to clobber
//! prose somebody wrote by hand. An owned file has no markers to get wrong.
//!
//! Into `CLAUDE.md`, yadgar writes exactly one line: the `@` reference, first,
//! or nothing.

use std::path::Path;

use super::jsonfile::{remove_regular_file, write_atomic_text};

/// The owned file's name, beside `CLAUDE.md` in the same directory.
pub const RULES_FILE: &str = "yadgar-rules.md";

/// The instructions themselves, bundled into the binary.
///
/// Bundled rather than installed as an asset for the same reason the hooks are
/// one binary: a file on disk is a second copy, and a second copy diverges.
const RULES_BODY: &str = include_str!("assets/rules.md");

/// The one line written into `CLAUDE.md`.
///
/// Absolute, not `@~/…`: a `~` in an import line is expanded by some readers and
/// not others, and the same reference has to be correct whether `CLAUDE.md` sits
/// in `~/.claude` or in a repository.
pub fn reference_line(rules_path: &Path) -> String {
    format!("@{}", rules_path.display())
}

/// Refuse to write a `CLAUDE.md` that is not ours to write.
///
/// **A symlink is the case this exists for.** On a nix-managed machine
/// `~/.claude/CLAUDE.md` points into the store; the Python version replaced it
/// with a regular file and the machine silently drifted from its own declared
/// configuration — the next `home-manager switch` then either failed or quietly
/// reverted the install. `symlink_metadata`, not `metadata`: the latter follows
/// the link and reports a perfectly ordinary regular file, so the check would
/// never fire on the one machine it matters on.
pub fn check_reference_target(claude_md: &Path) -> anyhow::Result<()> {
    let meta = match std::fs::symlink_metadata(claude_md) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => anyhow::bail!("cannot inspect {}: {e}", claude_md.display()),
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(claude_md).unwrap_or_default();
        anyhow::bail!(
            "{} is a symlink (-> {}), so something else declares it — nix, most \
             likely. Replacing it with a regular file would drift this machine \
             from its own configuration.\n\
             Add this line to whatever generates it, then re-run:\n    {}",
            claude_md.display(),
            target.display(),
            reference_line(&claude_md.with_file_name(RULES_FILE)),
        );
    }
    if meta.permissions().readonly() {
        anyhow::bail!(
            "{} is read-only. Make it writable, or add this line to it yourself, \
             then re-run:\n    {}",
            claude_md.display(),
            reference_line(&claude_md.with_file_name(RULES_FILE)),
        );
    }
    // The mode bits and our own access are DIFFERENT QUESTIONS, and only the
    // second one is the one being asked. `readonly()` is `mode & 0o222 == 0`:
    // it reports whether the file has any write bit at all, not whether this
    // process may read and write it. A root-owned 0644 `CLAUDE.md` has write
    // bits and passes — and the write then fails after `write_body` has already
    // landed, which is the half-install this whole pre-flight block exists to
    // make impossible. A 0200 file reads as writable and cannot be read at all.
    //
    // So: probe both directions, for real. `write(true)` alone neither creates
    // nor truncates, so asking the question does not answer it destructively.
    let refusal = |verb: &str, e: std::io::Error| {
        anyhow::anyhow!(
            "cannot {verb} {}: {e}. Fix the permissions, or add this line to it \
             yourself, then re-run:\n    {}",
            claude_md.display(),
            reference_line(&claude_md.with_file_name(RULES_FILE)),
        )
    };
    std::fs::File::open(claude_md).map_err(|e| refusal("read", e))?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(claude_md)
        .map_err(|e| refusal("write", e))?;
    Ok(())
}

/// Refuse to overwrite a rules file that somebody else manages.
///
/// yadgar owns this file completely, which means it may only own a real one. A
/// symlink here would be somebody else's file wearing our name, and a whole-file
/// replace would write straight through it.
pub fn check_owned(rules_path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(rules_path) {
        Ok(m) if m.file_type().is_symlink() => anyhow::bail!(
            "{} is a symlink, but yadgar owns that file outright and would \
             overwrite whatever it points at. Remove the symlink and re-run.",
            rules_path.display()
        ),
        _ => Ok(()),
    }
}

/// Write the owned rules file: a whole-file replace. Returns whether it changed.
///
/// Not written at all when the body on disk is already exactly this one — the
/// same idempotence [`ensure_reference`] has, and for the second of its two
/// reasons: a `Summary` that says "installed the rules file" on a run that
/// wrote nothing is reporting work nobody did.
pub fn write_body(rules_path: &Path) -> anyhow::Result<bool> {
    if std::fs::read_to_string(rules_path).is_ok_and(|body| body == RULES_BODY) {
        return Ok(false);
    }
    write_atomic_text(rules_path, RULES_BODY)?;
    Ok(true)
}

/// Remove the owned rules file, if it is still ours. Returns whether it went.
pub fn remove_body(rules_path: &Path) -> anyhow::Result<bool> {
    if check_owned(rules_path).is_err() {
        return Ok(false);
    }
    remove_regular_file(rules_path)
}

/// Is the reference already somewhere in this file?
pub fn has_reference(claude_md: &Path, line: &str) -> anyhow::Result<bool> {
    Ok(read(claude_md)?.lines().any(|l| l.trim() == line))
}

/// Put the reference on the first line. Returns whether the file changed.
///
/// Idempotent in the strong sense: when the line is already first, the file is
/// not written at all — not rewritten identically. Rewriting would change the
/// mtime of a file yadgar does not own on every install, for no reason.
pub fn ensure_reference(claude_md: &Path, line: &str) -> anyhow::Result<bool> {
    let existing = read(claude_md)?;
    let desired = format!("{line}\n{}", without_line(&existing, line));
    if desired == existing {
        return Ok(false);
    }
    write_atomic_text(claude_md, &desired)?;
    Ok(true)
}

/// Take the reference back out. Returns whether the file changed.
///
/// A file whose ENTIRE content was yadgar's one line is deleted rather than
/// written back empty. Uninstall used to leave a zero-byte `CLAUDE.md` on every
/// machine that had none before install, which is not "removes exactly what
/// install added" (D76).
///
/// The condition is that nothing is left, not that yadgar created it: what
/// remains here is whitespace, so there is nothing of anybody's in the file to
/// lose. One line of somebody's prose and it is written back instead — this
/// module has already had one bug that replaced a person's `CLAUDE.md` with a
/// single line, and every judgement call in it errs the same way since.
///
/// WHITESPACE, not the empty string, and the difference is a whole byte that
/// somebody has to go and delete. The reference line followed by a blank line
/// left a `CLAUDE.md` holding `"\n"` behind — the same residue as before, one
/// newline short of the case that was fixed.
pub fn remove_reference(claude_md: &Path, line: &str) -> anyhow::Result<bool> {
    let existing = read(claude_md)?;
    let desired = without_line(&existing, line);
    if desired == existing {
        return Ok(false);
    }
    if desired.trim().is_empty() && remove_regular_file(claude_md)? {
        return Ok(true);
    }
    write_atomic_text(claude_md, &desired)?;
    Ok(true)
}

/// The file's text, or an error — NEVER an empty string standing in for one.
///
/// This used to end in `unwrap_or_default()`, and that one call was the worst
/// thing this program could do. A read failure became an empty string,
/// [`ensure_reference`] then built a file consisting of the reference line and
/// nothing else, and wrote it over the top: somebody's `CLAUDE.md` replaced by
/// a single `@` line, by an installer that went on to report success. A file
/// that cannot be read is a refusal.
fn read(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        // Absent is not unreadable — install is about to create the file.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => anyhow::bail!(
            "cannot read {}: {e}.\nRefusing to touch it: writing over a file \
             yadgar cannot read would destroy whatever is in there.",
            path.display()
        ),
    }
}

/// Everything except the reference line, byte-for-byte.
///
/// Line-endings and a missing final newline are preserved by splitting on
/// inclusive segments rather than re-joining: the rest of the file belongs to
/// somebody else and must come back out exactly as it went in.
fn without_line(text: &str, line: &str) -> String {
    text.split_inclusive('\n')
        .filter(|seg| seg.trim() != line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_goes_first_and_the_rest_survives_byte_for_byte() {
        let dir = crate::install::tests::scratch("rules-reference");
        let path = dir.join("CLAUDE.md");
        std::fs::write(&path, "# Mine\r\n\r\nprose with no trailing newline").unwrap();
        assert!(ensure_reference(&path, "@/opt/yadgar-rules.md").unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "@/opt/yadgar-rules.md\n# Mine\r\n\r\nprose with no trailing newline"
        );
    }

    #[test]
    fn a_second_install_does_not_rewrite_the_file() {
        // The failure this prevents: an install that appends its line again, or
        // touches a file it does not own on every run.
        let dir = crate::install::tests::scratch("rules-idempotent");
        let path = dir.join("CLAUDE.md");
        std::fs::write(&path, "existing\n").unwrap();
        assert!(ensure_reference(&path, "@/opt/rules.md").unwrap());
        assert!(!ensure_reference(&path, "@/opt/rules.md").unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "@/opt/rules.md\nexisting\n"
        );
    }

    /// A second install must not TOUCH `CLAUDE.md`, not merely leave it equal.
    ///
    /// `#[cfg(unix)]` for the reason `refusals.rs` states the rule for: this
    /// needs an inode, which Windows has no equivalent of. The inode is the
    /// discriminator rather than the mtime because it cannot flake on a clock
    /// or on a filesystem's timestamp granularity — `write_atomic_text` renames
    /// a fresh temp file over the target, so a rewrite changes the inode while
    /// leaving every byte the same.
    ///
    /// That is exactly why `installing_twice_is_byte_identical` cannot see
    /// this: it compares BYTES, and the property here is that a file yadgar
    /// does not own is not written to at all. Deleting the
    /// `if desired == existing { return Ok(false) }` early return left the
    /// suite green.
    #[cfg(unix)]
    #[test]
    fn a_second_install_does_not_touch_the_file_it_does_not_own() {
        use std::os::unix::fs::MetadataExt;
        let dir = crate::install::tests::scratch("rules-untouched");
        let path = dir.join("CLAUDE.md");
        std::fs::write(&path, "# my own instructions\n").unwrap();

        assert!(ensure_reference(&path, "@/opt/rules.md").unwrap());
        let first = std::fs::metadata(&path).unwrap().ino();

        assert!(!ensure_reference(&path, "@/opt/rules.md").unwrap());

        assert_eq!(
            std::fs::metadata(&path).unwrap().ino(),
            first,
            "a second install rewrote a file it does not own — the bytes are \
             the same, and the file is not"
        );
    }

    #[test]
    fn a_file_left_holding_only_blank_lines_is_removed_too() {
        // The residue this module set out to remove, in its nearest neighbour
        // case. A `CLAUDE.md` created by install and holding the reference line
        // plus one blank line came out of an uninstall as a one-byte file
        // holding `"\n"` — the emptied file D76 says must not be left behind,
        // one newline short of the case that was fixed.
        //
        // Whitespace is nothing in the sense this decision needs: there is no
        // prose in it to lose. ONE WORD of somebody's writing and the file is
        // written back instead, which the second half asserts.
        let dir = crate::install::tests::scratch("rules-blank-residue");

        let residue = dir.join("CLAUDE.md");
        std::fs::write(&residue, "@/opt/rules.md\n\n").unwrap();
        assert!(remove_reference(&residue, "@/opt/rules.md").unwrap());
        assert!(
            !residue.exists(),
            "a file holding one newline was left behind: {:?}",
            std::fs::read_to_string(&residue)
        );

        let theirs = dir.join("theirs.md");
        std::fs::write(&theirs, "@/opt/rules.md\n\nmine\n").unwrap();
        assert!(remove_reference(&theirs, "@/opt/rules.md").unwrap());
        assert_eq!(
            std::fs::read_to_string(&theirs).unwrap(),
            "\nmine\n",
            "somebody's own prose was deleted with the reference line"
        );
    }

    /// Pins `read` itself, not the pre-flight check that also catches this.
    ///
    /// `install` refuses an unreadable `CLAUDE.md` twice over: once in
    /// `check_reference_target`, before anything is written, and once here.
    /// The end-to-end test can only ever see the first of them, so removing
    /// this one — going back to `unwrap_or_default()` — was invisible through
    /// the public path. Two independent defenses need two independent tests,
    /// or one of them is decoration.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_an_error_rather_than_an_empty_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::install::tests::scratch("rules-unreadable");
        let path = dir.join("CLAUDE.md");
        let theirs = "# my own instructions\n";
        std::fs::write(&path, theirs).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
        if std::fs::read_to_string(&path).is_ok() {
            // Root, or a filesystem that ignores the mode: nothing to test.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            return;
        }

        let outcome = ensure_reference(&path, "@/opt/yadgar-rules.md");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(outcome.is_err(), "an unreadable file was treated as empty");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            theirs,
            "the person's instructions were overwritten"
        );
    }

    #[test]
    fn a_reference_further_down_is_moved_to_the_front_not_duplicated() {
        let dir = crate::install::tests::scratch("rules-move");
        let path = dir.join("CLAUDE.md");
        std::fs::write(&path, "first\n@/opt/rules.md\nlast\n").unwrap();
        ensure_reference(&path, "@/opt/rules.md").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "@/opt/rules.md\nfirst\nlast\n"
        );
    }
}

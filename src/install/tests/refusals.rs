//! Every way `install` must refuse before it writes a byte.
//!
//! Split out of one file because the repository caps a file at 500 lines
//! (D59's complexity gate); the helpers and fixtures live in the parent.
//!
//! **Where the coverage stops, stated as a property so it cannot go stale.**
//! A test is `#[cfg(unix)]` here if and ONLY if it needs a permission mode
//! Windows has no equivalent for — a file that may be written and not read, or
//! read and not written. Those do not exist on Windows rather than failing
//! there. Everything else is platform-independent and carries no `#[cfg]`,
//! which is also why [`super::require_symlink`] fails instead of returning:
//! three refusals WERE gated on a client that ships to Windows, and four more
//! skipped silently, so Windows had no refusal coverage at all while the suite
//! read green. Adding a `#[cfg(unix)]` for any other reason narrows the
//! platform this client is actually tested on, and the reason belongs in the
//! test.
//!
//! One silent skip is left, and it is a precondition rather than a platform:
//! each of those tests returns early when it finds it can read or write the
//! file it just restricted — running as root, or on a filesystem that ignores
//! the mode. The case genuinely cannot be built there, and failing would turn
//! every root CI container red.

use std::path::Path;

use super::super::{hooks, install_with, mcp, uninstall, Layout};
use super::*;

#[test]
fn a_symlinked_claude_md_is_refused_before_anything_is_written() {
    // The failure this prevents, and it is the case on the machine this was
    // written on: ~/.claude/CLAUDE.md is a nix store symlink. The Python version
    // replaced it with a regular file, drifting the machine from its declared
    // configuration. Refusing LATE would be nearly as bad — the hooks and the
    // MCP entry would already be in place, a half-install nobody asked for.
    let home = scratch("symlink-refusal");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let target = home.join("nix-store-CLAUDE.md");
    std::fs::write(&target, "# managed elsewhere\n").unwrap();
    require_symlink(&target, &layout.claude_md());

    let err = install_with(&home, Path::new(BINARY))
        .unwrap_err()
        .to_string();
    assert!(err.contains("symlink"), "{err}");

    assert!(
        !layout.settings().exists(),
        "hooks were written despite the refusal"
    );
    assert!(
        !layout.mcp_config().exists(),
        "the MCP entry was written despite the refusal"
    );
    assert!(
        !layout.rules().exists(),
        "the rules file was written despite the refusal"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "# managed elsewhere\n"
    );
}

#[test]
fn a_read_only_claude_md_is_refused() {
    let home = scratch("readonly-refusal");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    std::fs::write(layout.claude_md(), "# theirs\n").unwrap();
    // The refusal reads the mode bits rather than attempting a write, so it
    // fires for root too — a test that only passes as an unprivileged user is a
    // test that stops running the day CI switches to a root container.
    let original = make_read_only(&layout.claude_md());

    let outcome = install_with(&home, Path::new(BINARY));

    std::fs::set_permissions(layout.claude_md(), original).unwrap();
    let err = outcome.unwrap_err().to_string();
    assert!(err.contains("read-only"), "{err}");
    assert!(!layout.settings().exists());
}

#[test]
fn an_unparseable_settings_file_stops_the_install() {
    // The failure this prevents: the Python loader turned a parse error into an
    // empty dict and then WROTE it, destroying every setting in the file (D75 —
    // never clobber a config that cannot be parsed).
    let home = scratch("unparseable");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let broken = "{ \"hooks\": oops }";
    std::fs::write(layout.settings(), broken).unwrap();

    let err = install_with(&home, Path::new(BINARY))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not valid JSON"), "{err}");
    assert_eq!(std::fs::read_to_string(layout.settings()).unwrap(), broken);
    assert!(
        !layout.rules().exists(),
        "the install wrote before it had finished refusing"
    );
}

#[cfg(unix)]
#[test]
fn a_claude_md_this_process_cannot_write_is_refused_though_its_mode_bits_say_writable() {
    // The failure this prevents, and the reason the pre-flight PROBES rather
    // than reading mode bits: `permissions().readonly()` is `mode & 0o222 == 0`
    // — it answers "does this file carry any write bit", not "may this process
    // write it". A root-owned 0644 CLAUDE.md sails through the mode check, and
    // the write then fails AFTER `write_body` has landed: the half-install the
    // whole pre-flight block exists to make impossible. Deleting the write
    // probe left the suite green.
    //
    // Mode 0466 is that case without needing root. Group and other may write,
    // so `readonly()` is false; the owner may read and not write, so only an
    // actual open can tell.
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("unwritable-claude-md");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let theirs = "# my own instructions\n";
    std::fs::write(layout.claude_md(), theirs).unwrap();
    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o466)).unwrap();
    if std::fs::OpenOptions::new()
        .write(true)
        .open(layout.claude_md())
        .is_ok()
    {
        // Root, or a filesystem that ignores the mode. There is no unwritable
        // file to test with, and the install is right to proceed.
        std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        return;
    }

    let outcome = install_with(&home, Path::new(BINARY));

    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = outcome.unwrap_err().to_string();
    assert!(err.contains("cannot write"), "{err}");
    assert_eq!(std::fs::read_to_string(layout.claude_md()).unwrap(), theirs);
    assert!(
        !layout.rules().exists(),
        "the rules file was written before the install had finished refusing"
    );
    assert!(
        !layout.settings().exists(),
        "hooks were written despite the refusal"
    );
}

// ─── The rules file, and the file yadgar does not own ────────────────────────

#[cfg(unix)]
#[test]
fn an_unreadable_claude_md_is_refused_rather_than_replaced() {
    // The worst thing this program can do. `read()` ended in
    // `unwrap_or_default()`, so a read failure became an empty string and the
    // reference line was written over the top — somebody's instructions deleted
    // by an installer that then reported success.
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("unreadable-claude-md");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let theirs = "# my own instructions\nline two\n";
    std::fs::write(layout.claude_md(), theirs).unwrap();
    // Write-only: the mode bits say writable, so a `readonly()` pre-flight
    // waves it through and only the read fails.
    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o200)).unwrap();

    if std::fs::read_to_string(layout.claude_md()).is_ok() {
        // Running as root, or on a filesystem that ignores the mode. There is
        // no unreadable file to test with here, and the install is right to
        // proceed — so this asserts nothing rather than asserting something
        // false.
        std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        return;
    }

    let outcome = install_with(&home, Path::new(BINARY));

    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = outcome.unwrap_err().to_string();
    assert!(err.contains("read"), "{err}");
    assert_eq!(
        std::fs::read_to_string(layout.claude_md()).unwrap(),
        theirs,
        "the person's instructions were destroyed"
    );
    assert!(
        !layout.settings().exists(),
        "hooks were written despite the refusal"
    );
    assert!(
        !layout.rules().exists(),
        "the rules file was written despite the refusal"
    );
}

#[test]
fn a_symlinked_rules_file_is_refused_and_never_written_through() {
    // The failure this prevents: `write_body` is a whole-file replace, so a
    // symlink wearing our name means yadgar overwrites somebody else's file.
    // `check_owned` is the only thing standing between the two, and it had no
    // test at all — making it a no-op left the suite green.
    let home = scratch("rules-symlink");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let target = home.join("somebody-elses.md");
    std::fs::write(&target, "# not ours\n").unwrap();
    require_symlink(&target, &layout.rules());

    let err = install_with(&home, Path::new(BINARY))
        .unwrap_err()
        .to_string();

    assert!(err.contains("symlink"), "{err}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# not ours\n");
    assert!(
        !layout.settings().exists(),
        "hooks were written despite the refusal"
    );
    assert!(
        !layout.mcp_config().exists(),
        "the MCP entry was written despite the refusal"
    );
}

#[test]
fn an_uninstall_leaves_a_symlinked_rules_file_alone() {
    // `remove_body` refuses through the same check: deleting the link would
    // take somebody else's file out of their own configuration.
    let home = scratch("rules-symlink-uninstall");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let target = home.join("somebody-elses.md");
    std::fs::write(&target, "# not ours\n").unwrap();
    require_symlink(&target, &layout.rules());

    uninstall(&home).unwrap();

    assert!(
        layout.rules().exists(),
        "uninstall removed a rules file it does not own"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# not ours\n");
}

#[cfg(unix)]
#[test]
fn an_unreadable_claude_md_does_not_stop_an_uninstall() {
    // The failure this prevents, and it was introduced by the fix for the
    // install side: making the reference check fail loudly turned an
    // unreadable `CLAUDE.md` into a machine yadgar cannot be removed from at
    // all. The hooks and the MCP entry come out without ever reading that
    // file, so they must come out; the one thing that genuinely needs it is
    // reported at the end, after everything removable has gone.
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("uninstall-unreadable");
    let layout = Layout::new(&home);
    install_with(&home, Path::new(BINARY)).unwrap();
    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o200)).unwrap();
    if std::fs::read_to_string(layout.claude_md()).is_ok() {
        std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        return; // Root, or a filesystem that ignores the mode.
    }

    let outcome = uninstall(&home);

    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        outcome.is_err(),
        "the reference line is still in a file yadgar cannot read, and nothing said so"
    );
    assert_no_managed_hooks(&layout.settings());
    assert_no_mcp_entry(&layout.mcp_config());
}

#[test]
fn a_symlinked_claude_md_does_not_stop_an_uninstall() {
    // The same failure as the test above, on the machine this whole module was
    // written for. `~/.claude/CLAUDE.md` here is a nix store symlink, so a
    // pre-flight `check_reference_target(&claude_md)?` at the TOP of `uninstall`
    // returned before `hooks::strip` ever ran: twelve hooks and the MCP entry
    // stayed live, and yadgar could not be removed from the machine at all
    // because of a file it was only trying to tidy.
    //
    // The symlink is still never written through — that is the refusal install
    // exists for — it is only reported at the END, once everything removable is
    // already gone, so re-running after taking the line out of whatever
    // generates that file finishes the job.
    let home = scratch("uninstall-symlinked");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();

    // Everything install would have written, written by hand instead — because
    // on this machine `install` refuses, and the person adds the reference to
    // whatever generates `CLAUDE.md`, exactly as the refusal tells them to.
    let store = home.join("nix-store-CLAUDE.md");
    let declared = format!("# declared elsewhere\n@{}\n", layout.rules().display());
    std::fs::write(&store, &declared).unwrap();
    require_symlink(&store, &layout.claude_md());
    std::fs::write(layout.rules(), "# rules").unwrap();
    let mut settings = serde_json::json!({});
    hooks::merge(&mut settings, Path::new(BINARY));
    std::fs::write(layout.settings(), settings.to_string()).unwrap();
    let mut config = serde_json::json!({});
    mcp::merge(&mut config, Path::new(BINARY));
    std::fs::write(layout.mcp_config(), config.to_string()).unwrap();

    let outcome = uninstall(&home);

    assert!(
        outcome.is_err(),
        "the reference line is still in a file yadgar may not write, and nothing said so"
    );
    assert_no_managed_hooks(&layout.settings());
    assert_no_mcp_entry(&layout.mcp_config());
    assert!(
        !layout.rules().exists(),
        "the owned rules file was left behind"
    );
    // And the file yadgar does not own came through untouched, symlink included.
    assert!(
        std::fs::symlink_metadata(layout.claude_md())
            .unwrap()
            .file_type()
            .is_symlink(),
        "the symlink was replaced with a regular file"
    );
    assert_eq!(std::fs::read_to_string(&store).unwrap(), declared);
}

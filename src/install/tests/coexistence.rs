//! Running beside the yadgar memory engine on one machine.
//!
//! The engine registers its own MCP server under `yadgar` in `~/.claude.json`,
//! and on a nix-managed machine `~/.claude/CLAUDE.md` is a store symlink. Both
//! belong to somebody else. These tests pin that this client neither writes
//! nor removes them.

use std::path::Path;

use super::super::{install_with, uninstall, Layout};
use super::*;

/// The engine's entry as it sits in a real `~/.claude.json`, with no install of
/// this client before it. The token is a fake.
const ENGINE_ONLY: &str = r#"{"numStartups":1,"mcpServers":{"yadgar":{"type":"http","url":"http://127.0.0.1:8765/mcp","headers":{"Authorization":"Bearer fake-test-token"}}}}"#;

#[cfg(unix)]
#[test]
fn uninstall_never_removes_a_foreign_yadgar_http_entry() {
    // Measured on 0.1.3 (the plan's run S4): `yaadgaar uninstall` on a machine
    // that had never installed it deleted the engine's `mcpServers.yadgar`
    // entry and left `{"numStartups":1,"mcpServers":{}}`. An uninstall that
    // removed nothing of ours must not WRITE the file at all — equal content
    // after a rewrite would still have reformatted a file we do not own.
    use std::os::unix::fs::MetadataExt;
    let home = scratch("coexist-uninstall-engine-entry");
    let layout = Layout::new(&home);
    std::fs::write(layout.mcp_config(), ENGINE_ONLY).unwrap();
    let before = std::fs::metadata(layout.mcp_config()).unwrap();

    uninstall(&home).unwrap();

    let after = std::fs::metadata(layout.mcp_config())
        .expect("uninstall deleted a ~/.claude.json holding the engine's entry");
    assert_eq!(after.ino(), before.ino(), "~/.claude.json was rewritten");
    assert_eq!(
        (after.mtime(), after.mtime_nsec()),
        (before.mtime(), before.mtime_nsec()),
        "~/.claude.json was written"
    );
    assert_eq!(
        std::fs::read_to_string(layout.mcp_config()).unwrap(),
        ENGINE_ONLY
    );
}

/// What a file looked like, to prove it was not written: bytes plus, on Unix,
/// inode and mtime — `write_atomic_text` renames a temp file over its target,
/// so even a byte-identical rewrite changes the inode.
#[derive(Debug, PartialEq)]
struct Snapshot {
    bytes: Vec<u8>,
    #[cfg(unix)]
    stamp: (u64, i64, i64),
}

fn snapshot(path: &Path) -> Snapshot {
    #[cfg(unix)]
    let stamp = {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).unwrap();
        (m.ino(), m.mtime(), m.mtime_nsec())
    };
    Snapshot {
        bytes: std::fs::read(path).unwrap(),
        #[cfg(unix)]
        stamp,
    }
}

/// A nix-shaped home: `~/.claude/CLAUDE.md` a symlink to a store file whose
/// text *declared* builds from the exact reference line. Returns the home and
/// the store file the link points at.
fn nix_home(
    name: &str,
    declared: impl FnOnce(&str) -> String,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = scratch(name);
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let store = home.join("nix-store-CLAUDE.md");
    let line = format!("@{}", layout.rules().display());
    std::fs::write(&store, declared(&line)).unwrap();
    require_symlink(&store, &layout.claude_md());
    (home, store)
}

fn assert_link_untouched(layout: &Layout, store: &Path, before: &Snapshot) {
    let meta = std::fs::symlink_metadata(layout.claude_md()).unwrap();
    assert!(
        meta.file_type().is_symlink(),
        "the symlink was replaced with a regular file"
    );
    assert_eq!(std::fs::read_link(layout.claude_md()).unwrap(), store);
    assert_eq!(
        &snapshot(store),
        before,
        "the file behind the symlink was written"
    );
}

#[test]
fn a_symlinked_claude_md_that_already_references_the_rules_installs_and_is_untouched() {
    // Max's machine: `CLAUDE.md` is a nix store symlink whose first line is
    // `@AGENTS.md`, and nix carries the reference further down. Install used
    // to refuse on the symlink alone, so it could not complete there at all.
    let (home, store) = nix_home("coexist-symlink-referenced", |line| {
        format!("@AGENTS.md\n\n# rules\n{line}\n")
    });
    let layout = Layout::new(&home);
    let before = snapshot(&store);

    let summary = install_with(&home, Path::new(BINARY)).unwrap();

    assert_link_untouched(&layout, &store, &before);
    assert!(summary.notices.is_empty(), "{:#?}", summary.notices);
    assert!(!summary.claude_md_changed);
    assert!(layout.settings().exists(), "the hooks were not installed");
    assert!(
        layout.mcp_config().exists(),
        "the MCP entry was not installed"
    );
    assert!(layout.rules().exists(), "the rules file was not installed");
}

#[test]
fn a_symlinked_claude_md_without_the_reference_installs_and_names_the_line() {
    // Never edited: replacing the link with a regular file drifts the machine
    // from its own declared configuration. Not a failure either — everything
    // this client owns is in place, and the one missing line is the person's
    // to add, so the notice names it exactly.
    let (home, store) = nix_home("coexist-symlink-missing", |_| "@AGENTS.md\n".into());
    let layout = Layout::new(&home);
    let before = snapshot(&store);

    let summary = install_with(&home, Path::new(BINARY)).unwrap();

    assert_link_untouched(&layout, &store, &before);
    let line = format!("@{}", layout.rules().display());
    assert!(
        summary
            .notices
            .iter()
            .any(|n| n.lines().any(|l| l.trim() == line)),
        "no notice names the exact line {line}: {:#?}",
        summary.notices
    );
    assert!(!summary.claude_md_changed);
    assert!(layout.settings().exists() && layout.mcp_config().exists() && layout.rules().exists());
}

#[test]
fn a_commented_out_reference_does_not_count() {
    // Relaxing the pre-flight to "the line is somewhere" must not accept a
    // line that Claude Code will not import. Exact line, nothing else.
    let (home, store) = nix_home("coexist-commented", |line| {
        format!("<!-- {line} -->\n# {line}\n`{line}`\n")
    });
    let layout = Layout::new(&home);
    let before = snapshot(&store);

    let summary = install_with(&home, Path::new(BINARY)).unwrap();

    assert_link_untouched(&layout, &store, &before);
    assert!(
        !summary.notices.is_empty(),
        "a commented-out reference was taken for the real one"
    );
}

#[test]
fn a_reference_not_on_the_first_line_is_accepted_and_not_moved() {
    // `verify` already accepts the line anywhere (health.rs, check_rules); an
    // install that then moved it to the top rewrote a file it does not own on
    // every run, against wherever the person had chosen to put it.
    let home = scratch("coexist-reference-further-down");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let text = format!("@AGENTS.md\n\n@{}\nmine\n", layout.rules().display());
    std::fs::write(layout.claude_md(), &text).unwrap();
    let before = snapshot(&layout.claude_md());

    let summary = install_with(&home, Path::new(BINARY)).unwrap();

    assert_eq!(
        snapshot(&layout.claude_md()),
        before,
        "CLAUDE.md was written"
    );
    assert!(!summary.claude_md_changed);
}

#[test]
fn a_read_only_claude_md_that_already_references_the_rules_installs() {
    // Read-only and already carrying the line: there is nothing to write, so
    // there is nothing to refuse.
    let home = scratch("coexist-readonly-referenced");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    std::fs::write(
        layout.claude_md(),
        format!("# mine\n@{}\n", layout.rules().display()),
    )
    .unwrap();
    let original = make_read_only(&layout.claude_md());
    let before = snapshot(&layout.claude_md());

    let outcome = install_with(&home, Path::new(BINARY));

    let after = snapshot(&layout.claude_md());
    std::fs::set_permissions(layout.claude_md(), original).unwrap();
    outcome.unwrap();
    assert_eq!(after, before, "CLAUDE.md was written");
}

#[test]
fn uninstall_on_a_symlinked_claude_md_succeeds_with_a_notice() {
    // `install.rs` used to end uninstall with an error here, after removing
    // everything else — so a complete uninstall on a nix machine exited
    // non-zero. The line belongs to whatever generates the file; say which
    // line to take out, and exit 0.
    let (home, store) = nix_home("coexist-uninstall-symlink", |line| {
        format!("# nix\n{line}\n")
    });
    let layout = Layout::new(&home);
    let line = format!("@{}", layout.rules().display());
    install_with(&home, Path::new(BINARY)).unwrap();
    let before = snapshot(&store);

    let summary = uninstall(&home).unwrap();

    assert_link_untouched(&layout, &store, &before);
    assert!(
        summary
            .notices
            .iter()
            .any(|n| n.lines().any(|l| l.trim() == line)),
        "{:#?}",
        summary.notices
    );
    assert!(!summary.claude_md_changed);
    assert!(!layout.rules().exists(), "the rules file was left behind");
}

/// The keys of the object at *pointer* in the JSON file at *path*, in file order.
fn keys_at(path: &Path, pointer: &str) -> Vec<String> {
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_object)
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

// KEY ORDER. `serde_json` is built with `preserve_order`, where `Map::remove`
// is `swap_remove`: it moves the LAST key into the removed key's slot. Every
// removal of ours from a map that is not at its end therefore reordered keys
// in a file somebody else owns.

#[test]
fn uninstall_keeps_the_order_of_the_other_mcp_servers() {
    let home = scratch("coexist-order-uninstall");
    let layout = Layout::new(&home);
    let config = serde_json::json!({ "numStartups": 1, "mcpServers": {
        "alpha": { "type": "http", "url": "http://a.test/mcp" },
        "yaadgaar": { "type": "stdio", "command": BINARY, "args": ["serve"] },
        "beta": { "type": "http", "url": "http://b.test/mcp" },
        "gamma": { "type": "http", "url": "http://c.test/mcp" },
    }});
    std::fs::write(layout.mcp_config(), config.to_string()).unwrap();

    uninstall(&home).unwrap();

    assert_eq!(
        keys_at(&layout.mcp_config(), "/mcpServers"),
        ["alpha", "beta", "gamma"]
    );
}

#[test]
fn the_legacy_migration_keeps_the_order_of_the_other_mcp_servers() {
    let home = scratch("coexist-order-legacy");
    let layout = Layout::new(&home);
    let config = serde_json::json!({ "numStartups": 1, "mcpServers": {
        "yadgar": { "type": "stdio", "command": BINARY, "args": ["serve"] },
        "alpha": { "type": "http", "url": "http://a.test/mcp" },
        "beta": { "type": "http", "url": "http://b.test/mcp" },
    }});
    std::fs::write(layout.mcp_config(), config.to_string()).unwrap();

    install_with(&home, Path::new(BINARY)).unwrap();

    assert_eq!(
        keys_at(&layout.mcp_config(), "/mcpServers"),
        ["alpha", "beta", "yaadgaar"]
    );
}

#[test]
fn removing_an_emptied_hook_event_keeps_the_order_of_the_others() {
    let home = scratch("coexist-order-hooks");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let ours = |name: &str| {
        serde_json::json!([{ "matcher": "", "hooks": [{ "type": "command",
            "command": format!("{BINARY} hook {name}") }] }])
    };
    let foreign = serde_json::json!([{ "matcher": "", "hooks": [{ "type": "command",
        "command": "/usr/local/bin/audit-shell record" }] }]);
    let settings = serde_json::json!({ "model": "opus", "hooks": {
        "PreCompact": ours("pre-compact"),
        "SessionStart": foreign.clone(),
        "Stop": foreign.clone(),
        "Notification": foreign,
    }});
    std::fs::write(layout.settings(), settings.to_string()).unwrap();

    uninstall(&home).unwrap();

    assert_eq!(
        keys_at(&layout.settings(), "/hooks"),
        ["SessionStart", "Stop", "Notification"]
    );
}

/// Make *store* unreadable (mode 000), or FAIL saying why that is impossible.
///
/// A FAILURE, not a skip. These tests used to `return` green when the process
/// could read the file anyway — as root, or on a filesystem that ignores the
/// mode — so a root CI reported them passing having asserted nothing. A
/// precondition that cannot be met is a failure, the same rule
/// `require_symlink` states in `mod.rs`.
#[cfg(unix)]
fn make_unreadable(store: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(store, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(store).is_ok() {
        std::fs::set_permissions(store, std::fs::Permissions::from_mode(0o644)).unwrap();
        panic!(
            "{} is still readable at mode 000: this process runs as root or the \
             filesystem ignores permissions, so this test cannot check anything. \
             Run the suite as an ordinary user.",
            store.display()
        );
    }
}

#[cfg(unix)]
fn restore_readable(store: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(store, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// The notice for a target that could not be read: names the target and the
/// exact line, and does not claim the line is missing. *check* is what the
/// person is told to verify, which differs between install and uninstall.
#[cfg(unix)]
fn assert_unreadable_notice(notices: &[String], store: &Path, check: &str) {
    let expected = format!("could not read {}; {check}", store.display());
    assert!(
        notices.iter().any(|n| n.contains(&expected)),
        "no notice says {expected:?}: {notices:#?}"
    );
    assert!(
        !notices.iter().any(|n| n.contains("Add this line")),
        "an unreadable target was reported as missing the line: {notices:#?}"
    );
}

#[cfg(unix)]
#[test]
fn install_on_an_unreadable_symlink_target_says_it_could_not_read_it() {
    let (home, store) = nix_home("coexist-unreadable-install", |line| format!("{line}\n"));
    let layout = Layout::new(&home);
    let line = format!("@{}", layout.rules().display());
    make_unreadable(&store);

    let outcome = install_with(&home, Path::new(BINARY));

    restore_readable(&store);
    let summary = outcome.expect("an unreadable symlink target must not fail install");
    assert_unreadable_notice(
        &summary.notices,
        &store,
        &format!("check that it contains {line}"),
    );
    assert!(std::fs::symlink_metadata(layout.claude_md())
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn uninstall_on_an_unreadable_symlink_target_says_it_could_not_read_it() {
    let (home, store) = nix_home("coexist-unreadable-uninstall", |line| format!("{line}\n"));
    let layout = Layout::new(&home);
    let line = format!("@{}", layout.rules().display());
    install_with(&home, Path::new(BINARY)).unwrap();
    make_unreadable(&store);

    let outcome = uninstall(&home);

    restore_readable(&store);
    let summary = outcome.expect("an unreadable symlink target must not fail uninstall");
    // After an uninstall the rules file is GONE, so the line must come out:
    // "check that it contains" would tell somebody to keep an import of a file
    // that no longer exists.
    assert_unreadable_notice(
        &summary.notices,
        &store,
        &format!("check that it no longer contains {line}: the rules file it imports was deleted"),
    );
    assert!(
        !summary
            .notices
            .iter()
            .any(|n| n.contains("check that it contains")),
        "uninstall told somebody to keep the line: {:#?}",
        summary.notices
    );
    assert!(!layout.rules().exists(), "the rules file was left behind");
}

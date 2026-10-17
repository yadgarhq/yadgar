//! Running beside the yadgar memory engine on one machine.
//!
//! The engine registers its own MCP server under `yadgar` in `~/.claude.json`,
//! and on a nix-managed machine `~/.claude/CLAUDE.md` is a store symlink. Both
//! belong to somebody else. These tests pin that this client neither writes
//! nor removes them.

use super::super::{uninstall, Layout};
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

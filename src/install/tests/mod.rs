//! End-to-end tests for install / uninstall / verify.
//!
//! Every one of these pins a failure that actually happened. They never touch
//! the real `~/.claude`: the whole install is parameterised on a home directory
//! precisely so a test can be given a scratch one, and a test that wrote into
//! somebody's live settings would have already failed at the thing this module
//! is for.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::Layout;

/// A binary path that is durable-looking and does not exist.
///
/// Durability and existence are checked by `verify`, not by `install` — and
/// separating them is what lets these tests register a plausible path without a
/// real binary. The path checks have their own tests.
const BINARY: &str = "/usr/local/bin/yaadgaar";

/// A fresh scratch home directory.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("yadgar-install-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn seed(home: &Path, settings: Value) {
    let layout = Layout::new(home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    std::fs::write(
        layout.settings(),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Make a symlink at *link* pointing at *target*, on whichever platform.
///
/// Returns the error rather than panicking so [`require_symlink`] can turn it
/// into a message that names the precondition. Creating a symlink on Windows
/// needs developer mode or an elevated process; the REFUSALS these guard are
/// platform-independent, which is why the tests below are no longer
/// `#[cfg(unix)]`. Three of them were, on a client that ships to Windows, so
/// Windows had no refusal coverage at all. The one gate left is the
/// unreadable-`CLAUDE.md` test, which needs a mode Windows has no equivalent
/// for: a file that may be written and not read.
#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

/// Make the symlink a test is about, or FAIL saying why it could not.
///
/// **A precondition that cannot be met is a failure, not a pass.** Four tests
/// used to `return` here with no assertions behind them, so on Windows without
/// developer mode they reported green having asserted nothing at all — on the
/// one platform `binary_stem` and `directory_names` were written for, and the
/// platform with no other refusal coverage. A green tick that means "this
/// machine could not run the test" is the same lie as a health check that only
/// ever prints reassuring things, which is the whole of D76.
///
/// Making one on Windows needs developer mode or an elevated process. That is
/// a thing to fix on the machine, and the message says so.
fn require_symlink(target: &Path, link: &Path) {
    if let Err(e) = symlink_file(target, link) {
        panic!(
            "cannot create the symlink this test is about ({} -> {}): {e}\n\
             On Windows this needs developer mode or an elevated process. \
             Refusing to report green having asserted nothing.",
            link.display(),
            target.display(),
        );
    }
}

/// Make *path* unwritable, and give back what it takes to undo that.
///
/// `set_readonly` is the one permission idea both platforms share: on Windows
/// it is the read-only attribute, on Unix it clears every write bit. Restoring
/// afterwards matters — Windows cannot delete a read-only file, so a scratch
/// directory left behind this way survives the next run's cleanup.
fn make_read_only(path: &Path) -> std::fs::Permissions {
    let original = std::fs::metadata(path).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(path, readonly).unwrap();
    original
}

/// A `settings.json` the size of a real one, with every foreign shape in it.
///
/// The fixture this replaces had two hook entries and two top-level keys, and
/// it was too small to notice two whole classes of regression. The real
/// `~/.claude/settings.json` on the machine this was written on has 21
/// top-level keys and 13 hook entries across 11 events: at that size a
/// reordering of the keys is visible in a diff, and an entry carrying two hooks
/// actually occurs. Both are pinned below.
fn foreign_settings() -> Value {
    json!({
        "model": "opus",
        "hooks": {
            "SessionStart": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "find /nix/store/caveman -name caveman.md -exec cat {} +" }] }
            ],
            "PostToolUse": [
                // A command that MENTIONS yadgar without being one of ours: the
                // Python install's own entry. Identity is the invocation shape,
                // not a substring, so this is foreign and must survive.
                { "matcher": "", "hooks": [{ "type": "command", "command": "/home/x/.local/pipx/venvs/yadgar/bin/python /home/x/.claude/hooks/hook_runner.py post-tool-capture" }] },
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "/usr/local/bin/audit-shell record" }] }
            ],
            "Stop": [
                // ONE ENTRY, TWO HOOKS, foreign first. Claude Code's schema
                // allows this and it is the case that broke identity: an
                // installer reading `hooks[0]` calls the whole entry foreign
                // and leaves the stale yadgar hook inside it, live.
                { "matcher": "", "hooks": [
                    { "type": "command", "command": "/usr/bin/other-tool run" },
                    { "type": "command", "command": "/old/path/yaadgaar hook stop-checkpoint" }
                ]},
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/local/bin/notify done" }] }
            ],
            "PreToolUse": [
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "/usr/local/bin/guard check" }] }
            ],
            "PreCompact": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/local/bin/archive-transcript" }] }
            ],
            "Notification": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/bin/notify-send claude" }] }
            ],
            "SubagentStop": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/local/bin/tally subagent" }] }
            ],
            "SessionEnd": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/local/bin/flush-metrics" }] }
            ],
            "InstructionsLoaded": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/local/bin/print-banner" }] }
            ],
            "FileChanged": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/local/bin/reindex" }] }
            ],
            "SubagentStart": [
                { "matcher": "", "hooks": [{ "type": "command", "command": "/usr/local/bin/tally start" }] }
            ]
        },
        "permissions": { "allow": ["Bash(git status)"], "deny": [] },
        "env": { "RUST_LOG": "warn" },
        "cleanupPeriodDays": 30,
        "includeCoAuthoredBy": false,
        "apiKeyHelper": "/usr/local/bin/get-key",
        "statusLine": { "type": "command", "command": "/usr/local/bin/line" },
        "outputStyle": "Explanatory",
        "forceLoginMethod": "claudeai",
        "enableAllProjectMcpServers": false,
        "enabledMcpjsonServers": [],
        "disabledMcpjsonServers": [],
        "awsAuthRefresh": "/usr/local/bin/aws-refresh",
        "awsCredentialExport": "/usr/local/bin/aws-export",
        "alwaysThinkingEnabled": true,
        "spinnerTipsEnabled": false,
        "attributionLine": "none",
        "autoUpdates": false,
        "verbose": true,
        "theme": "dark"
    })
}

/// The top-level keys of [`foreign_settings`], in the order they were written.
///
/// Spelled out here rather than read back off the fixture, because the fixture
/// is serialised by the very library whose ordering is under test: without
/// `serde_json`'s `preserve_order` feature both sides become alphabetical
/// together and agree with each other about the wrong answer. `model` and
/// `hooks` lead deliberately — alphabetically they belong in the middle, so
/// losing insertion order moves them immediately.
const SETTINGS_KEY_ORDER: &[&str] = &[
    "model",
    "hooks",
    "permissions",
    "env",
    "cleanupPeriodDays",
    "includeCoAuthoredBy",
    "apiKeyHelper",
    "statusLine",
    "outputStyle",
    "forceLoginMethod",
    "enableAllProjectMcpServers",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "awsAuthRefresh",
    "awsCredentialExport",
    "alwaysThinkingEnabled",
    "spinnerTipsEnabled",
    "attributionLine",
    "autoUpdates",
    "verbose",
    "theme",
];

// ─── One matcher entry, several hooks ────────────────────────────────────────

/// Every command in one entry, read straight out of the JSON.
///
/// Deliberately NOT `hooks::entry_commands`: a test that inspects the file
/// through the very helper it is checking cannot notice that helper going
/// blind, which is the defect these three tests exist for.
fn commands_in(entry: &Value) -> Vec<String> {
    entry["hooks"]
        .as_array()
        .map(|hooks| {
            hooks
                .iter()
                .filter_map(|h| h["command"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every command registered under one event, across all of its entries.
fn commands_for_event(settings: &Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .map(|entries| entries.iter().flat_map(commands_in).collect())
        .unwrap_or_default()
}

/// A `Stop` entry whose FIRST hook is foreign and whose second is a stale ours.
///
/// Claude Code's schema puts an ARRAY under `hooks`, so one matcher entry can
/// carry several commands. An installer that reads only `hooks[0]` sees this
/// entry as entirely foreign and leaves our stale registration inside it.
fn entry_with_a_foreign_hook_before_ours() -> Value {
    json!({ "hooks": { "Stop": [
        { "matcher": "", "hooks": [
            { "type": "command", "command": "/usr/bin/other-tool run" },
            { "type": "command", "command": "/old/path/yaadgaar hook stop-checkpoint" }
        ]}
    ]}})
}

mod lifecycle;
mod refusals;
mod registration;
mod verification;

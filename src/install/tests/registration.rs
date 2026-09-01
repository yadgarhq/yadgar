//! What `install` registers, and what it must not disturb.
//!
//! Split out of one file because the repository caps a file at 500 lines
//! (D59's complexity gate); the helpers and fixtures live in the parent.

use std::path::Path;

use serde_json::{json, Value};

use super::super::health::{drift, Drift};
use super::super::{hooks, install_with, uninstall, Layout, MANAGED_HOOKS};
use super::*;

#[test]
fn foreign_hook_entries_survive_an_install() {
    // The failure this prevents: `hooks[event] = [...]` deletes every entry
    // another tool wrote under the same key. On this machine that is nix's
    // SessionStart hook, and it goes without a word of warning.
    let home = scratch("foreign-survive");
    seed(&home, foreign_settings());
    install_with(&home, Path::new(BINARY)).unwrap();

    let settings = read_json(&Layout::new(&home).settings());
    let session_start = settings["hooks"]["SessionStart"].as_array().unwrap();
    assert!(
        session_start.iter().any(|e| hooks::entry_commands(e)
            .iter()
            .any(|c| c.contains("caveman.md"))),
        "the foreign SessionStart entry was eaten: {session_start:#?}"
    );
    let post_tool = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert!(
        post_tool.iter().any(|e| hooks::entry_commands(e)
            .iter()
            .any(|c| c.contains("hook_runner.py"))),
        "a foreign command that merely mentions yadgar was stripped: {post_tool:#?}"
    );
    // And unrelated top-level keys are still there, with their values.
    assert_eq!(settings["model"], "opus");
}

/// What `install` must write, spelled out INDEPENDENTLY of `MANAGED_HOOKS`.
///
/// Every literal here is a second, hand-written copy of something the installer
/// reads from its own table, and that duplication is the entire point. The
/// assertion this replaces compared the installed entry against the same
/// `HookSpec` the installer had just used to write it, so both sides of the
/// `assert_eq!` moved together and the test could only ever pass. Two measured
/// consequences: changing `pre-tool-guard`'s matcher to `"Bash"` was MISSED,
/// which silently un-routes the incident the matcher exists for — an agent used
/// Edit, not Bash, to add itself to a push allowlist — and flipping
/// `pre-compact-drain` to synchronous was MISSED, after which every compaction
/// blocks on the drain.
const EXPECTED_REGISTRATIONS: &[(&str, &str, &str, bool)] = &[
    ("PreCompact", "pre-compact-drain", "", true),
    ("SessionStart", "session-start-context", "", false),
    ("SessionStart", "post-compact-rehydrate", "compact", false),
    ("PostToolUse", "post-tool-capture", "", false),
    (
        "PostToolUse",
        "block-reflect",
        "mcp__yadgar__block_(create|update|delete|replace|append)",
        false,
    ),
    ("UserPromptSubmit", "prompt-recall", "", false),
    (
        "PreToolUse",
        "pre-tool-guard",
        "Bash|Edit|Write|NotebookEdit",
        false,
    ),
    ("Stop", "stop-checkpoint", "", false),
    ("SessionEnd", "session-end-capture", "", false),
    ("InstructionsLoaded", "instructions-loaded", "", false),
    ("SubagentStart", "subagent-start", "", false),
    ("FileChanged", "file-changed", "", false),
];

#[test]
fn every_managed_hook_is_registered_once_with_the_matcher_and_async_flag_it_needs() {
    let home = scratch("all-hooks");
    seed(&home, foreign_settings());
    install_with(&home, Path::new(BINARY)).unwrap();
    let settings = read_json(&Layout::new(&home).settings());

    for (event, name, matcher, fire_and_forget) in EXPECTED_REGISTRATIONS {
        let suffix = format!("hook {name}");
        let entries: Vec<&Value> = settings["hooks"][event]
            .as_array()
            .unwrap_or_else(|| panic!("no entries at all under {event}"))
            .iter()
            .filter(|e| commands_in(e).iter().any(|c| c.ends_with(&suffix)))
            .collect();
        assert_eq!(entries.len(), 1, "{event}/{name}: {entries:#?}");
        assert_eq!(entries[0]["matcher"], *matcher, "{event}/{name} matcher");

        let hook = entries[0]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["command"].as_str().is_some_and(|c| c.ends_with(&suffix)))
            .unwrap();
        // Absent and `false` mean the same thing to Claude Code, so the flag is
        // compared as a boolean rather than as a present-or-null key.
        assert_eq!(
            hook.get("async") == Some(&json!(true)),
            *fire_and_forget,
            "{event}/{name} async: {hook:#?}"
        );
    }

    // And the table above has to keep up: a hook added to MANAGED_HOOKS without
    // a line here would otherwise go registered but unchecked.
    assert_eq!(
        MANAGED_HOOKS.len(),
        EXPECTED_REGISTRATIONS.len(),
        "MANAGED_HOOKS and EXPECTED_REGISTRATIONS disagree on how many there are"
    );
    for spec in MANAGED_HOOKS {
        assert!(
            EXPECTED_REGISTRATIONS
                .iter()
                .any(|(event, name, _, _)| *event == spec.event && *name == spec.name),
            "{}/{} is registered but written down nowhere in this test",
            spec.event,
            spec.name
        );
    }
}

#[test]
fn one_written_entry_is_pinned_as_literal_bytes() {
    // EXPECTED_REGISTRATIONS above pins the event, the name, the matcher and
    // the async flag — and not one thing about the JSON around them. The
    // `{"type":"command","command":…}` framing is what Claude Code actually
    // reads, this is a durable format in a third party's file, and the shape
    // could change entirely with every assertion in this module still green.
    //
    // So: the bytes. Two entries, because the async one and the ordinary one
    // are different shapes and `async` is absent rather than false on the
    // second. Compared as text rather than as `Value`, so key ORDER is pinned
    // too — a `Value` comparison is order-insensitive and would not notice.
    let home = scratch("entry-bytes");
    install_with(&home, Path::new(BINARY)).unwrap();
    let settings = read_json(&Layout::new(&home).settings());

    assert_eq!(
        settings["hooks"]["PreCompact"].to_string(),
        r#"[{"matcher":"","hooks":[{"type":"command","command":"/usr/local/bin/yaadgaar hook pre-compact-drain","async":true}]}]"#
    );
    assert_eq!(
        settings["hooks"]["UserPromptSubmit"].to_string(),
        r#"[{"matcher":"","hooks":[{"type":"command","command":"/usr/local/bin/yaadgaar hook prompt-recall"}]}]"#
    );
}

#[test]
fn the_registered_command_names_the_binary_this_crate_actually_builds() {
    // The failure this prevents: `BINARY_NAME` and Cargo's `[[bin]] name` are
    // two spellings of one word, kept in two files. If they drift, every hook
    // this installer writes names a command that does not exist — and, worse,
    // one its own strip, merge and verify no longer recognise, so a reinstall
    // duplicates, an uninstall leaves everything live, and `verify` reports OK.
    // That is a dead pipeline behind healthy signals: D76, again.
    assert_eq!(
        hooks::BINARY_NAME,
        env!("CARGO_BIN_NAME"),
        "the hook commands name `{}` but Cargo builds `{}`",
        hooks::BINARY_NAME,
        env!("CARGO_BIN_NAME")
    );
    // And the word itself, written out once more, because the whole reason for
    // the rename is that `yadgar` is taken: the Python client owns it on PATH,
    // and two projects installing the same executable name cannot coexist.
    assert_eq!(hooks::BINARY_NAME, "yaadgaar");
}

#[test]
fn an_install_does_not_reorder_the_keys_of_a_file_it_does_not_own() {
    // The failure this prevents: `serde_json` without its `preserve_order`
    // feature reserialises an object with its keys in alphabetical order.
    // Nothing is lost and every line has moved, so somebody opens a whole-file
    // diff of their own settings that they did not ask for and cannot read —
    // the difference between a two-line diff and an unreviewable one. Removing
    // the feature left the suite green before this test existed.
    let home = scratch("key-order");
    seed(&home, foreign_settings());
    install_with(&home, Path::new(BINARY)).unwrap();

    let text = std::fs::read_to_string(Layout::new(&home).settings()).unwrap();
    let mut previous = 0;
    for key in SETTINGS_KEY_ORDER {
        // The two-space indent anchors the search to the top level, so a key
        // name that also occurs nested cannot move the answer.
        let needle = format!("\n  \"{key}\":");
        let at = text
            .find(&needle)
            .unwrap_or_else(|| panic!("{key} is not a top-level key any more:\n{text}"));
        assert!(
            at > previous,
            "{key} moved: the keys came back in a different order\n{text}"
        );
        previous = at;
    }
}

#[test]
fn installing_twice_is_byte_identical() {
    // The failure this prevents: every reinstall appending another copy of each
    // hook, so the capture fires N times and the settings file grows forever.
    let home = scratch("idempotent");
    seed(&home, foreign_settings());
    let layout = Layout::new(&home);
    install_with(&home, Path::new(BINARY)).unwrap();
    let first = (
        std::fs::read(layout.settings()).unwrap(),
        std::fs::read(layout.mcp_config()).unwrap(),
        std::fs::read(layout.claude_md()).unwrap(),
    );
    install_with(&home, Path::new(BINARY)).unwrap();
    let second = (
        std::fs::read(layout.settings()).unwrap(),
        std::fs::read(layout.mcp_config()).unwrap(),
        std::fs::read(layout.claude_md()).unwrap(),
    );
    assert_eq!(first.0, second.0, "settings.json changed on reinstall");
    assert_eq!(first.1, second.1, "the MCP config changed on reinstall");
    assert_eq!(first.2, second.2, "CLAUDE.md changed on reinstall");
}

#[test]
fn a_stale_registration_is_replaced_rather_than_duplicated() {
    // The failure this prevents: a previous install at a now-dead path is kept
    // beside the fresh one because the two command strings differ, and both
    // fire — one of them into nothing.
    let home = scratch("stale-replaced");
    seed(
        &home,
        json!({ "hooks": { "UserPromptSubmit": [
            { "matcher": "", "hooks": [{ "type": "command", "command": "/old/path/yaadgaar hook prompt-recall" }] }
        ]}}),
    );
    install_with(&home, Path::new(BINARY)).unwrap();
    let settings = read_json(&Layout::new(&home).settings());
    let entries = settings["hooks"]["UserPromptSubmit"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    assert!(hooks::entry_commands(&entries[0])[0].starts_with(BINARY));
}

#[test]
fn a_stale_hook_behind_a_foreign_one_in_the_same_entry_is_replaced_not_doubled() {
    // The failure this prevents: identity read `hooks[0]` only, so an entry
    // holding a foreign hook AND ours looked entirely foreign. The reinstall
    // appended a second copy and the capture fired twice — a doubled pipeline
    // no signal reports, which is the D76 failure mode exactly.
    let home = scratch("two-hooks-install");
    seed(&home, entry_with_a_foreign_hook_before_ours());

    install_with(&home, Path::new(BINARY)).unwrap();

    let settings = read_json(&Layout::new(&home).settings());
    let commands = commands_for_event(&settings, "Stop");
    assert!(
        commands.iter().any(|c| c.contains("other-tool")),
        "the foreign hook sharing the entry was eaten: {commands:#?}"
    );
    assert_eq!(
        commands
            .iter()
            .filter(|c| c.ends_with("hook stop-checkpoint"))
            .count(),
        1,
        "stop-checkpoint fires more than once: {commands:#?}"
    );
    assert!(
        !commands.iter().any(|c| c.starts_with("/old/path/")),
        "the stale registration survived: {commands:#?}"
    );
}

#[test]
fn an_uninstall_empties_a_shared_entry_of_ours_and_leaves_the_foreign_hook() {
    // The failure this prevents: uninstall dropped whole ENTRIES, so an entry
    // whose first hook was foreign was skipped and our hook stayed live against
    // an install that was gone.
    let home = scratch("two-hooks-uninstall");
    seed(&home, entry_with_a_foreign_hook_before_ours());
    install_with(&home, Path::new(BINARY)).unwrap();

    uninstall(&home).unwrap();

    let settings = read_json(&Layout::new(&home).settings());
    let commands = commands_for_event(&settings, "Stop");
    assert!(
        commands.iter().any(|c| c.contains("other-tool")),
        "uninstall ate the foreign hook: {commands:#?}"
    );
    assert!(
        !commands.iter().any(|c| c.contains("hook stop-checkpoint")),
        "a live yadgar hook was left behind: {commands:#?}"
    );
}

#[test]
fn verify_sees_a_duplicate_hidden_behind_a_foreign_hook_in_the_same_entry() {
    // The failure this prevents: `verify` read the same `hooks[0]`, so the one
    // check in the system that can notice a doubled pipeline was blind to it.
    let home = scratch("two-hooks-verify");
    install_with(&home, Path::new(BINARY)).unwrap();
    let layout = Layout::new(&home);
    let mut settings = read_json(&layout.settings());
    settings["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "matcher": "", "hooks": [
            { "type": "command", "command": "/usr/bin/other-tool run" },
            { "type": "command", "command": "/somewhere/else/yaadgaar hook stop-checkpoint" }
        ]}));
    std::fs::write(layout.settings(), settings.to_string()).unwrap();

    let found = drift(&home);
    assert!(
        found.iter().any(|d| matches!(
            d,
            Drift::HookDuplicated { name, .. } if name == "stop-checkpoint"
        )),
        "{found:#?}"
    );
}

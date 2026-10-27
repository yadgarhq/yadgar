//! A re-install must not move what is already in place.
//!
//! The installer used to remove its own hook entry and append a fresh one. Our
//! entry then jumped behind any foreign entry that followed it, so a re-install
//! rewrote `settings.json` with the other tool's hooks in a different order.

use std::path::Path;

use serde_json::Value;

use super::super::{install_with, Layout};
use super::*;

/// A settings file text exactly as `write_atomic` writes it, so a no-op
/// install can be held to byte equality rather than JSON equality.
fn settings_text(pre_tool_use: Value) -> String {
    let value = serde_json::json!({
        "model": "opus",
        "hooks": {
            "PreToolUse": pre_tool_use,
            // Our session-start entry is already in place behind the foreign
            // one, so every test here is about `PreToolUse` alone.
            "SessionStart": [
                { "matcher": "", "hooks": [{ "type": "command",
                    "command": "/usr/local/bin/audit-shell start" }] },
                { "matcher": "", "hooks": [{ "type": "command",
                    "command": format!("{BINARY} hook session-start") }] },
            ],
            "TaskCompleted": [
                { "matcher": "", "hooks": [{ "type": "command",
                    "command": format!("{BINARY} hook task-completed") }] },
            ],
        },
    });
    let mut text = serde_json::to_string_pretty(&value).unwrap();
    text.push('\n');
    text
}

/// A STALE entry of ours: `task-update` under a matcher it no longer has.
fn own_entry(matcher: &str) -> Value {
    serde_json::json!({ "matcher": matcher, "hooks": [{ "type": "command",
        "command": format!("{BINARY} hook task-update") }] })
}

/// Our one `PreToolUse` entry, as a current install writes it. `merge` places
/// it where the first one of ours sat.
fn task_update_entry() -> Value {
    own_entry("TaskUpdate")
}

fn foreign_entry() -> Value {
    serde_json::json!({ "matcher": "Bash", "hooks": [{ "type": "command",
        "command": "/home/u/.claude/hooks/yadgar-pretooluse-router.py" }] })
}

fn install_over(name: &str, seeded: &str) -> (String, bool) {
    let home = scratch(name);
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    std::fs::write(layout.settings(), seeded).unwrap();
    let summary = install_with(&home, Path::new(BINARY)).unwrap();
    (
        std::fs::read_to_string(layout.settings()).unwrap(),
        summary.settings_changed,
    )
}

#[test]
fn a_reinstall_leaves_our_hook_where_it_was_ahead_of_a_foreign_one() {
    let seeded = settings_text(serde_json::json!([task_update_entry(), foreign_entry()]));
    let (after, changed) = install_over("idem-own-first", &seeded);
    assert_eq!(after, seeded);
    assert!(!changed);
}

#[test]
fn a_reinstall_leaves_our_hook_where_it_was_behind_a_foreign_one() {
    let seeded = settings_text(serde_json::json!([foreign_entry(), task_update_entry()]));
    let (after, changed) = install_over("idem-own-last", &seeded);
    assert_eq!(after, seeded);
    assert!(!changed);
}

#[test]
fn a_reinstall_with_our_event_not_last_keeps_the_event_order() {
    // `PreToolUse` is the first key here and `SessionStart` follows it. Removing
    // the event key to re-add it would move it to the end. The entry is stale so
    // that the file IS rewritten: a no-op install never writes, and `Value`
    // equality ignores key order, so a no-op cannot show this.
    let seeded = settings_text(serde_json::json!([own_entry("Bash")]));
    let (after, changed) = install_over("idem-event-order", &seeded);
    assert!(changed);
    assert_eq!(
        after,
        settings_text(serde_json::json!([task_update_entry()]))
    );
}

#[test]
fn a_changed_entry_of_ours_is_replaced_at_its_index_and_foreign_ones_stay() {
    let seeded = settings_text(serde_json::json!([
        foreign_entry(),
        own_entry("Bash"),
        foreign_entry()
    ]));
    let (after, changed) = install_over("idem-update", &seeded);
    assert!(changed);
    assert_eq!(
        after,
        settings_text(serde_json::json!([
            foreign_entry(),
            task_update_entry(),
            foreign_entry()
        ]))
    );
}

#[test]
fn a_first_install_appends_after_the_foreign_entries() {
    let seeded = settings_text(serde_json::json!([foreign_entry()]));
    let (after, changed) = install_over("idem-first", &seeded);
    assert!(changed);
    assert_eq!(
        after,
        settings_text(serde_json::json!([foreign_entry(), task_update_entry()]))
    );
}

#[test]
fn duplicate_entries_of_ours_collapse_into_the_first_one_position() {
    let seeded = settings_text(serde_json::json!([
        own_entry("Bash"),
        foreign_entry(),
        own_entry("Edit")
    ]));
    let (after, _) = install_over("idem-dupes", &seeded);
    assert_eq!(
        after,
        settings_text(serde_json::json!([task_update_entry(), foreign_entry()]))
    );
}

#[test]
fn a_mixed_entry_keeps_its_foreign_hook_and_our_fresh_entries_go_after_it() {
    // ONE entry holding a foreign hook AND ours (mutant M2). Ours comes out of
    // it, the foreign hook stays where the entry was, and the fresh entries
    // land after that surviving entry, never in front of it.
    let foreign_hook = serde_json::json!({ "type": "command",
        "command": "/home/u/.claude/hooks/yadgar-pretooluse-router.py" });
    let mixed = serde_json::json!({ "matcher": "Bash", "hooks": [
        foreign_hook,
        { "type": "command", "command": format!("{BINARY} hook task-update") },
    ] });
    let seeded = settings_text(serde_json::json!([mixed]));
    let (after, changed) = install_over("idem-mixed", &seeded);
    assert!(changed);
    assert_eq!(
        after,
        settings_text(serde_json::json!([
            { "matcher": "Bash", "hooks": [foreign_hook] },
            task_update_entry()
        ]))
    );
}

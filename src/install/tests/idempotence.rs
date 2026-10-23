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

fn own_entry(matcher: &str) -> Value {
    serde_json::json!({ "matcher": matcher, "hooks": [{ "type": "command",
        "command": format!("{BINARY} hook pre-tool-guard") }] })
}

/// Our second `PreToolUse` entry. `merge` places both of ours where the first
/// one of ours sat, the guard first.
fn task_update_entry() -> Value {
    serde_json::json!({ "matcher": "TaskUpdate", "hooks": [{ "type": "command",
        "command": format!("{BINARY} hook task-update") }] })
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

const GUARD_MATCHER: &str = "Bash|Edit|Write|NotebookEdit";

#[test]
fn a_reinstall_leaves_our_hook_where_it_was_ahead_of_a_foreign_one() {
    let seeded = settings_text(serde_json::json!([
        own_entry(GUARD_MATCHER),
        task_update_entry(),
        foreign_entry()
    ]));
    let (after, changed) = install_over("idem-own-first", &seeded);
    assert_eq!(after, seeded);
    assert!(!changed);
}

#[test]
fn a_reinstall_leaves_our_hook_where_it_was_behind_a_foreign_one() {
    let seeded = settings_text(serde_json::json!([
        foreign_entry(),
        own_entry(GUARD_MATCHER),
        task_update_entry()
    ]));
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
        settings_text(serde_json::json!([
            own_entry(GUARD_MATCHER),
            task_update_entry()
        ]))
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
            own_entry(GUARD_MATCHER),
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
        settings_text(serde_json::json!([
            foreign_entry(),
            own_entry(GUARD_MATCHER),
            task_update_entry()
        ]))
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
        settings_text(serde_json::json!([
            own_entry(GUARD_MATCHER),
            task_update_entry(),
            foreign_entry()
        ]))
    );
}

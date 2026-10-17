//! Installing 0.1.5 over a 0.1.3/0.1.4-shaped home: twelve hooks of ours become one.
//!
//! Up to 0.1.4, `install` registered twelve handlers and only `pre-tool-guard`
//! did any work. The other eleven spawned this binary on every tool call, prompt
//! and stop, beside the memory engine's own hooks, and did nothing. An upgrade
//! must remove the eleven it wrote — identified by the invocation shape
//! `yaadgaar hook <name>`, never by handler name — and leave every engine entry
//! exactly as it was, including one that reuses a retired handler's name.

use std::path::Path;

use serde_json::{json, Value};

use super::super::health::{drift, Drift};
use super::super::{hooks, install_with, uninstall, Layout, MANAGED_HOOKS};
use super::*;

/// Where 0.1.3 installed from: `pipx install yaadgaar` puts it here.
const OLD_BINARY: &str = "/home/x/.local/bin/yaadgaar";

/// The engine's python hook runner, as the memory engine registers it.
const ENGINE: &str =
    "/home/x/.local/pipx/venvs/yadgar/bin/python /home/x/.claude/hooks/hook_runner.py";

/// The twelve registrations 0.1.4 wrote, as `(event, handler, matcher)`.
const RETIRED_INSTALL: &[(&str, &str, &str)] = &[
    ("PreCompact", "pre-compact-drain", ""),
    ("SessionStart", "session-start-context", ""),
    ("SessionStart", "post-compact-rehydrate", "compact"),
    ("PostToolUse", "post-tool-capture", ""),
    (
        "PostToolUse",
        "block-reflect",
        "mcp__yaadgaar__block_(create|update|delete|replace|append)",
    ),
    ("UserPromptSubmit", "prompt-recall", ""),
    (
        "PreToolUse",
        "pre-tool-guard",
        "Bash|Edit|Write|NotebookEdit",
    ),
    ("Stop", "stop-checkpoint", ""),
    ("SessionEnd", "session-end-capture", ""),
    ("InstructionsLoaded", "instructions-loaded", ""),
    ("SubagentStart", "subagent-start", ""),
    ("FileChanged", "file-changed", ""),
];

/// A live-shaped `settings.json`: engine entries, a caveman entry, and 0.1.4's twelve.
fn upgraded_home_settings() -> Value {
    let mut hooks = serde_json::Map::new();
    let mut push = |event: &str, entry: Value| {
        hooks
            .entry(event.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(entry);
    };
    // The engine's thirteen, with the SAME handler names this client used.
    for (event, name, matcher) in [
        ("SessionStart", "session-start-context", ""),
        ("SessionStart", "post-compact-rehydrate", "compact"),
        ("PostToolUse", "post-tool-capture", ""),
        (
            "PostToolUse",
            "block-reflect",
            "mcp__yadgar__block_(create|update|delete|replace|append)",
        ),
        ("UserPromptSubmit", "prompt-recall", ""),
        (
            "PreToolUse",
            "pretooluse-router",
            "Bash|Edit|Write|NotebookEdit",
        ),
        ("Stop", "stop-checkpoint", ""),
        ("SessionEnd", "session-end-capture", ""),
        ("PreCompact", "pre-compact-drain", ""),
        ("InstructionsLoaded", "instructions-loaded", ""),
        ("SubagentStart", "subagent-start", ""),
        ("FileChanged", "file-changed", ""),
        ("Notification", "notification", ""),
    ] {
        push(
            event,
            json!({ "matcher": matcher, "hooks": [{ "type": "command", "command": format!("{ENGINE} {name}") }] }),
        );
    }
    // A foreign binary literally called `yadgar` with `hook` as its verb: the
    // invocation shape of ours, one vowel short. Not ours.
    push(
        "Stop",
        json!({ "matcher": "", "hooks": [{ "type": "command", "command": "/opt/yadgar/bin/yadgar hook stop-checkpoint" }] }),
    );
    push(
        "SessionStart",
        json!({ "matcher": "", "hooks": [{ "type": "command", "command": "find /nix/store/caveman -name caveman.md -exec cat {} +" }] }),
    );
    for (event, name, matcher) in RETIRED_INSTALL {
        push(
            event,
            json!({ "matcher": matcher, "hooks": [{ "type": "command", "command": format!("{OLD_BINARY} hook {name}") }] }),
        );
    }
    json!({ "model": "opus", "hooks": hooks, "theme": "dark" })
}

/// Every entry holding no command of ours, per event, in file order.
fn foreign_entries(settings: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for (event, list) in settings["hooks"].as_object().into_iter().flatten() {
        let kept: Vec<Value> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| !commands_in(e).iter().any(|c| hooks::is_managed(c)))
            .cloned()
            .collect();
        if !kept.is_empty() {
            out.insert(event.clone(), Value::Array(kept));
        }
    }
    Value::Object(out)
}

/// Every command of ours in the file, as `(event, command)`.
fn our_commands(settings: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (event, list) in settings["hooks"].as_object().into_iter().flatten() {
        for entry in list.as_array().into_iter().flatten() {
            for cmd in commands_in(entry) {
                if hooks::is_managed(&cmd) {
                    out.push((event.clone(), cmd));
                }
            }
        }
    }
    out
}

#[test]
fn install_registers_only_the_guard() {
    let home = scratch("guard-only");
    install_with(&home, Path::new(BINARY)).unwrap();
    let settings = read_json(&Layout::new(&home).settings());
    assert_eq!(
        our_commands(&settings),
        vec![(
            "PreToolUse".to_string(),
            format!("{BINARY} hook pre-tool-guard")
        )]
    );
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["matcher"],
        "Bash|Edit|Write|NotebookEdit"
    );
}

#[test]
fn install_over_a_twelve_hook_install_leaves_exactly_one_of_ours() {
    let home = scratch("twelve-to-one");
    seed(&home, upgraded_home_settings());
    assert_eq!(
        our_commands(&read_json(&Layout::new(&home).settings())).len(),
        12
    );

    install_with(&home, Path::new(BINARY)).unwrap();

    let settings = read_json(&Layout::new(&home).settings());
    assert_eq!(
        our_commands(&settings),
        vec![(
            "PreToolUse".to_string(),
            format!("{BINARY} hook pre-tool-guard")
        )],
        "{settings:#?}"
    );
    assert!(
        drift(&home)
            .iter()
            .all(|d| !matches!(d, Drift::HookUnknown { .. } | Drift::HookDuplicated { .. })),
        "{:#?}",
        drift(&home)
    );
}

#[test]
fn engine_hook_entries_are_unchanged_by_install_and_uninstall() {
    // Compared as serialised text, entry by entry and in file order, so a
    // reordering or a rewritten matcher is as visible as a deletion.
    let home = scratch("engine-untouched");
    let seeded = upgraded_home_settings();
    seed(&home, seeded.clone());
    let before = serde_json::to_string(&foreign_entries(&seeded)).unwrap();

    install_with(&home, Path::new(BINARY)).unwrap();
    let after_install = read_json(&Layout::new(&home).settings());
    assert_eq!(
        serde_json::to_string(&foreign_entries(&after_install)).unwrap(),
        before
    );
    assert_eq!(after_install["model"], "opus");
    assert_eq!(after_install["theme"], "dark");

    uninstall(&home).unwrap();
    let after_uninstall = read_json(&Layout::new(&home).settings());
    assert_eq!(
        serde_json::to_string(&foreign_entries(&after_uninstall)).unwrap(),
        before
    );
    assert!(our_commands(&after_uninstall).is_empty());
}

#[test]
fn a_foreign_yadgar_binary_using_a_retired_handler_name_survives() {
    // "Provably ours" is the binary name `yaadgaar` plus the verb `hook`. A
    // handler name alone proves nothing: the engine uses the same names.
    let home = scratch("lookalike");
    seed(&home, upgraded_home_settings());
    install_with(&home, Path::new(BINARY)).unwrap();
    let stop = commands_for_event(&read_json(&Layout::new(&home).settings()), "Stop");
    assert!(
        stop.iter()
            .any(|c| c == "/opt/yadgar/bin/yadgar hook stop-checkpoint"),
        "{stop:#?}"
    );
    assert!(
        stop.iter()
            .any(|c| c == &format!("{ENGINE} stop-checkpoint")),
        "{stop:#?}"
    );
}

#[test]
fn a_foreign_non_command_hook_sharing_an_entry_with_ours_survives() {
    // An http/prompt/agent hook has no `command` key. Judging "is the entry
    // empty now?" by commands alone called this entry empty once ours was
    // stripped, and deleted the foreign http hook with it.
    let home = scratch("non-command-neighbour");
    let http = json!({ "type": "http", "url": "http://127.0.0.1:9/hook" });
    seed(
        &home,
        json!({ "hooks": { "Stop": [
            { "matcher": "", "hooks": [
                http.clone(),
                { "type": "command", "command": format!("{OLD_BINARY} hook stop-checkpoint") }
            ]}
        ]}}),
    );

    install_with(&home, Path::new(BINARY)).unwrap();
    let settings = read_json(&Layout::new(&home).settings());
    assert_eq!(
        settings["hooks"]["Stop"],
        json!([{ "matcher": "", "hooks": [http.clone()] }]),
        "{settings:#?}"
    );

    uninstall(&home).unwrap();
    let settings = read_json(&Layout::new(&home).settings());
    assert_eq!(
        settings["hooks"]["Stop"],
        json!([{ "matcher": "", "hooks": [http] }]),
        "{settings:#?}"
    );
}

#[test]
fn the_managed_list_is_the_guard_alone() {
    let names: Vec<_> = MANAGED_HOOKS.iter().map(|h| (h.event, h.name)).collect();
    assert_eq!(names, vec![("PreToolUse", "pre-tool-guard")]);
}

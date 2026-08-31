//! End-to-end tests for install / uninstall / verify.
//!
//! Every one of these pins a failure that actually happened. They never touch
//! the real `~/.claude`: the whole install is parameterised on a home directory
//! precisely so a test can be given a scratch one, and a test that wrote into
//! somebody's live settings would have already failed at the thing this module
//! is for.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::health::{drift, Drift};
use super::{install_with, uninstall, Layout, MANAGED_HOOKS};

/// A binary path that is durable-looking and does not exist.
///
/// Durability and existence are checked by `verify`, not by `install` — and
/// separating them is what lets these tests register a plausible path without a
/// real binary. The path checks have their own tests.
const BINARY: &str = "/usr/local/bin/yadgar";

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

/// The two foreign entries this machine really has, from nix.
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
                { "matcher": "", "hooks": [{ "type": "command", "command": "/home/x/.local/pipx/venvs/yadgar/bin/python /home/x/.claude/hooks/hook_runner.py post-tool-capture" }] }
            ]
        }
    })
}

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
        session_start
            .iter()
            .any(|e| super::hooks::entry_command(e).is_some_and(|c| c.contains("caveman.md"))),
        "the foreign SessionStart entry was eaten: {session_start:#?}"
    );
    let post_tool = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert!(
        post_tool
            .iter()
            .any(|e| super::hooks::entry_command(e).is_some_and(|c| c.contains("hook_runner.py"))),
        "a foreign command that merely mentions yadgar was stripped: {post_tool:#?}"
    );
    // And unrelated top-level keys are still there, with their values.
    assert_eq!(settings["model"], "opus");
}

#[test]
fn every_managed_hook_is_registered_exactly_once() {
    let home = scratch("all-hooks");
    install_with(&home, Path::new(BINARY)).unwrap();
    let settings = read_json(&Layout::new(&home).settings());

    for spec in MANAGED_HOOKS {
        let entries = settings["hooks"][spec.event].as_array().unwrap();
        let mine: Vec<_> = entries
            .iter()
            .filter(|e| {
                super::hooks::entry_command(e).and_then(super::hooks::managed_name)
                    == Some(spec.name.to_string())
            })
            .collect();
        assert_eq!(mine.len(), 1, "{}/{}", spec.event, spec.name);
        assert_eq!(mine[0]["matcher"], spec.matcher);
        assert_eq!(
            mine[0]["hooks"][0]["async"].is_null(),
            !spec.fire_and_forget
        );
    }
    // Twelve registrations across ten event keys — two events carry two each.
    assert_eq!(MANAGED_HOOKS.len(), 12);
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
            { "matcher": "", "hooks": [{ "type": "command", "command": "/old/path/yadgar hook prompt-recall" }] }
        ]}}),
    );
    install_with(&home, Path::new(BINARY)).unwrap();
    let settings = read_json(&Layout::new(&home).settings());
    let entries = settings["hooks"]["UserPromptSubmit"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    assert!(super::hooks::entry_command(&entries[0])
        .unwrap()
        .starts_with(BINARY));
}

#[cfg(unix)]
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
    std::os::unix::fs::symlink(&target, layout.claude_md()).unwrap();

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

#[cfg(unix)]
#[test]
fn a_read_only_claude_md_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("readonly-refusal");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    std::fs::write(layout.claude_md(), "# theirs\n").unwrap();
    // The refusal reads the mode bits rather than attempting a write, so it
    // fires for root too — a test that only passes as an unprivileged user is a
    // test that stops running the day CI switches to a root container.
    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o444)).unwrap();

    let err = install_with(&home, Path::new(BINARY))
        .unwrap_err()
        .to_string();
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

#[test]
fn verify_catches_a_stale_command_path() {
    // The failure this prevents, twice over: a dead pipeline while every signal
    // reads healthy. A registration pointing at a path that no longer exists is
    // invisible to the daemon, so if this check cannot fail, nothing can.
    let home = scratch("verify-stale");
    install_with(&home, Path::new("/gone/for/good/yadgar")).unwrap();

    let found = drift(&home);
    assert!(
        found
            .iter()
            .any(|d| matches!(d, Drift::CommandMissing { .. })),
        "{found:#?}"
    );
    // And it is a failure, not a note: `verify` must exit non-zero.
    assert!(super::verify(&home).is_err());
}

#[test]
fn verify_is_structurally_clean_after_an_install() {
    // The other half: a check that fails on a healthy machine gets switched off,
    // and then it is a check nobody runs.
    //
    // Command-path drift is excluded here and tested on its own above: these
    // tests register a plausible path with no binary behind it, which verify is
    // right to flag.
    let home = scratch("verify-clean");
    install_with(&home, Path::new(BINARY)).unwrap();
    let structural: Vec<_> = drift(&home)
        .into_iter()
        .filter(|d| {
            !matches!(
                d,
                Drift::CommandMissing { .. } | Drift::CommandEphemeral { .. }
            )
        })
        .collect();
    assert!(structural.is_empty(), "{structural:#?}");
}

#[test]
fn verify_catches_a_credential_left_in_the_mcp_config() {
    let home = scratch("verify-token");
    install_with(&home, Path::new(BINARY)).unwrap();
    let layout = Layout::new(&home);
    let mut config = read_json(&layout.mcp_config());
    config["mcpServers"]["yadgar"]["headers"] = json!({ "Authorization": "Bearer leaked" });
    std::fs::write(layout.mcp_config(), config.to_string()).unwrap();

    assert!(drift(&home)
        .iter()
        .any(|d| matches!(d, Drift::McpCarriesCredential)));
}

#[test]
fn the_mcp_entry_names_the_binary_and_carries_no_token() {
    let home = scratch("mcp-entry");
    let layout = Layout::new(&home);
    std::fs::write(
        layout.mcp_config(),
        json!({
            "numStartups": 41,
            "mcpServers": {
                "yadgar": { "type": "http", "url": "http://127.0.0.1:8765/mcp",
                            "headers": { "Authorization": "Bearer a-real-token" } },
                "browsermcp": { "type": "stdio", "command": "npx", "args": ["@browsermcp/mcp@latest"] }
            }
        })
        .to_string(),
    )
    .unwrap();

    install_with(&home, Path::new(BINARY)).unwrap();

    let config = read_json(&layout.mcp_config());
    assert_eq!(config["mcpServers"]["yadgar"]["command"], BINARY);
    assert_eq!(config["mcpServers"]["yadgar"]["args"], json!(["serve"]));
    assert!(config["mcpServers"]["yadgar"].get("headers").is_none());
    // Foreign servers and unrelated keys survive, values included.
    assert_eq!(config["mcpServers"]["browsermcp"]["command"], "npx");
    assert_eq!(config["numStartups"], 41);
}

#[test]
fn uninstall_removes_ours_and_keeps_everything_else() {
    // The failure this prevents: the Python uninstall left hooks, settings
    // entries and rules all firing against an install that was gone — and, the
    // other way, an uninstall that takes the exceptions file with it throws away
    // decisions a human made.
    let home = scratch("uninstall");
    let layout = Layout::new(&home);
    seed(&home, foreign_settings());
    install_with(&home, Path::new(BINARY)).unwrap();
    std::fs::write(
        layout.exceptions(),
        "{\"push_default_allowlist\":[\"nix\"]}",
    )
    .unwrap();
    std::fs::write(layout.mcp_config(), {
        let mut c = read_json(&layout.mcp_config());
        c["mcpServers"]["browsermcp"] = json!({ "type": "stdio", "command": "npx" });
        c.to_string()
    })
    .unwrap();

    uninstall(&home).unwrap();

    let settings = read_json(&layout.settings());
    assert!(
        settings["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| super::hooks::entry_command(e).is_some_and(|c| c.contains("caveman.md"))),
        "uninstall ate a foreign hook: {settings:#?}"
    );
    // Keys yadgar emptied are gone rather than left as residue.
    assert!(
        settings["hooks"].get("UserPromptSubmit").is_none(),
        "{settings:#?}"
    );
    assert_eq!(settings["model"], "opus");

    let config = read_json(&layout.mcp_config());
    assert!(config["mcpServers"].get("yadgar").is_none());
    assert_eq!(config["mcpServers"]["browsermcp"]["command"], "npx");

    assert!(
        !layout.rules().exists(),
        "the owned rules file was left behind"
    );
    let claude_md = std::fs::read_to_string(layout.claude_md()).unwrap();
    assert!(!claude_md.contains("yadgar-rules.md"), "{claude_md:?}");

    // Never removed: it holds decisions a human made.
    assert!(
        layout.exceptions().exists(),
        "uninstall took the exceptions file"
    );
}

#[test]
fn the_rules_file_is_owned_and_referenced_from_the_first_line() {
    let home = scratch("rules-owned");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    std::fs::write(layout.claude_md(), "# my own instructions\n").unwrap();

    install_with(&home, Path::new(BINARY)).unwrap();

    let claude_md = std::fs::read_to_string(layout.claude_md()).unwrap();
    let first = claude_md.lines().next().unwrap();
    assert_eq!(first, format!("@{}", layout.rules().display()));
    assert!(claude_md.contains("# my own instructions"));
    // Exactly one line was added, and nothing was spliced into it.
    assert_eq!(claude_md.lines().count(), 2);
    assert!(std::fs::read_to_string(layout.rules())
        .unwrap()
        .contains("Yadgar"));
}

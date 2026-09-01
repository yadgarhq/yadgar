//! The MCP entry, the rules file, and what an uninstall leaves behind.
//!
//! Split out of one file because the repository caps a file at 500 lines
//! (D59's complexity gate); the helpers and fixtures live in the parent.

use std::path::Path;

use serde_json::json;

use super::super::health::{drift, Drift};
use super::super::{hooks, install_with, mcp, uninstall, Layout};
use super::*;

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
    // Somebody's own instructions, so `CLAUDE.md` is a file that must SURVIVE
    // the uninstall with its reference line taken out. One created by install
    // and holding nothing else is deleted instead, and that is pinned on its own
    // below — the two cases are opposite and a single fixture cannot show both.
    std::fs::write(layout.claude_md(), "# my own instructions\n").unwrap();
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
            .any(|e| hooks::entry_commands(e)
                .iter()
                .any(|c| c.contains("caveman.md"))),
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

#[test]
fn verify_is_clean_on_the_machine_where_install_refused() {
    // The failure this prevents: `install` refuses on a nix box because
    // CLAUDE.md is a store symlink, the person does what the refusal message
    // says and adds the reference to whatever generates that file — and then a
    // scheduled `verify` reports drift at them forever. A check that cries wolf
    // at somebody who followed the instructions is a check they turn off, which
    // is the same ending as never writing it.
    let home = scratch("verify-after-refusal");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();

    // Everything install would have written, written by hand instead.
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let store = home.join("nix-store-CLAUDE.md");
    std::fs::write(
        &store,
        format!("# declared elsewhere\n@{}\n", layout.rules().display()),
    )
    .unwrap();
    require_symlink(&store, &layout.claude_md());
    std::fs::write(layout.rules(), "# rules").unwrap();
    let mut settings = serde_json::json!({});
    hooks::merge(&mut settings, Path::new(BINARY));
    std::fs::write(layout.settings(), settings.to_string()).unwrap();
    let mut mcp = serde_json::json!({});
    mcp::merge(&mut mcp, Path::new(BINARY));
    std::fs::write(layout.mcp_config(), mcp.to_string()).unwrap();

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
fn uninstall_removes_the_files_install_created() {
    // The failure this prevents: on a machine with no `~/.claude` at all,
    // install created all three files and uninstall left every one of them
    // behind, emptied — `{"hooks":{}}`, `{"mcpServers":{}}` and a zero-byte
    // `CLAUDE.md`. D76 says uninstall removes exactly what install added, and a
    // file yadgar created, then emptied and left, is not that.
    //
    // The delete is safe WITHOUT KNOWING WHO CREATED THE FILE, which is the
    // only thing that could be known here: what is left after the strip holds
    // nothing at all, so there is nothing of anybody's in it to lose. The other
    // direction is pinned by the test below, and it is the one that matters —
    // this module has already had one bug that replaced somebody's `CLAUDE.md`
    // with a single line.
    let home = scratch("uninstall-created-files");
    let layout = Layout::new(&home);

    install_with(&home, Path::new(BINARY)).unwrap();
    assert!(layout.settings().exists(), "install wrote no settings.json");
    assert!(layout.mcp_config().exists(), "install wrote no MCP config");
    assert!(layout.claude_md().exists(), "install wrote no CLAUDE.md");

    uninstall(&home).unwrap();

    assert!(
        !layout.settings().exists(),
        "settings.json was left behind: {:?}",
        std::fs::read_to_string(layout.settings())
    );
    assert!(
        !layout.mcp_config().exists(),
        "the MCP config was left behind: {:?}",
        std::fs::read_to_string(layout.mcp_config())
    );
    assert!(
        !layout.claude_md().exists(),
        "CLAUDE.md was left behind: {:?}",
        std::fs::read_to_string(layout.claude_md())
    );
    assert!(!layout.rules().exists(), "the rules file was left behind");
}

#[test]
fn uninstall_keeps_a_config_whose_only_content_is_a_name_somebody_chose() {
    // THE MEASURED DESTRUCTION, end to end. Both files below hold no scalar
    // anywhere, and the vacancy rule hunted for scalars — so an install
    // followed by an uninstall DELETED them:
    //
    //   settings.json  {"permissions":{"allow":[],"deny":[],"ask":[]}}  -> gone
    //   .claude.json   {"mcpServers":{"other-server":{}}}               -> gone
    //
    // Somebody's permissions configuration, and a THIRD PARTY'S MCP server
    // registration. The name `other-server` is the entire record that anybody
    // registered it, so the one thing that was data was the one thing a rule
    // reading leaf scalars could not see.
    let home = scratch("uninstall-keeps-named-keys");
    let layout = Layout::new(&home);
    seed(
        &home,
        json!({ "permissions": { "allow": [], "deny": [], "ask": [] } }),
    );
    std::fs::write(
        layout.mcp_config(),
        json!({ "mcpServers": { "other-server": {} } }).to_string(),
    )
    .unwrap();

    install_with(&home, Path::new(BINARY)).unwrap();
    uninstall(&home).unwrap();

    assert!(
        layout.settings().exists(),
        "somebody's permissions configuration was deleted"
    );
    assert_eq!(
        read_json(&layout.settings())["permissions"],
        json!({ "allow": [], "deny": [], "ask": [] }),
        "the permissions block came back changed"
    );
    assert!(
        layout.mcp_config().exists(),
        "a third party's MCP registration was deleted"
    );
    assert!(
        read_json(&layout.mcp_config())["mcpServers"]
            .get("other-server")
            .is_some(),
        "somebody else's MCP server went with the uninstall"
    );
    // And ours still came out, which is the half that must not be traded away.
    assert_no_managed_hooks(&layout.settings());
    assert_no_mcp_entry(&layout.mcp_config());
}

#[test]
fn an_uninstall_on_an_untouched_machine_writes_nothing_at_all() {
    // Two separate things, both invisible to every other test here.
    //
    // A machine with NO `settings.json`: the `ensure_object` defect made
    // `hooks::strip` create `"hooks": {}`, so an uninstall wrote a file that had
    // never existed. Two independent fixes were sharing one test, and either
    // could regress in silence — so BOTH are asserted here.
    //
    // The file not existing is the outcome. It is NOT enough on its own:
    // `write_or_prune`'s `!path.exists()` guard catches the created `{"hooks":
    // {}}` on the way out, so restoring `ensure_object` leaves this half green.
    // What it cannot mask is the SUMMARY — `merged` compares before against
    // after, and a key conjured out of nothing is a change. An uninstall that
    // removed nothing would report having rewritten `settings.json`.
    let bare = scratch("uninstall-bare-home");
    let layout = Layout::new(&bare);
    let summary = uninstall(&bare).unwrap();
    assert!(
        !layout.settings().exists(),
        "an uninstall created a settings.json on a machine that had none"
    );
    assert!(
        !layout.mcp_config().exists(),
        "an uninstall created an MCP config on a machine that had none"
    );
    assert!(
        !summary.settings_changed,
        "an uninstall on a bare machine claimed it changed settings.json"
    );
    assert!(
        !summary.mcp_changed,
        "an uninstall on a bare machine claimed it changed the MCP config"
    );

    // A machine with somebody's own configs and no yadgar install: the gates
    // around `write_or_prune`. Without them an uninstall that removed nothing
    // still rewrites both files — reserialised and pretty-printed, so the bytes
    // move — on a machine yadgar was never installed on. Seeded COMPACT for
    // exactly that reason: a rewrite is visible in the bytes.
    let theirs = scratch("uninstall-untouched-home");
    let layout = Layout::new(&theirs);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    let settings = json!({ "model": "opus" }).to_string();
    let config = json!({ "numStartups": 41 }).to_string();
    std::fs::write(layout.settings(), &settings).unwrap();
    std::fs::write(layout.mcp_config(), &config).unwrap();

    uninstall(&theirs).unwrap();

    assert_eq!(
        std::fs::read_to_string(layout.settings()).unwrap(),
        settings,
        "an uninstall rewrote a settings.json it had removed nothing from"
    );
    assert_eq!(
        std::fs::read_to_string(layout.mcp_config()).unwrap(),
        config,
        "an uninstall rewrote an MCP config it had removed nothing from"
    );
}

#[test]
fn uninstall_leaves_every_file_that_holds_somebody_elses_content() {
    // The direction that matters, and the reason the rule is "what remains is
    // nothing at all" rather than "yadgar created it". A `settings.json` whose
    // `hooks` yadgar has just emptied still carries the person's `model`; a
    // `CLAUDE.md` still carries their instructions. Removing either is
    // destroying their work, and the asymmetry with the test above is total:
    // leaving an empty file is untidy, and this is not.
    let home = scratch("uninstall-keeps-content");
    let layout = Layout::new(&home);
    seed(&home, json!({ "model": "opus" }));
    std::fs::write(
        layout.mcp_config(),
        json!({ "numStartups": 41 }).to_string(),
    )
    .unwrap();
    std::fs::write(layout.claude_md(), "# my own instructions\n").unwrap();

    install_with(&home, Path::new(BINARY)).unwrap();
    uninstall(&home).unwrap();

    assert_eq!(read_json(&layout.settings())["model"], "opus");
    assert_eq!(read_json(&layout.mcp_config())["numStartups"], 41);
    assert_eq!(
        std::fs::read_to_string(layout.claude_md()).unwrap(),
        "# my own instructions\n",
        "somebody's own instructions were deleted"
    );
}

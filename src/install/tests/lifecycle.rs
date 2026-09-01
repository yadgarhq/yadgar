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

//! `verify`: the only thing in the system that can see hook drift.
//!
//! Split out of one file because the repository caps a file at 500 lines
//! (D59's complexity gate); the helpers and fixtures live in the parent.

use std::path::Path;

use serde_json::json;

use super::super::health::{drift, Drift};
use super::super::{install_with, Layout};
use super::*;

#[test]
fn verify_catches_a_stale_command_path() {
    // The failure this prevents, twice over: a dead pipeline while every signal
    // reads healthy. A registration pointing at a path that no longer exists is
    // invisible to the daemon, so if this check cannot fail, nothing can.
    let home = scratch("verify-stale");
    install_with(&home, Path::new("/gone/for/good/yaadgaar")).unwrap();

    let found = drift(&home);
    assert!(
        found
            .iter()
            .any(|d| matches!(d, Drift::CommandMissing { .. })),
        "{found:#?}"
    );
    // And it is a failure, not a note: `verify` must exit non-zero.
    assert!(super::super::verify(&home).is_err());
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

// ─── Every detector, proved able to fire ─────────────────────────────────────
//
// **An empty-list assertion cannot falsify a detector.**
// `verify_is_structurally_clean_after_an_install` asserts the drift list is
// EMPTY, so deleting a check only ever makes it emptier and the test stays
// green. Measured: the four "absent" arms — `HookMissing`, `McpMissing`,
// `RulesMissing`, `ReferenceMissing` — were deleted TOGETHER and the whole
// suite passed, as were `HookUnknown` and the `CommandEphemeral` push. A
// machine with every hook deleted by hand, no MCP entry and no rules file
// reported `verify` clean.
//
// That is not four separate oversights, it is one structural hole: every arm
// needs a POSITIVE test that breaks one thing and watches that specific
// variant come back. `every_drift_variant_has_a_test_that_makes_it_fire`
// below is exhaustive over the enum, so a variant added without one does not
// compile.
//
// It matters more than the line count suggests. D76 makes `verify` the only
// thing in the system that can notice hook drift, precisely because the daemon
// cannot read `~/.claude/settings.json`. A `verify` that cannot detect absence
// is the dead-pipeline-reads-healthy failure D76 exists to prevent, rebuilt
// inside the component written to prevent it.

#[test]
fn an_install_removed_by_hand_is_never_reported_clean() {
    // All four absent arms at once, which is how they were deleted.
    let home = scratch("verify-nothing-installed");
    let layout = Layout::new(&home);
    std::fs::create_dir_all(layout.claude_dir()).unwrap();
    std::fs::write(layout.claude_md(), "# my own instructions\n").unwrap();

    let found = drift(&home);
    for (label, present) in [
        (
            "a deleted hook",
            found.iter().any(|d| matches!(d, Drift::HookMissing { .. })),
        ),
        (
            "a deleted MCP entry",
            found.iter().any(|d| matches!(d, Drift::McpMissing)),
        ),
        (
            "a deleted rules file",
            found
                .iter()
                .any(|d| matches!(d, Drift::RulesMissing { .. })),
        ),
        (
            "a CLAUDE.md with no reference",
            found
                .iter()
                .any(|d| matches!(d, Drift::ReferenceMissing { .. })),
        ),
    ] {
        assert!(present, "{label} went unreported: {found:#?}");
    }
    assert!(super::super::verify(&home).is_err());
}

#[test]
fn verify_catches_one_hook_deleted_by_hand() {
    // The realistic shape: somebody edits settings.json and takes one entry
    // out. Eleven of twelve handlers still fire, so nothing else notices.
    let home = scratch("verify-hook-gone");
    let layout = Layout::new(&home);
    install_with(&home, Path::new(BINARY)).unwrap();
    let mut settings = read_json(&layout.settings());
    settings["hooks"]
        .as_object_mut()
        .unwrap()
        .remove("UserPromptSubmit")
        .expect("the fixture never had the entry this test deletes");
    std::fs::write(layout.settings(), settings.to_string()).unwrap();

    let found = drift(&home);
    assert!(
        found.iter().any(|d| matches!(
            d,
            Drift::HookMissing { name, .. } if name == "prompt-recall"
        )),
        "{found:#?}"
    );
}

#[test]
fn verify_catches_a_hook_this_version_has_no_handler_for() {
    // A handler renamed in the binary but not in settings.json: the entry
    // fires `yaadgaar hook <something we no longer dispatch>` forever, and
    // nothing but this check can see it.
    let home = scratch("verify-hook-unknown");
    let layout = Layout::new(&home);
    install_with(&home, Path::new(BINARY)).unwrap();
    let mut settings = read_json(&layout.settings());
    settings["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "matcher": "", "hooks": [
            { "type": "command", "command": "/usr/local/bin/yaadgaar hook stop-checkpoint-old" }
        ]}));
    std::fs::write(layout.settings(), settings.to_string()).unwrap();

    let found = drift(&home);
    assert!(
        found.iter().any(|d| matches!(
            d,
            Drift::HookUnknown { name, .. } if name == "stop-checkpoint-old"
        )),
        "{found:#?}"
    );
}

#[test]
fn verify_catches_an_mcp_entry_of_the_wrong_shape() {
    // The entry live on machines today is `{"type":"http","url":…}`. An install
    // that half-landed leaves it there, and a client that cannot spawn a stdio
    // server just has no yadgar tools — silently.
    let home = scratch("verify-mcp-shape");
    let layout = Layout::new(&home);
    install_with(&home, Path::new(BINARY)).unwrap();

    let mut config = read_json(&layout.mcp_config());
    config["mcpServers"]["yadgar"]["type"] = json!("http");
    std::fs::write(layout.mcp_config(), config.to_string()).unwrap();
    let found = drift(&home);
    assert!(
        found
            .iter()
            .any(|d| matches!(d, Drift::McpWrongShape { .. })),
        "a non-stdio entry: {found:#?}"
    );

    let mut config = read_json(&layout.mcp_config());
    config["mcpServers"]["yadgar"] = json!({ "type": "stdio" });
    std::fs::write(layout.mcp_config(), config.to_string()).unwrap();
    let found = drift(&home);
    assert!(
        found
            .iter()
            .any(|d| matches!(d, Drift::McpWrongShape { .. })),
        "an entry naming no command: {found:#?}"
    );
}

#[test]
fn verify_catches_a_command_that_exists_today_and_will_not_last() {
    // Distinct from `CommandMissing`: the binary is right there, and the
    // registration is still doomed. The Python client was poisoned this way
    // three times — a path through a worktree or a temp venv, dead by the next
    // session, with nothing reporting it.
    let home = scratch("verify-ephemeral");
    let binary = home.join("yaadgaar"); // the scratch home is under the temp dir
    std::fs::write(&binary, "not really a binary").unwrap();
    install_with(&home, &binary).unwrap();

    let found = drift(&home);
    assert!(
        found
            .iter()
            .any(|d| matches!(d, Drift::CommandEphemeral { .. })),
        "{found:#?}"
    );
    // And not as a missing one: the file is there, so the two must not be
    // reported interchangeably — they have different fixes.
    assert!(
        !found
            .iter()
            .any(|d| matches!(d, Drift::CommandMissing { .. })),
        "{found:#?}"
    );
}

#[test]
fn verify_reports_an_unreadable_settings_file_rather_than_a_clean_bill() {
    // `drift` never returns an error, so the only way an unparseable
    // settings.json can be reported at all is as drift. If this arm goes, the
    // hook checks return early and `verify` prints OK over a file nothing can
    // read.
    let home = scratch("verify-unparseable");
    let layout = Layout::new(&home);
    install_with(&home, Path::new(BINARY)).unwrap();
    std::fs::write(layout.settings(), "{ \"hooks\": oops }").unwrap();

    let found = drift(&home);
    assert!(
        found.iter().any(|d| matches!(
            d,
            Drift::Unreadable { path, .. } if path.ends_with("settings.json")
        )),
        "{found:#?}"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_claude_md_is_reported_as_unreadable_not_as_a_missing_reference() {
    // The two have different fixes, and the comment in `check_rules` says so
    // without anything pinning it: telling somebody to re-run `install`
    // against a file nothing can read sends them in a circle.
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("verify-unreadable-claude-md");
    let layout = Layout::new(&home);
    install_with(&home, Path::new(BINARY)).unwrap();
    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o200)).unwrap();
    if std::fs::read_to_string(layout.claude_md()).is_ok() {
        std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        return; // Root, or a filesystem that ignores the mode.
    }

    let found = drift(&home);

    std::fs::set_permissions(layout.claude_md(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        found.iter().any(|d| matches!(
            d,
            Drift::Unreadable { path, .. } if path.ends_with("CLAUDE.md")
        )),
        "{found:#?}"
    );
    assert!(
        !found
            .iter()
            .any(|d| matches!(d, Drift::ReferenceMissing { .. })),
        "reported as a missing reference, which sends the fix in a circle: {found:#?}"
    );
}

#[test]
fn every_drift_variant_has_a_test_that_makes_it_fire() {
    // Exhaustive BY CONSTRUCTION: a variant added to `Drift` without a line
    // here does not compile, and the line has to name the test that breaks
    // something and watches that variant come back. This is the structural
    // half of the fix — the empty-list assertions cannot do it, because a
    // deleted detector only ever makes the list emptier.
    fn covered_by(drift: &Drift) -> &'static str {
        match drift {
            Drift::Unreadable { .. } => "verify_reports_an_unreadable_settings_file_rather_than_a_clean_bill",
            Drift::HookMissing { .. } => "verify_catches_one_hook_deleted_by_hand",
            Drift::HookDuplicated { .. } => {
                "registration::verify_sees_a_duplicate_hidden_behind_a_foreign_hook_in_the_same_entry"
            }
            Drift::HookUnknown { .. } => "verify_catches_a_hook_this_version_has_no_handler_for",
            Drift::CommandMissing { .. } => "verify_catches_a_stale_command_path",
            Drift::CommandEphemeral { .. } => {
                "verify_catches_a_command_that_exists_today_and_will_not_last"
            }
            Drift::McpMissing => "an_install_removed_by_hand_is_never_reported_clean",
            Drift::McpWrongShape { .. } => "verify_catches_an_mcp_entry_of_the_wrong_shape",
            Drift::McpCarriesCredential => "verify_catches_a_credential_left_in_the_mcp_config",
            Drift::RulesMissing { .. } => "an_install_removed_by_hand_is_never_reported_clean",
            Drift::ReferenceMissing { .. } => "an_install_removed_by_hand_is_never_reported_clean",
        }
    }
    assert_eq!(
        covered_by(&Drift::McpMissing),
        "an_install_removed_by_hand_is_never_reported_clean"
    );
}

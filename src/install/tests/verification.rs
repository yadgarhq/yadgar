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

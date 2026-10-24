//! The per-project registry record the memory engine's task gate reads
//! (yaadgaar #403, ADR-1000 ruling 6, yadgar #391 / m-agahi/yadgar#83).
//!
//! The engine stops seeding its own task list for a project yaadgaar holds.
//! WHICH projects yaadgaar holds is the gateway registry's answer, and only
//! this client can ask it: the engine must not use the yaadgaar credential,
//! and its daemon cannot see this client's files. So each SessionStart writes
//! what the gateway said into one small file, and the engine's host hook
//! reads it.
//!
//! ## The contract, as #403 states it
//!
//! - Path: `${YAADGAAR_STATE_DIR:-${XDG_STATE_HOME:-~/.local/state}/yaadgaar}`
//!   `/registry/<key>.json`, where `<key>` is Python's
//!   `urllib.parse.quote(project.casefold(), safe="")`.
//! - Body: `{project, registered, checked_at, client_version}`. `project` is
//!   the key sent to the gateway, unchanged.
//! - `registered: false` ONLY on an explicit "no such project" answer. On a
//!   network, auth or gateway fault NOTHING is written and the prior record
//!   stands; the engine then falls back to its `yadgarhq/` prefix.
//! - Atomic (temp + rename), rewritten every SessionStart, and never a reason
//!   for session start to fail.
//!
//! THE KEY IS THE ONE `serve` SENDS ([`crate::project::derive`], through
//! `ToolCaller::project`), which transcribes the engine's own mint. A second
//! derivation here would be a second rule to drift from it, and a drift fails
//! silently to the prefix.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::harness::Env;

/// The variable that names the client's whole state directory.
pub const STATE_DIR_ENV: &str = "YAADGAAR_STATE_DIR";

/// What the gateway said about one project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The listing loaded: the registry holds the project.
    Registered,
    /// The gateway answered that its registry names no such project.
    NotRegistered,
}

/// The record's file stem: `quote(project.casefold(), safe="")`.
///
/// Full Unicode case folding (`ß` -> `ss`, `K` (Kelvin) -> `k`), then every
/// UTF-8 byte outside `A-Z a-z 0-9 _ . - ~` as `%XX`, upper-case hex.
pub fn key(project: &str) -> String {
    let folded = unicase::UniCase::new(project).to_folded_case();
    let mut out = String::with_capacity(folded.len());
    for b in folded.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The directory every record lives in, or `None` with no home to put it in.
pub fn dir(env: &Env) -> Option<PathBuf> {
    Some(env.state_root()?.join("registry"))
}

/// Write the record for *project* into *dir*, atomically.
pub fn write(dir: &Path, project: &str, answer: Answer, now: i64) -> Result<PathBuf, String> {
    let name = format!("{}.json", key(project));
    let body = json!({
        "project": project,
        "registered": answer == Answer::Registered,
        "checked_at": crate::expiry::format_utc(now),
        "client_version": env!("CARGO_PKG_VERSION"),
    });
    let mut text = body.to_string();
    text.push('\n');
    crate::config::write_private(dir, &name, text.as_bytes())
        .map_err(|(p, e)| format!("{}: {e}", p.display()))?;
    Ok(dir.join(name))
}

/// Record *answer* for *project*, from session start. `None` (a fault, not
/// an answer) writes nothing. Never fails: a record that could not be written
/// is logged, and the engine keeps the prior one.
pub fn record(env: &Env, project: &str, answer: Option<Answer>) {
    let (Some(answer), Some(dir)) = (answer, dir(env)) else {
        return;
    };
    if let Err(e) = write(&dir, project, answer, crate::expiry::now()) {
        tracing::warn!("the registry record for {project} was not written: {e}");
    }
}

#[cfg(test)]
#[path = "registry/tests.rs"]
mod tests;

//! What the sync owns, per Claude Code list: the state file.
//!
//! OUTSIDE the Claude Code tree, at
//! `<YAADGAAR_STATE_DIR, else XDG_STATE_HOME or ~/.local/state + /yaadgaar>`
//! `/harness/claude-code/<list-id>.json`,
//! so nothing Claude Code does to its own directory can edit the record of
//! what this client wrote there. A file is this client's to change only while
//! BOTH this state lists it AND the file still carries the URN.
//!
//! `written` is the record as last written, kept as JSON and compared by
//! value, not by bytes: Claude Code rewrites a file in its own formatting
//! when a tool touches it, and a byte hash would read that as an edit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Env;

/// The state format this code reads and writes.
pub const FORMAT: u64 = 1;

/// One mirrored task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub harness_id: u64,
    pub number: i64,
    /// The yaadgaar version, status and title last seen.
    pub version: u64,
    pub status: String,
    pub title: String,
    /// The record as last written.
    pub written: Value,
}

/// The state of one list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub format: u64,
    /// The project this list mirrors. A list holds ONE project.
    pub project: String,
    /// The list directory this state is for. A second Claude Code config
    /// directory with the same list id is refused, never mirrored from here.
    pub dir: PathBuf,
    /// By URN.
    #[serde(default)]
    pub entries: BTreeMap<String, Entry>,
    /// Creates in flight, by URN: the INTENT record. A create saves the entry
    /// it is about to link here BEFORE the `hard_link`, so a crash between
    /// the link and the next save leaves a file this state can still name.
    /// The next pass adopts such a file only when the intent's id AND URN
    /// both match it; a URN file no intent names is never adopted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub creating: BTreeMap<String, Entry>,
    /// Completion markers, by harness task id (car 3). `task-update` writes
    /// one when it allows a `TaskUpdate` to `completed` on a mirrored task;
    /// `task-completed` closes the yaadgaar task only when it finds one for
    /// the same task, the same `prompt_id` AND the same `session_id`, and the
    /// marker is fresh ([`Marker::fresh`]). Any other `TaskUpdate` on that task
    /// clears it, and every sync pass prunes stale markers and markers for
    /// tasks it no longer mirrors. Optional on disk, so a car-2 state reads
    /// unchanged and [`FORMAT`] stays 1.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_completions: BTreeMap<String, Marker>,
}

/// One completion `task-update` allowed and `task-completed` has not seen yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// The `prompt_id` of the `TaskUpdate` call. `TaskCompleted` carries the
    /// same field and no `tool_use_id`, so this is what the match is on.
    pub prompt_id: String,
    /// The `session_id` of the `TaskUpdate` call. A named list is shared by
    /// every session that sets `CLAUDE_CODE_TASK_LIST_ID`, so another
    /// session's `TaskCompleted` must not consume this one's marker. Absent
    /// on disk reads as empty, which matches no session.
    #[serde(default)]
    pub session_id: String,
    /// When the marker was written, in seconds since the Unix epoch. A marker
    /// older than `completion_ttl_secs` is stale and completes nothing.
    /// Absent on disk reads as 0: stale.
    #[serde(default)]
    pub at: u64,
    /// Kept for a person reading the state; nothing matches on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

impl Marker {
    /// Is this marker younger than *ttl* at *now*? A marker from the future
    /// (a clock stepped back) is fresh only while the step is within *ttl*.
    pub fn fresh(&self, now: SystemTime, ttl: Duration) -> bool {
        epoch_secs(now).abs_diff(self.at) <= ttl.as_secs()
    }
}

/// *now* in whole seconds since the Unix epoch, as [`Marker::at`] stores it.
pub fn epoch_secs(now: SystemTime) -> u64 {
    now.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl State {
    pub fn new(project: &str, dir: &Path) -> Self {
        Self {
            format: FORMAT,
            project: project.to_string(),
            dir: dir.to_path_buf(),
            entries: BTreeMap::new(),
            creating: BTreeMap::new(),
            pending_completions: BTreeMap::new(),
        }
    }
}

/// The directory every list's state file lives in.
///
/// Under the client's ONE state root ([`Env::state_root`]), so
/// `YAADGAAR_STATE_DIR` moves this state and the registry record together.
pub fn state_dir(env: &Env) -> Option<PathBuf> {
    Some(env.state_root()?.join("harness/claude-code"))
}

/// The state file for *list_id* (already checked safe by the probe).
pub fn path(dir: &Path, list_id: &str) -> PathBuf {
    dir.join(format!("{list_id}.json"))
}

/// Read a state file. Absent is `Ok(None)`. Unreadable, not JSON, or another
/// format is an error: a state this cannot read is a state it must not
/// overwrite with a fresh one, because that would forget what it owns.
pub fn load(path: &Path) -> Result<Option<State>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{} could not be read ({e})", path.display())),
    };
    let state: State = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a valid sync state ({e})", path.display()))?;
    if state.format != FORMAT {
        return Err(format!(
            "{} is sync state format {}, and this build reads {FORMAT}",
            path.display(),
            state.format
        ));
    }
    Ok(Some(state))
}

/// Write a state file atomically: the file `0600`, its directory `0700`.
pub fn save(path: &Path, state: &State) -> Result<(), String> {
    let dir = path.parent().ok_or("the state path has no directory")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("the state path has no file name")?;
    let body = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    crate::config::write_private(dir, name, &body)
        .map_err(|(p, e)| format!("{} could not be written ({e})", p.display()))
}

#[cfg(test)]
#[path = "state/tests.rs"]
mod tests;

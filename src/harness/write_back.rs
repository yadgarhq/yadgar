//! Write-back, harness to yaadgaar (car 3 of `plans/harness-task-sync.md`).
//!
//! PURE. [`decide`] reads one `TaskUpdate` call on a MIRRORED task and says
//! what yaadgaar must be told, or why the call is refused. It sends nothing
//! and writes nothing: `hook::write_back` does both, under the sync's lock.
//!
//! The rules (ADR-1000):
//!
//! - a subject edit becomes `edit_task(title)`, with the mirror prefix
//!   (`#<number> ` and an optional `[blocked] `) taken off;
//! - `pending` <-> `in_progress` becomes `transition_task`, by the asymmetric
//!   table below, because `open` and `blocked` share `pending`;
//! - `completed` sends NOTHING here. It writes a marker, and `task-completed`
//!   closes the task when it finds that marker for the same `prompt_id`;
//! - `deleted`, a description or metadata edit, and a dependency edge are
//!   refused: the first would drop a mirror silently, the next two carry the
//!   URN, and yaadgaar has no edges to mirror.

use serde_json::Value;

use super::record;
use super::state::Entry;

/// The longest title yaadgaar stores, in characters (what JSON Schema's
/// `maxLength` counts, and what the gateway advertises on `edit_task`).
pub const TITLE_CAP: usize = 128; // ADR-0569-EXCEPTION(CB): task-db's own CHECK on a title (gateway `create::TITLE_MAX`), not a per-install knob.

/// What one `TaskUpdate` on a mirrored task asks of yaadgaar.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The new yaadgaar title, when the subject edit changes it.
    pub title: Option<String>,
    /// The yaadgaar status to move to, when the harness status asks for one.
    pub to: Option<&'static str>,
    /// The call completes the task: record a marker, send nothing.
    pub complete: bool,
    /// The file as the harness will write it once the call is allowed,
    /// WITHOUT a completion (that lands only if `task-completed` allows it).
    /// Stored as the state's `written`, so an allowed edit does not read as
    /// one that bypassed the hooks at the next sync.
    pub expected: Value,
}

/// [`decide`]'s answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Refuse the call, with the reason the agent is shown.
    Refuse(String),
    /// Allow it, after telling yaadgaar what [`Change`] says.
    Apply(Change),
}

/// The reason a title over [`TITLE_CAP`] is refused, wherever it is checked.
pub fn too_long(number: i64, chars: usize) -> String {
    format!(
        "a yaadgaar task title holds at most {TITLE_CAP} characters, and the new title of \
         #{number} has {chars}. Shorten the subject and retry."
    )
}

/// The new title in *subject*: the mirror prefix of task *number* taken off,
/// then trimmed. Only THIS task's own `#<number> ` is stripped, so a title
/// that itself starts with `#5` keeps it on task #7.
pub fn strip(subject: &str, number: i64) -> &str {
    let own = format!("#{number} ");
    let rest = subject.strip_prefix(own.as_str()).unwrap_or(subject);
    let rest = rest.strip_prefix("[blocked] ").unwrap_or(rest);
    rest.trim()
}

/// A title yaadgaar will take, or the reason it will not.
pub fn check_title(title: &str, number: i64) -> Result<(), String> {
    if title.is_empty() {
        return Err(format!(
            "the new subject of yaadgaar #{number} has an empty title, and a task needs one."
        ));
    }
    let chars = title.chars().count();
    if chars > TITLE_CAP {
        return Err(too_long(number, chars));
    }
    Ok(())
}

/// The yaadgaar move for a harness status, given the yaadgaar status last
/// seen. `None` sends nothing.
///
/// | harness       | yaadgaar now  | sent          |
/// | ------------- | ------------- | ------------- |
/// | `in_progress` | `open`        | `in_progress` |
/// | `in_progress` | `blocked`     | `in_progress` |
/// | `pending`     | `in_progress` | `open`        |
/// | anything else |               | nothing       |
///
/// `pending` on a `blocked` task sends nothing: a blocked task stays blocked.
pub fn transition(harness: &str, yaadgaar: &str) -> Option<&'static str> {
    match (harness, yaadgaar) {
        ("in_progress", "open" | "blocked") => Some("in_progress"),
        ("pending", "in_progress") => Some("open"),
        _ => None,
    }
}

/// Decide one `TaskUpdate` (*input* is its `tool_input`) on the mirrored
/// task *entry*, whose file on disk is *file*.
pub fn decide(entry: &Entry, file: &Value, input: &Value) -> Verdict {
    let n = entry.number;
    let status = input.get("status").and_then(Value::as_str);
    if status == Some("deleted") {
        return Verdict::Refuse(format!(
            "this task mirrors yaadgaar #{n} and cannot be deleted here. Drop it in yaadgaar \
             with mcp__yaadgaar__transition_task (to: dropped)."
        ));
    }
    let description = input.get("description");
    if description.is_some_and(|d| Some(d) != file.get("description"))
        || input.get("metadata").is_some_and(|m| !is_empty_object(m))
    {
        return Verdict::Refuse(format!(
            "the description and metadata of this task carry its yaadgaar id (#{n}), so they \
             cannot be edited here. Edit the task's body with mcp__yaadgaar__edit_task."
        ));
    }
    if ["addBlocks", "addBlockedBy"]
        .iter()
        .any(|k| input.get(k).is_some_and(|v| !is_empty_array(v)))
    {
        return Verdict::Refuse(format!(
            "yaadgaar has no dependency edges, so this task (yaadgaar #{n}) takes no addBlocks \
             or addBlockedBy."
        ));
    }

    let mut expected = file.clone();
    let mut title = None;
    if let Some(subject) = input.get("subject").and_then(Value::as_str) {
        // THE SUBJECT AS WRITTEN SENDS NOTHING. It is `printable`-escaped and
        // cut at 120 characters, so stripping it would write that cut, escaped
        // text into yaadgaar as the title.
        if file.get("subject").and_then(Value::as_str) != Some(subject) {
            // AND ANY EDIT OF A LOSSY SUBJECT IS REFUSED: one changed word in
            // a cut title would write the 120-character head and its `…` into
            // yaadgaar, under the 128 cap, with nothing to stop it.
            if record::lossy(&entry.title) {
                return Verdict::Refuse(format!(
                    "the subject shows the title of yaadgaar #{n} only in part (cut, or with \
                     control characters escaped), so an edit here would save that text as the \
                     title. Change the title with mcp__yaadgaar__edit_task."
                ));
            }
            let stripped = strip(subject, n);
            if let Err(reason) = check_title(stripped, n) {
                return Verdict::Refuse(reason);
            }
            if stripped != entry.title {
                title = Some(stripped.to_string());
            }
        }
        expected["subject"] = Value::from(subject);
    }
    for key in ["activeForm", "owner"] {
        if let Some(v) = input.get(key).and_then(Value::as_str) {
            expected[key] = Value::from(v);
        }
    }
    let (mut to, mut complete) = (None, false);
    match status {
        Some("completed") => complete = true,
        Some(s @ ("pending" | "in_progress")) => {
            to = transition(s, &entry.status);
            expected["status"] = Value::from(s);
        }
        _ => {}
    }
    Verdict::Apply(Change {
        title,
        to,
        complete,
        expected,
    })
}

fn is_empty_object(v: &Value) -> bool {
    v.as_object().is_some_and(serde_json::Map::is_empty)
}

fn is_empty_array(v: &Value) -> bool {
    v.as_array().is_some_and(Vec::is_empty)
}

/// The record a mirrored file is checked against: it must still carry *urn*.
pub fn owned(file: &Value, urn: &str) -> bool {
    record::owner(file) == Some(urn)
}

#[cfg(test)]
#[path = "write_back/tests.rs"]
mod tests;

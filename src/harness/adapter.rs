//! One adapter per harness, behind one interface (ADR-1000).
//!
//! Claude Code has a task list on disk that this client mirrors, behind the
//! probe. Every other harness (Codex, OpenCode, anything unknown) is
//! [`Generic`]: the banner, and nothing else. A Claude Code session whose probe
//! fails is ALSO reported through the banner only, with one line saying why.
//!
//! [`Harness::context`] runs the sync (car 2) when the probe says the list is
//! ready, and returns the harness part the banner gains: one line, or two
//! when the pass wrote entries (the second asks for one no-op `TaskUpdate`
//! so the Claude Code task panel shows the list). With the sync off
//! (`harness.scope = none` or the kill switch), it runs a delete-only pass
//! instead, which removes the files this client mirrored and writes nothing.

use serde_json::Value;

use crate::printable::printable;

use super::claude_code::{self, ClaudeCode, Probe};
use super::scope::Scope;
use super::shape::RecordShape;
use super::sync::{self, Input, Outcome, Task};
use super::{Env, SYNC_SWITCH};

/// What the banner and the sync need to know about this session's tasks.
pub struct Report<'a> {
    /// `harness.scope` as read: `Err` carries why it was refused.
    pub scope: Result<Scope, String>,
    /// The project the listing was for.
    pub project: &'a str,
    /// Every task the listing reached.
    pub tasks: &'a [Task],
    /// False when the listing stopped early.
    pub complete: bool,
    /// `completion_ttl_secs` as read (the default when it could not be).
    pub completion_ttl: std::time::Duration,
    /// Reads the project's done and dropped tasks ([`Input::closed`]).
    pub closed: Option<&'a dyn Fn() -> Result<sync::Closed, String>>,
}

/// The line a harness adds to the banner.
#[derive(Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// How many tasks are now in the harness's list: `0` unless a sync ran.
    pub mirrored: usize,
}

impl Line {
    fn note(text: String) -> Option<Self> {
        Some(Self { text, mirrored: 0 })
    }
}

/// One harness.
pub trait Harness {
    /// The line this harness adds to the banner, or `None` for no line.
    fn context(&self, env: &Env, shape: &RecordShape, report: &Report) -> Option<Line>;
}

/// Any harness with no task list this client can write.
pub struct Generic;

impl Harness for Generic {
    fn context(&self, _: &Env, _: &RecordShape, _: &Report) -> Option<Line> {
        None
    }
}

/// Every Claude Code line that is not a sync starts with this, so an agent and
/// a person can find it.
pub const NOT_SYNCED: &str = "TaskList not synced";

impl Harness for ClaudeCode {
    fn context(&self, env: &Env, shape: &RecordShape, report: &Report) -> Option<Line> {
        if env.sync_switch.as_deref() == Some("off") {
            return Line::note(self.off(env, shape, report, &format!("{SYNC_SWITCH}=off")));
        }
        let scope = match &report.scope {
            Err(reason) => return Line::note(format!("{NOT_SYNCED}: {reason}.")),
            Ok(Scope::Off) => {
                return Line::note(self.off(env, shape, report, "harness.scope = none"))
            }
            Ok(scope) => *scope,
        };
        let why = match self.probe(env, shape) {
            Probe::Ready { dir, listing } => {
                let input = Input {
                    project: report.project,
                    tasks: report.tasks,
                    complete: report.complete,
                    scope,
                    completion_ttl: report.completion_ttl,
                    closed: report.closed,
                };
                match contained(|| sync::run(&dir, &self.list_id, &listing, env, &input)) {
                    Ok(outcome) => {
                        return Some(Line {
                            text: synced(&outcome, report.complete),
                            mirrored: outcome.mirrored,
                        })
                    }
                    Err(reason) => reason,
                }
            }
            Probe::NoTools => "this session has no task tools (CLAUDE_CODE_ENABLE_TODO_TOOLS=1 \
                               turns them on)"
                .to_string(),
            Probe::BadListId(id) => format!("the list id {:?} is not a safe directory name", id),
            Probe::NoStore(reason) => reason,
            Probe::Mismatch(m) => {
                format!("the task list is off the recorded Claude Code format ({m})")
            }
        };
        Line::note(format!("{NOT_SYNCED}: {}.", printable(&why)))
    }
}

impl ClaudeCode {
    /// The sync is off (*why*). A list this client mirrored gets a DELETE-ONLY
    /// pass, so no stale mirror stays behind for an agent to edit while
    /// nothing writes its edits back. It creates and rewrites nothing, and a
    /// list with no state is not touched at all.
    fn off(&self, env: &Env, shape: &RecordShape, report: &Report, why: &str) -> String {
        let off = format!("TaskList sync is off ({why})");
        let Probe::Ready { dir, listing } = self.probe(env, shape) else {
            return format!("{off}.");
        };
        let input = Input {
            project: report.project,
            tasks: &[],
            complete: true,
            scope: Scope::Off,
            completion_ttl: report.completion_ttl,
            // A delete-only pass removes what the state owns, nothing else.
            closed: None,
        };
        match contained(|| sync::run(&dir, &self.list_id, &listing, env, &input)) {
            Ok(o) if o.deleted > 0 => format!("{off}; {} mirrored tasks removed.", o.deleted),
            Ok(_) => format!("{off}."),
            Err(reason) => format!(
                "{off}; the mirrored tasks were not removed: {}.",
                printable(&reason)
            ),
        }
    }
}

/// Run one pass in its OWN containment: a defect in the sync must cost the
/// sync, never the banner above it. Not `process::contain`, which the whole
/// hook already runs inside and which holds a lock that is not re-entrant.
fn contained(pass: impl FnOnce() -> Result<Outcome, String>) -> Result<Outcome, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(pass))
        .unwrap_or_else(|_| Err("the sync failed with an internal error".to_string()))
}

/// *n* copies, singular for one: "1 duplicate copy", "2 duplicate copies".
fn copies(n: usize, kind: &str) -> String {
    format!("{n} {kind}{}", if n == 1 { "copy" } else { "copies" })
}

/// The line for a pass that ran.
fn synced(o: &Outcome, complete: bool) -> String {
    let mut parts = vec![
        format!("{} new", o.created + o.recreated),
        format!("{} updated", o.updated + o.refreshed),
        format!("{} removed", o.deleted),
    ];
    if o.diverged > 0 {
        parts.push(format!(
            // Edits the write-back hooks never saw (car 4 writes them back).
            "{} edited past the write-back hooks and NOT saved to yaadgaar",
            o.diverged
        ));
    }
    if o.duplicates > 0 {
        parts.push(format!("{} removed", copies(o.duplicates, "duplicate ")));
    }
    if o.stale > 0 {
        let what = copies(o.stale, "");
        parts.push(format!("{what} of done or dropped tasks removed"));
    }
    if o.stale_kept > 0 {
        let what = copies(o.stale_kept, "");
        parts.push(format!(
            "{what} of tasks out of scope kept, their status could not be read"
        ));
    }
    if o.skipped > 0 {
        parts.push(format!("{} skipped, no task id", o.skipped));
    }
    let partial = if complete {
        ""
    } else {
        " The listing stopped early, so nothing was removed."
    };
    // A list created after startup is not shown by the Claude Code task panel
    // until an in-process TaskUpdate (even a no-op) makes it re-read the list.
    // The line NAMES the status: the harness status of the yaadgaar status the
    // state holds, which `write_back::transition` maps to no move for every
    // yaadgaar status. "Unchanged" alone left the agent to guess, and a wrong
    // guess (or the status of a file edited past the hooks) is a real
    // transition_task.
    let panel = o.first.map_or_else(String::new, |(id, status)| {
        format!(
            "\nBefore other work, call TaskUpdate once on taskId \"{id}\" with status \
             \"{status}\" (it sends nothing to yaadgaar), so the Claude Code task panel shows \
             this list (it misses a list created after startup)."
        )
    });
    format!(
        "TaskList holds {} yaadgaar tasks ({}). yaadgaar is the source of truth: change a \
         task with mcp__yaadgaar__edit_task or transition_task.{partial}{panel}",
        o.mirrored,
        parts.join(", ")
    )
}

/// The harness that sent *payload*. Anything not recognised is [`Generic`].
pub fn detect(payload: Option<&Value>, env: &Env) -> Box<dyn Harness> {
    match payload.and_then(|p| claude_code::detect(p, env)) {
        Some(cc) => Box::new(cc),
        None => Box::new(Generic),
    }
}

/// The harness line for a session-start banner, or `None` for no line.
///
/// `None` also when the recorded shape cannot be read, which is a build defect
/// a test pins; the banner must still go out.
#[cfg(test)]
pub fn session_line(payload: Option<&Value>, report: &Report, env: &Env) -> Option<String> {
    session_report(payload, report, env).map(|l| l.text)
}

/// [`session_line`], with the count of tasks the sync left in the harness list.
pub fn session_report(payload: Option<&Value>, report: &Report, env: &Env) -> Option<Line> {
    let shape = super::shape::recorded().ok()?;
    detect(payload, env).context(env, &shape, report)
}

#[cfg(test)]
#[path = "adapter/tests.rs"]
mod tests;

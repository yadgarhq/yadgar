//! One adapter per harness, behind one interface (ADR-1000).
//!
//! Claude Code has a task list on disk that this client can mirror, behind the
//! probe. Every other harness (Codex, OpenCode, anything unknown) is
//! [`Generic`]: the banner, and nothing else. A Claude Code session whose probe
//! fails is ALSO reported through the banner only, with one line saying why.
//!
//! READ-ONLY in this car. [`Harness::context`] reports what a sync WOULD do;
//! the car that writes adds `apply`.

use serde_json::Value;

use crate::printable::printable;

use super::claude_code::{self, ClaudeCode, Probe};
use super::scope::Scope;
use super::shape::RecordShape;
use super::{Env, SYNC_SWITCH};

/// What the banner needs to know about this session's tasks.
pub struct Report<'a> {
    /// `harness.scope` as read: `Err` carries why it was refused.
    pub scope: Result<Scope, String>,
    /// The status of every task the listing reached.
    pub statuses: &'a [&'a str],
}

/// One harness.
pub trait Harness {
    /// The line this harness adds to the banner, or `None` for no line.
    fn context(&self, env: &Env, shape: &RecordShape, report: &Report) -> Option<String>;
}

/// Any harness with no task list this client can write.
pub struct Generic;

impl Harness for Generic {
    fn context(&self, _: &Env, _: &RecordShape, _: &Report) -> Option<String> {
        None
    }
}

/// Every Claude Code line that is not a sync starts with this, so an agent and
/// a person can find it.
pub const NOT_SYNCED: &str = "TaskList not synced";

impl Harness for ClaudeCode {
    fn context(&self, env: &Env, shape: &RecordShape, report: &Report) -> Option<String> {
        if env.sync_switch.as_deref() == Some("off") {
            return Some(format!("TaskList sync is off ({SYNC_SWITCH}=off)."));
        }
        let scope = match &report.scope {
            Err(reason) => return Some(format!("{NOT_SYNCED}: {reason}.")),
            Ok(Scope::Off) => return Some("TaskList sync is off (harness.scope = none).".into()),
            Ok(scope) => *scope,
        };
        let why = match self.probe(env, shape) {
            Probe::Ready { .. } => {
                let wanted = report.statuses.iter().filter(|s| scope.holds(s)).count();
                let name = match scope {
                    Scope::AllOpen => "all-open",
                    Scope::Active => "active",
                    Scope::Off => "none",
                };
                return Some(format!(
                    "TaskList sync, dry run: would sync {wanted} tasks into this session's \
                     TaskList (harness.scope = {name}); nothing is written yet."
                ));
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
        Some(format!("{NOT_SYNCED}: {}.", printable(&why)))
    }
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
pub fn session_line(
    payload: Option<&Value>,
    scope: Result<Scope, String>,
    statuses: &[&str],
    env: &Env,
) -> Option<String> {
    let shape = super::shape::recorded().ok()?;
    let report = Report { scope, statuses };
    detect(payload, env).context(env, &shape, &report)
}

#[cfg(test)]
#[path = "adapter/tests.rs"]
mod tests;

//! The Claude Code adapter: which list this session has, and whether this
//! client may write it. READ-ONLY in this car (car 1 of
//! `plans/harness-task-sync.md`): the probe decides, and nothing is written.
//!
//! The probe FAILS CLOSED. A session without the task tools, a list id that is
//! not a safe directory name, a config directory that is not there, or a list
//! off the recorded shape all mean "do not write". Only [`Probe::Ready`] lets a
//! later car write, and every other outcome is said once in the banner.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::shape::{self, Listing, Mismatch, RecordShape};
use super::Env;

/// The longest list id accepted, as the engine seeder bounds it.
const LIST_ID_MAX: usize = 128; // ADR-0569-EXCEPTION(CB): the engine's own bound on the directory name (`task_seed.py:57`), not a per-install knob.

/// The Claude Code session this hook runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCode {
    /// `CLAUDE_CONFIG_DIR`, or `~/.claude`.
    pub base: PathBuf,
    /// `CLAUDE_CODE_TASK_LIST_ID`, or the payload's `session_id`. Not yet
    /// checked: [`ClaudeCode::probe`] refuses an unsafe one.
    pub list_id: String,
    /// The payload's `model`, which may be absent (hooks reference, F14).
    pub model: Option<String>,
}

/// What the probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The list may be written. The directory may not exist yet.
    Ready { dir: PathBuf, listing: Listing },
    /// The session has no task tools, so nothing would read the files.
    NoTools,
    /// The list id is not a safe directory name.
    BadListId(String),
    /// The Claude Code config directory is not there.
    NoStore(String),
    /// The list is off the recorded shape.
    Mismatch(Mismatch),
}

/// The Claude Code session named by *payload*, or `None` for any other harness.
///
/// Claude Code is recognised by a `transcript_path` inside its own config
/// directory, plus a `session_id`. Both are in every Claude Code hook payload
/// (hooks reference, common input fields). "Inside" is by path COMPONENT, so
/// `~/.claude-other/...` is not inside `~/.claude`.
pub fn detect(payload: &Value, env: &Env) -> Option<ClaudeCode> {
    let base = base_dir(env)?;
    let transcript = payload.get("transcript_path").and_then(Value::as_str)?;
    if !Path::new(transcript).starts_with(&base) {
        return None;
    }
    let session = payload.get("session_id").and_then(Value::as_str)?;
    Some(ClaudeCode {
        base,
        list_id: env
            .task_list_id
            .clone()
            .unwrap_or_else(|| session.to_string()),
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The config directory: `CLAUDE_CONFIG_DIR`, else `<home>/.claude`.
pub fn base_dir(env: &Env) -> Option<PathBuf> {
    env.claude_config_dir
        .clone()
        .or_else(|| env.home.as_ref().map(|h| h.join(".claude")))
}

/// A list id is used as a directory name, so it must not climb or hide:
/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`, the engine seeder's own rule.
pub fn safe_list_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    let first_ok = bytes.next().is_some_and(|b| b.is_ascii_alphanumeric());
    first_ok
        && id.len() <= LIST_ID_MAX
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Does this session have the task tools?
///
/// `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` turns them on for any model. ONLY `1`,
/// the documented value, counts: Claude Code documents no other, and a list
/// written for a session that has no tools is files nobody reads. Without it,
/// only Opus 4.7, Sonnet 4.6, Haiku 4.5 and earlier have them
/// (tools-reference, "Task tool availability"). `CLAUDE_CODE_ENABLE_TASKS=0`
/// selects the legacy `TodoWrite`, which keeps no such files. A model this
/// cannot read is taken to have no tools.
pub fn tools_present(env: &Env, model: Option<&str>) -> bool {
    if env.enable_tasks.as_deref() == Some("0") {
        return false;
    }
    if matches!(env.enable_todo_tools.as_deref(), Some("1")) {
        return true;
    }
    model.is_some_and(has_tools_by_default)
}

/// The newest version of each family that has the task tools by default.
const TOOLS_WITHOUT_OPT_IN: [(&str, (u32, u32)); 3] =
    [("opus", (4, 7)), ("sonnet", (4, 6)), ("haiku", (4, 5))]; // ADR-0569-EXCEPTION(CB): Claude Code's own documented list (tools-reference, "Task tool availability"), not a per-install knob.

/// Read a model id in either naming (`claude-opus-4-7`, `claude-3-5-haiku-…`),
/// ignoring a `[…]` suffix and a date. Anything unreadable has no tools.
fn has_tools_by_default(model: &str) -> bool {
    let model = model.split('[').next().unwrap_or_default();
    let mut tokens = model.split('-');
    if tokens.next() != Some("claude") {
        return false;
    }
    let mut family = None;
    let mut numbers = Vec::new();
    for token in tokens {
        if let Some((name, _)) = TOOLS_WITHOUT_OPT_IN.iter().find(|(name, _)| *name == token) {
            family = Some(*name);
        } else if (1..=2).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_digit()) {
            numbers.push(token.parse::<u32>().unwrap_or(u32::MAX));
        }
    }
    let (Some(family), Some(&major)) = (family, numbers.first()) else {
        return false;
    };
    let version = (major, numbers.get(1).copied().unwrap_or(0));
    TOOLS_WITHOUT_OPT_IN
        .iter()
        .any(|(name, newest)| *name == family && version <= *newest)
}

impl ClaudeCode {
    /// Decide whether the list may be written. Reads, never writes.
    pub fn probe(&self, env: &Env, shape: &RecordShape) -> Probe {
        if !tools_present(env, self.model.as_deref()) {
            return Probe::NoTools;
        }
        if !safe_list_id(&self.list_id) {
            return Probe::BadListId(self.list_id.clone());
        }
        if !self.base.is_dir() {
            return Probe::NoStore(format!(
                "the Claude Code config directory {} is not there",
                self.base.display()
            ));
        }
        let dir = self.base.join("tasks").join(&self.list_id);
        // Claude Code creates the list directory on its first task, so an
        // absent one is an empty list, not a missing store. `symlink_metadata`,
        // so a symlinked list is refused rather than followed elsewhere.
        match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Probe::Ready {
                    dir,
                    listing: Listing::default(),
                }
            }
            Ok(meta) if !meta.file_type().is_dir() => {
                return Probe::Mismatch(Mismatch {
                    entry: self.list_id.clone(),
                    reason: "the list is not a directory".into(),
                })
            }
            _ => {}
        }
        match shape::check_dir(&dir, shape) {
            Ok(listing) => Probe::Ready { dir, listing },
            Err(mismatch) => Probe::Mismatch(mismatch),
        }
    }
}

#[cfg(test)]
#[path = "claude_code/tests.rs"]
mod tests;

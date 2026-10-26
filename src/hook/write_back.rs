//! `yaadgaar hook task-update` and `yaadgaar hook task-completed`: an agent's
//! edit to a mirrored task is written back to yaadgaar (ADR-1000, car 3).
//!
//! `task-update` is a `PreToolUse` hook on `TaskUpdate`; `task-completed` is
//! the `TaskCompleted` event. Both run BEFORE the change lands, so a refusal
//! (exit 2, the reason on stderr) leaves the harness file exactly as it was.
//!
//! ## What passes through untouched
//!
//! Everything that is not a MIRRORED task: not Claude Code, the kill switch
//! or `harness.scope = none`, no state file, a state for another config dir
//! that shares the list id, a task id the state does not list, or a file that
//! no longer carries the URN. Those cost no gateway call and no config read.
//! A state or `harness.json` that cannot be read is NOT a pass-through when
//! the file at `taskId` carries a URN: that edit is refused with the reason,
//! so it never bypasses write-back in silence. An unreadable payload and a
//! panic pass through
//! (`process::run`): refusing every `TaskUpdate` on a client defect would lock
//! the agent out of its own checklist, and the next sync sees the edit.
//!
//! ## What refuses
//!
//! On a mirrored task: a delete, a description or metadata edit, an edge, a
//! bad title ([`crate::harness::write_back`]); yaadgaar refusing the write (a
//! stale `expect_version` or an illegal move), after which the file is
//! refreshed from yaadgaar, because yaadgaar wins; and a gateway that cannot be
//! reached, because allowing then would leave the two diverged.
//!
//! `task-completed` refuses a mirrored completion unless `task-update` left a
//! FRESH marker for the same task, the same `prompt_id` AND the same
//! `session_id` (`harness.json` `completion_ttl_secs`, default
//! [`crate::harness::scope::COMPLETION_TTL`]). `TaskCompleted` also fires when
//! an agent-team teammate ends its turn with tasks in progress, and that must
//! never close a yaadgaar task. Any other `TaskUpdate` on the task clears its
//! marker, and every sync pass prunes stale ones.
//!
//! KNOWN HAZARD, for car 4: `task-update` runs at `PreToolUse`, BEFORE the
//! permission check, and stores `written` as the file the harness WILL write.
//! If the call is then denied, yaadgaar holds the edit, the file does not, and
//! `written` describes a file that never existed. Car 4 must confirm in
//! `PostToolUse` (`tool_response.success`), or keep the pre-edit value as a
//! second accepted `written`, and the diff backstop must never trust
//! `written` alone.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::Decision;
use crate::config::{Config, ConfigError};
use crate::harness::claude_code;
use crate::harness::record::Desired;
use crate::harness::scope::Settings;
use crate::harness::state::{self, Entry, Marker, State};
use crate::harness::sync;
use crate::harness::write_back::{self, Verdict};
use crate::harness::Env;
use crate::printable::printable;
use crate::proxy::ToolCaller;

mod gateway;

use gateway::{bounded, call, caller, runtime, unreachable, Fault};

/// The `PreToolUse` handler, matcher `TaskUpdate`.
pub const TASK_UPDATE: &str = "task-update";

/// The `TaskCompleted` handler.
pub const TASK_COMPLETED: &str = "task-completed";

/// The handlers in this module. A panic in one of them passes the call
/// through (`process::run`).
#[cfg(test)]
pub const NAMES: [&str; 2] = [TASK_UPDATE, TASK_COMPLETED];

/// The text Claude Code reads, on `TaskCompleted`, as "the hook's file is
/// missing": exit 2 with it on stderr is a NON-blocking error (hooks
/// reference, "Exit code 2"). No refusal may carry it.
const MISSING_FILE: &str = "No such file or directory";

/// What a handler reads from outside, handed in so no test reads the real
/// environment, the real config or the live gateway.
pub struct Inputs<'a> {
    pub env: &'a Env,
    /// The saved login, read ONLY on the gateway path: a call that passes
    /// through never opens `config.json`.
    pub config: &'a dyn Fn() -> Result<Config, ConfigError>,
    /// The client's config directory, where `harness.json` lives.
    pub config_dir: &'a Path,
    pub deadline: Duration,
    /// The clock a completion marker is stamped and judged by.
    pub now: SystemTime,
}

/// Run handler *name* on *payload* with the process's own inputs.
pub fn run(name: &str, payload: &Value) -> Decision {
    handle(
        name,
        payload,
        Inputs {
            env: &Env::from_process(),
            config: &Config::load,
            config_dir: &crate::config::base_dir(),
            deadline: super::session_start::DEADLINE,
            now: SystemTime::now(),
        },
    )
}

/// [`run`], with every input a parameter.
pub fn handle(name: &str, payload: &Value, inputs: Inputs) -> Decision {
    let decision = match name {
        TASK_UPDATE => task_update(payload, inputs),
        TASK_COMPLETED => task_completed(payload, inputs),
        _ => Decision::Allow,
    };
    match decision {
        Decision::Deny(reason) => Decision::Deny(scrub(&reason)),
        allow => allow,
    }
}

/// *reason* without [`MISSING_FILE`], which would turn a refusal into a
/// non-blocking error. An `io::Error` text can carry it.
fn scrub(reason: &str) -> String {
    reason.replace(MISSING_FILE, "not found")
}

/// A mirrored task, found, with the sync's lock held.
struct Mirror {
    dir: PathBuf,
    spath: PathBuf,
    state: State,
    urn: String,
    id: u64,
    file: Value,
    settings: Settings,
    _lock: std::fs::File,
}

impl Mirror {
    fn entry(&self) -> Entry {
        self.state.entries[&self.urn].clone()
    }

    fn save(&self) -> Result<(), String> {
        state::save(&self.spath, &self.state)
    }
}

/// The mirrored task *task_id* in this session's list, or `None` when it is
/// not one (every pass-through case in the module docs). `Some(Err)` is a
/// task this hook cannot judge safely, and so refuses: the lock is held
/// elsewhere, or the state or `harness.json` cannot be read while the file
/// at *task_id* carries a yaadgaar URN. Passing such an edit through would
/// let it bypass write-back in silence.
fn locate(payload: &Value, inputs: &Inputs, task_id: &str) -> Option<Result<Mirror, String>> {
    let env = inputs.env;
    if env.sync_switch.as_deref() == Some("off") {
        return None;
    }
    let cc = claude_code::detect(payload, env)?;
    if !claude_code::safe_list_id(&cc.list_id) {
        return None;
    }
    let id: u64 = task_id.parse().ok()?;
    let sdir = state::state_dir(env)?;
    let spath = state::path(&sdir, &cc.list_id);
    let dir = cc.base.join("tasks").join(&cc.list_id);
    // A first, unlocked look, so a foreign task costs no lock and no config.
    let state = match load(&spath, &dir, id)? {
        Ok(state) => state,
        Err(refusal) => return Some(Err(refusal)),
    };
    mirrored(&state, &dir, id)?;
    // A harness.json the sync cannot read: the sync does not run, but this
    // file IS a mirror, so the edit is refused rather than lost.
    let settings = match Settings::from_dir(inputs.config_dir) {
        Ok(s) if s.scope == crate::harness::scope::Scope::Off => return None,
        Ok(s) => s,
        Err(why) => return Some(Err(cannot_judge(&why))),
    };
    let lock = match sync::lock(&sdir.join(format!("{}.lock", cc.list_id))) {
        Ok(lock) => lock,
        Err(_) => {
            return Some(Err(
                "the task list is busy (a yaadgaar sync holds its lock); the change was not \
                 made. Retry."
                    .into(),
            ))
        }
    };
    let state = match load(&spath, &dir, id)? {
        Ok(state) => state,
        Err(refusal) => return Some(Err(refusal)),
    };
    let (urn, file) = mirrored(&state, &dir, id)?;
    Some(Ok(Mirror {
        dir,
        spath,
        state,
        urn,
        id,
        file,
        settings,
        _lock: lock,
    }))
}

/// The state at *spath*, when it is the state of *dir*. `None`: no state, or
/// the state of another config dir that shares this list id; neither makes
/// task *id* a mirror. `Some(Err)`: the state cannot be read and the file at
/// *id* carries a URN, so the edit is refused.
fn load(spath: &Path, dir: &Path, id: u64) -> Option<Result<State, String>> {
    match state::load(spath) {
        Ok(Some(state)) if state.dir == dir => Some(Ok(state)),
        Ok(_) => None,
        Err(why) => {
            let file = sync::read_json(&dir.join(format!("{id}.json"))).ok()?;
            crate::harness::record::owner(&file)?;
            Some(Err(cannot_judge(&why)))
        }
    }
}

fn cannot_judge(why: &str) -> String {
    format!(
        "this task carries a yaadgaar URN, and {}; the change was not made. Fix that file, \
         or change the task with mcp__yaadgaar__edit_task or transition_task.",
        printable(why)
    )
}

/// The URN and file of harness task *id*, when *state* owns it AND the file
/// still carries that URN.
fn mirrored(state: &State, dir: &Path, id: u64) -> Option<(String, Value)> {
    let (urn, _) = state.entries.iter().find(|(_, e)| e.harness_id == id)?;
    let file = sync::read_json(&dir.join(format!("{id}.json"))).ok()?;
    write_back::owned(&file, urn).then(|| (urn.clone(), file))
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// `PreToolUse` on `TaskUpdate`.
fn task_update(payload: &Value, inputs: Inputs) -> Decision {
    if str_at(payload, "tool_name") != Some("TaskUpdate") {
        return Decision::Allow;
    }
    let Some(input) = payload.get("tool_input") else {
        return Decision::Allow;
    };
    let Some(task_id) = str_at(input, "taskId") else {
        return Decision::Allow;
    };
    let mut m = match locate(payload, &inputs, task_id) {
        None => return Decision::Allow,
        Some(Err(reason)) => return Decision::Deny(reason),
        Some(Ok(m)) => m,
    };
    let mut entry = m.entry();
    let verdict = write_back::decide(&entry, &m.file, input);
    // Any TaskUpdate on this task but an allowed completion clears its
    // marker: the completion it marked is not what this call does.
    let completes = matches!(&verdict, Verdict::Apply(c) if c.complete);
    if !completes
        && m.state
            .pending_completions
            .remove(&m.id.to_string())
            .is_some()
    {
        if let Err(e) = m.save() {
            tracing::warn!("the sync state was not saved after clearing a marker: {e}");
        }
    }
    let change = match verdict {
        Verdict::Refuse(reason) => return Decision::Deny(reason),
        Verdict::Apply(change) => change,
    };
    let prompt_id = str_at(payload, "prompt_id").filter(|p| !p.is_empty());
    if change.complete && prompt_id.is_none() {
        return Decision::Deny(format!(
            "this TaskUpdate carries no prompt_id, so completing yaadgaar #{} here cannot be \
             matched to it. Close it with mcp__yaadgaar__transition_task (to: done).",
            entry.number
        ));
    }
    if change.title.is_some() || change.to.is_some() {
        let mut sends = Vec::new();
        if let Some(title) = &change.title {
            sends.push(("edit_task", json!({ "title": title })));
        }
        if let Some(to) = change.to {
            sends.push(("transition_task", json!({ "to": to })));
        }
        if let Err(reason) = send_all(&mut m, &mut entry, &sends, &inputs, payload) {
            // Saved, so a refresh and any write that landed are recorded.
            if let Err(e) = m.save() {
                tracing::warn!("the sync state was not saved after a refusal: {e}");
            }
            return Decision::Deny(reason);
        }
        if let Some(title) = &change.title {
            entry.title.clone_from(title);
        }
        if let Some(to) = change.to {
            entry.status = to.to_string();
        }
    }
    entry.written = change.expected;
    m.state.entries.insert(m.urn.clone(), entry);
    if let (true, Some(prompt_id)) = (change.complete, prompt_id) {
        m.state.pending_completions.insert(
            m.id.to_string(),
            Marker {
                prompt_id: prompt_id.to_string(),
                session_id: str_at(payload, "session_id")
                    .unwrap_or_default()
                    .to_string(),
                at: state::epoch_secs(inputs.now),
                tool_use_id: str_at(payload, "tool_use_id").map(str::to_string),
            },
        );
    }
    match m.save() {
        Ok(()) => Decision::Allow,
        Err(e) => Decision::Deny(format!(
            "yaadgaar took the change, but the sync state could not be saved ({}); retry",
            printable(&e)
        )),
    }
}

/// `TaskCompleted`.
fn task_completed(payload: &Value, inputs: Inputs) -> Decision {
    let Some(task_id) = str_at(payload, "task_id") else {
        return Decision::Allow;
    };
    let mut m = match locate(payload, &inputs, task_id) {
        None => return Decision::Allow,
        Some(Err(reason)) => return Decision::Deny(reason),
        Some(Ok(m)) => m,
    };
    let mut entry = m.entry();
    let prompt_id = str_at(payload, "prompt_id").filter(|p| !p.is_empty());
    let session_id = str_at(payload, "session_id").filter(|s| !s.is_empty());
    let marked = m.state.pending_completions.get(task_id).is_some_and(|mk| {
        Some(mk.prompt_id.as_str()) == prompt_id
            && Some(mk.session_id.as_str()) == session_id
            && mk.fresh(inputs.now, m.settings.completion_ttl)
    });
    if !marked {
        return Decision::Deny(format!(
            "this task mirrors yaadgaar #{}. It is closed only by an explicit TaskUpdate to \
             completed, or by mcp__yaadgaar__transition_task (to: done).",
            entry.number
        ));
    }
    // CONSUMED whatever happens next: a retry is a new TaskUpdate, and so a
    // new marker.
    m.state.pending_completions.remove(task_id);
    let sent = send_all(
        &mut m,
        &mut entry,
        &[("transition_task", json!({ "to": "done" }))],
        &inputs,
        payload,
    );
    if sent.is_ok() {
        entry.status = "done".into();
        let mut written = m.file.clone();
        written["status"] = json!("completed");
        entry.written = written;
        m.state.entries.insert(m.urn.clone(), entry);
    }
    let saved = m.save();
    match (sent, saved) {
        (Err(reason), _) => Decision::Deny(reason),
        (Ok(()), Ok(())) => Decision::Allow,
        (Ok(()), Err(e)) => {
            tracing::warn!("yaadgaar closed the task, but the sync state was not saved: {e}");
            Decision::Allow
        }
    }
}

/// Send each `(tool, arguments)` in turn, each with the version the last one
/// returned. *entry* follows every call that landed, and the caller stores it
/// only when ALL of them did. On a failure the state keeps the version it
/// had, or the one `refresh` read from yaadgaar.
fn send_all(
    m: &mut Mirror,
    entry: &mut Entry,
    sends: &[(&str, Value)],
    inputs: &Inputs,
    payload: &Value,
) -> Result<(), String> {
    let caller = caller(inputs, &m.state.project, payload).map_err(|why| unreachable(&why))?;
    let deadline = inputs.deadline;
    let rt = runtime()?;
    let mut result = Ok(());
    for (tool, extra) in sends {
        let mut args = json!({ "id": m.urn, "expect_version": entry.version });
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        match rt.block_on(bounded(deadline, call(&caller, tool, args))) {
            Ok(answer) => match answer.get("version").and_then(Value::as_u64) {
                Some(version) => entry.version = version,
                None => {
                    result = Err(refresh(
                        m,
                        &rt,
                        &caller,
                        deadline,
                        "answered with no version",
                    ));
                    break;
                }
            },
            Err(Fault::Refused(why)) => {
                result = Err(refresh(m, &rt, &caller, deadline, &why));
                break;
            }
            Err(Fault::Unreachable(why)) => {
                result = Err(unreachable(&why));
                break;
            }
        }
    }
    rt.shutdown_background();
    // On a failure the state's version is NOT advanced past what it held,
    // even when an earlier call in *sends* landed. `refresh` replaced the
    // entry from yaadgaar if it could; if it could not, the stale version
    // makes the next sync see yaadgaar ahead and rewrite the file from it.
    result
}

/// Rewrite the file from yaadgaar (yaadgaar wins), and say why the call was
/// refused.
fn refresh(
    m: &mut Mirror,
    rt: &tokio::runtime::Runtime,
    caller: &ToolCaller,
    deadline: Duration,
    why: &str,
) -> String {
    let refused = format!("yaadgaar refused the change: {}", printable(why));
    let read = rt.block_on(bounded(
        deadline,
        call(caller, "read_task", json!({ "id": m.urn })),
    ));
    let outcome = match read.map(|task| apply_fresh(m, &task)) {
        Ok(Ok(())) => "the task was refreshed from yaadgaar; re-read it with TaskGet and retry",
        _ => "the task could not be refreshed from yaadgaar; retry",
    };
    format!("{refused}. The change was not made, and {outcome}.")
}

/// Write the task yaadgaar answered into the file and the state.
fn apply_fresh(m: &mut Mirror, task: &Value) -> Result<(), String> {
    let d = Desired {
        urn: m.urn.clone(),
        number: task
            .get("number")
            .and_then(Value::as_i64)
            .ok_or("no number")?,
        title: str_at(task, "title").ok_or("no title")?.to_string(),
        status: str_at(task, "status").ok_or("no status")?.to_string(),
        version: task
            .get("version")
            .and_then(Value::as_u64)
            .ok_or("no version")?,
    };
    if str_at(task, "id") != Some(m.urn.as_str()) {
        return Err("a different task".into());
    }
    if m.settings.scope.holds(&d.status) && sync::rewrite(&m.dir, m.id, &d)? {
        m.state.entries.insert(m.urn.clone(), sync::entry(m.id, &d));
    } else {
        if sync::remove(&m.dir, m.id, &m.urn)? {
            sync::stamp_highwatermark(&m.dir, m.id)?;
        }
        m.state.entries.remove(&m.urn);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/write_back.rs"]
mod tests;

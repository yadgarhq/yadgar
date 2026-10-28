//! One SessionStart sync pass into a Claude Code list (car 2).
//!
//! Called only after the probe said [`super::claude_code::Probe::Ready`]. It
//! holds the client's OWN lock for the whole pass (never Claude Code's
//! `.lock`, whose meaning is unknown), reads the state, plans with the pure
//! [`super::plan::plan`], applies, and saves the state. It never panics on
//! bad input and never writes a file the state does not own.
//!
//! A crash mid-pass never orphans a file: every create's intent is on disk
//! before its link ([`State::creating`]), and the next pass adopts a file
//! only by that intent. A rewrite or a delete that a crash leaves unrecorded
//! heals on the next pass: the rewritten file reads as yaadgaar moving on,
//! and the deleted one as a file that is gone.
//!
//! ONE FILE PER TASK (#458). Claude Code's fork copies the parent's list into
//! the new session's dir, and the new list id has no state. The record the
//! state owns is kept; with none owned, the lowest-id record of a task in
//! scope is ADOPTED. Every other record of it is deleted, and
//! `.highwatermark` is raised over them even when a later action fails. A file owned by nothing is never
//! written from: adoption only writes it from yaadgaar.
//!
//! A FORK ALSO COPIES RECORDS OF CLOSED TASKS (#460). A record no state owns,
//! of a task out of scope, is deleted only after yaadgaar lists that task as
//! done or dropped, read once per pass and only when such a record exists
//! (`sync/stale.rs`). A failed read keeps every such record.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::plan::plan;
use super::record::{self, Desired};
use super::scope::Scope;
use super::shape::Listing;
use super::state::{self, State};
use super::Env;

mod apply;
mod files;
mod stale;
use apply::apply;
pub(crate) use files::{read_json, remove, rewrite, stamp_highwatermark};

/// One yaadgaar task from the listing. `urn` and `version` are optional
/// because the listing must still render a banner from a row that lacks them;
/// the sync skips such a row instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub urn: Option<String>,
    pub number: i64,
    pub title: String,
    pub status: String,
    pub version: Option<u64>,
}

/// What a pass is asked to mirror.
pub struct Input<'a> {
    pub project: &'a str,
    pub tasks: &'a [Task],
    /// False when the listing stopped early: nothing is deleted then.
    pub complete: bool,
    pub scope: Scope,
    /// The freshness bound on a completion marker; staler ones are pruned.
    pub completion_ttl: std::time::Duration,
    /// Reads the project's done and dropped tasks from yaadgaar (#460).
    /// Called at most once, and only when the list holds a record this
    /// client wrote of a task out of scope that the state does not own.
    /// `None`: no such record is ever deleted.
    pub closed: Option<&'a dyn Fn() -> Result<Closed, String>>,
}

/// The project's done and dropped tasks: URN to task number.
pub type Closed = BTreeMap<String, i64>;

/// What a pass did, for the banner.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Entries the state owns after the pass.
    pub mirrored: usize,
    pub created: usize,
    pub recreated: usize,
    pub updated: usize,
    pub refreshed: usize,
    pub deleted: usize,
    pub diverged: usize,
    /// Extra copies of a mirrored task deleted (a fork's copied list, #458).
    pub duplicates: usize,
    /// Records of done or dropped tasks the state did not own, deleted (#460).
    pub stale: usize,
    /// Such records kept because yaadgaar could not be read (#460).
    pub stale_kept: usize,
    /// In-scope rows with no usable URN or version.
    pub skipped: usize,
    /// The lowest owned task id and its state's harness status; set when the pass wrote.
    pub first: Option<(u64, &'static str)>,
}

/// TEST-ONLY: a project name that makes [`run`] panic, so the adapter's
/// containment of a sync defect is testable. Compiled out of every build but
/// `cargo test`.
#[cfg(test)]
pub const PANIC_PROJECT: &str = "test/panic-in-the-sync";

/// How long a pass waits for another pass on the same list.
const LOCK_WAIT: Duration = Duration::from_secs(2); // ADR-0569-EXCEPTION(CC): bounds the session-start wait for a concurrent pass; far below the hook's own DEADLINE.

/// Run one pass into *dir*, the list the probe found ready with *listing*.
pub fn run(
    dir: &Path,
    list_id: &str,
    listing: &Listing,
    env: &Env,
    input: &Input,
) -> Result<Outcome, String> {
    #[cfg(test)]
    if input.project == PANIC_PROJECT {
        panic!("a planted panic in the sync");
    }
    let sdir = state::state_dir(env).ok_or("no state directory (no XDG_STATE_HOME and no home)")?;
    let spath = state::path(&sdir, list_id);
    // `scope: none` (or the kill switch) is a DELETE-ONLY pass: with no state,
    // this client owns nothing here, so nothing is created, not even the lock.
    if input.scope == Scope::Off && !spath.exists() {
        return Ok(Outcome::default());
    }
    private_dir(&sdir)?;
    let _lock = lock(&sdir.join(format!("{list_id}.lock")))?;
    let mut st = match state::load(&spath)? {
        Some(st) if st.project != input.project => {
            return Err(format!(
                "this task list already mirrors {}, not {}",
                crate::printable::printable(&st.project),
                crate::printable::printable(input.project)
            ))
        }
        // The state is keyed by list id alone, and two Claude Code config
        // directories can share one (CLAUDE_CODE_TASK_LIST_ID). Mirroring the
        // second from this state would recreate every entry there and lose
        // track of the first.
        Some(st) if st.dir != dir => {
            return Err(format!(
                "the sync state {} is for the list {}, not {}; this list is not mirrored",
                spath.display(),
                st.dir.display(),
                dir.display()
            ))
        }
        Some(st) => st,
        None => State::new(input.project, dir),
    };
    let files = read_files(dir, listing)?;
    let before = st.clone();
    adopt(&mut st, &files);
    let owned: Vec<u64> = st.entries.values().map(|e| e.harness_id).collect();
    if files
        .iter()
        .any(|(id, f)| !owned.contains(id) && record::engine_written(f))
    {
        return Err(
            "the memory engine's seeder also writes this list (#391); two writers, two id schemes"
                .into(),
        );
    }
    let (desired, skipped) = desired(input);
    let mut actions = plan(&desired, &st, &files, input.complete);
    let mut out = Outcome {
        skipped,
        ..Outcome::default()
    };
    // Only with the whole listing: then a task missing from it is truly out
    // of scope, and the banner's "nothing was removed" stays true otherwise.
    if let (true, Some(lookup)) = (input.complete, input.closed) {
        match stale::actions(&desired, &st, &files, lookup) {
            Ok(more) => actions.extend(more),
            Err(kept) => out.stale_kept = kept,
        }
    }
    let save = |st: &State| state::save(&spath, st);
    let applied = apply(
        dir, listing, &files, &desired, &actions, &mut st, &mut out, &save,
    );
    out.mirrored = st.entries.len();
    if out.created + out.recreated + out.updated + out.refreshed > 0 {
        let low = st.entries.values().min_by_key(|e| e.harness_id);
        out.first = low.map(|e| (e.harness_id, record::harness_status(&e.status)));
    }
    prune_markers(&mut st, input.completion_ttl);
    // Saved even after a failed action, so the state records what WAS done.
    if st != before || !spath.exists() {
        save(&st)?;
    }
    applied.map(|()| out)
}

/// Drop every completion marker that is stale, or whose task this state no
/// longer mirrors: neither can ever complete anything, and a stale one left
/// behind could match a later `TaskCompleted` by chance.
fn prune_markers(st: &mut State, ttl: std::time::Duration) {
    let now = std::time::SystemTime::now();
    let owned: Vec<String> = st
        .entries
        .values()
        .map(|e| e.harness_id.to_string())
        .collect();
    st.pending_completions
        .retain(|id, m| owned.contains(id) && m.fresh(now, ttl));
}

/// The state directory, created `0700` (an existing one is tightened).
fn private_dir(sdir: &Path) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", sdir.display());
    crate::config::create_private_dir(sdir).map_err(fail)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let meta = std::fs::symlink_metadata(sdir).map_err(fail)?;
        if meta.is_dir() && meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(sdir, std::fs::Permissions::from_mode(0o700)).map_err(fail)?;
        }
    }
    Ok(())
}

/// Settle the intents a crashed pass left ([`State::creating`]). A file is
/// adopted only when the intent names BOTH its id and its URN, and no other
/// entry owns that id. Every other intent is dropped: its link never landed,
/// or another writer took the id. A URN file no intent names stays foreign.
fn adopt(st: &mut State, files: &BTreeMap<u64, Value>) {
    for (urn, intent) in std::mem::take(&mut st.creating) {
        let ours = files
            .get(&intent.harness_id)
            .is_some_and(|f| record::owner(f) == Some(urn.as_str()));
        let taken = st
            .entries
            .iter()
            .any(|(u, e)| *u != urn && e.harness_id == intent.harness_id);
        if ours && !taken {
            st.entries.insert(urn, intent);
        }
    }
}

/// Hold the client's own lock on one list, waiting at most [`LOCK_WAIT`].
///
/// The write-back hooks take the SAME lock, so a hook and a pass never
/// interleave their read-modify-write of one state file.
pub(crate) fn lock(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err("another sync of this list holds the lock; skipped".into())
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!("the sync lock {}: {e}", path.display()))
            }
        }
    }
}

/// Every task file the probe listed, parsed.
fn read_files(dir: &Path, listing: &Listing) -> Result<BTreeMap<u64, Value>, String> {
    let mut files = BTreeMap::new();
    for id in &listing.ids {
        let path = dir.join(format!("{id}.json"));
        let value = read_json(&path)?;
        files.insert(*id, value);
    }
    Ok(files)
}

/// The in-scope rows, and how many in-scope rows had no usable URN or version.
fn desired(input: &Input) -> (Vec<Desired>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for t in input.tasks.iter().filter(|t| input.scope.holds(&t.status)) {
        match (&t.urn, t.version) {
            (Some(urn), Some(version)) if urn.starts_with(record::URN_SCHEME) => {
                out.push(Desired {
                    urn: urn.clone(),
                    number: t.number,
                    title: t.title.clone(),
                    status: t.status.clone(),
                    version,
                })
            }
            _ => skipped += 1,
        }
    }
    (out, skipped)
}

pub(crate) fn entry(id: u64, d: &Desired) -> state::Entry {
    state::Entry {
        harness_id: id,
        number: d.number,
        version: d.version,
        status: d.status.clone(),
        title: d.title.clone(),
        written: record::record(id, d),
    }
}

#[cfg(test)]
#[path = "sync/tests.rs"]
mod tests;

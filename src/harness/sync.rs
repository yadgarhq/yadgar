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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::plan::{plan, Action};
use super::record::{self, Desired};
use super::scope::Scope;
use super::shape::Listing;
use super::state::{self, State};
use super::Env;

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
}

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
    /// In-scope rows with no usable URN or version.
    pub skipped: usize,
}

/// TEST-ONLY: a project name that makes [`run`] panic, so the adapter's
/// containment of a sync defect is testable. Compiled out of every build but
/// `cargo test`.
#[cfg(test)]
pub const PANIC_PROJECT: &str = "test/panic-in-the-sync";

/// How long a pass waits for another pass on the same list.
const LOCK_WAIT: Duration = Duration::from_secs(2); // ADR-0569-EXCEPTION(CC): bounds the session-start wait for a concurrent pass; far below the hook's own DEADLINE.

/// How many ids past the first free one a create tries before giving up.
const CREATE_ATTEMPTS: u64 = 64; // ADR-0569-EXCEPTION(CB): bound on a retry loop against ids Claude Code takes concurrently, not a per-install knob.

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
    let actions = plan(&desired, &st, &files, input.complete);
    let mut out = Outcome {
        skipped,
        ..Outcome::default()
    };
    let save = |st: &State| state::save(&spath, st);
    let applied = apply(
        dir, listing, &files, &desired, &actions, &mut st, &mut out, &save,
    );
    out.mirrored = st.entries.len();
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

pub(crate) fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
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

/// Apply *actions* in order, recording each in *st* as it lands. *save*
/// persists *st*; a create calls it before each link (the intent record).
#[allow(clippy::too_many_arguments)]
fn apply(
    dir: &Path,
    listing: &Listing,
    files: &BTreeMap<u64, Value>,
    desired: &[Desired],
    actions: &[Action],
    st: &mut State,
    out: &mut Outcome,
    save: &dyn Fn(&State) -> Result<(), String>,
) -> Result<(), String> {
    let find = |urn: &str| desired.iter().find(|d| d.urn == urn);
    let mut next = files
        .keys()
        .copied()
        .chain(listing.highwatermark)
        .chain(st.entries.values().map(|e| e.harness_id))
        .max()
        .unwrap_or(0)
        + 1;
    // The intent record for EVERY create of this pass, saved ONCE before the
    // first link: one state write, not one per create. A create whose planned
    // id is taken re-plans from `next` and saves again (rare: only a file
    // that appeared after the scan).
    for action in actions {
        if let Action::Create { urn, .. } = action {
            let d = find(urn).ok_or("a create for a task not in scope")?;
            st.creating.insert(urn.clone(), entry(next, d));
            next += 1;
        }
    }
    if !st.creating.is_empty() {
        save(st)?;
    }
    let mut deleted_max = None;
    for action in actions {
        match action {
            Action::Create { urn, recreated } => {
                let d = find(urn).ok_or("a create for a task not in scope")?;
                let id = create(dir, &mut next, d, st, save)?;
                st.entries.insert(urn.clone(), entry(id, d));
                if *recreated {
                    out.recreated += 1;
                } else {
                    out.created += 1;
                }
            }
            Action::Update {
                urn,
                harness_id,
                refreshed,
            } => {
                let d = find(urn).ok_or("an update for a task not in scope")?;
                if rewrite(dir, *harness_id, d)? {
                    st.entries.insert(urn.clone(), entry(*harness_id, d));
                    if *refreshed {
                        out.refreshed += 1;
                    } else {
                        out.updated += 1;
                    }
                } else {
                    st.entries.remove(urn);
                }
            }
            Action::Keep { urn } => {
                if let (Some(d), Some(e)) = (find(urn), st.entries.get_mut(urn)) {
                    (e.version, e.status, e.title) = (d.version, d.status.clone(), d.title.clone());
                }
            }
            Action::Diverged { .. } => out.diverged += 1,
            Action::Delete { urn, harness_id } => {
                if remove(dir, *harness_id, urn)? {
                    out.deleted += 1;
                    deleted_max = deleted_max.max(Some(*harness_id));
                }
                st.entries.remove(urn);
            }
            Action::Forget { urn } => {
                st.entries.remove(urn);
            }
        }
    }
    match deleted_max {
        Some(id) => stamp_highwatermark(dir, listing.highwatermark.unwrap_or(0).max(id)),
        None => Ok(()),
    }
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

/// A temporary file BESIDE the list (in `tasks/`, same filesystem, not a list
/// directory the probe reads), created exclusively, never through a link.
fn temp_with(dir: &Path, body: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write as _;
    let parent = dir.parent().ok_or("the list has no parent directory")?;
    let stem = format!(
        ".yaadgaar-{}",
        dir.file_name().and_then(|n| n.to_str()).unwrap_or("list")
    );
    let (tmp, mut file) =
        crate::config::create_first_free(crate::config::temp_names(parent, &stem))
            .map_err(|e| format!("{}: {e}", parent.display()))?;
    if let Err(e) = file.write_all(body).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", tmp.display()));
    }
    Ok(tmp)
}

/// Create the record under the first free id from *next*, NEVER replacing a
/// file: `hard_link` fails on an existing name, which is a file that appeared
/// after the scan (Claude Code's own create), so the next id is tried.
///
/// CRASH-SAFE: the id it links is the one its intent ([`State::creating`])
/// already names on disk. A crash after the link and before the next save
/// leaves a file the next pass adopts by that intent, never a duplicate. A
/// taken id is replaced by a fresh one from *next*, saved before its link.
fn create(
    dir: &Path,
    next: &mut u64,
    d: &Desired,
    st: &mut State,
    save: &dyn Fn(&State) -> Result<(), String>,
) -> Result<u64, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let result = (|| {
        for attempt in 0..CREATE_ATTEMPTS {
            let id = match st.creating.get(&d.urn) {
                Some(intent) if attempt == 0 => intent.harness_id,
                _ => {
                    let id = *next;
                    *next += 1;
                    st.creating.insert(d.urn.clone(), entry(id, d));
                    save(st)?;
                    id
                }
            };
            let body = record::record(id, d).to_string();
            let tmp = temp_with(dir, body.as_bytes())?;
            let target = dir.join(format!("{id}.json"));
            let linked = std::fs::hard_link(&tmp, &target);
            let _ = std::fs::remove_file(&tmp);
            match linked {
                Ok(()) => {
                    #[cfg(test)]
                    tests::crash_point();
                    return Ok(id);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}: {e}", target.display())),
            }
        }
        Err(format!("no free task id after {CREATE_ATTEMPTS} tries"))
    })();
    // Settled either way: linked (the caller records the entry), or not.
    st.creating.remove(&d.urn);
    result
}

/// Rewrite an owned file. `false` when it no longer carries the URN: then it
/// is not ours, and it is left alone.
pub(crate) fn rewrite(dir: &Path, id: u64, d: &Desired) -> Result<bool, String> {
    let target = dir.join(format!("{id}.json"));
    if record::owner(&read_json(&target)?) != Some(d.urn.as_str()) {
        return Ok(false);
    }
    let tmp = temp_with(dir, record::record(id, d).to_string().as_bytes())?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", target.display())
    })?;
    Ok(true)
}

/// Delete an owned file, after checking again that it is ours.
pub(crate) fn remove(dir: &Path, id: u64, urn: &str) -> Result<bool, String> {
    let target = dir.join(format!("{id}.json"));
    match read_json(&target) {
        Ok(file) if record::owner(&file) == Some(urn) => std::fs::remove_file(&target)
            .map(|()| true)
            .map_err(|e| format!("{}: {e}", target.display())),
        _ => Ok(false),
    }
}

/// Raise `.highwatermark` to at least *id*, as Claude Code does on its own
/// deletes, so an id this client used is never handed out again.
///
/// NEVER LOWERS IT: the mark is read again right before the write, and the
/// larger of that and *id* is written, so a mark Claude Code raised after the
/// scan survives. A RACE REMAINS: Claude Code takes no lock this client knows
/// of (its `.lock` is unexplained, plan F10), so a delete Claude Code stamps
/// between this read and the rename below is overwritten with the smaller
/// value. The window is one read and one rename; ids it could hand out again
/// are only the ones that one delete freed.
///
/// The write-back hooks call this too, after a delete outside a pass.
pub(crate) fn stamp_highwatermark(dir: &Path, id: u64) -> Result<(), String> {
    let target = dir.join(super::shape::HIGHWATERMARK);
    let on_disk = std::fs::read_to_string(&target)
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if on_disk >= id {
        return Ok(());
    }
    let tmp = temp_with(dir, id.to_string().as_bytes())?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", target.display())
    })
}

#[cfg(test)]
#[path = "sync/tests.rs"]
mod tests;

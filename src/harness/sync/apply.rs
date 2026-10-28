//! Applying a pass's actions to the list, and the create that never replaces
//! a file. Split from `sync.rs` for the 500-line ceiling; the rules are the
//! ones its header states.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::super::plan::Action;
use super::super::record::{self, Desired};
use super::super::shape::Listing;
use super::super::state::State;
use super::files::{adopt_copy, drop_copy, remove, rewrite, stamp_highwatermark, temp_with};
use super::{entry, Outcome};

/// How many ids past the first free one a create tries before giving up.
const CREATE_ATTEMPTS: u64 = 64; // ADR-0569-EXCEPTION(CB): bound on a retry loop against ids Claude Code takes concurrently, not a per-install knob.

/// Apply *actions* in order, recording each in *st* as it lands. *save*
/// persists *st*; a create calls it before each link (the intent record).
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
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
    // The highest id deleted so far. Stamped into `.highwatermark` even when
    // a later action fails: a deleted id must never be handed out again.
    let mut deleted_max = None;
    let applied = actions.iter().try_for_each(|action| {
        let deleted = step(dir, desired, &mut next, action, st, out, save)?;
        deleted_max = deleted_max.max(deleted);
        Ok::<(), String>(())
    });
    let stamped = match deleted_max {
        Some(id) => stamp_highwatermark(dir, listing.highwatermark.unwrap_or(0).max(id)),
        None => Ok(()),
    };
    both(applied, stamped)
}

/// The result of a pass whose actions gave *applied* and whose stamp gave
/// *stamped*. When both failed, BOTH errors are said: the stamp's says a
/// deleted id may be handed out again, which the action's error does not.
fn both(applied: Result<(), String>, stamped: Result<(), String>) -> Result<(), String> {
    match (applied, stamped) {
        (Err(a), Err(s)) => Err(format!("{a}; and .highwatermark was not raised: {s}")),
        (applied, stamped) => applied.and(stamped),
    }
}

/// Apply one *action*, recording it in *st* and counting it in *out*. The id
/// it deleted, if it deleted a file.
fn step(
    dir: &Path,
    desired: &[Desired],
    next: &mut u64,
    action: &Action,
    st: &mut State,
    out: &mut Outcome,
    save: &dyn Fn(&State) -> Result<(), String>,
) -> Result<Option<u64>, String> {
    let find = |urn: &str| desired.iter().find(|d| d.urn == urn);
    match action {
        Action::Create { urn, recreated } => {
            let d = find(urn).ok_or("a create for a task not in scope")?;
            let id = create(dir, next, d, st, save)?;
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
            let removed = remove(dir, *harness_id, urn)?;
            st.entries.remove(urn);
            if removed {
                out.deleted += 1;
                return Ok(Some(*harness_id));
            }
        }
        Action::Forget { urn } => {
            st.entries.remove(urn);
        }
        Action::Adopt { urn, harness_id } => {
            let d = find(urn).ok_or("an adopt for a task not in scope")?;
            match adopt_copy(dir, *harness_id, d)? {
                Some(wrote) => {
                    st.entries.insert(urn.clone(), entry(*harness_id, d));
                    out.updated += usize::from(wrote);
                }
                // Changed hands since the scan: the next pass settles it.
                None => {
                    st.entries.remove(urn);
                }
            }
        }
        Action::Drop { urn, harness_id } => {
            let d = find(urn).ok_or("a drop for a task not in scope")?;
            // Only while a kept record of the task is owned: a failed
            // adopt must never leave the task with no file at all.
            let kept = st
                .entries
                .get(urn)
                .is_some_and(|e| e.harness_id != *harness_id);
            if kept && drop_copy(dir, *harness_id, d)? {
                out.duplicates += 1;
                return Ok(Some(*harness_id));
            }
        }
        Action::Stale {
            urn,
            number,
            harness_id,
        } => {
            // Checked again now: the file may have changed hands since the scan.
            if drop_copy(dir, *harness_id, &super::stale::probe(urn, *number))? {
                out.stale += 1;
                return Ok(Some(*harness_id));
            }
        }
    }
    Ok(None)
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
                    super::tests::crash_point();
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

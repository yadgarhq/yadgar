//! The sync planner: what one pass does, decided without touching anything.
//!
//! PURE. It takes the desired set D (yaadgaar rows in scope), the state S
//! (what this client owns) and the task files F (what is on disk), and
//! returns the actions. Every rule of the three-way table in
//! `plans/harness-task-sync.md` is here, so every rule is a unit test.
//!
//! A file that S does not list is never in an action this planner returns,
//! with one exception: a record of a task in D (its ownership line names the
//! task's URN, or it is a legacy seed of it) is adopted or dropped, so each
//! task has ONE file (#458). The sync adds a second exception after it reads
//! yaadgaar: [`Action::Stale`] (#460). Model-written tasks, engine-written
//! tasks and other tools' tasks are left exactly as they are.

use std::collections::BTreeMap;

use serde_json::Value;

use super::record::{self, Desired};
use super::state::State;

/// One thing a pass does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Write a new file under a newly allocated id. `recreated` when S had
    /// one whose file is gone: a mirrored entry is never silently dropped.
    Create { urn: String, recreated: bool },
    /// Rewrite an owned file from yaadgaar. `refreshed` when the file had
    /// been edited and yaadgaar moved too: yaadgaar wins.
    Update {
        urn: String,
        harness_id: u64,
        refreshed: bool,
    },
    /// Nothing to write; record the yaadgaar version in S.
    Keep { urn: String },
    /// The file was edited and yaadgaar did not move. Left alone; car 4
    /// writes such an edit back. Counted.
    Diverged { urn: String },
    /// Delete an owned file whose task is done, dropped or out of scope.
    Delete { urn: String, harness_id: u64 },
    /// Drop from S without touching the file: it is gone, or no longer
    /// carries this URN.
    Forget { urn: String },
    /// Take over an existing record of this task at `harness_id`, the LOWEST
    /// id that holds one when S owns none, and write it from yaadgaar. The record is a copy S
    /// does not own (a fork copies the parent's list, #458) or a legacy seed.
    Adopt { urn: String, harness_id: u64 },
    /// Delete another record of the same task: one file per URN.
    Drop { urn: String, harness_id: u64 },
    /// Delete a record S does not own of a task yaadgaar lists as done or
    /// dropped (#460). Never planned here: [`plan`] does not know a task's
    /// status out of scope. The sync adds it after a lookup.
    Stale {
        urn: String,
        number: i64,
        harness_id: u64,
    },
}

/// Plan one pass. *complete* is false when the listing stopped early: then
/// nothing is deleted, because a missing row may only be unread.
pub fn plan(
    desired: &[Desired],
    state: &State,
    files: &BTreeMap<u64, Value>,
    complete: bool,
) -> Vec<Action> {
    let mut actions = Vec::new();
    for d in desired {
        let urn = d.urn.clone();
        // Every record of this task on disk: ONE is kept. The one S owns, so
        // the live session's file (and an edit in it) survives; else the
        // LOWEST id, which is adopted.
        let copies: Vec<u64> = files
            .iter()
            .filter(|(_, f)| record::copy_of(f, d))
            .map(|(id, _)| *id)
            .collect();
        if let Some(&lowest) = copies.first() {
            let owned = state
                .entries
                .get(&d.urn)
                .map(|e| e.harness_id)
                .filter(|id| {
                    files
                        .get(id)
                        .is_some_and(|f| record::owner(f) == Some(d.urn.as_str()))
                });
            let keep = owned.unwrap_or(lowest);
            if owned.is_none() {
                actions.push(Action::Adopt {
                    urn: urn.clone(),
                    harness_id: keep,
                });
            }
            actions.extend(copies.iter().filter(|&&id| id != keep).map(|&harness_id| {
                Action::Drop {
                    urn: urn.clone(),
                    harness_id,
                }
            }));
            if owned.is_none() {
                continue;
            }
        }
        let Some(entry) = state.entries.get(&d.urn) else {
            actions.push(Action::Create {
                urn,
                recreated: false,
            });
            continue;
        };
        let Some(file) = files.get(&entry.harness_id) else {
            actions.push(Action::Create {
                urn,
                recreated: true,
            });
            continue;
        };
        if record::owner(file) != Some(d.urn.as_str()) {
            actions.push(Action::Forget { urn: urn.clone() });
            actions.push(Action::Create {
                urn,
                recreated: false,
            });
            continue;
        }
        let harness_id = entry.harness_id;
        let untouched = *file == entry.written;
        actions.push(if untouched {
            if *file == record::record(harness_id, d) {
                Action::Keep { urn }
            } else {
                Action::Update {
                    urn,
                    harness_id,
                    refreshed: false,
                }
            }
        } else if d.version > entry.version {
            Action::Update {
                urn,
                harness_id,
                refreshed: true,
            }
        } else {
            Action::Diverged { urn }
        });
    }
    for (urn, entry) in &state.entries {
        if desired.iter().any(|d| &d.urn == urn) {
            continue;
        }
        match files.get(&entry.harness_id) {
            Some(file) if record::owner(file) == Some(urn.as_str()) => {
                if complete {
                    actions.push(Action::Delete {
                        urn: urn.clone(),
                        harness_id: entry.harness_id,
                    });
                }
            }
            _ => actions.push(Action::Forget { urn: urn.clone() }),
        }
    }
    actions
}

#[cfg(test)]
#[path = "plan/tests.rs"]
mod tests;

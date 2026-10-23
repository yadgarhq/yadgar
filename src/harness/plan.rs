//! The sync planner: what one pass does, decided without touching anything.
//!
//! PURE. It takes the desired set D (yaadgaar rows in scope), the state S
//! (what this client owns) and the task files F (what is on disk), and
//! returns the actions. Every rule of the three-way table in
//! `plans/harness-task-sync.md` is here, so every rule is a unit test.
//!
//! A file that S does not list is never in an action: model-written tasks,
//! engine-written tasks and other tools' tasks are left exactly as they are.

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

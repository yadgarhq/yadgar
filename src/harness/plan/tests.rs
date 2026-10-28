use super::*;

use std::path::Path;

use serde_json::json;

use crate::harness::record::record;
use crate::harness::state::Entry;

fn d(n: i64, status: &str, version: u64) -> Desired {
    Desired {
        urn: format!("yadgar:task:{n}"),
        number: n,
        title: format!("task {n}"),
        status: status.into(),
        version,
    }
}

/// S owning *d* at harness id *id*, as last written, and the file on disk.
fn owned(state: &mut State, files: &mut BTreeMap<u64, Value>, id: u64, d: &Desired) {
    let written = record(id, d);
    state.entries.insert(
        d.urn.clone(),
        Entry {
            harness_id: id,
            number: d.number,
            version: d.version,
            status: d.status.clone(),
            title: d.title.clone(),
            written: written.clone(),
        },
    );
    files.insert(id, written);
}

fn empty() -> (State, BTreeMap<u64, Value>) {
    (State::new("p", Path::new("/d")), BTreeMap::new())
}

#[test]
fn a_task_not_yet_mirrored_is_created_one_record_per_task() {
    let (state, files) = empty();
    let want = [d(1, "open", 1), d(2, "in_progress", 1), d(3, "blocked", 1)];
    let actions = plan(&want, &state, &files, true);
    let created: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Create {
                urn,
                recreated: false,
            } => Some(urn.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(created, ["yadgar:task:1", "yadgar:task:2", "yadgar:task:3"]);
    assert_eq!(actions.len(), 3, "{actions:?}");
}

#[test]
fn an_unchanged_mirror_is_kept_and_nothing_is_written() {
    let (mut state, mut files) = empty();
    let t = d(1, "open", 1);
    owned(&mut state, &mut files, 5, &t);
    assert_eq!(
        plan(std::slice::from_ref(&t), &state, &files, true),
        [Action::Keep { urn: t.urn }]
    );
}

#[test]
fn a_task_yaadgaar_changed_is_updated() {
    let (mut state, mut files) = empty();
    owned(&mut state, &mut files, 5, &d(1, "open", 1));
    let mut moved = d(1, "in_progress", 2);
    moved.title = "renamed".into();
    assert_eq!(
        plan(&[moved], &state, &files, true),
        [Action::Update {
            urn: "yadgar:task:1".into(),
            harness_id: 5,
            refreshed: false
        }]
    );
}

#[test]
fn a_version_bump_with_the_same_record_is_kept_not_rewritten() {
    let (mut state, mut files) = empty();
    owned(&mut state, &mut files, 5, &d(1, "open", 1));
    // yaadgaar moved (a body edit, say) but nothing the record shows changed.
    let bumped = d(1, "open", 9);
    assert_eq!(
        plan(&[bumped], &state, &files, true),
        [Action::Keep {
            urn: "yadgar:task:1".into()
        }]
    );
}

#[test]
fn a_local_edit_with_yaadgaar_unmoved_is_left_and_counted() {
    let (mut state, mut files) = empty();
    let t = d(1, "open", 1);
    owned(&mut state, &mut files, 5, &t);
    files.get_mut(&5).unwrap()["status"] = json!("in_progress");
    assert_eq!(
        plan(std::slice::from_ref(&t), &state, &files, true),
        [Action::Diverged { urn: t.urn }]
    );
}

#[test]
fn a_local_edit_with_yaadgaar_moved_is_a_conflict_yaadgaar_wins() {
    let (mut state, mut files) = empty();
    owned(&mut state, &mut files, 5, &d(1, "open", 1));
    files.get_mut(&5).unwrap()["subject"] = json!("#1 the agent's name");
    assert_eq!(
        plan(&[d(1, "open", 2)], &state, &files, true),
        [Action::Update {
            urn: "yadgar:task:1".into(),
            harness_id: 5,
            refreshed: true
        }]
    );
}

#[test]
fn a_mirror_the_agent_deleted_is_recreated() {
    let (mut state, mut files) = empty();
    let t = d(1, "open", 1);
    owned(&mut state, &mut files, 5, &t);
    files.remove(&5);
    assert_eq!(
        plan(std::slice::from_ref(&t), &state, &files, true),
        [Action::Create {
            urn: t.urn,
            recreated: true
        }]
    );
}

#[test]
fn a_file_that_lost_its_urn_is_disowned_and_a_fresh_entry_made() {
    let (mut state, mut files) = empty();
    let t = d(1, "open", 1);
    owned(&mut state, &mut files, 5, &t);
    files.get_mut(&5).unwrap()["description"] = json!("someone else's now");
    assert_eq!(
        plan(std::slice::from_ref(&t), &state, &files, true),
        [
            Action::Forget { urn: t.urn.clone() },
            Action::Create {
                urn: t.urn,
                recreated: false
            }
        ]
    );
}

#[test]
fn a_task_gone_terminal_or_out_of_scope_is_deleted() {
    let (mut state, mut files) = empty();
    owned(&mut state, &mut files, 5, &d(1, "open", 1));
    owned(&mut state, &mut files, 6, &d(2, "open", 1));
    let actions = plan(&[d(2, "open", 1)], &state, &files, true);
    assert!(
        actions.contains(&Action::Delete {
            urn: "yadgar:task:1".into(),
            harness_id: 5
        }),
        "{actions:?}"
    );
}

#[test]
fn a_truncated_listing_deletes_nothing() {
    let (mut state, mut files) = empty();
    owned(&mut state, &mut files, 5, &d(1, "open", 1));
    let actions = plan(&[], &state, &files, false);
    assert!(
        !actions.iter().any(|a| matches!(a, Action::Delete { .. })),
        "{actions:?}"
    );
}

#[test]
fn an_owned_entry_whose_file_is_gone_or_not_ours_is_forgotten_not_deleted() {
    let (mut state, mut files) = empty();
    owned(&mut state, &mut files, 5, &d(1, "open", 1));
    owned(&mut state, &mut files, 6, &d(2, "open", 1));
    files.remove(&5);
    files.get_mut(&6).unwrap()["description"] = json!("taken over");
    let actions = plan(&[], &state, &files, true);
    assert_eq!(
        actions,
        [
            Action::Forget {
                urn: "yadgar:task:1".into()
            },
            Action::Forget {
                urn: "yadgar:task:2".into()
            }
        ]
    );
}

#[test]
fn a_file_the_state_does_not_list_is_never_in_any_action() {
    let (state, mut files) = empty();
    // A model-written task, and one that even carries a URN line.
    files.insert(1, json!({"id":"1","subject":"mine","description":"d","status":"pending","blocks":[],"blockedBy":[]}));
    files.insert(2, record(2, &d(7, "open", 1)));
    let actions = plan(&[], &state, &files, true);
    assert!(actions.is_empty(), "{actions:?}");
}

#[test]
fn a_re_run_after_a_clean_pass_only_keeps() {
    let (mut state, mut files) = empty();
    let want = [d(1, "open", 1), d(2, "blocked", 4)];
    for (i, t) in want.iter().enumerate() {
        owned(&mut state, &mut files, 10 + i as u64, t);
    }
    let actions = plan(&want, &state, &files, true);
    assert_eq!(actions.len(), want.len(), "{actions:?}");
    assert!(
        actions.iter().all(|a| matches!(a, Action::Keep { .. })),
        "{actions:?}"
    );
}

#[test]
fn copies_of_one_task_are_adopted_at_the_lowest_id_and_the_rest_dropped() {
    // #458: S owns nothing; a fork copied 3 and 7 in; the legacy seed holds 2.
    let (state, mut files) = empty();
    let want = d(1, "open", 1);
    files.insert(7, record(7, &want));
    files.insert(3, record(3, &want));
    files.insert(
        2,
        json!({
            "id": "2",
            "subject": "#1 task 1",
            "description": "yaadgaar #1 · yadgar:task:1 · open",
            "status": "pending",
            "blocks": [],
            "blockedBy": []
        }),
    );
    let actions = plan(std::slice::from_ref(&want), &state, &files, true);
    let urn = want.urn.clone();
    assert_eq!(
        actions,
        [
            Action::Adopt {
                urn: urn.clone(),
                harness_id: 2
            },
            Action::Drop {
                urn: urn.clone(),
                harness_id: 3
            },
            Action::Drop { urn, harness_id: 7 },
        ]
    );
}

#[test]
fn an_owned_record_at_the_lowest_id_is_kept_and_a_later_copy_dropped() {
    let (mut state, mut files) = empty();
    let want = d(1, "open", 1);
    owned(&mut state, &mut files, 2, &want);
    files.insert(9, record(9, &want));
    let actions = plan(std::slice::from_ref(&want), &state, &files, true);
    let urn = want.urn.clone();
    assert_eq!(
        actions,
        [
            Action::Drop {
                urn: urn.clone(),
                harness_id: 9
            },
            Action::Keep { urn },
        ]
    );
}

#[test]
fn an_owned_record_is_kept_over_a_lower_copy() {
    let (mut state, mut files) = empty();
    let want = d(1, "open", 1);
    owned(&mut state, &mut files, 7, &want);
    files.insert(3, record(3, &want));
    let actions = plan(std::slice::from_ref(&want), &state, &files, true);
    let urn = want.urn.clone();
    assert_eq!(
        actions,
        [
            Action::Drop {
                urn: urn.clone(),
                harness_id: 3
            },
            Action::Keep { urn },
        ]
    );
}

//! yaadgaar #460: a fork copies the parent's records of tasks that were done
//! or dropped when it forked. They are out of scope, so no in-scope pass
//! adopts or drops them, and the fork's state does not own them. The sync
//! deletes such a record only after yaadgaar says the task is done or
//! dropped.

use std::cell::Cell;

use super::*;

/// The lookup a pass is handed: *closed*, counting each call in *calls*.
fn lookup<'a>(
    closed: &'a Result<Closed, String>,
    calls: &'a Cell<usize>,
) -> impl Fn() -> Result<Closed, String> + 'a {
    move || {
        calls.set(calls.get() + 1);
        closed.clone()
    }
}

/// A lookup that must never run.
fn never() -> Result<Closed, String> {
    panic!("the done and dropped tasks were read when nothing needed them")
}

fn pass_closed(
    f: &Fixture,
    tasks: &[Task],
    complete: bool,
    closed: &dyn Fn() -> Result<Closed, String>,
) -> Result<Outcome, String> {
    let input = Input {
        project: PROJECT,
        tasks,
        complete,
        scope: Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
        closed: Some(closed),
    };
    run(&f.dir, "s-1", &listing(&f.dir), &f.env, &input)
}

fn closed_of(tasks: &[&Task]) -> Result<Closed, String> {
    Ok(tasks
        .iter()
        .map(|t| (t.urn.clone().unwrap(), t.number))
        .collect())
}

fn ids(dir: &Path) -> Vec<u64> {
    files(dir).keys().copied().collect()
}

fn write(f: &Fixture, id: u64, body: &str) {
    std::fs::create_dir_all(&f.dir).unwrap();
    std::fs::write(f.dir.join(format!("{id}.json")), body).unwrap();
}

/// A record this client wrote of *t*, as a fork copies it.
fn owned_copy(f: &Fixture, id: u64, t: &Task) {
    write(f, id, &record::record(id, &desired_of(t)).to_string());
}

/// A legacy seed record of *t*, with the harness status *status*.
fn legacy_copy(f: &Fixture, id: u64, t: &Task, status: &str) {
    let mut v: Value = serde_json::from_str(&legacy(id, t)).unwrap();
    v["status"] = json!(status);
    write(f, id, &v.to_string());
}

#[test]
fn an_unowned_record_of_a_done_or_dropped_task_is_deleted_and_its_id_stamped() {
    // Shape (a): records with an ownership line, copied from the parent, for
    // tasks that closed. The in-scope task's copy is adopted as before.
    let f = fixture("stale-owned");
    let live = task(1, "open", 1);
    let (done, dropped) = (task(2, "open", 1), task(5, "in_progress", 1));
    owned_copy(&f, 3, &live);
    owned_copy(&f, 4, &done);
    owned_copy(&f, 6, &dropped);
    let calls = Cell::new(0);
    let closed = closed_of(&[&done, &dropped]);

    let out = pass_closed(&f, &[live], true, &lookup(&closed, &calls)).unwrap();
    assert_eq!(
        (out.stale, out.stale_kept, out.created),
        (2, 0, 0),
        "{out:?}"
    );
    assert_eq!(ids(&f.dir), [3]);
    assert_eq!(calls.get(), 1);
    assert!(hwm(&f.dir) >= 6, "a deleted id is never handed out again");
}

#[test]
fn a_legacy_seed_record_of_a_done_task_is_deleted_whatever_its_status() {
    // Shape (b): `yaadgaar #<n> · <urn> · <status>`, no metadata, in every
    // harness status the forked list was seen to hold.
    let f = fixture("stale-legacy");
    let gone: Vec<Task> = (1..=3).map(|n| task(n, "done", 1)).collect();
    for (t, status) in gone.iter().zip(["completed", "pending", "in_progress"]) {
        legacy_copy(&f, t.number as u64, t, status);
    }
    let calls = Cell::new(0);
    let closed = closed_of(&gone.iter().collect::<Vec<_>>());

    let out = pass_closed(&f, &[], true, &lookup(&closed, &calls)).unwrap();
    assert_eq!(out.stale, 3, "{out:?}");
    assert!(ids(&f.dir).is_empty());
    assert!(hwm(&f.dir) >= 3);
}

#[test]
fn a_failed_read_of_yaadgaar_keeps_every_record_and_counts_them() {
    let f = fixture("stale-read-fails");
    let (a, b) = (task(2, "done", 1), task(3, "dropped", 1));
    owned_copy(&f, 2, &a);
    legacy_copy(&f, 3, &b, "pending");
    let before = snapshot(&f.dir);
    let calls = Cell::new(0);
    let down = Err("the gateway could not be reached".to_string());

    let out = pass_closed(&f, &[], true, &lookup(&down, &calls)).unwrap();
    assert_eq!((out.stale, out.stale_kept), (0, 2), "{out:?}");
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn a_record_whose_task_is_not_listed_done_or_dropped_is_kept() {
    // Not in the listing: a task of another project, one yaadgaar no longer
    // has, or one still open under a narrower scope. None is proof the task
    // closed, so none is deleted.
    let f = fixture("stale-not-listed");
    let (other, gone) = (task(8, "open", 1), task(9, "done", 1));
    owned_copy(&f, 8, &other);
    legacy_copy(&f, 9, &gone, "pending");
    let before = snapshot(&f.dir);
    let calls = Cell::new(0);
    let closed = closed_of(&[]);

    let out = pass_closed(&f, &[], true, &lookup(&closed, &calls)).unwrap();
    assert_eq!((out.stale, out.stale_kept), (0, 0), "{out:?}");
    assert_eq!(snapshot(&f.dir), before);
    assert_eq!(calls.get(), 1, "read once for both");
}

#[test]
fn a_legacy_record_whose_number_is_not_the_tasks_is_kept() {
    // The URN is closed, but the record says #4 and yaadgaar says #9: not a
    // strict legacy match, so not provably a record of that task.
    let f = fixture("stale-legacy-number");
    let gone = task(9, "done", 1);
    let mut wrong = gone.clone();
    wrong.number = 4;
    legacy_copy(&f, 4, &wrong, "pending");
    let before = snapshot(&f.dir);
    let calls = Cell::new(0);
    let closed = closed_of(&[&gone]);

    let out = pass_closed(&f, &[], true, &lookup(&closed, &calls)).unwrap();
    assert_eq!(out.stale, 0, "{out:?}");
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn files_with_no_ownership_line_are_never_touched_and_never_cost_a_read() {
    let f = fixture("stale-foreign");
    write(&f, 1, FOREIGN);
    let lookalike = r##"{"id":"2","subject":"#2 task 2","description":"mine","status":"pending","blocks":[],"blockedBy":[]}"##;
    write(&f, 2, lookalike);
    // An ownership line that names no yaadgaar URN.
    let odd = json!({"id":"3","subject":"x","description":format!("{}\n{}not-a-urn", record::FRAMING, record::URN_LINE),"status":"pending","blocks":[],"blockedBy":[]});
    write(&f, 3, &odd.to_string());
    let before = snapshot(&f.dir);

    let out = pass_closed(&f, &[], true, &never).unwrap();
    assert_eq!(out.stale, 0, "{out:?}");
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn a_forks_copies_of_tasks_in_scope_cost_no_read() {
    // They are adopted or dropped by the plan, as in #458.
    let f = fixture("stale-in-scope");
    let live = task(1, "open", 1);
    owned_copy(&f, 1, &live);
    owned_copy(&f, 2, &live);

    let out = pass_closed(&f, &[live], true, &never).unwrap();
    assert_eq!((out.duplicates, out.stale), (1, 0), "{out:?}");
    assert_eq!(ids(&f.dir), [1]);
}

#[test]
fn a_truncated_listing_deletes_no_stale_record_and_reads_nothing() {
    let f = fixture("stale-truncated");
    owned_copy(&f, 2, &task(2, "done", 1));
    let before = snapshot(&f.dir);

    let out = pass_closed(&f, &[], false, &never).unwrap();
    assert_eq!(out.stale, 0, "{out:?}");
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn a_record_the_state_owns_is_left_to_the_plan_and_costs_no_read() {
    // The common case: a mirrored task closes. Its owned file is deleted by
    // the plan, as before; the stale pass does not read yaadgaar for it.
    let f = fixture("stale-owned-by-state");
    pass(&f, &[task(1, "open", 1)]).unwrap();

    let out = pass_closed(&f, &[task(1, "done", 2)], true, &never).unwrap();
    assert_eq!((out.deleted, out.stale), (1, 0), "{out:?}");
    assert!(ids(&f.dir).is_empty());
}

#[test]
fn a_stale_record_that_changed_hands_after_the_scan_is_not_deleted() {
    let f = fixture("stale-recheck");
    let gone = task(2, "done", 1);
    owned_copy(&f, 2, &gone);
    let (seen, listed) = (scan(&f.dir), listing(&f.dir));
    let st = State::new(PROJECT, &f.dir);
    let closed = closed_of(&[&gone]);
    let actions = stale::actions(&[], &st, &seen, &|| closed.clone()).unwrap();
    assert!(
        matches!(actions[..], [Action::Stale { harness_id: 2, .. }]),
        "{actions:?}"
    );
    write(&f, 2, FOREIGN);
    let (mut st, mut out) = (st, Outcome::default());
    apply(
        &f.dir,
        &listed,
        &seen,
        &[],
        &actions,
        &mut st,
        &mut out,
        &|_| Ok(()),
    )
    .unwrap();
    assert_eq!(out.stale, 0, "{out:?}");
    assert_eq!(
        std::fs::read_to_string(f.dir.join("2.json")).unwrap(),
        FOREIGN
    );
}

#[test]
fn when_an_action_and_the_stamp_both_fail_both_errors_are_said() {
    let f = fixture("stale-two-errors");
    let gone = desired_of(&task(2, "done", 1));
    write(&f, 9, &record::record(9, &gone).to_string());
    let mut st = State::new(PROJECT, &f.dir);
    st.entries.insert(gone.urn.clone(), entry(9, &gone));
    let seen = scan(&f.dir);
    // A directory where the mark goes: the stamp's rename fails. Not a
    // permission trick, which a root CI runner would not feel.
    std::fs::create_dir_all(f.dir.join(".highwatermark/x")).unwrap();
    let listed = Listing {
        ids: vec![9],
        ..Listing::default()
    };
    let actions = [
        Action::Delete {
            urn: gone.urn.clone(),
            harness_id: 9,
        },
        Action::Update {
            urn: "yadgar:task:0000-404".into(),
            harness_id: 1,
            refreshed: false,
        },
    ];
    let mut out = Outcome::default();
    let err = apply(
        &f.dir,
        &listed,
        &seen,
        &[],
        &actions,
        &mut st,
        &mut out,
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(err.contains("an update for a task not in scope"), "{err}");
    assert!(err.contains(".highwatermark"), "{err}");
}

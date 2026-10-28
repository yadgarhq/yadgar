use super::*;

use std::path::PathBuf;

use serde_json::json;

use crate::harness::shape;
use crate::testserver::scratch_dir;

const PROJECT: &str = "yadgarhq/docs";

#[test]
fn a_file_that_changed_hands_after_the_plan_is_neither_rewritten_nor_deleted() {
    // The apply step checks ownership AGAIN, at the moment it writes: the
    // agent can edit a file between the scan and the write.
    let f = fixture("sync-recheck");
    std::fs::create_dir_all(&f.dir).unwrap();
    std::fs::write(f.dir.join("4.json"), FOREIGN).unwrap();
    let d = Desired {
        urn: "yadgar:task:0000-4".into(),
        number: 4,
        title: "t".into(),
        status: "open".into(),
        version: 1,
    };
    assert_eq!(rewrite(&f.dir, 4, &d), Ok(false));
    assert_eq!(remove(&f.dir, 4, &d.urn), Ok(false));
    assert_eq!(
        std::fs::read_to_string(f.dir.join("4.json")).unwrap(),
        FOREIGN
    );
}

struct Fixture {
    root: PathBuf,
    dir: PathBuf,
    env: Env,
}

fn fixture(label: &str) -> Fixture {
    let root = scratch_dir(label);
    let dir = root.join(".claude/tasks/s-1");
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    let env = Env {
        home: Some(root.clone()),
        state_home: Some(root.join("state")),
        enable_todo_tools: Some("1".into()),
        ..Env::default()
    };
    Fixture { root, dir, env }
}

fn task(n: i64, status: &str, version: u64) -> Task {
    Task {
        urn: Some(format!("yadgar:task:0000-{n}")),
        number: n,
        title: format!("task {n}"),
        status: status.into(),
        version: Some(version),
    }
}

/// The listing as the probe would hand it over: a real shape check.
fn listing(dir: &Path) -> Listing {
    if !dir.exists() {
        return Listing::default();
    }
    shape::check_dir(dir, &shape::recorded().unwrap())
        .expect("the list stays in the recorded shape")
}

fn pass(f: &Fixture, tasks: &[Task]) -> Result<Outcome, String> {
    pass_with(f, tasks, true, Scope::AllOpen)
}

fn pass_with(f: &Fixture, tasks: &[Task], complete: bool, scope: Scope) -> Result<Outcome, String> {
    let input = Input {
        project: PROJECT,
        tasks,
        complete,
        scope,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    };
    run(&f.dir, "s-1", &listing(&f.dir), &f.env, &input)
}

/// Every task file in the list, by id.
fn files(dir: &Path) -> BTreeMap<u64, Value> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".json") {
            let text = std::fs::read_to_string(e.path()).unwrap();
            out.insert(stem.parse().unwrap(), serde_json::from_str(&text).unwrap());
        }
    }
    out
}

fn urns(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = files(dir)
        .values()
        .filter_map(|f| record::owner(f).map(str::to_string))
        .collect();
    v.sort();
    v
}

/// Every byte under the list dir, for "nothing changed".
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

const FOREIGN: &str = r#"{"id":"1","subject":"the model's own","description":"mine","status":"in_progress","blocks":[],"blockedBy":[]}"#;

#[test]
fn every_non_terminal_task_becomes_one_record_carrying_its_urn() {
    let f = fixture("sync-create");
    let tasks = [
        task(1, "open", 1),
        task(2, "in_progress", 1),
        task(3, "blocked", 1),
    ];
    let out = pass(&f, &tasks).unwrap();
    assert_eq!(out.created, 3);
    assert_eq!(out.mirrored, 3);
    assert_eq!(
        urns(&f.dir),
        [
            "yadgar:task:0000-1",
            "yadgar:task:0000-2",
            "yadgar:task:0000-3"
        ]
    );
    // Every file is in the recorded shape and the subject names the number.
    let shape = shape::recorded().unwrap();
    for (id, file) in files(&f.dir) {
        shape::check_record(&id.to_string(), &file, &shape).unwrap();
        let number = file["metadata"]["yaadgaar"]["number"].as_i64().unwrap();
        assert!(file["subject"]
            .as_str()
            .unwrap()
            .starts_with(&format!("#{number} ")));
    }
}

#[test]
fn done_and_dropped_tasks_are_never_written_and_a_task_that_goes_terminal_is_deleted() {
    let f = fixture("sync-terminal");
    let tasks = [
        task(1, "open", 1),
        task(2, "done", 1),
        task(3, "dropped", 1),
    ];
    pass(&f, &tasks).unwrap();
    assert_eq!(urns(&f.dir), ["yadgar:task:0000-1"]);
    let id: u64 = *files(&f.dir).keys().next().unwrap();
    // #1 is now done: it leaves the listing, so its mirror goes.
    let out = pass(&f, &[]).unwrap();
    assert_eq!(out.deleted, 1);
    assert!(files(&f.dir).is_empty());
    // And `.highwatermark` covers it, so Claude Code never reuses the id.
    let hwm: u64 = std::fs::read_to_string(f.dir.join(".highwatermark"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(hwm >= id, "hwm {hwm} < deleted id {id}");
}

#[test]
fn a_foreign_task_file_is_never_touched_and_its_id_never_taken() {
    let f = fixture("sync-foreign");
    std::fs::create_dir_all(&f.dir).unwrap();
    std::fs::write(f.dir.join("1.json"), FOREIGN).unwrap();
    pass(&f, &[task(5, "open", 1), task(6, "open", 1)]).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.dir.join("1.json")).unwrap(),
        FOREIGN
    );
    let ids: Vec<u64> = files(&f.dir).keys().copied().collect();
    assert_eq!(ids, [1, 2, 3]);
    // Deleting everything ours still leaves the foreign file alone.
    pass(&f, &[]).unwrap();
    assert_eq!(files(&f.dir).keys().copied().collect::<Vec<_>>(), [1]);
    assert_eq!(
        std::fs::read_to_string(f.dir.join("1.json")).unwrap(),
        FOREIGN
    );
}

#[test]
fn a_file_that_appeared_after_the_scan_is_not_overwritten() {
    let f = fixture("sync-race");
    std::fs::create_dir_all(&f.dir).unwrap();
    let scanned = listing(&f.dir); // empty
                                   // Claude Code writes 1.json after the scan, before the create.
    std::fs::write(f.dir.join("1.json"), FOREIGN).unwrap();
    let input = Input {
        project: PROJECT,
        tasks: &[task(9, "open", 1)],
        complete: true,
        scope: Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    };
    run(&f.dir, "s-1", &scanned, &f.env, &input).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.dir.join("1.json")).unwrap(),
        FOREIGN
    );
    assert_eq!(urns(&f.dir), ["yadgar:task:0000-9"]);
}

#[test]
fn a_re_run_with_nothing_changed_writes_nothing() {
    let f = fixture("sync-idempotent");
    let tasks = [task(1, "open", 1), task(2, "blocked", 3)];
    pass(&f, &tasks).unwrap();
    let before = snapshot(&f.dir);
    let state_before =
        std::fs::read(state::path(&state::state_dir(&f.env).unwrap(), "s-1")).unwrap();
    let out = pass(&f, &tasks).unwrap();
    assert_eq!(
        (out.created, out.updated, out.deleted),
        (0, 0, 0),
        "{out:?}"
    );
    assert_eq!(out.mirrored, 2);
    assert_eq!(snapshot(&f.dir), before);
    let state_after =
        std::fs::read(state::path(&state::state_dir(&f.env).unwrap(), "s-1")).unwrap();
    assert_eq!(state_after, state_before);
}

#[test]
fn a_task_yaadgaar_changed_is_rewritten_in_place() {
    let f = fixture("sync-update");
    pass(&f, &[task(1, "open", 1)]).unwrap();
    let (id, _) = files(&f.dir).into_iter().next().unwrap();
    let mut moved = task(1, "in_progress", 2);
    moved.title = "renamed".into();
    let out = pass(&f, &[moved]).unwrap();
    assert_eq!(out.updated, 1);
    let file = &files(&f.dir)[&id];
    assert_eq!(file["subject"], "#1 renamed");
    assert_eq!(file["status"], "in_progress");
}

#[test]
fn a_conflict_refreshes_the_file_from_yaadgaar_and_a_lone_local_edit_is_left() {
    let f = fixture("sync-conflict");
    pass(&f, &[task(1, "open", 1)]).unwrap();
    let (id, mut file) = files(&f.dir).into_iter().next().unwrap();
    file["subject"] = json!("#1 the agent's name");
    std::fs::write(f.dir.join(format!("{id}.json")), file.to_string()).unwrap();
    // yaadgaar did not move: the edit stays, and is counted.
    let out = pass(&f, &[task(1, "open", 1)]).unwrap();
    assert_eq!(out.diverged, 1);
    assert_eq!(files(&f.dir)[&id]["subject"], "#1 the agent's name");
    // yaadgaar moved: yaadgaar wins.
    let out = pass(&f, &[task(1, "open", 2)]).unwrap();
    assert_eq!(out.refreshed, 1);
    assert_eq!(files(&f.dir)[&id]["subject"], "#1 task 1");
}

#[test]
fn a_mirror_the_agent_deleted_comes_back_under_a_new_id() {
    let f = fixture("sync-recreate");
    pass(&f, &[task(1, "open", 1)]).unwrap();
    let (id, _) = files(&f.dir).into_iter().next().unwrap();
    std::fs::remove_file(f.dir.join(format!("{id}.json"))).unwrap();
    let out = pass(&f, &[task(1, "open", 1)]).unwrap();
    assert_eq!(out.recreated, 1);
    let ids: Vec<u64> = files(&f.dir).keys().copied().collect();
    assert_eq!(ids.len(), 1);
    assert!(ids[0] > id, "an id was reused");
}

#[test]
fn a_truncated_listing_deletes_nothing() {
    let f = fixture("sync-truncated");
    pass(&f, &[task(1, "open", 1), task(2, "open", 1)]).unwrap();
    let out = pass_with(&f, &[task(2, "open", 1)], false, Scope::AllOpen).unwrap();
    assert_eq!(out.deleted, 0);
    assert_eq!(urns(&f.dir).len(), 2);
}

#[test]
fn a_narrower_scope_mirrors_less_and_deletes_what_fell_out() {
    let f = fixture("sync-scope");
    let tasks = [task(1, "open", 1), task(2, "in_progress", 1)];
    pass(&f, &tasks).unwrap();
    let out = pass_with(&f, &tasks, true, Scope::Active).unwrap();
    assert_eq!(out.deleted, 1);
    assert_eq!(urns(&f.dir), ["yadgar:task:0000-2"]);
}

#[test]
fn a_row_with_no_urn_or_version_is_skipped_and_counted() {
    let f = fixture("sync-skip");
    let mut no_urn = task(1, "open", 1);
    no_urn.urn = None;
    let mut bad_urn = task(2, "open", 1);
    bad_urn.urn = Some("not-a-task-urn".into());
    let mut no_version = task(3, "open", 1);
    no_version.version = None;
    let out = pass(&f, &[no_urn, bad_urn, no_version, task(4, "open", 1)]).unwrap();
    assert_eq!((out.skipped, out.created), (3, 1));
}

#[test]
fn a_shared_list_that_mirrors_another_project_is_refused() {
    let f = fixture("sync-other-project");
    pass(&f, &[task(1, "open", 1)]).unwrap();
    let before = snapshot(&f.dir);
    let input = Input {
        project: "someone/else",
        tasks: &[task(7, "open", 1)],
        complete: true,
        scope: Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    };
    let err = run(&f.dir, "s-1", &listing(&f.dir), &f.env, &input).unwrap_err();
    assert!(err.contains(PROJECT), "{err}");
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn a_list_the_engine_seeder_also_writes_is_refused() {
    let f = fixture("sync-engine");
    std::fs::create_dir_all(&f.dir).unwrap();
    let engine = r#"{"id":"675","subject":"675: Ship the backup runbook","description":"","status":"pending","blocks":[],"blockedBy":[]}"#;
    std::fs::write(f.dir.join("675.json"), engine).unwrap();
    let before = snapshot(&f.dir);
    let err = pass(&f, &[task(1, "open", 1)]).unwrap_err();
    assert!(err.contains("engine"), "{err}");
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn a_state_file_that_cannot_be_read_stops_the_pass_before_any_write() {
    let f = fixture("sync-bad-state");
    let sdir = state::state_dir(&f.env).unwrap();
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(state::path(&sdir, "s-1"), "not json").unwrap();
    assert!(pass(&f, &[task(1, "open", 1)]).is_err());
    assert!(!f.dir.exists(), "the list was written");
}

#[test]
fn a_pass_that_cannot_get_the_lock_is_skipped_and_says_so() {
    let f = fixture("sync-locked");
    let sdir = state::state_dir(&f.env).unwrap();
    std::fs::create_dir_all(&sdir).unwrap();
    let held = std::fs::File::create(sdir.join("s-1.lock")).unwrap();
    held.lock().unwrap();
    let err = pass(&f, &[task(1, "open", 1)]).unwrap_err();
    assert!(err.contains("lock"), "{err}");
    assert!(!f.dir.exists());
}

#[test]
fn no_state_home_and_no_home_is_refused() {
    let f = fixture("sync-no-state");
    let env = Env {
        state_home: None,
        home: None,
        ..f.env.clone()
    };
    let input = Input {
        project: PROJECT,
        tasks: &[task(1, "open", 1)],
        complete: true,
        scope: Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    };
    assert!(run(&f.dir, "s-1", &Listing::default(), &env, &input).is_err());
    assert!(!f.dir.exists());
    std::fs::remove_dir_all(&f.root).ok();
}

// ---- crash safety, the high-water mark, the state's own dir -----------------

thread_local! {
    /// TEST-ONLY: how many more links a create may make before the planted
    /// crash. `None`: no crash.
    static LINKS_BEFORE_CRASH: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Called right after a create's `hard_link` lands, before anything records
/// it: the worst moment for a crash.
pub(super) fn crash_point() {
    LINKS_BEFORE_CRASH.with(|left| match left.get() {
        Some(0) => panic!("a planted crash between a link and the state save"),
        Some(n) => left.set(Some(n - 1)),
        None => {}
    });
}

/// Run a pass that dies right after link number *links* + 1.
fn crashing_pass(f: &Fixture, tasks: &[Task], links: usize) {
    LINKS_BEFORE_CRASH.with(|left| left.set(Some(links)));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pass(f, tasks)));
    LINKS_BEFORE_CRASH.with(|left| left.set(None));
    assert!(crashed.is_err(), "the planted crash fired");
}

fn state_of(f: &Fixture) -> State {
    let sdir = state::state_dir(&f.env).unwrap();
    state::load(&state::path(&sdir, "s-1")).unwrap().unwrap()
}

fn save_state(f: &Fixture, st: &State) {
    let sdir = state::state_dir(&f.env).unwrap();
    state::save(&state::path(&sdir, "s-1"), st).unwrap();
}

fn desired_of(t: &Task) -> Desired {
    Desired {
        urn: t.urn.clone().unwrap(),
        number: t.number,
        title: t.title.clone(),
        status: t.status.clone(),
        version: t.version.unwrap(),
    }
}

#[test]
fn an_interrupted_pass_then_a_resumed_one_leaves_exactly_one_file_per_task() {
    let f = fixture("sync-crash-resume");
    let tasks: Vec<Task> = (1..=4).map(|n| task(n, "open", 1)).collect();
    // Dies after the second file is linked and before anything records it.
    crashing_pass(&f, &tasks, 1);
    assert_eq!(files(&f.dir).len(), 2, "the crash came after two links");
    let out = pass(&f, &tasks).unwrap();
    assert_eq!(out.mirrored, 4);
    let mut want: Vec<String> = tasks.iter().map(|t| t.urn.clone().unwrap()).collect();
    want.sort();
    assert_eq!(urns(&f.dir), want, "one file per task, none twice");
    // And the next pass agrees: nothing to create, nothing orphaned.
    let again = pass(&f, &tasks).unwrap();
    assert_eq!((again.created, again.mirrored), (0, 4), "{again:?}");
    assert_eq!(files(&f.dir).len(), 4);
}

#[test]
fn an_orphan_a_create_left_is_adopted_on_its_intent_or_its_urn() {
    let f = fixture("sync-orphan-intent");
    std::fs::create_dir_all(&f.dir).unwrap();
    let (a, b, c) = (task(1, "open", 1), task(2, "open", 1), task(3, "open", 1));
    // A crash before the save left 5.json, and the intent naming it.
    std::fs::write(
        f.dir.join("5.json"),
        record::record(5, &desired_of(&a)).to_string(),
    )
    .unwrap();
    // A URN file this client never wrote: the only intent for its URN names
    // another id, whose link never landed.
    let stranger = record::record(6, &desired_of(&b)).to_string();
    std::fs::write(f.dir.join("6.json"), &stranger).unwrap();
    // An intent whose id another writer took: the file is not ours.
    std::fs::write(
        f.dir.join("7.json"),
        FOREIGN.replace(r#""id":"1""#, r#""id":"7""#),
    )
    .unwrap();
    let mut st = State::new(PROJECT, &f.dir);
    st.creating
        .insert(a.urn.clone().unwrap(), entry(5, &desired_of(&a)));
    st.creating
        .insert(b.urn.clone().unwrap(), entry(8, &desired_of(&b)));
    st.creating
        .insert(c.urn.clone().unwrap(), entry(7, &desired_of(&c)));
    save_state(&f, &st);
    let before7 = std::fs::read_to_string(f.dir.join("7.json")).unwrap();

    let out = pass(&f, &[a.clone(), b.clone(), c.clone()]).unwrap();
    let st = state_of(&f);
    assert_eq!(
        st.entries[a.urn.as_deref().unwrap()].harness_id,
        5,
        "adopted, not duplicated"
    );
    assert!(st.creating.is_empty(), "every intent is settled");
    // #458: a file that carries a task's ownership line IS that task's record
    // (a fork copies the parent's list), so it is adopted, not written twice.
    // Adopting gives the file no say: the pass only ever rewrites it from
    // yaadgaar.
    assert_eq!(
        st.entries[b.urn.as_deref().unwrap()].harness_id,
        6,
        "adopted by its URN, not duplicated"
    );
    assert_ne!(st.entries[c.urn.as_deref().unwrap()].harness_id, 7);
    assert_eq!(
        std::fs::read_to_string(f.dir.join("6.json")).unwrap(),
        stranger,
        "untouched"
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.join("7.json")).unwrap(),
        before7,
        "untouched"
    );
    assert_eq!((out.created, out.recreated), (1, 0), "{out:?}");
    let owners_of_a = files(&f.dir)
        .values()
        .filter(|v| record::owner(v) == a.urn.as_deref())
        .count();
    assert_eq!(owners_of_a, 1);
}

#[test]
fn a_delete_never_lowers_a_high_water_mark_raised_after_the_scan() {
    let f = fixture("sync-hwm-raise");
    pass(&f, &[task(1, "open", 1), task(2, "open", 1)]).unwrap();
    // The scan the probe handed over saw no mark; Claude Code then deleted
    // its own task 9 and stamped 9 before this pass's delete.
    let scanned = listing(&f.dir);
    std::fs::write(f.dir.join(".highwatermark"), "9").unwrap();
    let input = Input {
        project: PROJECT,
        tasks: &[task(1, "open", 1), task(2, "done", 2)],
        complete: true,
        scope: Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    };
    let out = run(&f.dir, "s-1", &scanned, &f.env, &input).unwrap();
    assert_eq!(out.deleted, 1);
    let hwm = std::fs::read_to_string(f.dir.join(".highwatermark")).unwrap();
    assert_eq!(hwm.trim(), "9");
}

#[test]
fn a_high_water_mark_above_every_file_is_where_new_ids_start() {
    let f = fixture("sync-hwm-above");
    std::fs::create_dir_all(&f.dir).unwrap();
    std::fs::write(f.dir.join("1.json"), FOREIGN).unwrap();
    std::fs::write(f.dir.join(".highwatermark"), "20").unwrap();
    pass(&f, &[task(1, "open", 1)]).unwrap();
    let ids: Vec<u64> = files(&f.dir).keys().copied().collect();
    assert_eq!(
        ids,
        [1, 21],
        "an id at or below the mark is never handed out"
    );
}

#[test]
fn two_config_dirs_sharing_one_list_id_do_not_share_a_state() {
    let f = fixture("sync-two-dirs");
    pass(&f, &[task(1, "open", 1)]).unwrap();
    let before = snapshot(&f.dir);
    let sdir = state::state_dir(&f.env).unwrap();
    let state_before = std::fs::read(state::path(&sdir, "s-1")).unwrap();
    // A second CLAUDE_CONFIG_DIR, the same CLAUDE_CODE_TASK_LIST_ID, the same
    // state home.
    let other = f.root.join("other-claude/tasks/s-1");
    let input = Input {
        project: PROJECT,
        tasks: &[task(1, "open", 1)],
        complete: true,
        scope: Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    };
    let err = run(&other, "s-1", &Listing::default(), &f.env, &input).unwrap_err();
    assert!(err.contains("other-claude"), "{err}");
    assert!(!other.exists(), "nothing is written into the second dir");
    assert_eq!(snapshot(&f.dir), before);
    assert_eq!(
        std::fs::read(state::path(&sdir, "s-1")).unwrap(),
        state_before
    );
}

#[cfg(unix)]
#[test]
fn the_state_dir_is_owner_only_and_so_is_the_lock() {
    use std::os::unix::fs::PermissionsExt as _;
    let f = fixture("sync-modes");
    pass(&f, &[task(1, "open", 1)]).unwrap();
    let sdir = state::state_dir(&f.env).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&sdir), 0o700);
    assert_eq!(mode(&sdir.join("s-1.lock")), 0o600);
    assert_eq!(mode(&state::path(&sdir, "s-1")), 0o600);
}

#[cfg(unix)]
#[test]
fn an_existing_loose_state_dir_is_tightened_to_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let f = fixture("sync-tighten");
    let sdir = state::state_dir(&f.env).unwrap();
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::set_permissions(&sdir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&sdir), 0o755, "the fixture starts loose");
    pass(&f, &[task(1, "open", 1)]).unwrap();
    assert_eq!(mode(&sdir), 0o700);
}

#[test]
fn a_delete_only_pass_removes_the_mirror_and_writes_nothing_new() {
    let f = fixture("sync-delete-only");
    pass(&f, &[task(1, "open", 1), task(2, "open", 1)]).unwrap();
    std::fs::write(
        f.dir.join("9.json"),
        FOREIGN.replace(r#""id":"1""#, r#""id":"9""#),
    )
    .unwrap();
    let out = pass_with(
        &f,
        &[task(1, "open", 1), task(3, "open", 1)],
        true,
        Scope::Off,
    )
    .unwrap();
    assert_eq!(
        (out.created, out.deleted, out.mirrored),
        (0, 2, 0),
        "{out:?}"
    );
    assert_eq!(files(&f.dir).keys().copied().collect::<Vec<_>>(), [9]);
}

#[test]
fn a_delete_only_pass_with_no_state_creates_nothing_at_all() {
    let f = fixture("sync-delete-only-fresh");
    let out = pass_with(&f, &[task(1, "open", 1)], true, Scope::Off).unwrap();
    assert_eq!(out, Outcome::default());
    assert!(!f.dir.exists());
    assert!(
        !f.root.join("state").exists(),
        "no state dir, no lock, no state file"
    );
}

#[test]
fn every_pass_prunes_stale_markers_and_markers_for_tasks_it_no_longer_mirrors() {
    use crate::harness::state::Marker;
    let f = fixture("sync-prune-markers");
    pass(&f, &[task(1, "open", 1), task(2, "open", 1)]).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let marker = |at: u64| Marker {
        prompt_id: "p".into(),
        session_id: "s-1".into(),
        at,
        tool_use_id: None,
    };
    let mut st = state_of(&f);
    let ttl = crate::harness::scope::COMPLETION_TTL.as_secs();
    st.pending_completions.insert("1".into(), marker(now));
    st.pending_completions
        .insert("2".into(), marker(now - 2 * ttl));
    st.pending_completions.insert("99".into(), marker(now));
    save_state(&f, &st);
    pass(&f, &[task(1, "open", 1), task(2, "open", 1)]).unwrap();
    let kept: Vec<String> = state_of(&f).pending_completions.into_keys().collect();
    assert_eq!(kept, ["1"]);
}

// yaadgaar #458: a fork copies the parent's list into the new session's dir.
// The new list id has no state, so before the fix every copied record read
// as foreign and the pass wrote a second set above the copied mark.

/// One pass on the list *id* beside the fixture's own list.
fn pass_on(f: &Fixture, id: &str, tasks: &[Task]) -> Result<Outcome, String> {
    let dir = f.root.join(".claude/tasks").join(id);
    let input = Input {
        project: PROJECT,
        tasks,
        complete: true,
        scope: Scope::AllOpen,
        completion_ttl: crate::harness::scope::COMPLETION_TTL,
    };
    run(&dir, id, &listing(&dir), &f.env, &input)
}

/// Copy every entry of *from* into a new dir *to*, as a fork does.
fn copy_list(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

fn hwm(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join(".highwatermark"))
        .map(|t| t.trim().parse().unwrap())
        .unwrap_or(0)
}

/// The record the hand-run `seed_tasks.py` wrote on 2026-09-27: no metadata,
/// no ownership line, the URN inside a one-line description.
fn legacy(id: u64, t: &Task) -> String {
    json!({
        "id": id.to_string(),
        "subject": format!("#{} {}", t.number, t.title),
        "description": format!("yaadgaar #{} · {} · {}", t.number, t.urn.as_deref().unwrap(), t.status),
        "status": "pending",
        "blocks": [],
        "blockedBy": []
    })
    .to_string()
}

#[test]
fn a_forked_list_adopts_the_copied_mirror_and_a_second_pass_writes_nothing() {
    let f = fixture("sync-fork");
    let tasks: Vec<Task> = (1..=3).map(|n| task(n, "open", 1)).collect();
    pass(&f, &tasks).unwrap();
    let fork = f.root.join(".claude/tasks/s-2");
    copy_list(&f.dir, &fork);
    let before = snapshot(&fork);

    let out = pass_on(&f, "s-2", &tasks).unwrap();
    assert_eq!((out.created, out.recreated), (0, 0), "{out:?}");
    assert_eq!(out.mirrored, 3);
    assert_eq!(snapshot(&fork), before, "the copies are already current");
    let st = state::load(&state::path(&state::state_dir(&f.env).unwrap(), "s-2"))
        .unwrap()
        .unwrap();
    let mut owned: Vec<u64> = st.entries.values().map(|e| e.harness_id).collect();
    owned.sort_unstable();
    assert_eq!(owned, [1, 2, 3], "the fork owns the copied ids");

    // The SessionStart after the next resume or compact: nothing new.
    let again = pass_on(&f, "s-2", &tasks).unwrap();
    assert_eq!((again.created, again.updated), (0, 0), "{again:?}");
    assert_eq!(snapshot(&fork), before);
}

#[test]
fn two_files_for_one_urn_collapse_to_the_owned_one_updated_in_place() {
    // The state of bcdb580d: the fork's state owns 5, the copied 2 carries
    // the same URN. The owned record wins over a lower copy: the live
    // session's own file is never the one deleted.
    let f = fixture("sync-collapse");
    std::fs::create_dir_all(&f.dir).unwrap();
    let t = task(1, "open", 1);
    std::fs::write(
        f.dir.join("2.json"),
        record::record(2, &desired_of(&t)).to_string(),
    )
    .unwrap();
    std::fs::write(
        f.dir.join("5.json"),
        record::record(5, &desired_of(&t)).to_string(),
    )
    .unwrap();
    let mut st = State::new(PROJECT, &f.dir);
    st.entries
        .insert(t.urn.clone().unwrap(), entry(5, &desired_of(&t)));
    save_state(&f, &st);

    let mut moved = task(1, "in_progress", 2);
    moved.title = "renamed".into();
    let out = pass(&f, &[moved]).unwrap();
    assert_eq!((out.created, out.duplicates), (0, 1), "{out:?}");
    assert_eq!(files(&f.dir).keys().copied().collect::<Vec<_>>(), [5]);
    assert_eq!(
        files(&f.dir)[&5]["subject"],
        "#1 renamed",
        "updated in place"
    );
    assert_eq!(
        state_of(&f).entries[t.urn.as_deref().unwrap()].harness_id,
        5
    );
    assert!(hwm(&f.dir) >= 2, "the deleted id is never handed out again");

    let again = pass(&f, &[task(1, "in_progress", 2)]).unwrap();
    assert_eq!((again.created, again.duplicates), (0, 0), "{again:?}");
}

#[test]
fn an_owned_record_the_agent_edited_is_kept_and_counted_and_a_lower_copy_dropped() {
    let f = fixture("sync-collapse-diverged");
    let t = task(1, "open", 1);
    std::fs::create_dir_all(&f.dir).unwrap();
    std::fs::write(
        f.dir.join("2.json"),
        record::record(2, &desired_of(&t)).to_string(),
    )
    .unwrap();
    let mut st = State::new(PROJECT, &f.dir);
    st.entries
        .insert(t.urn.clone().unwrap(), entry(5, &desired_of(&t)));
    save_state(&f, &st);
    let mut edited = record::record(5, &desired_of(&t));
    edited["subject"] = json!("#1 the agent's name");
    std::fs::write(f.dir.join("5.json"), edited.to_string()).unwrap();

    let out = pass(&f, std::slice::from_ref(&t)).unwrap();
    assert_eq!((out.diverged, out.duplicates), (1, 1), "{out:?}");
    assert_eq!(files(&f.dir).keys().copied().collect::<Vec<_>>(), [5]);
    assert_eq!(
        files(&f.dir)[&5],
        edited,
        "the edit is kept, not overwritten"
    );
}

#[test]
fn a_legacy_seed_record_is_adopted_and_its_later_copies_dropped() {
    let f = fixture("sync-legacy");
    std::fs::create_dir_all(&f.dir).unwrap();
    let t = task(1, "open", 1);
    std::fs::write(f.dir.join("1.json"), legacy(1, &t)).unwrap();
    std::fs::write(
        f.dir.join("4.json"),
        record::record(4, &desired_of(&t)).to_string(),
    )
    .unwrap();
    // A model's own task that only LOOKS like one: same subject prefix, no URN.
    let lookalike = r##"{"id":"3","subject":"#1 task 1","description":"mine","status":"pending","blocks":[],"blockedBy":[]}"##;
    std::fs::write(f.dir.join("3.json"), lookalike).unwrap();
    // No state owns either record (a fresh fork): the LOWEST is kept.

    let out = pass(&f, std::slice::from_ref(&t)).unwrap();
    assert_eq!((out.created, out.duplicates), (0, 1), "{out:?}");
    let now = files(&f.dir);
    assert_eq!(now.keys().copied().collect::<Vec<_>>(), [1, 3]);
    assert_eq!(
        now[&1],
        record::record(1, &desired_of(&t)),
        "adopted: metadata set"
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.join("3.json")).unwrap(),
        lookalike
    );
    assert_eq!(
        state_of(&f).entries[t.urn.as_deref().unwrap()].harness_id,
        1
    );
    assert!(hwm(&f.dir) >= 4);

    // And the adoption holds: the next pass neither drops nor recreates it.
    let again = pass(&f, &[t]).unwrap();
    assert_eq!(
        (again.created, again.recreated, again.updated),
        (0, 0, 0),
        "{again:?}"
    );
    assert_eq!(files(&f.dir).keys().copied().collect::<Vec<_>>(), [1, 3]);
}

#[test]
fn a_legacy_seed_record_for_a_task_not_in_scope_is_left_alone() {
    let f = fixture("sync-legacy-terminal");
    std::fs::create_dir_all(&f.dir).unwrap();
    let t = task(1, "done", 1);
    std::fs::write(f.dir.join("1.json"), legacy(1, &t)).unwrap();
    let before = snapshot(&f.dir);
    pass(&f, &[t]).unwrap();
    assert_eq!(snapshot(&f.dir), before);
}

/// A pass's `files` map, as the scan saw it, for calling [`apply`] directly.
fn scan(dir: &Path) -> BTreeMap<u64, Value> {
    files(dir)
}

#[test]
fn a_failed_adopt_drops_no_copy_so_the_task_keeps_its_records() {
    // The scan saw 1, 2 and 3 as records of the task; before the writes, the
    // agent replaced 1.json with its own task. The adopt of 1 fails, and the
    // drops of 2 and 3 must not run: they would leave the task no file.
    let f = fixture("sync-adopt-fails");
    std::fs::create_dir_all(&f.dir).unwrap();
    let t = task(1, "open", 1);
    let d = desired_of(&t);
    for id in 1..=3 {
        std::fs::write(
            f.dir.join(format!("{id}.json")),
            record::record(id, &d).to_string(),
        )
        .unwrap();
    }
    let seen = scan(&f.dir);
    let st0 = State::new(PROJECT, &f.dir);
    let actions = plan(std::slice::from_ref(&d), &st0, &seen, true);
    assert!(
        matches!(actions[0], Action::Adopt { harness_id: 1, .. }),
        "{actions:?}"
    );
    std::fs::write(f.dir.join("1.json"), FOREIGN).unwrap();
    let mut st = st0.clone();
    let mut out = Outcome::default();
    let listed = listing(&f.dir);
    apply(
        &f.dir,
        &listed,
        &seen,
        std::slice::from_ref(&d),
        &actions,
        &mut st,
        &mut out,
        &|_| Ok(()),
    )
    .unwrap();
    assert_eq!(out.duplicates, 0, "{out:?}");
    assert_eq!(
        files(&f.dir).keys().copied().collect::<Vec<_>>(),
        [1, 2, 3],
        "both extra copies survive"
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.join("1.json")).unwrap(),
        FOREIGN
    );
    assert!(st.entries.is_empty());
}

#[test]
fn a_pass_that_fails_midway_still_stamps_the_ids_it_already_deleted() {
    let f = fixture("sync-hwm-on-error");
    std::fs::create_dir_all(&f.dir).unwrap();
    let (a, b) = (
        desired_of(&task(1, "open", 1)),
        desired_of(&task(2, "done", 1)),
    );
    std::fs::write(f.dir.join("1.json"), record::record(1, &a).to_string()).unwrap();
    std::fs::write(f.dir.join("7.json"), record::record(7, &a).to_string()).unwrap();
    std::fs::write(f.dir.join("9.json"), record::record(9, &b).to_string()).unwrap();
    let mut st = State::new(PROJECT, &f.dir);
    st.entries.insert(a.urn.clone(), entry(1, &a));
    st.entries.insert(b.urn.clone(), entry(9, &b));
    let actions = [
        Action::Drop {
            urn: a.urn.clone(),
            harness_id: 7,
        },
        Action::Delete {
            urn: b.urn.clone(),
            harness_id: 9,
        },
        // Fails: no such task in the desired set.
        Action::Update {
            urn: "yadgar:task:0000-404".into(),
            harness_id: 1,
            refreshed: false,
        },
    ];
    let seen = scan(&f.dir);
    let listed = listing(&f.dir);
    let mut out = Outcome::default();
    let result = apply(
        &f.dir,
        &listed,
        &seen,
        std::slice::from_ref(&a),
        &actions,
        &mut st,
        &mut out,
        &|_| Ok(()),
    );
    assert!(result.is_err());
    assert_eq!((out.duplicates, out.deleted), (1, 1), "{out:?}");
    assert_eq!(files(&f.dir).keys().copied().collect::<Vec<_>>(), [1]);
    assert!(hwm(&f.dir) >= 9, "hwm {} after a failed pass", hwm(&f.dir));
}

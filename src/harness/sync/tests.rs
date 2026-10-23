use super::*;

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
fn an_orphan_a_create_left_is_adopted_only_on_its_intent() {
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
    assert_ne!(
        st.entries[b.urn.as_deref().unwrap()].harness_id,
        6,
        "never adopted by URN alone"
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
    assert_eq!((out.created, out.recreated), (2, 0), "{out:?}");
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

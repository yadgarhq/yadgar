use super::*;

use serde_json::json;

use crate::testserver::scratch_dir;

#[test]
fn the_state_dir_is_xdg_state_home_else_under_home_never_the_claude_tree() {
    let env = Env {
        state_home: Some("/s".into()),
        home: Some("/h".into()),
        ..Env::default()
    };
    assert_eq!(
        state_dir(&env),
        Some(PathBuf::from("/s/yaadgaar/harness/claude-code"))
    );
    let env = Env {
        home: Some("/h".into()),
        ..Env::default()
    };
    assert_eq!(
        state_dir(&env),
        Some(PathBuf::from(
            "/h/.local/state/yaadgaar/harness/claude-code"
        ))
    );
    assert_eq!(state_dir(&Env::default()), None);
}

#[test]
fn a_state_round_trips_and_an_absent_one_is_none() {
    let dir = scratch_dir("state-roundtrip");
    let p = path(&dir.join("deep"), "s-1");
    assert_eq!(load(&p), Ok(None));
    let mut state = State::new("yadgarhq/docs", Path::new("/x/tasks/s-1"));
    state.entries.insert(
        "yadgar:task:a".into(),
        Entry {
            harness_id: 4,
            number: 7,
            version: 2,
            status: "open".into(),
            title: "t".into(),
            written: json!({"id": "4"}),
        },
    );
    save(&p, &state).unwrap();
    assert_eq!(load(&p), Ok(Some(state)));
}

#[test]
fn a_state_that_cannot_be_read_is_an_error_never_a_fresh_start() {
    for body in [
        "not json",
        r#"{"format": 2, "project": "p", "dir": "/d"}"#,
        "[]",
    ] {
        let dir = scratch_dir("state-bad");
        let p = path(&dir, "s-1");
        std::fs::write(&p, body).unwrap();
        assert!(load(&p).is_err(), "{body}");
    }
}

#[test]
fn yaadgaar_state_dir_moves_the_sync_state_with_the_registry() {
    // ONE state root for the whole client: the variable that moves the
    // registry record moves the sync state too.
    let env = Env {
        yaadgaar_state_dir: Some("/y".into()),
        state_home: Some("/s".into()),
        home: Some("/h".into()),
        ..Env::default()
    };
    assert_eq!(
        state_dir(&env),
        Some(PathBuf::from("/y/harness/claude-code"))
    );
    assert_eq!(
        crate::registry::dir(&env).unwrap().parent(),
        state_dir(&env).unwrap().parent().unwrap().parent()
    );
}

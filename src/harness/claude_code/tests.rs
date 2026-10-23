use super::*;

use serde_json::json;

use crate::testserver::scratch_dir;

fn shape() -> RecordShape {
    shape::recorded().unwrap()
}

/// An environment whose home is *home* and which has the task tools on.
fn env(home: &Path) -> Env {
    Env {
        home: Some(home.to_path_buf()),
        enable_todo_tools: Some("1".into()),
        ..Env::default()
    }
}

fn payload(home: &Path, session: &str) -> Value {
    json!({
        "hook_event_name": "SessionStart",
        "session_id": session,
        "transcript_path": home.join(".claude/projects/p").join(format!("{session}.jsonl")),
        "cwd": "/somewhere",
        "model": "claude-opus-5-5",
        "source": "startup"
    })
}

/// A home with an existing `.claude` directory.
fn home(label: &str) -> PathBuf {
    let home = scratch_dir(label);
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    home
}

fn session(home: &Path) -> ClaudeCode {
    detect(&payload(home, "s-1"), &env(home)).expect("a Claude Code session")
}

/// Every path under *dir*, so a test can say "nothing was written".
fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out.sort();
    out
}

// ---- detect ---------------------------------------------------------------

#[test]
fn a_transcript_inside_the_claude_config_dir_is_claude_code() {
    let home = home("cc-detect");
    let found = session(&home);
    assert_eq!(found.base, home.join(".claude"));
    assert_eq!(found.list_id, "s-1");
    assert_eq!(found.model.as_deref(), Some("claude-opus-5-5"));
}

#[test]
fn claude_config_dir_moves_the_base_and_the_transcript_must_be_inside_it() {
    let home = home("cc-config-dir");
    let other = home.join("elsewhere");
    let env = Env {
        claude_config_dir: Some(other.clone()),
        ..env(&home)
    };
    // The transcript is under ~/.claude, but the config dir is elsewhere.
    assert_eq!(detect(&payload(&home, "s-1"), &env), None);
    let moved = json!({"session_id": "s-1", "transcript_path": other.join("projects/x.jsonl")});
    assert_eq!(detect(&moved, &env).map(|c| c.base), Some(other));
}

#[test]
fn a_payload_from_another_harness_is_not_claude_code() {
    let home = home("cc-other");
    let env = env(&home);
    let codex = json!({"session_id": "s", "transcript_path": home.join(".codex/sessions/x.jsonl")});
    assert_eq!(detect(&codex, &env), None);
    assert_eq!(detect(&json!({"cwd": "/x"}), &env), None);
    // A path that merely STARTS with the same characters is not inside it.
    let lookalike =
        json!({"session_id": "s", "transcript_path": home.join(".claude-other/x.jsonl")});
    assert_eq!(detect(&lookalike, &env), None);
    let no_session = json!({"transcript_path": home.join(".claude/projects/x.jsonl")});
    assert_eq!(detect(&no_session, &env), None);
}

#[test]
fn a_named_shared_list_wins_over_the_session_id() {
    let home = home("cc-named");
    let env = Env {
        task_list_id: Some("team-list".into()),
        ..env(&home)
    };
    assert_eq!(
        detect(&payload(&home, "s-1"), &env).unwrap().list_id,
        "team-list"
    );
}

// ---- the list id ----------------------------------------------------------

#[test]
fn a_list_id_that_could_climb_or_hide_is_refused() {
    for good in [
        "s-1",
        "0b6e4c1e-81f6-4a39-9e43-0d0c6c1f0d2a",
        "a.b_c-d",
        "x",
    ] {
        assert!(safe_list_id(good), "{good}");
    }
    let long = "a".repeat(129);
    for bad in [
        "",
        "..",
        ".hidden",
        "a/b",
        "a\\b",
        "-x",
        "_x",
        "a b",
        long.as_str(),
    ] {
        assert!(!safe_list_id(bad), "{bad:?}");
    }
    assert!(safe_list_id(&"a".repeat(128)));
}

#[test]
fn a_bad_list_id_is_refused_by_the_probe_and_nothing_is_written() {
    let home = home("cc-bad-id");
    let env = Env {
        task_list_id: Some("../escape".into()),
        ..env(&home)
    };
    let cc = detect(&payload(&home, "s-1"), &env).unwrap();
    let before = tree(&home);
    assert!(matches!(cc.probe(&env, &shape()), Probe::BadListId(_)));
    assert_eq!(tree(&home), before);
}

// ---- the tools ------------------------------------------------------------

#[test]
fn the_opt_in_variable_turns_the_tools_on_for_any_model() {
    let env = Env {
        enable_todo_tools: Some("1".into()),
        ..Env::default()
    };
    for model in [Some("claude-opus-5-5"), Some("haiku"), None] {
        assert!(tools_present(&env, model), "{model:?}");
    }
}

#[test]
fn without_the_variable_only_the_models_that_have_them_by_default_do() {
    let env = Env::default();
    for model in [
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-sonnet-4-6",
        "claude-sonnet-4-20250514",
        "claude-haiku-4-5-20251001",
        "claude-3-5-haiku-20241022",
        "claude-3-7-sonnet-20250219",
        "claude-opus-4-7[1m]",
    ] {
        assert!(tools_present(&env, Some(model)), "{model}");
    }
    for model in [
        "claude-opus-4-8",
        "claude-opus-5-5",
        "claude-sonnet-4-7",
        "claude-haiku-4-6",
        "claude-sonnet-5",
        "opus",
        "something-else",
        "",
    ] {
        assert!(!tools_present(&env, Some(model)), "{model}");
    }
    // No model at all (after /clear, say) and no variable: no tools.
    assert!(!tools_present(&env, None));
}

#[test]
fn the_legacy_todo_switch_means_no_task_files() {
    let env = Env {
        enable_todo_tools: Some("1".into()),
        enable_tasks: Some("0".into()),
        ..Env::default()
    };
    assert!(!tools_present(&env, Some("claude-opus-4-7")));
}

#[test]
fn only_the_documented_1_opts_in() {
    // Claude Code documents `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`. Any other value
    // may not turn the tools on, and a list written for a session with no
    // tools is files nobody reads: fail closed.
    for value in ["0", "", "yes", "false", "true", "TRUE", " 1", "1 "] {
        let env = Env {
            enable_todo_tools: Some(value.into()),
            ..Env::default()
        };
        assert!(!tools_present(&env, Some("claude-opus-5-5")), "{value:?}");
    }
    let env = Env {
        enable_todo_tools: Some("1".into()),
        ..Env::default()
    };
    assert!(tools_present(&env, Some("claude-opus-5-5")));
}

// ---- the probe ------------------------------------------------------------

#[test]
fn no_tools_is_its_own_outcome_and_nothing_is_read_or_written() {
    let home = home("cc-no-tools");
    let env = Env {
        enable_todo_tools: None,
        ..env(&home)
    };
    let cc = session(&home);
    let before = tree(&home);
    assert_eq!(cc.probe(&env, &shape()), Probe::NoTools);
    assert_eq!(tree(&home), before);
}

#[test]
fn a_missing_config_dir_is_no_store() {
    let home = scratch_dir("cc-no-store");
    let env = env(&home);
    let cc = ClaudeCode {
        base: home.join(".claude"),
        list_id: "s-1".into(),
        model: None,
    };
    assert!(matches!(cc.probe(&env, &shape()), Probe::NoStore(_)));
    assert!(
        !home.join(".claude").exists(),
        "the probe created the store"
    );
}

#[test]
fn an_absent_list_dir_is_ready_and_is_not_created() {
    let home = home("cc-absent-list");
    let env = env(&home);
    let cc = session(&home);
    let dir = home.join(".claude/tasks/s-1");
    assert_eq!(
        cc.probe(&env, &shape()),
        Probe::Ready {
            dir: dir.clone(),
            listing: Listing::default()
        }
    );
    assert!(!dir.exists(), "the probe created the list dir");
}

#[test]
fn a_list_in_the_recorded_shape_is_ready_with_its_listing() {
    let home = home("cc-ready");
    let env = env(&home);
    let dir = home.join(".claude/tasks/s-1");
    std::fs::create_dir_all(&dir).unwrap();
    let record = json!({"id":"4","subject":"s","description":"d","status":"in_progress","blocks":[],"blockedBy":[]});
    std::fs::write(dir.join("4.json"), record.to_string()).unwrap();
    std::fs::write(dir.join(".lock"), "").unwrap();
    let before = tree(&home);
    match session(&home).probe(&env, &shape()) {
        Probe::Ready { dir: d, listing } => {
            assert_eq!(d, dir);
            assert_eq!(listing.ids, [4]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(tree(&home), before, "the probe wrote");
}

#[cfg(unix)]
#[test]
fn a_list_dir_that_is_a_symlink_is_a_mismatch_and_is_not_followed() {
    let home = home("cc-symlinked-list");
    let elsewhere = scratch_dir("cc-symlink-target");
    std::fs::create_dir_all(home.join(".claude/tasks")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.join(".claude/tasks/s-1")).unwrap();
    assert!(matches!(
        session(&home).probe(&env(&home), &shape()),
        Probe::Mismatch(_)
    ));
    std::fs::remove_dir_all(&elsewhere).ok();
}

#[test]
fn each_shape_violation_is_a_mismatch_and_nothing_is_written() {
    let good = json!({"id":"1","subject":"s","description":"d","status":"pending","blocks":[],"blockedBy":[]});
    let mut extra = good.clone();
    extra["priority"] = json!(1);
    let mut status = good.clone();
    status["status"] = json!("deleted");
    let cases: Vec<(&str, &str, String)> = vec![
        ("extra key", "1.json", extra.to_string()),
        ("non-digit stem", "one.json", good.to_string()),
        ("unknown status", "1.json", status.to_string()),
        ("non-empty .lock", ".lock", "pid".into()),
        ("bad .highwatermark", ".highwatermark", "x".into()),
    ];
    for (what, name, body) in cases {
        let home = home("cc-mismatch");
        let env = env(&home);
        let dir = home.join(".claude/tasks/s-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
        let before = tree(&home);
        assert!(
            matches!(session(&home).probe(&env, &shape()), Probe::Mismatch(_)),
            "{what}"
        );
        assert_eq!(tree(&home), before, "{what}: the probe wrote");
    }
}

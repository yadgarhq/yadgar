use super::*;

use std::path::PathBuf;

use serde_json::json;

use crate::testserver::scratch_dir;

fn shape() -> RecordShape {
    recorded().expect("the committed fixture parses")
}

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

fn task(id: &str) -> Value {
    json!({
        "id": id,
        "subject": "a task",
        "description": "what it is",
        "status": "pending",
        "blocks": [],
        "blockedBy": []
    })
}

fn put(dir: &Path, stem: &str, record: &Value) {
    write(dir, &format!("{stem}.json"), &record.to_string());
}

fn mismatch(dir: &Path) -> Mismatch {
    check_dir(dir, &shape()).expect_err("a directory off the recorded shape passed")
}

/// One task file, then the check; returns the reason it failed.
fn one(stem: &str, record: &Value) -> String {
    let dir = scratch_dir("shape-one");
    put(&dir, stem, record);
    let reason = mismatch(&dir).to_string();
    std::fs::remove_dir_all(&dir).ok();
    reason
}

#[test]
fn the_committed_fixture_parses_and_names_the_documented_record() {
    let shape = shape();
    // The documented logical record (F6) is the required set; nothing else is.
    for key in [
        "id",
        "subject",
        "description",
        "status",
        "blocks",
        "blockedBy",
    ] {
        assert!(shape.required.iter().any(|k| k == key), "{key}");
        assert!(shape.types.contains_key(key), "{key} has no type");
    }
    // `types` is the allowed set the check reads, so it must be exactly the
    // two lists and nothing more.
    let mut named: Vec<&String> = shape.required.iter().chain(&shape.optional).collect();
    named.sort();
    assert_eq!(named, shape.types.keys().collect::<Vec<_>>());
    assert_eq!(shape.statuses, ["pending", "in_progress", "completed"]);
}

#[test]
fn a_fixture_of_another_format_is_refused_rather_than_half_read() {
    let other = FIXTURE.replacen("\"format\": 1", "\"format\": 2", 1);
    assert_ne!(other, FIXTURE, "the fixture no longer spells its format");
    let err = parse(&other).expect_err("format 2 was read as format 1");
    assert!(err.contains("format 2"), "{err}");
}

#[test]
fn the_directory_a_real_claude_code_run_wrote_passes() {
    // Copied byte for byte from the exploratory canary run on Claude Code
    // 2.1.296 (plan F26), except the newline the repo's end-of-file hook adds.
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-code/list-2.1.296");
    let listing = check_dir(&dir, &shape()).expect("the real run's directory");
    assert_eq!(listing.ids, [1]);
    assert_eq!(listing.highwatermark, Some(2));
    assert!(listing.lock);
}

#[test]
fn a_directory_in_the_recorded_shape_passes_and_is_listed() {
    let dir = scratch_dir("shape-good");
    let mut full = task("3");
    full["activeForm"] = json!("doing it");
    full["owner"] = json!("someone");
    full["metadata"] = json!({"yaadgaar": {"urn": "yadgar:task:x", "number": 7}});
    full["blocks"] = json!(["1"]);
    put(&dir, "3", &full);
    put(&dir, "1", &task("1"));
    write(&dir, LOCK, "");
    write(&dir, HIGHWATERMARK, "4");
    let listing = check_dir(&dir, &shape()).expect("the recorded shape");
    assert_eq!(
        listing,
        Listing {
            ids: vec![1, 3],
            highwatermark: Some(4),
            lock: true
        }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_directory_passes() {
    let dir = scratch_dir("shape-empty");
    assert_eq!(check_dir(&dir, &shape()), Ok(Listing::default()));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_key_the_shape_does_not_record_is_a_mismatch_that_names_it() {
    let mut record = task("1");
    record["priority"] = json!("high");
    let reason = one("1", &record);
    assert!(
        reason.contains("1.json") && reason.contains("priority"),
        "{reason}"
    );
}

#[test]
fn a_missing_required_key_is_a_mismatch_that_names_it() {
    let mut record = task("1");
    record.as_object_mut().unwrap().remove("blockedBy");
    let reason = one("1", &record);
    assert!(reason.contains("blockedBy"), "{reason}");
}

#[test]
fn a_stem_that_is_not_a_decimal_id_is_a_mismatch() {
    for stem in ["abc", "1a", "01", "-1", ""] {
        let reason = one(stem, &task(stem));
        assert!(reason.contains(&format!("{stem}.json")), "{stem}: {reason}");
    }
}

#[test]
fn an_id_that_is_not_the_stem_is_a_mismatch() {
    let reason = one("2", &task("1"));
    assert!(
        reason.contains("2.json") && reason.contains("id"),
        "{reason}"
    );
}

#[test]
fn a_status_the_shape_does_not_record_is_a_mismatch() {
    let mut record = task("1");
    record["status"] = json!("deleted");
    let reason = one("1", &record);
    assert!(reason.contains("deleted"), "{reason}");
}

#[test]
fn a_value_of_the_wrong_type_is_a_mismatch() {
    let cases = [
        ("subject", json!(7)),
        ("id", json!(1)),
        ("blocks", json!("1")),
        ("blockedBy", json!([1])),
        ("metadata", json!("x")),
        ("activeForm", json!(null)),
    ];
    for (key, value) in cases {
        let mut record = task("1");
        record[key] = value;
        let reason = one("1", &record);
        assert!(reason.contains(key), "{key}: {reason}");
    }
}

#[test]
fn a_task_file_that_is_not_a_json_object_is_a_mismatch() {
    for body in ["not json", "[]", "\"1\""] {
        let dir = scratch_dir("shape-body");
        write(&dir, "1.json", body);
        assert_eq!(mismatch(&dir).entry, "1.json", "{body}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn a_lock_that_is_not_an_empty_file_is_a_mismatch() {
    let dir = scratch_dir("shape-lock");
    write(&dir, LOCK, "pid 42");
    assert_eq!(mismatch(&dir).entry, LOCK);
    std::fs::remove_file(dir.join(LOCK)).unwrap();
    std::fs::create_dir(dir.join(LOCK)).unwrap();
    assert_eq!(mismatch(&dir).entry, LOCK);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_highwatermark_that_is_not_an_integer_is_a_mismatch() {
    for body in ["x", "", "-1", "1.5", "1 2"] {
        let dir = scratch_dir("shape-hwm");
        write(&dir, HIGHWATERMARK, body);
        assert_eq!(mismatch(&dir).entry, HIGHWATERMARK, "{body:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn an_entry_the_shape_does_not_name_is_a_mismatch() {
    let dir = scratch_dir("shape-extra");
    write(&dir, "notes.txt", "hello");
    assert_eq!(mismatch(&dir).entry, "notes.txt");
    std::fs::remove_file(dir.join("notes.txt")).unwrap();
    std::fs::create_dir(dir.join("7.json")).unwrap();
    assert_eq!(mismatch(&dir).entry, "7.json");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_symlink_is_a_mismatch_even_when_it_points_at_a_good_record() {
    // Following it would judge, and later write, a file outside the list.
    let outside = scratch_dir("shape-link-target");
    put(&outside, "1", &task("1"));
    let dir = scratch_dir("shape-link");
    std::os::unix::fs::symlink(outside.join("1.json"), dir.join("1.json")).unwrap();
    let err = mismatch(&dir);
    assert_eq!(err.entry, "1.json");
    assert!(err.reason.contains("regular file"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[test]
fn a_directory_that_cannot_be_read_is_a_mismatch_not_a_pass() {
    let dir = scratch_dir("shape-missing").join("absent");
    let err = mismatch(&dir);
    assert!(!err.reason.is_empty(), "{err}");
}

/// The payloads `make canary-record` captured from a real run.
const PAYLOADS: &str = include_str!("../../../tests/fixtures/claude-code/hook-payloads.jsonl");

fn hooks_fixture() -> Value {
    serde_json::from_str::<Value>(FIXTURE).unwrap()["hooks"].clone()
}

fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// Every key of *object* is one the fixture section names, and every required
/// one is there.
fn keys_match(section: &Value, object: &Value, what: &str) {
    let required = names(&section["required"]);
    let optional = names(&section["optional"]);
    let keys: Vec<&String> = object.as_object().unwrap().keys().collect();
    for key in &required {
        assert!(keys.contains(&key), "{what}: no `{key}` in {object}");
    }
    for key in keys {
        assert!(
            required.contains(key) || optional.contains(key),
            "{what}: `{key}` is not in the fixture"
        );
    }
}

#[test]
fn the_recorded_payloads_agree_with_the_recorded_hook_shape() {
    // Two fixtures, one run: a hand edit to either that the other does not
    // follow is caught here, in CI, without a real `claude`.
    let hooks = hooks_fixture();
    let events: Vec<Value> = PAYLOADS
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let event = |e: &Value| {
        e["hook_event_name"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let update = |e: &Value| e["tool_name"] == "TaskUpdate";
    let statuses = names(&hooks["task_update_statuses"]);
    let mut seen = 0;
    for e in &events {
        match event(e).as_str() {
            "PreToolUse" if update(e) => {
                keys_match(&hooks["task_update_input"], &e["tool_input"], "tool_input");
                if let Some(s) = e["tool_input"]["status"].as_str() {
                    assert!(statuses.iter().any(|x| x == s), "{s}");
                }
                seen += 1;
            }
            "PostToolUse" if update(e) => {
                keys_match(
                    &hooks["task_update_response"],
                    &e["tool_response"],
                    "tool_response",
                );
            }
            "TaskCompleted" => keys_match(&hooks["task_completed"], e, "TaskCompleted"),
            _ => {}
        }
    }
    assert!(seen >= 4, "the recording holds {seen} TaskUpdate calls");
    // The order the write-back rests on (plan F26).
    let completing: Vec<&Value> = events
        .iter()
        .filter(|e| {
            (update(e) && e["tool_input"]["status"] == "completed") || event(e) == "TaskCompleted"
        })
        .collect();
    let order: Vec<String> = completing.iter().map(|e| event(e)).collect();
    assert_eq!(order, names(&hooks["order_completing_task_update"]));
    assert_eq!(completing[0]["prompt_id"], completing[1]["prompt_id"]);
}

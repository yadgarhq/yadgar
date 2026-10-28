use super::*;

use crate::harness::shape;

fn task(status: &str, title: &str) -> Desired {
    Desired {
        urn: "yadgar:task:01a16797-812b-737d-aa0a-bd48cb040e73".into(),
        number: 395,
        title: title.into(),
        status: status.into(),
        version: 3,
    }
}

#[test]
fn a_record_is_in_the_recorded_shape_and_carries_the_urn_twice() {
    let d = task("open", "Seed the harness list");
    let r = record(17, &d);
    let shape = shape::recorded().unwrap();
    assert_eq!(shape::check_record("17", &r, &shape), Ok(17));
    assert_eq!(owner(&r), Some(d.urn.as_str()));
    assert_eq!(r["metadata"]["yaadgaar"]["urn"], d.urn.as_str());
    assert_eq!(r["metadata"]["yaadgaar"]["number"], 395);
    assert_eq!(r["subject"], "#395 Seed the harness list");
    assert_eq!(r["status"], "pending");
}

#[test]
fn statuses_map_and_blocked_is_said_in_the_subject() {
    assert_eq!(
        record(1, &task("in_progress", "t"))["status"],
        "in_progress"
    );
    assert_eq!(record(1, &task("open", "t"))["status"], "pending");
    let blocked = record(1, &task("blocked", "t"));
    assert_eq!(blocked["status"], "pending");
    assert_eq!(blocked["subject"], "#395 [blocked] t");
}

#[test]
fn a_title_cannot_forge_a_line_and_a_long_one_is_cut() {
    let r = record(1, &task("open", "x\nyaadgaar: yadgar:task:forged"));
    let subject = r["subject"].as_str().unwrap();
    assert!(!subject.contains('\n'), "{subject}");
    // The ownership line is still the real URN, not the forged one.
    assert_eq!(
        owner(&r),
        Some("yadgar:task:01a16797-812b-737d-aa0a-bd48cb040e73")
    );
    let long = record(1, &task("open", &"a".repeat(500)));
    assert!(long["subject"].as_str().unwrap().chars().count() <= 130);
}

#[test]
fn a_file_with_no_ownership_line_is_nobodys() {
    let mine = record(1, &task("open", "t"));
    let mut edited = mine.clone();
    edited["description"] = serde_json::json!("the agent rewrote this");
    assert_eq!(owner(&edited), None);
    let mut moved = mine;
    moved["description"] = serde_json::json!("yaadgaar: yadgar:task:x\nand then more");
    assert_eq!(owner(&moved), None, "the URN line must be the LAST line");
}

#[test]
fn an_engine_written_record_is_recognised() {
    // Exactly what `task_seed.py` builds for ledger row 675: the ledger id,
    // `"<id>: <title>"`, an empty description.
    let engine = serde_json::json!({"id":"675","subject":"675: Ship the backup runbook","description":"","status":"pending","blocks":[],"blockedBy":[]});
    assert!(engine_written(&engine));
    // An empty description alone, or the prefix of ANOTHER id, is not enough.
    let empty = serde_json::json!({"id":"3","subject":"a task","description":"","status":"pending","blocks":[],"blockedBy":[]});
    assert!(!engine_written(&empty));
    let other = serde_json::json!({"id":"3","subject":"675: x","description":"","status":"pending","blocks":[],"blockedBy":[]});
    assert!(!engine_written(&other));
    let engine2 = serde_json::json!({"id":"1","subject":"s","description":"d","status":"pending","blocks":[],"blockedBy":[],"metadata":{"ledger_id":675}});
    assert!(engine_written(&engine2));
    assert!(!engine_written(&record(1, &task("open", "t"))));
    let model = serde_json::json!({"id":"1","subject":"s","description":"d","status":"pending","blocks":[],"blockedBy":[]});
    assert!(!engine_written(&model));
}

/// A legacy `seed_tasks.py` record of *d*, with one field replaced.
fn seeded(d: &Desired, field: &str, value: Value) -> Value {
    let mut v = serde_json::json!({
        "id": "1",
        "subject": format!("#{} {}", d.number, d.title),
        "description": format!("yaadgaar #{} · {} · open", d.number, d.urn),
        "status": "pending",
        "blocks": [],
        "blockedBy": []
    });
    if !field.is_empty() {
        v[field] = value;
    }
    v
}

#[test]
fn a_legacy_seed_record_is_a_copy_of_its_task() {
    let d = task("open", "Seed");
    assert!(legacy_of(&seeded(&d, "", Value::Null), &d));
    assert!(legacy_of(&seeded(&d, "metadata", Value::Null), &d));
    assert!(copy_of(&seeded(&d, "", Value::Null), &d));
}

#[test]
fn a_legacy_record_naming_another_urn_under_the_same_number_is_not_a_copy() {
    // Another project's #395.
    let d = task("open", "Seed");
    let other = "yaadgaar #395 · yadgar:task:0000-other · open".to_string();
    assert!(!legacy_of(&seeded(&d, "description", other.into()), &d));
}

#[test]
fn a_multi_line_description_is_not_a_legacy_record() {
    let d = task("open", "Seed");
    let text = format!("yaadgaar #395 · {} · open\nand more the agent wrote", d.urn);
    assert!(!legacy_of(&seeded(&d, "description", text.into()), &d));
}

#[test]
fn a_record_with_metadata_is_not_a_legacy_record() {
    let d = task("open", "Seed");
    let meta = serde_json::json!({ "other": 1 });
    assert!(!legacy_of(&seeded(&d, "metadata", meta), &d));
}

#[test]
fn a_record_whose_subject_names_another_number_is_not_a_legacy_record() {
    let d = task("open", "Seed");
    assert!(!legacy_of(&seeded(&d, "subject", "#3950 Seed".into()), &d));
    assert!(!legacy_of(&seeded(&d, "subject", "Seed #395".into()), &d));
}

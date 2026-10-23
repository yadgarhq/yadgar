use super::*;

use serde_json::json;

use crate::harness::record::{self, Desired};

fn desired(status: &str) -> Desired {
    Desired {
        urn: "yadgar:task:a".into(),
        number: 7,
        title: "Cut over".into(),
        status: status.into(),
        version: 3,
    }
}

fn entry(status: &str) -> Entry {
    let d = desired(status);
    Entry {
        harness_id: 4,
        number: d.number,
        version: d.version,
        status: d.status.clone(),
        title: d.title.clone(),
        written: record::record(4, &d),
    }
}

fn apply(status: &str, input: Value) -> Change {
    let e = entry(status);
    match decide(&e, &e.written, &input) {
        Verdict::Apply(c) => c,
        Verdict::Refuse(r) => panic!("refused: {r}"),
    }
}

fn refuse(input: Value) -> String {
    let e = entry("open");
    match decide(&e, &e.written, &input) {
        Verdict::Refuse(r) => r,
        Verdict::Apply(c) => panic!("allowed: {c:?}"),
    }
}

#[test]
fn a_subject_edit_sends_the_title_with_the_mirror_prefix_stripped() {
    let c = apply(
        "open",
        json!({"taskId": "4", "subject": "#7 Cut over at noon"}),
    );
    assert_eq!(c.title.as_deref(), Some("Cut over at noon"));
    assert_eq!(c.to, None);
    assert_eq!(c.expected["subject"], "#7 Cut over at noon");
    let c = apply(
        "blocked",
        json!({"taskId": "4", "subject": "#7 [blocked] New"}),
    );
    assert_eq!(c.title.as_deref(), Some("New"));
    let c = apply("open", json!({"taskId": "4", "subject": "  bare title "}));
    assert_eq!(c.title.as_deref(), Some("bare title"));
}

#[test]
fn only_this_tasks_own_number_is_stripped() {
    assert_eq!(strip("#5 keep me", 7), "#5 keep me");
    assert_eq!(strip("#7 #5 keep me", 7), "#5 keep me");
}

#[test]
fn the_subject_as_written_sends_nothing_even_when_it_is_cut() {
    let mut d = desired("open");
    d.title = "x".repeat(200);
    let e = Entry {
        title: d.title.clone(),
        written: record::record(4, &d),
        ..entry("open")
    };
    let subject = e.written["subject"].as_str().unwrap().to_string();
    let Verdict::Apply(c) = decide(&e, &e.written, &json!({"taskId": "4", "subject": subject}))
    else {
        panic!("refused the subject it wrote itself");
    };
    assert_eq!(c.title, None);
    // And a stripped title equal to the one last seen sends nothing either.
    let c = apply("open", json!({"taskId": "4", "subject": "Cut over"}));
    assert_eq!(c.title, None);
}

#[test]
fn an_edit_of_a_subject_that_shows_the_title_only_in_part_is_refused() {
    // A 125-character title is cut at 120 in the subject; a title with a line
    // break shows it escaped. Editing either subject would save the shown
    // text, not the title, and both fit the 128 cap.
    for title in ["y".repeat(125), "two\nlines".to_string()] {
        let mut d = desired("open");
        d.title.clone_from(&title);
        let e = Entry {
            title,
            written: record::record(4, &d),
            ..entry("open")
        };
        let shown = e.written["subject"].as_str().unwrap();
        let edited = format!("{shown} more");
        let Verdict::Refuse(reason) =
            decide(&e, &e.written, &json!({"taskId": "4", "subject": edited}))
        else {
            panic!("allowed an edit of a lossy subject");
        };
        assert!(reason.contains("only in part"), "{reason}");
        assert!(reason.contains("edit_task"), "{reason}");
    }
    assert!(!record::lossy("Cut over"));
    assert!(record::lossy(&"z".repeat(121)));
    assert!(!record::lossy(&"z".repeat(120)));
}

#[test]
fn an_empty_title_and_one_over_128_characters_are_refused_locally() {
    assert!(refuse(json!({"taskId": "4", "subject": "#7 "})).contains("empty title"));
    let long = format!("#7 {}", "é".repeat(129));
    let reason = refuse(json!({"taskId": "4", "subject": long}));
    assert_eq!(reason, too_long(7, 129));
    // 128 CHARACTERS fit, whatever their byte length.
    let c = apply("open", json!({"taskId": "4", "subject": "é".repeat(128)}));
    assert_eq!(c.title.map(|t| t.chars().count()), Some(128));
}

#[test]
fn a_status_change_follows_the_asymmetric_table() {
    for (harness, now, sent) in [
        ("in_progress", "open", Some("in_progress")),
        ("in_progress", "blocked", Some("in_progress")),
        ("in_progress", "in_progress", None),
        ("pending", "in_progress", Some("open")),
        ("pending", "open", None),
        ("pending", "blocked", None),
    ] {
        let c = apply(now, json!({"taskId": "4", "status": harness}));
        assert_eq!(c.to, sent, "{harness} on {now}");
        assert_eq!(c.expected["status"], harness, "{harness} on {now}");
        assert!(!c.complete);
    }
}

#[test]
fn a_completion_sends_nothing_and_leaves_the_expected_status_alone() {
    let c = apply("in_progress", json!({"taskId": "4", "status": "completed"}));
    assert!(c.complete);
    assert_eq!(c.to, None);
    assert_eq!(c.title, None);
    assert_eq!(c.expected["status"], "in_progress");
}

#[test]
fn a_delete_a_description_metadata_or_an_edge_is_refused() {
    assert!(refuse(json!({"taskId": "4", "status": "deleted"})).contains("cannot be deleted"));
    assert!(refuse(json!({"taskId": "4", "description": "mine now"})).contains("edit_task"));
    assert!(refuse(json!({"taskId": "4", "metadata": {"k": 1}})).contains("metadata"));
    assert!(refuse(json!({"taskId": "4", "addBlocks": ["9"]})).contains("dependency"));
    assert!(refuse(json!({"taskId": "4", "addBlockedBy": ["9"]})).contains("dependency"));
}

#[test]
fn an_unchanged_description_and_empty_extras_are_allowed() {
    let e = entry("open");
    let input = json!({
        "taskId": "4",
        "description": e.written["description"],
        "metadata": {},
        "addBlocks": [],
        "activeForm": "Cutting over",
        "owner": "me"
    });
    let Verdict::Apply(c) = decide(&e, &e.written, &input) else {
        panic!("refused a call that changes nothing that is carried");
    };
    assert_eq!(c.expected["activeForm"], "Cutting over");
    assert_eq!(c.expected["owner"], "me");
    assert_eq!((c.title, c.to, c.complete), (None, None, false));
}

#[test]
fn a_refusal_never_says_a_file_is_missing() {
    // Claude Code reads exit 2 from TaskCompleted with "No such file or
    // directory" on stderr as a NON-blocking error (hooks reference).
    for input in [
        json!({"taskId": "4", "status": "deleted"}),
        json!({"taskId": "4", "description": "x"}),
        json!({"taskId": "4", "addBlocks": ["1"]}),
        json!({"taskId": "4", "subject": "#7 "}),
    ] {
        assert!(!refuse(input).contains("No such file"));
    }
}

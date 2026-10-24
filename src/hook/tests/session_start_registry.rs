//! `session-start` writes the registry record (yaadgaar #403): registered on
//! a listing, not registered on the gateway's explicit "no such project", and
//! NOTHING on a fault. Scratch state directory, test server, never the real
//! state or the live gateway.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::*;
use crate::harness::Env;

/// A workspace whose `.yadgar/project-id` names *project*.
fn workspace(label: &str, project: &str) -> PathBuf {
    let dir = crate::testserver::scratch_dir(label);
    std::fs::create_dir_all(dir.join(".yadgar")).unwrap();
    std::fs::write(dir.join(".yadgar/project-id"), format!("{project}\n")).unwrap();
    dir
}

fn env(dir: &Path) -> Env {
    Env {
        home: Some(dir.to_path_buf()),
        yaadgaar_state_dir: Some(dir.join("ystate")),
        ..Env::default()
    }
}

fn registry(dir: &Path) -> PathBuf {
    dir.join("ystate/registry")
}

const LISTED: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[],"structuredContent":{"tasks":[],"next_page_token":""}}}"#;

const UNREGISTERED: &str = r#"{"error":{"code":-32600,"data":{"reason":"PROJECT_UNREGISTERED_ORG"},"message":"not registered"},"id":1,"jsonrpc":"2.0"}"#;

const NOT_MEMBER: &str = r#"{"error":{"code":-32600,"data":{"reason":"PROJECT_NOT_MEMBER"},"message":"not a member"},"id":1,"jsonrpc":"2.0"}"#;

/// Session start in *dir* against a gateway answering *status* and *body*.
fn start(dir: &Path, status: &'static str, body: &'static str) -> Option<String> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (addr, served) = rt.block_on(crate::testserver::answer_once(status, body));
    let config = Config::new(dir, format!("http://{addr}/"), "tok".into());
    let payload = json!({"hook_event_name": "SessionStart", "cwd": dir.display().to_string()});
    let text = context_text_in(Ok(payload), Ok(config), None, DEADLINE, &env(dir));
    rt.block_on(served).unwrap();
    text
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_listing_records_the_project_as_registered() {
    let dir = workspace("registry-listed", "sentinel-org/no-such-repo");
    assert!(start(&dir, "200 OK", LISTED).is_some());
    let body = read(&registry(&dir).join("sentinel-org%2Fno-such-repo.json"));
    assert_eq!(body["registered"], true);
    assert_eq!(body["project"], "sentinel-org/no-such-repo");
}

#[test]
fn an_explicit_no_such_project_records_false_and_still_prints_nothing() {
    let dir = workspace("registry-unregistered", "sentinel-org/no-such-repo");
    assert_eq!(start(&dir, "400 Bad Request", UNREGISTERED), None);
    let body = read(&registry(&dir).join("sentinel-org%2Fno-such-repo.json"));
    assert_eq!(body["registered"], false);
}

#[test]
fn a_fault_keeps_the_prior_record_and_writes_nothing() {
    let dir = workspace("registry-fault", "sentinel-org/no-such-repo");
    assert!(start(&dir, "200 OK", LISTED).is_some());
    let path = registry(&dir).join("sentinel-org%2Fno-such-repo.json");
    let prior = std::fs::read(&path).unwrap();
    for (status, body) in [
        ("503 Service Unavailable", "{}"),
        ("401 Unauthorized", "{}"),
        // A refusal of a project that may well be registered is not an
        // answer about the registry.
        ("400 Bad Request", NOT_MEMBER),
        ("200 OK", "not json"),
    ] {
        assert!(start(&dir, status, body).is_some(), "{status}");
        assert_eq!(std::fs::read(&path).unwrap(), prior, "{status} rewrote it");
    }
    let names = std::fs::read_dir(registry(&dir)).unwrap().count();
    assert_eq!(names, 1, "a fault left a second file");
}

#[test]
fn a_fault_with_no_prior_record_writes_none() {
    let dir = workspace("registry-fault-fresh", "sentinel-org/no-such-repo");
    assert!(start(&dir, "503 Service Unavailable", "{}").is_some());
    assert!(!registry(&dir).exists());
}

#[test]
fn a_record_that_cannot_be_written_still_starts_the_session_with_the_banner() {
    let dir = workspace("registry-unwritable", "sentinel-org/no-such-repo");
    // The state directory is a regular file, so no record can go under it.
    std::fs::write(dir.join("ystate"), "not a directory").unwrap();
    let text = start(&dir, "200 OK", LISTED).expect("the banner");
    assert!(text.contains("live in yaadgaar"), "{text}");
}

#[test]
fn the_filename_is_keyed_on_the_project_id_file_casefolded() {
    // Key parity with the engine's mint: `.yadgar/project-id` wins outright,
    // its case kept in the body, and casefolded only in the filename.
    let dir = workspace("registry-mixed-case", "Sentinel-Org/Straße");
    assert!(start(&dir, "200 OK", LISTED).is_some());
    let path = registry(&dir).join("sentinel-org%2Fstrasse.json");
    assert_eq!(read(&path)["project"], "Sentinel-Org/Straße");
}

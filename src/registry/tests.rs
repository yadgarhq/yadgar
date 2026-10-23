use super::*;

use serde_json::Value;

use crate::testserver::scratch_dir;

#[test]
fn the_key_is_pythons_quote_of_the_casefolded_project() {
    // Each expectation is `urllib.parse.quote(s.casefold(), safe="")`, run on
    // Python 3.14.7 (Unicode 16.0.0) on 2026-10-23, pinned as a literal.
    for (project, expected) in [
        ("acme/widget", "acme%2Fwidget"),
        ("Acme/Widget", "acme%2Fwidget"),
        ("yadgarhq/yadgar", "yadgarhq%2Fyadgar"),
        ("a b~c_d.e-f", "a%20b~c_d.e-f"),
        // Casefold, not lowercase: `ß` folds to `ss`, `ς` to `σ`, the `ﬁ`
        // ligature to `fi`, and the Kelvin sign to `k`.
        ("straße/ß", "strasse%2Fss"),
        ("ΟΔΟΣ/ς", "%CE%BF%CE%B4%CE%BF%CF%83%2F%CF%83"),
        ("ﬁle/\u{212A}", "file%2Fk"),
        ("İstanbul/x", "i%CC%87stanbul%2Fx"),
        ("group/sub/repo.io", "group%2Fsub%2Frepo.io"),
    ] {
        assert_eq!(key(project), expected, "{project}");
    }
}

#[test]
fn the_directory_is_yaadgaar_state_dir_else_xdg_state_home_else_under_home() {
    let all = Env {
        yaadgaar_state_dir: Some("/y".into()),
        state_home: Some("/s".into()),
        home: Some("/h".into()),
        ..Env::default()
    };
    assert_eq!(dir(&all), Some(PathBuf::from("/y/registry")));
    let xdg = Env {
        yaadgaar_state_dir: None,
        ..all.clone()
    };
    assert_eq!(dir(&xdg), Some(PathBuf::from("/s/yaadgaar/registry")));
    let home = Env {
        state_home: None,
        ..xdg
    };
    assert_eq!(
        dir(&home),
        Some(PathBuf::from("/h/.local/state/yaadgaar/registry"))
    );
    assert_eq!(dir(&Env::default()), None);
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_record_holds_the_four_fields_and_registered_follows_the_answer() {
    let d = scratch_dir("registry-body").join("registry");
    let path = write(&d, "Acme/Widget", Answer::Registered, 0).unwrap();
    assert_eq!(path, d.join("acme%2Fwidget.json"));
    let body = read(&path);
    assert_eq!(body["project"], "Acme/Widget", "the key sent, unchanged");
    assert_eq!(body["registered"], true);
    assert_eq!(body["checked_at"], "1970-01-01T00:00:00Z");
    assert_eq!(body["client_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body.as_object().unwrap().len(), 4, "{body}");
    write(&d, "Acme/Widget", Answer::NotRegistered, 60).unwrap();
    assert_eq!(read(&path)["registered"], false);
    assert_eq!(read(&path)["checked_at"], "1970-01-01T00:01:00Z");
}

#[test]
fn a_rewrite_is_atomic_and_leaves_no_temporary_file() {
    let d = scratch_dir("registry-atomic").join("registry");
    for n in 0..3 {
        write(&d, "acme/widget", Answer::Registered, n).unwrap();
    }
    let names: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["acme%2Fwidget.json".to_string()]);
}

#[test]
fn no_answer_writes_nothing() {
    let home = scratch_dir("registry-none");
    let env = Env {
        yaadgaar_state_dir: Some(home.join("y")),
        ..Env::default()
    };
    record(&env, "acme/widget", None);
    assert!(!home.join("y").exists());
    record(&env, "acme/widget", Some(Answer::Registered));
    assert!(home.join("y/registry/acme%2Fwidget.json").is_file());
}

//! What the config file IS on disk, and what a rebuild of it must keep.
//!
//! Split out of `config.rs` when the file passed its size ceiling, along the
//! seam `login`, `proxy` and `install` already use.

use super::*;

#[test]
fn address_and_token_round_trip_together() {
    let dir = tempdir();
    Config::new(&dir, "https://gw.example:18443/".into(), "tok-abc".into())
        .save()
        .unwrap();
    let back = Config::load_from(&dir).unwrap();
    assert_eq!(back.gateway_url(), "https://gw.example:18443/");
    assert_eq!(back.token(), "tok-abc");
    std::fs::remove_dir_all(&dir).ok();
}

/// The file, spelled out. Not generated, not round-tripped.
///
/// `config.json` is a DURABLE format: every machine that has ever run
/// `yaadgaar login` holds one, and this client has to keep reading it. A
/// round trip cannot notice a rename, because both halves rename together
/// — `#[serde(rename = "gw")]` on `gateway` writes `gw`, reads `gw`, and
/// leaves the suite green while every config in existence stops resolving
/// and every person is told to log in again.
const KNOWN_CONFIG: &str = r#"{
  "gateway": "https://gw.example:18443/",
  "token": "tok-abc"
}"#;

#[test]
fn a_config_written_by_an_older_client_still_parses() {
    // Fixed bytes IN, expected values OUT. The literal is what is on disk on
    // somebody's laptop; nothing in this test can rename with the struct.
    let dir = tempdir();
    std::fs::write(dir.join(FILE), KNOWN_CONFIG).unwrap();
    let config = Config::load_from(&dir).unwrap();
    assert_eq!(config.gateway_url(), "https://gw.example:18443/");
    assert_eq!(config.token(), "tok-abc");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_this_client_writes_is_byte_identical_to_that_format() {
    // The other direction, and it needs saying separately: a client that
    // READS the old name and WRITES a new one leaves a file the previous
    // release cannot read, which is the same outage pointing backwards.
    //
    // The directory is deliberately absent from these bytes. It is where the
    // file IS, and a file that records its own location is a second answer
    // to a question that cannot disagree with itself while it has only one.
    let dir = tempdir();
    Config::new(&dir, "https://gw.example:18443/".into(), "tok-abc".into())
        .save()
        .unwrap();
    let written = std::fs::read_to_string(dir.join(FILE)).unwrap();
    assert_eq!(written, KNOWN_CONFIG);
    std::fs::remove_dir_all(&dir).ok();
}

/// The file an enrolled machine holds, spelled out for the same reason
/// `KNOWN_CONFIG` is: every field here is DURABLE, and a rename leaves a
/// person logged in but nameless, or an install correlated to nothing.
const ENROLLED_CONFIG: &str = r#"{
  "gateway": "https://gw.example:18443/",
  "token": "tok-abc",
  "username": "sentinel.person",
  "instance": "99999999-9999-4999-8999-999999999999",
  "ca_pem": "-----BEGIN CERTIFICATE-----\nsentinel-anchor\n-----END CERTIFICATE-----\n"
}"#;

#[test]
fn an_enrolled_config_round_trips_every_field_by_its_stored_name() {
    // Fixed bytes IN, expected values OUT — a round trip cannot notice a
    // rename, because both halves rename together.
    let dir = tempdir();
    std::fs::write(dir.join(FILE), ENROLLED_CONFIG).unwrap();
    let config = Config::load_from(&dir).unwrap();
    assert_eq!(config.username(), Some("sentinel.person"));
    assert_eq!(
        config.instance(),
        Some("99999999-9999-4999-8999-999999999999")
    );
    assert_eq!(
        config.ca_pem(),
        Some("-----BEGIN CERTIFICATE-----\nsentinel-anchor\n-----END CERTIFICATE-----\n")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_from_before_these_fields_existed_still_loads() {
    // The upgrade path. Every machine that has ever run `yaadgaar login`
    // holds a two-field file, and a client that refused it would tell every
    // existing person to log in again for a feature they did not ask for.
    let dir = tempdir();
    std::fs::write(dir.join(FILE), KNOWN_CONFIG).unwrap();
    let config = Config::load_from(&dir).unwrap();
    assert_eq!(config.username(), None);
    assert_eq!(config.instance(), None);
    assert_eq!(config.ca_pem(), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_with_none_of_the_new_fields_is_written_exactly_as_before() {
    // The other direction, and it is why the three fields skip when absent.
    // A client that wrote `"username": null` into every file leaves one the
    // previous release still reads — and a whole-file diff on every laptop
    // that upgrades, over three keys nobody set.
    let dir = tempdir();
    Config::new(&dir, "https://gw.example:18443/".into(), "tok-abc".into())
        .save()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(FILE)).unwrap(),
        KNOWN_CONFIG
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_install_id_is_minted_once_and_never_changes_again() {
    // THE PROPERTY ADR-0511 CHOSE A UUID FOR. It must survive a restart, a
    // second `install`, and a re-login — a value that changed on any of
    // those would silently break the correlation it exists to provide, and
    // nothing would report it.
    let dir = tempdir();
    let mut config = Config::new(&dir, "https://gw".into(), "tok".into());
    assert_eq!(config.instance(), None);

    assert!(config.ensure_instance().unwrap(), "the first call mints");
    let minted = config.instance().expect("an id").to_string();
    assert!(!minted.is_empty());

    assert!(
        !config.ensure_instance().unwrap(),
        "a second call must not mint a second id"
    );
    assert_eq!(config.instance(), Some(minted.as_str()));

    // It was PERSISTED, not merely held: a restart re-reads the file, and
    // an id that lived only in memory would be new on every process.
    let reloaded = Config::load_from(&dir).unwrap();
    assert_eq!(reloaded.instance(), Some(minted.as_str()));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn two_installs_do_not_share_an_id() {
    // "Distinct per checkout". A constant would satisfy every assertion
    // above and correlate every machine in the deployment to one another.
    let a = tempdir();
    let b = tempdir();
    let mut one = Config::new(&a, "https://gw".into(), "tok".into());
    let mut two = Config::new(&b, "https://gw".into(), "tok".into());
    one.ensure_instance().unwrap();
    two.ensure_instance().unwrap();
    assert_ne!(one.instance(), two.instance());
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

#[test]
fn the_tool_cache_lands_beside_the_config_it_was_loaded_with() {
    // The hazard this replaces: `read_tool_cache` and `write_tool_cache`
    // went through the process-global `base_dir()` while the config came
    // from wherever `load_from` was pointed. A test that loaded a config
    // from a temp directory read and OVERWROTE the real
    // `~/.config/yadgar/tools-cache.json` belonging to the person running
    // it, and nothing said so.
    let dir = tempdir();
    let config = Config::new(&dir, "https://gw".into(), "tok".into());
    assert_eq!(config.read_tool_cache(), None);
    config.write_tool_cache("{\"tools\":[]}").unwrap();
    assert_eq!(config.read_tool_cache().as_deref(), Some("{\"tools\":[]}"));
    assert!(
        dir.join(TOOL_CACHE).exists(),
        "the cache was written somewhere other than beside its own config"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_config_says_how_to_fix_it() {
    // The error a person actually meets first. It must name the command, not
    // the filename.
    let err = Config::load_from(Path::new("/nonexistent/yadgar")).unwrap_err();
    assert!(matches!(err, ConfigError::Missing));
    assert!(err.to_string().contains("yaadgaar login"));
}

#[test]
fn malformed_config_is_distinguishable_from_absent() {
    // Absent means "log in"; malformed means "this file is broken". Telling
    // someone to log in when the file is corrupt sends them round a loop
    // that cannot terminate.
    let dir = tempdir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE), "{ not json").unwrap();
    assert!(matches!(
        Config::load_from(&dir).unwrap_err(),
        ConfigError::Malformed(..)
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn the_config_is_not_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir();
    Config::new(&dir, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    let mode = std::fs::metadata(dir.join(FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o077,
        0,
        "a file holding a bearer token must be owner-only"
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn tempdir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "yadgar-cfg-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

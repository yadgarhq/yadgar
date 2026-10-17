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
    // `~/.config/yaadgaar/tools-cache.json` belonging to the person running
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

#[test]
fn a_stored_cleartext_gateway_is_refused_at_load_time() {
    // Ledger 717's second half: `login` and `enrol` refuse `http://` before a
    // config is ever written, but that check runs once, at adoption. This is
    // the file a PRE-FIX BINARY wrote, or one hand-edited afterwards — `serve`
    // has no prompt in front of it, so the file must be refused on its own,
    // not trusted because it is already on disk. A sentinel host, not this
    // repository's real gateway, so this could not pass by accident against a
    // fixture the implementation happens to contain.
    let dir = tempdir();
    std::fs::write(
        dir.join(FILE),
        r#"{"gateway":"http://gw.sentinel.invalid/","token":"tok-abc"}"#,
    )
    .unwrap();
    let err = Config::load_from(&dir).expect_err("a cleartext gateway must be refused");
    assert!(
        matches!(&err, ConfigError::InsecureScheme { scheme, .. } if scheme == "http"),
        "got {err:?}"
    );
    let said = err.to_string();
    assert!(said.contains("gw.sentinel.invalid"), "{said}");
    assert!(said.contains("yaadgaar login"), "{said}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_stored_gateway_with_no_scheme_is_refused_rather_than_defaulted() {
    // ADR-0569: a default chosen silently is a value nobody chose, used as if
    // somebody had. A bare host in a stored config is refused exactly like
    // `http://` is, not completed to `https` on the owner's behalf.
    let dir = tempdir();
    std::fs::write(
        dir.join(FILE),
        r#"{"gateway":"gw.sentinel.invalid/","token":"tok-abc"}"#,
    )
    .unwrap();
    let err = Config::load_from(&dir).expect_err("a schemeless gateway must be refused");
    assert!(matches!(err, ConfigError::NoScheme { .. }), "got {err:?}");
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

#[cfg(unix)]
#[test]
fn a_config_directory_it_creates_is_owner_only() {
    // Ledger 1547: the directory followed the umask — 0775 under umask 002 —
    // while the file inside it was 0600. A group-writable directory lets
    // another user replace `config.json` outright, and the 0600 on the file
    // does nothing about that. Asserted as "no group or other bits", which is
    // red under any normal umask without this test touching the process-wide
    // one.
    use std::os::unix::fs::PermissionsExt;
    let parent = tempdir();
    let dir = parent.join(DIR);
    Config::new(&dir, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "got {:o}", mode & 0o777);
    std::fs::remove_dir_all(&parent).ok();
}

#[cfg(unix)]
#[test]
fn a_looser_config_directory_left_by_an_older_client_is_tightened() {
    // Every 0.1.x install already has a 0775 directory, so fixing only the
    // create path fixes nobody who upgrades.
    use std::os::unix::fs::PermissionsExt;
    let parent = tempdir();
    let dir = parent.join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o775)).unwrap();
    Config::new(&dir, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "got {:o}", mode & 0o777);
    std::fs::remove_dir_all(&parent).ok();
}

#[cfg(unix)]
#[test]
fn the_tool_cache_path_creates_an_owner_only_directory_too() {
    // The second create site. `serve` writes the cache, and on a machine
    // whose config was written elsewhere this can be the first write.
    use std::os::unix::fs::PermissionsExt;
    let parent = tempdir();
    let dir = parent.join(DIR);
    Config::new(&dir, "https://gw".into(), "secret".into())
        .write_tool_cache("[]")
        .unwrap();
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "got {:o}", mode & 0o777);
    std::fs::remove_dir_all(&parent).ok();
}

#[cfg(unix)]
#[test]
fn a_directory_not_named_for_yaadgaar_is_never_chmodded() {
    // `YAADGAAR_CONFIG_DIR` can point anywhere. An EXISTING directory is
    // tightened only when its leaf is `yaadgaar` — which includes the
    // `./yaadgaar` fallback and an override ending in `yaadgaar` — so a
    // directory somebody chose under any other name keeps the permissions they
    // gave it.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    Config::new(&dir, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "got {:o}", mode & 0o777);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_directory_is_not_tightened() {
    // A home-manager-style setup can make `~/.config/yaadgaar` a symlink into a
    // directory something else declares. `set_permissions` follows the link
    // and would chmod that target; the link is left alone instead.
    use std::os::unix::fs::PermissionsExt;
    let parent = tempdir();
    let target = parent.join("declared");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    let link = parent.join(DIR);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    Config::new(&link, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    let mode = std::fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "got {:o}", mode & 0o777);
    std::fs::remove_dir_all(&parent).ok();
}

#[cfg(unix)]
#[test]
fn the_memory_engines_directory_is_never_chmodded() {
    // `~/.config/yadgar` belongs to the yadgar memory engine now: its
    // `config.yaml` and `secrets.env` live there. A config pointed at that
    // directory (an override, or a test) must leave its mode alone.
    use std::os::unix::fs::PermissionsExt;
    let parent = tempdir();
    let dir = parent.join("yadgar");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    Config::new(&dir, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "got {:o}", mode & 0o777);
    std::fs::remove_dir_all(&parent).ok();
}

#[test]
fn the_config_lives_in_its_own_yaadgaar_directory() {
    // Not `yadgar`: that directory holds the memory engine's secrets.
    let root = PathBuf::from("/home/x/.config");
    assert_eq!(
        base_dir_from(None, Some(root.clone())),
        root.join("yaadgaar")
    );
    assert_eq!(base_dir_from(None, None), PathBuf::from("./yaadgaar"));
}

#[test]
fn the_override_is_yaadgaar_config_dir_and_wins() {
    assert_eq!(CONFIG_DIR_ENV, "YAADGAAR_CONFIG_DIR");
    assert_eq!(
        base_dir_from(
            Some("/elsewhere".into()),
            Some(PathBuf::from("/home/x/.config"))
        ),
        PathBuf::from("/elsewhere")
    );
}

/// Plant a symlink at *name*'s temporary path pointing at a file outside the
/// directory, and give back that file.
#[cfg(unix)]
fn plant_tmp_link(dir: &Path, name: &str) -> PathBuf {
    let victim = dir.join("victim");
    std::fs::write(&victim, "untouched").unwrap();
    std::os::unix::fs::symlink(&victim, dir.join(format!(".{name}.tmp"))).unwrap();
    victim
}

#[cfg(unix)]
#[test]
fn save_never_writes_through_a_symlink_planted_at_its_temporary_path() {
    // Ledger 1553: `fs::write` follows a symlink, so a link at
    // `.config.json.tmp` sent the bearer token into whatever it pointed at, and
    // the rename then made `config.json` itself that link.
    let dir = tempdir();
    let victim = plant_tmp_link(&dir, FILE);
    Config::new(&dir, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    let meta = std::fs::symlink_metadata(dir.join(FILE)).unwrap();
    assert!(
        meta.file_type().is_file(),
        "config.json is not a plain file"
    );
    let loaded = Config::load_from(&dir).unwrap();
    assert_eq!(loaded.token(), "secret");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn the_temporary_file_is_created_exclusively_and_never_through_a_link() {
    let dir = tempdir();
    let victim = plant_tmp_link(&dir, "x");
    assert!(
        private::create_exclusive(&dir.join(".x.tmp")).is_err(),
        "followed a link"
    );
    assert!(
        private::create_exclusive(&victim).is_err(),
        "opened an existing file"
    );
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_symlink_at_config_json_itself_is_replaced_and_not_written_through() {
    let dir = tempdir();
    let victim = dir.join("victim");
    std::fs::write(&victim, "untouched").unwrap();
    std::os::unix::fs::symlink(&victim, dir.join(FILE)).unwrap();
    Config::new(&dir, "https://gw".into(), "secret".into())
        .save()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    let meta = std::fs::symlink_metadata(dir.join(FILE)).unwrap();
    assert!(meta.file_type().is_file(), "config.json is still a link");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn concurrent_writes_all_land_and_leave_no_temporary_file_behind() {
    // Several `serve` processes write the tool cache at once. With one fixed
    // temporary name, one writer unlinked or renamed another's file and that
    // writer's rename then failed.
    let dir = tempdir();
    let config = Config::new(&dir, "https://gw".into(), "secret".into());
    std::thread::scope(|s| {
        for t in 0..8 {
            let config = &config;
            s.spawn(move || {
                for i in 0..25 {
                    config.write_tool_cache(&format!("[{t},{i}]")).unwrap();
                    config.save().unwrap();
                }
            });
        }
    });
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn the_tool_cache_never_writes_through_a_planted_symlink_either() {
    let dir = tempdir();
    let victim = plant_tmp_link(&dir, TOOL_CACHE);
    Config::new(&dir, "https://gw".into(), "secret".into())
        .write_tool_cache("[]")
        .unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    let meta = std::fs::symlink_metadata(dir.join(TOOL_CACHE)).unwrap();
    assert!(meta.file_type().is_file(), "the cache is not a plain file");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_config_directory_owned_by_somebody_else_is_refused() {
    // Ledger 1553: an existing directory kept its owner, so a directory another
    // user created (and can therefore replace files in) took the credential.
    // Two uids cannot be arranged without root, so the decision is tested here
    // and the wiring by the same-owner tests above.
    let dir = PathBuf::from("/somewhere/yaadgaar");
    let err = private::refuse_foreign_owner(&dir, 0, 1000).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(err.to_string().contains("/somewhere/yaadgaar"), "{err}");
    assert!(private::refuse_foreign_owner(&dir, 1000, 1000).is_ok());
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

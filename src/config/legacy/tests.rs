//! The old config is noticed, and the engine's files beside it are never touched.

use super::*;

/// A 0.1.4-shaped `~/.config`: the engine's two files plus this client's old one.
fn seeded() -> (PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "yaadgaar-legacy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let legacy = root.join(LEGACY_DIR);
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("secrets.env"), "YADGAR_TOKEN=fake\n").unwrap();
    std::fs::write(legacy.join("config.yaml"), "engine: true\n").unwrap();
    std::fs::write(
        legacy.join(FILE),
        r#"{"gateway":"https://gw.example","token":"fake"}"#,
    )
    .unwrap();
    let current = root.join(DIR);
    (root, legacy, current)
}

/// Bytes, inode, mode and mtime of every file in *dir*, and of *dir* itself.
#[cfg(unix)]
fn stamp(dir: &Path) -> Vec<(String, Vec<u8>, u64, u32, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let meta = std::fs::metadata(dir).unwrap();
    out.push((
        ".".to_string(),
        Vec::new(),
        meta.ino(),
        meta.mode(),
        meta.mtime(),
        meta.mtime_nsec(),
    ));
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    for path in names {
        let meta = std::fs::metadata(&path).unwrap();
        out.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            std::fs::read(&path).unwrap(),
            meta.ino(),
            meta.mode(),
            meta.mtime(),
            meta.mtime_nsec(),
        ));
    }
    out
}

#[test]
fn an_old_config_with_no_new_one_gets_a_hint_naming_both_and_login() {
    let (root, legacy, current) = seeded();
    let said = hint(&current, &legacy).expect("a hint");
    assert!(
        said.contains(&legacy.join(FILE).display().to_string()),
        "{said}"
    );
    assert!(said.contains(&current.display().to_string()), "{said}");
    assert!(said.contains("yaadgaar login"), "{said}");
    assert!(said.contains("enrol"), "{said}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_tool_cache_alone_is_also_an_old_config() {
    let (root, legacy, current) = seeded();
    std::fs::remove_file(legacy.join(FILE)).unwrap();
    std::fs::write(legacy.join(TOOL_CACHE), "[]").unwrap();
    assert!(hint(&current, &legacy).is_some());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn no_hint_once_the_new_config_exists() {
    let (root, legacy, current) = seeded();
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(current.join(FILE), "{}").unwrap();
    assert_eq!(hint(&current, &legacy), None);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn no_hint_when_only_the_engine_files_are_there() {
    // `config.yaml` and `secrets.env` are the engine's. They are not an old
    // config of ours and say nothing about whether anybody needs to log in.
    let (root, legacy, current) = seeded();
    std::fs::remove_file(legacy.join(FILE)).unwrap();
    assert_eq!(hint(&current, &legacy), None);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn no_hint_when_the_override_points_at_the_old_directory() {
    let (root, legacy, _) = seeded();
    assert_eq!(hint(&legacy, &legacy), None);
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
#[test]
fn detection_leaves_every_file_in_the_old_directory_exactly_as_it_was() {
    use std::os::unix::fs::PermissionsExt;
    let (root, legacy, current) = seeded();
    // Group-readable, which `ensure_private_dir` used to tighten.
    std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = stamp(&legacy);

    assert!(hint(&current, &legacy).is_some());

    assert_eq!(stamp(&legacy), before);
    assert!(
        !current.exists(),
        "detection created the new directory; only login and enrol may"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_new_directory_and_the_old_one_are_different_names() {
    assert_eq!(DIR, "yaadgaar");
    assert_eq!(LEGACY_DIR, "yadgar");
}

//! Reading and writing the JSON configs, without ever losing one.
//!
//! Two rules, both earned:
//!
//! * **A config that will not parse is a refusal, not an empty dict.** The
//!   Python version caught `ValueError` and carried on with `{}`, which then
//!   got written back — destroying a file whose only problem was a trailing
//!   comma somebody was mid-way through fixing. D75 is explicit: never clobber
//!   a config that cannot be parsed.
//! * **Every write is atomic**, so a crash mid-install cannot truncate a
//!   settings file yadgar does not own.

use std::path::Path;

use serde_json::Value;

/// Load *path*, or an empty object if it does not exist.
///
/// A parse failure is an error naming the file. The file is the user's, it is
/// probably the only copy, and "yadgar deleted my settings" is a worse outcome
/// than "yadgar refused to run".
pub fn load(path: &Path) -> anyhow::Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Object(default())),
        Err(e) => anyhow::bail!("cannot read {}: {e}", path.display()),
    };
    if text.trim().is_empty() {
        return Ok(Value::Object(default()));
    }
    serde_json::from_str(&text).map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid JSON ({e}) — refusing to touch it. \
             Fix or move the file and re-run; overwriting it would destroy \
             whatever is in there.",
            path.display()
        )
    })
}

fn default() -> serde_json::Map<String, Value> {
    serde_json::Map::new()
}

/// Serialise *value* over *path* atomically.
pub fn write_atomic(path: &Path, value: &Value) -> anyhow::Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_atomic_text(path, &text)
}

/// Write *text* over *path* atomically: temp file in the same directory, then
/// rename. Same directory because a rename across filesystems is not atomic and
/// falls back to a copy — which is precisely the truncation this avoids.
pub fn write_atomic_text(path: &Path, text: &str) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("yadgar");
    let tmp = dir.join(format!(".{name}.yadgar-tmp.{}", std::process::id()));

    let result = (|| -> std::io::Result<()> {
        std::fs::write(&tmp, text)?;
        // Carry the existing file's mode across. A fresh temp file is 0600, and
        // renaming it over a 0644 settings.json would quietly change the
        // permissions of a file yadgar does not own.
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))
}

/// Serialise *value* over *path* — or DELETE the file, when *value* holds nothing.
///
/// *container* is the ONE key in this file that yadgar itself creates:
/// `hooks` in `settings.json`, `mcpServers` in `~/.claude.json`. It is passed
/// in rather than known here because which key is structural depends on which
/// file is being written, and getting that backwards is the bug below.
///
/// The residue this removes: on a machine with no `~/.claude`, install created
/// `settings.json` and `~/.claude.json`, and uninstall left both behind as
/// `{"hooks":{}}` and `{"mcpServers":{}}`. D76 says uninstall removes exactly
/// what install added, and a file yadgar created, then emptied and left, is not
/// that.
///
/// **The rule is what the file now HOLDS, not who created it** — which is the
/// only thing knowable here, since nothing records that. A value that is vacant
/// after the strip contains nothing of anybody's, so deleting the file cannot
/// destroy work, whoever made it. Anything else is written back untouched: a
/// `settings.json` still carrying somebody's `model` or `permissions` stays,
/// even though yadgar has just emptied its `hooks`. The two failure modes are
/// not comparable — leaving an empty file is untidy, and deleting a file
/// somebody wrote is destroying their work — so this errs the same way every
/// time.
pub fn write_or_prune(path: &Path, value: &Value, container: &str) -> anyhow::Result<()> {
    if is_vacant(value, container) {
        if remove_regular_file(path)? {
            return Ok(());
        }
        // Nothing to delete and nothing worth writing: an absent path must not
        // be CREATED holding `{}` on the way out of an uninstall.
        if !path.exists() {
            return Ok(());
        }
    }
    write_atomic(path, value)
}

/// Delete *path*, if *path* is a regular file. Returns whether a file went.
///
/// **A symlink is not unlinked here, and that is not the same as a symlink
/// being safe.** `remove_file` on a link deletes the link, which takes
/// somebody else's file out of their own configuration — the harm
/// `rules::check_owned` names — so this refuses. But the only caller then
/// falls through to [`write_atomic`], which lands the stripped value by
/// `rename(2)`: rename REPLACES the link rather than writing through it, so a
/// symlinked `settings.json` comes out of an uninstall as a regular file
/// holding the stripped value.
///
/// That is a KNOWN GAP, stated rather than promised away. It is not the loss
/// this function prevents — the link's target keeps every byte it had, and a
/// re-run of whatever declares the link puts it back — but a caller must not
/// read this function as "a symlinked config survives an uninstall intact".
/// Nothing here refuses a symlinked config the way `check_reference_target`
/// refuses a symlinked `CLAUDE.md`, and closing the gap means adding that
/// refusal, not changing this.
pub fn remove_regular_file(path: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => {}
        _ => return Ok(false),
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => anyhow::bail!("cannot remove {}: {e}", path.display()),
    }
}

/// Does this value hold nothing at all — nothing of anybody's, anywhere in it?
///
/// **A KEY IS CONTENT.** This used to hunt for a leaf SCALAR and never read a
/// key at all, so `{"mcpServers":{"other-server":{}}}` was "nothing" and an
/// uninstall DELETED `~/.claude.json` — a third party's MCP registration gone,
/// because the only thing recording it was the name, and the name was not
/// looked at. `{"permissions":{"allow":[],"deny":[],"ask":[]}}` went the same
/// way: somebody's permissions configuration, holding no scalar and deleted
/// for it. Both were measured against the built binary, not argued.
///
/// So the rule is flat, and it is the smallest one that can be true. Exactly
/// two values are nothing: the empty object, and the empty *container* — the
/// one key in this file that yadgar creates itself and has just emptied.
/// **Everything else, at any depth, is somebody's.** Not recursive, because a
/// recursive form asks the same question of an entry INSIDE the container,
/// where the answer is opposite: an MCP server that somebody named `hooks` is
/// a name, and reading it as a structure is the bug above rebuilt one level
/// down.
///
/// `null` is content too, and for the same reason: somebody wrote
/// `"model": null` on purpose. Every judgement call in here goes the same
/// way — under-delete, never over.
fn is_vacant(value: &Value, container: &str) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    map.iter()
        .all(|(key, held)| key == container && held.as_object().is_some_and(|c| c.is_empty()))
}

/// Borrow `value[key]` as an object, creating it if absent.
pub fn ensure_object<'a>(
    value: &'a mut Value,
    key: &str,
) -> Option<&'a mut serde_json::Map<String, Value>> {
    if !value.is_object() {
        *value = Value::Object(default());
    }
    let root = value.as_object_mut()?;
    let slot = root.entry(key).or_insert_with(|| Value::Object(default()));
    if !slot.is_object() {
        *slot = Value::Object(default());
    }
    slot.as_object_mut()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::hooks::HOOKS_KEY;
    use crate::install::mcp::SERVERS_KEY;

    #[test]
    fn an_unparseable_config_is_refused_rather_than_replaced() {
        // The failure this prevents: the Python loader returned {} on a parse
        // error and then wrote that back, deleting every setting in the file.
        let dir = crate::install::tests::scratch("jsonfile-parse");
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ \"hooks\": [ ,,, }").unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains("not valid JSON"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ \"hooks\": [ ,,, }"
        );
    }

    #[test]
    fn a_value_holding_nothing_is_deleted_and_one_holding_anything_is_kept() {
        // Both directions of the uninstall rule, at the level of the function
        // that decides it. The asymmetry is the point: an emptied file is
        // untidy, and a deleted `settings.json` carrying somebody's `model` is
        // their work destroyed — this module has already had one bug of that
        // exact shape, and then a second.
        let dir = crate::install::tests::scratch("jsonfile-prune");

        // The ONE shape that is nothing: the container yadgar emptied, and
        // nothing else in the file at all.
        let vacant = dir.join("vacant.json");
        std::fs::write(&vacant, "{}").unwrap();
        write_or_prune(&vacant, &serde_json::json!({ "hooks": {} }), HOOKS_KEY).unwrap();
        assert!(!vacant.exists(), "a file holding nothing was left behind");

        let theirs = dir.join("theirs.json");
        std::fs::write(&theirs, "{}").unwrap();
        write_or_prune(
            &theirs,
            &serde_json::json!({ "hooks": {}, "model": "opus" }),
            HOOKS_KEY,
        )
        .unwrap();
        assert_eq!(
            load(&theirs).unwrap()["model"],
            "opus",
            "a file carrying somebody's own setting was deleted"
        );

        // `null` is something a person wrote, not emptiness.
        let explicit = dir.join("explicit-null.json");
        std::fs::write(&explicit, "{}").unwrap();
        write_or_prune(&explicit, &serde_json::json!({ "model": null }), HOOKS_KEY).unwrap();
        assert!(explicit.exists(), "a key somebody set to null was deleted");

        // A KEY NOBODY HERE CHOSE is content, whatever hangs off it. `list` was
        // in the vacant fixture above and deleted the file, because the rule
        // hunted for leaf scalars and an empty array has none — so a name
        // somebody picked read as nothing.
        let named = dir.join("named-empty-list.json");
        std::fs::write(&named, "{}").unwrap();
        write_or_prune(&named, &serde_json::json!({ "list": [] }), HOOKS_KEY).unwrap();
        assert!(named.exists(), "a name somebody chose was read as nothing");

        // An entry INSIDE the container, likewise: the third party's server is
        // registered by name, and its body being empty says nothing about that.
        let registered = dir.join("registered.json");
        std::fs::write(&registered, "{}").unwrap();
        write_or_prune(
            &registered,
            &serde_json::json!({ "mcpServers": { "other-server": {} } }),
            SERVERS_KEY,
        )
        .unwrap();
        assert!(
            registered.exists(),
            "somebody else's MCP registration was deleted"
        );

        // Keys with keys under them but no scalar anywhere — the empty
        // `permissions` block Claude Code writes into a fresh `settings.json`.
        let structure = dir.join("structure.json");
        std::fs::write(&structure, "{}").unwrap();
        write_or_prune(
            &structure,
            &serde_json::json!({ "permissions": { "allow": [], "deny": [], "ask": [] } }),
            HOOKS_KEY,
        )
        .unwrap();
        assert!(
            structure.exists(),
            "somebody's permissions configuration was deleted"
        );

        // The container carrying a key of somebody else's is not empty either,
        // even when what is under that key is. `hooks.rs` calls an entry that
        // arrived with no hooks in it "somebody else's oddity".
        let oddity = dir.join("oddity.json");
        std::fs::write(&oddity, "{}").unwrap();
        write_or_prune(
            &oddity,
            &serde_json::json!({ "hooks": { "Stop": [] } }),
            HOOKS_KEY,
        )
        .unwrap();
        assert!(
            oddity.exists(),
            "an event key somebody left behind was deleted"
        );

        // And the container is only nothing when it is THE container of this
        // file. An MCP server somebody named `hooks` is a name, not a structure.
        let homonym = dir.join("homonym.json");
        std::fs::write(&homonym, "{}").unwrap();
        write_or_prune(&homonym, &serde_json::json!({ "hooks": {} }), SERVERS_KEY).unwrap();
        assert!(
            homonym.exists(),
            "a key that is only structural elsewhere was deleted"
        );
    }

    #[test]
    fn a_symlinked_config_is_never_unlinked_and_its_target_keeps_every_byte() {
        // The symlink arm of `remove_regular_file` was DEAD: no caller path
        // reached it, so swapping `symlink_metadata` for `metadata` — which
        // follows the link and deletes it as an ordinary file — left the suite
        // green, and a `panic!` in the arm was never reached. A guard nothing
        // exercises, carrying a comment that promised a mechanism the code does
        // not have, is not a guard.
        //
        // Reached here through `write_or_prune`, which is its only caller.
        let dir = crate::install::tests::scratch("jsonfile-symlink");
        let target = dir.join("declared-elsewhere.json");
        let theirs = "{\"theirs\": true}\n";
        std::fs::write(&target, theirs).unwrap();
        let link = dir.join("settings.json");
        crate::install::tests::require_symlink(&target, &link);

        write_or_prune(&link, &serde_json::json!({ "hooks": {} }), HOOKS_KEY).unwrap();

        // What the guard IS for: the link is not unlinked, so the file it
        // points at is not taken out of somebody's own configuration, and it
        // comes back byte for byte.
        assert!(target.exists(), "the symlink's target was deleted");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), theirs);

        // And what it is NOT for, asserted so it cannot change by accident.
        // `write_atomic` lands by `rename(2)`, which REPLACES the link rather
        // than writing through it, so the path is a regular file afterwards.
        // Closing that gap means refusing a symlinked config the way
        // `check_reference_target` refuses a symlinked `CLAUDE.md` — a
        // deliberate change, not a silent one.
        let after = std::fs::symlink_metadata(&link).expect("the link itself was removed");
        assert!(
            !after.file_type().is_symlink(),
            "a symlinked config survived the write — better than the comment \
             promises, so update the comment rather than this assertion"
        );
        assert_eq!(load(&link).unwrap(), serde_json::json!({ "hooks": {} }));
    }

    #[test]
    fn an_uninstall_does_not_create_the_file_it_is_removing() {
        let dir = crate::install::tests::scratch("jsonfile-no-create");
        let path = dir.join("absent.json");
        write_or_prune(&path, &serde_json::json!({}), HOOKS_KEY).unwrap();
        assert!(!path.exists(), "an absent config was created holding {{}}");
    }

    #[test]
    fn a_missing_config_loads_as_empty() {
        let dir = crate::install::tests::scratch("jsonfile-missing");
        let value = load(&dir.join("nope.json")).unwrap();
        assert_eq!(value, serde_json::json!({}));
    }

    #[test]
    fn a_write_replaces_the_file_rather_than_writing_through_it() {
        // Atomicity is named as an earned rule at the top of this module and
        // nothing tested it: replacing the temp-file-and-rename with a straight
        // `fs::write` left the suite green. A truncating write over a
        // settings.json yadgar does not own leaves it half-written when the
        // process dies or the disk fills — the exact loss the rule is for.
        //
        // A HARD LINK is what makes the difference observable. A rename gives
        // the name a new inode and leaves the old one whole; a write through
        // changes the bytes both names see.
        let dir = crate::install::tests::scratch("jsonfile-atomic");
        let path = dir.join("settings.json");
        std::fs::write(&path, "theirs\n").unwrap();
        let witness = dir.join("witness.json");
        std::fs::hard_link(&path, &witness).unwrap();

        write_atomic_text(&path, "ours\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ours\n");
        assert_eq!(
            std::fs::read_to_string(&witness).unwrap(),
            "theirs\n",
            "the file was written through instead of replaced, so a crash \
             mid-write would have truncated it"
        );
        // And the temp file is not left lying beside it.
        let strays: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("yadgar-tmp"))
            .collect();
        assert!(strays.is_empty(), "{strays:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_write_carries_the_existing_files_mode_across() {
        // Deleting the carry-over left the suite green, so re-permissioning a
        // settings.json yadgar does not own went unnoticed. The temp file is
        // created fresh and takes the umask's mode, and renaming it over the
        // target hands the person's file whatever that happened to be.
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::install::tests::scratch("jsonfile-mode");

        // A mode a fresh file will NOT have on its own, so the assertion cannot
        // pass by coincidence of this machine's umask.
        let probe = dir.join("umask-probe");
        std::fs::write(&probe, "x").unwrap();
        let fresh = std::fs::metadata(&probe).unwrap().permissions().mode() & 0o777;
        let wanted = if fresh == 0o600 { 0o640 } else { 0o600 };

        let path = dir.join("settings.json");
        std::fs::write(&path, "theirs\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(wanted)).unwrap();

        write_atomic_text(&path, "ours\n").unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            wanted,
            "the rename re-permissioned a file yadgar does not own (fresh files \
             here are {fresh:o})"
        );
    }
}

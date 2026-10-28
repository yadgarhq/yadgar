//! The writes a pass makes to files in the list, beside the create. Each one
//! checks the file AGAIN at the moment it writes, because the agent or Claude
//! Code can change the file between the scan and the write.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::super::record::{self, Desired};

pub(crate) fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A temporary file BESIDE the list (in `tasks/`, same filesystem, not a list
/// directory the probe reads), created exclusively, never through a link.
pub(super) fn temp_with(dir: &Path, body: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write as _;
    let parent = dir.parent().ok_or("the list has no parent directory")?;
    let stem = format!(
        ".yaadgaar-{}",
        dir.file_name().and_then(|n| n.to_str()).unwrap_or("list")
    );
    let (tmp, mut file) =
        crate::config::create_first_free(crate::config::temp_names(parent, &stem))
            .map_err(|e| format!("{}: {e}", parent.display()))?;
    if let Err(e) = file.write_all(body).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", tmp.display()));
    }
    Ok(tmp)
}

/// Rewrite an owned file. `false` when it no longer carries the URN: then it
/// is not ours, and it is left alone.
pub(crate) fn rewrite(dir: &Path, id: u64, d: &Desired) -> Result<bool, String> {
    let target = dir.join(format!("{id}.json"));
    if record::owner(&read_json(&target)?) != Some(d.urn.as_str()) {
        return Ok(false);
    }
    let tmp = temp_with(dir, record::record(id, d).to_string().as_bytes())?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", target.display())
    })?;
    Ok(true)
}

/// Take over the record of *d* at *id* ([`record::copy_of`], checked again
/// now) and write it from yaadgaar, never from what the file says. `None`
/// when the file is no longer a record of *d*; else whether it was written
/// (a copy that already matches is left as it is).
pub(super) fn adopt_copy(dir: &Path, id: u64, d: &Desired) -> Result<Option<bool>, String> {
    let target = dir.join(format!("{id}.json"));
    let Ok(file) = read_json(&target) else {
        return Ok(None);
    };
    if !record::copy_of(&file, d) {
        return Ok(None);
    }
    let want = record::record(id, d);
    if file == want {
        return Ok(Some(false));
    }
    let tmp = temp_with(dir, want.to_string().as_bytes())?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", target.display())
    })?;
    Ok(Some(true))
}

/// Delete an extra record of *d*, after checking again that it is one.
pub(super) fn drop_copy(dir: &Path, id: u64, d: &Desired) -> Result<bool, String> {
    let target = dir.join(format!("{id}.json"));
    match read_json(&target) {
        Ok(file) if record::copy_of(&file, d) => std::fs::remove_file(&target)
            .map(|()| true)
            .map_err(|e| format!("{}: {e}", target.display())),
        _ => Ok(false),
    }
}

/// Delete an owned file, after checking again that it is ours.
pub(crate) fn remove(dir: &Path, id: u64, urn: &str) -> Result<bool, String> {
    let target = dir.join(format!("{id}.json"));
    match read_json(&target) {
        Ok(file) if record::owner(&file) == Some(urn) => std::fs::remove_file(&target)
            .map(|()| true)
            .map_err(|e| format!("{}: {e}", target.display())),
        _ => Ok(false),
    }
}

/// Raise `.highwatermark` to at least *id*, as Claude Code does on its own
/// deletes, so an id this client used is never handed out again.
///
/// NEVER LOWERS IT: the mark is read again right before the write, and the
/// larger of that and *id* is written, so a mark Claude Code raised after the
/// scan survives. A RACE REMAINS: Claude Code takes no lock this client knows
/// of (its `.lock` is unexplained, plan F10), so a delete Claude Code stamps
/// between this read and the rename below is overwritten with the smaller
/// value. The window is one read and one rename; ids it could hand out again
/// are only the ones that one delete freed.
///
/// The write-back hooks call this too, after a delete outside a pass.
pub(crate) fn stamp_highwatermark(dir: &Path, id: u64) -> Result<(), String> {
    let target = dir.join(super::super::shape::HIGHWATERMARK);
    let on_disk = std::fs::read_to_string(&target)
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if on_disk >= id {
        return Ok(());
    }
    let tmp = temp_with(dir, id.to_string().as_bytes())?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", target.display())
    })
}

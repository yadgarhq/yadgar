//! The recorded shape of a Claude Code task list directory, and the check
//! against it.
//!
//! The format is internal to Claude Code (yadgarhq/docs
//! `plans/harness-task-sync.md`, F7). What IS documented is the logical record
//! the task tools take and return (F6), and the keys found on disk are exactly
//! that record (F8). So the check is strict: every entry in the directory must
//! be one this shape names, every task file must hold only recorded keys of
//! the recorded types, and anything else is a [`Mismatch`]. A mismatch means
//! "do not write here", never "write anyway".
//!
//! THE FIXTURE IS THE ONE SOURCE. `tests/fixtures/claude-code/task-shape-v1.json`
//! is compiled in here and read by the canary script too, so the probe and the
//! canary cannot disagree about what "the recorded shape" means. A red canary
//! means: change the fixture, and this check follows.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

/// The recorded shape, as committed.
pub const FIXTURE: &str = include_str!("../../tests/fixtures/claude-code/task-shape-v1.json");

/// The fixture format this code reads. A fixture of another format is refused
/// rather than half-read.
const FORMAT: u64 = 1;

/// The lock marker Claude Code keeps in every list directory.
pub const LOCK: &str = ".lock";

/// The id high-water mark Claude Code writes when a task is deleted.
pub const HIGHWATERMARK: &str = ".highwatermark";

/// The type a record key must hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    String,
    Object,
    StringArray,
}

/// The per-task record, as the fixture records it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RecordShape {
    pub required: Vec<String>,
    pub optional: Vec<String>,
    pub types: BTreeMap<String, Kind>,
    pub statuses: Vec<String>,
}

#[derive(Deserialize)]
struct Fixture {
    format: u64,
    record: RecordShape,
}

/// What a directory that passed the check holds.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The task ids, ascending.
    pub ids: Vec<u64>,
    /// `.highwatermark`, when the file exists.
    pub highwatermark: Option<u64>,
    /// Whether `.lock` exists.
    pub lock: bool,
}

/// Why a directory does not have the recorded shape. Names the entry, so the
/// person reading the banner can look at the file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{entry}: {reason}")]
pub struct Mismatch {
    pub entry: String,
    pub reason: String,
}

impl Mismatch {
    fn new(entry: &str, reason: impl Into<String>) -> Self {
        Self {
            entry: entry.to_string(),
            reason: reason.into(),
        }
    }
}

/// The committed shape. `Err` when the fixture itself is unreadable, which is a
/// build defect a test pins rather than a state a session can reach.
pub fn recorded() -> Result<RecordShape, String> {
    parse(FIXTURE)
}

/// A fixture text, parsed and checked for the format this code reads.
pub fn parse(text: &str) -> Result<RecordShape, String> {
    let fixture: Fixture =
        serde_json::from_str(text).map_err(|e| format!("the shape fixture is not valid: {e}"))?;
    if fixture.format != FORMAT {
        return Err(format!(
            "the shape fixture is format {}, and this build reads format {FORMAT}",
            fixture.format
        ));
    }
    Ok(fixture.record)
}

/// Check every entry of *dir* against *shape*. Read-only.
///
/// A directory that cannot be read is a mismatch, not an empty pass: the
/// caller decides what an ABSENT list directory means (Claude Code creates it
/// lazily), and it must ask that question itself rather than have this check
/// answer "fine" for a permission error.
pub fn check_dir(dir: &Path, shape: &RecordShape) -> Result<Listing, Mismatch> {
    let shown = dir.display().to_string();
    let entries = std::fs::read_dir(dir).map_err(|e| Mismatch::new(&shown, e.to_string()))?;
    let mut listing = Listing::default();
    for entry in entries {
        let entry = entry.map_err(|e| Mismatch::new(&shown, e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        // `symlink_metadata`: a symlink is not a regular file, whatever it
        // points at, and following one would read outside the directory.
        let meta =
            std::fs::symlink_metadata(&path).map_err(|e| Mismatch::new(&name, e.to_string()))?;
        if !meta.file_type().is_file() {
            return Err(Mismatch::new(&name, "is not a regular file"));
        }
        match name.as_str() {
            LOCK if meta.len() == 0 => listing.lock = true,
            LOCK => return Err(Mismatch::new(&name, "is not empty")),
            HIGHWATERMARK => listing.highwatermark = Some(highwatermark(&path)?),
            _ => {
                let Some(stem) = name.strip_suffix(".json") else {
                    return Err(Mismatch::new(
                        &name,
                        "is not an entry the recorded shape names",
                    ));
                };
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| Mismatch::new(&name, e.to_string()))?;
                let record: Value = serde_json::from_str(&text)
                    .map_err(|e| Mismatch::new(&name, format!("is not JSON: {e}")))?;
                listing.ids.push(check_record(stem, &record, shape)?);
            }
        }
    }
    listing.ids.sort_unstable();
    Ok(listing)
}

/// `.highwatermark`: a decimal integer. One trailing newline is allowed, and
/// nothing else, because Claude Code writes the bare digits.
fn highwatermark(path: &Path) -> Result<u64, Mismatch> {
    let text =
        std::fs::read_to_string(path).map_err(|e| Mismatch::new(HIGHWATERMARK, e.to_string()))?;
    let digits = text.strip_suffix('\n').unwrap_or(&text);
    decimal(digits).ok_or_else(|| {
        Mismatch::new(
            HIGHWATERMARK,
            format!("{digits:?} is not a decimal integer"),
        )
    })
}

/// A canonical decimal: digits only, no sign, no leading zero (except `0`).
/// `"01"` is refused because it would name the same id as `"1"`.
fn decimal(text: &str) -> Option<u64> {
    let canonical = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    canonical.then(|| text.parse().ok()).flatten()
}

/// Check one task record, read from the file `<stem>.json`. Returns its id.
pub fn check_record(stem: &str, record: &Value, shape: &RecordShape) -> Result<u64, Mismatch> {
    let name = format!("{stem}.json");
    let fail = |reason: String| Mismatch::new(&name, reason);
    let id = decimal(stem).ok_or_else(|| fail("the file name is not a decimal task id".into()))?;
    let Some(object) = record.as_object() else {
        return Err(fail("is not a JSON object".into()));
    };
    for key in &shape.required {
        if !object.contains_key(key) {
            return Err(fail(format!("has no `{key}`")));
        }
    }
    // `types` IS the set of allowed keys; a test pins that it is exactly
    // `required` plus `optional`, so there is one list to read here.
    for (key, value) in object {
        let Some(kind) = shape.types.get(key) else {
            return Err(fail(format!(
                "has `{key}`, which the recorded shape does not name"
            )));
        };
        if !holds(*kind, value) {
            return Err(fail(format!("`{key}` is not a {kind:?}")));
        }
    }
    if object.get("id").and_then(Value::as_str) != Some(stem) {
        return Err(fail("its `id` is not the file name".into()));
    }
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !shape.statuses.iter().any(|s| s == status) {
        return Err(fail(format!(
            "status {status:?} is not one the recorded shape names"
        )));
    }
    Ok(id)
}

fn holds(kind: Kind, value: &Value) -> bool {
    match kind {
        Kind::String => value.is_string(),
        Kind::Object => value.is_object(),
        Kind::StringArray => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
    }
}

#[cfg(test)]
#[path = "shape/tests.rs"]
mod tests;

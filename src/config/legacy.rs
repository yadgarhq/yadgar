//! Noticing a config an older client wrote, without touching it.
//!
//! Up to 0.1.4 this client kept `config.json` and `tools-cache.json` in
//! `~/.config/yadgar/`. That directory also holds the yadgar memory engine's
//! `config.yaml` and `secrets.env`, so the client now uses `~/.config/yaadgaar/`
//! and leaves the old one entirely to the engine.
//!
//! **NO FILE IS MOVED.** A migration would be this client writing into, and
//! deleting from, a directory that holds another program's secrets — the
//! highest-blast-radius step the coexistence plan contained, for a population
//! of pre-1.0 users who can simply log in again. So the old files are only
//! DETECTED, by existence alone: nothing here opens, parses, chmods or deletes
//! anything in the old directory, and nothing names any file there except the
//! two this client itself once wrote.

use std::path::{Path, PathBuf};

use super::{DIR, FILE, TOOL_CACHE};

/// The directory 0.1.4 and earlier used. The memory engine's, now.
pub const LEGACY_DIR: &str = "yadgar";

/// Where the old config would be: `<OS config dir>/yadgar`.
///
/// `None` when the OS names no config directory, in which case there is no
/// old default location to look in either.
pub fn legacy_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|root| root.join(LEGACY_DIR))
}

/// A hint to log in again, when *legacy* holds an old config and *current* none.
///
/// PURE apart from two existence checks per directory, so it is tested on
/// scratch directories rather than on anybody's home. Says nothing when
/// *current* already has a config (the person has logged in since), and
/// nothing when *legacy* and *current* are the same directory — which they are
/// when `YAADGAAR_CONFIG_DIR` points at the old one.
pub fn hint(current: &Path, legacy: &Path) -> Option<String> {
    if current == legacy || current.join(FILE).is_file() {
        return None;
    }
    let found: Vec<PathBuf> = [FILE, TOOL_CACHE]
        .iter()
        .map(|name| legacy.join(name))
        .filter(|path| path.is_file())
        .collect();
    let first = found.first()?;
    Some(format!(
        "note: found a config written by an older yaadgaar at {}; this version \
         keeps its config in {} instead (the `{DIR}` directory) and does not read \
         the old one. Run `yaadgaar login` (or `yaadgaar enrol <token>`) again to \
         create it. Nothing in {} was read, moved or deleted.",
        first.display(),
        current.display(),
        legacy.display(),
    ))
}

#[cfg(test)]
mod tests;

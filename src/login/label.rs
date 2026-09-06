//! A human-readable name for this machine, for the credential's label.
//!
//! Split out of `login.rs` when the file passed its size ceiling. The seam is
//! the obvious one and the one `proxy` and `install` already use: this
//! subsystem shares no state with the rest of `login` — it only ever answers
//! "what is this machine called" for the `label` field a login or an
//! enrolment sends.

use std::path::Path;

/// A human-readable name for this machine, for the credential's label.
///
/// Cosmetic but not pointless: it is how a person tells their laptop's
/// credential from their desktop's when revoking one, so a wrong answer here
/// costs somebody the ability to revoke confidently.
pub(super) fn label() -> String {
    label_from(|k| std::env::var(k).ok(), Path::new("/etc/hostname"))
}

pub(super) fn label_from(var: impl Fn(&str) -> Option<String>, etc_hostname: &Path) -> String {
    hostname_from(var, etc_hostname).unwrap_or_else(|| "unnamed machine".to_string())
}

/// Best effort, in the order most likely to be right on each platform.
///
/// `/etc/hostname` alone was WRONG: it is Linux-only, so macOS and Windows would
/// silently have labelled every credential "unknown host" and the whole point of
/// the field would have quietly stopped working on two of the three platforms.
/// The client must run on x86_64 and aarch64 across Linux, macOS and Windows.
///
/// BOTH SOURCES ARE HANDED IN, rather than read here. Reading the environment
/// and `/etc/hostname` directly is why none of this order was ever exercised: a
/// test could only assert whatever the machine running it happened to be called,
/// so on a Linux host with `HOSTNAME` set, every arm but the first is
/// unreachable — and the fallbacks that exist FOR the other two platforms are
/// then never executed anywhere.
pub(super) fn hostname_from(
    var: impl Fn(&str) -> Option<String>,
    etc_hostname: &Path,
) -> Option<String> {
    // Windows sets this; Unix shells usually export it, though a non-interactive
    // shell may not, which is why the file fallback stays.
    for name in ["COMPUTERNAME", "HOSTNAME"] {
        if let Some(v) = var(name).filter(|v| !v.trim().is_empty()) {
            return Some(v.trim().to_string());
        }
    }
    // Linux, and some BSDs. Absent on macOS and Windows, which is fine — the
    // environment above covers them, and the fallback covers neither being set.
    std::fs::read_to_string(etc_hostname)
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}

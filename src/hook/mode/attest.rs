//! The sudo attestation: what lets protected mode relax (ADR-1028, ruling B
//! on yaadgaar #439 item 5).
//!
//! Protected mode refuses many shapes only because they might hide an
//! escalation (`guard/escalate.rs`). When sudo cannot be reused without a
//! person, a hidden escalation stops at a password prompt, so only a line
//! that NAMES an escalator or the protected path needs refusing.
//!
//! **The hook cannot read sudoers**: `/etc/sudoers` is root-only (0440)
//! almost everywhere, so a verdict computed in the hook is UNKNOWN. So
//! `sudo yaadgaar guard lock` computes it as root and writes this file into
//! the protected directory, root-owned, beside the exceptions file. It holds
//! the verdict and a FINGERPRINT of every input the verdict depends on: the
//! sudoers files, doas's, nsswitch (where sudo may look its rules up) and
//! PAM's (which decides whether sudo asks a password). A non-root user can
//! `stat` each of them without reading it.
//!
//! **FAIL CLOSED.** The hook relaxes only when the file passes the same
//! ownership walk as the exceptions file ([`super::verify`]) AND its text is
//! exactly what SAFE with today's fingerprints renders. A changed input, a
//! missing file, an unreadable directory, a NOT SAFE verdict or a format this
//! build does not know: the strict rules. Cost: one verified walk and a stat
//! per input and per directory entry; no process, no network.
//!
//! **What this does not see**: a sudoers source other than files (refused at
//! lock time, see [`nsswitch_files_only`]); polkit, which decides whether
//! `pkexec`, `run0`, `systemd-run` and `machinectl` ask a person, and may
//! keep an authorisation (`auth_admin_keep`) that a name built at run time
//! would reuse: so ANY of them present is NOT SAFE ([`polkit_escalators`]),
//! which on most systemd machines keeps the strict rules; a `sudo.conf`
//! plugin or path (refused, [`sudo_conf_modelled`]); and anything an input's
//! fingerprint does not change for.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::{Location, Verified};

/// The attestation's name in the protected directory.
pub const ATTESTATION_FILE: &str = "sudo-attestation";

/// What the verdict depends on: FIXED system paths, never read from the
/// environment. An absent one fingerprints as absent.
pub const INPUTS: &[&str] = &[
    "/etc/sudoers",          // ADR-0569-EXCEPTION(CB): sudo's own fixed path, stat only.
    "/etc/sudoers.d",        // ADR-0569-EXCEPTION(CB): sudo's own fixed path, stat only.
    "/etc/sudo.conf",        // ADR-0569-EXCEPTION(CB): sudo's plugin configuration, stat only.
    "/etc/doas.conf",        // ADR-0569-EXCEPTION(CB): doas's own fixed path, stat only.
    "/etc/nsswitch.conf",    // ADR-0569-EXCEPTION(CB): where sudo looks its rules up, stat only.
    "/etc/pam.d",            // ADR-0569-EXCEPTION(CB): whether sudo asks a password, stat only.
    "/usr/lib/pam.d",        // ADR-0569-EXCEPTION(CB): vendor PAM configuration, stat only.
    "/usr/etc/pam.d",        // ADR-0569-EXCEPTION(CB): vendor PAM configuration, stat only.
    "/etc/security",         // ADR-0569-EXCEPTION(CB): PAM module configuration, stat only.
    "/etc/polkit-1/rules.d", // ADR-0569-EXCEPTION(CB): polkit rules, stat only.
    "/usr/share/polkit-1/rules.d", // ADR-0569-EXCEPTION(CB): vendor polkit rules, stat only.
    "/etc/passwd",           // ADR-0569-EXCEPTION(CB): the account database, stat only.
    "/etc/group",            // ADR-0569-EXCEPTION(CB): the group database, stat only.
    "/etc/gshadow",          // ADR-0569-EXCEPTION(CB): group passwords, stat only.
];

/// The escalators that authenticate through polkit, not sudo.
pub const POLKIT_NAMES: &[&str] = &["pkexec", "run0", "systemd-run", "machinectl"];

/// Where a polkit escalator is looked for: FIXED system directories.
pub const POLKIT_DIRS: &[&str] = &[
    "/usr/bin",                   // ADR-0569-EXCEPTION(CB): fixed system path, stat only.
    "/bin",                       // ADR-0569-EXCEPTION(CB): fixed system path, stat only.
    "/usr/sbin",                  // ADR-0569-EXCEPTION(CB): fixed system path, stat only.
    "/sbin",                      // ADR-0569-EXCEPTION(CB): fixed system path, stat only.
    "/usr/local/bin",             // ADR-0569-EXCEPTION(CB): fixed system path, stat only.
    "/run/current-system/sw/bin", // ADR-0569-EXCEPTION(CB): NixOS's fixed system path, stat only.
    "/run/wrappers/bin",          // ADR-0569-EXCEPTION(CB): NixOS's setuid wrappers, stat only.
];

/// Where polkit's daemon or its D-Bus activation file may be: any present
/// means polkit is reachable over D-Bus (`busctl`, `gdbus`, `dbus-send`),
/// whatever escalator binary is or is not installed.
pub const POLKIT_SERVICE: &[&str] = &[
    "/usr/share/dbus-1/system-services/org.freedesktop.PolicyKit1.service", // ADR-0569-EXCEPTION(CB): polkit's fixed D-Bus activation file, stat only.
    "/run/current-system/sw/share/dbus-1/system-services/org.freedesktop.PolicyKit1.service", // ADR-0569-EXCEPTION(CB): the same on NixOS, stat only.
    "/usr/lib/polkit-1", // ADR-0569-EXCEPTION(CB): polkitd's fixed directory, stat only.
    "/usr/libexec/polkitd", // ADR-0569-EXCEPTION(CB): polkitd's fixed path, stat only.
    "/usr/libexec/polkit-1", // ADR-0569-EXCEPTION(CB): polkitd's fixed directory, stat only.
    "/run/current-system/sw/lib/polkit-1", // ADR-0569-EXCEPTION(CB): polkitd on NixOS, stat only.
    "/etc/polkit-1",     // ADR-0569-EXCEPTION(CB): polkit's configuration, stat only.
];

/// The [`POLKIT_SERVICE`] paths present under *paths*, and a running
/// `polkitd` found in *proc* (`/proc`). Any of them is NOT SAFE.
pub fn polkit_present(paths: &[&Path], proc: &Path) -> Vec<String> {
    let mut found: Vec<String> = paths
        .iter()
        .filter(|p| super::present(p))
        .map(|p| p.display().to_string())
        .collect();
    if let Ok(entries) = std::fs::read_dir(proc) {
        let running = entries.flatten().any(|e| {
            std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == "polkitd")
        });
        if running {
            found.push("a running polkitd".into());
        }
    }
    found
}

/// Every input: [`INPUTS`], [`POLKIT_SERVICE`], and each [`POLKIT_NAMES`]
/// in each [`POLKIT_DIRS`], so polkit installed after `lock` makes the
/// attestation stale.
pub fn inputs() -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = INPUTS
        .iter()
        .chain(POLKIT_SERVICE)
        .map(PathBuf::from)
        .collect();
    for dir in POLKIT_DIRS {
        for name in POLKIT_NAMES.iter().chain(PAM_ESCALATORS) {
            all.push(Path::new(dir).join(name));
        }
    }
    all
}

/// Escalators whose PAM stack `lock` judges when installed in a
/// [`POLKIT_DIRS`] directory (`lock/pam.rs`): fingerprinted, so one
/// installed after `lock` makes the attestation stale.
pub const PAM_ESCALATORS: &[&str] = &["su", "doas"];

/// The polkit escalators present in *dirs* (a dangling symlink included),
/// in order. polkit may keep an authorisation (`auth_admin_keep`) and its
/// JavaScript rules cannot be modelled, so ANY of them present keeps the
/// strict rules (fail closed).
pub fn polkit_escalators(dirs: &[&Path]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in dirs {
        for name in POLKIT_NAMES {
            let path = dir.join(name);
            if super::present(&path) {
                found.push(path);
            }
        }
    }
    found
}

/// Whether sudo.conf text *conf* (`None`: no file) leaves sudo on the
/// policy the verdict models: no `Plugin` (another policy, or another
/// `sudoers_file`) and no `Path` directive.
pub fn sudo_conf_modelled(conf: Option<&str>) -> bool {
    let Some(conf) = conf else {
        return true;
    };
    conf.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .all(|l| {
            let word = l.split_whitespace().next().unwrap_or("");
            !word.eq_ignore_ascii_case("plugin") && !word.eq_ignore_ascii_case("path")
        })
}

/// The first line; a different one is a format this build does not read.
const HEADER: &str = "yaadgaar sudo attestation v1";

/// The line that records the user's group set (`getgroups` and the
/// primary gid, sorted): the hook's own set must be exactly this.
const GIDS: &str = "user gids:";

/// The attestation that relaxes: SAFE, the user's group set *gids*, and the
/// fingerprints of *inputs* as they are now. `Err` when an input cannot be
/// fingerprinted.
pub fn safe_text(inputs: &[&Path], gids: &[u32]) -> Result<String, String> {
    let mut set = gids.to_vec();
    set.sort_unstable();
    set.dedup();
    let listed: Vec<String> = set.iter().map(u32::to_string).collect();
    let mut text = format!("{HEADER}\nverdict: SAFE\n{GIDS} {}\n", listed.join(" "));
    for input in inputs {
        fingerprint(input, &mut text)?;
    }
    Ok(text)
}

/// The attestation that relaxes nothing, and why.
pub fn unsafe_text(why: &str) -> String {
    format!("{HEADER}\nverdict: NOT SAFE ({why})\n")
}

/// Whether the attestation beside *location*'s exceptions file verifies and
/// says SAFE for *inputs* as they are now and for the group set *gids*.
/// Anything else is `false`.
pub fn holds(location: &Location, inputs: &[&Path], gids: &[u32]) -> bool {
    if inputs.is_empty() {
        return false;
    }
    let attestation = Location {
        file: ATTESTATION_FILE.to_string(),
        ..location.clone()
    };
    let Verified::Valid(text) = super::verify(&attestation) else {
        return false;
    };
    safe_text(inputs, gids).is_ok_and(|now| now == text)
}

/// This process's group set, as the kernel reports it in
/// `/proc/self/status`: the supplementary groups (`getgroups`) and the
/// effective gid. `None` when it cannot be read, which is NOT SAFE: off
/// Linux there is no `/proc`, and no relaxation.
pub fn current_gids() -> Option<Vec<u32>> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    gids_from_status(&status)
}

/// [`current_gids`] from `/proc/self/status` text *status*. PURE.
pub fn gids_from_status(status: &str) -> Option<Vec<u32>> {
    let field = |key: &str| status.lines().find_map(|l| l.strip_prefix(key));
    // `Gid:` real, effective, saved, filesystem.
    let egid = field("Gid:")?.split_whitespace().nth(1)?.parse().ok()?;
    let mut gids = vec![egid];
    for g in field("Groups:")?.split_whitespace() {
        gids.push(g.parse().ok()?);
    }
    Some(gids)
}

/// The group set of uid *uid* by the account databases *passwd* and
/// *group*: its primary gid and every group that lists it. `None` when
/// *passwd* does not list *uid* exactly once. PURE.
pub fn gids_of(passwd: &str, group: &str, uid: u32) -> Option<Vec<u32>> {
    let rows: Vec<Vec<&str>> = passwd
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .filter(|f| f.get(2).and_then(|u| u.parse::<u32>().ok()) == Some(uid))
        .collect();
    let [row] = rows.as_slice() else {
        return None;
    };
    let name = row[0];
    let mut gids = vec![row.get(3)?.parse::<u32>().ok()?];
    for g in group.lines().map(|l| l.split(':').collect::<Vec<_>>()) {
        let member = g
            .get(3)
            .is_some_and(|m| m.split(',').any(|x| x.trim() == name));
        if member {
            gids.push(g.get(2)?.parse::<u32>().ok()?);
        }
    }
    Some(gids)
}

/// Whether nsswitch text *conf* (`None`: no file, the C library's default
/// of files) has sudo read its rules, and the C library read accounts and
/// groups, from files only. Any other source (`sss`, `ldap`, `systemd`)
/// can change them without changing a fingerprinted file.
///
/// The keys are matched as sudo matches them: ignoring case, with optional
/// whitespace before the colon. A line whose first word looks like the
/// sudoers key but does not parse exactly is NOT files-only (fail closed).
pub fn nsswitch_files_only(conf: Option<&str>) -> bool {
    let Some(conf) = conf else {
        return true;
    };
    let files_only = |sources: &str| {
        !sources.trim().is_empty() && sources.split_whitespace().all(|s| s == "files")
    };
    conf.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .all(|line| {
            let first = line
                .split(|c: char| c.is_whitespace() || c == ':')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let Some((key, sources)) = line.split_once(':') else {
                return !first.contains("sudoers");
            };
            let key = key.trim().to_ascii_lowercase();
            if matches!(key.as_str(), "sudoers" | "passwd" | "group") {
                return files_only(sources);
            }
            !first.contains("sudoers")
        })
}

/// Append *path*'s fingerprint to *out*, and one per entry if it is a
/// directory (one level, sorted). Absent is a fingerprint; any other error
/// is `Err`.
fn fingerprint(path: &Path, out: &mut String) -> Result<(), String> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out.push_str(&format!("{} absent\n", path.display()));
            return Ok(());
        }
        Err(e) => return Err(format!("cannot stat {}: {e}", path.display())),
    };
    let real = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    out.push_str(&line(path, &real, &meta));
    if meta.is_dir() {
        let mut entries = Vec::new();
        for entry in
            std::fs::read_dir(path).map_err(|e| format!("cannot list {}: {e}", path.display()))?
        {
            entries.push(
                entry
                    .map_err(|e| format!("cannot list {}: {e}", path.display()))?
                    .path(),
            );
        }
        entries.sort();
        for entry in entries {
            let meta = std::fs::metadata(&entry)
                .map_err(|e| format!("cannot stat {}: {e}", entry.display()))?;
            let real = std::fs::canonicalize(&entry)
                .map_err(|e| format!("cannot resolve {}: {e}", entry.display()))?;
            out.push_str(&line(&entry, &real, &meta));
        }
    }
    Ok(())
}

fn line(path: &Path, real: &Path, m: &std::fs::Metadata) -> String {
    format!(
        "{} -> {} dev={} ino={} mode={:o} uid={} gid={} size={} mtime={}.{} ctime={}.{}\n",
        path.display(),
        real.display(),
        m.dev(),
        m.ino(),
        m.mode(),
        m.uid(),
        m.gid(),
        m.size(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec()
    )
}

#[cfg(test)]
mod tests;

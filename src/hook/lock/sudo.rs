//! Whether an agent could become root without a person, which is what
//! protected mode actually rests on (ADR-1028).
//!
//! The protected file is root-owned, so the only agent write left is one
//! made as root. The guard refuses the escalation commands it can SEE
//! (`guard/escalate.rs`), but a text check is not the defence: a cached
//! sudo timestamp an agent can reuse, a rule that asks no password, or a
//! group that is root by another name, is the hole. `guard status` and
//! `guard lock` read the configuration and say so loudly.
//!
//! **FAIL CLOSED.** The verdict says OK only when the parse is complete
//! and unambiguous. An include the guard did not read, a scoped `Defaults`
//! (`Defaults:user`, `@host`, `>runas`, `!cmd`), or a timestamp option it
//! does not model is UNKNOWN, which `guard status` reports as DEGRADED. A
//! repeated option takes its LAST value, as sudo does.

/// Where sudo and doas keep their rules, and the account databases.
const SUDOERS: &str = "/etc/sudoers"; // ADR-0569-EXCEPTION(CB): sudo's own fixed path, read-only.
const SUDOERS_D: &str = "/etc/sudoers.d"; // ADR-0569-EXCEPTION(CB): sudo's own fixed path, read-only.
const DOAS_CONF: &str = "/etc/doas.conf"; // ADR-0569-EXCEPTION(CB): doas's own fixed path, read-only.
const PASSWD: &str = "/etc/passwd"; // ADR-0569-EXCEPTION(CB): the system account database, read-only.
const GROUP: &str = "/etc/group"; // ADR-0569-EXCEPTION(CB): the system group database, read-only.

/// The closing line every verdict carries.
pub const ONLY_AS_STRONG: &str = "protected mode is only as strong as these settings: an agent \
     that can run sudo or doas without a person can rewrite the root-owned file (ADR-1028).";

/// Groups that are root by another name, and why.
const ROOT_GROUPS: &[(&str, &str)] = &[
    (
        "docker",
        "runs containers as root that can mount the host's /",
    ),
    ("lxd", "starts privileged containers with host mounts"),
    ("incus", "starts privileged containers with host mounts"),
    (
        "incus-admin",
        "starts privileged containers with host mounts",
    ),
    ("libvirt", "qemu:///system can attach host disks to a VM"),
    ("libvirtd", "qemu:///system can attach host disks to a VM"),
    (
        "disk",
        "reads and writes raw block devices, the root filesystem included",
    ),
];

/// The verdict lines for *sudoers* (the text of `/etc/sudoers` and the
/// files under `/etc/sudoers.d`; `None` when they cannot be read) and
/// *doas* (`/etc/doas.conf`, `None` when absent or unreadable). PURE.
pub fn verdict(sudoers: Option<&str>, doas: Option<&str>) -> Vec<String> {
    let mut lines = vec![sudo_verdict(sudoers)];
    if let Some(conf) = doas {
        lines.extend(doas_verdict(conf));
    }
    lines.push(ONLY_AS_STRONG.into());
    lines
}

fn unknown(why: &str) -> String {
    format!("sudo: UNKNOWN — {why}; protected mode is reported DEGRADED until this is clear")
}

/// What the `Defaults` lines set, last value winning.
#[derive(Default)]
struct Settings {
    timeout: Option<String>,
    kind: Option<String>,
    no_auth: bool,
}

fn sudo_verdict(sudoers: Option<&str>) -> String {
    let Some(text) = sudoers else {
        return unknown(
            "the sudoers files are not readable by this user (run `sudo yaadgaar guard status`)",
        );
    };
    let mut rules: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let words: Vec<&str> = line.split_whitespace().collect();
        if matches!(words[0], "@include" | "#include") {
            return unknown(&format!("an include the guard did not read ({line})"));
        }
        if matches!(words[0], "@includedir" | "#includedir") {
            if words.get(1) != Some(&SUDOERS_D) {
                return unknown(&format!("an include the guard did not read ({line})"));
            }
            continue;
        }
        if !line.starts_with('#') {
            rules.push(words.join(" "));
        }
    }
    let mut set = Settings::default();
    for rule in rules.iter().filter(|r| r.starts_with("Defaults")) {
        let rest = &rule["Defaults".len()..];
        if rest.starts_with([':', '@', '>', '!']) {
            return unknown(&format!(
                "a scoped `Defaults` the guard does not model ({rule})"
            ));
        }
        for opt in rest.split(',').map(|o| o.replace(' ', "")) {
            if let Err(why) = apply(&mut set, &opt) {
                return unknown(&why);
            }
        }
    }
    if rules.iter().any(|l| l.contains("NOPASSWD")) {
        return "sudo: WARNING — a NOPASSWD rule lets a process running as you, an agent \
                included, run commands as root with no password"
            .into();
    }
    if set.no_auth
        || rules
            .iter()
            .any(|l| !l.starts_with("Defaults") && l.contains("!authenticate"))
    {
        return "sudo: WARNING — `!authenticate`: sudo asks no password at all".into();
    }
    timestamp_verdict(&set)
}

/// Fold one `Defaults` option into *set*; `Err` for a timestamp option the
/// guard does not model.
fn apply(set: &mut Settings, opt: &str) -> Result<(), String> {
    if let Some(v) = opt.strip_prefix("timestamp_timeout=") {
        set.timeout = Some(v.to_string());
    } else if let Some(v) = opt.strip_prefix("timestamp_type=") {
        set.kind = Some(v.to_string());
    } else if opt == "!tty_tickets" {
        set.kind = Some("global".into());
    } else if opt == "tty_tickets" {
        set.kind = Some("tty".into());
    } else if opt == "!authenticate" {
        set.no_auth = true;
    } else if opt == "authenticate" {
        set.no_auth = false;
    } else if opt.contains("timestamp") || opt.contains("tty_tickets") {
        return Err(format!(
            "a timestamp option the guard does not model ({opt})"
        ));
    }
    Ok(())
}

fn timestamp_verdict(set: &Settings) -> String {
    let zero = set
        .timeout
        .as_deref()
        .and_then(|t| t.parse::<f64>().ok())
        .is_some_and(|t| t == 0.0);
    if zero {
        return "sudo: OK — `Defaults timestamp_timeout=0`: every sudo asks for a password".into();
    }
    match set.kind.as_deref() {
        Some("global") => "sudo: WARNING — `timestamp_type=global`: one sudo anywhere is \
             reusable by every process of yours, an agent included; set `Defaults \
             timestamp_timeout=0`"
            .into(),
        Some("tty") => "sudo: CAUTION — `timestamp_type=tty`: a sudo run in a terminal is \
             reusable for the timestamp timeout by anything attached to that terminal, an \
             agent harness started there included; set `Defaults timestamp_timeout=0` to close it"
            .into(),
        _ => "sudo: WARNING — neither `Defaults timestamp_type=tty` nor `Defaults \
             timestamp_timeout=0` is set, so an agent may reuse a sudo you ran; set `Defaults \
             timestamp_timeout=0`"
            .into(),
    }
}

fn doas_verdict(conf: &str) -> Vec<String> {
    let rules: Vec<&str> = conf
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .collect();
    let has = |w: &str| rules.iter().any(|l| l.split_whitespace().any(|x| x == w));
    let mut out = Vec::new();
    if has("nopass") {
        out.push("doas: WARNING — a `nopass` rule runs commands as root with no password".into());
    }
    if has("persist") {
        out.push(
            "doas: WARNING — `persist` lets a recent doas be reused without a password".into(),
        );
    }
    out
}

/// The root-equivalent groups uid *uid* is in, by *passwd* (its primary
/// group) and *group* (its supplementary ones), each as a WARNING. PURE.
pub fn group_warnings(passwd: &str, group: &str, uid: u32) -> Vec<String> {
    let user = passwd
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.get(2).and_then(|u| u.parse::<u32>().ok()) == Some(uid));
    let Some(user) = user else {
        return Vec::new();
    };
    let (name, primary) = (user[0], user.get(3).copied().unwrap_or(""));
    group
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .filter(|g| {
            g.get(2) == Some(&primary)
                || g.get(3)
                    .is_some_and(|m| m.split(',').any(|x| x.trim() == name))
        })
        .filter_map(|g| ROOT_GROUPS.iter().find(|(n, _)| *n == g[0]))
        .map(|(n, why)| {
            format!("groups: WARNING — member of `{n}`, which {why}: root by another name")
        })
        .collect()
}

/// WARNINGs from `sudo -l -U <user>` output, which sudo itself resolved:
/// what a parse of the files may have missed. PURE.
pub fn sudo_list_warnings(output: &str) -> Vec<String> {
    let mut out = Vec::new();
    if output.contains("NOPASSWD") {
        out.push("sudo -l: WARNING — sudo lists a NOPASSWD rule for this user".into());
    }
    if output.contains("!authenticate") {
        out.push("sudo -l: WARNING — sudo lists `!authenticate` for this user".into());
    }
    out
}

/// What `guard status` prints in place of `mode: protected` when the
/// verdict is not safe for agents, and the exact fix.
pub const FIX: &str = "fix: add `Defaults timestamp_timeout=0` with `visudo -f \
     /etc/sudoers.d/yaadgaar` (on NixOS, `security.sudo.extraConfig`), remove NOPASSWD and \
     `!authenticate` rules and doas `nopass`/`persist` for this user, and leave \
     root-equivalent groups (docker, lxd, incus, libvirt, disk). `timestamp_type=tty` is not \
     enough: an agent harness runs in the terminal that holds the ticket.";

/// Whether *verdict* is safe for agents. `Err` says why not.
///
/// ONLY `timestamp_timeout=0` IS SAFE (review of #94): with
/// `timestamp_type=tty` an agent harness runs in the very terminal that
/// holds the ticket, so the CAUTION verdict degrades too.
pub fn safe(verdict: &[String]) -> Result<(), &'static str> {
    let sudo = verdict.first().map_or("", String::as_str);
    let unknown = sudo.starts_with("sudo: UNKNOWN")
        || verdict.iter().any(|l| l.starts_with("sudo -l: UNKNOWN"));
    if unknown {
        return Err("sudo settings not verified");
    }
    let warned = verdict.iter().any(|l| {
        ["doas: WARNING", "groups: WARNING", "sudo -l: WARNING"]
            .iter()
            .any(|p| l.starts_with(p))
    });
    if !sudo.starts_with("sudo: OK") || warned {
        return Err("sudo reusable");
    }
    Ok(())
}

/// The status headline for protected mode under *verdict*.
pub fn protected_headline(verdict: &[String]) -> Vec<String> {
    match safe(verdict) {
        Ok(()) => vec!["mode: protected".into()],
        Err(why) => vec![format!("mode: PROTECTED (DEGRADED: {why})"), FIX.into()],
    }
}

/// [`verdict`] for this machine and the user *uid* (the one the guard
/// protects against), with its root-equivalent groups and, when this runs
/// as root, what `sudo -l -U` resolves. Read-only.
pub fn this_machine(uid: Option<u32>) -> Vec<String> {
    let doas = std::fs::read_to_string(DOAS_CONF).ok();
    let sudoers = read_sudoers_from(
        std::path::Path::new(SUDOERS),
        std::path::Path::new(SUDOERS_D),
    );
    let mut lines = verdict(sudoers.as_deref(), doas.as_deref());
    let closing = lines.pop();
    if let Some(uid) = uid {
        let passwd = std::fs::read_to_string(PASSWD).unwrap_or_default();
        let group = std::fs::read_to_string(GROUP).unwrap_or_default();
        lines.extend(group_warnings(&passwd, &group, uid));
        #[cfg(unix)]
        if super::super::mode::effective_uid() == Some(0) {
            lines.extend(sudo_list(&passwd, uid));
        }
    }
    lines.extend(closing);
    lines
}

/// Where `sudo` is looked for: FIXED ABSOLUTE PATHS, never the inherited
/// `PATH`, which root inherits from the user who ran `sudo yaadgaar …`.
pub const SUDO_CANDIDATES: &[&str] = &["/run/wrappers/bin/sudo", "/usr/bin/sudo", "/bin/sudo"]; // ADR-0569-EXCEPTION(CB): fixed system paths, so PATH cannot choose the binary root runs.

/// The first of *candidates* that *verified* accepts, as the path it
/// returns; else the UNKNOWN line to report. PURE given *verified*.
pub fn pick_sudo(
    candidates: &[&str],
    verified: impl Fn(&str) -> Result<std::path::PathBuf, String>,
) -> Result<std::path::PathBuf, String> {
    let mut why = Vec::new();
    for c in candidates {
        match verified(c) {
            Ok(path) => return Ok(path),
            Err(e) => why.push(format!("{c}: {e}")),
        }
    }
    Err(format!(
        "sudo -l: UNKNOWN — no root-owned sudo at a fixed path verifies ({})",
        why.join("; ")
    ))
}

/// A candidate's real path, if it and every directory above it are
/// root-owned and not group- or other-writable. The symlink NixOS puts at
/// `/run/wrappers/bin` is resolved first; what is run is the resolved path.
#[cfg(unix)]
fn verified_sudo(candidate: &str) -> Result<std::path::PathBuf, String> {
    let real = std::fs::canonicalize(candidate).map_err(|e| e.to_string())?;
    super::super::mode::verify::owned_path(&real, super::super::mode::TRUSTED_UID, false)?;
    Ok(real)
}

/// `sudo -l -U <user>` as root, read-only, through a verified binary. No
/// verified binary is UNKNOWN; a run that fails adds nothing.
#[cfg(unix)]
fn sudo_list(passwd: &str, uid: u32) -> Vec<String> {
    let name = passwd
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.get(2).and_then(|u| u.parse::<u32>().ok()) == Some(uid))
        .map(|f| f[0].to_string());
    let Some(name) = name else {
        return Vec::new();
    };
    let sudo = match pick_sudo(SUDO_CANDIDATES, verified_sudo) {
        Ok(path) => path,
        Err(unknown) => return vec![unknown],
    };
    std::process::Command::new(sudo)
        .args(["-n", "-l", "-U", &name])
        .env_clear()
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| sudo_list_warnings(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// *main* and the files sudo reads under *dir*, or `None` if any of them
/// cannot be read: a verdict on part of the rules would be a guess. Only a
/// *dir* that does not exist is skipped; any other error is `None`.
pub fn read_sudoers_from(main: &std::path::Path, dir: &std::path::Path) -> Option<String> {
    let mut all = std::fs::read_to_string(main).ok()?;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(all),
        Err(_) => return None,
    };
    let mut files = Vec::new();
    for entry in entries {
        files.push(entry.ok()?.path());
    }
    files.sort();
    for file in files {
        // sudo skips names holding a `.` or ending in `~`.
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned());
        if name.is_some_and(|n| n.contains('.') || n.ends_with('~')) {
            continue;
        }
        all.push('\n');
        all.push_str(&std::fs::read_to_string(&file).ok()?);
    }
    Some(all)
}

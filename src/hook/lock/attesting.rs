//! The sudo attestation `guard lock` writes (`hook/mode/attest.rs`;
//! ADR-1028, ruling B): what it is SAFE for, decided as root.

use std::path::{Path, PathBuf};

use super::sudo;
use crate::hook::mode::attest;

/// What the attestation depends on besides the sudo verdict, read by
/// `lock` as root. `Ok(None)`: no such file; `Err`: unreadable.
pub struct Facts<'a> {
    pub nsswitch: Result<Option<&'a str>, String>,
    pub sudo_conf: Result<Option<&'a str>, String>,
    /// The polkit escalators present (`attest::polkit_escalators`).
    pub polkit_escalators: Vec<PathBuf>,
    /// polkit's daemon, D-Bus activation file or process, present
    /// (`attest::polkit_present`).
    pub polkit_service: Vec<String>,
    /// Whether the PAM stack of every sudo service, and of su and doas
    /// when installed, asks a password (`lock/pam.rs`).
    pub pam: Result<(), String>,
    /// The user's group set by the account databases (`attest::gids_of`);
    /// `None` when it cannot be told.
    pub gids: Option<&'a [u32]>,
}

/// [`attestation`] for this machine, read as root, for the user whose
/// group set is *gids*.
pub fn for_this_machine(verdict: &[String], inputs: &[&Path], gids: Option<&[u32]>) -> String {
    let read = |path: &str| match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {path}: {e}")),
    };
    let (nsswitch, sudo_conf) = (read("/etc/nsswitch.conf"), read("/etc/sudo.conf"));
    fn paths(list: &'static [&'static str]) -> Vec<&'static Path> {
        list.iter().map(Path::new).collect()
    }
    let facts = Facts {
        nsswitch: nsswitch
            .as_ref()
            .map(Option::as_deref)
            .map_err(Clone::clone),
        sudo_conf: sudo_conf
            .as_ref()
            .map(Option::as_deref)
            .map_err(Clone::clone),
        polkit_escalators: attest::polkit_escalators(&paths(attest::POLKIT_DIRS)),
        polkit_service: attest::polkit_present(&paths(attest::POLKIT_SERVICE), Path::new("/proc")),
        pam: super::pam::escalators_ask(
            &super::pam::read_service,
            &|binary| {
                attest::POLKIT_DIRS
                    .iter()
                    .any(|dir| crate::hook::mode::present(&Path::new(dir).join(binary)))
            },
            &super::pam::list_services,
        ),
        gids,
    };
    attestation(verdict, &facts, inputs)
}

/// The sudo attestation `lock` writes for *verdict* (`sudo::this_machine`),
/// *facts* and *inputs* (`mode/attest.rs`). SAFE only when the verdict is,
/// root-equivalent groups included ([`sudo::safe`]); sudo reads its rules,
/// and the C library its accounts and groups, from files only, through the
/// policy the verdict models, behind PAM stacks that ask a password; polkit
/// is absent: no escalator, no daemon, no D-Bus activation file, no
/// process; and the user's group set is known, so the hook can match it.
pub fn attestation(verdict: &[String], facts: &Facts, inputs: &[&Path]) -> String {
    use attest::{nsswitch_files_only, safe_text, sudo_conf_modelled, unsafe_text};
    if let Err(why) = sudo::safe(verdict) {
        return unsafe_text(why);
    }
    match &facts.nsswitch {
        Err(e) => return unsafe_text(e),
        Ok(conf) if !nsswitch_files_only(*conf) => {
            return unsafe_text("nsswitch names a sudoers, passwd or group source other than files")
        }
        Ok(_) => {}
    }
    match &facts.sudo_conf {
        Err(e) => return unsafe_text(e),
        Ok(conf) if !sudo_conf_modelled(*conf) => {
            return unsafe_text("/etc/sudo.conf sets a Plugin or Path the verdict does not model")
        }
        Ok(_) => {}
    }
    if !facts.polkit_escalators.is_empty() {
        let found: Vec<String> = facts
            .polkit_escalators
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        return unsafe_text(&format!(
            "polkit escalators present, whose kept authorisations the guard cannot rule out: {}",
            found.join(", ")
        ));
    }
    if !facts.polkit_service.is_empty() {
        return unsafe_text(&format!(
            "polkit is reachable over D-Bus, whose kept authorisations the guard cannot rule \
             out: {}",
            facts.polkit_service.join(", ")
        ));
    }
    if let Err(why) = &facts.pam {
        return unsafe_text(why);
    }
    let Some(gids) = facts.gids else {
        return unsafe_text("the user's groups cannot be told from /etc/passwd and /etc/group");
    };
    safe_text(inputs, gids).unwrap_or_else(|why| unsafe_text(&why))
}

/// What `lock` says about attestation *text*, which the hook's own check
/// found to hold, or not, as *holds* says.
pub fn attestation_line(text: &str, holds: bool) -> String {
    let safe = text.lines().nth(1) == Some("verdict: SAFE");
    if safe && !holds {
        return "sudo attestation: written SAFE but it does not verify for the hook (an input \
                changed while it was written, or it is too large); protected mode keeps its \
                strict rules. Run `sudo yaadgaar guard lock` again."
            .into();
    }
    if safe {
        "sudo attestation: SAFE — protected mode refuses only escalator names and the protected \
         path; a change to sudo, doas, nsswitch, PAM, polkit or group settings, or to your \
         groups, restores the strict rules until the next `sudo yaadgaar guard lock`"
            .into()
    } else {
        format!(
            "sudo attestation: {} — protected mode keeps its strict rules",
            text.lines().nth(1).unwrap_or("verdict: NOT SAFE")
        )
    }
}

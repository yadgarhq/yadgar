//! `yaadgaar guard status` and `yaadgaar guard lock` (ADR-1028, #434).
//!
//! `lock` is the one step that needs root, run once: it creates the
//! protected directory root:root 0755 and installs the user's exceptions
//! there root:root 0644, then checks the result with the guard's own
//! verification. It COPIES: the user's file is left exactly as it was.
//!
//! **Root reads a path the user controls**, so the source is treated as
//! hostile: whose it is comes from `SUDO_USER` and `SUDO_UID` and must agree
//! with the password database, never from `$HOME`; it is opened without
//! following a symlink, must be a regular file that user owns, and must be a
//! usable decision. Otherwise nothing is written. And because an agent can
//! write that copy, nothing is installed unseen (`lock/review.rs`).

use std::path::PathBuf;

use super::mode::Location;

/// What `lock` prints when the protected file is already the user's.
pub const NOTHING: &str = "nothing to change — the protected exceptions file is already in place";

/// What `lock` installs when the user has no exceptions file: a decision
/// that switches nothing off.
const EMPTY: &str = "{\"disabled_guards\": []}\n";

/// The owner `lock` installs as: root in production, the test's own uid
/// and gid in a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
}

/// The `yaadgaar guard` verbs.
#[derive(Debug, clap::Subcommand)]
pub enum GuardCommand {
    /// Print the guard's mode (default, protected or invalid) and why. It
    /// prints no exception and changes nothing.
    Status,
    /// Install your exceptions as the root-owned protected file. The one
    /// step that needs root: run `sudo yaadgaar guard lock` at a terminal.
    /// It prints the exact content and every guard it switches off, and
    /// asks before it writes.
    Lock {
        /// Go on without asking, ONLY when the content is what the protected
        /// file already holds or switches no guard off; anything else needs
        /// a person at a terminal.
        #[arg(long)]
        yes: bool,
    },
}

/// Run one `yaadgaar guard` verb.
pub fn run(command: GuardCommand) -> anyhow::Result<()> {
    let lines = match command {
        GuardCommand::Status => status(),
        GuardCommand::Lock { yes } => lock_system(yes)?,
    };
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

/// The mode, and whether the guard binary itself is protected.
fn status() -> Vec<String> {
    let mut location = Location::system();
    // UNDER SUDO, judged for the user who ran it: the guard never runs as
    // root, and root is the one uid the check refuses.
    if let Some(loc) = location.as_mut().filter(|l| l.current_uid == Some(0)) {
        let sudo_uid = std::env::var("SUDO_UID").ok().and_then(|u| u.parse().ok());
        loc.current_uid = sudo_uid.filter(|u| *u != 0).or(loc.current_uid);
    }
    let user = dirs::home_dir().map(|h| crate::install::Layout::new(&h).exceptions());
    let loaded = super::mode::load(location.as_ref(), user.as_deref());
    let mut lines = status_lines(
        &loaded,
        location.as_ref(),
        binary_line(),
        sudo::this_machine(location.as_ref().and_then(|l| l.current_uid)),
    );
    if loaded.mode == super::mode::Mode::Protected {
        // Judged for THIS process's groups: run as the user, not with sudo.
        let held = location
            .as_ref()
            .is_some_and(|l| super::mode::sudo_attested(l, None, None));
        lines.push(relaxed_line(held));
    }
    lines
}

/// What `guard status` says about the relaxed rules, given whether a SAFE
/// attestation holds. PURE.
pub fn relaxed_line(held: bool) -> String {
    if held {
        "relaxed rules: ON — the sudo attestation verifies and matches today's settings".into()
    } else {
        "relaxed rules: OFF — no verified SAFE sudo attestation matches today's settings and \
         your groups (run `sudo yaadgaar guard lock` to refresh it)"
            .into()
    }
}

/// What `guard status` prints, given what it found. PURE.
///
/// PROTECTED ONLY IF SUDO IS SAFE FOR AGENTS: the root-owned file holds
/// only while no agent can become root without a person.
pub fn status_lines(
    loaded: &super::mode::Loaded,
    location: Option<&Location>,
    binary: String,
    verdict: Vec<String>,
) -> Vec<String> {
    let mut lines = super::mode::status_lines(loaded, location);
    if loaded.mode == super::mode::Mode::Protected {
        lines.splice(0..1, sudo::protected_headline(&verdict));
    }
    lines.push(binary);
    lines.extend(verdict);
    lines
}

/// Protected mode is only as strong as the binary that enforces it: one the
/// user can write is one an agent can replace (ADR-1028).
fn binary_line() -> String {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize());
    #[cfg(unix)]
    let judged = exe
        .map_err(|e| e.to_string())
        // STICKY ANCESTORS ALLOWED, for this check only: `/nix/store` is
        // 1775 root:nixbld, and its entries cannot be replaced by others.
        .and_then(|p| {
            super::mode::verify::owned_path(&p, super::mode::TRUSTED_UID, true).map(|()| p)
        });
    #[cfg(not(unix))]
    let judged: Result<PathBuf, String> = exe
        .map_err(|e| e.to_string())
        .and_then(|_| Err("no ownership check on this platform".into()));
    match judged {
        Ok(path) => format!(
            "guard binary: {} is root-owned and not writable by others",
            path.display()
        ),
        Err(why) => format!(
            "guard binary: NOT protected ({why}); an agent running as this user can replace \
             the guard itself. Protected mode needs a root-owned binary and a system-wide \
             hook registration (ADR-1028)."
        ),
    }
}

/// Who `lock` installs for: `(name, uid)` from `SUDO_USER` and `SUDO_UID`.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn preconditions(
    euid: Option<u32>,
    user: Option<String>,
    uid: Option<String>,
) -> Result<(String, u32), String> {
    if euid != Some(0) {
        return Err(
            "`yaadgaar guard lock` must run as root: run `sudo yaadgaar guard lock`".into(),
        );
    }
    let ambiguous = |what: &str| {
        format!(
            "{what}, so whose exceptions to install is ambiguous; run `sudo yaadgaar guard lock` \
             from that user's own account (SUDO_USER and SUDO_UID must name it)"
        )
    };
    let user = user
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ambiguous("SUDO_USER is not set"))?;
    let uid: u32 = uid
        .and_then(|u| u.parse().ok())
        .ok_or_else(|| ambiguous("SUDO_UID is not set or not a number"))?;
    if uid == 0 || user == "root" {
        return Err(ambiguous("SUDO_USER or SUDO_UID is root"));
    }
    Ok((user, uid))
}

/// *name*'s home in the password database text *passwd*, which must list
/// that name exactly once, with *uid*, and an absolute home.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn passwd_home(passwd: &str, name: &str, uid: u32) -> Result<PathBuf, String> {
    let rows: Vec<Vec<&str>> = passwd
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .filter(|f| f.first() == Some(&name))
        .collect();
    let [row] = rows.as_slice() else {
        return Err(format!(
            "/etc/passwd lists {name} {} times, not once",
            rows.len()
        ));
    };
    if row.get(2).and_then(|u| u.parse::<u32>().ok()) != Some(uid) {
        return Err(format!(
            "/etc/passwd gives {name} a uid other than {uid} (SUDO_UID)"
        ));
    }
    match row.get(5).map(PathBuf::from) {
        Some(home) if home.is_absolute() => Ok(home),
        _ => Err(format!("/etc/passwd gives {name} no absolute home")),
    }
}

#[cfg(unix)]
fn lock_system(yes: bool) -> anyhow::Result<Vec<String>> {
    let (name, uid) = preconditions(
        super::mode::effective_uid(),
        std::env::var("SUDO_USER").ok(),
        std::env::var("SUDO_UID").ok(),
    )
    .map_err(anyhow::Error::msg)?;
    let passwd = std::fs::read_to_string("/etc/passwd")?;
    let home = passwd_home(&passwd, &name, uid).map_err(anyhow::Error::msg)?;
    let source = crate::install::Layout::new(&home).exceptions();
    let location = Location::system().ok_or_else(|| anyhow::anyhow!("no protected mode here"))?;
    let owner = Owner {
        uid: super::mode::TRUSTED_UID,
        gid: 0,
    };
    let plan = install::plan(&location, &source, uid).map_err(anyhow::Error::msg)?;
    // NOTHING IS INSTALLED UNSEEN (`lock/review.rs`): the source is a file an
    // agent could have written.
    let target = location.path().display().to_string();
    let verdict = sudo::this_machine(Some(uid));
    for line in review::review_lines(&plan, &target)
        .into_iter()
        .chain(verdict.iter().cloned())
    {
        println!("{line}");
    }
    use std::io::IsTerminal as _;
    match review::consent(std::io::stdin().is_terminal(), yes, &plan) {
        review::Consent::Proceed => {}
        review::Consent::Refuse(why) => anyhow::bail!("nothing changed: {why}"),
        review::Consent::Ask => {
            use std::io::Write as _;
            print!("install this content? type yes to confirm: ");
            std::io::stdout().flush()?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !review::confirms(&answer) {
                anyhow::bail!("not confirmed; nothing changed");
            }
        }
    }
    let mut lines = install::apply(&location, &plan, owner).map_err(anyhow::Error::msg)?;
    let all = super::mode::attest::inputs();
    let inputs: Vec<&std::path::Path> = all.iter().map(PathBuf::as_path).collect();
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let gids = super::mode::attest::gids_of(&passwd, &group, uid);
    let text = attesting::for_this_machine(&verdict, &inputs, gids.as_deref());
    install::write_attestation(&location, &text, owner).map_err(anyhow::Error::msg)?;
    // JUDGED AS THE HOOK WILL JUDGE IT, for the user and not for root: what
    // was written may still not hold (an input changed meanwhile, or the
    // file is past the read cap).
    let as_hook = Location {
        current_uid: Some(uid),
        ..location.clone()
    };
    let holds = gids
        .as_deref()
        .is_some_and(|g| super::mode::sudo_attested(&as_hook, None, Some(g)));
    lines.push(attestation_line(&text, holds));
    lines.push(format!(
        "{} is left as it was. Protected mode also needs a root-owned guard binary and a \
         system-wide hook registration (ADR-1028): see `yaadgaar guard status`. On NixOS the \
         nix configuration, not this copy, is the source of truth.",
        source.display()
    ));
    Ok(lines)
}

#[cfg(not(unix))]
fn lock_system(_yes: bool) -> anyhow::Result<Vec<String>> {
    anyhow::bail!("protected mode needs a unix system: there is no root-owned path to verify here")
}

#[cfg(unix)]
mod attesting;
#[cfg(unix)]
pub use attesting::attestation_line;
#[cfg(all(unix, test))]
pub use attesting::{attestation, Facts};
#[cfg(unix)]
mod install;
#[cfg(unix)]
mod pam;
#[cfg(all(unix, test))]
pub use install::lock_into;
#[cfg(unix)]
mod review;
mod sudo;

#[cfg(test)]
mod tests;

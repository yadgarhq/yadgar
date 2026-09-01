//! `yadgar install`, `uninstall` and `verify` — the whole client surface (D76).
//!
//! Three files are touched and no others: the agent's user-level MCP config
//! (`~/.claude.json`), its settings (`~/.claude/settings.json`), and a rules
//! file yadgar owns outright, referenced from the first line of `CLAUDE.md`.
//!
//! **Almost every rule in this module is a scar.** The Python installer this
//! replaces shipped a directory of hook scripts; nix's home-manager kept its own
//! copies of the same handlers; the two diverged on `project_id` and the capture
//! pipeline was dead for six days while every signal still read healthy. So:
//! one binary, at one durable path, invoked as `yaadgaar hook <name>` — there is
//! no second copy to diverge from, and no script on disk to rename.
//!
//! The three properties that keep this from breaking somebody's machine:
//!
//! * **Pre-flight, then write.** Everything that can refuse — an ephemeral
//!   binary path, a symlinked `CLAUDE.md`, an unparseable config — refuses
//!   before the first byte is written. A refusal discovered halfway through
//!   leaves a half-install on exactly the machine the refusal exists for.
//! * **Foreign-preserving merges.** Never `hooks[event] = [...]`. Other tools
//!   write into the same keys (on this machine, nix writes a `SessionStart`
//!   entry), and a hard assignment deletes them silently.
//! * **Atomic writes.** A crash mid-install must not truncate a settings file
//!   yadgar does not own.
//!
//! `main.rs` wires the three public functions to the three subcommands.
//! [`verify`] prints its own report and returns `Err` on drift, so the obvious
//! `install::verify(&home)?` exits non-zero without anyone remembering to check
//! a return value — the daemon cannot see `~/.claude/settings.json`, so this is
//! the only thing in the system that can ever notice hook drift.

use std::path::{Path, PathBuf};

mod command;
pub mod health;
pub mod hooks;
mod jsonfile;
mod mcp;
mod rules;
mod shellword;

#[cfg(test)]
mod tests;

pub use command::resolve_durable_command;
pub use health::verify;
pub use hooks::MANAGED_HOOKS;

/// Where each managed file lives, relative to a home directory.
///
/// Every path is derived from one parameter so a test can point the whole
/// install at a temp directory. Nothing here reads `$HOME` on its own: a test
/// that writes into the real `~/.claude` has already failed.
pub struct Layout {
    home: PathBuf,
}

impl Layout {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
        }
    }

    pub fn claude_dir(&self) -> PathBuf {
        self.home.join(".claude")
    }

    /// Hook registrations live here.
    pub fn settings(&self) -> PathBuf {
        self.claude_dir().join("settings.json")
    }

    /// The MCP registration lives in `~/.claude.json`, NOT in `.claude/`.
    ///
    /// User-level, once per machine (D75): `project_id` is derived from the
    /// working directory at call time, so a per-project entry would be N
    /// identical copies of something that never varies — and each copy a place
    /// to forget, so yadgar works in the repositories somebody remembered and is
    /// silently absent from the rest.
    pub fn mcp_config(&self) -> PathBuf {
        self.home.join(".claude.json")
    }

    /// The rules file yadgar owns ENTIRELY. Updating it is a whole-file replace.
    pub fn rules(&self) -> PathBuf {
        self.claude_dir().join(rules::RULES_FILE)
    }

    /// The file yadgar does not own, and writes exactly one line into.
    pub fn claude_md(&self) -> PathBuf {
        self.claude_dir().join("CLAUDE.md")
    }

    /// Never written and never removed: it holds decisions a human made.
    ///
    /// Referenced only by the test that proves an uninstall leaves it alone,
    /// which is the point — no code path here touches it, and the name exists so
    /// that stays deliberate rather than accidental.
    #[allow(dead_code)]
    pub fn exceptions(&self) -> PathBuf {
        self.claude_dir().join("yadgar-hook-exceptions.json")
    }
}

/// What an install or uninstall actually did, for the CLI to print.
#[derive(Debug)]
pub struct Summary {
    pub hooks: usize,
    pub settings: PathBuf,
    pub mcp_config: PathBuf,
    pub rules: PathBuf,
    /// False when the reference line was already the first line of `CLAUDE.md`.
    pub claude_md_changed: bool,
}

/// Install with this binary's own path as the registered command.
pub fn install(home: &Path) -> anyhow::Result<Summary> {
    let binary = resolve_durable_command()?;
    install_with(home, &binary)
}

/// Install, registering *binary* as the command every hook invokes.
///
/// *binary* is assumed already durable — [`resolve_durable_command`] is what
/// enforces that, and it is separated out so a test can register a path that
/// does not exist without the temp-directory rule refusing it.
pub fn install_with(home: &Path, binary: &Path) -> anyhow::Result<Summary> {
    let layout = Layout::new(home);

    // ── Pre-flight ───────────────────────────────────────────────────────────
    //
    // Nothing below this block writes, and nothing above it can. On a
    // nix-managed machine `CLAUDE.md` is a store symlink and this install MUST
    // stop — if the hooks and the MCP entry had already landed, stopping here
    // would leave a half-install rather than an untouched machine.
    rules::check_reference_target(&layout.claude_md())?;
    rules::check_owned(&layout.rules())?;
    let mut settings = jsonfile::load(&layout.settings())?;
    let mut mcp = jsonfile::load(&layout.mcp_config())?;

    // ── Writes ───────────────────────────────────────────────────────────────
    rules::write_body(&layout.rules())?;
    let reference = rules::reference_line(&layout.rules());
    let claude_md_changed = rules::ensure_reference(&layout.claude_md(), &reference)?;

    hooks::merge(&mut settings, binary);
    jsonfile::write_atomic(&layout.settings(), &settings)?;

    mcp::merge(&mut mcp, binary);
    jsonfile::write_atomic(&layout.mcp_config(), &mcp)?;

    Ok(Summary {
        hooks: MANAGED_HOOKS.len(),
        settings: layout.settings(),
        mcp_config: layout.mcp_config(),
        rules: layout.rules(),
        claude_md_changed,
    })
}

/// Remove what [`install`] added, and nothing else.
///
/// The Python uninstall removed the services and left the entire agent
/// environment — hooks, settings entries, rules — all still firing against an
/// install that was gone. This removes the hook entries, the MCP entry, the
/// rules file and the reference line, and touches nothing it did not write.
/// `yadgar-hook-exceptions.json` in particular survives: it holds decisions a
/// human made, and a reinstall must find them still there.
pub fn uninstall(home: &Path) -> anyhow::Result<Summary> {
    let layout = Layout::new(home);

    // NO PRE-FLIGHT ON CLAUDE.md, and the asymmetry with `install` above is the
    // whole point. Install refuses before writing, because it is about to write
    // to a file it does not own. Uninstall must remove everything it CAN and
    // report what it could not at the END: the hooks and the MCP entry come out
    // without `CLAUDE.md` being touched at all, so a problem with that file may
    // not stop them coming out. Bailing first leaves somebody unable to remove
    // yadgar from their machine because of a file yadgar is only trying to tidy.
    //
    // Both refusals were reintroduced here once already, and each time by a
    // check placed at the top. `unwrap_or(false)` fixed the UNREADABLE case and
    // the very next line reinstated it for the SYMLINKED one — on a nix-managed
    // machine, where `~/.claude/CLAUDE.md` is a store symlink, which is the
    // machine this module exists for. Twelve hooks and the MCP entry stayed live.
    //
    // A parse failure in the two JSON configs is different and still refuses
    // first: those are the files being REWRITTEN, and D75 is explicit that a
    // config which cannot be parsed must never be clobbered.
    let mut settings = jsonfile::load(&layout.settings())?;
    let mut mcp = jsonfile::load(&layout.mcp_config())?;

    let removed = hooks::strip(&mut settings);
    jsonfile::write_atomic(&layout.settings(), &settings)?;

    mcp::strip(&mut mcp);
    jsonfile::write_atomic(&layout.mcp_config(), &mcp)?;

    rules::remove_body(&layout.rules())?;

    // LAST, and the only step that needs `CLAUDE.md` — so everything removable
    // is already gone by the time any of this can fail, and re-running after
    // fixing the file finishes the job. The check still guards the write: a
    // symlink is never written through, because replacing it with a regular
    // file would drift the machine from its own declared configuration. It is
    // reported rather than pre-flighted.
    let reference = rules::reference_line(&layout.rules());
    if rules::has_reference(&layout.claude_md(), &reference).unwrap_or(false) {
        rules::check_reference_target(&layout.claude_md())?;
    }
    let claude_md_changed = rules::remove_reference(&layout.claude_md(), &reference)?;

    Ok(Summary {
        hooks: removed,
        settings: layout.settings(),
        mcp_config: layout.mcp_config(),
        rules: layout.rules(),
        claude_md_changed,
    })
}

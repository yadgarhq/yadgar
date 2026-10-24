//! Which standing refusals a human has switched off, read from the exceptions file.
//!
//! The yadgar memory engine and this client both register a `PreToolUse` guard,
//! and both read the same file, `~/.claude/yadgar-hook-exceptions.json`. Before
//! this module the client only PROTECTED that file and never read it, so a guard
//! somebody had switched off in the engine still refused through the client:
//! two guards, one decision file, two different policies.
//!
//! ## Read-only, and the ids are the engine's own
//!
//! Nothing here writes, creates or repairs the file. It holds a decision a human
//! made, and the guard itself refuses any agent that tries to edit it.
//!
//! Only three ids are honoured, spelled exactly as the engine's
//! `pretooluse-router.py` spells them, so the same list switches off the same
//! refusal in both guards with no name map in between:
//!
//! * `terraform_family` — terraform, tofu and tfp by any route, AND a digger
//!   command posted to a forge. The engine files digger-on-PR under this id too;
//!   it has no separate one.
//! * `git_commit_flags` — a commit that skips hooks or signing.
//! * `hook_config_tamper` — an edit of the exceptions file, and any Bash
//!   command that names it, a read included (#64); and an agent write to a
//!   Claude Code settings file or `.git/hooks` (ADR-1005).
//!
//! Every other id — the engine's `git_push_default` and `db_lockdown`, which this
//! client does not implement, a misspelling, a different case — changes nothing.
//!
//! ## Fail closed
//!
//! The opposite direction from every other error in this hook, on purpose. A
//! missing, unreadable or malformed file means EVERY guard stays on: the file can
//! only ever switch a refusal off, so not being able to read it must never be
//! what switches one off. The hook still never crashes; it logs a notice to
//! stderr and refuses or allows exactly as it would with no file at all.

use std::io::Read as _;
use std::path::Path;

use serde_json::Value;

/// The file's name, beside the agent client's own settings in `~/.claude`.
pub const EXCEPTIONS_FILE: &str = "yadgar-hook-exceptions.json";

/// The key holding the list of switched-off guard ids.
const DISABLED_KEY: &str = "disabled_guards";

/// The engine's push allowlist. Not used here, but shape-checked as the engine
/// checks it, so a file the engine ignores is ignored here too.
const PUSH_ALLOWLIST_KEY: &str = "push_default_allowlist";

/// The most of the file that is ever read. A real one is a few hundred bytes.
///
/// A BOUND, not a knob: without it a symlink to `/dev/zero` is a read that
/// never ends. Anything longer is malformed, and malformed means every guard on.
const MAX_BYTES: u64 = 64 * 1024; // ADR-0569-EXCEPTION(CB): bound on a read of an untrusted path, not a per-install knob.

/// Which refusals are ENFORCED. `true` means the guard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub terraform_family: bool,
    pub git_commit_flags: bool,
    pub hook_config_tamper: bool,
}

impl Policy {
    /// Every guard on: the answer whenever the file says nothing usable.
    pub const ALL_ON: Self = Self {
        terraform_family: true,
        git_commit_flags: true,
        hook_config_tamper: true,
    };

    /// The policy a file's TEXT describes, or why the text is not usable.
    ///
    /// PURE, so every malformed shape is a unit test rather than a file on disk.
    /// A file with no `disabled_guards` key at all is a valid file that disables
    /// nothing. A `disabled_guards` that is not a list is malformed: a bare
    /// string is NOT read as a one-element list, because guessing what a
    /// malformed decision meant is how a guess turns a refusal off.
    pub fn parse(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        let Some(root) = root.as_object() else {
            return Err("the top level is not a JSON object".to_string());
        };
        let mut policy = Self::ALL_ON;
        // PARITY WITH THE ENGINE (m-agahi/yadgar#77): a push allowlist of the
        // wrong shape makes the whole file malformed there, so here too.
        if let Some(allow) = root.get(PUSH_ALLOWLIST_KEY) {
            let strings = allow
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string));
            if !strings {
                return Err(format!("`{PUSH_ALLOWLIST_KEY}` is not a list of strings"));
            }
        }
        let Some(disabled) = root.get(DISABLED_KEY) else {
            return Ok(policy);
        };
        let Some(ids) = disabled.as_array() else {
            return Err(format!("`{DISABLED_KEY}` is not a list"));
        };
        // ALL OR NOTHING (ledger 1589): one entry that is not a string makes
        // the whole list malformed, as the engine's router reads it
        // (m-agahi/yadgar#77). Skipping it and honouring the rest would switch
        // off here what the engine, ignoring the file, keeps on.
        let Some(ids) = ids.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
            return Err(format!("`{DISABLED_KEY}` is not a list of strings"));
        };
        // Exact, case-sensitive matches only. Anything else names no refusal
        // this guard makes, and is left for the engine to read.
        for id in ids {
            match id {
                "terraform_family" => policy.terraform_family = false,
                "git_commit_flags" => policy.git_commit_flags = false,
                "hook_config_tamper" => policy.hook_config_tamper = false,
                _ => {}
            }
        }
        Ok(policy)
    }

    /// The policy in the file at *path*; every guard on when it cannot be read.
    ///
    /// *path* is `None` when the home directory itself could not be found, and
    /// that is not a reason to switch anything off either.
    ///
    /// The notice goes to stderr through `tracing`, never to stdout: a hook's
    /// stdout is a decision the agent client parses. A MISSING file is logged
    /// at debug level only — it is the normal state on every machine without
    /// the memory engine, and a warning on every Bash call would be noise. An
    /// unreadable or malformed file is a warning, because somebody wrote a
    /// decision there and it is not being honoured.
    pub fn load(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            tracing::warn!(
                "cannot find the home directory, so {EXCEPTIONS_FILE} was not read; every guard stays on"
            );
            return Self::ALL_ON;
        };
        let text = match read_bounded(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("{} is absent; every guard stays on", path.display());
                return Self::ALL_ON;
            }
            Err(e) => {
                tracing::warn!("cannot read {}: {e}; every guard stays on", path.display());
                return Self::ALL_ON;
            }
        };
        Self::parse(&text).unwrap_or_else(|why| {
            tracing::warn!(
                "{} is not usable ({why}); every guard stays on",
                path.display()
            );
            Self::ALL_ON
        })
    }
}

/// The file's text, if it is a regular file of at most [`MAX_BYTES`].
///
/// **The checks are on the OPEN FILE, not the path.** Opened non-blocking, so a
/// FIFO with no writer does not block the open; then `fstat` on that same
/// handle, so what is judged is what would be read, with no gap between a path
/// check and the open. A FIFO, a device or a socket is refused rather than
/// read: a read of one can block or never end, and Claude Code treats a hook it
/// timed out as no decision at all — every guarded call would then run.
/// `O_NONBLOCK` has no effect on a regular file's reads.
///
/// `O_NOFOLLOW` (ledger 1589): a symlink in the file's place is refused, even
/// to a valid file, because the engine's router opens it the same way
/// (m-agahi/yadgar#77). Two guards reading one decision file must reach one
/// decision, and a symlink both follows would be a second route to the file.
fn read_bounded(path: &Path) -> std::io::Result<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut text = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("longer than {MAX_BYTES} bytes"),
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests;

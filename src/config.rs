//! Where the credential and the gateway address live.
//!
//! **Both, together, as one unit** (D72). A config carrying a token but no
//! address, or an address from one deployment paired with a token from another,
//! fails at connect time rather than at login — and the person has by then
//! forgotten which deployment they logged into.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DIR: &str = "yadgar";
const FILE: &str = "config.json";
const TOOL_CACHE: &str = "tools-cache.json";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    /// Where the gateway is. Asked for at login, because nothing else can supply
    /// it: the agent spawns `yaadgaar serve` with no arguments, so the address has
    /// to come from a file the client already trusts (D72).
    pub gateway: String,
    /// The long-lived credential. Shown once by `iam` at login and stored here;
    /// the server holds only a hash.
    token: String,

    /// Who this credential belongs to — **FOR DISPLAY, NEVER FOR AUTHORITY**.
    ///
    /// It is stored because `POST /auth/enrol` is the only place it is ever
    /// said: a person enrolling on their first machine has no other way to
    /// learn the username the deployment gave them, and without it a later
    /// `login` on a second machine cannot be completed at all.
    ///
    /// **IT IS NEVER SENT.** ADR-0511: the gateway resolves the caller from the
    /// bearer token via `iam.ResolveCredential` and mints the `Scope` itself,
    /// because a self-asserted username is forgeable by anyone holding any
    /// valid token — precisely what ADR-0488 exists to prevent. There is
    /// deliberately no `x-yadgar-user` anywhere in this client, and adding one
    /// would supersede a decision rather than fix a bug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,

    /// This install, as one UUID (ADR-0511).
    ///
    /// Minted ONCE and stored, so it survives restarts, re-logins and a moved
    /// directory. A hostname-and-path hash was rejected for exactly that: it
    /// changes when a directory moves, silently breaking the identity it exists
    /// to provide, and it leaks a path and a hostname on every request. A
    /// per-process value was rejected too — nothing could then correlate a
    /// client across restarts, which rate-limit accounting and audit both need.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instance: Option<String>,

    /// The root CA from the enrolment token, trusted for the gateway ALONE.
    ///
    /// It never touches the system trust store: the mechanism is scoped to one
    /// host, and installing a CA machine-wide risks the whole machine to solve
    /// that. `None` means the deployment uses a publicly-trusted certificate
    /// and system trust applies — a legitimate deployment, not a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ca_pem: Option<String>,

    /// The directory this config was loaded from, and the only one it writes to.
    ///
    /// A FIELD, not a call to [`base_dir`], and that is the difference between a
    /// test and a hazard. The tool cache used to be read and written through the
    /// process-global directory while the config itself came from wherever
    /// [`Config::load_from`] was pointed — so a test that loaded a config from a
    /// temp directory still read and OVERWROTE the real
    /// `~/.config/yadgar/tools-cache.json` on the machine running it. Every
    /// path a `Config` touches now comes from the `Config`.
    ///
    /// Skipped by serde in BOTH directions: it is where the file is, so writing
    /// it into the file would be a second answer to a question the file's own
    /// location already answers, and an older config would deserialise without
    /// it anyway.
    #[serde(skip)]
    dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "not logged in. Run `yaadgaar login` — it asks for the gateway address and \
         your credentials once and stores both. If an admin gave you an enrolment \
         token and you have never set a password, run `yaadgaar enrol <token>` \
         instead: the token already names the gateway, so nothing is typed twice."
    )]
    Missing,
    #[error("cannot read {0}: {1}")]
    Unreadable(PathBuf, std::io::Error),
    #[error("{0} is not valid config: {1}")]
    Malformed(PathBuf, serde_json::Error),
    #[error("cannot write {0}: {1}")]
    Unwritable(PathBuf, std::io::Error),
}

/// The config directory, overridable so tests never touch a real one.
pub fn base_dir() -> PathBuf {
    if let Ok(over) = std::env::var("YADGAR_CONFIG_DIR") {
        return PathBuf::from(over);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DIR)
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&base_dir())
    }

    pub fn load_from(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join(FILE);
        if !path.exists() {
            return Err(ConfigError::Missing);
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| ConfigError::Unreadable(path.clone(), e))?;
        let mut config: Self =
            serde_json::from_str(&text).map_err(|e| ConfigError::Malformed(path, e))?;
        config.dir = dir.to_path_buf();
        Ok(config)
    }

    pub fn new(dir: &Path, gateway: String, token: String) -> Self {
        Self {
            gateway,
            token,
            username: None,
            instance: None,
            ca_pem: None,
            dir: dir.to_path_buf(),
        }
    }

    /// What `POST /auth/enrol` said this person is called. Display only.
    pub fn with_username(mut self, username: Option<String>) -> Self {
        self.username = username;
        self
    }

    /// The CA the enrolment token carried, if it carried one.
    pub fn with_ca_pem(mut self, ca_pem: Option<String>) -> Self {
        self.ca_pem = ca_pem;
        self
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub fn instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    /// Carry an existing install id into a config being rebuilt.
    ///
    /// NOT a builder like `with_username` and `with_ca_pem`, and named `set_`
    /// deliberately: it restores a value that already existed rather than
    /// choosing one. The only caller is `login::reconcile`, which rebuilds the
    /// whole config after a new credential and must not let a re-login reset
    /// the id ADR-0511 requires to be stable.
    pub fn set_instance(&mut self, instance: Option<String>) {
        self.instance = instance;
    }

    pub fn ca_pem(&self) -> Option<&str> {
        self.ca_pem.as_deref()
    }

    /// Mint this install's id if it has none, and say whether one was minted.
    ///
    /// IDEMPOTENT, and that is the whole contract: called from `install`, where
    /// ADR-0511 says the id is minted, AND from `serve`, so a config written by
    /// a client that predates this field acquires one without the person having
    /// to reinstall. Whichever runs first mints; every later call is a no-op and
    /// the value never changes.
    ///
    /// A UUID, and nothing derived from the machine. ADR-0511 rejected a
    /// hostname-and-path hash because it changes when a directory moves — the
    /// one thing an install id must survive — and because it puts a filesystem
    /// path and a hostname on every request.
    pub fn ensure_instance(&mut self) -> Result<bool, ConfigError> {
        if self.instance.is_some() {
            return Ok(false);
        }
        self.instance = Some(uuid::Uuid::new_v4().to_string());
        self.save()?;
        Ok(true)
    }

    /// Write both fields together, atomically, readable only by this user.
    ///
    /// A half-written config is worse than none: it fails later, somewhere else,
    /// with an error about the gateway rather than about the file.
    pub fn save(&self) -> Result<(), ConfigError> {
        let dir = &self.dir;
        std::fs::create_dir_all(dir).map_err(|e| ConfigError::Unwritable(dir.into(), e))?;
        let path = dir.join(FILE);
        let tmp = dir.join(format!(".{FILE}.tmp"));
        let body = serde_json::to_string_pretty(self).expect("config serialises");
        std::fs::write(&tmp, body).map_err(|e| ConfigError::Unwritable(tmp.clone(), e))?;
        restrict(&tmp);
        std::fs::rename(&tmp, &path).map_err(|e| ConfigError::Unwritable(path, e))?;
        Ok(())
    }

    pub fn gateway_url(&self) -> &str {
        &self.gateway
    }

    /// The credential.
    ///
    /// A method rather than a public field, so `grep -n 'token()'` finds every
    /// place it is read. There are two: the proxy attaches it, and login writes
    /// it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The last tool list the gateway returned.
    ///
    /// Cached so the agent can start while the gateway is unreachable (D75). An
    /// empty list would be indistinguishable from yadgar not being installed,
    /// and the agent would silently lose memory and tasks with nothing to report.
    pub fn read_tool_cache(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join(TOOL_CACHE)).ok()
    }

    /// Drop the cached tool list, because it belongs to a different deployment.
    ///
    /// **AN ASSOCIATED FUNCTION taking the DIRECTORY, not a method**, because
    /// the only caller is `login::reconcile` — which holds the OLD config while
    /// building the new one, and so has no single `Config` whose cache this is.
    /// The file is keyed by directory either way.
    ///
    /// The cache is served on an offline start (D75), so left in place across a
    /// change of gateway it is deployment A's tool list presented as B's. Best
    /// effort by design: a cache that will not delete must not fail a login, and
    /// the worst case of deleting one that still mattered is one online fetch.
    pub fn forget_tool_cache(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(TOOL_CACHE));
    }

    /// Best effort. A cache that cannot be written must never fail the request
    /// that produced it — the answer is already correct and already going back.
    pub fn write_tool_cache(&self, body: &str) -> std::io::Result<()> {
        let dir = &self.dir;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{TOOL_CACHE}.tmp"));
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, dir.join(TOOL_CACHE))
    }
}

/// Owner-only. The file holds a bearer credential.
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

/// No-op on Windows, and that is a real gap rather than a shrug.
///
/// There is no chmod; the equivalent is an ACL, which needs a Windows-specific
/// crate this client does not carry. The file lands under the user's profile
/// directory, which is not world-readable by default — so the property holds by
/// where it sits rather than by what this sets, and that is a weaker guarantee.
/// Stated here so nobody later reads the empty function as "nothing to do".
#[cfg(not(unix))]
fn restrict(_path: &Path) {}

#[cfg(test)]
mod tests;

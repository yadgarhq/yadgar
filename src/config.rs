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
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "not logged in. Run `yaadgaar login` — it asks for the gateway address and \
         your credentials once, stores both, and registers the MCP server."
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
        serde_json::from_str(&text).map_err(|e| ConfigError::Malformed(path, e))
    }

    pub fn new(gateway: String, token: String) -> Self {
        Self { gateway, token }
    }

    /// Write both fields together, atomically, readable only by this user.
    ///
    /// A half-written config is worse than none: it fails later, somewhere else,
    /// with an error about the gateway rather than about the file.
    pub fn save_to(&self, dir: &Path) -> Result<(), ConfigError> {
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
        std::fs::read_to_string(base_dir().join(TOOL_CACHE)).ok()
    }

    /// Best effort. A cache that cannot be written must never fail the request
    /// that produced it — the answer is already correct and already going back.
    pub fn write_tool_cache(&self, body: &str) -> std::io::Result<()> {
        let dir = base_dir();
        std::fs::create_dir_all(&dir)?;
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
mod tests {
    use super::*;

    #[test]
    fn address_and_token_round_trip_together() {
        let dir = tempdir();
        Config::new("https://gw.example:18443/".into(), "tok-abc".into())
            .save_to(&dir)
            .unwrap();
        let back = Config::load_from(&dir).unwrap();
        assert_eq!(back.gateway_url(), "https://gw.example:18443/");
        assert_eq!(back.token(), "tok-abc");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_config_says_how_to_fix_it() {
        // The error a person actually meets first. It must name the command, not
        // the filename.
        let err = Config::load_from(Path::new("/nonexistent/yadgar")).unwrap_err();
        assert!(matches!(err, ConfigError::Missing));
        assert!(err.to_string().contains("yaadgaar login"));
    }

    #[test]
    fn malformed_config_is_distinguishable_from_absent() {
        // Absent means "log in"; malformed means "this file is broken". Telling
        // someone to log in when the file is corrupt sends them round a loop
        // that cannot terminate.
        let dir = tempdir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), "{ not json").unwrap();
        assert!(matches!(
            Config::load_from(&dir).unwrap_err(),
            ConfigError::Malformed(..)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_config_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir();
        Config::new("https://gw".into(), "secret".into())
            .save_to(&dir)
            .unwrap();
        let mode = std::fs::metadata(dir.join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "a file holding a bearer token must be owner-only"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "yadgar-cfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

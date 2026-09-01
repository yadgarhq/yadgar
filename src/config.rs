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
        let mut config: Self =
            serde_json::from_str(&text).map_err(|e| ConfigError::Malformed(path, e))?;
        config.dir = dir.to_path_buf();
        Ok(config)
    }

    pub fn new(dir: &Path, gateway: String, token: String) -> Self {
        Self {
            gateway,
            token,
            dir: dir.to_path_buf(),
        }
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
mod tests {
    use super::*;

    #[test]
    fn address_and_token_round_trip_together() {
        let dir = tempdir();
        Config::new(&dir, "https://gw.example:18443/".into(), "tok-abc".into())
            .save()
            .unwrap();
        let back = Config::load_from(&dir).unwrap();
        assert_eq!(back.gateway_url(), "https://gw.example:18443/");
        assert_eq!(back.token(), "tok-abc");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The file, spelled out. Not generated, not round-tripped.
    ///
    /// `config.json` is a DURABLE format: every machine that has ever run
    /// `yaadgaar login` holds one, and this client has to keep reading it. A
    /// round trip cannot notice a rename, because both halves rename together
    /// — `#[serde(rename = "gw")]` on `gateway` writes `gw`, reads `gw`, and
    /// leaves the suite green while every config in existence stops resolving
    /// and every person is told to log in again.
    const KNOWN_CONFIG: &str = r#"{
  "gateway": "https://gw.example:18443/",
  "token": "tok-abc"
}"#;

    #[test]
    fn a_config_written_by_an_older_client_still_parses() {
        // Fixed bytes IN, expected values OUT. The literal is what is on disk on
        // somebody's laptop; nothing in this test can rename with the struct.
        let dir = tempdir();
        std::fs::write(dir.join(FILE), KNOWN_CONFIG).unwrap();
        let config = Config::load_from(&dir).unwrap();
        assert_eq!(config.gateway_url(), "https://gw.example:18443/");
        assert_eq!(config.token(), "tok-abc");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_config_this_client_writes_is_byte_identical_to_that_format() {
        // The other direction, and it needs saying separately: a client that
        // READS the old name and WRITES a new one leaves a file the previous
        // release cannot read, which is the same outage pointing backwards.
        //
        // The directory is deliberately absent from these bytes. It is where the
        // file IS, and a file that records its own location is a second answer
        // to a question that cannot disagree with itself while it has only one.
        let dir = tempdir();
        Config::new(&dir, "https://gw.example:18443/".into(), "tok-abc".into())
            .save()
            .unwrap();
        let written = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(written, KNOWN_CONFIG);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_tool_cache_lands_beside_the_config_it_was_loaded_with() {
        // The hazard this replaces: `read_tool_cache` and `write_tool_cache`
        // went through the process-global `base_dir()` while the config came
        // from wherever `load_from` was pointed. A test that loaded a config
        // from a temp directory read and OVERWROTE the real
        // `~/.config/yadgar/tools-cache.json` belonging to the person running
        // it, and nothing said so.
        let dir = tempdir();
        let config = Config::new(&dir, "https://gw".into(), "tok".into());
        assert_eq!(config.read_tool_cache(), None);
        config.write_tool_cache("{\"tools\":[]}").unwrap();
        assert_eq!(config.read_tool_cache().as_deref(), Some("{\"tools\":[]}"));
        assert!(
            dir.join(TOOL_CACHE).exists(),
            "the cache was written somewhere other than beside its own config"
        );
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
        Config::new(&dir, "https://gw".into(), "secret".into())
            .save()
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

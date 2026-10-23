//! Which yaadgaar tasks the harness list mirrors: the `harness.scope` key.
//!
//! Max's ruling (ADR-1000) is every task that is not `done` or `dropped`, and
//! the reminder cost of that is accepted. The scope is still a config key,
//! because it may be narrowed later, and a narrowed list must never be a
//! silent one.
//!
//! `harness.json` also holds `completion_ttl_secs` (car 3, see [`KEYS`]).
//!
//! THE KEY LIVES IN ITS OWN FILE, `harness.json` in the client's config
//! directory (`{"scope": "active"}`), and not in `config.json`. That file holds
//! the credential, and `login` rebuilds it from scratch (`login::reconcile`):
//! a setting kept there would be dropped by every re-login unless each rebuild
//! remembered to carry it.

use std::path::Path;
use std::time::Duration;

/// The settings file, beside `config.json`.
pub const FILE: &str = "harness.json";

/// One `harness.scope` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `all-open`: in_progress, blocked and open. The ruling.
    AllOpen,
    /// `active`: in_progress and blocked only.
    Active,
    /// `none`: nothing is mirrored; the banner alone.
    Off,
}

/// The value the ruling names, used when the key is absent.
pub const RULED: Scope = Scope::AllOpen; // ADR-0569-EXCEPTION(ABS): Max's ruling in ADR-1000 is the scope when `harness.scope` is absent; it is a decision, not a guess.

/// The words `harness.scope` accepts, in the order a message lists them.
pub const VALUES: [&str; 3] = ["all-open", "active", "none"];

/// Every key `harness.json` may hold. Any other key is refused, so a typo
/// (`"scop": "none"`) never silently means the ruling, the widest scope.
/// `completion_ttl_secs` (car 3) is how long a completion marker waits for
/// its `TaskCompleted`, in whole seconds.
pub const KEYS: [&str; 2] = ["scope", "completion_ttl_secs"];

/// How long a completion marker stays fresh when `completion_ttl_secs` is
/// absent. `task-update` writes the marker and `TaskCompleted` fires for the
/// same call, so the gap is normally milliseconds; a permission prompt
/// between them is what the margin is for.
pub const COMPLETION_TTL: Duration = Duration::from_secs(600); // ADR-0569-EXCEPTION(CC): bounds the gap between one TaskUpdate's PreToolUse and its TaskCompleted, a permission prompt included; `completion_ttl_secs` in harness.json overrides it.

/// Everything `harness.json` sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub scope: Scope,
    /// The freshness bound on a completion marker.
    pub completion_ttl: Duration,
}

impl Settings {
    /// Read `<config_dir>/harness.json`. An absent file, or an absent key, is
    /// the default for that key. A file that cannot be read or parsed, a key
    /// outside [`KEYS`], or a value of the wrong kind is an error, never the
    /// default: a person who wrote a setting must not have it ignored in
    /// silence.
    pub fn from_dir(config_dir: &Path) -> Result<Self, String> {
        let path = config_dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    scope: Scope::parse(None)?,
                    completion_ttl: COMPLETION_TTL,
                })
            }
            Err(e) => return Err(format!("{} could not be read ({e})", path.display())),
        };
        let invalid = |why: &str| format!("{} is not valid: {why}", path.display());
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| invalid(&e.to_string()))?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("not a JSON object"))?;
        if let Some(key) = object.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(invalid(&format!(
                "unknown key {:?} (the keys are {})",
                crate::printable::printable(key),
                KEYS.join(", ")
            )));
        }
        let scope = match object.get("scope") {
            None => Scope::parse(None),
            Some(serde_json::Value::String(s)) => Scope::parse(Some(s)),
            Some(_) => Err(invalid("`scope` is not a string")),
        }?;
        let completion_ttl = match object.get("completion_ttl_secs") {
            None => COMPLETION_TTL,
            Some(v) => match v.as_u64().filter(|n| *n > 0) {
                Some(n) => Duration::from_secs(n),
                None => {
                    return Err(invalid(
                        "`completion_ttl_secs` is not a whole number above 0",
                    ))
                }
            },
        };
        Ok(Self {
            scope,
            completion_ttl,
        })
    }
}

impl Scope {
    /// Read `harness.scope`. `None` (the key is absent) is the ruling.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None => Ok(RULED),
            Some("all-open") => Ok(Self::AllOpen),
            Some("active") => Ok(Self::Active),
            Some("none") => Ok(Self::Off),
            Some(other) => Err(format!(
                "harness.scope {:?} is not one of {}",
                crate::printable::printable(other),
                VALUES.join(", ")
            )),
        }
    }

    /// The yaadgaar statuses this scope mirrors.
    pub fn statuses(self) -> &'static [&'static str] {
        match self {
            Self::AllOpen => &["in_progress", "blocked", "open"],
            Self::Active => &["in_progress", "blocked"],
            Self::Off => &[],
        }
    }

    /// Does this scope mirror a task in *status*?
    pub fn holds(self, status: &str) -> bool {
        self.statuses().contains(&status)
    }
}

#[cfg(test)]
#[path = "scope/tests.rs"]
mod tests;

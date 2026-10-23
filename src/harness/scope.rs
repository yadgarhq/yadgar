//! Which yaadgaar tasks the harness list mirrors: the `harness.scope` key.
//!
//! Max's ruling (ADR-1000) is every task that is not `done` or `dropped`, and
//! the reminder cost of that is accepted. The scope is still a config key,
//! because it may be narrowed later, and a narrowed list must never be a
//! silent one.
//!
//! THE KEY LIVES IN ITS OWN FILE, `harness.json` in the client's config
//! directory (`{"scope": "active"}`), and not in `config.json`. That file holds
//! the credential, and `login` rebuilds it from scratch (`login::reconcile`):
//! a setting kept there would be dropped by every re-login unless each rebuild
//! remembered to carry it.

use std::path::Path;

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
pub const KEYS: [&str; 1] = ["scope"];

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

    /// Read `harness.scope` from `<config_dir>/harness.json`. An absent file,
    /// or a file with no `scope`, is the ruling. A file that cannot be read or
    /// parsed, or that holds a key outside [`KEYS`], is an error, never the
    /// ruling: a person who wrote a setting must not have it ignored in
    /// silence.
    pub fn from_dir(config_dir: &Path) -> Result<Self, String> {
        let path = config_dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::parse(None),
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
        match object.get("scope") {
            None => Self::parse(None),
            Some(serde_json::Value::String(s)) => Self::parse(Some(s)),
            Some(_) => Err(invalid("`scope` is not a string")),
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

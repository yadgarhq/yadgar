//! What `login` and `enrol` print once a credential is stored, or about to
//! be asked for.
//!
//! The gateway address comes from a prompt, a token blob or `config.json`,
//! and the username from the gateway, so each goes through
//! [`printable`] before it reaches the terminal.

use std::path::Path;

use crate::config::Config;
use crate::printable::printable;

/// The line `login` prints after storing the credential.
pub fn logged_in(config: &Config, dir: &Path) -> String {
    format!(
        "logged in to {} — the credential is stored in {}",
        printable(config.gateway_url()),
        dir.display()
    )
}

/// The line `enrol` prints after storing the credential. THE USERNAME IS SAID
/// HERE AND NOWHERE ELSE: `auth/enrol` is the only place the deployment tells
/// a person what they are called.
pub fn enrolled(config: &Config) -> String {
    format!(
        "enrolled with {} as {}",
        printable(config.gateway_url()),
        printable(config.username().unwrap_or("(the gateway named nobody)"))
    )
}

/// The line `enrol` prints before asking for a password, naming the address
/// the token carried.
pub fn enrolling(gateway: &str) -> String {
    format!("Enrolling with {}", printable(gateway))
}

#[cfg(test)]
mod tests;

//! `yaadgaar login` — the one sign-in a person performs (D72, D73).
//!
//! It asks for the gateway address, a username and a password; exchanges them
//! for a long-lived token; and writes both address and token as one unit.
//!
//! **The password is never stored and never echoed.** It is exchanged for a
//! token and dropped. The token the gateway returns is shown once and held only
//! here; the server keeps a hash.

use std::io::{self, Write as _};
use std::path::Path;

use serde::Deserialize;

use crate::config::Config;

/// Where the gateway serves login.
///
/// NOT an MCP method. Authentication and administration live on a separate path
/// from the tool surface (D73), so they never appear in `tools/list` and cannot
/// be reached by anything that can influence an agent's context.
const LOGIN_PATH: &str = "auth/login";

#[derive(Debug, Deserialize)]
struct LoginResponse {
    token: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("cannot read input: {0}")]
    Input(#[from] io::Error),
    #[error("cannot reach {0}: {1}")]
    Unreachable(String, reqwest::Error),
    /// One message for every way it can fail, matching what `iam` returns.
    ///
    /// Wrong password, unknown user, no password set — the gateway deliberately
    /// makes them indistinguishable so the endpoint cannot enumerate accounts
    /// (D73). Restating the distinction here would hand back exactly what the
    /// server went to some trouble to hide.
    #[error("invalid username or password")]
    Refused,
    #[error("the gateway answered with {0}")]
    Unexpected(reqwest::StatusCode),
    #[error("the gateway's answer was not a login response: {0}")]
    Malformed(reqwest::Error),
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
}

/// Prompt, exchange, store.
pub async fn login(config_dir: &Path) -> Result<Config, LoginError> {
    let gateway = prompt("Gateway address (e.g. https://gateway.yadgar.localhost:18443): ")?;
    let username = prompt("Username: ")?;
    // Read without echo. A password in the terminal's scrollback outlives the
    // session and reaches whatever records it.
    let password = rpassword::prompt_password("Password: ")?;

    let gateway = normalise(&gateway);
    let token = exchange(&gateway, &username, &password).await?;

    let config = Config::new(config_dir, gateway, token);
    config.save()?;
    Ok(config)
}

async fn exchange(gateway: &str, username: &str, password: &str) -> Result<String, LoginError> {
    let url = format!("{gateway}{LOGIN_PATH}");
    let response = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({
            "username": username,
            "password": password,
            // Free text naming this machine, so a person can tell their laptop's
            // credential from their desktop's when revoking one.
            "label": label(),
        }))
        .send()
        .await
        .map_err(|e| LoginError::Unreachable(url, e))?;

    match response.status() {
        s if s.is_success() => Ok(response
            .json::<LoginResponse>()
            .await
            .map_err(LoginError::Malformed)?
            .token),
        reqwest::StatusCode::UNAUTHORIZED => Err(LoginError::Refused),
        other => Err(LoginError::Unexpected(other)),
    }
}

/// A trailing slash, exactly one.
///
/// The address is joined with a path later, and `https://gw` + `auth/login`
/// silently becomes `https://gwauth/login` — a request to a host that does not
/// exist, reported as a DNS failure rather than as the typo it is.
fn normalise(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    format!("{trimmed}/")
}

/// A human-readable name for this machine, for the credential's label.
///
/// Cosmetic but not pointless: it is how a person tells their laptop's
/// credential from their desktop's when revoking one, so a wrong answer here
/// costs somebody the ability to revoke confidently.
fn label() -> String {
    hostname().unwrap_or_else(|| "unnamed machine".to_string())
}

/// Best effort, in the order most likely to be right on each platform.
///
/// `/etc/hostname` alone was WRONG: it is Linux-only, so macOS and Windows would
/// silently have labelled every credential "unknown host" and the whole point of
/// the field would have quietly stopped working on two of the three platforms.
/// The client must run on x86_64 and aarch64 across Linux, macOS and Windows.
fn hostname() -> Option<String> {
    // Windows sets this; Unix shells usually export it, though a non-interactive
    // shell may not, which is why the file fallback stays.
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Some(v) = std::env::var(var).ok().filter(|v| !v.trim().is_empty()) {
            return Some(v.trim().to_string());
        }
    }
    // Linux, and some BSDs. Absent on macOS and Windows, which is fine — the
    // environment above covers them, and the fallback covers neither being set.
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}

fn prompt(question: &str) -> Result<String, io::Error> {
    print!("{question}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_gets_exactly_one_trailing_slash() {
        // Without this, "https://gw" + "auth/login" becomes "https://gwauth/login"
        // — a DNS failure that reads as the gateway being down rather than as a
        // missing slash.
        assert_eq!(normalise("https://gw"), "https://gw/");
        assert_eq!(normalise("https://gw/"), "https://gw/");
        assert_eq!(normalise("https://gw///"), "https://gw/");
        assert_eq!(normalise("  https://gw  "), "https://gw/");
    }

    #[test]
    fn a_port_survives_normalisation() {
        // The development gateway is on 18443, so eating the port would break
        // every local login.
        assert_eq!(
            normalise("https://gateway.yadgar.localhost:18443"),
            "https://gateway.yadgar.localhost:18443/"
        );
    }

    #[test]
    fn every_refusal_reads_the_same() {
        // iam returns one message for wrong-password, unknown-user and
        // no-password-set so the endpoint cannot enumerate accounts.
        assert_eq!(
            LoginError::Refused.to_string(),
            "invalid username or password"
        );
    }

    #[test]
    fn a_refusal_is_not_reported_as_an_outage() {
        // A 401 and an unreachable gateway call for different actions from the
        // person: retype the password, or check the network.
        let refused = LoginError::Refused.to_string();
        assert!(!refused.contains("unreachable"));
        assert!(!refused.contains("gateway"));
    }
}

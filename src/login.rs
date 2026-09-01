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
    let gateway = prompt("Gateway address (e.g. https://gateway.yadgar.test:18443): ")?;
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

/// The URL a login request goes to, composed in ONE place.
///
/// Each half is already pinned — [`normalise`] guarantees exactly one trailing
/// slash, and `LOGIN_PATH` carries none. The JOIN is what nothing covered:
/// `LOGIN_PATH` gaining a leading slash makes `https://gw//auth/login`, which
/// every test of either half still passes, most servers quietly accept, and
/// some path-matching proxy in front of one does not.
fn login_url(gateway: &str) -> String {
    format!("{}{LOGIN_PATH}", normalise(gateway))
}

/// What the gateway's status means for the person at the terminal.
///
/// Separated from the request so the mapping is testable without a server, and
/// so the 401 arm cannot be deleted unnoticed — it is what stops a refusal being
/// reported as an outage, and sends somebody to retype a password instead of to
/// look at their network.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Read the token out of the body.
    Issued,
    /// The credentials were wrong.
    Refused,
    /// Not the person's to fix.
    Unexpected,
}

fn verdict(status: reqwest::StatusCode) -> Verdict {
    match status {
        s if s.is_success() => Verdict::Issued,
        reqwest::StatusCode::UNAUTHORIZED => Verdict::Refused,
        _ => Verdict::Unexpected,
    }
}

async fn exchange(gateway: &str, username: &str, password: &str) -> Result<String, LoginError> {
    let url = login_url(gateway);
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

    let status = response.status();
    match verdict(status) {
        Verdict::Issued => Ok(response
            .json::<LoginResponse>()
            .await
            .map_err(LoginError::Malformed)?
            .token),
        Verdict::Refused => Err(LoginError::Refused),
        Verdict::Unexpected => Err(LoginError::Unexpected(status)),
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
    label_from(|k| std::env::var(k).ok(), Path::new("/etc/hostname"))
}

fn label_from(var: impl Fn(&str) -> Option<String>, etc_hostname: &Path) -> String {
    hostname_from(var, etc_hostname).unwrap_or_else(|| "unnamed machine".to_string())
}

/// Best effort, in the order most likely to be right on each platform.
///
/// `/etc/hostname` alone was WRONG: it is Linux-only, so macOS and Windows would
/// silently have labelled every credential "unknown host" and the whole point of
/// the field would have quietly stopped working on two of the three platforms.
/// The client must run on x86_64 and aarch64 across Linux, macOS and Windows.
///
/// BOTH SOURCES ARE HANDED IN, rather than read here. Reading the environment
/// and `/etc/hostname` directly is why none of this order was ever exercised: a
/// test could only assert whatever the machine running it happened to be called,
/// so on a Linux host with `HOSTNAME` set, every arm but the first is
/// unreachable — and the fallbacks that exist FOR the other two platforms are
/// then never executed anywhere.
fn hostname_from(var: impl Fn(&str) -> Option<String>, etc_hostname: &Path) -> Option<String> {
    // Windows sets this; Unix shells usually export it, though a non-interactive
    // shell may not, which is why the file fallback stays.
    for name in ["COMPUTERNAME", "HOSTNAME"] {
        if let Some(v) = var(name).filter(|v| !v.trim().is_empty()) {
            return Some(v.trim().to_string());
        }
    }
    // Linux, and some BSDs. Absent on macOS and Windows, which is fine — the
    // environment above covers them, and the fallback covers neither being set.
    std::fs::read_to_string(etc_hostname)
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
            normalise("https://gateway.yadgar.test:18443"),
            "https://gateway.yadgar.test:18443/"
        );
    }

    #[test]
    fn the_login_url_is_pinned_as_a_whole_string() {
        // Both halves are already tested and the JOIN is what is not. A leading
        // slash on LOGIN_PATH gives "https://gw//auth/login": every test of
        // `normalise` still passes, every test of the constant still passes, and
        // the extra slash is the kind of thing nothing notices until a proxy in
        // front of the gateway matches on the path.
        assert_eq!(login_url("https://gw"), "https://gw/auth/login");
        assert_eq!(login_url("https://gw/"), "https://gw/auth/login");
        assert_eq!(
            login_url("https://gateway.yadgar.test:18443"),
            "https://gateway.yadgar.test:18443/auth/login"
        );
    }

    #[test]
    fn a_401_is_a_refusal_and_every_other_failure_is_not() {
        // Deleting the 401 arm makes a wrong password read as "the gateway
        // answered with 401 Unauthorized" — technically true, and it sends
        // somebody to look at their network instead of retyping a password.
        assert_eq!(verdict(reqwest::StatusCode::UNAUTHORIZED), Verdict::Refused);
        assert_eq!(verdict(reqwest::StatusCode::OK), Verdict::Issued);
        assert_eq!(
            verdict(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            Verdict::Unexpected
        );
        assert_eq!(verdict(reqwest::StatusCode::NOT_FOUND), Verdict::Unexpected);
    }

    #[tokio::test]
    async fn no_refusal_can_tell_a_person_which_half_was_wrong() {
        // THE PROPERTY, not the sentence. `iam` returns one answer for wrong
        // password, unknown user and no-password-set so the endpoint cannot
        // enumerate accounts (D73). Restating the `#[error]` string would pass
        // whatever the code did with the body; this sends three 401s that say
        // different things and asserts the person is shown one identical
        // message, because the body is never read on this path at all.
        let mut seen = std::collections::BTreeSet::new();
        for body in [
            r#"{"detail":"no such user"}"#,
            r#"{"detail":"wrong password"}"#,
            r#"{"detail":"that account has no password set"}"#,
        ] {
            let (addr, served) = crate::testserver::answer_once("401 Unauthorized", body).await;
            let err = exchange(&format!("http://{addr}/"), "someone", "hunter2")
                .await
                .expect_err("a 401 is not a login");
            let _ = served.await;
            assert!(matches!(err, LoginError::Refused), "got {err:?}");
            seen.insert(err.to_string());
        }
        assert_eq!(
            seen.len(),
            1,
            "the gateway's wording reached the person, and the endpoint now enumerates accounts: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_refusal_and_an_outage_are_different_things_to_the_caller() {
        // THE PROPERTY, not the wording. The old test asserted the message did
        // not contain "gateway", so rewording it broke the test while the thing
        // that matters — that a caller can tell "retype your password" from
        // "check the network" — was never checked at all.
        let (addr, served) = crate::testserver::answer_once("401 Unauthorized", "{}").await;
        let refused = exchange(&format!("http://{addr}/"), "someone", "hunter2")
            .await
            .expect_err("a 401 is not a login");
        let _ = served.await;

        // A port bound and immediately dropped: nothing is listening, so this is
        // a connection failure rather than an answer.
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        let unreachable = exchange(&format!("http://{dead_addr}/"), "someone", "hunter2")
            .await
            .expect_err("nothing is listening there");

        assert!(matches!(refused, LoginError::Refused), "got {refused:?}");
        assert!(
            matches!(unreachable, LoginError::Unreachable(..)),
            "an unreachable gateway was reported as something else: {unreachable:?}"
        );
    }

    #[test]
    fn the_machine_name_comes_from_the_environment_first() {
        // Windows sets COMPUTERNAME and nothing else; this arm is why the label
        // is not "unnamed machine" on two of the three platforms.
        assert_eq!(
            hostname_from(
                |k| (k == "COMPUTERNAME").then(|| "DESK-01".to_string()),
                Path::new("/nonexistent/etc/hostname"),
            ),
            Some("DESK-01".into())
        );
    }

    #[test]
    fn an_empty_variable_falls_through_rather_than_naming_the_machine_nothing() {
        // A non-interactive shell can export HOSTNAME as an empty string, and a
        // credential labelled "" is worse than one labelled by the file below.
        let file = crate::testserver::scratch_dir("login-hostname").join("hostname");
        std::fs::write(&file, "workshop\n").unwrap();
        assert_eq!(
            hostname_from(|_| Some("   ".to_string()), &file),
            Some("workshop".into())
        );
        std::fs::remove_dir_all(file.parent().unwrap()).ok();
    }

    #[test]
    fn a_machine_that_will_not_say_its_name_is_labelled_rather_than_left_blank() {
        assert_eq!(
            hostname_from(|_| None, Path::new("/nonexistent/etc/hostname")),
            None
        );
        assert_eq!(
            label_from(|_| None, Path::new("/nonexistent/etc/hostname")),
            "unnamed machine"
        );
    }
}

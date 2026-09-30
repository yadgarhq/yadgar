//! What goes on the wire to `auth/login` and `auth/enrol`, and what comes back.
//!
//! Split out of `login.rs` when the file passed its size ceiling again. The
//! seam is the one `label.rs` and `reconcile.rs` already use: this subsystem
//! is everything about the two HTTP requests themselves — the paths, the
//! bodies, the status-code verdict — while `login.rs` keeps the prompting,
//! the trust decisions and the config it assembles from what comes back here.

use serde::Deserialize;

use super::LoginError;

/// Where the gateway serves login.
///
/// NOT an MCP method. Authentication and administration live on a separate path
/// from the tool surface (D73), so they never appear in `tools/list` and cannot
/// be reached by anything that can influence an agent's context.
const LOGIN_PATH: &str = "auth/login";

/// Where the gateway serves enrolment — the unauthenticated half of D73.
const ENROL_PATH: &str = "auth/enrol";

#[derive(Debug, Deserialize)]
struct LoginResponse {
    token: String,
}

/// What `auth/enrol` answers with.
///
/// THE USERNAME IS THE HALF THAT ONLY EXISTS HERE. The token is a credential
/// like any other, but the username is minted by the deployment and said once:
/// a person enrolling on their first machine has no other way to learn what
/// they are called, and cannot complete a `login` anywhere else without it.
#[derive(Debug, Deserialize)]
pub(super) struct EnrolResponse {
    pub(super) token: String,
    pub(super) username: String,
}

/// The URL an enrolment goes to. Same join rule as [`login_url`], same reason.
pub(super) fn enrol_url(gateway: &str) -> String {
    format!("{}{ENROL_PATH}", super::normalise(gateway))
}

/// Present the secret, set the password, take back a credential and a name.
///
/// **THE SECRET FIELD, NEVER THE WHOLE BLOB.** The contract is explicit, and
/// sending the base64 envelope would present a string the server never hashed —
/// refused as a wrong secret, with the person holding a token that is fine.
pub(super) async fn redeem(
    client: &reqwest::Client,
    gateway: &str,
    secret: &str,
    password: &str,
) -> Result<EnrolResponse, LoginError> {
    let url = enrol_url(gateway);
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "secret": secret,
            "password": password,
            "label": super::label::label(),
        }))
        .send()
        .await
        .map_err(|e| LoginError::Unreachable(url, e))?;

    let status = response.status();
    match verdict(status) {
        // A REPLAYED SECRET AND AN UNKNOWN ONE ANSWER IDENTICALLY, by design —
        // `RedeemEnrolment` is unauthenticated, so telling them apart would say
        // whether a given secret ever existed.
        Verdict::Issued => response.json().await.map_err(LoginError::Malformed),
        Verdict::Refused => Err(LoginError::SecretRefused),
        Verdict::Unexpected => Err(LoginError::Unexpected(status)),
    }
}

/// The URL a login request goes to, composed in ONE place.
///
/// Each half is already pinned — [`super::normalise`] guarantees exactly one
/// trailing slash, and `LOGIN_PATH` carries none. The JOIN is what nothing
/// covered: `LOGIN_PATH` gaining a leading slash makes `https://gw//auth/login`,
/// which every test of either half still passes, most servers quietly accept,
/// and some path-matching proxy in front of one does not.
pub(super) fn login_url(gateway: &str) -> String {
    format!("{}{LOGIN_PATH}", super::normalise(gateway))
}

/// What the gateway's status means for the person at the terminal.
///
/// Separated from the request so the mapping is testable without a server, and
/// so the 401 arm cannot be deleted unnoticed — it is what stops a refusal being
/// reported as an outage, and sends somebody to retype a password instead of to
/// look at their network.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Read the token out of the body.
    Issued,
    /// The credentials were wrong.
    Refused,
    /// Not the person's to fix.
    Unexpected,
}

pub(super) fn verdict(status: reqwest::StatusCode) -> Verdict {
    match status {
        s if s.is_success() => Verdict::Issued,
        reqwest::StatusCode::UNAUTHORIZED => Verdict::Refused,
        _ => Verdict::Unexpected,
    }
}

pub(super) async fn exchange(
    client: &reqwest::Client,
    gateway: &str,
    username: &str,
    password: &str,
) -> Result<String, LoginError> {
    let url = login_url(gateway);
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "username": username,
            "password": password,
            // Free text naming this machine, so a person can tell their laptop's
            // credential from their desktop's when revoking one.
            "label": super::label::label(),
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

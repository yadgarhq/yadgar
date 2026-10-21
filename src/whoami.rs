//! `yaadgaar whoami` — ask the gateway who the saved credential names
//! (ADR-0959), so a person can build a private project path
//! `local/<account_segment>/…` (ADR-0954) without guessing their uuid.
//!
//! **THE TOKEN IS NEVER PRINTED.** It travels in the `Authorization` header and
//! nowhere else: not on stdout, not on stderr, not in an error message.

use serde::Deserialize;

use crate::admin::Gateway;
use crate::config::Config;

/// Why `whoami` failed.
#[derive(Debug, thiserror::Error)]
pub enum WhoamiError {
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error(transparent)]
    Trust(#[from] crate::trust::TrustError),
    /// A transport failure: DNS, connect, TLS, or a timeout. The cause is the
    /// `source`, so anyhow prints the whole chain (a certificate error, say).
    #[error("cannot reach {0}")]
    Unreachable(String, #[source] reqwest::Error),
    /// 401: the stored credential did not resolve.
    #[error(
        "the gateway refused your saved credential ({0}): it is invalid or has been \
         revoked. Run `yaadgaar login` again"
    )]
    Refused(String),
    /// 401 after the stored expiry has passed (ADR-0972 (3)).
    #[error("your credential expired at {0}; run `yaadgaar login`")]
    Expired(String),
    /// 404: a gateway older than the route.
    #[error("this gateway does not serve /auth/whoami (gateway older than 0.12.17?)")]
    RouteNotServed,
    #[error("the gateway answered {0}: {1}")]
    Rejected(reqwest::StatusCode, String),
    #[error("the gateway's answer was not an identity: {0}")]
    Malformed(String),
    /// 200 with no account segment: the gateway resolved the caller through a
    /// path that carries no account (the dev trusted-headers path).
    #[error(
        "the gateway named you {0} but gave no account segment, so there is no private \
         `local/` prefix to show (is it running on the dev trusted-headers path?)"
    )]
    NoSegment(String),
    /// 200 with a segment that cannot be ONE path segment: a slash,
    /// whitespace or a control character. Shown escaped, never raw.
    #[error("the gateway gave an account segment that is not one path segment: {0:?}")]
    UnsafeSegment(String),
    #[error("cannot write to the terminal: {0}")]
    Output(#[from] std::io::Error),
}

/// What `/auth/whoami` answered.
#[derive(Debug, Deserialize)]
pub struct Identity {
    pub user_id: String,
    #[serde(default)]
    pub account_segment: Option<String>,
}

/// Run `whoami` against the logged-in gateway.
pub async fn run() -> Result<(), WhoamiError> {
    let config = Config::load()?;
    let client = crate::trust::client(config.ca_pem(), Some(crate::proxy::REQUEST_TIMEOUT))?;
    let gateway = Gateway {
        url: config.gateway_url(),
        bearer: config.token(),
        instance: config.instance(),
        expires_at: config.expires_at(),
    };
    execute(&client, &gateway, &mut std::io::stdout().lock()).await
}

/// [`run`], given its client, gateway and stream, so every path is reachable
/// from a test.
async fn execute(
    client: &reqwest::Client,
    gateway: &Gateway<'_>,
    stdout: &mut dyn std::io::Write,
) -> Result<(), WhoamiError> {
    let identity = match request(client, gateway).await {
        // A 401 after the stored expiry is the expiry, not the guess.
        Err(WhoamiError::Refused(reason)) => {
            let expired = crate::expiry::expired_at(gateway.expires_at, crate::expiry::now());
            return Err(match expired {
                Some(at) => WhoamiError::Expired(at.to_string()),
                None => WhoamiError::Refused(reason),
            });
        }
        other => other?,
    };
    // NO PREFIX WITHOUT A SEGMENT: `local//` is not a path anybody can use.
    let segment = match identity.account_segment.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Err(WhoamiError::NoSegment(identity.user_id)),
    };
    if segment
        .chars()
        .any(|c| c == '/' || c.is_whitespace() || c.is_control())
    {
        return Err(WhoamiError::UnsafeSegment(segment));
    }
    // The gateway wrote it: escape sequences must not reach the terminal.
    let user_id = crate::printable::printable(&identity.user_id);
    writeln!(stdout, "user_id:         {user_id}")?;
    writeln!(stdout, "account_segment: {segment}")?;
    writeln!(stdout, "private prefix:  local/{segment}/")?;
    // THE STORED expiry, as `login` or `enrol` received it (ADR-0972 (3)).
    let expires = crate::expiry::shown(gateway.expires_at);
    writeln!(stdout, "expires_at:      {expires}")?;
    Ok(())
}

/// `GET <gateway>auth/whoami`, as *bearer*. The status is checked before the
/// body is believed.
async fn request(client: &reqwest::Client, gateway: &Gateway<'_>) -> Result<Identity, WhoamiError> {
    // NORMALISED, for `login::normalise`'s reason: a hand-edited `config.json`
    // without its trailing slash would otherwise name host `gwauth`.
    let url = format!("{}auth/whoami", crate::login::normalise(gateway.url));
    let mut builder = client.get(&url).bearer_auth(gateway.bearer);
    if let Some(instance) = gateway.instance {
        builder = builder.header("x-yadgar-instance", instance);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| WhoamiError::Unreachable(url, e))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    match status {
        s if s.is_success() => {
            serde_json::from_str(&body).map_err(|e| WhoamiError::Malformed(e.to_string()))
        }
        reqwest::StatusCode::UNAUTHORIZED => {
            Err(WhoamiError::Refused(crate::admin::gateway_error(&body)))
        }
        reqwest::StatusCode::NOT_FOUND => Err(WhoamiError::RouteNotServed),
        _ => Err(WhoamiError::Rejected(
            status,
            crate::admin::gateway_error(&body),
        )),
    }
}

#[cfg(test)]
mod tests;

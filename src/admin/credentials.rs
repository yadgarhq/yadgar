//! `admin list-credentials` and `admin revoke-credential` (ADR-0972 (2),
//! ledger 1627): one user's credentials, listed or revoked one at a time.
//!
//! **NEITHER PRINTS THE USER ID**, for the reason `admin::dispatch` gives: the
//! operator typed it, and CodeQL's cleartext-logging rule flags it. A row
//! carries the credential id, its times and its label, and no token: the
//! gateway lists none (gateway#127).

use std::collections::HashSet;

use serde::Deserialize;

use super::{gateway_error, AdminError, Gateway};
use crate::printable::printable;

const LIST: &str = "list-credentials";
const REVOKE: &str = "revoke-credential";

/// The gateway's exact 404 body for a credential this user does not hold.
const NO_SUCH_CREDENTIAL: &str = "no such credential for this user";

/// One page of `/admin/list-credentials`.
#[derive(Debug, Deserialize)]
struct Page {
    credentials: Vec<Row>,
    /// Empty on the last page.
    #[serde(default)]
    next_page_token: String,
}

/// One listed credential. Times are RFC 3339 as the gateway wrote them.
#[derive(Debug, Deserialize)]
struct Row {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    created_at: Option<String>,
    /// Absent for a row minted before ADR-0972 (3): it never expires.
    #[serde(default)]
    expires_at: Option<String>,
}

/// What `/admin/revoke-credential` answered.
#[derive(Debug, Deserialize)]
struct Receipt {
    user_id: String,
    credential_id: String,
}

/// List every credential of *user_id*, following each page to the end.
///
/// Rows go to stdout, one per line and tab-separated, so a pipe can cut them;
/// the count goes to stderr.
pub(super) async fn list(
    client: &reqwest::Client,
    gateway: &Gateway<'_>,
    user_id: &str,
    stdout: &mut dyn std::io::Write,
    stderr: &mut dyn std::io::Write,
) -> Result<(), AdminError> {
    let mut seen = HashSet::new();
    let mut token = String::new();
    let mut count = 0usize;
    loop {
        let mut body = serde_json::json!({ "user_id": user_id });
        if !token.is_empty() {
            body["page_token"] = serde_json::Value::String(token.clone());
        }
        let (status, text) = post(client, gateway, LIST, &body).await?;
        let page: Page = match status {
            s if s.is_success() => serde_json::from_str(&text)
                .map_err(|e| AdminError::MalformedAnswer("a credential page", e.to_string()))?,
            // An unknown user is an empty page, so ANY 404 is a missing route.
            reqwest::StatusCode::NOT_FOUND => return Err(AdminError::RouteAbsent(LIST)),
            _ => return Err(refusal(status, &text)),
        };
        for row in &page.credentials {
            writeln!(stdout, "{}", render(row))?;
        }
        count += page.credentials.len();
        if page.next_page_token.is_empty() {
            break;
        }
        if !seen.insert(page.next_page_token.clone()) {
            return Err(AdminError::RepeatedPageToken);
        }
        token = page.next_page_token;
    }
    match count {
        0 => writeln!(stderr, "no credentials (none live, or no such user)")?,
        1 => writeln!(stderr, "1 credential")?,
        n => writeln!(stderr, "{n} credentials")?,
    }
    Ok(())
}

/// Revoke *credential_id* of *user_id*, and check the receipt names both.
pub(super) async fn revoke(
    client: &reqwest::Client,
    gateway: &Gateway<'_>,
    user_id: &str,
    credential_id: &str,
    stdout: &mut dyn std::io::Write,
) -> Result<(), AdminError> {
    let credential_id = credential_id.trim();
    if credential_id.is_empty() {
        return Err(AdminError::EmptyCredentialId);
    }
    let body = serde_json::json!({ "user_id": user_id, "credential_id": credential_id });
    let (status, text) = post(client, gateway, REVOKE, &body).await?;
    let receipt: Receipt = match status {
        s if s.is_success() => serde_json::from_str(&text)
            .map_err(|e| AdminError::MalformedAnswer("a revocation receipt", e.to_string()))?,
        reqwest::StatusCode::NOT_FOUND => {
            // ONLY THE GATEWAY'S OWN WORDS mean a missing credential: a
            // gateway without the route also answers 404.
            let said = gateway_error(&text);
            return Err(if said == NO_SUCH_CREDENTIAL {
                AdminError::NoSuchCredential(said)
            } else {
                AdminError::RouteAbsent(REVOKE)
            });
        }
        _ => return Err(refusal(status, &text)),
    };
    // REFUSED, NOT WARNED: a receipt for something else means something else
    // may be gone.
    if receipt.user_id != user_id || receipt.credential_id != credential_id {
        return Err(AdminError::CredentialEchoMismatch);
    }
    writeln!(stdout, "revoked credential {}", printable(credential_id))?;
    Ok(())
}

/// Every non-success, non-404 answer, mapped the way revoke-credentials maps
/// it.
fn refusal(status: reqwest::StatusCode, body: &str) -> AdminError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED => AdminError::Refused,
        reqwest::StatusCode::FORBIDDEN => AdminError::CredentialForbidden(gateway_error(body)),
        reqwest::StatusCode::BAD_REQUEST => AdminError::BadRequest(gateway_error(body)),
        _ => AdminError::Rejected(status, gateway_error(body)),
    }
}

/// `POST <gateway>admin/<route>` as the caller, with *body* and nothing else:
/// both routes reject unknown keys.
async fn post(
    client: &reqwest::Client,
    gateway: &Gateway<'_>,
    route: &str,
    body: &serde_json::Value,
) -> Result<(reqwest::StatusCode, String), AdminError> {
    let url = format!("{}admin/{route}", crate::login::normalise(gateway.url));
    let mut builder = client.post(&url).bearer_auth(gateway.bearer).json(body);
    if let Some(instance) = gateway.instance {
        builder = builder.header("x-yadgar-instance", instance);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| AdminError::Unreachable(url, e))?;
    let status = response.status();
    Ok((status, response.text().await.unwrap_or_default()))
}

/// One row: id, created, expires, label. Every field is gateway-written, so
/// each goes through [`printable`] before it reaches a terminal.
fn render(row: &Row) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        printable(&row.id),
        printable(row.created_at.as_deref().unwrap_or("unknown")),
        printable(row.expires_at.as_deref().unwrap_or("never")),
        printable(&row.label),
    )
}

#[cfg(test)]
mod tests;

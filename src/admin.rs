//! `yaadgaar admin` — the administrative verbs an administrator runs from the
//! client, instead of hand-built `curl` against `/admin/*` (ledger 1552, 1569).
//!
//! **NOT MCP TOOLS.** ADR-0492 rules that an administrative verb never appears
//! as one, and the gateway serves these as plain HTTPS routes outside `/`. This
//! module talks to those routes directly with the caller's OWN credential; the
//! proxy never sees them.
//!
//! **AUTHORITY IS THE GATEWAY'S, NEVER THIS CLIENT'S.** The bearer token names
//! the caller, and the gateway asks `iam` whether that caller is an
//! administrator (ADR-0511). Nothing here checks or claims admin status: a
//! refusal comes back as 403 and is reported as one.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::Deserialize;

use crate::config::Config;

mod credentials;
mod error;

pub use error::AdminError;

#[derive(Subcommand)]
pub enum AdminCommand {
    /// Issue an enrolment token for an existing user: their first enrolment,
    /// or a password reset.
    ///
    /// The person redeems it with `yaadgaar enrol --token-file <file>` and
    /// chooses their own password; you never learn it. Calls the gateway's
    /// `/admin/issue-enrolment` with YOUR credential, so you must be an
    /// administrator.
    ///
    /// THE TOKEN IS NEVER PRINTED BY DEFAULT. It is a single-use secret valid
    /// for 24 hours, so one of `--out` or `--stdout` is required: a terminal's
    /// scrollback is not where it should land by accident.
    #[command(group(clap::ArgGroup::new("sink").required(true).args(["out", "stdout"])))]
    IssueEnrolment {
        /// The user's id, as `/admin/create-user` answered it. The gateway has
        /// no lookup by username, so the id is the only key it accepts.
        #[arg(long = "user-id")]
        user_id: String,

        /// Write the token to this NEW file, created owner-only (0600). An
        /// existing file or a symlink is refused rather than replaced.
        #[arg(long)]
        out: Option<PathBuf>,

        /// Print the token alone on stdout, for a pipe.
        #[arg(long)]
        stdout: bool,

        /// Revoke every credential the user holds when they REDEEM this
        /// token: a password reset after a lost or leaked machine (ledger
        /// 1569). Nothing is revoked when the token is issued, so the old
        /// credentials keep working until the person runs `enrol`. Off by
        /// default, because it also logs out the user's other machines. The
        /// bootstrap token may not ask for it.
        #[arg(long = "revoke-existing")]
        revoke_existing: bool,
    },

    /// Revoke EVERY live credential a user holds, now (ADR-0945): a leaked
    /// token, or a lost machine, killed at once rather than at the person's
    /// next redemption.
    ///
    /// Calls the gateway's `/admin/revoke-credentials` with YOUR credential,
    /// so you must be an administrator. It logs the user out of every
    /// machine. It does NOT change their password and does NOT lock the
    /// account: they can log in again. You may not revoke your own
    /// credentials. It asks no confirmation, so it can run unattended.
    RevokeCredentials {
        /// The user's id, `yadgar:user:<uuidv7>`, as `/admin/create-user`
        /// answered it. The gateway has no lookup by username.
        #[arg(long = "user-id")]
        user_id: String,
    },

    /// List a user's live credentials, one row each: id, created, expires
    /// (`never` for one minted before expiry existed), label.
    ///
    /// Calls `/admin/list-credentials` with YOUR credential, following every
    /// page, so you must be an administrator. No token is ever listed.
    ListCredentials {
        /// The user's id, `yadgar:user:<uuidv7>`.
        #[arg(long = "user-id")]
        user_id: String,
    },

    /// Revoke ONE credential of a user, by the id `list-credentials` showed:
    /// one lost machine, without logging the user out everywhere.
    ///
    /// Calls `/admin/revoke-credential` with YOUR credential, so you must be an
    /// administrator. You MAY revoke your own credential (ADR-0972 (2)).
    RevokeCredential {
        /// The user's id, `yadgar:user:<uuidv7>`.
        #[arg(long = "user-id")]
        user_id: String,
        /// The credential's id, as `list-credentials` printed it.
        #[arg(long = "credential-id")]
        credential_id: String,
    },
}

/// What `/admin/issue-enrolment` answered.
#[derive(Debug, Deserialize)]
pub struct Issued {
    /// The base64 blob the person redeems. A secret: shown once.
    pub token: String,
    pub enrolment_id: String,
    /// The gateway's echo of the revocation demand (ledger 1579). Absent from
    /// a gateway older than the echo.
    #[serde(default)]
    pub revoke_existing_credentials: Option<bool>,
}

/// What `/admin/revoke-credentials` answered.
#[derive(Debug, Deserialize)]
pub struct Revoked {
    pub user_id: String,
    pub revoked: u64,
    /// Outstanding enrolments revoke-all voided (ADR-0972 (5)). Absent from a
    /// gateway older than gateway#127, and then not printed.
    #[serde(default)]
    pub voided_enrolments: Option<u64>,
}

/// Where an administrative call goes, and as whom.
pub struct Gateway<'a> {
    pub url: &'a str,
    /// The caller's own credential, sent as `Authorization: Bearer`.
    pub bearer: &'a str,
    pub instance: Option<&'a str>,
    /// The stored expiry of *bearer*, for the wording of a 401 only.
    pub expires_at: Option<&'a str>,
}

/// Run one `admin` subcommand against the logged-in gateway.
///
/// Bounded by the proxy's `REQUEST_TIMEOUT`: a gateway that never answers
/// must not hang an administrator's terminal either.
pub async fn run(command: AdminCommand) -> Result<(), AdminError> {
    let config = Config::load()?;
    let client = crate::trust::client(config.ca_pem(), Some(crate::proxy::REQUEST_TIMEOUT))?;
    let gateway = Gateway {
        url: config.gateway_url(),
        bearer: config.token(),
        instance: config.instance(),
        expires_at: config.expires_at(),
    };
    execute(
        command,
        &client,
        &gateway,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
    .await
}

/// [`run`], given its client, gateway and streams, so every path through a
/// parsed command is reachable from a test. A 401 after the stored expiry
/// names the expiry (ADR-0972 (3)).
async fn execute(
    command: AdminCommand,
    client: &reqwest::Client,
    gateway: &Gateway<'_>,
    stdout: &mut dyn std::io::Write,
    stderr: &mut dyn std::io::Write,
) -> Result<(), AdminError> {
    let result = dispatch(command, client, gateway, stdout, stderr).await;
    let expired = crate::expiry::expired_at(gateway.expires_at, crate::expiry::now());
    result.map_err(|e| e.naming_expiry(expired))
}

/// One parsed command, run.
///
/// THE RECEIPT NAMES THE ENROLMENT, NEVER THE USER. The operator typed the
/// user id, and CodeQL's cleartext-logging rule flags it on stderr; the
/// enrolment id is enough to find the row.
async fn dispatch(
    command: AdminCommand,
    client: &reqwest::Client,
    gateway: &Gateway<'_>,
    stdout: &mut dyn std::io::Write,
    stderr: &mut dyn std::io::Write,
) -> Result<(), AdminError> {
    match command {
        AdminCommand::IssueEnrolment {
            user_id,
            out,
            stdout: _,
            revoke_existing,
        } => {
            let user_id = require_user_id(&user_id)?;
            // PRE-FLIGHT: asking first would mint a token with nowhere to go.
            // `write_token_file`'s `create_new` stays the real guard.
            if let Some(path) = &out {
                refuse_existing(path)?;
            }
            let issued = request(
                client,
                gateway.url,
                gateway.bearer,
                gateway.instance,
                &user_id,
                revoke_existing,
            )
            .await?;
            if let Some(warning) = check_echo(revoke_existing, issued.revoke_existing_credentials)?
            {
                writeln!(stderr, "warning: {warning}")?;
            }
            // The receipt goes to STDERR in both cases, so `--stdout` leaves
            // the token alone on stdout for whatever reads the pipe.
            match out {
                Some(path) => {
                    write_token_file(&path, &issued.token)
                        .map_err(|e| AdminError::TokenFile(path.clone(), e))?;
                    writeln!(
                        stderr,
                        "enrolment {} issued; the token is in {}",
                        issued.enrolment_id,
                        path.display()
                    )?;
                }
                None => {
                    writeln!(stdout, "{}", issued.token)?;
                    writeln!(stderr, "enrolment {} issued", issued.enrolment_id)?;
                }
            }
            writeln!(stderr, "it is single-use and expires in 24 hours")?;
            Ok(())
        }
        AdminCommand::RevokeCredentials { user_id } => {
            let user_id = canonical_user_id(&user_id)?;
            let receipt = revoke_request(client, gateway, &user_id).await?;
            // REFUSED, NOT WARNED: an answer about someone else means someone
            // else's credentials may be gone.
            if receipt.user_id != user_id {
                return Err(AdminError::RevokeEchoMismatch);
            }
            // The receipt carries the count, never the user id (see `execute`).
            let noun = if receipt.revoked == 1 {
                "credential"
            } else {
                "credentials"
            };
            writeln!(stdout, "revoked {} {noun}", receipt.revoked)?;
            if let Some(voided) = receipt.voided_enrolments {
                let noun = if voided == 1 {
                    "enrolment"
                } else {
                    "enrolments"
                };
                writeln!(stdout, "voided {voided} outstanding {noun}")?;
            }
            if receipt.revoked == 0 {
                writeln!(
                    stderr,
                    "warning: nothing was live; the user held no live credentials, or \
                     this is a retry of a revocation that already happened"
                )?;
            }
            Ok(())
        }
        AdminCommand::ListCredentials { user_id } => {
            let user_id = canonical_user_id(&user_id)?;
            credentials::list(client, gateway, &user_id, stdout, stderr).await
        }
        AdminCommand::RevokeCredential {
            user_id,
            credential_id,
        } => {
            let user_id = canonical_user_id(&user_id)?;
            credentials::revoke(client, gateway, &user_id, &credential_id, stdout).await
        }
    }
}

/// Refuse *path* if anything is there, a dangling symlink included.
fn refuse_existing(path: &Path) -> Result<(), AdminError> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AdminError::TokenFile(path.to_path_buf(), e)),
        Ok(_) => Err(AdminError::TokenFile(
            path.to_path_buf(),
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "something is already there; it is never replaced",
            ),
        )),
    }
}

/// The id, trimmed, or a refusal before anything is sent.
fn require_user_id(raw: &str) -> Result<String, AdminError> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(AdminError::EmptyUserId);
    }
    Ok(id.to_string())
}

/// The id, trimmed, and proven canonical (`yadgar:user:<lowercase uuidv7>`).
///
/// The gateway answers 400 for any other form (ADR-0945); refusing here says
/// so before a request is made. The message never repeats the id.
fn canonical_user_id(raw: &str) -> Result<String, AdminError> {
    let id = require_user_id(raw)?;
    let canonical = id
        .strip_prefix("yadgar:user:")
        .and_then(|rest| uuid::Uuid::parse_str(rest).ok().map(|u| (rest, u)))
        // `parse_str` also takes uppercase, braced, simple and URN forms; only
        // the lowercase hyphenated text round-trips to itself.
        .is_some_and(|(rest, u)| {
            u.get_version_num() == 7
                && u.get_variant() == uuid::Variant::RFC4122
                && u.hyphenated().to_string() == rest
        });
    if canonical {
        Ok(id)
    } else {
        Err(AdminError::NotCanonicalUserId)
    }
}

/// `POST <gateway>admin/revoke-credentials`, as *bearer*, with `{"user_id"}`
/// and nothing else: the route rejects unknown keys (ADR-0945).
async fn revoke_request(
    client: &reqwest::Client,
    gateway: &Gateway<'_>,
    user_id: &str,
) -> Result<Revoked, AdminError> {
    let url = format!(
        "{}admin/revoke-credentials",
        crate::login::normalise(gateway.url)
    );
    let mut builder = client
        .post(&url)
        .bearer_auth(gateway.bearer)
        .json(&serde_json::json!({ "user_id": user_id }));
    if let Some(instance) = gateway.instance {
        builder = builder.header("x-yadgar-instance", instance);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| AdminError::Unreachable(url, e))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    match status {
        s if s.is_success() => {
            serde_json::from_str(&body).map_err(|e| AdminError::MalformedRevocation(e.to_string()))
        }
        reqwest::StatusCode::UNAUTHORIZED => Err(AdminError::Refused),
        reqwest::StatusCode::FORBIDDEN => Err(AdminError::RevokeForbidden(gateway_error(&body))),
        reqwest::StatusCode::NOT_FOUND => {
            // ONLY THE GATEWAY'S OWN WORDS mean a missing user: a gateway
            // without the route also answers 404, with some other body.
            let text = gateway_error(&body);
            if text == "no such user" {
                Err(AdminError::NoSuchUser(text))
            } else {
                Err(AdminError::RouteNotServed)
            }
        }
        reqwest::StatusCode::BAD_REQUEST => Err(AdminError::BadRequest(gateway_error(&body))),
        _ => Err(AdminError::Rejected(status, gateway_error(&body))),
    }
}

/// `POST <gateway>admin/issue-enrolment`, as *bearer*.
///
/// The gateway reads `Authorization: Bearer <token>` and nothing else as the
/// caller's identity. `x-yadgar-instance` is sent when this install has one,
/// as the proxy sends it; `/admin` needs no project header (ledger 1545).
async fn request(
    client: &reqwest::Client,
    gateway: &str,
    bearer: &str,
    instance: Option<&str>,
    user_id: &str,
    revoke_existing: bool,
) -> Result<Issued, AdminError> {
    // NORMALISED, for `login::normalise`'s reason: `config.json` can be edited
    // by hand, and `https://gw` + `admin/...` is a request to host `gwadmin`.
    let url = format!("{}admin/issue-enrolment", crate::login::normalise(gateway));
    let mut builder = client
        .post(&url)
        .bearer_auth(bearer)
        .json(&serde_json::json!({
            "user_id": user_id,
            "revoke_existing_credentials": revoke_existing,
        }));
    if let Some(instance) = instance {
        builder = builder.header("x-yadgar-instance", instance);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| AdminError::Unreachable(url, e))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    match status {
        s if s.is_success() => {
            serde_json::from_str(&body).map_err(|e| AdminError::Malformed(e.to_string()))
        }
        reqwest::StatusCode::UNAUTHORIZED => Err(AdminError::Refused),
        reqwest::StatusCode::FORBIDDEN => Err(AdminError::NotAdmin(gateway_error(&body))),
        _ => Err(AdminError::Rejected(status, gateway_error(&body))),
    }
}

/// Compare the revocation demand *sent* with the gateway's *echoed* one.
///
/// A different echo is an error. No echo at all is a gateway older than ledger
/// 1579: when revocation was asked for, that gateway may also predate the
/// field and have ignored it, so the old credentials may SURVIVE. That earns a
/// warning, never silence. Not an error: the enrolment itself was issued.
fn check_echo(sent: bool, echoed: Option<bool>) -> Result<Option<String>, AdminError> {
    match echoed {
        Some(echoed) if echoed != sent => Err(AdminError::EchoMismatch { sent, echoed }),
        Some(_) => Ok(None),
        None if sent => Ok(Some(
            "the gateway did not confirm revoke_existing_credentials; an older gateway \
             ignores it, and the user's existing credentials would then keep working"
                .to_string(),
        )),
        None => Ok(None),
    }
}

/// The gateway's `{"error": ...}` text, or the raw body when it sent none,
/// made [`printable`](crate::printable::printable): it ends up on a terminal.
pub(crate) fn gateway_error(body: &str) -> String {
    let text = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    crate::printable::printable(&text)
}

/// Write *token* to a NEW owner-only file at *path*.
///
/// `O_CREAT|O_EXCL` and, on Unix, `O_NOFOLLOW` at mode 0600: an existing file
/// may be the last copy of a token not yet handed over, and a symlink would
/// send the secret wherever it points.
fn write_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{token}")?;
    file.sync_all()
}

#[cfg(test)]
mod tests;

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

#[derive(Subcommand)]
pub enum AdminCommand {
    /// Issue an enrolment token for an existing user: their first enrolment,
    /// or a password reset.
    ///
    /// The person redeems it with `yaadgaar enrol` and chooses their own
    /// password; you never learn it. Calls the gateway's
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

        /// Revoke every credential the user holds when they redeem this
        /// token: a password reset after a lost or leaked machine (ledger
        /// 1569). Off by default, because it also logs out the user's other
        /// machines. The bootstrap token may not ask for it.
        #[arg(long = "revoke-existing")]
        revoke_existing: bool,
    },
}

/// Why an administrative call failed.
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error(transparent)]
    Trust(#[from] crate::trust::TrustError),
    #[error("cannot reach {0}: {1}")]
    Unreachable(String, reqwest::Error),
    /// 401: the stored credential did not resolve.
    #[error("the gateway refused your credential; run `yaadgaar login` again")]
    Refused,
    /// 403: the credential resolved, to someone who is not an administrator.
    #[error("the gateway says you are not an administrator")]
    NotAdmin,
    #[error("the gateway answered {0}: {1}")]
    Rejected(reqwest::StatusCode, String),
    #[error("the gateway's answer was not an enrolment: {0}")]
    Malformed(String),
    /// The gateway echoed a different `revoke_existing_credentials` than was
    /// sent (ledger 1579). The enrolment exists, but it does not do what was
    /// asked, so it is never reported as a success.
    #[error(
        "the gateway issued the enrolment with revoke_existing_credentials={echoed}, \
         but {sent} was asked for; do not hand this token over"
    )]
    EchoMismatch { sent: bool, echoed: bool },
    #[error("--user-id must not be empty")]
    EmptyUserId,
    #[error("cannot write the token to {0}: {1}")]
    TokenFile(PathBuf, std::io::Error),
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

/// Run one `admin` subcommand against the logged-in gateway.
pub async fn run(command: AdminCommand) -> Result<(), AdminError> {
    match command {
        AdminCommand::IssueEnrolment {
            user_id,
            out,
            stdout,
            revoke_existing,
        } => {
            let user_id = require_user_id(&user_id)?;
            let config = Config::load()?;
            let client = crate::trust::client(config.ca_pem(), None)?;
            let issued = request(
                &client,
                config.gateway_url(),
                config.token(),
                config.instance(),
                &user_id,
                revoke_existing,
            )
            .await?;
            if let Some(warning) = check_echo(revoke_existing, issued.revoke_existing_credentials)?
            {
                eprintln!("warning: {warning}");
            }
            // The receipt goes to STDERR in both cases, so `--stdout` leaves
            // the token alone on stdout for whatever reads the pipe.
            match out {
                Some(path) => {
                    write_token_file(&path, &issued.token)
                        .map_err(|e| AdminError::TokenFile(path.clone(), e))?;
                    eprintln!(
                        "enrolment {} issued for user {user_id}; the token is in {}",
                        issued.enrolment_id,
                        path.display()
                    );
                }
                None => {
                    debug_assert!(stdout, "clap requires --out or --stdout");
                    println!("{}", issued.token);
                    eprintln!(
                        "enrolment {} issued for user {user_id}",
                        issued.enrolment_id
                    );
                }
            }
            eprintln!("it is single-use and expires in 24 hours");
            Ok(())
        }
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
        reqwest::StatusCode::FORBIDDEN => Err(AdminError::NotAdmin),
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

/// The gateway's `{"error": ...}` text, or the raw body when it sent none.
fn gateway_error(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string())
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

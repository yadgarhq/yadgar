//! Why an administrative call failed.
//!
//! Split out of `admin.rs` when the file neared its size ceiling. Every
//! variant is one thing the operator can act on, so none is reused for a
//! route whose rule differs.

use std::path::PathBuf;

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
    /// 403: the credential resolved, to someone who may not do this. The
    /// gateway's own text is kept: it says which rule refused (not an
    /// administrator, or the bootstrap token asking to revoke).
    #[error("the gateway refused this as not permitted for you: {0}")]
    NotAdmin(String),
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
    #[error("--user-id must look like yadgar:user:<lowercase uuidv7>; nothing was sent")]
    NotCanonicalUserId,
    /// 403 from `/admin/revoke-credentials`: not an admin, the bootstrap token,
    /// or the caller targeting themselves (ADR-0945).
    #[error(
        "the gateway refused the revocation: {0} (only an administrator may revoke, the \
         bootstrap token may not, and you cannot revoke your own credentials)"
    )]
    RevokeForbidden(String),
    /// 404 from `/admin/revoke-credentials`.
    #[error("the gateway has no such user: {0}")]
    NoSuchUser(String),
    /// 400 from `/admin/revoke-credentials`.
    #[error("the gateway rejected the request as malformed: {0}")]
    BadRequest(String),
    /// 404 with any body but the gateway's "no such user": the route is absent.
    #[error("this gateway does not serve /admin/revoke-credentials (gateway older than #120?)")]
    RouteNotServed,
    #[error("the gateway's answer was not a revocation receipt: {0}")]
    MalformedRevocation(String),
    /// The gateway answered about a different user than the one named. Someone
    /// else's credentials may have been revoked, so it is never a success.
    #[error("the gateway answered for a different user than the one named; check who was revoked")]
    RevokeEchoMismatch,
    /// 401 after the stored expiry has passed (ADR-0972 (3)).
    #[error("your credential expired at {0}; run `yaadgaar login`")]
    Expired(String),
    /// A 404 from a gateway that does not serve the named route.
    #[error("this gateway does not serve /admin/{0} (gateway older than #127?)")]
    RouteAbsent(&'static str),
    /// 404 with the gateway's own words from `/admin/revoke-credential`: an
    /// unknown id and another user's id answer the same (gateway#127).
    #[error("the gateway has no such credential for this user: {0}")]
    NoSuchCredential(String),
    /// 403 from the credential routes: not an administrator, or the bootstrap
    /// token. An administrator MAY revoke their own credential (ADR-0972 (2)).
    #[error(
        "the gateway refused this: {0} (only an administrator may, and the bootstrap \
         token may not)"
    )]
    CredentialForbidden(String),
    #[error("the gateway's answer was not {0}: {1}")]
    MalformedAnswer(&'static str, String),
    /// The receipt names another user or another credential than the one
    /// named. Something else may have been revoked, so it is never a success.
    #[error(
        "the gateway answered for a different credential than the one named; check what \
         was revoked"
    )]
    CredentialEchoMismatch,
    /// The gateway handed back a page token it already gave: following it
    /// would loop forever.
    #[error("the gateway repeated a page token; the listing is incomplete")]
    RepeatedPageToken,
    #[error("--credential-id must not be empty")]
    EmptyCredentialId,
    #[error("cannot write the token to {0}: {1}")]
    TokenFile(PathBuf, std::io::Error),
    #[error("cannot write to the terminal: {0}")]
    Output(#[from] std::io::Error),
}

impl AdminError {
    /// A 401 after the stored expiry names the expiry instead of the guess.
    ///
    /// *expired* is the stored expiry only when it has already passed
    /// ([`crate::expiry::expired_at`]); every other error is unchanged.
    pub(super) fn naming_expiry(self, expired: Option<&str>) -> Self {
        match (self, expired) {
            (Self::Refused, Some(at)) => Self::Expired(at.to_string()),
            (e, _) => e,
        }
    }
}

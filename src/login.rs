//! `yaadgaar login` — the one sign-in a person performs (D72, D73).
//!
//! It asks for the gateway address, a username and a password; exchanges them
//! for a long-lived token; and writes both address and token as one unit.
//!
//! **The password is never stored and never echoed.** It is exchanged for a
//! token and dropped. The token the gateway returns is shown once and held only
//! here; the server keeps a hash.
//!
//! # `enrol` IS A SEPARATE SUBCOMMAND, not a first run of `login`
//!
//! The two look alike and are not the same act. Every difference points one way:
//!
//! * **`login` asks for the gateway; `enrol` must not.** The enrolment token
//!   already carries the address, filled in by `iam` from its own configuration
//!   precisely so nobody assembles it and nobody types it wrong (D73). A first
//!   run of `login` that kept its prompt would ask a person who has never met
//!   this deployment for a hostname they cannot check, while the blob in their
//!   clipboard already says it.
//! * **`login` PRESENTS a password; `enrol` SETS one.** So `enrol` asks twice
//!   and compares: a typo is a lockout rather than a retry, because the whole
//!   point of D73 is that the admin never learns the password and so there is
//!   nobody left to ask for it.
//! * **They are different endpoints, and only one of them says the username.**
//!   `auth/login` takes a username; `auth/enrol` takes a secret and RETURNS the
//!   username, which is the only place a person on their first machine can
//!   learn it.
//! * **A first-run detection would have to guess.** An absent `config.json`
//!   means "never logged in", and it equally means "the file was deleted" and
//!   "this is another machine for an account that already exists" — three
//!   states with different answers behind one signal. `login` is already right
//!   for two of them, and a subcommand somebody types is a statement rather
//!   than an inference.
//!
//! What `login` gains instead is the smaller, honest half: it also honours a CA
//! a previous enrolment stored, so re-logging in on an enrolled machine does not
//! need the certificate installed by hand.

use std::io::{self, Write as _};
use std::path::Path;

use serde::Deserialize;

use crate::config::Config;
use crate::enrolment::{self, EnrolmentError};

mod label;

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
struct EnrolResponse {
    token: String,
    username: String,
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
    #[error(transparent)]
    Enrolment(#[from] EnrolmentError),
    /// One message for a replayed secret and for one that never existed.
    ///
    /// `RedeemEnrolment` is unauthenticated by construction, so an answer that
    /// told the two apart would say whether a given secret ever existed. The
    /// gateway makes them identical; so does this. What it does NOT say is
    /// "invalid username or password", because an enrolment names no username
    /// and that wording sends somebody hunting for an account they do not have
    /// yet.
    #[error("that enrolment secret was refused; it may already have been used")]
    SecretRefused,
    /// Named separately from [`LoginError::Refused`], and it has to be.
    ///
    /// D73 gives an enrolment token 24 hours. An expired one is refused by the
    /// gateway exactly like an unknown or replayed secret — deliberately, so
    /// the endpoint cannot be probed — so the ONE place that can tell a person
    /// their token simply timed out is here, before the request is made. Left to
    /// the server's answer, somebody with a day-old token is told their secret
    /// is wrong and goes looking for a typo.
    #[error("that enrolment token expired; ask for a new one (they last 24 hours)")]
    Expired,
    /// The two typed passwords differed. `enrol` SETS the password, so this is
    /// the only moment a typo can still be caught.
    #[error("those two passwords are not the same; nothing was changed")]
    PasswordMismatch,
    #[error(transparent)]
    Trust(#[from] crate::trust::TrustError),
    /// The address named no scheme at all.
    ///
    /// ADR-0569: a default chosen silently is a value nobody chose, used as
    /// if somebody had. `https` is REQUIRED, never assumed — a bare host is
    /// refused rather than completed to `https` on the person's behalf.
    #[error("{0} names no scheme; write the full address as https://{0}")]
    NoScheme(String),
    /// Every scheme but `https` is refused, with no exemption for loopback.
    ///
    /// A gateway running on a developer's own machine can still be reached
    /// over `https`: the same CA-distribution [`crate::trust`] already uses
    /// for a private deployment works for one running on localhost, so the
    /// exemption is not needed even for that case. It is also not safe to
    /// grant: a carve-out keyed on the hostname "localhost" or "127.0.0.1" is
    /// a rule an attacker can opt into with a hosts-file entry or a local
    /// proxy, not a safety valve reserved for a developer.
    #[error("{url} uses the `{scheme}` scheme; the gateway must be reached over https")]
    InsecureScheme { url: String, scheme: String },
}

/// Prompt, exchange, store.
///
/// A CA a previous enrolment stored is REUSED **for the deployment it came
/// from**, so re-logging in on an enrolled machine does not need the certificate
/// installed by hand. Its absence is not an error: it is the ordinary case on a
/// publicly-trusted deployment.
///
/// **THE GATEWAY MUST MATCH, and that is the same rule `enrol` gets by carrying
/// nothing.** `login` has no CA source, so for the deployment already stored the
/// machine's anchor is the only candidate and reusing it is right. Point `login`
/// at a DIFFERENT address, though, and reusing it hands private deployment A's
/// issuer authority over connections to B — the identical trust transfer that
/// made the fallback wrong on the enrol path, arriving through the one caller
/// that legitimately has a fallback at all. A CA is only ever kept for the
/// deployment it was issued for.
pub async fn login(config_dir: &Path) -> Result<Config, LoginError> {
    let stored = Config::load_from(config_dir).ok();

    let gateway = prompt("Gateway address (e.g. https://gateway.yadgar.internal:18443): ")?;
    let gateway = normalise(&gateway);
    // CHECKED BEFORE ASKING FOR CREDENTIALS. A person whose address is
    // refused should not first type a username and a password that were
    // always going to be sent nowhere.
    require_https(&gateway)?;

    let username = prompt("Username: ")?;
    // Read without echo. A password in the terminal's scrollback outlives the
    // session and reaches whatever records it.
    let password = rpassword::prompt_password("Password: ")?;

    // READ AFTER THE ADDRESS IS KNOWN, because whether it may be used depends on
    // the address.
    let ca_pem = ca_for(stored.as_ref(), &gateway);
    let client = crate::trust::client(ca_pem.as_deref(), None).map_err(LoginError::Trust)?;
    let token = exchange(&client, &gateway, &username, &password).await?;

    let config = reconcile(
        config_dir,
        stored.as_ref(),
        gateway,
        token,
        username,
        ca_pem,
    );
    config.save()?;
    Ok(config)
}

/// The stored CA, but ONLY when it belongs to the address being logged into.
///
/// Pure and separate, so the rule is exercisable without a terminal — `login`
/// reads its address from a prompt, and a rule buried in that function could
/// only ever be asserted by driving stdin.
fn ca_for(previous: Option<&Config>, gateway: &str) -> Option<String> {
    previous
        // NORMALISED ON BOTH SIDES. Everything this client writes is already
        // normalised, so the comparison is sound for configs it produced — but
        // `config.json` is a file a person can edit, and one whose address lost
        // its trailing slash would compare unequal to itself. The CA sitting
        // right there would then be dropped and `login` would fail with a
        // handshake error against the deployment it was issued for.
        .filter(|p| normalise(p.gateway_url()) == gateway)
        .and_then(Config::ca_pem)
        .map(str::to_string)
}

/// Build the config to write, KEEPING what the new credential does not replace.
///
/// **THE INSTALL ID SURVIVES, and that is the whole reason this function
/// exists.** `Config::new` starts from nothing and `save` writes the whole
/// struct, so assembling the new config from the response alone silently
/// dropped the UUID on every re-login — and `serve` then minted a fresh one.
/// ADR-0511 rejected a per-process instance because "nothing could correlate a
/// client across restarts, which rate-limit accounting and audit both need"; an
/// id that a second `login` resets is that rejection reintroduced through
/// another door. A re-enrolment is not hypothetical either: `iam.proto`
/// describes an admin re-issuing a token as a password reset.
///
/// **THE CA IS DECIDED BY THE CALLER AND IS NOT CARRIED FORWARD HERE.** This
/// function used to fall back to the previous config's `ca_pem` whenever the new
/// one was `None`, which is right for `login` and is a TRUST BUG on `enrol`:
///
/// * On the `login` path there is no CA source at all, so `None` means "I was
///   not told" and the machine's existing anchor is the only candidate.
/// * On the `enrol` path `None` is a POSITIVE STATEMENT the contract defines —
///   "this deployment uses a publicly-trusted certificate, use system trust".
///   Carrying a previous anchor over it means that enrolling into deployment B,
///   on a machine previously enrolled into private deployment A, writes A's CA
///   into the config — and the proxy then trusts A for every connection to B.
///   A can mint a certificate for B's hostname and this client accepts it.
///
/// One function cannot hold two opposite correct answers behind one `Option`,
/// so it holds neither: each caller passes what it means, and the difference is
/// visible at the two call sites rather than buried here.
///
/// The USERNAME is not carried either, and does not need to be — both paths
/// always have one, `login` from the prompt and `enrol` from the response — so
/// a fallback would be a branch nothing could ever execute.
///
/// **THE TOOL CACHE IS DROPPED WHEN THE DEPLOYMENT CHANGES.** It holds the last
/// `tools/list` the gateway answered, and it is served on an offline start
/// (D75). Left in place across a change of gateway it is deployment A's tool
/// list presented as B's — the same blast radius as the CA, one directory
/// along. Best effort: a cache that cannot be deleted must not fail a login,
/// and the worst case of deleting one that mattered is a single online fetch.
///
/// TAKES THE PREVIOUS CONFIG AS A PARAMETER rather than loading it, so the rule
/// is exercisable without a terminal — the same reason `Context::discover` takes
/// its directory and `hostname_from` takes its sources.
fn reconcile(
    config_dir: &Path,
    previous: Option<&Config>,
    gateway: String,
    token: String,
    username: String,
    ca_pem: Option<String>,
) -> Config {
    // Normalised on both sides, for the reason `ca_for` gives: a hand-edited
    // address that lost its trailing slash must not read as a different
    // deployment and throw away a cache that is still correct.
    if previous.is_some_and(|p| normalise(p.gateway_url()) != gateway) {
        Config::forget_tool_cache(config_dir);
    }
    let mut config = Config::new(config_dir, gateway, token)
        .with_username(Some(username))
        .with_ca_pem(ca_pem);
    config.set_instance(previous.and_then(Config::instance).map(str::to_string));
    config
}

/// Redeem an enrolment token: set a password, and learn the username.
///
/// **NOTHING IS ASKED THAT THE TOKEN ALREADY ANSWERS.** The address and the CA
/// come out of the blob, so the only questions are the password and its
/// confirmation — which is the whole reason this is not a mode of [`login`].
pub async fn enrol(config_dir: &Path, blob: &str) -> Result<Config, LoginError> {
    let previous = Config::load_from(config_dir).ok();
    let enrolment = enrolment::decode(blob)?;
    if expired(enrolment.expires_at, now()) {
        return Err(LoginError::Expired);
    }
    let gateway = normalise(&enrolment.gateway);
    // CHECKED BEFORE ASKING FOR A PASSWORD, same reason as the expiry check
    // above: nothing here is asked that a refusal would make pointless.
    // `enrol` posts the new password to this same address, so a cleartext
    // scheme is the identical exposure `login` refuses, arriving from a
    // token an admin built rather than an address a person typed.
    require_https(&gateway)?;

    println!("Enrolling with {}", enrolment.gateway);
    if enrolment.ca_pem.is_some() {
        // Said out loud, because it is the difference between this machine
        // trusting one certificate for one host and somebody installing a CA
        // system-wide by hand.
        println!("The token carries a CA; it will be trusted for this gateway only.");
    }
    // Twice, and compared. `enrol` SETS the password — the admin never learns
    // it (D73), so a typo is a lockout with nobody to ask.
    let password = rpassword::prompt_password("Choose a password: ")?;
    if password != rpassword::prompt_password("Repeat it: ")? {
        return Err(LoginError::PasswordMismatch);
    }

    let client =
        crate::trust::client(enrolment.ca_pem.as_deref(), None).map_err(LoginError::Trust)?;
    let redeemed = redeem(&client, &gateway, &enrolment.secret, &password).await?;

    let config = reconcile(
        config_dir,
        previous.as_ref(),
        gateway,
        redeemed.token,
        redeemed.username,
        enrolment.ca_pem,
    );
    config.save()?;
    Ok(config)
}

/// Has this token's hour passed? A token carrying no expiry has not.
///
/// Pure, and split out for that reason: a rule keyed on the wall clock is one
/// nothing can exercise while it reads the clock itself.
fn expired(expires_at: Option<i64>, now: i64) -> bool {
    expires_at.is_some_and(|at| at <= now)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The URL an enrolment goes to. Same join rule as [`login_url`], same reason.
fn enrol_url(gateway: &str) -> String {
    format!("{}{ENROL_PATH}", normalise(gateway))
}

/// Present the secret, set the password, take back a credential and a name.
///
/// **THE SECRET FIELD, NEVER THE WHOLE BLOB.** The contract is explicit, and
/// sending the base64 envelope would present a string the server never hashed —
/// refused as a wrong secret, with the person holding a token that is fine.
async fn redeem(
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
            "label": label::label(),
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

async fn exchange(
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
            "label": label::label(),
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

/// Refuse anything but `https` (ledger 717).
///
/// **PURE, and split out for the reason [`expired`] is:** a rule about which
/// scheme is safe is worth nothing if a test can only ever exercise it
/// through a live socket. Takes an already-[`normalise`]d address so the
/// scheme sits at a fixed offset and the check cannot be fooled by leading
/// whitespace or a doubled slash.
///
/// **NOTHING IS DEFAULTED.** A bare host such as `gw.example.com` names no
/// scheme, and completing it to `https` on the person's behalf is the same
/// silent invented value ADR-0569 refuses for a configuration knob — visible
/// nowhere, and wrong the one time somebody meant `http`. So it is refused
/// exactly like `http://` is, not upgraded.
///
/// **THE CLASSIFICATION IS SHARED with [`crate::config`]'s copy of this
/// check; only the error is not.** [`crate::scheme::scheme_of`] holds the
/// one rule both call sites need agree on; this function's whole job is to
/// turn that classification into the wording that fits a person mid-prompt,
/// which is a different wording from a file `serve` loaded unattended.
fn require_https(gateway: &str) -> Result<(), LoginError> {
    match crate::scheme::scheme_of(gateway) {
        crate::scheme::Scheme::Https => Ok(()),
        crate::scheme::Scheme::Other(scheme) => Err(LoginError::InsecureScheme {
            url: gateway.to_string(),
            scheme: scheme.to_string(),
        }),
        crate::scheme::Scheme::Absent => Err(LoginError::NoScheme(gateway.to_string())),
    }
}

fn prompt(question: &str) -> Result<String, io::Error> {
    print!("{question}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests;

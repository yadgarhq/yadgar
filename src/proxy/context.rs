//! The workspace facts a credential cannot carry (ADR-0511).
//!
//! Split out of `proxy.rs` when the file passed its size ceiling, along the seam
//! that was already there: everything else in that module is about forwarding a
//! message, and this is about WHERE the person forwarding it is sitting.

use std::path::Path;

use crate::config::Config;

/// The two CONTEXT headers `tools/call` requires (ADR-0511).
///
/// **THERE IS NO `x-yadgar-user`, AND THERE MUST NEVER BE ONE.** The gateway
/// resolves the caller from the bearer token via `iam.ResolveCredential` and
/// mints the `Scope` itself, because a self-asserted username is forgeable by
/// anyone holding any valid token — which is what ADR-0488 exists to prevent.
/// Sending the username this client stores would be the smaller diff and the
/// wrong one; it is stored for display and never leaves the machine.
///
/// These two stay caller-supplied because they are workspace facts no
/// credential can carry: which project the person is working in, and which
/// install this is. A token cannot know which directory somebody is sitting in.
pub(super) const HEADER_PROJECT: &str = "x-yadgar-project";
pub(super) const HEADER_INSTANCE: &str = "x-yadgar-instance";

/// The workspace facts, resolved once per process.
///
/// ONCE, not per request: `serve` is spawned by the agent in the directory it
/// is working in and stays there for the session, so a per-request derivation
/// would shell out to git on the latency-critical path (D25) to answer the same
/// question with the same answer. A moved directory is a new process.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Context {
    /// `owner/repo`, derived exactly as the Python client derives it.
    pub project: Option<String>,
    /// This install's UUID, minted at `install` and stored in `config.json`.
    pub instance: Option<String>,
}

impl Context {
    /// Read the project out of the working directory and the instance out of
    /// the config.
    ///
    /// **AN UNRESOLVED VALUE IS OMITTED, NEVER INVENTED.** ADR-0227 deleted
    /// every fallback in the Python client for the reason it gives: a fallback
    /// that cannot fail manufactures a plausible-looking wrong answer, and
    /// nothing downstream can tell it from a real one. A directory with no
    /// project is still a real place to run an agent, so the proxy serves and
    /// the gateway decides what a call without a project may do.
    /// **THE DIRECTORY IS HANDED IN, rather than read here**, and that is the
    /// difference between a test and a hazard. While this read
    /// `std::env::current_dir()` itself, no test could point it anywhere: every
    /// wire test built a `Context` by hand and every derivation test called
    /// `derive` with a path, so NOTHING JOINED THE TWO — deleting the whole
    /// derivation and hardcoding `None` left all 180 tests green, which is the
    /// same shape as the `.bearer_auth(...)` deletion that survived a full
    /// suite. `login::hostname_from` takes its sources as parameters for the
    /// identical reason.
    pub fn discover(config: &Config, cwd: &Path) -> Self {
        let project = sendable(HEADER_PROJECT, crate::project::derive(cwd));
        if project.is_none() {
            tracing::warn!(
                "no project identity for this directory: no .yadgar/project-id was found \
                 walking up from it, and `git config remote.origin.url` produced nothing. \
                 Calls needing a project will be scoped without one."
            );
        }
        Self {
            project,
            instance: sendable(HEADER_INSTANCE, config.instance().map(str::to_string)),
        }
    }
}

/// A value that cannot be a header value is no value at all.
///
/// **BOTH FIELDS, not only the project.** `.yadgar/project-id` is a file a
/// person writes and `config.json` is a file a person can edit, so either can
/// end up holding a line break or a null byte. `reqwest` refuses such a value
/// when the request is BUILT, so the WHOLE request fails and the agent is told
/// `yadgar gateway unreachable` — somebody then goes to look at their network
/// over a character in a file. Dropping the value costs one scope and says why,
/// which is a failure that can be diagnosed.
///
/// THE CHECK IS THE HTTP LIBRARY'S OWN, not a hand-written approximation of it.
/// A field value may legitimately carry obs-text and HTAB (RFC 9110 §5.5), so a
/// guard that "obviously" refused everything outside printable ASCII would drop
/// legitimate values — the same failure, pointing the other way.
pub(super) fn sendable(header: &str, value: Option<String>) -> Option<String> {
    let value = value.filter(|v| !v.is_empty())?;
    if reqwest::header::HeaderValue::from_str(&value).is_err() {
        tracing::warn!(
            "{header} cannot carry {value:?} — it holds a line break or a null byte. \
             Fix the file it came from; requests will be sent without that header \
             until then, rather than failing as an outage."
        );
        return None;
    }
    Some(value)
}

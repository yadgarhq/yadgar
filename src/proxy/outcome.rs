//! What one forwarded request produced, and the refusal token read from it.
//!
//! Split out of `proxy.rs` for the file-size ceiling. `forward` decides which
//! outcome happened; the proxy's `respond` and the `session-start` hook each
//! decide what to say about it.

use serde_json::Value;

/// What one forwarded request produced.
///
/// THREE OUTCOMES, not two, and the missing third one was a real defect.
/// `send().await` returns `Ok` for a 502, and `.text()` then hands back the
/// proxy's HTML error page — so a gateway that was down looked exactly like a
/// gateway that had answered. The page was returned to the agent as a result
/// AND written into `tools-cache.json`, so every later offline start served the
/// HTML back, permanently, until a good call happened to land.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// A success status, and the body that came with it.
    Answered(String),
    /// Reached and refused, and the gateway's own message and reason token
    /// when it sent them.
    ///
    /// The body is still dropped — nothing in an error page is worth showing an
    /// agent, and it is what poisoned the cache. What survives is two fields of
    /// a JSON-RPC error: `error.message`, for a person, and `error.data.reason`,
    /// the machine token a client branches on ([`gateway_reason`]).
    Rejected(reqwest::StatusCode, Option<String>, Option<String>),
    /// Not reached at all.
    Unreachable(String),
}

/// The gateway's `error.data.reason` token, when the body is a JSON-RPC error
/// that carries one.
///
/// THE TOKEN, NOT THE SENTENCE, is what a client may branch on: the gateway
/// keeps the token in a closed set (`gateway/src/project.rs`, `Reason::token`)
/// and rewords the sentence beside it freely.
pub(crate) fn gateway_reason(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .pointer("/error/data/reason")?
        .as_str()
        .filter(|r| !r.is_empty())
        .map(str::to_string)
}

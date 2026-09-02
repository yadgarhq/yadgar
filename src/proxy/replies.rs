//! What the proxy says when it has nothing to forward back.
//!
//! Split out of `proxy.rs` when the file passed its size ceiling. The seam is
//! real rather than a line count: `forward` and `respond` decide WHAT happened,
//! and every function here decides only how to SAY it — which is why they are
//! all pure, and why the wording rules they carry are testable at all.

use serde_json::Value;

/// Re-point a cached response at the request that asked for it.
///
/// A JSON-RPC reply is matched by `id`, so replaying yesterday's response with
/// yesterday's id would be a reply to nothing and the agent would wait forever.
pub(super) fn retarget(cached: &str, id: &Value) -> String {
    match serde_json::from_str::<Value>(cached) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("id".into(), id.clone());
            }
            v.to_string()
        }
        Err(_) => unreachable_error(id, &"the cached tool list is unreadable"),
    }
}

pub(super) fn parse_error() -> String {
    // id is null: the request could not be parsed, so there is no id to answer.
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": serde_json::Value::Null,
        "error": { "code": -32700, "message": "invalid JSON" },
    })
    .to_string()
}

/// What the gateway said, if it said anything a person can act on.
///
/// ONE FIELD, and the narrowness is the point. A refusal body is an error page as
/// often as it is a JSON-RPC error, and an error page is what got cached and
/// served back forever — so anything that is not `error.message` of parseable
/// JSON is dropped exactly as before.
pub(super) fn gateway_message(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("error")?
        .get("message")?
        .as_str()
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

/// The gateway answered, and said no.
///
/// Separated from [`unreachable_error`] because the two call for different
/// actions from the person: check the network, or log in again. A 401 reported
/// as "unreachable" sends somebody to look at their wifi.
///
/// THE STATUS AND THE MESSAGE ARE BOTH KEPT. The status is what tells a request
/// that was wrong from a gateway that is down; the message is the only part that
/// says what to change.
///
/// **`run yaadgaar login again` is a GUESS, and it is only offered when the
/// gateway made none of its own.** A 401 can mean the credential expired, and it
/// can equally mean the request never carried an identity the gateway could
/// attest — for which logging in again changes nothing. Printing the gateway's
/// reason and that instruction together hands the person two answers that
/// contradict each other, and the wrong one is the one phrased as an action. So
/// a refusal that came with a reason shows the reason alone.
pub(super) fn rejected_error(
    id: &Value,
    status: reqwest::StatusCode,
    detail: Option<&str>,
) -> String {
    let credential_was_refused = matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    );
    let message = match (credential_was_refused, detail) {
        (true, Some(detail)) => {
            format!("yadgar gateway rejected the credential ({status}): {detail}")
        }
        (true, None) => {
            format!("yadgar gateway rejected the credential ({status}); run `yaadgaar login` again")
        }
        (false, Some(detail)) => format!("yadgar gateway answered {status}: {detail}"),
        (false, None) => format!("yadgar gateway answered {status}"),
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32603, "message": message },
    })
    .to_string()
}

pub(super) fn unreachable_error(id: &Value, e: &dyn std::fmt::Display) -> String {
    // -32603 internal error, and the message names the GATEWAY rather than
    // reporting a tool failure. The agent should be able to tell "yadgar is
    // unreachable" from "the tool said no", because only one of them is worth
    // telling the person about.
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32603,
            "message": format!("yadgar gateway unreachable: {e}"),
        },
    })
    .to_string()
}

//! The stdio MCP server the agent spawns, forwarding to the gateway.
//!
//! **This module knows no tools.** There is no tool list here, no match arm per
//! tool, no feature flag — `tools/list` is forwarded and its answer returned
//! verbatim, and so is everything else (D75).
//!
//! That is not laziness, it is the boundary. A client-side list is something the
//! client asserts; forwarding makes the gateway's answer the only answer. The
//! gateway resolves identity before it replies, so filtering on `is_admin` (D73)
//! or a disabled plugin (D1) is enforcement rather than a suggestion the client
//! is trusted to honour. It also means adding a tool server-side needs no client
//! release — which matters, because the client lives on people's laptops and the
//! gateway does not.
//!
//! **The agent never sees the credential.** It is attached here, from config, on
//! the way out. That is the whole reason a shim exists rather than the agent
//! talking to the gateway directly: MCP 2026-07-28 is stateless, so something
//! must present a credential on every request, and this makes that something not
//! be the agent.

use std::io::{self, Write as _};

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::config::Config;

/// How long to wait for the gateway before giving up on one request.
///
/// Short, deliberately. The agent is blocked on this: a request that hangs for a
/// minute is worse for the person than one that fails in five seconds and says
/// why, because the agent can report a failure and carry on.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Methods whose answer is cached so the agent can start while offline.
///
/// Only the tool list. A cached `tools/call` would be a fabricated result, which
/// is the one thing a proxy must never produce.
const CACHEABLE: &str = "tools/list";

pub async fn serve(config: Config) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // Line-delimited JSON on stdin, one request per line, in order.
    //
    // Sequential rather than concurrent, and that is a real constraint rather
    // than an oversight: MCP over stdio has no framing that lets responses come
    // back out of order, so answering request two before request one would
    // corrupt the stream. Concurrency belongs at the gateway, which has many
    // replicas; here it would buy nothing and cost correctness.
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle(&client, &config, &line).await;
        if let Some(body) = response {
            let mut out = io::stdout().lock();
            writeln!(out, "{body}")?;
            // Flushed per message. Without it the agent waits on a buffer that
            // fills only when the next request arrives, which reads as the
            // server hanging.
            out.flush()?;
        }
    }
    Ok(())
}

/// Forward one message. Returns `None` for a notification, which takes no reply.
async fn handle(client: &reqwest::Client, config: &Config, line: &str) -> Option<String> {
    // Parsed only far enough to answer two questions: does this need a reply,
    // and is it cacheable. The BODY is forwarded as received — reserialising it
    // would silently normalise a client's JSON and could change a field order or
    // a number's representation the gateway is entitled to see unchanged.
    let parsed: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            // Malformed JSON is the agent's problem to see, not ours to swallow.
            return Some(parse_error());
        }
    };
    let id = parsed.get("id").cloned();
    let method = parsed
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();

    let outcome = forward(client, config, line).await;

    // The cache is read HERE rather than inside [`respond`], and only when it
    // could possibly be used. That is what keeps `respond` a pure function, and
    // `respond` being pure is what makes every rule below testable without a
    // network or a gateway.
    let cached = match &outcome {
        Outcome::Answered(_) => None,
        _ if method == CACHEABLE => config.read_tool_cache(),
        _ => None,
    };

    let answer = respond(&method, id.as_ref(), outcome, cached);
    if let Some(body) = &answer.cache {
        // Best effort: a cache that cannot be written must not fail the request
        // that produced it.
        let _ = config.write_tool_cache(body);
    }
    answer.reply
}

/// What one forwarded request produced.
///
/// THREE OUTCOMES, not two, and the missing third one was a real defect.
/// `send().await` returns `Ok` for a 502, and `.text()` then hands back the
/// proxy's HTML error page — so a gateway that was down looked exactly like a
/// gateway that had answered. The page was returned to the agent as a result
/// AND written into `tools-cache.json`, so every later offline start served the
/// HTML back, permanently, until a good call happened to land.
#[derive(Debug)]
enum Outcome {
    /// A success status, and the body that came with it.
    Answered(String),
    /// Reached and refused. The body is deliberately dropped: nothing in an
    /// error page is worth showing an agent, and it is what poisoned the cache.
    Rejected(reqwest::StatusCode),
    /// Not reached at all.
    Unreachable(String),
}

/// What one message turns into: a line to write, and a body to keep.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    /// The line to write to stdout, or `None` for a notification.
    reply: Option<String>,
    /// The body to store as the tool-list cache, or `None` for every other case.
    cache: Option<String>,
}

/// Decide what to answer and what to keep. Pure — no network, no filesystem.
///
/// Split out because none of these four rules could be exercised otherwise, and
/// all four had been silently broken: the offline fallback, the notification
/// that takes no reply, the cache written on success, and the cache NOT written
/// on failure.
fn respond(
    method: &str,
    id: Option<&serde_json::Value>,
    outcome: Outcome,
    cached: Option<String>,
) -> Answer {
    match outcome {
        Outcome::Answered(body) => Answer {
            cache: (method == CACHEABLE && is_a_usable_answer(&body)).then(|| body.clone()),
            // A notification has no id and takes no reply. Sending one is a
            // response to a request nobody made.
            reply: id.is_some().then_some(body),
        },

        failure => {
            let Some(id) = id else {
                // A notification that failed is still a notification.
                return Answer {
                    reply: None,
                    cache: None,
                };
            };
            // NOTHING IS CACHED ON A FAILURE, on any path below. The cache is
            // the thing an offline start depends on, so a bad write to it is not
            // a degraded answer — it is a permanent one.
            let reply = match failure {
                Outcome::Answered(_) => unreachable!("handled above"),
                Outcome::Rejected(status) => {
                    // OFFLINE, and the tool list is what makes the difference
                    // between "yadgar is down" and "yadgar was never installed"
                    // (D75). Server-side only: a 4xx means this request was
                    // wrong — a rejected credential, a wrong path — and serving
                    // a cached list over it hides the one thing the person could
                    // act on.
                    match cached.filter(|_| method == CACHEABLE && status.is_server_error()) {
                        Some(cached) => {
                            tracing::warn!(
                                "gateway returned {status}; serving the cached tool list"
                            );
                            retarget(&cached, id)
                        }
                        None => rejected_error(id, status),
                    }
                }
                Outcome::Unreachable(e) => match cached.filter(|_| method == CACHEABLE) {
                    Some(cached) => {
                        tracing::warn!("gateway unreachable; serving the cached tool list");
                        retarget(&cached, id)
                    }
                    None => unreachable_error(id, &e),
                },
            };
            Answer {
                reply: Some(reply),
                cache: None,
            }
        }
    }
}

/// Is this body a tool list worth keeping until the gateway comes back?
///
/// A success status is not enough on its own. A captive portal or a corporate
/// proxy answers 200 with an HTML interstitial, and a JSON-RPC error is a
/// perfectly well-formed 200 — either one, cached, is served to every later
/// offline start forever.
fn is_a_usable_answer(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    value.get("error").is_none() && value.get("result").is_some()
}

async fn forward(client: &reqwest::Client, config: &Config, body: &str) -> Outcome {
    let sent = client
        .post(config.gateway_url())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        // The spec requires the client to advertise both, and the server picks.
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        // The credential, attached here and nowhere else. Removing this line is
        // not a degraded proxy, it is the whole reason the proxy exists: the
        // agent never holds or sends a token (D75).
        .bearer_auth(config.token())
        .body(body.to_string())
        .send()
        .await;

    let response = match sent {
        Ok(response) => response,
        Err(e) => return Outcome::Unreachable(e.to_string()),
    };

    // CHECKED, rather than assumed from `send()` succeeding. `send()` is `Ok`
    // for every status the gateway can return.
    let status = response.status();
    if !status.is_success() {
        return Outcome::Rejected(status);
    }
    match response.text().await {
        Ok(body) => Outcome::Answered(body),
        // The status arrived and the body did not — a truncated response is not
        // an answer.
        Err(e) => Outcome::Unreachable(e.to_string()),
    }
}

/// Re-point a cached response at the request that asked for it.
///
/// A JSON-RPC reply is matched by `id`, so replaying yesterday's response with
/// yesterday's id would be a reply to nothing and the agent would wait forever.
fn retarget(cached: &str, id: &serde_json::Value) -> String {
    match serde_json::from_str::<serde_json::Value>(cached) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("id".into(), id.clone());
            }
            v.to_string()
        }
        Err(_) => unreachable_error(id, &"the cached tool list is unreadable"),
    }
}

fn parse_error() -> String {
    // id is null: the request could not be parsed, so there is no id to answer.
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": serde_json::Value::Null,
        "error": { "code": -32700, "message": "invalid JSON" },
    })
    .to_string()
}

/// The gateway answered, and said no.
///
/// Separated from [`unreachable_error`] because the two call for different
/// actions from the person: check the network, or log in again. A 401 reported
/// as "unreachable" sends somebody to look at their wifi.
fn rejected_error(id: &serde_json::Value, status: reqwest::StatusCode) -> String {
    let message = match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            format!("yadgar gateway rejected the credential ({status}); run `yaadgaar login` again")
        }
        other => format!("yadgar gateway answered {other}"),
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32603, "message": message },
    })
    .to_string()
}

fn unreachable_error(id: &serde_json::Value, e: &dyn std::fmt::Display) -> String {
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

#[cfg(test)]
mod tests;

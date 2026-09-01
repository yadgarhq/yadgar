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
//!
//! **The protocol headers are ECHOED out of the envelope, never pinned.** MCP
//! 2026-07-28 mirrors three body fields into headers so an HTTP layer can route
//! and police MCP traffic without parsing a JSON-RPC body, and the gateway
//! CROSS-CHECKS the two: a header disagreeing with the body it mirrors is
//! `-32020 HeaderMismatch`. So the proxy cannot pick a version — it reads each
//! one out of the message it is forwarding. That is the same rule as the tool
//! list: what this module asserts about the protocol is nothing, and the gateway
//! stays the only authority. Pinning would also couple every spec revision to a
//! client release, on laptops, which is exactly what forwarding avoids.

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

/// The `_meta` key carrying the protocol version, and the exact string matters.
///
/// `_meta` keys are REVERSE-DNS NAMESPACED in this revision. A near-miss — plain
/// `protocolVersion` — is not a near-miss to the gateway: it reads as the field
/// being absent, and the request is refused for a reason that names a key the
/// envelope appears to contain.
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";

/// The three headers the body is mirrored into.
///
/// Only the first is required on every POST. The other two are cross-checked
/// when present and are sent anyway: mirroring is what the headers exist for, and
/// a header the gateway would have validated is worth more than one it never
/// sees.
const HEADER_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
const HEADER_METHOD: &str = "Mcp-Method";
const HEADER_NAME: &str = "Mcp-Name";

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
    /// Reached and refused, and the gateway's own message when it sent one.
    ///
    /// The body is still dropped — nothing in an error page is worth showing an
    /// agent, and it is what poisoned the cache. What survives is one field: the
    /// `error.message` of a JSON-RPC error, and nothing else.
    Rejected(reqwest::StatusCode, Option<String>),
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
                Outcome::Rejected(status, detail) => {
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
                        None => rejected_error(id, status, detail.as_deref()),
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

/// What the envelope says, ready to be mirrored into headers.
///
/// Every field is optional and an absent one is OMITTED rather than invented.
/// There is no safe value to substitute: a version the body did not declare is a
/// `HeaderMismatch` the person did not cause, and it hides the real fault. When
/// the envelope declares no version the gateway refuses it on the body — it
/// validates `params._meta` BEFORE it looks at any header — and that refusal
/// names the missing `_meta` key, which is the thing to fix.
#[derive(Debug, Default, PartialEq, Eq)]
struct Mirrored {
    /// `params._meta["io.modelcontextprotocol/protocolVersion"]`.
    version: Option<String>,
    /// The top-level JSON-RPC `method`.
    method: Option<String>,
    /// `params.name` — present for the methods that name a thing.
    name: Option<String>,
}

/// Read the three mirrored values out of one envelope.
///
/// Pure, and separate from [`forward`], so every rule above is exercised without
/// a socket. Reading only: the body itself is still forwarded byte for byte.
fn mirrored(parsed: &serde_json::Value) -> Mirrored {
    let text = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let params = parsed.get("params");
    Mirrored {
        version: text(
            params
                .and_then(|p| p.get("_meta"))
                .and_then(|m| m.get(META_PROTOCOL_VERSION)),
        ),
        method: text(parsed.get("method")),
        name: text(params.and_then(|p| p.get("name"))),
    }
}

async fn forward(client: &reqwest::Client, config: &Config, body: &str) -> Outcome {
    // Read, not rewritten: the body still goes out byte for byte below.
    //
    // Parsed a second time, after [`handle`] — which is the cost of this
    // function being callable with a body and nothing else, and of the socket
    // tests exercising the same path a real message takes. `handle` answers
    // unparseable input before reaching here, so the default below is reached
    // only by a direct call; it stands because an envelope this cannot read is
    // one it must not invent headers for either.
    let mirrored = serde_json::from_str(body)
        .map(|v| mirrored(&v))
        .unwrap_or_default();

    let mut request = client
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
        .bearer_auth(config.token());

    // Required on every POST, and its absence is why nothing this proxy sent
    // ever worked. Echoed rather than pinned — see the module comment.
    for (header, value) in [
        (HEADER_PROTOCOL_VERSION, &mirrored.version),
        (HEADER_METHOD, &mirrored.method),
        (HEADER_NAME, &mirrored.name),
    ] {
        if let Some(value) = value {
            request = request.header(header, value);
        }
    }

    let sent = request.body(body.to_string()).send().await;

    let response = match sent {
        Ok(response) => response,
        Err(e) => return Outcome::Unreachable(e.to_string()),
    };

    // CHECKED, rather than assumed from `send()` succeeding. `send()` is `Ok`
    // for every status the gateway can return.
    let status = response.status();
    if !status.is_success() {
        // The BODY IS READ ON THIS PATH TOO, and only the one field worth
        // showing is kept. `yadgar gateway answered 400 Bad Request` is true and
        // tells nobody what to change; the gateway says exactly what was wrong,
        // in a JSON-RPC error, and dropping it costs the next person a packet
        // capture. An error page is still discarded — [`gateway_message`] keeps
        // nothing that is not a JSON-RPC error message, so the HTML that
        // poisoned the cache has no way back in.
        let detail = match response.text().await {
            Ok(body) => gateway_message(&body),
            Err(_) => None,
        };
        return Outcome::Rejected(status, detail);
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

/// What the gateway said, if it said anything a person can act on.
///
/// ONE FIELD, and the narrowness is the point. A refusal body is an error page as
/// often as it is a JSON-RPC error, and an error page is what got cached and
/// served back forever — so anything that is not `error.message` of parseable
/// JSON is dropped exactly as before.
fn gateway_message(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
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
fn rejected_error(
    id: &serde_json::Value,
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

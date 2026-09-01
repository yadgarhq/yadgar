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

    match forward(client, config, line).await {
        Ok(body) => {
            if method == CACHEABLE {
                // Best effort: a cache that cannot be written must not fail the
                // request that produced it.
                let _ = config.write_tool_cache(&body);
            }
            id.is_some().then_some(body)
        }
        Err(e) => {
            let id = id?;
            // OFFLINE, and the tool list is what makes the difference between
            // "yadgar is down" and "yadgar was never installed" (D75).
            if method == CACHEABLE {
                if let Some(cached) = config.read_tool_cache() {
                    tracing::warn!("gateway unreachable; serving the cached tool list");
                    return Some(retarget(&cached, &id));
                }
            }
            Some(unreachable_error(&id, &e))
        }
    }
}

async fn forward(
    client: &reqwest::Client,
    config: &Config,
    body: &str,
) -> Result<String, reqwest::Error> {
    client
        .post(config.gateway_url())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        // The spec requires the client to advertise both, and the server picks.
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        // The credential, attached here and nowhere else.
        .bearer_auth(config.token())
        .body(body.to_string())
        .send()
        .await?
        .text()
        .await
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_cached_reply_is_retargeted_at_the_new_request() {
        // The failure this prevents: replaying a response carrying the OLD id
        // answers nothing, and the agent blocks forever on a request it thinks
        // is still in flight.
        let cached = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}}).to_string();
        let out = retarget(&cached, &json!(42));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], 42);
        assert!(v["result"]["tools"].is_array());
    }

    #[test]
    fn a_corrupt_cache_becomes_an_error_rather_than_garbage() {
        let out = retarget("{not json", &json!(7));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["error"]["code"], -32603);
    }

    #[test]
    fn an_unreachable_gateway_is_distinguishable_from_a_tool_failure() {
        // A tool that fails returns a RESULT with isError; this returns a
        // protocol error naming the gateway. An agent that cannot tell them
        // apart reports the wrong thing to the person.
        let out = unreachable_error(&json!(1), &"connection refused");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], -32603);
        assert!(v["error"]["message"].as_str().unwrap().contains("gateway"));
        assert!(v["result"].is_null());
    }

    #[test]
    fn malformed_input_answers_with_a_null_id() {
        // There is no id to answer with, and inventing one would be a reply to a
        // request nobody made.
        let v: serde_json::Value = serde_json::from_str(&parse_error()).unwrap();
        assert!(v["id"].is_null());
        assert_eq!(v["error"]["code"], -32700);
    }

    #[test]
    fn only_the_tool_list_is_cacheable() {
        // Pins the rule rather than the constant: caching a tools/call answer
        // would mean fabricating a result, which is the one thing a proxy must
        // never do.
        assert_eq!(CACHEABLE, "tools/list");
        assert_ne!(CACHEABLE, "tools/call");
    }
}

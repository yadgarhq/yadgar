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

    /// The tool list as the gateway returns it: a result, and no error.
    fn a_tool_list() -> String {
        json!({"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"recall"}]}}).to_string()
    }

    #[test]
    fn a_notification_is_not_replied_to() {
        // No id means no reply, on EVERY path. A response to a notification is a
        // reply to a request nobody made, and the agent has nothing to match it
        // against.
        for outcome in [
            Outcome::Answered(a_tool_list()),
            Outcome::Rejected(reqwest::StatusCode::BAD_GATEWAY),
            Outcome::Unreachable("connection refused".into()),
        ] {
            let answer = respond("notifications/initialized", None, outcome, None);
            assert_eq!(answer.reply, None, "a notification was answered");
            assert_eq!(answer.cache, None);
        }
    }

    #[test]
    fn a_successful_tool_list_is_kept_for_the_next_offline_start() {
        let answer = respond(
            "tools/list",
            Some(&json!(1)),
            Outcome::Answered(a_tool_list()),
            None,
        );
        assert_eq!(answer.cache.as_deref(), Some(a_tool_list().as_str()));
        assert_eq!(answer.reply.as_deref(), Some(a_tool_list().as_str()));
    }

    #[test]
    fn nothing_but_the_tool_list_is_ever_kept() {
        // Caching a `tools/call` answer would mean replaying a fabricated
        // result, which is the one thing a proxy must never produce.
        let body = json!({"jsonrpc":"2.0","id":1,"result":{"content":[]}}).to_string();
        let answer = respond("tools/call", Some(&json!(1)), Outcome::Answered(body), None);
        assert_eq!(answer.cache, None);
    }

    #[test]
    fn a_gateway_that_answered_with_a_status_is_not_an_answer() {
        // THE DEFECT THIS EXISTS FOR. `send()` is `Ok` for a 502 and `.text()`
        // hands back the proxy's HTML error page, so the page was returned as a
        // result and written into tools-cache.json — and every later offline
        // start served the HTML, permanently, until a good call happened to land.
        let answer = respond(
            "tools/list",
            Some(&json!(1)),
            Outcome::Rejected(reqwest::StatusCode::BAD_GATEWAY),
            None,
        );
        assert_eq!(answer.cache, None, "an error status was cached");
        let v: serde_json::Value = serde_json::from_str(answer.reply.as_deref().unwrap()).unwrap();
        assert_eq!(v["error"]["code"], -32603);
        assert!(v["result"].is_null());
    }

    #[test]
    fn a_json_rpc_error_is_not_kept_either() {
        // A well-formed 200 carrying an error is the other way to poison the
        // cache, and a status check alone does not catch it.
        let body =
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no"}}).to_string();
        let answer = respond(
            "tools/list",
            Some(&json!(1)),
            Outcome::Answered(body.clone()),
            None,
        );
        assert_eq!(
            answer.cache, None,
            "a JSON-RPC error was cached as a tool list"
        );
        // Still RETURNED: the agent is entitled to see what the gateway said.
        assert_eq!(answer.reply.as_deref(), Some(body.as_str()));
    }

    #[test]
    fn a_page_that_is_not_json_at_all_is_not_kept() {
        // A captive portal or a corporate proxy answers 200 with HTML.
        let answer = respond(
            "tools/list",
            Some(&json!(1)),
            Outcome::Answered("<html><body>502 Bad Gateway</body></html>".into()),
            None,
        );
        assert_eq!(answer.cache, None);
    }

    #[test]
    fn an_unreachable_gateway_serves_the_cached_tool_list() {
        // D75: an empty list is indistinguishable from yadgar never having been
        // installed, and the agent silently loses memory and tasks with nothing
        // to report.
        let answer = respond(
            "tools/list",
            Some(&json!(42)),
            Outcome::Unreachable("connection refused".into()),
            Some(a_tool_list()),
        );
        let v: serde_json::Value = serde_json::from_str(answer.reply.as_deref().unwrap()).unwrap();
        assert_eq!(v["id"], 42, "the cached reply was not retargeted");
        assert_eq!(v["result"]["tools"][0]["name"], "recall");
        assert_eq!(
            answer.cache, None,
            "a cached answer was written back as new"
        );
    }

    #[test]
    fn an_offline_tool_call_is_never_answered_from_the_cache() {
        let answer = respond(
            "tools/call",
            Some(&json!(1)),
            Outcome::Unreachable("connection refused".into()),
            Some(a_tool_list()),
        );
        let v: serde_json::Value = serde_json::from_str(answer.reply.as_deref().unwrap()).unwrap();
        assert_eq!(v["error"]["code"], -32603);
        assert!(
            v["result"].is_null(),
            "a tool call was answered from a cache"
        );
    }

    #[test]
    fn a_rejected_credential_is_not_papered_over_by_the_cache() {
        // A 5xx is the gateway being down and the cache is the right answer. A
        // 401 is THIS REQUEST being wrong, and serving a cached list over it
        // hides the one thing the person could act on.
        let answer = respond(
            "tools/list",
            Some(&json!(1)),
            Outcome::Rejected(reqwest::StatusCode::UNAUTHORIZED),
            Some(a_tool_list()),
        );
        let v: serde_json::Value = serde_json::from_str(answer.reply.as_deref().unwrap()).unwrap();
        assert!(v["result"].is_null());
        let message = v["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("login"),
            "a rejected credential must say what to do about it, got: {message}"
        );
    }

    #[test]
    fn a_gateway_outage_still_serves_the_cache() {
        // The other half of the pair above, so neither arm can be deleted alone.
        let answer = respond(
            "tools/list",
            Some(&json!(1)),
            Outcome::Rejected(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            Some(a_tool_list()),
        );
        let v: serde_json::Value = serde_json::from_str(answer.reply.as_deref().unwrap()).unwrap();
        assert_eq!(v["result"]["tools"][0]["name"], "recall");
    }

    #[tokio::test]
    async fn the_credential_is_attached_to_the_request_that_leaves_this_process() {
        // The reason the shim exists at all: MCP 2026-07-28 is stateless, so
        // something must present a credential on every request, and D75 says
        // that something is never the agent. Deleting `.bearer_auth(...)` is
        // invisible to every pure test, so this reads the bytes off a socket.
        let (addr, served) = crate::testserver::answer_once(
            "200 OK",
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#,
        )
        .await;
        let dir = crate::testserver::scratch_dir("proxy-auth");
        let config = Config::new(&dir, format!("http://{addr}/"), "tok-secret".into());

        let outcome = forward(
            &reqwest::Client::new(),
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        )
        .await;

        let head = served.await.unwrap().to_lowercase();
        assert!(
            head.contains("authorization: bearer tok-secret"),
            "the credential never left the process; the request head was:\n{head}"
        );
        assert!(matches!(outcome, Outcome::Answered(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_five_hundred_and_two_arrives_as_a_refusal_and_not_as_a_body() {
        // End to end over a real socket, because the defect was in the seam
        // between `send()` returning `Ok` and the body being read anyway.
        let (addr, served) =
            crate::testserver::answer_once("502 Bad Gateway", "<html>nginx</html>").await;
        let dir = crate::testserver::scratch_dir("proxy-502");
        let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());

        let outcome = forward(&reqwest::Client::new(), &config, "{}").await;
        let _ = served.await;

        match outcome {
            Outcome::Rejected(status) => assert_eq!(status, 502),
            other => panic!("an error page was taken for an answer: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
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

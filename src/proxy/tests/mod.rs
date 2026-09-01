//! What one forwarded message turns into, decided without a gateway.

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
    let body = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no"}}).to_string();
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

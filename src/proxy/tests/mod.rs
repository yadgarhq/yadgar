//! What one forwarded message turns into, decided without a gateway.

use super::*;
use serde_json::json;

/// The identity and context headers, asserted on the wire (ADR-0511).
mod identity;

/// What terminates at this client rather than at the gateway.
mod session;

/// Telling the host when the gateway's catalogue moved.
mod watch;

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

/// `handle` as [`super::session::Session::message`] calls it, for the tests that
/// were written before a session layer existed.
///
/// It parses the line the same way the loop does and declares no host
/// capabilities, which is the state before any `initialize` has arrived.
async fn forwarded(
    client: &reqwest::Client,
    config: &Config,
    context: &Context,
    line: &str,
) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(line).expect("a fixture parses");
    handle(
        client,
        config,
        context,
        line,
        &parsed,
        &json!({}),
        &super::watch::Catalogue::default(),
    )
    .await
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
    //
    // THE METHOD NAMED HERE IS ONE THAT STILL REACHES THIS FUNCTION.
    // `notifications/initialized` used to be the fixture and no longer arrives:
    // it terminates in `session` and never reaches a gateway, so a test about
    // forwarding it would be asserting over a case that cannot occur.
    for outcome in [
        Outcome::Answered(a_tool_list()),
        Outcome::Rejected(reqwest::StatusCode::BAD_GATEWAY, None),
        Outcome::Unreachable("connection refused".into()),
    ] {
        let answer = respond("notifications/progress", None, outcome, None, None);
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
    let answer = respond(
        "tools/call",
        Some(&json!(1)),
        Outcome::Answered(body),
        None,
        None,
    );
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
        Outcome::Rejected(reqwest::StatusCode::BAD_GATEWAY, None),
        None,
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
        None,
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
        None,
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
        Outcome::Rejected(reqwest::StatusCode::UNAUTHORIZED, None),
        Some(a_tool_list()),
        None,
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
        Outcome::Rejected(reqwest::StatusCode::SERVICE_UNAVAILABLE, None),
        Some(a_tool_list()),
        None,
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
        &Context::default(),
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

/// An envelope carrying the three body fields the gateway mirrors into headers,
/// and NOT ONE OF THEM IS A VALUE A HARDCODE WOULD PRODUCE.
///
/// The version is deliberately not `2026-07-28`. That is the revision the
/// gateway implements and exactly what a client that pinned instead of echoing
/// would write — so a fixture declaring it cannot tell the two apart: replace
/// the extracted version with the literal and the socket test below still
/// passes, while the whole change argues for echoing. The method and the name
/// are picked the same way, so each header can only have come out of the body.
fn an_envelope_no_hardcode_could_produce() -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/read",
        "params": {
            "name": "sentinel-of-the-body",
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "1999-01-01",
                "io.modelcontextprotocol/clientCapabilities": {},
            },
        },
    })
    .to_string()
}

#[tokio::test]
async fn the_protocol_headers_the_gateway_requires_leave_with_the_request() {
    // THE DEFECT THIS EXISTS FOR. The gateway requires `MCP-Protocol-Version`
    // on every POST and cross-checks it, `Mcp-Method` and `Mcp-Name` against
    // the body. The proxy sent none of them, so every forwarded request came
    // back 400 and the proxy did not work at all — while the whole suite
    // passed, because no test read the request off a socket.
    //
    // Every asserted value comes from the fixture and from nowhere else, so
    // this fails for a proxy that sends no header AND for one that sends a
    // pinned value. The canned server validates nothing, which is what lets the
    // envelope declare a version no real gateway would accept.
    let (addr, served) = crate::testserver::answer_once(
        "200 OK",
        r#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete"}}"#,
    )
    .await;
    let dir = crate::testserver::scratch_dir("proxy-headers");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());

    let outcome = forward(
        &reqwest::Client::new(),
        &config,
        &Context::default(),
        &an_envelope_no_hardcode_could_produce(),
    )
    .await;

    let head = served.await.unwrap().to_lowercase();
    assert!(
        head.contains("mcp-protocol-version: 1999-01-01"),
        "the version on the wire is not the one the body declares; the request head was:\n{head}"
    );
    assert!(
        head.contains("mcp-method: resources/read"),
        "the method header is absent or disagrees with the body; the request head was:\n{head}"
    );
    assert!(
        head.contains("mcp-name: sentinel-of-the-body"),
        "the name header is absent or disagrees with the body; the request head was:\n{head}"
    );
    assert!(matches!(outcome, Outcome::Answered(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_refusal_carries_the_gateway_s_own_message() {
    // `yadgar gateway answered 400 Bad Request` is true and unactionable. The
    // gateway says exactly what was wrong in a JSON-RPC error body, and dropping
    // that costs the next person a packet capture.
    let (addr, served) = crate::testserver::answer_once(
        "400 Bad Request",
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"the MCP-Protocol-Version header is required on every POST"}}"#,
    )
    .await;
    let dir = crate::testserver::scratch_dir("proxy-400");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());

    let outcome = forward(
        &reqwest::Client::new(),
        &config,
        &Context::default(),
        &an_envelope_no_hardcode_could_produce(),
    )
    .await;
    let _ = served.await;

    let answer = respond("resources/read", Some(&json!(1)), outcome, None, None);
    let v: serde_json::Value = serde_json::from_str(answer.reply.as_deref().unwrap()).unwrap();
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("400"),
        "the status is what tells a 400 from a 500, got: {message}"
    );
    assert!(
        message.contains("the MCP-Protocol-Version header is required"),
        "the gateway said what was wrong and the proxy dropped it, got: {message}"
    );
    assert_eq!(answer.cache, None, "a refusal was cached");
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

    let outcome = forward(&reqwest::Client::new(), &config, &Context::default(), "{}").await;
    let _ = served.await;

    match outcome {
        Outcome::Rejected(status, _) => assert_eq!(status, 502),
        other => panic!("an error page was taken for an answer: {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_version_is_echoed_out_of_the_envelope_rather_than_pinned() {
    // A PINNED version would disagree with a body declaring another one, and
    // the gateway answers a disagreement with -32020 HeaderMismatch — an error
    // about the proxy, for something the person did not do. Echoing makes the
    // same envelope earn -32022, which names both versions and is the truth.
    let envelope = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" } },
    });
    assert_eq!(
        mirrored(&envelope).version.as_deref(),
        Some("1999-01-01"),
        "the client asserted a version the envelope did not declare"
    );
}

#[test]
fn a_namespaced_key_is_the_only_one_that_counts() {
    // `_meta` keys are reverse-DNS namespaced in this revision, and a plain
    // `protocolVersion` is not a near-miss to the gateway — it reads as absent.
    // Mirroring it would send a header the body does not contain.
    let envelope = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": { "_meta": { "protocolVersion": "2026-07-28" } },
    });
    assert_eq!(mirrored(&envelope).version, None);
}

#[test]
fn nothing_is_mirrored_that_the_envelope_did_not_say() {
    // OMITTED, not invented. There is no safe substitute: a header the body
    // does not carry is a HeaderMismatch nobody asked for, and it hides the
    // real fault — which the gateway names, because it validates the body
    // before it looks at a header.
    let bare = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    assert_eq!(
        mirrored(&bare),
        Mirrored {
            version: None,
            method: Some("tools/list".into()),
            name: None,
        }
    );
    // Not an object, not a string, empty: none of them is a value to send.
    assert_eq!(
        mirrored(&json!({"params": {"name": ""}})),
        Mirrored::default()
    );
    assert_eq!(mirrored(&json!(7)), Mirrored::default());
}

#[test]
fn the_name_is_mirrored_when_the_envelope_names_a_thing() {
    // Keyed on `params.name` rather than on the method, which is what the
    // gateway cross-checks against: it populates its own side from params and
    // compares only when both are present.
    let call = serde_json::from_str::<serde_json::Value>(&an_envelope_no_hardcode_could_produce())
        .unwrap();
    assert_eq!(
        mirrored(&call).name.as_deref(),
        Some("sentinel-of-the-body")
    );
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});
    assert_eq!(mirrored(&list).name, None);
}

#[test]
fn a_refusal_that_came_with_a_reason_does_not_also_guess() {
    // The other half of `a_rejected_credential_is_not_papered_over_by_the_cache`,
    // which pins the no-detail arm. `run yaadgaar login again` is a guess. The
    // literal below is `gateway::attest::AttestError::MissingIdentity`'s
    // message, returned only under `Attestation::TrustedHeaders`; the shipped
    // default, `Attestation::Iam`, draws
    // "request carries no `Authorization: Bearer <token>` header" instead for
    // a missing credential (`gateway::attest::AttestError::MissingCredential`).
    // Either way, logging in again fixes none of it. Two answers, and the
    // wrong one phrased as the action.
    let out = rejected_error(
        &json!(1),
        reqwest::StatusCode::UNAUTHORIZED,
        Some("request is missing the X-Yadgar-User header, which identifies the caller"),
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("X-Yadgar-User"),
        "the gateway's reason was dropped, got: {message}"
    );
    assert!(
        message.contains("401"),
        "the status tells a refused credential from an absent one, got: {message}"
    );
    assert!(
        !message.contains("login"),
        "an instruction that cannot fix this was offered alongside the reason: {message}"
    );
}

#[test]
fn only_a_json_rpc_message_survives_a_refusal_body() {
    // The narrowness is the point. An error page is what got cached and served
    // back forever, so a refusal body that is not a JSON-RPC error keeps
    // contributing nothing.
    assert_eq!(
        gateway_message(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"no _meta"}}"#)
            .as_deref(),
        Some("no _meta")
    );
    assert_eq!(gateway_message("<html>nginx</html>"), None);
    assert_eq!(gateway_message(r#"{"error":{"code":-32602}}"#), None);
    assert_eq!(gateway_message(r#"{"error":{"message":""}}"#), None);
}

#[test]
fn only_the_tool_list_is_cacheable() {
    // Pins the rule rather than the constant: caching a tools/call answer
    // would mean fabricating a result, which is the one thing a proxy must
    // never do.
    assert_eq!(CACHEABLE, "tools/list");
    assert_ne!(CACHEABLE, "tools/call");
}

/// A [`session::Watch`] for the tests that are not about watching.
struct Unwatched;

impl super::session::Watch for Unwatched {
    fn host_connected(&mut self, _capabilities: serde_json::Value) {}
}

#[tokio::test]
async fn a_host_that_sends_no_meta_still_sends_the_gateway_what_it_requires() {
    // THE GATE, IN MINIATURE. Claude Code over stdio sends no `params._meta`. The
    // gateway requires two keys in it and refuses on the BODY, before it reads any
    // header — measured on the Debian test VM against gateway 0.9.37, 2026-09-11:
    //
    //   params._meta["io.modelcontextprotocol/protocolVersion"] is required
    //   params._meta["io.modelcontextprotocol/clientCapabilities"] is required
    //
    // So a header default alone fixes nothing, and this reads the whole request —
    // head AND body — off a socket to prove both halves leave the process.
    let (addr, served) = crate::testserver::answer_once(
        "200 OK",
        r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#,
    )
    .await;
    let dir = crate::testserver::scratch_dir("proxy-filled-meta");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());

    let mut session = super::session::Session::new(Unwatched, super::watch::Catalogue::default());
    let reply = session
        .message(
            &reqwest::Client::new(),
            &config,
            &Context::default(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .await;

    let request = served.await.unwrap().to_lowercase();
    // Taken from the handshake rather than written out, so this test cannot pass
    // by agreeing with a literal that the handshake has since stopped using.
    let declared: serde_json::Value =
        serde_json::from_str(&super::session::initialize_reply(&json!(1))).unwrap();
    let version = declared["result"]["protocolVersion"]
        .as_str()
        .unwrap()
        .to_lowercase();
    assert!(
        request.contains(&format!("mcp-protocol-version: {version}")),
        "no protocol header left the process for an envelope that declared none:\n{request}"
    );
    assert!(
        request.contains("io.modelcontextprotocol/protocolversion"),
        "the body still carries no version, so the gateway refuses it:\n{request}"
    );
    assert!(
        request.contains("io.modelcontextprotocol/clientcapabilities"),
        "the second required key is still absent, so the gateway refuses it:\n{request}"
    );
    assert!(reply.is_some_and(|r| r.contains("result")));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_handshake_acknowledgement_reaches_no_socket() {
    // A NOTIFICATION TAKES NO REPLY, so there is nothing in a return value that
    // could tell a swallowed one from a forwarded one — the only observable
    // difference is whether anything arrived at the gateway. Hence a real socket
    // that nothing must connect to.
    //
    // 300 ms IS NOT A GUESS ABOUT SPEED, it is orders of magnitude above a
    // loopback connect, which is what this is waiting for. A forwarded
    // notification arrives in about a millisecond; this budget being generous is
    // what makes the test's failure mean "nothing was sent" rather than "the
    // machine was busy".
    let (addr, served) = crate::testserver::answer_once("200 OK", "{}").await;
    let dir = crate::testserver::scratch_dir("proxy-session-notification");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());

    let mut session = super::session::Session::new(Unwatched, super::watch::Catalogue::default());
    let reply = session
        .message(
            &reqwest::Client::new(),
            &config,
            &Context::default(),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .await;
    assert_eq!(reply, None, "a notification was answered");

    let arrived = tokio::time::timeout(std::time::Duration::from_millis(300), served).await;
    assert!(
        arrived.is_err(),
        "the gateway was sent a handshake acknowledgement for a handshake it never saw: {arrived:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

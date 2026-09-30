//! What terminates at this client, and what still leaves it.
//!
//! THE GATE THESE EXIST FOR. Measured on the Debian test VM against gateway
//! 0.9.37, 2026-09-11, driving the shipped binary over stdio:
//!
//! - `initialize` with a complete `_meta` → `-32601 unknown method: initialize`
//! - `tools/list` with no `_meta` → 400 `params._meta["…/protocolVersion"] is required`
//! - `tools/list` with only the version → 400 `…["…/clientCapabilities"] is required`
//! - `tools/list` with both, version `2025-11-25` → 400 `this server implements
//!   2026-07-28; the request declares 2025-11-25`
//!
//! The last arm CORRECTS ledger 640, which recorded the version value as never
//! checked. It is checked, so a filled-in version has exactly one acceptable
//! value and the tests below are careful never to assert it as a literal.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::config::Config;
use crate::proxy::revision::*;
use crate::proxy::session::*;
use crate::proxy::watch::Catalogue;
use crate::proxy::Context;

/// A [`Watch`] that records rather than spawns, so "nothing is polled until a
/// host connects" is an ordinary assertion rather than a claim about a task
/// nobody can see.
#[derive(Clone, Default)]
struct Counted {
    starts: Arc<AtomicUsize>,
    declared: Arc<Mutex<Vec<Value>>>,
}

impl Watch for Counted {
    fn host_connected(&mut self, capabilities: Value) {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.declared.lock().expect("a lock").push(capabilities);
    }
}

/// A gateway address with nothing behind it.
///
/// Every message that reaches it comes back `Outcome::Unreachable`, which is the
/// point: a reply that is a real result could not have been forwarded, so this
/// tells "answered here" from "answered by a gateway that agreed" without a
/// server to mock.
fn nowhere() -> Config {
    Config::new(
        &std::env::temp_dir().join("yadgar-session-tests-unused"),
        "http://127.0.0.1:1/".into(),
        "tok".into(),
    )
}

fn initialize(protocol: &str, capabilities: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": protocol,
            "capabilities": capabilities,
            "clientInfo": { "name": "a-host-that-sends-no-meta", "version": "0" },
        },
    })
    .to_string()
}

#[tokio::test]
async fn initialize_is_answered_here_and_never_forwarded() {
    // THE DEFECT THIS EXISTS FOR. The gateway does not implement `initialize` —
    // it is stateless and holds no session — so a forwarded one earned
    // `-32601 unknown method`. Every MCP host sends `initialize` first and stops
    // when it fails, which made the whole stack unusable from any host.
    //
    // The gateway address points at nothing, so a forwarded `initialize` comes
    // back as `yadgar gateway unreachable` and this test goes red.
    let mut session = Session::new(Counted::default(), Catalogue::default());
    let reply = session
        .message(
            &reqwest::Client::new(),
            &nowhere(),
            &Context::default(),
            &initialize("1999-01-01", json!({})),
        )
        .await
        .expect("a host that asked is answered");

    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["id"], 1, "the answer does not match the request");
    assert!(
        v["error"].is_null(),
        "`initialize` was forwarded rather than answered: {reply}"
    );
    assert!(
        v["result"]["protocolVersion"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the handshake named no revision"
    );
    assert_eq!(
        v["result"]["capabilities"]["tools"]["listChanged"],
        json!(true),
        "a client that polls the tool list must say so, or no host re-reads it"
    );
    assert!(
        v["result"]["serverInfo"]["name"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the host was not told which server it is talking to"
    );
}

/// Drive one `initialize` at *requested* through a session and return the
/// revision it was answered with. `None` sends a handshake with no version.
async fn answered_with(requested: Option<Value>) -> String {
    let mut session = Session::new(Counted::default(), Catalogue::default());
    let mut hello: Value = serde_json::from_str(&initialize("placeholder", json!({}))).unwrap();
    match requested {
        Some(v) => hello["params"]["protocolVersion"] = v,
        None => {
            hello["params"]
                .as_object_mut()
                .unwrap()
                .remove("protocolVersion");
        }
    }
    let reply = session
        .message(
            &reqwest::Client::new(),
            &nowhere(),
            &Context::default(),
            &hello.to_string(),
        )
        .await
        .unwrap();
    serde_json::from_str::<Value>(&reply).unwrap()["result"]["protocolVersion"]
        .as_str()
        .expect("the handshake named no revision")
        .to_string()
}

/// The revision a synthesised upstream `_meta` carries — read off `fill_meta`
/// rather than written out, so no test below agrees with a literal.
fn upstream_revision() -> Value {
    let bare = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    let filled: Value = serde_json::from_str(&fill_meta(&bare, &json!({})).unwrap()).unwrap();
    filled["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"].clone()
}

#[tokio::test]
async fn a_legacy_revision_this_client_can_serve_is_echoed() {
    // THE DEFECT THIS EXISTS FOR, measured 2026-09-30: Claude Code 2.1.282 sends
    // `protocolVersion: "2025-11-25"`, was answered `2026-07-28`, and refused the
    // server with "Server's protocol version is not supported". The lifecycle
    // rule is that a server supporting the requested version answers with THAT
    // version. Both legacy revisions are asserted, so a hardcoded `2025-11-25`
    // — the one value a real host happens to send today — is red.
    for requested in ["2025-11-25", "2025-06-18"] {
        assert_eq!(
            answered_with(Some(json!(requested))).await,
            requested,
            "a host asking for a revision this client serves was answered with another"
        );
    }
}

#[tokio::test]
async fn a_revision_this_client_cannot_serve_is_answered_with_the_newest_that_has_a_handshake() {
    // Values no implementation plausibly contains: one older than MCP, one from
    // the future, one absent and one of the wrong type. The spec: answer with a
    // version this server supports, SHOULD be its latest — and the host decides.
    //
    // THE LATEST *THAT HAS `initialize`*. 2026-07-28 removed the handshake, so a
    // host that sends one speaks 2025-11-25 or older, and answering it with the
    // upstream revision is exactly the reply Claude Code 2.1.282 refused. The
    // expected value is the SPEC's newest handshake revision, written here as a
    // spec fact, and asserted apart from the upstream revision.
    let upstream = upstream_revision();
    for requested in [
        Some(json!("1999-01-01")),
        Some(json!("2099-01-01")),
        Some(json!(7)),
        None,
    ] {
        let answered = answered_with(requested.clone()).await;
        assert_eq!(
            answered, "2025-11-25",
            "an unservable request ({requested:?}) was not answered with the newest handshake revision"
        );
        assert_ne!(
            json!(answered),
            upstream,
            "a host that sent `initialize` was answered with a revision that has none"
        );
        assert_ne!(
            Some(json!(answered)),
            requested,
            "an unknown revision was echoed"
        );
    }
}

#[tokio::test]
async fn a_repeated_handshake_negotiates_again() {
    // A host re-sending `initialize` is re-introducing itself, possibly at a
    // different revision, and the capabilities are already re-read on it.
    let mut session = Session::new(Counted::default(), Catalogue::default());
    let (client, config, context) = (reqwest::Client::new(), nowhere(), Context::default());
    for requested in ["2025-06-18", "2025-11-25"] {
        let reply = session
            .message(
                &client,
                &config,
                &context,
                &initialize(requested, json!({})),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap()["result"]["protocolVersion"],
            json!(requested)
        );
    }
    // THE SESSION REMEMBERS THE LATEST, not only the reply: re-introduced at the
    // gateway's own revision, the session is no longer legacy, so a `ping` is
    // forwarded (and, with nothing listening, is unreachable) rather
    // than answered as the legacy session it was a moment ago.
    session
        .message(
            &client,
            &config,
            &context,
            &initialize("2026-07-28", json!({})),
        )
        .await;
    let ping = session
        .message(
            &client,
            &config,
            &context,
            r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#,
        )
        .await
        .unwrap();
    assert!(
        ping.contains("unreachable"),
        "the session kept its first revision after a second handshake: {ping}"
    );
}

#[tokio::test]
async fn a_legacy_ping_is_answered_here_and_a_current_one_is_not() {
    // Relational: the same `ping` line, two sessions differing ONLY in the
    // negotiated revision. The gateway address points at nothing, so a forwarded
    // ping comes back `unreachable` — which is what the current-revision session
    // must produce, and what the legacy one must not.
    let ping = r#"{"jsonrpc":"2.0","id":42,"method":"ping"}"#;
    let (client, config, context) = (reqwest::Client::new(), nowhere(), Context::default());

    let mut legacy = Session::new(Counted::default(), Catalogue::default());
    legacy
        .message(
            &client,
            &config,
            &context,
            &initialize("2025-11-25", json!({})),
        )
        .await;
    let pong: Value = serde_json::from_str(
        &legacy
            .message(&client, &config, &context, ping)
            .await
            .expect("a ping is a request and is answered"),
    )
    .unwrap();
    assert_eq!(pong["id"], json!(42));
    assert_eq!(
        pong["result"],
        json!({}),
        "a legacy ping was not answered here: {pong}"
    );

    let mut current = Session::new(Counted::default(), Catalogue::default());
    current
        .message(
            &client,
            &config,
            &context,
            &initialize("2026-07-28", json!({})),
        )
        .await;
    let forwarded = current
        .message(&client, &config, &context, ping)
        .await
        .unwrap();
    assert!(
        forwarded.contains("unreachable"),
        "a ping in the gateway's own revision was answered locally: {forwarded}"
    );
}

/// The `tools/call` result the gateway actually returned, measured on the VM
/// 2026-09-30 — the fields 2026-07-28 added included.
const MEASURED_CALL: &str = r#"{"id":2,"jsonrpc":"2.0","result":{"content":[{"text":"{}","type":"text"}],"resultType":"complete","structuredContent":{"next_page_token":"","tasks":[]}}}"#;

#[test]
fn nothing_is_rewritten_at_the_gateway_s_own_revision() {
    // Byte for byte, even for bodies a legacy revision could not hold.
    for body in [MEASURED_CALL, INPUT_REQUIRED, SCALAR_STRUCTURED] {
        assert_eq!(shape_for(GATEWAY_REVISION, body), None, "{body}");
    }
}

#[test]
fn fields_a_legacy_result_tolerates_are_left_alone() {
    // A legacy `Result` is an OPEN object (`[key: string]: unknown` in both
    // schemas), so `resultType`, `ttlMs`, `cacheScope` and namespaced `_meta`
    // keys are extra fields, not a shape a legacy host rejects. Stripping them
    // would rewrite every body for nothing.
    let list = r#"{"id":1,"jsonrpc":"2.0","result":{"_meta":{"io.yadgarhq/toolsPollIntervalSeconds":600},"cacheScope":"public","resultType":"complete","tools":[],"ttlMs":600000}}"#;
    for legacy in &HOST_REVISIONS[1..] {
        assert_eq!(shape_for(legacy, MEASURED_CALL), None, "{legacy}");
        assert_eq!(shape_for(legacy, list), None, "{legacy}");
    }
}

#[test]
fn a_result_type_a_legacy_host_cannot_read_becomes_an_error() {
    // `ResultType` is an OPEN union in 2026-07-28 (`"complete" | "input_required"
    // | string`), so the translation is an ALLOWLIST: absent or `complete` pass,
    // every other value — including one invented after this line — is refused.
    for body in [
        r#"{"id":11,"jsonrpc":"2.0","result":{"content":[],"resultType":"sentinel-result-type"}}"#,
        r#"{"id":11,"jsonrpc":"2.0","result":{"content":[],"resultType":5}}"#,
    ] {
        assert_eq!(shape_for(GATEWAY_REVISION, body), None, "{body}");
        for legacy in &HOST_REVISIONS[1..] {
            let shaped: Value =
                serde_json::from_str(&shape_for(legacy, body).expect("rewritten")).unwrap();
            assert_eq!(shaped["id"], json!(11));
            assert!(shaped["result"].is_null(), "{shaped}");
            assert!(shaped["error"]["code"].is_i64(), "{shaped}");
        }
    }
    // Absent is `complete` by the 2026-07-28 rule, and passes untouched.
    let absent = r#"{"id":12,"jsonrpc":"2.0","result":{"content":[]}}"#;
    for legacy in &HOST_REVISIONS[1..] {
        assert_eq!(shape_for(legacy, absent), None);
    }
}

#[test]
fn an_output_schema_without_an_object_root_is_dropped_for_a_legacy_host() {
    // Both legacy schemas type `outputSchema` with `type: "object"` at its root,
    // and a host validating the list rejects ALL of it for one tool that is not.
    // 2026-07-28 allows any root (SEP-2106). The matching `structuredContent` is
    // already dropped by the rule above, so an `outputSchema` left behind would
    // also promise a structured result the host never receives.
    let list = json!({"id": 13, "jsonrpc": "2.0", "result": {"tools": [
        {"name": "scalar", "inputSchema": {"type": "object"},
         "outputSchema": {"type": "string", "description": "sentinel-of-the-schema"}},
        {"name": "untyped", "inputSchema": {"type": "object"},
         "outputSchema": {"$schema": "https://json-schema.org/draft/2020-12/schema", "sentinel": 1}},
        {"name": "object", "inputSchema": {"type": "object"},
         "outputSchema": {"type": "object", "properties": {"sentinelKey": {}}}},
        {"name": "none", "inputSchema": {"sentinelInput": true}},
    ]}})
    .to_string();
    assert_eq!(shape_for(GATEWAY_REVISION, &list), None);
    for legacy in &HOST_REVISIONS[1..] {
        let shaped: Value =
            serde_json::from_str(&shape_for(legacy, &list).expect("rewritten")).unwrap();
        let tools = &shaped["result"]["tools"];
        assert!(tools[0].get("outputSchema").is_none(), "{shaped}");
        assert!(tools[1].get("outputSchema").is_none(), "{shaped}");
        assert_eq!(
            tools[2]["outputSchema"]["properties"]["sentinelKey"],
            json!({}),
            "an object-root schema a legacy host accepts was dropped: {shaped}"
        );
        // `inputSchema` is the gateway's to state (D75), even when odd.
        assert_eq!(tools[3]["inputSchema"], json!({"sentinelInput": true}));
        assert_eq!(tools.as_array().unwrap().len(), 4, "a tool was dropped");
    }
}

/// An MRTR interim result: 2026-07-28 only. No legacy revision can carry it.
const INPUT_REQUIRED: &str = r#"{"id":7,"jsonrpc":"2.0","result":{"resultType":"input_required","inputRequests":{"sentinel-of-the-body":{"method":"elicitation/create"}}}}"#;

/// `structuredContent` as a scalar: legal since 2026-07-28 (SEP-2106), an object
/// in both legacy schemas.
const SCALAR_STRUCTURED: &str = r#"{"id":8,"jsonrpc":"2.0","result":{"content":[{"type":"text","text":"sentinel-of-the-body"}],"resultType":"complete","structuredContent":"sentinel-of-the-body"}}"#;

#[test]
fn an_interim_result_becomes_an_error_a_legacy_host_can_read() {
    // A legacy host reads an `input_required` result as a `CallToolResult`
    // missing its required `content` — a schema failure, or worse an empty
    // success. An explicit JSON-RPC error with the same id is the honest shape.
    for legacy in &HOST_REVISIONS[1..] {
        let shaped: Value =
            serde_json::from_str(&shape_for(legacy, INPUT_REQUIRED).expect("rewritten")).unwrap();
        assert_eq!(shaped["id"], json!(7));
        assert!(shaped["result"].is_null(), "{shaped}");
        assert!(shaped["error"]["code"].is_i64(), "{shaped}");
        assert!(
            shaped["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains(legacy)),
            "the error does not say which revision could not carry it: {shaped}"
        );
    }
}

#[test]
fn a_scalar_structured_result_keeps_its_text_and_loses_what_a_legacy_host_rejects() {
    for legacy in &HOST_REVISIONS[1..] {
        let shaped: Value =
            serde_json::from_str(&shape_for(legacy, SCALAR_STRUCTURED).expect("rewritten"))
                .unwrap();
        assert!(
            shaped["result"].get("structuredContent").is_none(),
            "a scalar the legacy schema types as an object reached the host: {shaped}"
        );
        assert_eq!(
            shaped["result"]["content"][0]["text"],
            json!("sentinel-of-the-body")
        );
        assert_eq!(shaped["id"], json!(8));
    }
}

#[tokio::test]
async fn a_cached_list_served_offline_is_shaped_too() {
    // THE OFFLINE PATH, which bypasses the gateway entirely: what a legacy host
    // reads must be shaped however it was produced. The cache is contrived to
    // hold something only 2026-07-28 can carry, so shaping is observable.
    let dir = crate::testserver::scratch_dir("session-shaped-cache");
    let config = Config::new(&dir, "http://127.0.0.1:1/".into(), "tok".into());
    config.write_tool_cache(INPUT_REQUIRED).unwrap();
    let (client, context) = (reqwest::Client::new(), Context::default());

    let mut legacy = Session::new(Counted::default(), Catalogue::default());
    legacy
        .message(
            &client,
            &config,
            &context,
            &initialize("2025-06-18", json!({})),
        )
        .await;
    let reply = legacy
        .message(
            &client,
            &config,
            &context,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#,
        )
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert!(
        v["error"].is_object(),
        "the cached body reached a legacy host unshaped: {reply}"
    );
    assert_eq!(v["id"], json!(3));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_two_keys_the_gateway_requires_are_both_filled_in() {
    // MEASURED, both of them, and the second is the one the brief for this work
    // did not name: an envelope carrying only the version earns
    // `params._meta["io.modelcontextprotocol/clientCapabilities"] is required`.
    // The key literals are the GATEWAY'S contract rather than this client's
    // choice, which is why they appear here as strings.
    let bare = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let filled: Value = serde_json::from_str(&fill_meta(&bare, &json!({})).unwrap()).unwrap();
    let meta = &filled["params"]["_meta"];
    assert!(
        meta["io.modelcontextprotocol/protocolVersion"].is_string(),
        "the version the gateway validates first is absent: {filled}"
    );
    assert!(
        meta["io.modelcontextprotocol/clientCapabilities"].is_object(),
        "the second required key is absent, so the request still earns 400: {filled}"
    );
    // Untouched otherwise: the method and the id are the host's.
    assert_eq!(filled["method"], json!("tools/list"));
    assert_eq!(filled["id"], json!(1));
}

#[test]
fn a_version_the_envelope_declared_is_never_overwritten() {
    // The module comment on `super::super` promises the version is echoed out of
    // the envelope. Filling in an ABSENT field keeps that promise; rewriting a
    // declared one would break it, and would hide a host's real disagreement
    // with the gateway behind a value this client chose.
    let declared = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": { "_meta": {
            "io.modelcontextprotocol/protocolVersion": "1999-01-01",
            "io.modelcontextprotocol/clientCapabilities": {},
        }},
    });
    assert_eq!(
        fill_meta(&declared, &json!({})),
        None,
        "an envelope that needed nothing was rewritten anyway"
    );
}

#[test]
fn only_the_absent_half_is_filled_in() {
    // The gateway checks the two independently, so this fills them
    // independently. A host that declares one and not the other keeps its own.
    let half = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" } },
    });
    let filled: Value = serde_json::from_str(&fill_meta(&half, &json!({})).unwrap()).unwrap();
    assert_eq!(
        filled["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        json!("1999-01-01"),
        "a declared version was replaced while its neighbour was filled in"
    );
    assert!(filled["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"].is_object());
}

#[test]
fn an_envelope_this_cannot_read_is_not_edited() {
    // `params` that is not an object is a malformed envelope. The gateway names
    // the real fault; a proxy that "fixed" it would forward something the host
    // never sent.
    let odd = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": [1, 2]});
    assert_eq!(fill_meta(&odd, &json!({})), None);
}

#[tokio::test]
async fn the_capabilities_sent_are_the_ones_the_host_declared() {
    // MIRRORED, NOT INVENTED. `{}` would satisfy the gateway's presence check and
    // would also be this client asserting something about a host it can simply
    // quote. The sentinel is a capability name no implementation would contain.
    let mut session = Session::new(Counted::default(), Catalogue::default());
    session
        .message(
            &reqwest::Client::new(),
            &nowhere(),
            &Context::default(),
            &initialize(
                "1999-01-01",
                json!({ "sentinelOfTheHost": { "enabled": true } }),
            ),
        )
        .await;

    // Reached through the session rather than by calling `fill_meta` directly, so
    // the capabilities have to have been REMEMBERED from the handshake.
    let reply = session
        .message(
            &reqwest::Client::new(),
            &nowhere(),
            &Context::default(),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        )
        .await
        .unwrap();
    // The gateway is unreachable, so the reply is an error — the assertion below
    // is about what this client REMEMBERED, which the socket test in `mod.rs`
    // pins on the wire.
    assert!(reply.contains("unreachable"), "unexpected reply: {reply}");

    let bare = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"});
    let filled: Value = serde_json::from_str(
        &fill_meta(&bare, &json!({ "sentinelOfTheHost": { "enabled": true } })).unwrap(),
    )
    .unwrap();
    assert_eq!(
        filled["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"]
            ["sentinelOfTheHost"]["enabled"],
        json!(true),
        "the host's own declaration was replaced with an empty object"
    );
}

#[test]
fn the_handshake_acknowledgement_dies_here_and_nothing_else_does() {
    // THE TEST IS WHETHER THE GATEWAY COULD ACT ON IT. It never saw the
    // `initialize` that `notifications/initialized` confirms and holds no session
    // to advance, so forwarding it is traffic about a conversation it was not
    // part of. Everything else forwards, INCLUDING notifications invented after
    // this line was written — a prefix match would swallow those silently,
    // because a notification takes no reply and nothing would ever report it.
    assert_eq!(
        terminates("initialize", Some(&json!(1)), None),
        Terminates::Here
    );
    assert_eq!(
        terminates("notifications/initialized", None, None),
        Terminates::Silently
    );
    assert_eq!(
        terminates("notifications/progress", None, None),
        Terminates::No
    );
    assert_eq!(
        terminates("notifications/cancelled", None, None),
        Terminates::No
    );
    assert_eq!(terminates("ping", Some(&json!(1)), None), Terminates::No);
    assert_eq!(
        terminates("tools/list", Some(&json!(1)), None),
        Terminates::No
    );
    assert_eq!(
        terminates("tools/call", Some(&json!(1)), None),
        Terminates::No
    );
    // `ping` IS THE GATEWAY'S IN ITS OWN REVISION, which removed it, and this
    // process's in a legacy one, where it is part of the lifecycle.
    assert_eq!(
        terminates("ping", Some(&json!(1)), Some(GATEWAY_REVISION)),
        Terminates::No
    );
    for legacy in &HOST_REVISIONS[1..] {
        assert_eq!(
            terminates("ping", Some(&json!(1)), Some(legacy)),
            Terminates::Pong,
            "a legacy host's ping was forwarded to a gateway that removed it ({legacy})"
        );
    }
    // AN ID CHANGES WHAT IT IS. A request by this name expects an answer, and
    // swallowing it leaves the host waiting for one forever.
    assert_eq!(
        terminates("notifications/initialized", Some(&json!(9)), None),
        Terminates::No
    );
}

#[tokio::test]
async fn a_session_notification_is_answered_with_nothing() {
    let mut session = Session::new(Counted::default(), Catalogue::default());
    let reply = session
        .message(
            &reqwest::Client::new(),
            &nowhere(),
            &Context::default(),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .await;
    assert_eq!(reply, None, "a notification was answered");
}

#[tokio::test]
async fn nothing_is_polled_until_a_host_completes_a_handshake() {
    // AN IDLE INSTALL MUST COST NOTHING. `serve` runs for as long as an agent
    // session does, and a watch that started on process start would poll a
    // gateway on behalf of a host that may never speak.
    let watch = Counted::default();
    let (starts, declared) = (watch.starts.clone(), watch.declared.clone());
    let mut session = Session::new(watch, Catalogue::default());
    let (client, config, context) = (reqwest::Client::new(), nowhere(), Context::default());

    session
        .message(
            &client,
            &config,
            &context,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .await;
    assert_eq!(
        starts.load(Ordering::SeqCst),
        0,
        "a watch started for a host that never completed a handshake"
    );

    session
        .message(
            &client,
            &config,
            &context,
            &initialize("1999-01-01", json!({})),
        )
        .await;
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "a host connected and nothing began watching the catalogue"
    );

    // ONCE PER PROCESS. A host re-introducing itself is not a second host, and a
    // second watch would double the gateway traffic per extra handshake.
    session
        .message(
            &client,
            &config,
            &context,
            &initialize("1999-01-01", json!({})),
        )
        .await;
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "a repeated handshake started a second watch"
    );
    assert_eq!(declared.lock().unwrap().len(), 1);
}

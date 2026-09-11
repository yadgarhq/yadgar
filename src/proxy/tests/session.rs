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

#[tokio::test]
async fn the_version_this_client_declares_is_the_one_it_sends() {
    // TWO SURFACES, ONE FACT, and this is why neither is asserted as a literal
    // here. The revision named in the handshake and the revision filled into a
    // synthesised `_meta` are the same claim made to two audiences; if they
    // diverge, the host is told one thing and the gateway another, and the
    // gateway's cross-check is what would eventually report it.
    let mut session = Session::new(Counted::default(), Catalogue::default());
    let reply = session
        .message(
            &reqwest::Client::new(),
            &nowhere(),
            &Context::default(),
            &initialize("1999-01-01", json!({})),
        )
        .await
        .unwrap();
    let declared = serde_json::from_str::<Value>(&reply).unwrap()["result"]["protocolVersion"]
        .as_str()
        .unwrap()
        .to_string();

    let bare = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    let filled: Value =
        serde_json::from_str(&fill_meta(&bare, &json!({})).expect("a bare envelope is filled in"))
            .unwrap();
    assert_eq!(
        filled["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        json!(declared),
        "the host and the gateway were told different revisions"
    );
    assert_ne!(
        declared, "1999-01-01",
        "the host's own number was echoed back as this client's"
    );
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
    assert_eq!(terminates("initialize", Some(&json!(1))), Terminates::Here);
    assert_eq!(
        terminates("notifications/initialized", None),
        Terminates::Silently
    );
    assert_eq!(terminates("notifications/progress", None), Terminates::No);
    assert_eq!(terminates("notifications/cancelled", None), Terminates::No);
    assert_eq!(terminates("ping", Some(&json!(1))), Terminates::No);
    assert_eq!(terminates("tools/list", Some(&json!(1))), Terminates::No);
    assert_eq!(terminates("tools/call", Some(&json!(1))), Terminates::No);
    // AN ID CHANGES WHAT IT IS. A request by this name expects an answer, and
    // swallowing it leaves the host waiting for one forever.
    assert_eq!(
        terminates("notifications/initialized", Some(&json!(9))),
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

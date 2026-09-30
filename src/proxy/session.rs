//! The session layer: what this client answers rather than forwards.
//!
//! **THIS IS NOT A SECOND TOOL CATALOGUE, and the distinction is the whole of
//! D75.** "The client knows no tools" forbids inventing a catalogue, because a
//! catalogue is the gateway's to state and only the gateway can resolve who is
//! asking (D73, D1). It does not forbid answering a question about THIS PROCESS.
//! `initialize` asks the local MCP server who it is and what it can do, and the
//! gateway cannot answer that: it is stateless, holds no session, and is not the
//! thing the host spawned. Forwarding it produced `-32601 unknown method:
//! initialize` — measured on the Debian test VM, 2026-09-11 — and every MCP host
//! sends `initialize` first and stops when it fails, so the whole stack was
//! unusable from any host while every test passed.
//!
//! So the split is: SESSION methods terminate here, and the tool catalogue still
//! always comes from the gateway. Nothing below answers `tools/list`, and
//! nothing below decides what a tool is.

use serde_json::{json, Value};

use super::{Outcome, META_PROTOCOL_VERSION};

use super::revision::{mirrored_capabilities, negotiate, shape_for, GATEWAY_REVISION};

/// The second `_meta` key the gateway requires, beside the version.
///
/// PRESENCE-CHECKED RATHER THAN READ, by the gateway's own comment: an empty
/// object is a valid value meaning "no capabilities", which is different from
/// not saying. Measured on the VM: an envelope carrying only the version earns
/// `params._meta["io.modelcontextprotocol/clientCapabilities"] is required`, so
/// filling in one key and not the other fixes nothing.
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

/// The method that asks who the local MCP server is.
const INITIALIZE: &str = "initialize";

/// The notifications that die here, and NOTHING ELSE DOES.
///
/// **AN EXPLICIT SET, not a `notifications/` prefix match.** The test is whether
/// the GATEWAY COULD ACT ON IT. `notifications/initialized` acknowledges a
/// handshake that terminated in this process: the gateway never saw the
/// `initialize` it confirms, holds no session to advance, and answers it 400
/// because a notification carries no `_meta` either — traffic it cannot parse,
/// sent about a conversation it was not part of.
///
/// A prefix match would also swallow every notification invented after this line
/// was written, including ones the gateway does implement, and it would do it
/// silently: a notification takes no reply, so nothing anywhere would report the
/// loss. Defaulting to FORWARD costs at most a refusal nobody sees, and that is
/// the cheaper direction to be wrong in.
///
/// `notifications/cancelled` and `notifications/progress` are therefore still
/// forwarded, deliberately. `ping` is forwarded too, EXCEPT in a session
/// negotiated at a legacy revision — see [`Terminates::Pong`].
const TERMINATED_HERE: [&str; 1] = ["notifications/initialized"];

/// The liveness request of the legacy lifecycle.
const PING: &str = "ping";

/// What this client does with one message before any socket is involved.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Terminates {
    /// `initialize`: answered from what this client knows about itself.
    Here,
    /// A session notification: nothing is sent and nothing is answered.
    Silently,
    /// `ping` in a session negotiated at a legacy revision: answered `{}` here.
    ///
    /// **THE REVISION DECIDES WHO THE COUNTERPART IS.** In 2025-06-18 and
    /// 2025-11-25 `ping` is part of the lifecycle and the receiver MUST answer it
    /// promptly with an empty result — and the receiver is the process the host
    /// spawned, which is this one. 2026-07-28 removed `ping`, so the gateway
    /// answers a forwarded one `-32601 unknown method: ping` (measured on the VM,
    /// 2026-09-30), which a legacy host may read as a dead server. Answering it
    /// here claims only this process's liveness, which is what a legacy `ping`
    /// asks about. In a session at the gateway's own revision `ping` does not
    /// exist and is forwarded, so the gateway still names it.
    Pong,
    /// The gateway's, as everything about tools always is.
    No,
}

/// Decide where one message ends. Pure, so the rule is exercised without a
/// socket — which is the only way to tell "answered locally" from "forwarded to
/// a gateway that happened to answer the same way".
///
/// *negotiated* is the revision the host was answered with, `None` before any
/// handshake.
pub(super) fn terminates(method: &str, id: Option<&Value>, negotiated: Option<&str>) -> Terminates {
    if method == INITIALIZE {
        return Terminates::Here;
    }
    if method == PING && id.is_some() && negotiated.is_some_and(|n| n != GATEWAY_REVISION) {
        return Terminates::Pong;
    }
    // A NOTIFICATION CARRIES NO ID. One of these names arriving WITH an id is
    // not the notification it looks like — it is a request the host expects an
    // answer to, and swallowing it leaves that host waiting forever. Forward it
    // and let the gateway say what it is.
    if id.is_none() && TERMINATED_HERE.contains(&method) {
        return Terminates::Silently;
    }
    Terminates::No
}

/// The `initialize` result: this client, and what it undertakes to do.
///
/// **`tools.listChanged` IS DECLARED BECAUSE IT IS BUILT.** A capability is a
/// promise the host acts on — declaring `listChanged` and never sending the
/// notification means a host that trusts it never re-reads the catalogue, which
/// is worse than not declaring it at all. [`super::watch`] is the half that keeps
/// this honest.
///
/// The version reported is *negotiated*, from [`negotiate`] — the host's own
/// when this client can serve it (ADR-0821, superseding ADR-0661). ADR-0661
/// answered with the gateway's revision unconditionally, reasoning that echoing
/// the host's number would claim a revision this client does not implement; its
/// revisit trigger fired on a real host (Claude Code 2.1.282 refuses
/// `2026-07-28`), and the claim is now true for every revision in
/// [`super::revision::HOST_REVISIONS`] because [`shape_for`] makes it so.
pub(super) fn initialize_reply(id: &Value, negotiated: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": negotiated,
            "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": {
                "name": env!("CARGO_PKG_NAME"),
                // `CARGO_PKG_VERSION` IS THE RIGHT SOURCE *HERE*, and it is the
                // wrong one in the service repositories — theirs is a
                // placeholder nothing writes (ledger 670, 789) and their version
                // is stamped from the release tag. This crate's manifest version
                // is real and is what the wheel is published at, so a later
                // reader should not "fix" this into a tag lookup.
                "version": env!("CARGO_PKG_VERSION"),
            },
        },
    })
    .to_string()
}

/// Fill in the `_meta` fields the gateway requires and the host never sent.
///
/// Returns `None` when there was nothing to add — and that `None` is what keeps
/// [`super::forward`]'s promise that the body goes out byte for byte. A message
/// this touches is reserialised; every other message is not.
///
/// **ABSENT FIELDS ONLY. A DECLARED VALUE IS NEVER OVERWRITTEN.** The two halves
/// are filled independently, because the gateway checks them independently. A
/// host that declares a version this client does not speak must earn
/// `-32022 UnsupportedProtocolVersion` naming its own number; rewriting it to
/// something the gateway accepts would make this client answer a question the
/// host never asked, and hide the disagreement from both ends.
///
/// `params` that is not an object is left entirely alone: an envelope this cannot
/// read is one it must not edit either, and the gateway's refusal names the real
/// fault.
pub(super) fn fill_meta(parsed: &Value, capabilities: &Value) -> Option<String> {
    let params = parsed.get("params");
    let declared = params.and_then(|p| p.get("_meta"));
    let has_version = declared
        .and_then(|m| m.get(META_PROTOCOL_VERSION))
        .is_some();
    let has_capabilities = declared
        .and_then(|m| m.get(META_CLIENT_CAPABILITIES))
        .is_some();
    if has_version && has_capabilities {
        return None;
    }
    if params.is_some_and(|p| !p.is_object()) {
        return None;
    }

    let mut filled = parsed.clone();
    let meta = filled
        .as_object_mut()?
        .entry("params")
        .or_insert_with(|| json!({}))
        .as_object_mut()?
        .entry("_meta")
        .or_insert_with(|| json!({}))
        .as_object_mut()?;
    if !has_version {
        meta.insert(META_PROTOCOL_VERSION.into(), json!(GATEWAY_REVISION));
    }
    if !has_capabilities {
        meta.insert(META_CLIENT_CAPABILITIES.into(), capabilities.clone());
    }
    Some(filled.to_string())
}

/// The `tools/list` request the watch sends on its own behalf.
///
/// Its `id` is deliberately not a number a host would mint: this reply never
/// reaches the host, and an id that could collide with one in flight is a trap
/// for whoever reads a packet capture later.
pub(super) fn poll_request(capabilities: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": "yaadgaar/tool-list-watch",
        "method": super::CACHEABLE,
        "params": {
            "_meta": {
                META_PROTOCOL_VERSION: GATEWAY_REVISION,
                META_CLIENT_CAPABILITIES: capabilities.clone(),
            },
        },
    })
    .to_string()
}

/// Something that wants to know when a host has finished a handshake.
///
/// A TRAIT RATHER THAN A DIRECT `tokio::spawn`, because "nothing is polled until
/// a host connects" is otherwise an unassertable claim: a test cannot see a task
/// that was never spawned. With the start behind one method, a counting fake
/// makes the rule an ordinary assertion — and moving the call out of the
/// `initialize` arm reddens it.
pub(super) trait Watch {
    /// A host completed `initialize`, declaring *capabilities*.
    fn host_connected(&mut self, capabilities: Value);
}

/// One host's conversation with this client.
///
/// Holds the two things a stateless proxy still has to remember: whether a host
/// has connected, and what it said it can do.
pub(super) struct Session<W: Watch> {
    watch: W,
    /// What the host was last shown, shared with the watch that compares
    /// against it.
    catalogue: super::watch::Catalogue,
    connected: bool,
    /// The revision the host was answered with at its last `initialize`, and so
    /// the shape every reply to it must take. `None` until a host says hello.
    negotiated: Option<&'static str>,
    /// What the host declared at `initialize`, mirrored into every synthesised
    /// `_meta` — so `clientCapabilities` is something a host actually said
    /// rather than a value this client made up. `{}` until a host says
    /// otherwise, which is the gateway's own documented "no capabilities".
    capabilities: Value,
}

impl<W: Watch> Session<W> {
    pub(super) fn new(watch: W, catalogue: super::watch::Catalogue) -> Self {
        Self {
            watch,
            catalogue,
            connected: false,
            negotiated: None,
            capabilities: json!({}),
        }
    }

    /// Handle one line. `None` means write nothing, which is what a notification
    /// and a locally-terminated one both earn.
    pub(super) async fn message(
        &mut self,
        client: &reqwest::Client,
        config: &crate::config::Config,
        context: &super::Context,
        line: &str,
    ) -> Option<String> {
        let parsed: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            // Malformed JSON is the agent's problem to see, not ours to swallow.
            Err(_) => return Some(super::parse_error()),
        };
        let id = parsed.get("id").cloned();
        let method = parsed
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match terminates(method, id.as_ref(), self.negotiated) {
            Terminates::Here => {
                // RE-NEGOTIATED ON EVERY HANDSHAKE, like the capabilities below:
                // a host re-sending `initialize` is re-introducing itself, and may
                // do so at a different revision.
                let negotiated = negotiate(parsed.pointer("/params/protocolVersion"));
                self.negotiated = Some(negotiated);
                self.capabilities = mirrored_capabilities(
                    negotiated,
                    parsed
                        .pointer("/params/capabilities")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                );
                // ONCE PER PROCESS. A host re-sending `initialize` is
                // re-introducing itself, not arriving a second time, and a
                // second watch would double the gateway's poll traffic per
                // extra handshake.
                if !self.connected {
                    self.connected = true;
                    self.watch.host_connected(self.capabilities.clone());
                }
                id.as_ref().map(|id| initialize_reply(id, negotiated))
            }
            Terminates::Silently => None,
            Terminates::Pong => id
                .as_ref()
                .map(|id| json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string()),
            Terminates::No => {
                let reply = super::handle(
                    client,
                    config,
                    context,
                    line,
                    &parsed,
                    &self.capabilities,
                    &self.catalogue,
                )
                .await?;
                // SHAPED AFTER EVERY PATH — a live answer, a cached list served
                // offline, and a refusal alike — because the host reads them all
                // at the revision it negotiated.
                match self.negotiated {
                    Some(negotiated) => Some(shape_for(negotiated, &reply).unwrap_or(reply)),
                    None => Some(reply),
                }
            }
        }
    }
}

/// The [`Watch`] that actually polls: it spawns the loop and hands it a fetcher.
pub(super) struct Spawn {
    pub(super) client: reqwest::Client,
    pub(super) config: crate::config::Config,
    pub(super) context: super::Context,
    pub(super) catalogue: super::watch::Catalogue,
    /// WEAK, so the watch cannot hold stdout open past the host that asked for
    /// it — see [`super::watch::poll`].
    pub(super) out: tokio::sync::mpsc::WeakUnboundedSender<String>,
}

impl Watch for Spawn {
    fn host_connected(&mut self, capabilities: Value) {
        let (client, config, context) = (
            self.client.clone(),
            self.config.clone(),
            self.context.clone(),
        );
        let (catalogue, out) = (self.catalogue.clone(), self.out.clone());
        let body = poll_request(&capabilities);
        tokio::spawn(async move {
            let fetch = move || {
                let (client, config, context, body) = (
                    client.clone(),
                    config.clone(),
                    context.clone(),
                    body.clone(),
                );
                async move {
                    match super::forward(&client, &config, &context, &body).await {
                        Outcome::Answered(body) => Some(body),
                        // A gateway that is down or refusing is not a catalogue
                        // that changed. The watch says nothing and waits.
                        _ => None,
                    }
                }
            };
            super::watch::poll(catalogue, fetch, out).await;
        });
    }
}

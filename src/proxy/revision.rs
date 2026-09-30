//! Which MCP revision each side of this client speaks, and the translation
//! between them.
//!
//! **TWO BOUNDARIES, TWO REVISIONS.** The host is answered at the revision it
//! asked for at `initialize`, when this client can serve it; the gateway accepts
//! exactly one. This module holds both facts and the rules that keep a legacy
//! host's view consistent with a 2026-07-28 gateway — kept apart from
//! [`super::session`], which decides what terminates here, because none of this
//! decides that.

use serde_json::{json, Value};

/// The MCP revision this client speaks UPSTREAM, to the gateway.
///
/// **PINNED, and it is the one thing here the envelope does not decide.** The
/// module comment on [`super`] says the proxy asserts nothing about the protocol
/// and echoes every version out of the message it forwards. That rule still holds
/// for a version somebody else declared; it cannot hold for a version NOBODY
/// declared, which is the case that made the client unusable: Claude Code over
/// stdio sends no `params._meta`, the gateway requires two keys in it, and a
/// proxy with nothing to mirror sent nothing and earned 400 on every request.
///
/// A version is not a fact about the caller that this client would be forging by
/// stating it — it is a fact about this client, which genuinely does speak this
/// revision to the gateway. The alternative was the gateway trusting the bare
/// HTTP header, and that was rejected because it moves a trust boundary: the body
/// is what the gateway validates and cross-checks, and a header believed on its
/// own is a header a proxy can rewrite without the body disagreeing.
///
/// **NOT THE REVISION THE HOST IS ANSWERED WITH.** That one is negotiated per
/// session — see [`HOST_REVISIONS`]. The two are separate on purpose: this client
/// is a translator between two protocol boundaries, and the gateway accepts
/// exactly one revision whatever the host speaks. Passing the negotiated revision
/// upstream would earn `-32022 UnsupportedProtocolVersion` on every call.
///
/// NOT A CONFIGURATION KNOB, so ADR-0569 does not reach it. It is a contract
/// bound: the two ends must agree on one revision, and an installation varying
/// it would make callers get different answers about what a field means — the
/// exact case that ADR's own consequences carve out.
pub(super) const GATEWAY_REVISION: &str = "2026-07-28";

/// The revisions this client can SERVE A HOST at, newest first.
///
/// **MEASURED, and it is why this list exists.** Claude Code 2.1.282 over stdio
/// sends `initialize` with `protocolVersion: "2025-11-25"` and disconnects with
/// "Server's protocol version is not supported: 2026-07-28" when answered with
/// the gateway's revision — measured on the Debian test VM, 2026-09-30. The
/// lifecycle section of every revision that has `initialize` says a server that
/// supports the requested version MUST answer with that same version, and one
/// that does not answers with another it supports, SHOULD be its latest.
///
/// A revision is listed only when everything this client relays can be
/// represented in it: `initialize`, `ping`, `tools/list`, `tools/call` and
/// `notifications/tools/list_changed`. Both legacy revisions define all five, and
/// their `Result` is an OPEN object (`[key: string]: unknown`), so the fields
/// 2026-07-28 added to results — `resultType`, `ttlMs`, `cacheScope`, the
/// namespaced `_meta` keys — are extra fields a legacy host must tolerate rather
/// than a shape it rejects. What a legacy revision CANNOT hold is translated by
/// [`shape_for`]. `2025-03-26` is not listed. Its `Result` is open too, so it is
/// not known to break; but its tool shapes (no `structuredContent`, no
/// `outputSchema`) have not been measured against a host through this client,
/// and no host this client serves asks for it. Listing it is a later measurement,
/// not a spec obstacle.
pub(super) const HOST_REVISIONS: [&str; 3] = [GATEWAY_REVISION, "2025-11-25", "2025-06-18"];

/// The revision an `initialize` naming nothing servable is answered with.
///
/// **THE NEWEST REVISION THAT HAS `initialize`, not the newest this client
/// speaks.** 2026-07-28 removed the handshake (SEP-2575), so a host sending one
/// speaks 2025-11-25 or older; answering it with 2026-07-28 is exactly the reply
/// Claude Code 2.1.282 refused. The spec's "SHOULD be the latest" is read as the
/// latest a handshaking host could possibly accept.
const HANDSHAKE_FALLBACK: &str = "2025-11-25";

/// The client capabilities that invite the server to ask the HOST for something.
///
/// In 2026-07-28 the server asks through an `input_required` result (MRTR),
/// which a legacy session cannot carry — [`shape_for`] turns it into an error.
/// So in a legacy session they are withheld from the upstream mirror, and the
/// gateway refuses a tool that needs one with `MissingRequiredClientCapability`
/// instead of starting an exchange that cannot finish.
const RELAYED_REQUEST_CAPABILITIES: [&str; 3] = ["elicitation", "sampling", "roots"];

/// What of the host's declared capabilities is mirrored upstream.
pub(super) fn mirrored_capabilities(negotiated: &str, declared: Value) -> Value {
    match declared {
        Value::Object(mut map) if negotiated != GATEWAY_REVISION => {
            for withheld in RELAYED_REQUEST_CAPABILITIES {
                map.remove(withheld);
            }
            Value::Object(map)
        }
        other => other,
    }
}

/// Pick the revision to answer a host's `initialize` with.
///
/// The requested one when this client can serve it, otherwise
/// [`HANDSHAKE_FALLBACK`] — the spec's rule, and the host decides whether it can
/// live with that. `None` or a non-string is a request that named no version this
/// client could honour, and gets the same answer as an unknown one.
pub(super) fn negotiate(requested: Option<&Value>) -> &'static str {
    requested
        .and_then(Value::as_str)
        .and_then(|asked| HOST_REVISIONS.iter().find(|served| **served == asked))
        .copied()
        .unwrap_or(HANDSHAKE_FALLBACK)
}

/// Translate one gateway reply into the shape the negotiated revision defines.
///
/// `None` means the body goes to the host unchanged, byte for byte — always so
/// at [`GATEWAY_REVISION`], and so at a legacy revision whenever there is nothing
/// it cannot hold.
///
/// **ONLY WHAT A LEGACY SCHEMA CANNOT HOLD.** Extra fields stay (see
/// [`HOST_REVISIONS`]). What changes:
///
/// - a `resultType` other than absent or `complete` becomes a JSON-RPC error
///   naming the revision. `ResultType` is an OPEN union in 2026-07-28, so this is
///   an allowlist; `input_required` (MRTR) is the known case, and a legacy host
///   would read it as a `CallToolResult` missing its required `content`. The
///   error loses `inputRequests`: relaying them needs a host-facing request
///   loop this client does not have;
/// - a `structuredContent` that is not an object is dropped: both legacy schemas
///   type it as an object, and the spec already has the server repeat it as text
///   in `content`, so nothing the model needs is lost;
/// - in `tools/list`, an `outputSchema` whose root is not `type: "object"` is
///   dropped: both legacy schemas require that root, a strict host rejects the
///   whole list over one tool, and a declared schema promises a structured
///   result the rule above would drop anyway.
///
/// **A TOOL'S `inputSchema` IS NOT REWRITTEN**, although 2026-07-28 loosened it
/// and both legacy schemas require `type: "object"` at its root. Rewriting it is
/// deciding what a tool accepts, which is the gateway's to state (D75); the
/// gateway's tools all declare an object root today, and a host that rejects one
/// that does not is reporting a real disagreement this client cannot settle.
/// `outputSchema` differs: dropping it withholds a promise, it does not change
/// what the tool accepts.
pub(super) fn shape_for(negotiated: &str, body: &str) -> Option<String> {
    if negotiated == GATEWAY_REVISION {
        return None;
    }
    let mut parsed: Value = serde_json::from_str(body).ok()?;
    let result = parsed.get_mut("result")?.as_object_mut()?;
    if let Some(kind) = result
        .get("resultType")
        .filter(|kind| *kind != "complete")
        .cloned()
    {
        return Some(
            json!({
                "jsonrpc": "2.0",
                "id": parsed.get("id").cloned().unwrap_or(Value::Null),
                "error": {
                    "code": -32603,
                    "message": format!(
                        "the yadgar gateway answered with a {kind} result, \
                         which MCP {negotiated} cannot carry to the host"
                    ),
                },
            })
            .to_string(),
        );
    }
    let mut changed = false;
    if result
        .get("structuredContent")
        .is_some_and(|structured| !structured.is_object())
    {
        result.remove("structuredContent");
        changed = true;
    }
    for tool in result
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut)
    {
        if tool
            .get("outputSchema")
            .is_some_and(|schema| schema.get("type") != Some(&json!("object")))
        {
            tool.remove("outputSchema");
            changed = true;
        }
    }
    changed.then(|| parsed.to_string())
}

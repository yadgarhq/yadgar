//! Telling the host when the gateway's tool list has changed.
//!
//! MCP 2026-07-28 is stateless and has no server-initiated stream in this
//! revision, so there is nothing for the gateway to push and nothing for this
//! client to subscribe to. A host that has fetched `tools/list` once therefore
//! holds that answer until something tells it otherwise, and a tool added at the
//! gateway — which needs no client release, by design (D75) — reaches nobody.
//!
//! **THIS IS NOT THE TOOL CACHE, and the two must not be merged.**
//! [`crate::config::Config::read_tool_cache`] is a FALLBACK: written only on a
//! usable answer, read only when the gateway is unreachable or broken, so an
//! agent can start offline. If `tools/list` were served from it on the happy path
//! it would silently become a FRESHNESS cache and hand back a stale catalogue
//! while the gateway was reachable and correct. So the live fetch stays live, the
//! fallback stays a fallback, and change detection lives here instead — on a
//! fingerprint this module keeps in memory for the life of one host connection.
//!
//! A poll does not WRITE that cache either, and that is a choice rather than an
//! omission. The cache exists so the next start has a list, and the host fetches
//! `tools/list` itself at every start — which is the write path, through
//! [`super::respond`], on a request somebody actually made. Writing it from here
//! too would put a second writer on a file whose whole value is that it holds the
//! last answer a host was given.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc::WeakUnboundedSender;

/// What the host is told, and the exact string is the protocol's.
const LIST_CHANGED: &str = "notifications/tools/list_changed";

/// Where the gateway names how often to ask.
///
/// **THE GATEWAY'S NUMBER, AND THIS IS ITS SPELLING RATHER THAN A PROPOSAL.** It is
/// the end that knows how often its catalogue moves, it can change that without a
/// client release, and a client-side figure would be a second source for one fact
/// (ADR-0569). Read off the live wire from `/root/work` against gateway v0.9.38
/// (digest `sha256:beec1a24…`), which shipped the writing half BEFORE this reading
/// half existed:
///
/// ```text
/// result._meta: { "io.yadgarhq/toolsPollIntervalSeconds": 600 }
/// ```
///
/// NOT UNDER `io.modelcontextprotocol/`. That namespace is the spec's, and a key
/// invented inside it is indistinguishable from one the spec defines — the
/// near-miss failure the gateway's own `_meta` comment warns about.
///
/// **SECONDS, AND THE ESTATE IS NOW INCONSISTENT ABOUT UNITS ON PURPOSE.**
/// `server/discover` answers `ttlMs: 3600000`, so milliseconds are the estate's own
/// precedent and this key departs from it. Recorded as a residual rather than
/// fixed: the writing half is a DEPLOYED service, and realigning it would cost a
/// second release for a difference the `Seconds` suffix already makes unambiguous.
/// What matters is that this constant and that suffix agree, which is the thing a
/// reader of one end can check.
///
/// **ONE SPELLING, READ IN ONE PLACE, AND NO TOLERANT FALLBACK.** This client wrote
/// `com.github.yadgarhq/toolListPollMs` first and the gateway wrote the key above:
/// two spellings for one fact, which is the failure that ships SILENT, because a
/// reader that finds neither key simply uses its own default forever with nothing
/// to report. A reader accepting both spellings would hide the next such
/// divergence the same way, so there is exactly one.
pub(super) const POLL_INTERVAL_KEY: &str = "io.yadgarhq/toolsPollIntervalSeconds";

/// How long this client waits when the GATEWAY NAMES NOTHING.
///
/// **THIS IS NOT "THE POLL INTERVAL", and writing it down as one would be the
/// two-sources defect ADR-0569 exists to prevent.** It is the answer to a
/// different question — what this client does in the absence of an answer — and
/// the gateway's own number, whenever it sends one, is the poll interval. The two
/// are never compared and never have to agree.
///
/// Ten minutes is chosen for what it costs rather than for what it catches: an
/// idle laptop makes six requests an hour, and a tool added at the gateway
/// reaches a host inside a coffee break. It is also why the watch runs only while
/// a host is connected — an install nobody is using makes no requests at all.
///
/// **THE DEPLOYED GATEWAY HAPPENS TO NAME THE SAME NUMBER — 600 — AND THAT
/// COINCIDENCE HIDES THE DIFFERENCE BETWEEN THE TWO FACTS.** Measured on the live
/// wire against v0.9.38. So neither a log line nor a test that reads only the
/// LENGTH can tell "the gateway named 600" from "the gateway named nothing and
/// this client fell back to 600" — which is exactly the state a silent key
/// mismatch leaves behind. [`Wait`] therefore carries WHO SAID SO beside the
/// number, and nothing anywhere asserts on 600 alone.
const UNNAMED_INTERVAL: Duration = Duration::from_secs(600);

/// What the host was last shown, and how often to look.
#[derive(Debug, Default)]
struct State {
    /// A fingerprint of the tool list, or `None` before anything has been seen.
    tools: Option<String>,
    /// The interval the gateway named, if it has named one.
    interval: Option<Duration>,
}

/// How long to wait, and WHETHER THE GATEWAY IS WHAT SAID SO.
///
/// **THE SOURCE IS CARRIED BECAUSE THE LENGTH DOES NOT DISTINGUISH IT.** The
/// deployed gateway names 600 seconds and [`UNNAMED_INTERVAL`] is 600 seconds, so
/// the one observation that proves the two halves of this feature actually meet —
/// that a real reply was read — is invisible in the number. It is the difference
/// between a working feature and a silently inert one, so it is a field rather
/// than a comment.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Wait {
    pub(super) how_long: Duration,
    pub(super) named_by_the_gateway: bool,
}

/// The catalogue as this process last saw it, shared by the two things that see
/// it: the request loop, which sees what the host itself fetched, and the watch.
///
/// **THE BASELINE IS WHAT THE HOST WAS LAST SHOWN, and that is the whole reason
/// this is shared rather than private to the watch.** A watch keeping its own
/// first observation as the baseline would say nothing about a change that
/// happened between the host's own `tools/list` and the first poll — the host
/// would hold a catalogue it could never learn was stale.
#[derive(Debug, Default, Clone)]
pub(super) struct Catalogue(Arc<Mutex<State>>);

impl Catalogue {
    /// Record what one tool list said. `true` when the host must be told.
    ///
    /// **THE FIRST LIST IS A BASELINE AND NEVER A CHANGE.** "Changed" is a
    /// statement relative to something, and before anything has been seen there
    /// is no relative to. A host told its catalogue changed the moment it
    /// connected would re-fetch what it had just fetched, every session.
    ///
    /// A body that is not a usable tool list changes nothing and DESTROYS
    /// NOTHING: an error page or a JSON-RPC refusal leaves the baseline exactly
    /// as it was, so the next real answer is still compared against what the host
    /// actually holds.
    pub(super) fn record(&self, body: &str) -> bool {
        let Ok(parsed) = serde_json::from_str::<Value>(body) else {
            return false;
        };
        let Some(tools) = fingerprint(&parsed) else {
            return false;
        };
        let mut state = self.lock();
        if let Some(named) = named_interval(&parsed) {
            state.interval = Some(named);
        }
        let changed = state.tools.as_ref().is_some_and(|seen| *seen != tools);
        state.tools = Some(tools);
        changed
    }

    /// How long to wait before asking again, and who decided.
    ///
    /// ONE LOCK FOR BOTH HALVES. Asking "how long" and "who said so" separately
    /// could straddle a poll that changed the answer, and a log line pairing a
    /// length with the wrong source is worse than one with no source at all.
    pub(super) fn wait(&self) -> Wait {
        match self.lock().interval {
            Some(how_long) => Wait {
                how_long,
                named_by_the_gateway: true,
            },
            None => Wait {
                how_long: UNNAMED_INTERVAL,
                named_by_the_gateway: false,
            },
        }
    }

    /// A poisoned lock is recovered rather than propagated: the state behind it is
    /// two fields with no invariant between them, and a panic in one poll must not
    /// take the MCP server down with it.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A fingerprint of the tool list in one reply, or `None` when there is none.
///
/// ORDER-INSENSITIVE ACROSS TOOLS, KEY-INSENSITIVE WITHIN ONE. The gateway is
/// free to answer the same catalogue with the tools in another order, or with a
/// tool's own fields serialised in another order, and neither is a change the
/// host needs to hear about — while a spurious notification makes every host
/// re-fetch the same list forever.
fn fingerprint(parsed: &Value) -> Option<String> {
    if parsed.get("error").is_some() {
        return None;
    }
    let tools = parsed.get("result")?.get("tools")?.as_array()?;
    let mut each: Vec<String> = tools.iter().map(canonical).collect();
    each.sort_unstable();
    Some(each.join("\n"))
}

/// One value, rendered so that two equal values render equally.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            let rendered: Vec<String> = keys
                .iter()
                .map(|k| format!("{k}={}", canonical(&fields[*k])))
                .collect();
            format!("{{{}}}", rendered.join(","))
        }
        Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", rendered.join(","))
        }
        other => other.to_string(),
    }
}

/// The interval the gateway named in this reply, if it named a usable one.
///
/// Zero is refused along with anything that is not a positive number: a zero
/// interval is a loop with no sleep in it, which would hammer the gateway from
/// every laptop that had a host attached.
fn named_interval(parsed: &Value) -> Option<Duration> {
    let seconds = parsed
        .get("result")?
        .get("_meta")?
        .get(POLL_INTERVAL_KEY)?
        .as_u64()?;
    // SECONDS, because the key says `Seconds` and the writing half is deployed.
    // Reading a seconds value as milliseconds would poll a thousand times too
    // often and would look like a working feature while doing it.
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// The notification, exactly as the protocol words it: no id, because a
/// notification is not answered.
pub(super) fn list_changed() -> String {
    json!({ "jsonrpc": "2.0", "method": LIST_CHANGED }).to_string()
}

/// Ask once, and tell the host only if the answer differs. `false` means the host
/// has gone and there is nobody left to tell.
///
/// **THE SENDER IS WEAK, AND THAT IS WHAT LETS `serve` EXIT.** A strong clone held
/// by this task would keep the write channel open for as long as the task ran,
/// while the task itself only ends when that channel closes — each waiting on the
/// other, with the process hung after the host's stdin reached end of file. A weak
/// handle inverts it: the loop's own sender is the only thing keeping stdout open,
/// and upgrading fails the moment it is dropped.
pub(super) async fn tick_once<F, Fut>(
    catalogue: &Catalogue,
    fetch: &F,
    out: &WeakUnboundedSender<String>,
) -> bool
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    // CHECKED BEFORE THE REQUEST, not only before the send. A host that has gone
    // is not worth asking the gateway on behalf of, and checking here is what
    // bounds the watch's life at one interval past the host's.
    //
    // **UPGRADED TWICE RATHER THAN ONCE, AND THE HANDLE IS NEVER HELD ACROSS THE
    // FETCH.** An upgraded handle is a STRONG sender, so one kept for the length of
    // a request would hold the write channel open for as long as that request
    // took — up to `REQUEST_TIMEOUT` — and `serve` waits on that channel closing
    // before it returns. A clean exit is the entire reason the handle is weak, and
    // holding an upgrade across the await gives half of it back: the process would
    // linger for up to thirty seconds after the host disconnected, in the one case
    // where a poll was in flight. So liveness is a question asked and answered, and
    // the sender that does the sending is a second, short-lived upgrade.
    let host_is_still_there = out.upgrade().is_some();
    if !host_is_still_there {
        return false;
    }
    let Some(body) = fetch().await else {
        // Unreachable or refused. Nothing is known to have changed, so nothing is
        // said — and the next tick asks again.
        tracing::debug!("the gateway did not answer the tool list; nothing to tell the host");
        return true;
    };
    if !catalogue.record(&body) {
        // LOGGED, though nothing happened, and that is the point. A watch doing
        // its job correctly is INVISIBLE: it says nothing to the host and nothing
        // to the log, so "the catalogue has not changed" and "the watch is not
        // running" look identical from outside — including to whoever is asked
        // why a new tool never appeared. One debug line is the difference.
        tracing::debug!("the gateway's tool list is unchanged");
        return true;
    }
    tracing::info!("the gateway's tool list changed; telling the host");
    // The host may have gone WHILE the request was in flight, which is what makes
    // this a second check rather than a formality.
    out.upgrade()
        .is_some_and(|sender| sender.send(list_changed()).is_ok())
}

/// Poll while a host is connected.
///
/// Started from the `initialize` arm and nowhere else, so an install with no host
/// attached makes no requests at all. It ends when the write channel closes,
/// which is what the host's stdin reaching end of file does.
pub(super) async fn poll<F, Fut>(catalogue: Catalogue, fetch: F, out: WeakUnboundedSender<String>)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    loop {
        // READ EVERY TIME ROUND, not captured once: the gateway may name an
        // interval in any reply, and a value read once would pin the first
        // answer's number for the life of the session.
        // **THE FIRST WAIT IS THIS CLIENT'S OWN, EVEN WHEN THE GATEWAY NAMES ONE,
        // and that is worth stating because the log line below makes it visible.**
        // The watch arms at `initialize`, and at that instant no `tools/list` has
        // been answered, so there is no named interval to have read — a fallback is
        // by definition what happens in the absence of an answer. The host's own
        // `tools/list` lands moments later and every wait after the first is the
        // gateway's.
        //
        // Measured on the VM against gateway v0.9.38: the first line of a real
        // session reads `named_by_the_gateway=false` and the next reads `true`.
        // NOT A DEFECT WORTH THE FIX IT WOULD TAKE — interrupting a sleep already in
        // flight — but absolutely worth writing down, because `false` on the first
        // line is what a spelling mismatch between the two ends looks like too, and
        // telling those apart is the entire reason this field exists.
        let wait = catalogue.wait();
        // BOTH THE LENGTH AND THE SOURCE. The deployed gateway names 600 seconds
        // and this client falls back to 600 seconds, so the length alone cannot
        // say whether a reply was ever read — and "the key is spelled differently
        // at the two ends" looks exactly like "the gateway named ten minutes".
        // This line is the only place that difference is observable in the field.
        tracing::debug!(
            interval = ?wait.how_long,
            named_by_the_gateway = wait.named_by_the_gateway,
            "waiting before asking for the tool list again"
        );
        tokio::time::sleep(wait.how_long).await;
        if !tick_once(&catalogue, &fetch, &out).await {
            return;
        }
    }
}

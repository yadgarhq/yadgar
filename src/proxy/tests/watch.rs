//! Telling the host when the catalogue moved, and only then.
//!
//! TIMING IS VIRTUAL HERE. The interval is minutes, and a test that waits one is
//! a test nobody runs — so every test that asserts about waiting takes a paused
//! clock and advances it. They finish in milliseconds and they are deterministic,
//! which a `sleep` in CI is not.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::proxy::watch::*;

/// A tool list as the gateway returns one.
fn a_list(names: &[&str]) -> String {
    let tools: Vec<Value> = names
        .iter()
        .map(|n| json!({ "name": n, "description": "a tool" }))
        .collect();
    json!({"jsonrpc": "2.0", "id": 1, "result": { "tools": tools }}).to_string()
}

/// The same, with the gateway naming how often to ask.
///
/// The key is written out as a LITERAL rather than taken from the constant, so a
/// rename of the constant does not silently rename what this test checks.
/// [`the_gateway_names_the_interval_under_a_key_of_its_own`] is what ties the
/// literal and the constant together.
fn a_list_naming_an_interval(ms: u64, names: &[&str]) -> String {
    let mut value: Value = serde_json::from_str(&a_list(names)).unwrap();
    value["result"]["_meta"] = json!({ "com.github.yadgarhq/toolListPollMs": ms });
    value.to_string()
}

#[test]
fn the_first_tool_list_is_a_baseline_rather_than_a_change() {
    // "Changed" is a statement relative to something. A host told its catalogue
    // changed the moment it connected would re-fetch what it had just fetched,
    // every session, forever.
    let catalogue = Catalogue::default();
    assert!(
        !catalogue.record(&a_list(&["recall"])),
        "the first list this process ever saw was reported as a change"
    );
}

#[test]
fn an_unchanged_tool_list_tells_the_host_nothing() {
    // THE MUTATION THIS CATCHES: emitting on every poll. A host told nothing
    // changed learns nothing; a host told something changed re-fetches the list,
    // so an unconditional emit is a re-fetch every interval for the life of the
    // session.
    let catalogue = Catalogue::default();
    catalogue.record(&a_list(&["recall", "memorize"]));
    assert!(!catalogue.record(&a_list(&["recall", "memorize"])));
    assert!(!catalogue.record(&a_list(&["recall", "memorize"])));
}

#[test]
fn a_changed_tool_list_tells_the_host_once() {
    let catalogue = Catalogue::default();
    catalogue.record(&a_list(&["recall"]));
    assert!(
        catalogue.record(&a_list(&["recall", "memorize"])),
        "a tool appeared at the gateway and no host would ever hear about it"
    );
    assert!(
        !catalogue.record(&a_list(&["recall", "memorize"])),
        "the same change was reported twice"
    );
}

#[test]
fn a_tool_that_changed_shape_is_a_change_too() {
    // A catalogue is not a list of names: a changed description or a changed
    // input schema is a different tool to a host that renders it.
    let catalogue = Catalogue::default();
    catalogue.record(&a_list(&["recall"]));
    let described = json!({"jsonrpc": "2.0", "id": 1, "result": { "tools": [
        {"name": "recall", "description": "something else entirely"}
    ]}})
    .to_string();
    assert!(catalogue.record(&described));
}

#[test]
fn a_reordered_tool_list_is_not_a_change() {
    // The gateway is free to answer the same catalogue in another order, and a
    // host that re-fetches on a reorder re-fetches forever for nothing.
    let catalogue = Catalogue::default();
    catalogue.record(&a_list(&["recall", "memorize", "wiki_read"]));
    assert!(!catalogue.record(&a_list(&["wiki_read", "recall", "memorize"])));
}

#[test]
fn an_answer_that_is_not_a_tool_list_destroys_no_baseline() {
    // A refusal, an error page or a JSON-RPC error changes nothing and must not
    // erase what the host is actually holding — otherwise one bad poll turns the
    // next good one into a spurious change.
    let catalogue = Catalogue::default();
    catalogue.record(&a_list(&["recall"]));
    for not_a_list in [
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"no"}}"#.to_string(),
        "<html>nginx</html>".to_string(),
        r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_string(),
    ] {
        assert!(
            !catalogue.record(&not_a_list),
            "{not_a_list} read as a change"
        );
    }
    assert!(
        !catalogue.record(&a_list(&["recall"])),
        "a bad answer erased the baseline, so an unchanged list read as a change"
    );
    assert!(catalogue.record(&a_list(&["recall", "memorize"])));
}

#[test]
fn the_interval_the_gateway_named_is_the_one_this_client_holds() {
    // ONE SOURCE FOR THE NUMBER (ADR-0569). The gateway is the end that knows how
    // often its catalogue moves, and it can change that without a client release.
    // 1234 ms is a value no implementation would carry, so this cannot pass for a
    // client that ignored the reply and used its own.
    let catalogue = Catalogue::default();
    catalogue.record(&a_list_naming_an_interval(1234, &["recall"]));
    assert_eq!(catalogue.interval(), Duration::from_millis(1234));
}

#[test]
fn an_interval_nobody_named_is_not_a_number_from_a_reply() {
    // The client's own answer to "the gateway named none" is a DIFFERENT FACT
    // from the poll interval, and the two are never compared. What is asserted
    // here is only that an unnamed interval is not some other reply's, and that
    // it is long enough to be a wait rather than a loop.
    let unnamed = Catalogue::default();
    assert_ne!(unnamed.interval(), Duration::from_millis(1234));
    assert!(unnamed.interval() >= Duration::from_secs(60));
}

#[test]
fn an_interval_of_zero_is_refused() {
    // A zero interval is a loop with no sleep in it, run from every laptop with a
    // host attached.
    let catalogue = Catalogue::default();
    let bare = Catalogue::default();
    catalogue.record(&a_list_naming_an_interval(0, &["recall"]));
    assert_eq!(catalogue.interval(), bare.interval());
}

#[test]
fn the_gateway_names_the_interval_under_a_key_of_its_own() {
    // Pins the fixture above to the constant, so renaming the constant reddens a
    // test rather than silently making every gateway-named interval invisible.
    //
    // NOT UNDER `io.modelcontextprotocol/`: a key invented inside the spec's
    // namespace is indistinguishable from one the spec defines, which is the
    // near-miss failure this estate has already met on `_meta` keys.
    assert_eq!(POLL_INTERVAL_KEY, "com.github.yadgarhq/toolListPollMs");
    assert!(!POLL_INTERVAL_KEY.starts_with("io.modelcontextprotocol/"));
}

#[test]
fn the_host_is_told_in_the_protocol_s_own_words() {
    let v: Value = serde_json::from_str(&list_changed()).unwrap();
    assert_eq!(v["method"], json!("notifications/tools/list_changed"));
    assert!(
        v["id"].is_null(),
        "a notification carries no id; a host would try to match this to a request"
    );
    assert_eq!(v["jsonrpc"], json!("2.0"));
}

/// A fetcher that answers from a script, and counts when it was asked.
fn scripted(
    answers: Vec<Option<String>>,
) -> (
    impl Fn() -> std::future::Ready<Option<String>>,
    Arc<Mutex<usize>>,
) {
    let calls = Arc::new(Mutex::new(0usize));
    let seen = calls.clone();
    let fetch = move || {
        let mut n = seen.lock().expect("a lock");
        let answer = answers.get(*n).cloned().unwrap_or(None);
        *n += 1;
        std::future::ready(answer)
    };
    (fetch, calls)
}

#[tokio::test]
async fn a_tick_tells_the_host_only_when_the_catalogue_moved() {
    // BOTH MUTATIONS AT ONCE: deleting the emit leaves the channel empty on the
    // second tick, and making it unconditional puts a line there on the first and
    // third.
    let (fetch, calls) = scripted(vec![
        Some(a_list(&["recall"])),
        Some(a_list(&["recall", "memorize"])),
        Some(a_list(&["recall", "memorize"])),
    ]);
    let (out, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let weak = out.downgrade();
    let catalogue = Catalogue::default();

    assert!(tick_once(&catalogue, &fetch, &weak).await);
    assert!(
        rx.try_recv().is_err(),
        "the first list was announced as a change"
    );

    assert!(tick_once(&catalogue, &fetch, &weak).await);
    let told = rx
        .try_recv()
        .expect("a changed catalogue was not announced");
    assert!(told.contains("notifications/tools/list_changed"));

    assert!(tick_once(&catalogue, &fetch, &weak).await);
    assert!(
        rx.try_recv().is_err(),
        "an unchanged list was announced as a change"
    );
    assert_eq!(*calls.lock().unwrap(), 3);
}

#[tokio::test]
async fn a_gateway_that_would_not_answer_is_not_a_change() {
    let (fetch, _calls) = scripted(vec![Some(a_list(&["recall"])), None, None]);
    let (out, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let weak = out.downgrade();
    let catalogue = Catalogue::default();
    for _ in 0..3 {
        assert!(
            tick_once(&catalogue, &fetch, &weak).await,
            "an unreachable gateway ended the watch"
        );
    }
    assert!(rx.try_recv().is_err(), "an outage was reported as a change");
}

#[tokio::test]
async fn a_host_that_has_gone_ends_the_watch() {
    // THE ONE THING THAT MUST NOT DEADLOCK. The watch holds a WEAK handle on
    // stdout, so the loop's own sender is what keeps the writer alive; a strong
    // clone here would have each waiting on the other and `serve` would hang
    // after the host's stdin reached end of file.
    let (fetch, calls) = scripted(vec![Some(a_list(&["recall"]))]);
    let (out, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let weak = out.downgrade();
    drop(out);
    assert!(
        !tick_once(&Catalogue::default(), &fetch, &weak).await,
        "the watch carried on polling for a host that had gone"
    );
    assert_eq!(
        *calls.lock().unwrap(),
        0,
        "the gateway was asked on behalf of a host that had gone"
    );
}

#[tokio::test(start_paused = true)]
async fn the_gateway_s_interval_is_what_is_actually_waited() {
    // THE MUTATION THIS CATCHES: reading the interval once, or ignoring it and
    // waiting the client's own. 1234 ms is not a value this client could hold, and
    // five seconds is far short of what an unnamed interval waits — so a client
    // that ignored the reply polls zero times here.
    let catalogue = Catalogue::default();
    let body = a_list_naming_an_interval(1234, &["recall"]);
    catalogue.record(&body);

    let when = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let (recorded, answer) = (when.clone(), body.clone());
    let start = tokio::time::Instant::now();
    let fetch = move || {
        recorded.lock().expect("a lock").push(start.elapsed());
        std::future::ready(Some(answer.clone()))
    };

    let (out, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let weak = out.downgrade();
    let _ = tokio::time::timeout(Duration::from_secs(5), poll(catalogue, fetch, weak)).await;

    let seen = when.lock().unwrap().clone();
    assert!(
        !seen.is_empty(),
        "nothing was polled in five seconds; the interval the gateway named was ignored"
    );
    assert_eq!(
        seen[0],
        Duration::from_millis(1234),
        "the first poll did not wait the interval the gateway named"
    );
    assert!(
        seen.len() > 3,
        "the interval was read once rather than every time round: {seen:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_gateway_that_names_no_interval_is_not_polled_on_a_sentinel_one() {
    // The other half of the pair above, so neither can be deleted alone. With
    // nothing named, five seconds must pass with the gateway untouched.
    let (fetch, calls) = scripted(vec![Some(a_list(&["recall"]))]);
    let (out, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let weak = out.downgrade();
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        poll(Catalogue::default(), fetch, weak),
    )
    .await;
    assert_eq!(
        *calls.lock().unwrap(),
        0,
        "a gateway that named no interval was polled within five seconds"
    );
}

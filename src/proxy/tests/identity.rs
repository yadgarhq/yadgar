//! The two CONTEXT headers, and the one header that must never exist.
//!
//! Every assertion here reads bytes off a socket. A test that asserted a request
//! was BUILT proves nothing: the headers are attached inside a request builder,
//! and deleting the lines that attach them is invisible to every pure test in
//! this crate — which is precisely how the client shipped sending none of them.

use super::super::context::sendable;
use super::super::*;

/// A context whose every value is one no derivation could produce here.
///
/// `sentinel-org/no-such-repo` is not this repository's key and not the key of
/// anything checked out on the machine running this, so a proxy that derived a
/// project at request time — or fell back to a directory name — cannot produce
/// it. The instance is a UUID this checkout never minted, chosen the same way.
fn a_context_no_derivation_could_produce() -> Context {
    Context {
        project: Some("sentinel-org/no-such-repo".to_string()),
        instance: Some("11111111-2222-4333-8444-555555555555".to_string()),
    }
}

fn a_tool_call() -> &'static str {
    r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"recall"}}"#
}

#[tokio::test]
async fn the_context_headers_the_gateway_requires_leave_with_the_request() {
    // THE DEFECT THIS EXISTS FOR (task 502). `tools/call` reads
    // `x-yadgar-project` and `x-yadgar-instance`, and this client sent neither
    // — so every tool call was scoped to nothing at all, while the whole suite
    // passed.
    let (addr, served) = crate::testserver::answer_once(
        "200 OK",
        r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#,
    )
    .await;
    let dir = crate::testserver::scratch_dir("proxy-context");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());

    let outcome = forward(
        &reqwest::Client::new(),
        &config,
        &a_context_no_derivation_could_produce(),
        a_tool_call(),
    )
    .await;

    let sent = served.await.unwrap().to_lowercase();
    assert!(
        sent.contains("x-yadgar-project: sentinel-org/no-such-repo"),
        "the project header never left the process; the request was:\n{sent}"
    );
    assert!(
        sent.contains("x-yadgar-instance: 11111111-2222-4333-8444-555555555555"),
        "the instance header never left the process; the request was:\n{sent}"
    );
    assert!(matches!(outcome, Outcome::Answered(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn no_request_this_client_makes_ever_names_the_user() {
    // ADR-0511, ASSERTED ON THE WIRE. The gateway resolves the caller from the
    // bearer token via `iam.ResolveCredential` and mints the Scope itself,
    // because a self-asserted username is forgeable by anyone holding any valid
    // token — which is what ADR-0488 exists to prevent. Sending the stored
    // username is the smaller diff, it would make `tools/call` work today, and
    // it is exactly the change this test exists to fail.
    //
    // The config below HOLDS a username, so this cannot pass merely because
    // there was nothing to send.
    let (addr, served) = crate::testserver::answer_once(
        "200 OK",
        r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#,
    )
    .await;
    let dir = crate::testserver::scratch_dir("proxy-no-user");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into())
        .with_username(Some("sentinel.person".into()));

    let _ = forward(
        &reqwest::Client::new(),
        &config,
        &a_context_no_derivation_could_produce(),
        a_tool_call(),
    )
    .await;

    let sent = served.await.unwrap().to_lowercase();
    assert!(
        !sent.contains("x-yadgar-user"),
        "this client asserted an identity the gateway must attest; the request was:\n{sent}"
    );
    assert!(
        !sent.contains("sentinel.person"),
        "the stored username reached the wire under some other name; the request was:\n{sent}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_context_that_resolved_to_nothing_sends_no_header_at_all() {
    // ADR-0227: no fallback — and an EMPTY header is a fallback with a
    // different shape. The gateway cannot tell `x-yadgar-project: ` from a
    // project genuinely called nothing, and a claim of "" is still a claim.
    let (addr, served) = crate::testserver::answer_once(
        "200 OK",
        r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#,
    )
    .await;
    let dir = crate::testserver::scratch_dir("proxy-no-context");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());

    let _ = forward(
        &reqwest::Client::new(),
        &config,
        &Context::default(),
        a_tool_call(),
    )
    .await;

    let sent = served.await.unwrap().to_lowercase();
    assert!(
        !sent.contains("x-yadgar-project"),
        "an unresolved project was claimed as an empty one; the request was:\n{sent}"
    );
    assert!(
        !sent.contains("x-yadgar-instance"),
        "an absent install id was claimed as an empty one; the request was:\n{sent}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_project_on_the_wire_came_out_of_the_working_directory() {
    // THE SEAM NOTHING JOINED. Every test above builds a `Context` by hand, and
    // every test in `project::tests` calls `derive` with a path — so replacing
    // the whole derivation inside `discover` with `None` left all 180 tests
    // green, which is the same shape as the `.bearer_auth(...)` deletion that
    // survived a full suite. This is the one assertion that runs the real
    // derivation and reads the result off a socket.
    //
    // `sentinel/from-the-working-directory` is not the key of this repository
    // and not the key of anything on the machine, so neither a hardcode nor a
    // fallback to a directory name can produce it — it can only have been read
    // out of the file below.
    let (addr, served) = crate::testserver::answer_once(
        "200 OK",
        r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#,
    )
    .await;
    let workspace = crate::testserver::scratch_dir("proxy-derives");
    std::fs::create_dir_all(workspace.join(".yadgar")).unwrap();
    std::fs::write(
        workspace.join(".yadgar").join("project-id"),
        "sentinel/from-the-working-directory\n",
    )
    .unwrap();

    let dir = crate::testserver::scratch_dir("proxy-derives-config");
    let config = Config::new(&dir, format!("http://{addr}/"), "tok".into());
    let context = Context::discover(&config, &workspace);

    let _ = forward(&reqwest::Client::new(), &config, &context, a_tool_call()).await;

    let sent = served.await.unwrap().to_lowercase();
    assert!(
        sent.contains("x-yadgar-project: sentinel/from-the-working-directory"),
        "the project header did not come from the working directory; the request was:\n{sent}"
    );
    std::fs::remove_dir_all(&workspace).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_value_that_cannot_be_a_header_is_dropped_rather_than_sent() {
    // BOTH FIELDS come out of files a person can edit — `.yadgar/project-id`
    // and `config.json` — so either can hold a line break. reqwest refuses such
    // a value when the request is BUILT, so the WHOLE request fails and the
    // agent is told `yadgar gateway unreachable`: somebody then goes to look at
    // their network over a character in a file.
    assert_eq!(
        sendable(HEADER_PROJECT, Some("owner/repo".into())).as_deref(),
        Some("owner/repo")
    );
    assert_eq!(sendable(HEADER_PROJECT, Some("owner/re\npo".into())), None);
    assert_eq!(sendable(HEADER_INSTANCE, Some("a-uuid\r".into())), None);
    assert_eq!(sendable(HEADER_INSTANCE, Some("a-\0uuid".into())), None);
    assert_eq!(sendable(HEADER_PROJECT, Some(String::new())), None);
    assert_eq!(sendable(HEADER_PROJECT, None), None);
    // A NON-ASCII VALUE IS SENDABLE, AND SO IS ONE WITH A TAB, and that
    // surprise is the argument for using the HTTP library's own check rather
    // than a hand-written approximation. A field value may carry obs-text and
    // HTAB (RFC 9110 §5.5), so a guard that "obviously" refused everything
    // outside printable ASCII would drop legitimate values and scope every call
    // to nothing — the same failure, pointing the other way.
    assert_eq!(
        sendable(HEADER_PROJECT, Some("ägare/repo".into())).as_deref(),
        Some("ägare/repo")
    );
}

#[test]
fn an_unsendable_value_in_either_file_costs_one_header_and_not_the_request() {
    // The whole path rather than the predicate: a `discover` that skipped the
    // check would hand the value on and every call would fail as an outage.
    let workspace = crate::testserver::scratch_dir("context-unsendable");
    std::fs::create_dir_all(workspace.join(".yadgar")).unwrap();
    std::fs::write(
        workspace.join(".yadgar").join("project-id"),
        "sentinel/one\nsentinel/two\n",
    )
    .unwrap();

    // Written as bytes, because `ensure_instance` could never mint this.
    let dir = crate::testserver::scratch_dir("context-unsendable-config");
    std::fs::write(
        dir.join("config.json"),
        "{\"gateway\":\"https://gw/\",\"token\":\"t\",\"instance\":\"7777\\n7777\"}",
    )
    .unwrap();
    let config = Config::load_from(&dir).unwrap();
    assert_eq!(
        config.instance(),
        Some("7777\n7777"),
        "the fixture is wrong"
    );

    let context = Context::discover(&config, &workspace);
    assert_eq!(context.project, None);
    assert_eq!(context.instance, None);
    std::fs::remove_dir_all(&workspace).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_context_headers_are_named_exactly_as_the_gateway_reads_them() {
    // A near-miss is not a near-miss to the gateway: it reads as the header
    // being absent, and the scope is minted without it.
    assert_eq!(HEADER_PROJECT, "x-yadgar-project");
    assert_eq!(HEADER_INSTANCE, "x-yadgar-instance");
}

#[test]
fn the_instance_a_request_carries_is_the_one_the_config_stored() {
    // It must come from the file rather than from a fresh mint per process: a
    // value that changed on every restart would correlate a client to nothing,
    // which is what ADR-0511 rejected a per-process instance for.
    let dir = crate::testserver::scratch_dir("proxy-instance");
    let mut config = Config::new(&dir, "https://gw".into(), "tok".into());
    config.ensure_instance().unwrap();
    let stored = config.instance().unwrap().to_string();
    assert_eq!(
        Context::discover(&config, &dir).instance,
        Some(stored.clone())
    );
    // A SECOND discovery, to pin that discovering does not mint.
    assert_eq!(Context::discover(&config, &dir).instance, Some(stored));
    std::fs::remove_dir_all(&dir).ok();
}

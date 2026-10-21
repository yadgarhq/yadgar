//! What `yaadgaar whoami` sends, what it makes of each answer, and that the
//! saved token never reaches any output.

use super::*;

/// Distinctive, so a substring search cannot match anything else.
const TOKEN: &str = "s3cr3t-t0ken-zz";
const SEGMENT: &str = "0199f1a2-7b3c-7d4e-8f50-123456789abc";

/// Run [`execute`] against a one-shot server answering *status* and *body*.
///
/// Returns the result, what was printed, and the request as it arrived.
async fn ask_at(
    suffix: &str,
    status: &'static str,
    body: &'static str,
) -> (Result<(), WhoamiError>, String, String) {
    let (addr, served) = crate::testserver::answer_once(status, body).await;
    let client = crate::trust::client(None, None).unwrap();
    let url = format!("http://{addr}{suffix}");
    let gateway = Gateway {
        url: &url,
        bearer: TOKEN,
        instance: Some("inst-1"),
        expires_at: None,
    };
    let mut out = Vec::new();
    let result = execute(&client, &gateway, &mut out).await;
    // BOUNDED: a request that never reached the server must fail, not hang.
    let sent = tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .map(|s| s.unwrap())
        .unwrap_or_default();
    (result, String::from_utf8(out).unwrap(), sent)
}

async fn ask(
    status: &'static str,
    body: &'static str,
) -> (Result<(), WhoamiError>, String, String) {
    ask_at("/", status, body).await
}

/// The token is in neither the printed output nor any error in the chain,
/// which `main` prints through anyhow.
fn assert_no_token(result: &Result<(), WhoamiError>, printed: &str) {
    assert!(!printed.contains(TOKEN), "token printed: {printed}");
    let mut next: Option<&dyn std::error::Error> = result.as_ref().err().map(|e| e as _);
    while let Some(e) = next {
        assert!(!e.to_string().contains(TOKEN), "token in error: {e}");
        next = e.source();
    }
}

const HAPPY: &str = r#"{"account_segment":"0199f1a2-7b3c-7d4e-8f50-123456789abc","user_id":"yadgar:user:0199f1a2-7b3c-7d4e-8f50-123456789abc"}"#;

#[tokio::test]
async fn it_prints_the_user_id_the_segment_and_the_private_prefix() {
    let (result, printed, _) = ask("200 OK", HAPPY).await;
    assert!(result.is_ok(), "{result:?}");
    assert!(
        printed.contains(&format!("yadgar:user:{SEGMENT}")),
        "{printed}"
    );
    assert!(printed.contains(&format!("local/{SEGMENT}/")), "{printed}");
    assert_eq!(printed.matches(SEGMENT).count(), 3, "{printed}");
    assert_no_token(&result, &printed);
}

#[tokio::test]
async fn the_request_is_a_get_carrying_the_saved_credential_and_instance() {
    let (_, _, sent) = ask("200 OK", HAPPY).await;
    assert!(sent.starts_with("GET /auth/whoami "), "{sent}");
    let lower = sent.to_ascii_lowercase();
    assert!(
        lower.contains(&format!("authorization: bearer {TOKEN}")),
        "{sent}"
    );
    assert!(lower.contains("x-yadgar-instance: inst-1"), "{sent}");
}

#[tokio::test]
async fn a_gateway_address_without_its_trailing_slash_still_reaches_the_route() {
    let (result, _, sent) = ask_at("", "200 OK", HAPPY).await;
    assert!(result.is_ok(), "{result:?}");
    assert!(sent.starts_with("GET /auth/whoami "), "{sent}");
}

#[tokio::test]
async fn a_401_says_the_credential_is_invalid_or_revoked_and_to_log_in_again() {
    let (result, printed, _) = ask("401 Unauthorized", r#"{"error":"unauthenticated"}"#).await;
    assert!(
        matches!(&result, Err(WhoamiError::Refused(m)) if m == "unauthenticated"),
        "{result:?}"
    );
    let message = result.as_ref().unwrap_err().to_string();
    assert!(message.contains("revoked"), "{message}");
    assert!(message.contains("yaadgaar login"), "{message}");
    assert!(printed.is_empty(), "{printed}");
    assert_no_token(&result, &printed);
}

#[tokio::test]
async fn a_non_success_status_is_never_read_as_an_identity() {
    // A well-formed identity under a 500 is still a failure: the status is
    // checked before the body is believed.
    let (result, printed, _) = ask("500 Internal Server Error", HAPPY).await;
    assert!(
        matches!(&result, Err(WhoamiError::Rejected(s, _)) if s.as_u16() == 500),
        "{result:?}"
    );
    assert!(printed.is_empty(), "{printed}");
    assert_no_token(&result, &printed);
}

#[tokio::test]
async fn a_404_names_a_gateway_older_than_the_route() {
    let (result, printed, _) = ask("404 Not Found", "").await;
    assert!(
        matches!(&result, Err(WhoamiError::RouteNotServed)),
        "{result:?}"
    );
    assert_no_token(&result, &printed);
}

#[tokio::test]
async fn a_null_missing_or_empty_segment_is_refused_and_prints_no_prefix() {
    for body in [
        r#"{"account_segment":null,"user_id":"yadgar:user:dev"}"#,
        r#"{"user_id":"yadgar:user:dev"}"#,
        r#"{"account_segment":"  ","user_id":"yadgar:user:dev"}"#,
    ] {
        let (result, printed, _) = ask("200 OK", body).await;
        assert!(
            matches!(&result, Err(WhoamiError::NoSegment(id)) if id == "yadgar:user:dev"),
            "{body}: {result:?}"
        );
        assert!(!printed.contains("local/"), "{body}: {printed}");
        assert_no_token(&result, &printed);
    }
}

#[tokio::test]
async fn a_success_without_a_user_id_is_not_reported_as_one() {
    let (result, printed, _) = ask("200 OK", r#"{"account_segment":"x"}"#).await;
    assert!(
        matches!(&result, Err(WhoamiError::Malformed(_))),
        "{result:?}"
    );
    assert!(printed.is_empty(), "{printed}");
    assert_no_token(&result, &printed);
}

#[tokio::test]
async fn a_segment_that_cannot_be_one_path_segment_is_refused() {
    // A slash, whitespace or a control character would make `local/<segment>/`
    // a different path, or put a terminal escape on screen. The uuid shape is
    // the gateway's to own, so nothing stricter is checked here.
    for body in [
        r#"{"account_segment":"a/b","user_id":"yadgar:user:x"}"#,
        r#"{"account_segment":"a b","user_id":"yadgar:user:x"}"#,
        r#"{"account_segment":"a\tb","user_id":"yadgar:user:x"}"#,
        r#"{"account_segment":"a\u001b[2Jb","user_id":"yadgar:user:x"}"#,
    ] {
        let (result, printed, _) = ask("200 OK", body).await;
        assert!(
            matches!(&result, Err(WhoamiError::UnsafeSegment(_))),
            "{body}: {result:?}"
        );
        assert!(printed.is_empty(), "{body}: {printed}");
        let message = result.as_ref().unwrap_err().to_string();
        assert!(
            !message.contains('\u{1b}'),
            "{body}: raw escape in {message:?}"
        );
        assert_no_token(&result, &printed);
    }
}

#[tokio::test]
async fn an_unreachable_gateway_is_a_transport_error_naming_the_address() {
    // Bind then drop, so the port is very likely closed.
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let client = crate::trust::client(None, None).unwrap();
    let url = format!("http://{addr}/");
    let gateway = Gateway {
        url: &url,
        bearer: TOKEN,
        instance: None,
        expires_at: None,
    };
    let mut out = Vec::new();
    let result = execute(&client, &gateway, &mut out).await;
    assert!(
        matches!(&result, Err(WhoamiError::Unreachable(u, _)) if u.contains(&addr.to_string())),
        "{result:?}"
    );
    let printed = String::from_utf8(out).unwrap();
    assert_no_token(&result, &printed);
}

// ---- the stored expiry (ADR-0972 (3), ledger 1628) ----

/// [`execute`] against a one-shot server, with *expires_at* stored.
async fn ask_expiring(
    status: &'static str,
    body: &'static str,
    expires_at: Option<&str>,
) -> (Result<(), WhoamiError>, String) {
    let (addr, _served) = crate::testserver::answer_once(status, body).await;
    let client = crate::trust::client(None, None).unwrap();
    let url = format!("http://{addr}/");
    let gateway = Gateway {
        url: &url,
        bearer: TOKEN,
        instance: None,
        expires_at,
    };
    let mut out = Vec::new();
    let result = execute(&client, &gateway, &mut out).await;
    (result, String::from_utf8(out).unwrap())
}

#[tokio::test]
async fn whoami_prints_the_stored_expiry() {
    let (result, printed) = ask_expiring("200 OK", HAPPY, Some("2999-01-01T00:00:00Z")).await;
    assert!(result.is_ok(), "{result:?}");
    assert!(
        printed.contains("expires_at:      2999-01-01T00:00:00Z\n"),
        "{printed}"
    );
}

#[tokio::test]
async fn whoami_says_unknown_when_no_expiry_is_stored() {
    let (result, printed) = ask_expiring("200 OK", HAPPY, None).await;
    assert!(result.is_ok(), "{result:?}");
    assert!(printed.contains("expires_at:      unknown\n"), "{printed}");
}

#[tokio::test]
async fn a_401_after_the_stored_expiry_names_the_expiry() {
    let (result, printed) = ask_expiring(
        "401 Unauthorized",
        r#"{"error":"unauthenticated"}"#,
        Some("2020-01-01T00:00:00Z"),
    )
    .await;
    let e = result.as_ref().unwrap_err();
    assert!(matches!(e, WhoamiError::Expired(_)), "{e:?}");
    assert_eq!(
        e.to_string(),
        "your credential expired at 2020-01-01T00:00:00Z; run `yaadgaar login`"
    );
    assert_no_token(&result, &printed);
}

#[tokio::test]
async fn a_401_before_the_stored_expiry_keeps_the_gateways_reason() {
    let (result, _) = ask_expiring(
        "401 Unauthorized",
        r#"{"error":"unauthenticated"}"#,
        Some("2999-01-01T00:00:00Z"),
    )
    .await;
    assert!(
        matches!(&result, Err(WhoamiError::Refused(m)) if m == "unauthenticated"),
        "{result:?}"
    );
}

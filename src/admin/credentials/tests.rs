//! `admin list-credentials` and `admin revoke-credential` (ADR-0972 (2),
//! ledger 1627): what goes on the wire, what each answer prints, and that no
//! secret and no user id reaches any output.

use super::super::{execute, AdminCommand, AdminError, Gateway};

const USER: &str = "yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f";
const CRED: &str = "cred-0199f1a2";
const BEARER: &str = "adm1n-t0ken-qq";

fn parse(args: &[&str]) -> AdminCommand {
    use clap::Parser as _;
    #[derive(clap::Parser)]
    struct Probe {
        #[command(subcommand)]
        command: AdminCommand,
    }
    let mut all = vec!["probe"];
    all.extend_from_slice(args);
    Probe::try_parse_from(all).unwrap().command
}

struct Run {
    result: Result<(), AdminError>,
    sent: Vec<String>,
    out: String,
    err: String,
}

/// Run *args* against a server answering each request with the next of
/// *answers*, with *expires_at* as the stored expiry.
async fn run_with(
    args: &[&str],
    answers: Vec<(&'static str, &'static str)>,
    expires_at: Option<&str>,
) -> Run {
    let (addr, served) = crate::testserver::answer_each(answers).await;
    let client = crate::trust::client(None, None).unwrap();
    let url = format!("http://{addr}/");
    let gateway = Gateway {
        url: &url,
        bearer: BEARER,
        instance: None,
        expires_at,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = execute(parse(args), &client, &gateway, &mut out, &mut err).await;
    // The server may still wait for a request that never came; never hang.
    let sent = match tokio::time::timeout(std::time::Duration::from_millis(500), served).await {
        Ok(s) => s.unwrap(),
        Err(_) => Vec::new(),
    };
    Run {
        result,
        sent,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

async fn run(args: &[&str], answers: Vec<(&'static str, &'static str)>) -> Run {
    run_with(args, answers, None).await
}

fn list_args() -> Vec<&'static str> {
    vec!["list-credentials", "--user-id", USER]
}

fn revoke_args() -> Vec<&'static str> {
    vec![
        "revoke-credential",
        "--user-id",
        USER,
        "--credential-id",
        CRED,
    ]
}

const ONE_PAGE: &str = r#"{"credentials":[
  {"id":"cred-a","label":"laptop","created_at":"2026-10-01T08:00:00Z","expires_at":"2026-12-30T08:00:00Z"},
  {"id":"cred-b","label":"desktop","created_at":"2026-09-01T08:00:00.5Z"}
],"next_page_token":""}"#;

#[tokio::test]
async fn list_credentials_prints_one_row_per_credential() {
    let r = run(&list_args(), vec![("200 OK", ONE_PAGE)]).await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    let rows: Vec<&str> = r.out.lines().collect();
    assert_eq!(rows.len(), 2, "{}", r.out);
    assert_eq!(
        rows[0],
        "cred-a\t2026-10-01T08:00:00Z\t2026-12-30T08:00:00Z\tlaptop"
    );
    // A row minted before ADR-0972 (3) carries no expiry: it never expires.
    assert_eq!(rows[1], "cred-b\t2026-09-01T08:00:00.5Z\tnever\tdesktop");
}

#[tokio::test]
async fn the_list_request_is_the_user_id_alone_with_the_callers_bearer() {
    let r = run(&list_args(), vec![("200 OK", ONE_PAGE)]).await;
    let sent = &r.sent[0];
    assert!(sent.starts_with("POST /admin/list-credentials "), "{sent}");
    assert!(
        sent.to_ascii_lowercase()
            .contains(&format!("authorization: bearer {BEARER}")),
        "{sent}"
    );
    let body = sent.split("\r\n\r\n").nth(1).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::json!({ "user_id": USER })
    );
}

#[tokio::test]
async fn every_page_is_followed_and_the_token_goes_back_as_page_token() {
    let r = run(
        &list_args(),
        vec![
            (
                "200 OK",
                r#"{"credentials":[{"id":"cred-a","label":"a","created_at":"2026-10-01T08:00:00Z"}],"next_page_token":"p2"}"#,
            ),
            (
                "200 OK",
                r#"{"credentials":[{"id":"cred-b","label":"b","created_at":"2026-10-02T08:00:00Z"}],"next_page_token":""}"#,
            ),
        ],
    )
    .await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert_eq!(r.out.lines().count(), 2, "{}", r.out);
    assert_eq!(r.sent.len(), 2);
    let second = r.sent[1].split("\r\n\r\n").nth(1).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(second).unwrap(),
        serde_json::json!({ "user_id": USER, "page_token": "p2" })
    );
}

#[tokio::test]
async fn a_page_token_the_gateway_repeats_is_refused_rather_than_looped_on() {
    let page = r#"{"credentials":[],"next_page_token":"same"}"#;
    let r = run(&list_args(), vec![("200 OK", page), ("200 OK", page)]).await;
    assert!(
        matches!(r.result, Err(AdminError::RepeatedPageToken)),
        "{:?}",
        r.result
    );
}

#[tokio::test]
async fn an_empty_list_says_so_on_stderr_and_prints_no_rows() {
    let r = run(
        &list_args(),
        vec![("200 OK", r#"{"credentials":[],"next_page_token":""}"#)],
    )
    .await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert_eq!(r.out, "");
    assert!(r.err.contains("no credentials"), "{}", r.err);
}

#[tokio::test]
async fn a_label_with_control_characters_is_printed_escaped() {
    let r = run(
        &list_args(),
        vec![(
            "200 OK",
            r#"{"credentials":[{"id":"cred-a","label":"evil\u001b[2J\tx","created_at":"2026-10-01T08:00:00Z"}],"next_page_token":""}"#,
        )],
    )
    .await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert!(!r.out.contains('\u{1b}'), "{:?}", r.out);
    assert!(r.out.contains(r"evil\u{1b}[2J\tx"), "{:?}", r.out);
}

#[tokio::test]
async fn a_list_answer_without_credentials_is_refused() {
    let r = run(&list_args(), vec![("200 OK", r#"{"next_page_token":""}"#)]).await;
    assert!(
        matches!(r.result, Err(AdminError::MalformedAnswer(..))),
        "{:?}",
        r.result
    );
}

#[tokio::test]
async fn any_404_on_list_is_an_older_gateway() {
    // An unknown user is an empty page, never a 404 (gateway#127).
    let r = run(
        &list_args(),
        vec![("404 Not Found", r#"{"error":"no such user"}"#)],
    )
    .await;
    let e = r.result.unwrap_err();
    assert!(matches!(e, AdminError::RouteAbsent(_)), "{e:?}");
    assert!(e.to_string().contains("/admin/list-credentials"), "{e}");
}

#[tokio::test]
async fn the_revoke_request_is_exactly_the_user_and_the_credential() {
    let r = run(
        &revoke_args(),
        vec![(
            "200 OK",
            r#"{"user_id":"yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f","credential_id":"cred-0199f1a2"}"#,
        )],
    )
    .await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    let sent = &r.sent[0];
    assert!(sent.starts_with("POST /admin/revoke-credential "), "{sent}");
    assert!(
        sent.to_ascii_lowercase()
            .contains(&format!("authorization: bearer {BEARER}")),
        "{sent}"
    );
    let body = sent.split("\r\n\r\n").nth(1).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::json!({ "user_id": USER, "credential_id": CRED })
    );
    assert_eq!(r.out.trim(), "revoked credential cred-0199f1a2");
}

#[tokio::test]
async fn a_revoke_receipt_for_another_credential_or_user_is_refused() {
    for body in [
        r#"{"user_id":"yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f","credential_id":"other"}"#,
        r#"{"user_id":"yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e00","credential_id":"cred-0199f1a2"}"#,
    ] {
        let r = run(&revoke_args(), vec![("200 OK", body)]).await;
        assert!(
            matches!(r.result, Err(AdminError::CredentialEchoMismatch)),
            "{:?}",
            r.result
        );
        assert_eq!(r.out, "", "a mismatch must print no receipt");
    }
}

#[tokio::test]
async fn only_the_gateways_own_words_mean_no_such_credential() {
    let r = run(
        &revoke_args(),
        vec![(
            "404 Not Found",
            r#"{"error":"no such credential for this user"}"#,
        )],
    )
    .await;
    assert!(
        matches!(r.result, Err(AdminError::NoSuchCredential(_))),
        "{:?}",
        r.result
    );
    let r = run(&revoke_args(), vec![("404 Not Found", "not found")]).await;
    let e = r.result.unwrap_err();
    assert!(matches!(e, AdminError::RouteAbsent(_)), "{e:?}");
    assert!(e.to_string().contains("/admin/revoke-credential"), "{e}");
}

#[tokio::test]
async fn a_403_on_revoke_credential_does_not_claim_self_revocation_is_refused() {
    // ADR-0972 (2): an administrator MAY revoke their own credential.
    let r = run(
        &revoke_args(),
        vec![("403 Forbidden", r#"{"error":"not an administrator"}"#)],
    )
    .await;
    let e = r.result.unwrap_err();
    assert!(matches!(e, AdminError::CredentialForbidden(_)), "{e:?}");
    assert!(!e.to_string().contains("your own"), "{e}");
    assert!(e.to_string().contains("not an administrator"), "{e}");
}

#[tokio::test]
async fn revoke_credential_maps_401_to_login_again() {
    for args in [revoke_args(), list_args()] {
        let r = run(&args, vec![("401 Unauthorized", r#"{"error":"x"}"#)]).await;
        let e = r.result.unwrap_err();
        assert!(matches!(e, AdminError::Refused), "{e:?}");
        assert!(e.to_string().contains("yaadgaar login"), "{e}");
    }
}

#[tokio::test]
async fn a_401_after_the_stored_expiry_names_the_expiry() {
    for args in [revoke_args(), list_args()] {
        let r = run_with(
            &args,
            vec![("401 Unauthorized", r#"{"error":"x"}"#)],
            Some("2020-01-01T00:00:00Z"),
        )
        .await;
        let e = r.result.unwrap_err();
        assert!(matches!(e, AdminError::Expired(_)), "{e:?}");
        assert_eq!(
            e.to_string(),
            "your credential expired at 2020-01-01T00:00:00Z; run `yaadgaar login`"
        );
    }
}

#[tokio::test]
async fn a_401_before_the_stored_expiry_keeps_the_refusal() {
    let r = run_with(
        &revoke_args(),
        vec![("401 Unauthorized", r#"{"error":"x"}"#)],
        Some("2999-01-01T00:00:00Z"),
    )
    .await;
    assert!(
        matches!(r.result, Err(AdminError::Refused)),
        "{:?}",
        r.result
    );
}

#[tokio::test]
async fn a_bad_user_id_or_an_empty_credential_id_sends_nothing() {
    let r = run(
        &[
            "revoke-credential",
            "--user-id",
            "bob",
            "--credential-id",
            CRED,
        ],
        vec![("200 OK", "{}")],
    )
    .await;
    assert!(
        matches!(r.result, Err(AdminError::NotCanonicalUserId)),
        "{:?}",
        r.result
    );
    assert!(r.sent.is_empty());
    let r = run(
        &[
            "revoke-credential",
            "--user-id",
            USER,
            "--credential-id",
            "  ",
        ],
        vec![("200 OK", "{}")],
    )
    .await;
    assert!(
        matches!(r.result, Err(AdminError::EmptyCredentialId)),
        "{:?}",
        r.result
    );
    assert!(r.sent.is_empty());
    let r = run(
        &["list-credentials", "--user-id", "bob"],
        vec![("200 OK", "{}")],
    )
    .await;
    assert!(
        matches!(r.result, Err(AdminError::NotCanonicalUserId)),
        "{:?}",
        r.result
    );
    assert!(r.sent.is_empty());
}

#[tokio::test]
async fn no_bearer_and_no_user_id_reaches_any_output() {
    let mut seen = Vec::new();
    for args in [revoke_args(), list_args()] {
        for (status, body) in [
            ("200 OK", ONE_PAGE),
            ("401 Unauthorized", r#"{"error":"x"}"#),
            ("403 Forbidden", r#"{"error":"x"}"#),
            ("404 Not Found", r#"{"error":"x"}"#),
            ("400 Bad Request", r#"{"error":"x"}"#),
            ("503 Service Unavailable", r#"{"error":"x"}"#),
            ("200 OK", "not json"),
        ] {
            let r = run(&args, vec![(status, body)]).await;
            seen.push(r.out);
            seen.push(r.err);
            if let Err(e) = r.result {
                seen.push(format!("{e} {e:?}"));
            }
        }
    }
    for text in seen {
        assert!(!text.contains(BEARER), "the bearer leaked: {text}");
        assert!(!text.contains(USER), "the user id was printed: {text}");
    }
}

// ---- terminal escapes from the gateway never print raw ----

#[tokio::test]
async fn an_escape_sequence_in_any_listed_field_never_prints_raw() {
    let r = run(
        &list_args(),
        vec![(
            "200 OK",
            r#"{"credentials":[{"id":"c\u001b]52;c;aGk=\u0007","label":"l\u009b2J","created_at":"\u001b[2J","expires_at":"\u001b]0;title\u0007"}],"next_page_token":""}"#,
        )],
    )
    .await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert!(
        !r.out.contains('\u{1b}') && !r.out.contains('\u{7}') && !r.out.contains('\u{9b}'),
        "{:?}",
        r.out
    );
    assert!(r.out.contains(r"c\u{1b}]52;c;aGk=\u{7}"), "{:?}", r.out);
}

#[tokio::test]
async fn an_escape_sequence_in_a_refusal_body_never_reaches_the_error() {
    let r = run(
        &revoke_args(),
        vec![("403 Forbidden", r#"{"error":"x\u001b]52;c;aGk=\u0007"}"#)],
    )
    .await;
    let message = r.result.unwrap_err().to_string();
    assert!(
        !message.contains('\u{1b}') && !message.contains('\u{7}'),
        "{message:?}"
    );
}

#[tokio::test]
async fn an_escape_sequence_in_the_credential_id_never_prints_raw() {
    let r = run(
        &[
            "revoke-credential",
            "--user-id",
            USER,
            "--credential-id",
            "c\u{1b}[2J",
        ],
        vec![(
            "200 OK",
            r#"{"user_id":"yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f","credential_id":"c\u001b[2J"}"#,
        )],
    )
    .await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert!(!r.out.contains('\u{1b}'), "{:?}", r.out);
}

#[tokio::test]
async fn a_bidi_override_in_a_label_never_prints_raw() {
    let r = run(
        &list_args(),
        vec![(
            "200 OK",
            r#"{"credentials":[{"id":"cred-a","label":"lap\u202etop","created_at":"2026-10-01T08:00:00Z"}],"next_page_token":""}"#,
        )],
    )
    .await;
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert!(!r.out.contains('\u{202e}'), "{:?}", r.out);
    assert!(r.out.contains(r"lap\u{202e}top"), "{:?}", r.out);
}

#[tokio::test]
async fn an_escape_sequence_in_the_gateway_address_never_reaches_the_error() {
    let client = crate::trust::client(None, None).unwrap();
    let gateway = Gateway {
        url: "http://127.0.0.1:1/\u{1b}]52;c;aGk=\u{7}",
        bearer: BEARER,
        instance: None,
        expires_at: None,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = execute(parse(&list_args()), &client, &gateway, &mut out, &mut err).await;
    let e = result.unwrap_err();
    assert!(matches!(e, AdminError::Unreachable(..)), "{e:?}");
    let message = e.to_string();
    assert!(
        !message.contains('\u{1b}') && !message.contains('\u{7}'),
        "{message:?}"
    );
}

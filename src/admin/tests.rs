//! What `yaadgaar admin issue-enrolment` sends, what it makes of each answer,
//! and where the token may land.

use super::*;

async fn ask(status: &'static str, body: &'static str) -> (Result<Issued, AdminError>, String) {
    ask_at("/", status, body).await
}

/// [`ask`], with the gateway address ending in *suffix* after the port.
async fn ask_at(
    suffix: &str,
    status: &'static str,
    body: &'static str,
) -> (Result<Issued, AdminError>, String) {
    let (addr, served) = crate::testserver::answer_once(status, body).await;
    let client = crate::trust::client(None, None).unwrap();
    let gateway = format!("http://{addr}{suffix}");
    let result = request(
        &client,
        &gateway,
        "adm1n-t0ken",
        Some("inst-1"),
        "u-42",
        true,
    )
    .await;
    // BOUNDED: a request that never reached the server leaves it waiting for a
    // connection, and the test must fail rather than hang.
    let sent = tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .map(|s| s.unwrap())
        .unwrap_or_default();
    (result, sent)
}

#[tokio::test]
async fn a_gateway_address_without_its_trailing_slash_still_reaches_the_route() {
    // `config.json` can be edited by hand; `login` normalises for this reason.
    let (result, sent) = ask_at("", "200 OK", r#"{"token":"B","enrolment_id":"e"}"#).await;
    assert!(result.is_ok(), "{result:?}");
    assert!(sent.starts_with("POST /admin/issue-enrolment "), "{sent}");
}

#[cfg(unix)]
#[test]
fn a_token_file_written_here_is_one_enrol_token_file_reads() {
    // The whole workflow: the admin writes the file, the person passes it to
    // `yaadgaar enrol --token-file`, which decodes what it reads.
    let blob = "CjRjMlZ1ZEdsdVpXd3RjMlZqY21WMExXNXZkQzFoTFdoaGNtUmpiMlJsTFdGaFlXRmhZUT09EiBodHRwczovL2d3LnNlbnRpbmVsLmludmFsaWQ6MTk5OSIGCICgsLQD";
    let dir = crate::testserver::scratch_dir("admin-round-trip");
    let path = dir.join("token");
    write_token_file(&path, blob).unwrap();
    let read = std::fs::read_to_string(&path).unwrap();
    let enrolment = crate::enrolment::decode(&read).unwrap();
    assert_eq!(enrolment.gateway, "https://gw.sentinel.invalid:1999");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_request_names_the_user_and_carries_the_callers_own_credential() {
    let (result, sent) = ask("200 OK", r#"{"token":"BLOB==","enrolment_id":"e-7"}"#).await;
    let issued = result.unwrap();
    assert_eq!(issued.token, "BLOB==");
    assert_eq!(issued.enrolment_id, "e-7");
    assert!(sent.starts_with("POST /admin/issue-enrolment "), "{sent}");
    let lower = sent.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer adm1n-t0ken"),
        "{sent}"
    );
    assert!(lower.contains("x-yadgar-instance: inst-1"), "{sent}");
    let body = &sent[sent.find("\r\n\r\n").unwrap() + 4..];
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    // The revocation demand travels as a JSON bool (ledger 1569).
    assert_eq!(
        body,
        serde_json::json!({"user_id": "u-42", "revoke_existing_credentials": true})
    );
}

#[tokio::test]
async fn each_refusal_says_what_the_person_can_do_about_it() {
    let (result, _) = ask("401 Unauthorized", r#"{"error":"x"}"#).await;
    assert!(matches!(result, Err(AdminError::Refused)), "{result:?}");

    let (result, _) = ask("403 Forbidden", r#"{"error":"x"}"#).await;
    // The gateway's own words are kept: they say which rule refused.
    assert!(
        matches!(&result, Err(AdminError::NotAdmin(m)) if m == "x"),
        "{result:?}"
    );

    // The gateway's own words, because they name what was wrong.
    let (result, _) = ask("404 Not Found", r#"{"error":"no such user"}"#).await;
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("404") && message.contains("no such user"),
        "{message}"
    );
}

#[tokio::test]
async fn a_success_without_a_token_is_not_reported_as_one() {
    let (result, _) = ask("200 OK", r#"{"enrolment_id":"e-7"}"#).await;
    assert!(
        matches!(result, Err(AdminError::Malformed(_))),
        "{result:?}"
    );
}

#[cfg(unix)]
#[test]
fn the_token_file_is_owner_only_and_never_replaces_anything() {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::testserver::scratch_dir("admin-token-file");
    let path = dir.join("token");
    write_token_file(&path, "BLOB==").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "BLOB==\n");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "got {:o}", mode & 0o777);

    // An existing file is refused, not overwritten: it may be the last copy
    // of a token somebody has not handed over yet.
    assert!(write_token_file(&path, "OTHER").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "BLOB==\n");

    // A symlink is refused, not followed.
    let victim = dir.join("victim");
    std::fs::write(&victim, "untouched").unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    assert!(write_token_file(&link, "BLOB==").is_err());
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_user_id_is_refused_before_anything_is_sent() {
    assert!(matches!(
        require_user_id("  "),
        Err(AdminError::EmptyUserId)
    ));
    assert_eq!(require_user_id(" u-42 ").unwrap(), "u-42");
}

#[test]
fn an_echo_that_contradicts_the_revocation_asked_for_is_an_error() {
    // Ledger 1579: the gateway echoes `revoke_existing_credentials`. An echo
    // that differs means the old credentials may still work, or were revoked
    // when nobody asked: either way the person must not be told it went well.
    assert!(matches!(
        check_echo(true, Some(false)),
        Err(AdminError::EchoMismatch {
            sent: true,
            echoed: false
        })
    ));
    assert!(matches!(
        check_echo(false, Some(true)),
        Err(AdminError::EchoMismatch {
            sent: false,
            echoed: true
        })
    ));
    assert_eq!(check_echo(true, Some(true)).unwrap(), None);
    assert_eq!(check_echo(false, Some(false)).unwrap(), None);
}

#[test]
fn an_older_gateway_that_echoes_nothing_earns_a_warning_when_revocation_was_asked() {
    // A gateway before the field ignores it, so the old credentials SURVIVE.
    // Not an error (the enrolment was issued), but never silent.
    let warning = check_echo(true, None).unwrap().expect("a warning");
    assert!(warning.contains("not confirm"), "{warning}");
    assert_eq!(check_echo(false, None).unwrap(), None);
}

#[tokio::test]
async fn the_echo_is_read_off_the_gateways_answer() {
    let (result, _) = ask(
        "200 OK",
        r#"{"token":"B","enrolment_id":"e","revoke_existing_credentials":true}"#,
    )
    .await;
    assert_eq!(result.unwrap().revoke_existing_credentials, Some(true));
}

#[test]
fn revoke_existing_is_an_opt_in_flag() {
    use clap::Parser as _;
    #[derive(clap::Parser)]
    struct Probe {
        #[command(subcommand)]
        command: AdminCommand,
    }
    let parse = |extra: &[&str]| {
        let mut args = vec!["probe", "issue-enrolment", "--user-id", "u", "--stdout"];
        args.extend_from_slice(extra);
        match Probe::try_parse_from(args).unwrap().command {
            AdminCommand::IssueEnrolment {
                revoke_existing, ..
            } => revoke_existing,
            AdminCommand::RevokeCredentials { .. } => unreachable!("parsed issue-enrolment"),
        }
    };
    assert!(!parse(&[]));
    assert!(parse(&["--revoke-existing"]));
}

/// Parse an `admin` command line the way the binary does.
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

/// Run a parsed command against a one-shot server, capturing both streams.
async fn execute_against(
    args: &[&str],
    status: &'static str,
    body: &'static str,
) -> (Result<(), AdminError>, String, Vec<u8>, Vec<u8>) {
    let (addr, served) = crate::testserver::answer_once(status, body).await;
    let client = crate::trust::client(None, None).unwrap();
    let url = format!("http://{addr}/");
    let gateway = Gateway {
        url: &url,
        bearer: "adm1n-t0ken",
        instance: None,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = execute(parse(args), &client, &gateway, &mut out, &mut err).await;
    let sent = tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .map(|s| s.unwrap())
        .unwrap_or_default();
    (result, sent, out, err)
}

#[tokio::test]
async fn an_echo_mismatch_writes_no_token_file_and_prints_nothing_on_stdout() {
    // The enrolment the gateway issued does not do what was asked, so the
    // token must not reach anyone: no file, and nothing on stdout.
    let dir = crate::testserver::scratch_dir("admin-mismatch");
    let file = dir.join("token");
    let body = r#"{"token":"BLOB","enrolment_id":"e","revoke_existing_credentials":false}"#;
    let path = file.to_str().unwrap();
    let args = ["issue-enrolment", "--user-id", "u", "--revoke-existing"];
    let (result, _, out, _) =
        execute_against(&[&args[..], &["--out", path]].concat(), "200 OK", body).await;
    assert!(
        matches!(result, Err(AdminError::EchoMismatch { .. })),
        "{result:?}"
    );
    assert!(!file.exists(), "the token file was written");
    assert!(out.is_empty());

    let (result, _, out, _) =
        execute_against(&[&args[..], &["--stdout"]].concat(), "200 OK", body).await;
    assert!(
        matches!(result, Err(AdminError::EchoMismatch { .. })),
        "{result:?}"
    );
    assert!(out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_parsed_revoke_existing_flag_is_what_goes_on_the_wire() {
    let body = r#"{"token":"BLOB","enrolment_id":"e","revoke_existing_credentials":true}"#;
    let args = [
        "issue-enrolment",
        "--user-id",
        "u",
        "--stdout",
        "--revoke-existing",
    ];
    let (result, sent, out, err) = execute_against(&args, "200 OK", body).await;
    result.unwrap();
    assert!(
        sent.contains(r#""revoke_existing_credentials":true"#),
        "{sent}"
    );
    assert_eq!(String::from_utf8(out).unwrap(), "BLOB\n");
    // The receipt names the enrolment, never the user id or the token.
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("enrolment e issued"), "{err}");
    assert!(!err.contains("BLOB") && !err.contains(" u "), "{err}");
}

#[tokio::test]
async fn an_existing_out_path_is_refused_before_the_gateway_is_asked() {
    // Asking first would mint a token with nowhere to go.
    let dir = crate::testserver::scratch_dir("admin-preflight");
    let file = dir.join("token");
    std::fs::write(&file, "theirs").unwrap();
    let args = [
        "issue-enrolment",
        "--user-id",
        "u",
        "--out",
        file.to_str().unwrap(),
    ];
    let (result, sent, _, _) = execute_against(&args, "200 OK", "{}").await;
    assert!(
        matches!(result, Err(AdminError::TokenFile(..))),
        "{result:?}"
    );
    assert!(sent.is_empty(), "the gateway was asked: {sent}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "theirs");
    std::fs::remove_dir_all(&dir).ok();
}

// ---- revoke-credentials (ADR-0945) ----

const ID: &str = "yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f";
const ID_JSON: &str =
    r#"{"user_id":"yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f","revoked":2}"#;
const BEARER: &str = "adm1n-t0ken";

async fn revoke_against(
    status: &'static str,
    body: &'static str,
) -> (Result<(), AdminError>, String, String, String) {
    let (result, sent, out, err) =
        execute_against(&["revoke-credentials", "--user-id", ID], status, body).await;
    (
        result,
        sent,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[tokio::test]
async fn the_revoke_request_is_exactly_the_user_id_with_the_callers_bearer() {
    let (result, sent, _, _) = revoke_against("200 OK", ID_JSON).await;
    assert!(result.is_ok(), "the revocation should succeed");
    assert!(
        sent.starts_with("POST /admin/revoke-credentials "),
        "wrong method or path"
    );
    assert!(
        sent.to_ascii_lowercase()
            .contains("authorization: bearer adm1n-t0ken"),
        "the bearer header is missing"
    );
    let body = &sent[sent.find("\r\n\r\n").unwrap() + 4..];
    assert_eq!(
        body,
        format!(r#"{{"user_id":"{ID}"}}"#),
        "the body must hold user_id and nothing else"
    );
}

#[test]
fn a_non_canonical_user_id_is_refused() {
    for bad in [
        "u-42",
        "018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f",
        "yadgar:user:018F3A52-7C1E-7B2A-9D4E-5A6B7C8D9E0F",
        "yadgar:user:018f3a52-7c1e-4b2a-9d4e-5a6b7c8d9e0f",
        "yadgar:user:018f3a527c1e7b2a9d4e5a6b7c8d9e0f",
        "yadgar:user:{018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f}",
        "yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f/x",
        "yadgar:team:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f",
        "yadgar:user:",
    ] {
        assert!(
            matches!(canonical_user_id(bad), Err(AdminError::NotCanonicalUserId)),
            "a non-canonical id was accepted"
        );
    }
    assert!(
        matches!(canonical_user_id("  "), Err(AdminError::EmptyUserId)),
        "an empty id must reuse the empty-id refusal"
    );
    assert_eq!(
        canonical_user_id(&format!("  {ID} ")).unwrap(),
        ID,
        "a canonical id is accepted after trimming"
    );
}

#[tokio::test]
async fn a_non_canonical_user_id_sends_no_request() {
    let (result, sent, _, _) =
        execute_against(&["revoke-credentials", "--user-id", "u-42"], "200 OK", "{}").await;
    assert!(
        matches!(result, Err(AdminError::NotCanonicalUserId)),
        "the id should be refused client-side"
    );
    assert!(sent.is_empty(), "a request reached the gateway");
}

#[tokio::test]
async fn an_echo_for_a_different_user_is_refused() {
    let (result, _, out, _) = revoke_against(
        "200 OK",
        r#"{"user_id":"yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e00","revoked":2}"#,
    )
    .await;
    assert!(
        matches!(result, Err(AdminError::RevokeEchoMismatch)),
        "a mismatched echo was accepted"
    );
    assert!(out.is_empty(), "a mismatch must not print a success line");
}

#[tokio::test]
async fn an_answer_without_a_count_is_refused() {
    let (result, _, _, _) = revoke_against("200 OK", r#"{"user_id":"x"}"#).await;
    assert!(
        matches!(result, Err(AdminError::MalformedRevocation(_))),
        "a receipt without a count was accepted"
    );
}

#[tokio::test]
async fn the_revoked_count_is_printed() {
    let (result, _, out, err) = revoke_against("200 OK", ID_JSON).await;
    assert!(result.is_ok(), "the revocation should succeed");
    assert!(
        out.contains("revoked 2 credentials"),
        "the count is missing"
    );
    assert!(!err.contains("warning"), "a non-zero count must not warn");
}

#[tokio::test]
async fn zero_revoked_warns_that_nothing_was_live() {
    let (result, _, _, err) = revoke_against(
        "200 OK",
        r#"{"user_id":"yadgar:user:018f3a52-7c1e-7b2a-9d4e-5a6b7c8d9e0f","revoked":0}"#,
    )
    .await;
    assert!(result.is_ok(), "zero revoked is still a success");
    assert!(
        err.contains("warning") && err.contains("nothing was live") && err.contains("retry"),
        "the zero-count warning is missing"
    );
}

#[tokio::test]
async fn forbidden_not_found_and_bad_request_are_distinct_and_actionable() {
    let (forbidden, _, _, _) = revoke_against("403 Forbidden", r#"{"error":"self"}"#).await;
    let forbidden = forbidden.unwrap_err();
    assert!(
        matches!(forbidden, AdminError::RevokeForbidden(_)),
        "403 has the wrong variant"
    );
    assert!(
        forbidden.to_string().contains("your own credentials"),
        "403 must mention that self-revocation is refused"
    );

    let (missing, _, _, _) = revoke_against("404 Not Found", r#"{"error":"no user"}"#).await;
    let missing = missing.unwrap_err();
    assert!(
        matches!(missing, AdminError::NoSuchUser(_)),
        "404 has the wrong variant"
    );
    assert!(
        missing.to_string().contains("no such user"),
        "404 must say the user does not exist"
    );

    let (bad, _, _, _) = revoke_against("400 Bad Request", r#"{"error":"bad body"}"#).await;
    assert!(
        matches!(bad.unwrap_err(), AdminError::BadRequest(_)),
        "400 has the wrong variant"
    );
}

#[tokio::test]
async fn no_secret_reaches_the_output_or_an_error() {
    let mut seen = Vec::new();
    for (status, body) in [
        ("200 OK", ID_JSON),
        ("401 Unauthorized", r#"{"error":"x"}"#),
        ("403 Forbidden", r#"{"error":"x"}"#),
        ("404 Not Found", r#"{"error":"x"}"#),
        ("400 Bad Request", r#"{"error":"x"}"#),
        ("503 Service Unavailable", r#"{"error":"x"}"#),
        ("429 Too Many Requests", r#"{"error":"x"}"#),
        ("200 OK", "not json"),
    ] {
        let (result, _, out, err) = revoke_against(status, body).await;
        seen.push(out);
        seen.push(err);
        if let Err(e) = result {
            seen.push(format!("{e} {e:?}"));
        }
    }
    for text in seen {
        assert!(!text.contains(BEARER), "the bearer leaked");
    }
}

#[test]
fn revoke_credentials_needs_a_user_id() {
    use clap::Parser as _;
    #[derive(clap::Parser)]
    struct Probe {
        #[command(subcommand)]
        command: AdminCommand,
    }
    assert!(
        Probe::try_parse_from(["probe", "revoke-credentials"]).is_err(),
        "--user-id is required"
    );
}

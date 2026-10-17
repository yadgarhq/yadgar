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
    let result = request(&client, &gateway, "adm1n-t0ken", Some("inst-1"), "u-42").await;
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
    assert_eq!(body, serde_json::json!({"user_id": "u-42"}));
}

#[tokio::test]
async fn each_refusal_says_what_the_person_can_do_about_it() {
    let (result, _) = ask("401 Unauthorized", r#"{"error":"x"}"#).await;
    assert!(matches!(result, Err(AdminError::Refused)), "{result:?}");

    let (result, _) = ask("403 Forbidden", r#"{"error":"x"}"#).await;
    assert!(matches!(result, Err(AdminError::NotAdmin)), "{result:?}");

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

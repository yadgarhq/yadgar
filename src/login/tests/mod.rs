//! What `login` and `enrol` do, decided without a gateway where possible and
//! read off a real socket where not.
//!
//! Split out of `login.rs` when the file passed its size ceiling. The seam is
//! the obvious one and the one `proxy` and `install` already use: the module
//! keeps the behaviour, the file beside it keeps the assertions about it.

use super::*;

/// What a re-login keeps, and what it must not carry across deployments.
mod reconcile;

#[test]
fn an_address_gets_exactly_one_trailing_slash() {
    // Without this, "https://gw" + "auth/login" becomes "https://gwauth/login"
    // — a DNS failure that reads as the gateway being down rather than as a
    // missing slash.
    assert_eq!(normalise("https://gw"), "https://gw/");
    assert_eq!(normalise("https://gw/"), "https://gw/");
    assert_eq!(normalise("https://gw///"), "https://gw/");
    assert_eq!(normalise("  https://gw  "), "https://gw/");
}

#[test]
fn a_port_survives_normalisation() {
    // The development gateway is on 18443, so eating the port would break
    // every local login.
    assert_eq!(
        normalise("https://gateway.yadgar.internal:18443"),
        "https://gateway.yadgar.internal:18443/"
    );
}

#[test]
fn the_login_url_is_pinned_as_a_whole_string() {
    // Both halves are already tested and the JOIN is what is not. A leading
    // slash on LOGIN_PATH gives "https://gw//auth/login": every test of
    // `normalise` still passes, every test of the constant still passes, and
    // the extra slash is the kind of thing nothing notices until a proxy in
    // front of the gateway matches on the path.
    assert_eq!(login_url("https://gw"), "https://gw/auth/login");
    assert_eq!(login_url("https://gw/"), "https://gw/auth/login");
    assert_eq!(
        login_url("https://gateway.yadgar.internal:18443"),
        "https://gateway.yadgar.internal:18443/auth/login"
    );
}

#[test]
fn a_401_is_a_refusal_and_every_other_failure_is_not() {
    // Deleting the 401 arm makes a wrong password read as "the gateway
    // answered with 401 Unauthorized" — technically true, and it sends
    // somebody to look at their network instead of retyping a password.
    assert_eq!(verdict(reqwest::StatusCode::UNAUTHORIZED), Verdict::Refused);
    assert_eq!(verdict(reqwest::StatusCode::OK), Verdict::Issued);
    assert_eq!(
        verdict(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
        Verdict::Unexpected
    );
    assert_eq!(verdict(reqwest::StatusCode::NOT_FOUND), Verdict::Unexpected);
}

#[tokio::test]
async fn no_refusal_can_tell_a_person_which_half_was_wrong() {
    // THE PROPERTY, not the sentence. `iam` returns one answer for wrong
    // password, unknown user and no-password-set so the endpoint cannot
    // enumerate accounts (D73). Restating the `#[error]` string would pass
    // whatever the code did with the body; this sends three 401s that say
    // different things and asserts the person is shown one identical
    // message, because the body is never read on this path at all.
    let mut seen = std::collections::BTreeSet::new();
    for body in [
        r#"{"detail":"no such user"}"#,
        r#"{"detail":"wrong password"}"#,
        r#"{"detail":"that account has no password set"}"#,
    ] {
        let (addr, served) = crate::testserver::answer_once("401 Unauthorized", body).await;
        let err = exchange(
            &reqwest::Client::new(),
            &format!("http://{addr}/"),
            "someone",
            "hunter2",
        )
        .await
        .expect_err("a 401 is not a login");
        let _ = served.await;
        assert!(matches!(err, LoginError::Refused), "got {err:?}");
        seen.insert(err.to_string());
    }
    assert_eq!(
        seen.len(),
        1,
        "the gateway's wording reached the person, and the endpoint now enumerates accounts: {seen:?}"
    );
}

#[tokio::test]
async fn a_refusal_and_an_outage_are_different_things_to_the_caller() {
    // THE PROPERTY, not the wording. The old test asserted the message did
    // not contain "gateway", so rewording it broke the test while the thing
    // that matters — that a caller can tell "retype your password" from
    // "check the network" — was never checked at all.
    let (addr, served) = crate::testserver::answer_once("401 Unauthorized", "{}").await;
    let refused = exchange(
        &reqwest::Client::new(),
        &format!("http://{addr}/"),
        "someone",
        "hunter2",
    )
    .await
    .expect_err("a 401 is not a login");
    let _ = served.await;

    // A port bound and immediately dropped: nothing is listening, so this is
    // a connection failure rather than an answer.
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);
    let unreachable = exchange(
        &reqwest::Client::new(),
        &format!("http://{dead_addr}/"),
        "someone",
        "hunter2",
    )
    .await
    .expect_err("nothing is listening there");

    assert!(matches!(refused, LoginError::Refused), "got {refused:?}");
    assert!(
        matches!(unreachable, LoginError::Unreachable(..)),
        "an unreachable gateway was reported as something else: {unreachable:?}"
    );
}

#[test]
fn the_machine_name_comes_from_the_environment_first() {
    // Windows sets COMPUTERNAME and nothing else; this arm is why the label
    // is not "unnamed machine" on two of the three platforms.
    assert_eq!(
        hostname_from(
            |k| (k == "COMPUTERNAME").then(|| "DESK-01".to_string()),
            Path::new("/nonexistent/etc/hostname"),
        ),
        Some("DESK-01".into())
    );
}

#[test]
fn an_empty_variable_falls_through_rather_than_naming_the_machine_nothing() {
    // A non-interactive shell can export HOSTNAME as an empty string, and a
    // credential labelled "" is worse than one labelled by the file below.
    let file = crate::testserver::scratch_dir("login-hostname").join("hostname");
    std::fs::write(&file, "workshop\n").unwrap();
    assert_eq!(
        hostname_from(|_| Some("   ".to_string()), &file),
        Some("workshop".into())
    );
    std::fs::remove_dir_all(file.parent().unwrap()).ok();
}

/// The `secret` FIELD of a real token, and the base64 envelope carrying it.
///
/// Both are needed, because the contract's rule is which of the two goes on
/// the wire — "the `secret` field of the decoded EnrolmentToken, never the
/// whole blob" — and a test holding only one cannot tell them apart.
const A_TOKEN: &str = "CjRjMlZ1ZEdsdVpXd3RjMlZqY21WMExXNXZkQzFoTFdoaGNtUmpiMlJsTFdGaFlXRmhZUT09EiBodHRwczovL2d3LnNlbnRpbmVsLmludmFsaWQ6MTk5OSIGCICgsLQD";
const ITS_SECRET: &str = "c2VudGluZWwtc2VjcmV0LW5vdC1hLWhhcmRjb2RlLWFhYWFhYQ==";

#[test]
fn the_enrolment_url_is_pinned_as_a_whole_string() {
    // Same rule as `login_url`, same reason: both halves can be right while
    // the join makes `https://gw//auth/enrol`, which every test of either
    // half still passes and some proxy in front of the gateway does not.
    assert_eq!(enrol_url("https://gw"), "https://gw/auth/enrol");
    assert_eq!(enrol_url("https://gw/"), "https://gw/auth/enrol");
    assert_ne!(enrol_url("https://gw"), login_url("https://gw"));
}

#[tokio::test]
async fn the_secret_field_goes_on_the_wire_and_never_the_whole_blob() {
    // THE CONTRACT'S RULE, read off a socket. `RedeemEnrolment` takes "the
    // `secret` field of the decoded EnrolmentToken, never the whole blob",
    // and a client that posted the envelope would be refused as a wrong
    // secret while holding a token that is perfectly good — a failure that
    // reads as the admin's fault and is not.
    let (addr, served) = crate::testserver::answer_once(
        "200 OK",
        r#"{"token":"tok-from-the-gateway","username":"sentinel.person"}"#,
    )
    .await;
    let answer = redeem(
        &reqwest::Client::new(),
        &format!("http://{addr}/"),
        ITS_SECRET,
        "a-new-password",
    )
    .await
    .expect("a 200 is an enrolment");

    let sent = served.await.unwrap();
    assert!(
        sent.contains(&format!(r#""secret":"{ITS_SECRET}""#)),
        "the secret field never left the process; the request was:\n{sent}"
    );
    assert!(
        !sent.contains(A_TOKEN),
        "the whole enrolment blob was posted instead of its secret field; the request was:\n{sent}"
    );
    // The path, on the wire, and NOT `auth/login`: the two endpoints take
    // different bodies, and a redemption sent to `login` is a 401 that
    // names a username nobody supplied.
    assert!(sent.starts_with("POST /auth/enrol "), "{sent}");
    // Both halves of the answer are kept. Dropping the username leaves a
    // person enrolled and unable to log in on any second machine.
    assert_eq!(answer.token, "tok-from-the-gateway");
    assert_eq!(answer.username, "sentinel.person");
}

#[tokio::test]
async fn a_refused_secret_does_not_report_a_wrong_username_or_password() {
    // An enrolment names no username, so the login wording sends somebody
    // hunting for an account they do not have yet. It must also stay silent
    // on WHY: replayed and never-existed answer identically, because the
    // endpoint is unauthenticated and telling them apart says whether a
    // given secret ever existed.
    let (addr, served) =
        crate::testserver::answer_once("401 Unauthorized", r#"{"detail":"already redeemed"}"#)
            .await;
    let err = redeem(
        &reqwest::Client::new(),
        &format!("http://{addr}/"),
        ITS_SECRET,
        "a-new-password",
    )
    .await
    .expect_err("a 401 is not an enrolment");
    let _ = served.await;

    assert!(matches!(err, LoginError::SecretRefused), "got {err:?}");
    let said = err.to_string();
    assert!(!said.contains("username"), "{said}");
    assert!(!said.contains("already redeemed"), "{said}");
}

#[test]
fn an_expired_token_is_named_as_expired_rather_than_as_a_bad_secret() {
    // The gateway cannot say this: it refuses an expired secret exactly as
    // it refuses an unknown one, deliberately. So the only place a person
    // can be told their token simply timed out is before the request.
    //
    // The fixture is 1999-01-01 against a clock of 1999-01-02 — values no
    // implementation reading the real clock could produce.
    assert!(expired(Some(915_148_800), 915_235_200));
    // One second before the deadline is not expired; the deadline itself is.
    assert!(!expired(Some(915_148_800), 915_148_799));
    assert!(expired(Some(915_148_800), 915_148_800));
    // A token carrying no expiry is not expired. Refusing it would refuse a
    // token a newer `iam` is entitled to mint without the field.
    assert!(!expired(None, 915_235_200));
}

#[tokio::test]
async fn enrolment_refuses_a_token_it_cannot_read_before_asking_for_a_password() {
    // The order matters and nothing else can check it: a client that
    // prompted first would ask somebody to invent a password twice and
    // THEN tell them the blob was unusable. This returns without reading
    // stdin at all, which is why the test can run with no terminal.
    let dir = crate::testserver::scratch_dir("enrol-refusal");
    let err = enrol(&dir, "not a token at all!")
        .await
        .expect_err("that is not a token");
    assert!(
        matches!(err, LoginError::Enrolment(EnrolmentError::NotBase64)),
        "got {err:?}"
    );
    assert!(
        !dir.join("config.json").exists(),
        "a refused enrolment wrote a config"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_machine_that_will_not_say_its_name_is_labelled_rather_than_left_blank() {
    assert_eq!(
        hostname_from(|_| None, Path::new("/nonexistent/etc/hostname")),
        None
    );
    assert_eq!(
        label_from(|_| None, Path::new("/nonexistent/etc/hostname")),
        "unnamed machine"
    );
}

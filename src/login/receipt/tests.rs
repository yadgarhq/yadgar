//! What `login` and `enrol` print once a credential is stored.

use super::*;

const ESC_GATEWAY: &str = "https://gw\u{1b}]52;c;aGk=\u{7}/";

#[test]
fn the_login_receipt_escapes_a_gateway_address_with_control_characters() {
    let dir = crate::testserver::scratch_dir("receipt-login");
    let config = Config::new(&dir, ESC_GATEWAY.into(), "tok".into());
    let line = logged_in(&config, &dir);
    assert!(
        !line.contains('\u{1b}') && !line.contains('\u{7}'),
        "{line:?}"
    );
    assert!(
        line.starts_with(r"logged in to https://gw\u{1b}]52;c;aGk="),
        "{line:?}"
    );
}

#[test]
fn the_enrol_receipt_escapes_the_gateway_and_the_username() {
    let dir = crate::testserver::scratch_dir("receipt-enrol");
    let config = Config::new(&dir, ESC_GATEWAY.into(), "tok".into())
        .with_username(Some("bob\u{1b}[2J\u{202e}".into()));
    let line = enrolled(&config);
    assert!(
        !line.contains('\u{1b}') && !line.contains('\u{7}') && !line.contains('\u{202e}'),
        "{line:?}"
    );
    assert!(line.ends_with(r"as bob\u{1b}[2J\u{202e}"), "{line:?}");
}

#[test]
fn an_enrolment_that_named_nobody_says_so() {
    let dir = crate::testserver::scratch_dir("receipt-nobody");
    let config = Config::new(&dir, "https://gw/".into(), "tok".into());
    assert_eq!(
        enrolled(&config),
        "enrolled with https://gw/ as (the gateway named nobody)"
    );
}

#[test]
fn the_enrolling_line_escapes_the_address_the_token_carried() {
    let line = enrolling(ESC_GATEWAY);
    assert!(!line.contains('\u{1b}'), "{line:?}");
    assert!(line.starts_with("Enrolling with https://gw"), "{line:?}");
}

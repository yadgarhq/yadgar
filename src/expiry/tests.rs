//! The stored credential expiry: how it is read, when it counts as past, and
//! how it is shown.

use super::*;

#[test]
fn format_utc_is_the_inverse_of_parse_across_eras_and_leap_days() {
    for text in [
        "1970-01-01T00:00:00Z",
        "2000-02-29T23:59:59Z",
        "2026-10-23T12:34:56Z",
        "2100-03-01T00:00:01Z",
        "1969-12-31T23:59:59Z",
    ] {
        let secs = parse(text).unwrap();
        assert_eq!(format_utc(secs), text, "{secs}");
    }
    assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
}

#[test]
fn a_whole_second_utc_time_parses_to_its_unix_seconds() {
    assert_eq!(parse("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse("2026-10-21T06:22:04Z"), Some(1_792_563_724));
}

#[test]
fn every_fraction_width_prost_emits_parses_and_is_truncated() {
    // `prost_types::Timestamp`'s Display writes 0, 3, 6 or 9 digits.
    for text in [
        "2026-10-21T06:22:04Z",
        "2026-10-21T06:22:04.305Z",
        "2026-10-21T06:22:04.305156Z",
        "2026-10-21T06:22:04.305156789Z",
    ] {
        assert_eq!(parse(text), Some(1_792_563_724), "{text}");
    }
}

#[test]
fn a_leap_day_and_the_last_second_of_a_year_parse() {
    assert_eq!(parse("2024-02-29T12:00:00Z"), Some(1_709_208_000));
    assert_eq!(parse("1999-12-31T23:59:59Z"), Some(946_684_799));
}

#[test]
fn anything_but_rfc3339_utc_is_not_a_time() {
    for text in [
        "",
        "unknown",
        "2026-10-21",
        "2026-10-21 06:22:04Z",
        "2026-10-21T06:22:04",
        "2026-10-21T06:22:04+02:00",
        "2026-13-01T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-10-21T24:00:00Z",
        "2026-10-21T06:60:00Z",
        "2026-10-21T06:22:04.Z",
        "2026-10-21T06:22:04.12aZ",
        "+026-10-21T06:22:04Z",
    ] {
        assert_eq!(parse(text), None, "{text:?}");
    }
}

#[test]
fn a_stored_expiry_at_or_before_now_is_past() {
    let at = Some("2026-10-21T06:22:04Z");
    assert_eq!(expired_at(at, 1_792_563_724), at);
    assert_eq!(expired_at(at, 1_792_563_725), at);
    assert_eq!(expired_at(at, 1_792_563_723), None);
}

#[test]
fn no_expiry_or_an_unreadable_one_is_never_past() {
    // A legacy credential has none and keeps working (ADR-0972 (3)); an
    // unreadable value must not turn the gateway's own reason into a guess.
    assert_eq!(expired_at(None, i64::MAX), None);
    assert_eq!(expired_at(Some("yesterday"), i64::MAX), None);
}

#[test]
fn the_shown_expiry_is_the_stored_text_or_unknown() {
    assert_eq!(shown(Some("2026-10-21T06:22:04Z")), "2026-10-21T06:22:04Z");
    assert_eq!(shown(None), "unknown");
}

#[test]
fn the_expired_sentence_names_the_time_and_the_fix() {
    assert_eq!(
        expired_sentence("2026-10-21T06:22:04Z"),
        "your credential expired at 2026-10-21T06:22:04Z; run `yaadgaar login`"
    );
}

#[test]
fn a_stored_expiry_that_does_not_parse_is_never_shown_raw() {
    // A tampered config must not reach the terminal: only a value the strict
    // parser accepts (digits, `-`, `:`, `T`, `.`, `Z`) is shown as stored.
    assert_eq!(shown(Some("2999-01-01T00:00:00Z\u{1b}[2J")), "unreadable");
    assert_eq!(shown(Some("\u{1b}]52;c;aGk=\u{7}")), "unreadable");
}

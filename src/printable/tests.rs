//! The one terminal sanitiser for gateway- and file-derived text.

use super::*;

#[test]
fn escape_sequences_never_survive_raw() {
    for raw in [
        "\u{1b}]52;c;aGk=\u{7}",
        "\u{1b}[2J",
        "\u{9b}2J",
        "\u{1b}]8;;https://evil/\u{1b}\\link\u{1b}]8;;\u{1b}\\",
        "a\u{7f}b\rc\n",
    ] {
        let out = printable(raw);
        assert!(!out.chars().any(char::is_control), "{raw:?} -> {out:?}");
    }
}

#[test]
fn a_control_character_is_shown_escaped_not_silently_dropped() {
    assert_eq!(printable("x\u{1b}[2J"), r"x\u{1b}[2J");
    assert_eq!(printable("a\tb"), r"a\tb");
}

#[test]
fn printable_unicode_is_kept_as_is() {
    assert_eq!(printable("laptop — Zoë's 💻"), "laptop — Zoë's 💻");
}

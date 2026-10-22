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

#[test]
fn bidi_overrides_and_zero_width_format_characters_are_escaped() {
    for c in [
        '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}',
        '\u{2068}', '\u{2069}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{200e}', '\u{200f}',
        '\u{feff}', '\u{061c}',
    ] {
        let out = printable(&format!("a{c}b"));
        assert!(!out.contains(c), "{c:?} -> {out:?}");
        assert_eq!(out, format!("a\\u{{{:x}}}b", c as u32));
    }
}

/// U+2028 and U+2029 are Zl and Zp, not Cc or Cf, so neither check above
/// catches them — and a JSON or JavaScript reader, or a model, takes either as
/// a line break. Checked by character, not with `str::lines()`, which splits on
/// `\n` only and would pass a raw separator straight through.
#[test]
fn line_and_paragraph_separators_are_escaped() {
    for c in ['\u{2028}', '\u{2029}'] {
        let out = printable(&format!("a{c}- #999 forged"));
        assert!(!out.contains(c), "{c:?} -> {out:?}");
        assert_eq!(out, format!("a\\u{{{:x}}}- #999 forged", c as u32));
    }
}

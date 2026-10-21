//! Gateway-, token- and file-derived text, made safe for a terminal.
//!
//! A compromised gateway, a crafted enrolment token or a tampered
//! `config.json` could otherwise write escape sequences to the operator's
//! terminal (an OSC 52 clipboard write, an OSC 8 spoofed hyperlink, a title
//! change, a screen clear) or reorder what is shown with a bidi override. ONE
//! sanitiser, so every printed string from those sources gets the same rule.

/// *text* with every control character (C0, DEL and C1, so ESC, BEL and the
/// 8-bit CSI `U+009B` too) and every invisible format character (bidi
/// controls, zero-width characters, the BOM) shown escaped, as `\u{1b}` or
/// `\u{202e}`. Printable Unicode is kept as is. Escaped rather than dropped,
/// so the operator sees that something odd was sent.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_debug().to_string()
            } else if is_format(c) {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Unicode general category `Cf` (format): invisible characters that change
/// how the text around them is shown.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

#[cfg(test)]
mod tests;

//! Text from the gateway or a file, made safe for a terminal.
//!
//! A compromised gateway or a tampered `config.json` could otherwise write
//! escape sequences to the operator's terminal: an OSC 52 clipboard write, an
//! OSC 8 spoofed hyperlink, a title change, a screen clear. ONE sanitiser, so
//! every printed gateway- or file-derived string gets the same rule.

/// *text* with every control character (C0, DEL and C1, so ESC, BEL and the
/// 8-bit CSI `U+009B` too) shown escaped, as `\u{1b}`. Printable Unicode is
/// kept as is. Escaped rather than dropped, so the operator sees that the
/// gateway sent something odd.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;

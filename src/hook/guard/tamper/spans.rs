//! Expansion spans: `$(…)`, `${…}`, backticks. Split out of `expand.rs` at
//! its size ceiling.
//!
//! [`mask`] runs on the RAW line, before the scanner: the scanner has no
//! notion of a span and cuts one at its spaces and at its `(`, so
//! `` rm `echo ~/.claude`/y* `` reached the checks as three words, and which
//! backtick opened and which closed could no longer be told. Each span the
//! shell would expand becomes one [`EXPANSION`] there, so it stays one word.
//! What is left for [`collapse`] are spans the mask could not close.

use super::expand::EXPANSION;
use super::{CLOSE_BRACE, OPEN_BRACE};

/// *line* with each backtick span, `$(…)` and `${…}` that is not a plain
/// `${NAME}` replaced by [`EXPANSION`], read as the shell reads it: nothing
/// inside single quotes, and a backslash escapes the next character. A span
/// with no closer is left as it is.
///
/// The text inside a span is still checked: the guard runs every `$(…)` and
/// backtick body as a line of its own.
pub(super) fn mask(line: &str) -> String {
    let c: Vec<char> = line.chars().collect();
    let closes = Closers::of(&c);
    let mut out = String::new();
    let (mut single, mut double) = (false, false);
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if single {
            single = ch != '\'';
            out.push(ch);
            i += 1;
            continue;
        }
        let span_end = match ch {
            '`' => closes.backtick[i],
            '$' if c.get(i + 1) == Some(&'(') => closes.paren[i + 1],
            '$' if c.get(i + 1) == Some(&OPEN_BRACE) => closes
                .brace
                .get(i + 2)
                .copied()
                .flatten()
                .filter(|&j| !is_plain_name(&c[i + 2..j])),
            _ => None,
        };
        if let Some(j) = span_end {
            out.push(EXPANSION);
            i = j + 1;
            continue;
        }
        match ch {
            '\\' => {
                out.push(ch);
                if let Some(&next) = c.get(i + 1) {
                    out.push(next);
                }
                i += 2;
                continue;
            }
            '\'' if !double => single = true,
            '"' => double = !double,
            _ => {}
        }
        out.push(ch);
        i += 1;
    }
    out
}

fn is_plain_name(chars: &[char]) -> bool {
    chars
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
        && chars.iter().all(|c| c.is_ascii_alphanumeric() || *c == '_')
}

/// For each index, where the span opened there closes, found in one pass
/// each, so a line of 30 000 `$(` is not scanned 30 000 times.
struct Closers {
    /// At a backtick: the next unescaped backtick after it.
    backtick: Vec<Option<usize>>,
    /// At a `(`: its matching `)`, counted outside single quotes.
    paren: Vec<Option<usize>>,
    /// At any index: the next closing brace at or after it.
    brace: Vec<Option<usize>>,
}

impl Closers {
    fn of(c: &[char]) -> Self {
        let n = c.len();
        let mut backtick = vec![None; n];
        let mut paren = vec![None; n];
        let mut brace = vec![None; n + 1];
        let mut escaped = vec![false; n];
        let mut i = 0;
        while i < n {
            if c[i] == '\\' && i + 1 < n {
                escaped[i + 1] = true;
                i += 2;
            } else {
                i += 1;
            }
        }
        let mut next_backtick = None;
        let mut next_brace = None;
        for i in (0..n).rev() {
            if c[i] == CLOSE_BRACE {
                next_brace = Some(i);
            }
            brace[i] = next_brace;
            if c[i] == '`' && !escaped[i] {
                backtick[i] = next_backtick;
                next_backtick = Some(i);
            }
        }
        let mut stack = Vec::new();
        let mut single = false;
        for i in 0..n {
            match c[i] {
                _ if escaped[i] => {}
                '\'' => single = !single,
                '(' if !single => stack.push(i),
                ')' if !single => {
                    if let Some(open) = stack.pop() {
                        paren[open] = Some(i);
                    }
                }
                _ => {}
            }
        }
        Self {
            backtick,
            paren,
            brace,
        }
    }
}

/// *word* with each `$NAME`, `${…}`, `$(…)`, `$'…'` or backtick span, and
/// each `$` that opens nothing the scanner kept, replaced by [`EXPANSION`].
///
/// The scanner cuts a span at its spaces, so a word can hold half of one. A
/// `$(` or `${` with no closer opens a span that runs to the word's end. A
/// lone backtick may OPEN a span running to the end (`` ~/.claude/`echo ``)
/// or CLOSE one that began in an earlier word, so that everything before it
/// is the span's output (`` ~/.claude/x`/y* `` from `` `dirname ~/.claude/x`/y* ``):
/// both readings are returned. So is the reading in which the word's FIRST
/// backtick closes such a span and the rest pair up after it:
/// `` ~/.claude`/`echo `` is the end of one span and the start of the next.
pub(super) fn collapse(word: &str) -> Vec<String> {
    let mut readings = collapse_paired(word);
    if let Some(first) = word.find('`') {
        readings.extend(
            collapse_paired(&word[first + 1..])
                .into_iter()
                .map(|r| format!("{EXPANSION}{r}")),
        );
    }
    readings
}

fn collapse_paired(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let end = match chars[i] {
            '$' => match chars.get(i + 1) {
                Some(&OPEN_BRACE) => closing(&chars, i + 1, OPEN_BRACE, CLOSE_BRACE),
                Some('(') => closing(&chars, i + 1, '(', ')'),
                _ => Some(
                    i + 1
                        + chars[i + 1..]
                            .iter()
                            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                            .count(),
                ),
            }
            .unwrap_or(chars.len()),
            '`' => match chars[i + 1..].iter().position(|&c| c == '`') {
                Some(n) => i + n + 2,
                None => {
                    let rest: String = chars[i + 1..].iter().collect();
                    let mut readings = vec![format!("{out}{EXPANSION}")];
                    readings.extend(
                        collapse_paired(&rest)
                            .into_iter()
                            .map(|r| format!("{EXPANSION}{r}")),
                    );
                    return readings;
                }
            },
            c => {
                out.push(c);
                i += 1;
                continue;
            }
        };
        out.push(EXPANSION);
        i = end.min(chars.len());
    }
    vec![out]
}

/// The index just past the bracket that closes the one at *open*, if any.
fn closing(chars: &[char], open: usize, left: char, right: char) -> Option<usize> {
    let mut depth = 0;
    for (i, &c) in chars.iter().enumerate().skip(open) {
        if c == left {
            depth += 1;
        } else if c == right {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_is_one_expansion_whatever_it_holds() {
        let one = |w: &str| collapse(w).join("|");
        assert_eq!(one("`adr/`"), "\0|\0adr/\0|\0\0");
        assert_eq!(one("${b:-x/y}z$(c/d)"), "\0z\0");
        assert_eq!(one(".${X:-claude}"), ".\0");
        assert_eq!(one("$(cat a/b"), "\0", "an open span runs to the end");
        assert_eq!(one("$\\x2eclaude"), "\0\\x2eclaude");
        assert_eq!(one("$1/$@"), "\0/\0@");
        assert!(
            collapse("~/.claude`/`echo").contains(&"\0/\0".to_string()),
            "the first backtick may close a span"
        );
        assert_eq!(
            one("~/.claude/x`/y*"),
            "~/.claude/x\0|\0/y*|\0/y*",
            "a lone backtick opens a span, or closes one"
        );
    }

    #[test]
    fn the_mask_keeps_a_span_one_word_and_leaves_quoted_text_alone() {
        assert_eq!(mask("rm `echo ~/.claude`/y*"), "rm \0/y*");
        assert_eq!(mask("rm $(echo ~/.claude a)/y*"), "rm \0/y*");
        assert_eq!(mask("rm ${D:-a b}/y ${D}"), "rm \0/y ${D}");
        assert_eq!(mask("echo '`'; rm `x`"), "echo '`'; rm \0");
        assert_eq!(mask("echo \\`x\\`"), "echo \\`x\\`");
        assert_eq!(mask("rm `open"), "rm `open", "an unclosed span is left");
    }
}

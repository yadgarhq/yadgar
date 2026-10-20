//! What the shell may put in place of a `$`-expansion or a backtick span.
//!
//! Split out of `tamper.rs` at its size ceiling. Two steps, in this order:
//!
//! 1. [`substitute`]: a `$NAME` or `${NAME}` whose NAME is SET ON THE SAME
//!    LINE (`NAME=value`, `export NAME=value`, `for NAME in a b c`) is replaced
//!    by each value it is set to there. A variable can hold only a value from
//!    this line or from the environment the session started with, and a
//!    command cannot change that environment for a later call; so a variable
//!    set here holds what it is set to here.
//! 2. [`collapse`]: every expansion left (`$NAME`, `${…}`, `$(…)`, a backtick
//!    pair) becomes one [`EXPANSION`] character, BEFORE the word is cut at
//!    `/`. A span holds `/` of its own: `` `adr/` `` in a Markdown heredoc was
//!    cut in two and read as `*/*`. An expansion whose text names `claude`
//!    (`$CLAUDE_CONFIG_DIR`, `$(dirname ~/.claude/x)`) becomes `.claude`
//!    instead, since that is the directory it may produce.

use std::collections::HashMap;

use super::{CLOSE_BRACE, MAX_ALTERNATIVES, OPEN_BRACE};
use crate::install::shellword::SimpleCommand;

/// The text of one expansion, after [`collapse`]. NUL cannot occur in a
/// shell word, so it cannot be confused with a character that was typed.
pub(super) const EXPANSION: char = '\0';

/// Each variable set on the line, and every value it is set to.
pub(super) type Bindings = HashMap<String, Vec<String>>;

/// The variables *commands* set: any `NAME=value` word, and the words of a
/// `for NAME in …` list.
///
/// Read wide on purpose: a word like `-c x=y` binds `x` too. Binding a name
/// that the shell does not bind only replaces its value, and the value of a
/// name nothing on the line sets comes from the session's environment, which
/// no command of this call controls.
pub(super) fn bindings(commands: &[SimpleCommand]) -> Bindings {
    let mut out = Bindings::new();
    for command in commands {
        // `do for f in …` after a `;` is still a `for`.
        let skip = command
            .words
            .iter()
            .take_while(|w| matches!(w.as_str(), "do" | "then" | "else" | "!"))
            .count();
        let words = &command.words[skip..];
        if words.first().is_some_and(|w| w == "for") && words.get(2).is_some_and(|w| w == "in") {
            if let Some(name) = words.get(1).filter(|n| is_name(n)) {
                out.entry(name.clone())
                    .or_default()
                    .extend(words[3..].iter().cloned());
            }
            continue;
        }
        for word in words {
            if let Some((name, value)) = word.split_once('=').filter(|(n, _)| is_name(n)) {
                out.entry(name.to_string())
                    .or_default()
                    .push(value.to_string());
            }
        }
    }
    out
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// How many times a value is itself substituted into: `T=$S/x` and
/// `for f in $d/*` set a variable from another one.
const MAX_PASSES: usize = 3; // ADR-0569-EXCEPTION(CB): bound on expanding untrusted input, not a per-install knob.

/// *word* with each `$NAME` / `${NAME}` that *bindings* holds replaced by
/// each of its values, the values' own references too up to [`MAX_PASSES`],
/// or `None` past [`MAX_ALTERNATIVES`].
pub(super) fn substitute(word: &str, bindings: &Bindings) -> Option<Vec<String>> {
    let mut words = vec![word.to_string()];
    for _ in 0..MAX_PASSES {
        let mut next = Vec::new();
        for w in &words {
            next.extend(substitute_once(w, bindings)?);
            if next.len() > MAX_ALTERNATIVES {
                return None;
            }
        }
        if next == words {
            break;
        }
        words = next;
    }
    Some(words)
}

fn substitute_once(word: &str, bindings: &Bindings) -> Option<Vec<String>> {
    let chars: Vec<char> = word.chars().collect();
    let mut alternatives = vec![String::new()];
    let mut i = 0;
    while i < chars.len() {
        let bound = reference(&chars, i).and_then(|(name, end)| Some((bindings.get(&name)?, end)));
        match bound {
            Some((values, end)) => {
                alternatives = alternatives
                    .iter()
                    .flat_map(|a| values.iter().map(move |v| format!("{a}{v}")))
                    .collect();
                if alternatives.len() > MAX_ALTERNATIVES {
                    return None;
                }
                i = end;
            }
            None => {
                alternatives.iter_mut().for_each(|a| a.push(chars[i]));
                i += 1;
            }
        }
    }
    Some(alternatives)
}

/// The NAME of a plain `$NAME` or `${NAME}` at *at*, and the index after it.
fn reference(chars: &[char], at: usize) -> Option<(String, usize)> {
    if chars[at] != '$' {
        return None;
    }
    let braced = chars.get(at + 1) == Some(&OPEN_BRACE);
    let start = at + 1 + usize::from(braced);
    let len = chars[start..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
        .count();
    let name: String = chars[start..start + len].iter().collect();
    if !is_name(&name) {
        return None;
    }
    let end = start + len;
    if !braced {
        return Some((name, end));
    }
    // `${NAME:-x}` and the like are not a plain reference: left to `collapse`.
    (chars.get(end) == Some(&CLOSE_BRACE)).then(|| (name, end + 1))
}

/// *word* with each `$NAME`, `${…}`, `$(…)` or backtick span replaced by
/// [`EXPANSION`], or by `.claude` when the span's text names `claude`.
///
/// An opener with no closer in the word is one [`EXPANSION`] on its own, and
/// the text after it is read on: the scanner cuts a span at its spaces, so in
/// `` `dirname ~/.claude/x`/y* `` the second word starts inside the span and
/// its backtick CLOSES one.
pub(super) fn collapse(word: &str) -> String {
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
            .unwrap_or(i + 2),
            '`' => chars[i + 1..]
                .iter()
                .position(|&c| c == '`')
                .map_or(i + 1, |n| i + n + 2),
            c => {
                out.push(c);
                i += 1;
                continue;
            }
        };
        let span: String = chars[i..end].iter().collect();
        if span.to_ascii_lowercase().contains("claude") {
            out.push_str(".claude");
        } else {
            out.push(EXPANSION);
        }
        i = end;
    }
    out
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
    use crate::install::shellword::shell_commands;

    #[test]
    fn a_variable_set_on_the_line_takes_its_values_there() {
        let b = bindings(&shell_commands(
            "D=~/.claude; export E=/tmp; for w in a b; do :; done",
        ));
        assert_eq!(substitute("$D/x", &b).unwrap(), ["~/.claude/x"]);
        assert_eq!(substitute("${E}/$w", &b).unwrap(), ["/tmp/a", "/tmp/b"]);
        assert_eq!(
            substitute("$UNSET/${E:-x}", &b).unwrap(),
            ["$UNSET/${E:-x}"]
        );
        let b = bindings(&shell_commands(
            "for d in a; do for f in $d/*; do :; done; done",
        ));
        assert_eq!(
            substitute("$f", &b).unwrap(),
            ["a/*"],
            "a value's own references"
        );
        let b = bindings(&shell_commands("P=$P:x"));
        assert_eq!(substitute("$P", &b).unwrap(), ["$P:x:x:x"], "bounded");
        let many = format!("for v in {}; do :; done", "x ".repeat(20));
        let b = bindings(&shell_commands(&many));
        assert!(substitute("$v/$v", &b).is_none(), "past the bound refuses");
    }

    #[test]
    fn a_span_is_one_expansion_whatever_it_holds() {
        assert_eq!(collapse("`adr/`"), "\0");
        assert_eq!(collapse("$a/${b}x$(c/d)"), "\0/\0x\0");
        assert_eq!(collapse("$CLAUDE_CONFIG_DIR/$f"), ".claude/\0");
        assert_eq!(collapse("$(echo ~/.claude)/y"), ".claude/y");
        assert_eq!(collapse("x`/y*"), "x\0/y*", "a lone backtick eats nothing");
        assert_eq!(collapse("$(cat"), "\0cat");
    }
}

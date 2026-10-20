//! What the shell may put in place of a `$`-expansion or a backtick span.
//!
//! Split out of `tamper.rs` at its size ceiling. Two steps, in this order:
//!
//! 1. [`substitute`]: a `$NAME` or `${NAME}` whose NAME is set on the same
//!    line is replaced by each value the line sets it to (see [`bindings`]).
//!    Unless the guard can tell the binding is IN FORCE where the variable is
//!    read, the reference itself is kept as one more alternative, standing
//!    for the value the variable may hold from the environment.
//! 2. [`collapse`]: every expansion left (`$NAME`, `${…}`, `$(…)`, a backtick
//!    pair) becomes one [`EXPANSION`] character, BEFORE the word is cut at
//!    `/`. A span holds `/` of its own: `` `adr/` `` in a Markdown heredoc was
//!    cut in two and read as `*/*`. An expansion whose text names `claude`
//!    (`$CLAUDE_CONFIG_DIR`, `$(dirname ~/.claude/x)`) becomes `.claude`
//!    instead, since that is the directory it may produce.

use super::{CLOSE_BRACE, MAX_ALTERNATIVES, OPEN_BRACE};
use crate::install::shellword::SimpleCommand;

/// The text of one expansion, after [`collapse`]. NUL cannot occur in a
/// shell word, so it cannot be confused with a character that was typed.
pub(super) const EXPANSION: char = '\0';

/// One value the line may give a variable.
pub(super) struct Binding {
    name: String,
    value: String,
    /// The index of the command that sets it.
    at: usize,
    /// For a `for` loop's variable: the index of the loop's `done`. Inside the
    /// loop body the variable HOLDS one of the loop's words, and nothing else.
    body_until: Option<usize>,
}

/// Every variable binding on the line.
pub(super) type Bindings = Vec<Binding>;

/// The variables *commands* may set: any `NAME=value` word, and the words of
/// a `for NAME in …` list.
///
/// An assignment is NEVER taken as in force where the variable is read. It
/// may be a prefix (`F=x cmd` sets nothing after `cmd`), in a subshell
/// (`(F=x)`), behind a condition (`false && F=x`), after the read, inside a
/// heredoc body, or not an assignment at all (`git -c F=x`). Then the read
/// sees the environment, and the environment is the user's profile, which an
/// earlier call can have changed. So an assignment ADDS its value as an
/// alternative and replaces nothing: it decides only what a variable may
/// hold, never what it may not.
///
/// A `for` loop's variable is in force inside the loop's body, and there it
/// replaces the reference. A non-empty literal list runs the body with each
/// of its words, whatever conditions surround the loop.
pub(super) fn bindings(commands: &[SimpleCommand]) -> Bindings {
    let mut out = Bindings::new();
    for (at, command) in commands.iter().enumerate() {
        let words = after_keywords(&command.words);
        if words.first().is_some_and(|w| w == "for") && words.get(2).is_some_and(|w| w == "in") {
            if let Some(name) = words.get(1).filter(|n| is_name(n)) {
                let until = matching_done(commands, at);
                out.extend(words[3..].iter().map(|value| Binding {
                    name: name.clone(),
                    value: value.clone(),
                    at,
                    body_until: Some(until),
                }));
            }
            continue;
        }
        for word in words {
            // A value holding whitespace is not taken: unquoted, whitespace
            // ends the word, so it came from quotes, and in practice it is a
            // whole `bash -c 'S=…; …'` script read as one `S=` word. Leaving
            // the variable unbound gives it the environment's value, the
            // alternative every assignment keeps anyway.
            if let Some((name, value)) = word
                .split_once('=')
                .filter(|(n, v)| is_name(n) && !v.contains(char::is_whitespace))
            {
                out.push(Binding {
                    name: name.to_string(),
                    value: value.to_string(),
                    at,
                    body_until: None,
                });
            }
        }
    }
    out
}

/// *words* after the reserved words that may open a command: `do for f in …`
/// after a `;` is still a `for`.
fn after_keywords(words: &[String]) -> &[String] {
    let skip = words
        .iter()
        .take_while(|w| matches!(w.as_str(), "do" | "then" | "else" | "!"))
        .count();
    &words[skip..]
}

/// The index of the `done` that closes the loop opened at *open*, or the
/// line's end.
fn matching_done(commands: &[SimpleCommand], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, command) in commands.iter().enumerate().skip(open) {
        let words = after_keywords(&command.words);
        match words.first().map(String::as_str) {
            Some("for" | "while" | "until" | "select") => depth += 1,
            Some("done") => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    commands.len()
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// How deep a value is itself substituted into: `T=$S/x` and
/// `for f in $d/*` set a variable from another one.
const MAX_DEPTH: usize = 3; // ADR-0569-EXCEPTION(CB): bound on expanding untrusted input, not a per-install knob.

/// *word*, read by the command at index *at*, with each `$NAME` / `${NAME}`
/// the line binds replaced by each value it may hold there, or `None` past
/// [`MAX_ALTERNATIVES`].
pub(super) fn substitute(word: &str, bindings: &Bindings, at: usize) -> Option<Vec<String>> {
    expand_refs(word, bindings, at, MAX_DEPTH)
}

fn expand_refs(word: &str, bindings: &Bindings, at: usize, depth: usize) -> Option<Vec<String>> {
    let chars: Vec<char> = word.chars().collect();
    let mut alternatives = vec![String::new()];
    let mut i = 0;
    while i < chars.len() {
        let bound = match reference(&chars, i).filter(|_| depth > 0) {
            Some((name, end)) => {
                values_at(&chars[i..end], &name, bindings, at, depth)?.map(|values| (values, end))
            }
            None => None,
        };
        let Some((values, end)) = bound else {
            alternatives.iter_mut().for_each(|a| a.push(chars[i]));
            i += 1;
            continue;
        };
        alternatives = alternatives
            .iter()
            .flat_map(|a| values.iter().map(move |v| format!("{a}{v}")))
            .collect();
        if alternatives.len() > MAX_ALTERNATIVES {
            return None;
        }
        i = end;
    }
    Some(alternatives)
}

/// What the reference *text* to *name* may hold at command *at*: `Some(None)`
/// when the line does not bind it, `None` past [`MAX_ALTERNATIVES`]. The
/// values are substituted in turn, as read where they were set.
fn values_at(
    text: &[char],
    name: &str,
    bindings: &Bindings,
    at: usize,
    depth: usize,
) -> Option<Option<Vec<String>>> {
    let in_force = |b: &Binding| b.body_until.is_some_and(|until| b.at < at && at < until);
    // Every value the line may give it, a loop's words outside the loop too:
    // after `done` the variable keeps the last word.
    let known: Vec<&Binding> = bindings.iter().filter(|b| b.name == name).collect();
    if known.is_empty() {
        return Some(None);
    }
    let mut values = Vec::new();
    for b in &known {
        values.extend(expand_refs(&b.value, bindings, b.at, depth - 1)?);
        if values.len() > MAX_ALTERNATIVES {
            return None;
        }
    }
    if !known.iter().any(|b| in_force(b)) {
        values.push(text.iter().collect());
    }
    Some(Some(values))
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
    fn an_assignment_adds_a_value_and_a_loop_variable_replaces_it() {
        // 0 `D=…`  1 `export E=…`  2 `for w in a b`  3 `do echo $w`  4 `done`
        let b = bindings(&shell_commands(
            "D=~/.claude; export E=/tmp; for w in a b; do echo $w; done",
        ));
        assert_eq!(substitute("$D/x", &b, 3).unwrap(), ["~/.claude/x", "$D/x"]);
        assert_eq!(
            substitute("${E}/$w", &b, 3).unwrap(),
            ["/tmp/a", "/tmp/b", "${E}/a", "${E}/b"],
            "inside the body the loop variable holds only its words"
        );
        assert_eq!(
            substitute("$w", &b, 5).unwrap(),
            ["a", "b", "$w"],
            "after the loop it may not be in force"
        );
        assert_eq!(
            substitute("$UNSET/${E:-x}", &b, 3).unwrap(),
            ["$UNSET/${E:-x}"]
        );
    }

    #[test]
    fn a_value_is_substituted_where_it_was_set_to_a_bound() {
        // 0 `for d in a`  1 `do for f in $d/*`  2 `do echo $f`  3 `done`  4 `done`
        let b = bindings(&shell_commands(
            "for d in a; do for f in $d/*; do echo $f; done; done",
        ));
        assert_eq!(substitute("$f", &b, 2).unwrap(), ["a/*"]);
        let b = bindings(&shell_commands("P=$P:x"));
        let p = substitute("$P", &b, 1).unwrap();
        assert_eq!(p.len(), 4, "bounded: {p:?}");
        assert!(p.contains(&"$P".to_string()));
        let many = format!("for v in {}; do echo $v/$v; done", "x ".repeat(20));
        let b = bindings(&shell_commands(&many));
        assert!(
            substitute("$v/$v", &b, 1).is_none(),
            "past the bound refuses"
        );
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

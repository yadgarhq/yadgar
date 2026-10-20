//! What the shell may put in place of a `$`-expansion or a backtick span.
//!
//! Split out of `tamper.rs` at its size ceiling. Two steps, in this order:
//!
//! 1. [`Scope::substitute`]: each plain `$NAME` / `${NAME}` is replaced by
//!    every value it may hold where it is read: the values the line gives it
//!    (see [`Scope::of`]), and, unless one of those is PROVEN in force there,
//!    its value in the agent's shell ([`Environment`]). A value nobody can
//!    know becomes [`EXPANSION`].
//! 2. [`collapse`]: every expansion left (`${…:-…}`, `$1`, `$(…)`, `$'…'`, a
//!    backtick pair) becomes one [`EXPANSION`], BEFORE the word is cut at `/`.
//!    A span holds `/` of its own: `` `adr/` `` in a Markdown heredoc was cut
//!    in two and read as `*/*`.
//!
//! [`EXPANSION`] may be ANY text, `/` and `.claude` included, so a component
//! holding one can match `.claude` exactly as a `*` can.

use std::collections::HashSet;

use super::env::Environment;
use super::{CLOSE_BRACE, MAX_ALTERNATIVES, OPEN_BRACE};
use crate::install::shellword::SimpleCommand;

/// One expansion whose text is unknown. NUL cannot occur in a shell word.
pub(super) const EXPANSION: char = '\0';

/// How deep a value is itself substituted into: `T=$S/x` and
/// `for f in $d/*` set a variable from another one.
const MAX_DEPTH: usize = 3; // ADR-0569-EXCEPTION(CB): bound on expanding untrusted input, not a per-install knob.

/// One value the line may give a variable.
struct Binding {
    name: String,
    value: String,
    /// The index of the command that sets it.
    at: usize,
    kind: Kind,
}

enum Kind {
    /// `NAME=value`. `definite` when it is certain to have run, in the
    /// line's own shell, before any later command.
    Assign { definite: bool },
    /// A `for NAME in …` word. `until` is the index of the loop's `done`:
    /// between the two, the variable holds one of the loop's words. `None`
    /// for a loop with no `done` (`for x in a; { … }`), which proves nothing.
    Loop { until: Option<usize> },
}

/// What the line tells the substitution of one of its words.
pub(super) struct Scope<'a> {
    bindings: Vec<Binding>,
    /// Names the line writes some other way (`read`, `printf -v`, `+=`, …):
    /// their value is unknown wherever they are read.
    tainted: HashSet<String>,
    /// `eval`, `source` or `.` on the line: any variable may be anything.
    all_tainted: bool,
    /// The line runs `cd`, `pushd` or `popd`, so `$PWD` is not the payload's.
    cd: bool,
    env: &'a Environment,
}

impl<'a> Scope<'a> {
    /// The bindings and writes of *commands*, each with the operators before
    /// it, resolving the rest against *env*.
    pub(super) fn of(
        commands: &[(SimpleCommand, Vec<String>)],
        line: &str,
        env: &'a Environment,
    ) -> Self {
        let mut scope = Scope {
            bindings: Vec::new(),
            tainted: HashSet::new(),
            all_tainted: false,
            cd: false,
            env,
        };
        // NO ASSIGNMENT IS PROVEN on a line holding a heredoc or a `#`: the
        // scanner reads heredoc bodies and comments as commands.
        let unprovable = has_heredoc(line)
            || commands
                .iter()
                .any(|(c, _)| c.words.iter().any(|w| w.starts_with('#')));
        let mut parens = 0i32;
        let mut nesting = 0i32;
        for (at, (command, before)) in commands.iter().enumerate() {
            parens += before
                .iter()
                .map(|op| i32::from(op == "(") - i32::from(op == ")"))
                .sum::<i32>();
            let words = after_keywords(&command.words);
            let first = words.first().map(String::as_str).unwrap_or("");
            scope.note_writes(words);
            if first == "for" && words.get(2).is_some_and(|w| w == "in") {
                if let Some(name) = words.get(1).filter(|n| is_name(n)) {
                    let until = matching_done(commands, at);
                    for value in &words[3..] {
                        scope.push(name, value, at, Kind::Loop { until });
                    }
                }
            } else {
                let after = commands
                    .get(at + 1)
                    .and_then(|(_, b)| b.first())
                    .map(String::as_str);
                let unconditional = !unprovable
                    && parens == 0
                    && nesting == 0
                    && before.last().is_none_or(|op| op == ";" || op == "\n")
                    && !matches!(after, Some("|" | "&"));
                scope.note_assignments(&command.words, at, unconditional);
            }
            nesting += i32::from(matches!(
                first,
                "for" | "while" | "until" | "if" | "case" | "select" | "{"
            )) - i32::from(matches!(first, "done" | "fi" | "esac" | "}"));
        }
        scope
    }

    fn push(&mut self, name: &str, value: &str, at: usize, kind: Kind) {
        self.bindings.push(Binding {
            name: name.to_string(),
            value: value.to_string(),
            at,
            kind,
        });
    }

    /// Record the `NAME=value` words of one command.
    ///
    /// Leading words, and the operands of `export` / `readonly` / `declare` /
    /// `typeset` / `local`, are assignments; a value there holding whitespace
    /// is split into its words, as an unquoted expansion of it would be. Any
    /// other `NAME=value` word may reach a shell too (`env D=… sh -c …`), so
    /// it adds its value as well, unless it holds whitespace: then it is a
    /// quoted argument, such as a whole `bash -c 'S=…; …'` script.
    fn note_assignments(&mut self, words: &[String], at: usize, unconditional: bool) {
        let declares = words.first().is_some_and(|w| {
            matches!(
                w.as_str(),
                "export" | "readonly" | "declare" | "typeset" | "local"
            )
        });
        let leading = words.iter().take_while(|w| assignment(w).is_some()).count();
        let pure = leading == words.len()
            || (declares && words[1..].iter().all(|w| assignment(w).is_some()));
        for (i, word) in words.iter().enumerate() {
            let Some((name, value)) = assignment(word) else {
                continue;
            };
            let in_place = i < leading || declares;
            if !in_place && value.contains(char::is_whitespace) {
                continue;
            }
            let definite = unconditional && pure;
            for piece in pieces(value) {
                self.push(name, piece, at, Kind::Assign { definite });
            }
        }
    }

    /// Record a command that writes a variable other than by `=`.
    fn note_writes(&mut self, words: &[String]) {
        let first = words.first().map(String::as_str).unwrap_or("");
        if matches!(first, "eval" | "source" | ".") {
            self.all_tainted = true;
        }
        if matches!(first, "cd" | "pushd" | "popd") {
            self.cd = true;
        }
        let writer = matches!(
            first,
            "read" | "mapfile" | "readarray" | "getopts" | "unset" | "printf" | "readonly"
        ) || (matches!(first, "declare" | "typeset" | "local" | "export")
            && words.iter().any(|w| w.starts_with('-')));
        for word in words {
            if let Some((name, _)) = word.split_once("+=").filter(|(n, _)| is_name(n)) {
                self.tainted.insert(name.to_string());
            }
            if writer {
                let name = word.split('=').next().unwrap_or("");
                if is_name(name) {
                    self.tainted.insert(name.to_string());
                }
            }
        }
    }

    /// *word*, read by the command at index *at*, with each `$NAME` /
    /// `${NAME}` replaced by each value it may hold there, or `None` past
    /// [`MAX_ALTERNATIVES`].
    pub(super) fn substitute(&self, word: &str, at: usize) -> Option<Vec<String>> {
        self.expand_refs(word, at, MAX_DEPTH)
    }

    fn expand_refs(&self, word: &str, at: usize, depth: usize) -> Option<Vec<String>> {
        let chars: Vec<char> = word.chars().collect();
        let mut alternatives = vec![String::new()];
        let mut i = 0;
        while i < chars.len() {
            let Some((name, end)) = reference(&chars, i).filter(|_| depth > 0) else {
                alternatives.iter_mut().for_each(|a| a.push(chars[i]));
                i += 1;
                continue;
            };
            let values = self.values_at(&name, at, depth)?;
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

    /// Every value *name* may hold at command *at*, each substituted in turn
    /// as read where it was set, or `None` past [`MAX_ALTERNATIVES`].
    fn values_at(&self, name: &str, at: usize, depth: usize) -> Option<Vec<String>> {
        let tainted = self.all_tainted || self.tainted.contains(name);
        let mine: Vec<&Binding> = self.bindings.iter().filter(|b| b.name == name).collect();
        // Inside the body of a loop over it, the variable holds one of the
        // loop's words, or a value assigned to it in the body before *at*.
        let in_loop = mine
            .iter()
            .find(|b| matches!(b.kind, Kind::Loop { until: Some(u) } if b.at < at && at < u));
        let known: Vec<&Binding> = match in_loop {
            Some(lp) if !tainted => mine
                .iter()
                .copied()
                .filter(|b| match b.kind {
                    Kind::Loop { until } => b.at == lp.at && until.is_some(),
                    Kind::Assign { .. } => lp.at < b.at && b.at < at,
                })
                .collect(),
            _ => mine.clone(),
        };
        let proven = !tainted
            && (in_loop.is_some()
                || mine
                    .iter()
                    .any(|b| matches!(b.kind, Kind::Assign { definite: true }) && b.at < at));
        let mut values = Vec::new();
        for b in &known {
            values.extend(self.expand_refs(&b.value, b.at, depth - 1)?);
            if values.len() > MAX_ALTERNATIVES {
                return None;
            }
        }
        if tainted {
            values.push(EXPANSION.to_string());
        } else if !proven {
            match self.env.value(name, self.cd) {
                Some(value) => values.extend(pieces(&value).map(str::to_string)),
                None => values.push(EXPANSION.to_string()),
            }
        }
        Some(values)
    }
}

/// *value* split at whitespace as an unquoted expansion splits it; an empty
/// value is one empty piece.
fn pieces(value: &str) -> impl Iterator<Item = &str> {
    let mut words: Vec<&str> = value.split_whitespace().collect();
    if words.is_empty() {
        words.push("");
    }
    words.into_iter()
}

/// The NAME and value of a `NAME=value` word.
fn assignment(word: &str) -> Option<(&str, &str)> {
    word.split_once('=').filter(|(n, _)| is_name(n))
}

/// Whether *line* holds a heredoc operator: `<<`, but not `<<<`.
fn has_heredoc(line: &str) -> bool {
    line.match_indices("<<")
        .any(|(i, _)| !line[i + 2..].starts_with('<') && (i == 0 || !line[..i].ends_with('<')))
}

/// *words* after the reserved words that may open a command: `do for f in …`
/// after a `;` is still a `for`.
fn after_keywords(words: &[String]) -> &[String] {
    let skip = words
        .iter()
        .take_while(|w| matches!(w.as_str(), "do" | "then" | "else" | "!" | "time"))
        .count();
    &words[skip..]
}

/// The index of the `done` that closes the loop opened at *open*, if any.
fn matching_done(commands: &[(SimpleCommand, Vec<String>)], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, (command, _)) in commands.iter().enumerate().skip(open) {
        match after_keywords(&command.words).first().map(String::as_str) {
            Some("for" | "while" | "until" | "select") => depth += 1,
            Some("done") => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::shellword::shell_commands_with_separators as scan;

    fn subst(line: &str, env: &Environment, word: &str, at: usize) -> Option<Vec<String>> {
        Scope::of(&scan(line), line, env).substitute(word, at)
    }

    #[test]
    fn a_proven_assignment_replaces_and_any_other_adds() {
        let env = Environment::unknown();
        assert_eq!(
            subst("D=/tmp; rm $D/x", &env, "$D/x", 1).unwrap(),
            ["/tmp/x"]
        );
        for line in [
            "D=/tmp true; rm $D",
            "false && D=/tmp; rm $D",
            "(D=/tmp); rm $D",
            "D=/tmp | cat; rm $D",
            "cat <<'EOF'\nD=/tmp\nEOF\nrm $D",
        ] {
            let at = scan(line).len() - 1;
            assert!(
                subst(line, &env, "$D/x", at)
                    .unwrap()
                    .contains(&"\0/x".to_string()),
                "{line}: not proven, so the environment's value stays"
            );
        }
    }

    #[test]
    fn a_loop_variable_is_proven_only_inside_a_closed_loop_it_alone_writes() {
        let env = Environment::unknown();
        // 0 `for w in a b`  1 `do echo $w`  2 `done`  3 `echo $w`
        let line = "for w in a b; do echo $w; done; echo $w";
        assert_eq!(subst(line, &env, "$w", 1).unwrap(), ["a", "b"]);
        assert_eq!(subst(line, &env, "$w", 3).unwrap(), ["a", "b", "\0"]);
        let line = "for w in a; do read w; echo $w; done";
        assert!(subst(line, &env, "$w", 2)
            .unwrap()
            .contains(&"\0".to_string()));
        let line = "for w in a; { :; }; echo $w";
        assert!(subst(line, &env, "$w", 2)
            .unwrap()
            .contains(&"\0".to_string()));
    }

    #[test]
    fn a_variable_the_line_does_not_set_resolves_against_the_environment() {
        let env = Environment::of(&[("SRC", "/tmp/a b")], Some("/w"));
        assert_eq!(
            subst("cp $SRC/x y", &env, "$SRC/x", 0).unwrap(),
            ["/tmp/a/x", "b/x"]
        );
        assert_eq!(subst("rm $NOPE/x", &env, "$NOPE/x", 0).unwrap(), ["\0/x"]);
        assert_eq!(subst("ls $PWD/x", &env, "$PWD/x", 0).unwrap(), ["/w/x"]);
        assert_eq!(
            subst("cd /; ls $PWD/x", &env, "$PWD/x", 1).unwrap(),
            ["\0/x"]
        );
    }

    #[test]
    fn a_value_is_substituted_where_it_was_set_to_a_bound() {
        let env = Environment::unknown();
        let line = "for d in a; do for f in $d/*; do echo $f; done; done";
        assert_eq!(subst(line, &env, "$f", 2).unwrap(), ["a/*"]);
        let line = "P=$P:x; echo $P";
        assert!(subst(line, &env, "$P", 1).unwrap().len() > 1, "bounded");
        let many = format!("for v in {}; do echo $v/$v; done", "x ".repeat(20));
        assert!(
            subst(&many, &env, "$v/$v", 1).is_none(),
            "past the bound refuses"
        );
    }
}

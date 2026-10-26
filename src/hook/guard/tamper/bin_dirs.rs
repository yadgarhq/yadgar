//! A write into a PATH directory that comes before the real gh (yaadgaar
//! #240, ADR-1020).
//!
//! A line-scoped check cannot see a gh planted by an EARLIER call and found
//! first on PATH by a later one (the ADR-0971 residual). So the place it
//! would be planted is protected: the directories the hook's PATH lists
//! before the first one holding an executable gh, resolved when the hook
//! runs (`guard/env.rs`). On a machine where `~/.local/bin` precedes gh,
//! `cp x ~/.local/bin/tool` refuses; where it follows gh, it does not.
//!
//! * `Edit`, `Write`, `MultiEdit` and `NotebookEdit` on such a directory, or
//!   on a file directly inside it, refuse.
//! * A Bash word that names one, after `~` and `$HOME` are expanded,
//!   refuses outside a reading command and outside program position
//!   (running `~/.local/bin/tool` writes nothing). The value of an
//!   assignment or `--opt=` word counts as a word, so `D=~/.local/bin`
//!   refuses before `$D` is ever used.
//!   A redirect onto one refuses whatever the command is. A `$NAME` the line
//!   does not assign, or a glob, stands for one component, as in the
//!   exceptions check: `~/.local/bin/$f` refuses (shape (c) of ADR-1005).
//!   A home spelled another way (`${HOME%/}`, `${HOME:-…}`, `~user`) or a
//!   leading `$(…)` or backtick span may be ANY prefix: the rest of the word
//!   is matched against the end of each protected directory.
//!   An unquoted `$(…)` is followed past the scanner's split (see
//!   [`Substitutions`]): what comes right after its `)` belongs to the
//!   command it is an argument of. A substitution whose output a writing
//!   command gets, holding a word that may be a protected directory,
//!   refuses: `cp a $(echo ~/.local/bin)/tool`.
//! * A hook PATH that is unset, empty or holds no absolute directory FAILS
//!   CLOSED: every absolute path is protected.
//!
//! **THE INSTALLER ALLOWLIST IS EMPTY**, and needs nothing: `pipx install
//! yaadgaar` names no path on its line, so nothing here sees it.
//!
//! **WHAT THIS DOES NOT SEE**: `cd ~/.local/bin && cp x tool`, a directory
//! named by a variable set outside the line, and a PATH that differs
//! between the hook process and the Bash tool's shell.

use super::protected::{normalized, reader_arguments};
use super::{as_glob, brace_expand, closing, glob_matches, CLOSE_BRACE, OPEN_BRACE};
use crate::hook::guard::runs::{executed, SPLIT_OFF};
use crate::hook::guard::GuardEnv;
use crate::install::shellword::{shell_split, SimpleCommand};

mod shipped;

/// The protected directories, as normalized components.
fn protected(env: &GuardEnv) -> Vec<Vec<String>> {
    env.before_gh
        .iter()
        .map(|d| normalized(&d.to_string_lossy()))
        .filter(|d| !d.is_empty())
        .collect()
}

/// Whether exact *components* are a protected directory or a file directly
/// inside one.
fn in_a_dir(components: &[String], dirs: &[Vec<String>]) -> bool {
    dirs.iter().any(|d| {
        (components.len() == d.len() || components.len() == d.len() + 1)
            && components.iter().zip(d).all(|(c, d)| c == d)
    })
}

/// Whether an Edit/Write/MultiEdit/NotebookEdit *path* is in a protected
/// directory.
pub(in crate::hook::guard) fn is_before_gh(path: &str, env: &GuardEnv) -> bool {
    let dirs = protected(env);
    path.split('\n')
        .any(|p| (env.path_unknown && absolute(p)) || in_a_dir(&normalized(p), &dirs))
}

/// Whether *path* is absolute, `/x` or `C:\x`.
fn absolute(path: &str) -> bool {
    path.starts_with(['/', '\\']) || path.get(1..3).is_some_and(|d| d == ":\\" || d == ":/")
}

/// Whether a Bash *line*, scanned as *commands* each with the separators
/// before it (*runs*), writes into a protected directory.
///
/// TWO READINGS, and either refuses. [`Substitutions`] follows each
/// unquoted `$(…)` to its closing paren. The reading shipped on main stays
/// as a floor (`bin_dirs/shipped.rs`), so a paren the count reads wrong (a
/// `case` arm, a `#` comment) can only cost a refusal it adds, never one
/// main made (ADR-0645).
pub(in crate::hook::guard) fn writes_before_gh(
    commands: &[SimpleCommand],
    runs: &[Vec<String>],
    env: &GuardEnv,
) -> bool {
    let dirs = protected(env);
    if dirs.is_empty() && !env.path_unknown {
        return false;
    }
    let to = Target {
        home: env.home.as_ref().map(|h| h.to_string_lossy().into_owned()),
        dirs,
        any: env.path_unknown,
    };
    let balanced = parens_balance(runs);
    let mut open = Substitutions::default();
    let mut floor = shipped::Shipped::default();
    commands.iter().zip(runs).any(|(c, run)| {
        let carried = open.enter(run);
        let read = carried.as_ref().map(|s| s.reads_with(&c.words));
        // The ONE answer the floor gives up: a lone `}`, on a line whose
        // parens all balance. One that continues a substitution, as in
        // `cp a $(x)}`, is still refused by `judged`.
        let hit = judged(c, read, open.in_a_write(), &to) | floor.judged(c, &to, balanced);
        open.leave(c, carried);
        hit
    })
}

/// Whether every `)` in *runs* closes a `(` before it, and none is left
/// open.
fn parens_balance(runs: &[Vec<String>]) -> bool {
    let mut depth = 0usize;
    for c in runs
        .iter()
        .filter(|r| !(r.len() == 1 && r[0] == SPLIT_OFF))
        .flatten()
        .flat_map(|op| op.chars())
    {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return false,
            ')' => depth -= 1,
            _ => {}
        }
    }
    depth == 0
}

/// The command a substitution's output is an argument of.
#[derive(Clone)]
struct Opened {
    /// Its words up to the one that ends at `$`.
    argv: Vec<String>,
    /// The substitution is a redirect target.
    write: bool,
}

impl Opened {
    /// Whether the words after the substitution, *words*, are read rather
    /// than written. Decided on the WHOLE argument list, *words*
    /// included: `git log $(x)--output=f` writes `f`.
    fn reads_with(&self, words: &[String]) -> bool {
        !self.write && reads(&self.argv) && reads(&merged(&self.argv, words))
    }
}

/// Whether a command of *words* is a reading command.
fn reads(words: &[String]) -> bool {
    let words = words.to_vec();
    reader_arguments(&SimpleCommand {
        words,
        writes_to: Vec::new(),
    })
    .is_some()
}

/// *argv*, whose last word ends at `$`, followed by *words*: the first one
/// both glued to that word and on its own, since the scanner cannot tell.
fn merged(argv: &[String], words: &[String]) -> Vec<String> {
    let mut out = argv.to_vec();
    if let (Some(last), Some(first)) = (out.last_mut(), words.first()) {
        last.push_str("()");
        last.push_str(first);
    }
    out.extend(words.iter().cloned());
    out
}

/// The unquoted `$(…)` substitutions open at each command.
///
/// The scanner ends a command at the `$` of `$(`, scans the substitution's
/// commands, and starts another after the `)`. That one's first word is the
/// rest of the argument the substitution's output starts, and belongs to
/// the command that ended at `$`, ONLY when the separators before it are
/// the `)` that closes the substitution: `$(x)/b` or `$(x) b`, never
/// `$(x); b`. The parens are counted one by one, because the scanner reads
/// `))` as one separator.
#[derive(Default)]
struct Substitutions {
    /// One entry per open paren: `Some` when it opened a substitution.
    open: Vec<Option<Opened>>,
    /// The last command ended at `$`: what a `(` right after it opens.
    opener: Option<Opened>,
    /// What the last command continued, for one split off after it.
    last: Option<Opened>,
}

impl Substitutions {
    /// Read *run*, the separators before a command: `Some` when the
    /// command continues the argument a substitution's output starts.
    fn enter(&mut self, run: &[String]) -> Option<Opened> {
        if run.len() == 1 && run[0] == SPLIT_OFF {
            return self.last.clone(); // Split off at a brace: it runs on as before.
        }
        let mut opener = self.opener.take();
        let mut carried = None;
        for (n, c) in run.iter().flat_map(|op| op.chars()).enumerate() {
            carried = match c {
                '(' => {
                    let opened = if n == 0 { opener.take() } else { None };
                    self.open.push(opened);
                    None
                }
                ')' => self.open.pop().flatten(),
                _ => None,
            };
        }
        carried
    }

    /// Whether a command runs inside a substitution whose output a command
    /// that is not a reader gets.
    fn in_a_write(&self) -> bool {
        self.open
            .iter()
            .flatten()
            .any(|o| o.write || !reads(&o.argv))
    }

    /// Note whether *c*, which continued *carried*, ended at `$`: in a
    /// redirect target that is a write; in a word, an argument of the
    /// command it is part of.
    fn leave(&mut self, c: &SimpleCommand, carried: Option<Opened>) {
        let into = c.writes_to.last().is_some_and(|w| w.ends_with('$'));
        let argument = c.words.last().is_some_and(|w| w.ends_with('$'));
        self.opener = if into {
            Some(Opened {
                argv: Vec::new(),
                write: true,
            })
        } else if argument {
            Some(match &carried {
                Some(o) => Opened {
                    argv: merged(&o.argv, &c.words),
                    write: o.write,
                },
                None => Opened {
                    argv: c.words.clone(),
                    write: false,
                },
            })
        } else {
            None
        };
        self.last = carried;
    }
}

/// Whether command *c* writes into a protected directory.
///
/// *carried*: it continues the argument a substitution's output starts, so
/// none of its words is a program, and `Some(true)` when the command it
/// belongs to reads them; its first word may be glued to the output or
/// spaced from it, and the scanner cannot tell, so both readings are
/// checked. *in_a_write*: what it prints may be a write's target, so no
/// argument of it is a reader's.
fn judged(c: &SimpleCommand, carried: Option<bool>, in_a_write: bool, to: &Target) -> bool {
    let checked = |word: &str| {
        let value = word.split_once('=').map(|(_, v)| v);
        std::iter::once(word).chain(value).any(|w| to.may_hold(w))
    };
    if c.writes_to.iter().any(|w| checked(w)) {
        return true;
    }
    let glued = match carried {
        Some(true) if !in_a_write => return false,
        Some(_) => c
            .words
            .first()
            .is_some_and(|w| to.may_hold(&format!("$(){w}"))),
        None => false,
    };
    let program = match carried {
        Some(_) => None,
        None => executed(c).map(|(_, args)| c.words.len() - args.len() - 1),
    };
    let reader = match carried.is_some() || in_a_write {
        true => None,
        false => reader_arguments(c),
    };
    glued
        || c.words.iter().enumerate().any(|(n, w)| {
            program != Some(n) && !reader.is_some_and(|start| n >= start) && checked(w)
        })
}

/// What a word is checked against.
struct Target {
    home: Option<String>,
    dirs: Vec<Vec<String>>,
    /// The PATH is unknown: every absolute directory is protected.
    any: bool,
}

impl Target {
    fn may_hold(&self, word: &str) -> bool {
        let alt = home_expanded(word, self.home.as_deref());
        if self.any && (absolute(&alt) || any_prefix(&alt)) {
            return true;
        }
        if any_prefix(&alt) {
            return self.suffix_may_hold(&alt) || self.output_may_hold(&alt);
        }
        may_be_in_a_dir(&alt, &self.dirs)
    }

    /// Whether *word*'s leading `$(…)` or backtick span holds an argument
    /// that may be a protected directory, so that its output may be one:
    /// `"$(echo /run/wrappers/bin)"/tool`.
    fn output_may_hold(&self, word: &str) -> bool {
        let end = prefix_end(word).saturating_sub(1);
        let inner = if word.starts_with("$(") {
            word.get(2..end)
        } else if word.starts_with('`') {
            word.get(1..end)
        } else {
            None
        };
        inner.is_some_and(|i| shell_split(i).iter().skip(1).any(|w| self.may_hold(w)))
    }

    /// *word*'s first component may be any absolute prefix: a home spelled
    /// another way, or a substitution. The rest must match the END of a
    /// protected directory, with or without one file below it.
    fn suffix_may_hold(&self, word: &str) -> bool {
        let rest: Vec<String> = word[prefix_end(word)..]
            .split(['/', '\\'])
            .map(as_glob)
            .filter(|p| !p.is_empty() && p != ".")
            .collect();
        let Some(rest) = brace_free(rest) else {
            return true; // A brace list or `..` after the prefix: refuse rather than guess.
        };
        self.dirs.iter().any(|d| {
            [0, 1].iter().any(|&leaf| {
                let m = rest.len().saturating_sub(leaf);
                m >= 1
                    && m <= d.len()
                    && rest.len() == m + leaf
                    && rest[..m]
                        .iter()
                        .zip(&d[d.len() - m..])
                        .all(|(p, d)| glob_matches(p, d))
            })
        })
    }
}

/// *parts*, or `None` when one holds a brace or is `..`.
fn brace_free(parts: Vec<String>) -> Option<Vec<String>> {
    let clean = parts
        .iter()
        .all(|p| p != ".." && !p.contains(['\u{7b}', '\u{7d}']));
    clean.then_some(parts)
}

/// Whether *word* starts with a component that may expand to any absolute
/// prefix: `${HOME…}` with an operator, `~user`, a `$(…)` or a backtick.
fn any_prefix(word: &str) -> bool {
    word.starts_with("$\u{7b}HOME")
        || word.starts_with("$(")
        || word.starts_with('`')
        || (word.starts_with('~') && !word.starts_with("~/") && word.len() > 1)
}

/// The byte index where *word*'s leading expansion ends: past the span
/// that closes `${…}`, `$(…)` or a backtick pair, which may hold `/`; or,
/// for `~user`, at the first separator.
fn prefix_end(word: &str) -> usize {
    let chars: Vec<char> = word.chars().collect();
    let end = match chars.first() {
        Some('$') if chars.get(1) == Some(&OPEN_BRACE) => {
            closing(&chars, 1, OPEN_BRACE, CLOSE_BRACE)
        }
        Some('$') if chars.get(1) == Some(&'(') => closing(&chars, 1, '(', ')'),
        Some('`') => chars[1..]
            .iter()
            .position(|&c| c == '`')
            .map_or(chars.len(), |n| n + 2),
        _ => chars
            .iter()
            .position(|&c| c == '/' || c == '\\')
            .unwrap_or(chars.len()),
    };
    chars[..end].iter().map(|c| c.len_utf8()).sum()
}

/// *word* with a leading `~`, `$HOME` or `${HOME}` replaced by *home*.
fn home_expanded(word: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return word.to_string();
    };
    for prefix in ["~", "$HOME", "${HOME}"] {
        if let Some(rest) = word.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with(['/', '\\']) {
                return format!("{home}{rest}");
            }
        }
    }
    word.to_string()
}

/// Whether *alt*, `~` and `$HOME` already expanded, may be in a protected
/// directory.
fn may_be_in_a_dir(alt: &str, dirs: &[Vec<String>]) -> bool {
    // A relative word is not resolved against a working directory the
    // hook does not share.
    if !absolute(alt) {
        return false;
    }
    if !alt.contains(['$', '*', '?', '[', '\u{7b}', '`']) {
        return in_a_dir(&normalized(alt), dirs);
    }
    let Some(expanded) = brace_expand(alt) else {
        return true; // Too many alternatives to check: refuse rather than guess.
    };
    expanded.iter().any(|e| globs_in_a_dir(e, dirs))
}

/// [`in_a_dir`] for a word whose components are glob patterns.
fn globs_in_a_dir(word: &str, dirs: &[Vec<String>]) -> bool {
    let mut parts: Vec<String> = Vec::new();
    for part in word.split(['/', '\\']).map(as_glob) {
        match part.as_str() {
            "" | "." => {}
            ".." => match parts.pop() {
                // `$X/..` climbs out of a directory that may be anything.
                Some(last) if last.contains(['*', '?', '[']) => return true,
                _ => {}
            },
            _ => parts.push(part),
        }
    }
    dirs.iter().any(|d| {
        (parts.len() == d.len() || parts.len() == d.len() + 1)
            && parts.iter().zip(d).all(|(p, d)| glob_matches(p, d))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Vec<Vec<String>> {
        vec![normalized("/home/x/.local/bin")]
    }

    /// Mutant bin_toomany: past the brace bound the word refuses.
    #[test]
    fn a_word_with_too_many_brace_alternatives_refuses() {
        let word = format!("/tmp/{}/x", "\u{7b}a,b\u{7d}".repeat(9));
        assert!(may_be_in_a_dir(&word, &dirs()));
        assert!(!may_be_in_a_dir("/tmp/\u{7b}a,b\u{7d}/x", &dirs()));
    }

    /// Mutant bin_dotdot: `$X/..` climbs out of a directory that may be
    /// anything, so the word refuses.
    #[test]
    fn climbing_out_of_an_expansion_refuses() {
        assert!(globs_in_a_dir("/tmp/*/../x", &dirs()));
        assert!(!globs_in_a_dir("/tmp/a/../x", &dirs()));
    }
}

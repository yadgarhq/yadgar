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
use crate::hook::guard::runs::executed;
use crate::hook::guard::GuardEnv;
use crate::install::shellword::SimpleCommand;

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

/// Whether a Bash *line*, scanned as *commands*, writes into a protected
/// directory.
pub(in crate::hook::guard) fn writes_before_gh(commands: &[SimpleCommand], env: &GuardEnv) -> bool {
    let dirs = protected(env);
    if dirs.is_empty() && !env.path_unknown {
        return false;
    }
    let to = Target {
        home: env.home.as_ref().map(|h| h.to_string_lossy().into_owned()),
        dirs,
        any: env.path_unknown,
    };
    // An unquoted `$(…)` ends the command at `$`; the next one's first word
    // is the rest of the same argument, starting with the substitution's
    // output, and belongs to the same program. As in `tamper.rs`, this
    // holds for every later command's first word: the substitution's own
    // commands come between.
    let mut continues: Option<bool> = None;
    commands.iter().any(|c| {
        let program = executed(c).map(|(_, args)| c.words.len() - args.len() - 1);
        let reader = reader_arguments(c);
        let carried = continues;
        let hit = c
            .words
            .iter()
            .chain(&c.writes_to)
            .enumerate()
            .any(|(n, word)| {
                let is_word = n < c.words.len();
                if n == 0 && is_word {
                    if let Some(read) = carried {
                        return !read && to.may_hold(&format!("$(){word}"));
                    }
                }
                if is_word && (program == Some(n) || reader.is_some_and(|start| n >= start)) {
                    return false;
                }
                let value = word.split_once('=').map(|(_, v)| v);
                std::iter::once(word.as_str())
                    .chain(value)
                    .any(|w| to.may_hold(w))
            });
        if c.words.last().is_some_and(|w| w.ends_with('$')) {
            continues = Some(carried.unwrap_or(false) || reader.is_some());
            // Sticky from here on.
        }
        hit
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
            return self.suffix_may_hold(&alt);
        }
        may_be_in_a_dir(&alt, &self.dirs)
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

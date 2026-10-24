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
//!
//! **THE INSTALLER ALLOWLIST IS EMPTY**, and needs nothing: `pipx install
//! yaadgaar` names no path on its line, so nothing here sees it.
//!
//! **WHAT THIS DOES NOT SEE**: `cd ~/.local/bin && cp x tool`, a directory
//! named by a variable set outside the line, and a PATH that differs
//! between the hook process and the Bash tool's shell.

use super::protected::{normalized, reader_arguments};
use super::{as_glob, brace_expand, glob_matches};
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
    !dirs.is_empty() && path.split('\n').any(|p| in_a_dir(&normalized(p), &dirs))
}

/// Whether a Bash *line*, scanned as *commands*, writes into a protected
/// directory.
pub(in crate::hook::guard) fn writes_before_gh(commands: &[SimpleCommand], env: &GuardEnv) -> bool {
    let dirs = protected(env);
    if dirs.is_empty() {
        return false;
    }
    let home = env.home.as_ref().map(|h| h.to_string_lossy().into_owned());
    commands.iter().any(|c| {
        let program = executed(c).map(|(_, args)| c.words.len() - args.len() - 1);
        let reader = reader_arguments(c);
        c.words
            .iter()
            .chain(&c.writes_to)
            .enumerate()
            .any(|(n, word)| {
                let is_word = n < c.words.len();
                if is_word && (program == Some(n) || reader.is_some_and(|start| n >= start)) {
                    return false;
                }
                let value = word.split_once('=').map(|(_, v)| v);
                std::iter::once(word.as_str())
                    .chain(value)
                    .any(|w| may_be_in_a_dir(w, home.as_deref(), &dirs))
            })
    })
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

/// Whether *word*, `~` and `$HOME` expanded, may be in a protected
/// directory.
fn may_be_in_a_dir(word: &str, home: Option<&str>, dirs: &[Vec<String>]) -> bool {
    let alt = home_expanded(word, home);
    // A relative word is not resolved against a working directory the
    // hook does not share.
    if !alt.starts_with(['/', '\\']) && !alt.get(1..3).is_some_and(|d| d == ":\\" || d == ":/") {
        return false;
    }
    if !alt.contains(['$', '*', '?', '[', '\u{7b}', '`']) {
        return in_a_dir(&normalized(&alt), dirs);
    }
    let Some(expanded) = brace_expand(&alt) else {
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

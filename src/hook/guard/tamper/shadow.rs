//! A line that shadows gh (yaadgaar #240, ADR-1020).
//!
//! The exceptions-file check lets one shape through because `gh` is the
//! real gh (`gh_api.rs`, ADR-0971); `gh-personal` is trusted the same way.
//! A line that makes either name run something else turns that allowance
//! into a write. So a line refuses when it:
//!
//! * defines a function named gh (`gh() …`, `function gh`) or an alias
//!   (`alias gh=…`, zsh `aliases[gh]=…`, `functions[gh]=…`, `commands[gh]=…`);
//! * assigns PATH in any spelling bash or zsh reads: `PATH=`, `PATH+=`, zsh
//!   `path=(…)` and `path[1]=`, through `export`/`declare`/`env`, or via
//!   `read PATH` and `printf -v PATH`;
//! * runs `hash`, `enable`, `source`, `.` or `eval` with an argument that
//!   names gh, or `command -p` running gh;
//! * writes a file named gh: a redirect onto one, or a path argument whose
//!   last component is gh outside a reading command, or a bare `gh` given to
//!   a command on [`WRITERS`].
//!
//! The program word itself (`gh pr view`, `/usr/bin/gh auth status`) and
//! the `gh api` endpoint (`repos/cli/gh`) are not writes and are skipped.
//!
//! **WHAT THIS DOES NOT SEE**: a name built at run time (`g""h` survives
//! quote removal and is caught; `$(printf g)h` is not), shell startup files
//! (`~/.zshrc`, `BASH_ENV`), a write that names only the directory
//! (`cp x ~/bin/`, unless that directory precedes gh on PATH: `bin_dirs.rs`),
//! and a known over-refusal: a path argument such as `gh repo view cli/gh`.

use super::gh_api;
use super::protected::reader_arguments;
use crate::hook::guard::runs::executed;
use crate::install::shellword::SimpleCommand;

/// The names the guard trusts as the real gh.
const NAMES: &[&str] = &["gh", "gh-personal"];

/// Commands that write a bare file name they are given.
const WRITERS: &[&str] = &[
    "cp", "mv", "ln", "install", "tee", "touch", "dd", "curl", "wget", "rsync", "scp", "chmod",
];

/// Whether *line*, scanned as *commands*, shadows gh.
pub(in crate::hook::guard) fn shadows_gh(line: &str, commands: &[SimpleCommand]) -> bool {
    defines_a_gh_function(line) || commands.iter().any(command_shadows_gh)
}

/// Whether *word*, quotes and backslashes removed and case folded, is one
/// of [`NAMES`].
fn is_name(word: &str) -> bool {
    let plain = plain(word);
    let plain = plain.strip_suffix("()").unwrap_or(&plain);
    NAMES.contains(&plain)
}

/// *text* with quotes and backslashes removed, ASCII case folded.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Whether *word* names gh among its tokens: `gh`, `gh.sh`, `gh=/x`.
fn touches_gh(word: &str) -> bool {
    plain(word)
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .any(|t| NAMES.contains(&t))
}

/// `name ()` anywhere in the line: the scanner drops the parentheses, so
/// the text is read. Quotes are removed first, so `"gh"()` counts.
fn defines_a_gh_function(line: &str) -> bool {
    let text = plain(line);
    let bytes = text.as_bytes();
    NAMES.iter().any(|name| {
        text.match_indices(name).any(|(at, _)| {
            let before = at.checked_sub(1).map(|i| bytes[i]);
            let opens = before.is_none_or(|b| b.is_ascii_whitespace() || b";&|({`!".contains(&b));
            let rest = text[at + name.len()..].trim_start();
            opens
                && rest
                    .strip_prefix('(')
                    .is_some_and(|r| r.trim_start().starts_with(')'))
        })
    })
}

/// The assignment's name in *word* (`NAME=`, `NAME+=`, `NAME[i]=`), with
/// any `[…]` subscript split off.
fn assigned(word: &str) -> Option<(&str, Option<&str>)> {
    if word.starts_with('-') {
        return None;
    }
    let (name, _) = word.split_once('=')?;
    let name = name.strip_suffix('+').unwrap_or(name);
    Some(match name.split_once('[') {
        Some((base, sub)) => (base, Some(sub.strip_suffix(']').unwrap_or(sub))),
        None => (name, None),
    })
}

/// Whether *word* assigns PATH, or a zsh table entry for gh.
fn assigns_a_shadow(word: &str) -> bool {
    match assigned(word) {
        Some(("PATH" | "path", _)) => true,
        Some(("functions" | "aliases" | "galiases" | "commands" | "builtins", Some(sub))) => {
            is_name(sub)
        }
        _ => false,
    }
}

fn command_shadows_gh(command: &SimpleCommand) -> bool {
    let words = &command.words;
    if words.first().is_some_and(|w| w == "function") && words[1..].iter().any(|w| is_name(w)) {
        return true;
    }
    let Some((program, args)) = executed(command) else {
        // Assignments only: `PATH=/x`, `path=(…)`.
        return words.iter().any(|w| assigns_a_shadow(w));
    };
    let at = command.words.len() - args.len() - 1;
    // Before the program: assignments, and wrapper words (`env PATH=…`).
    if command.words[..at].iter().any(|w| assigns_a_shadow(w)) {
        return true;
    }
    let runs_gh_by_default_path = command.words[..at]
        .windows(2)
        .any(|w| w[0] == "command" && w[1].starts_with('-') && w[1].contains('p'));
    if runs_gh_by_default_path && is_name(program) {
        return true;
    }
    let arguments_shadow = match program {
        "export" | "declare" | "typeset" | "local" | "readonly" | "integer" | "float" => {
            args.iter().any(|a| assigns_a_shadow(a))
        }
        "alias" => args
            .iter()
            .any(|a| a.split_once('=').is_some_and(|(n, _)| is_name(n))),
        "read" => args.iter().any(|a| a == "PATH" || a == "path"),
        "printf" => args.iter().enumerate().any(|(i, a)| {
            (a == "-v" && args.get(i + 1).is_some_and(|v| v == "PATH" || v == "path"))
                || a == "-vPATH"
        }),
        "hash" | "enable" | "source" | "." | "eval" => args.iter().any(|a| touches_gh(a)),
        _ => false,
    };
    arguments_shadow || writes_a_gh_file(command, program, at)
}

/// Whether *command*, whose program word is at *at*, writes a file named gh.
fn writes_a_gh_file(command: &SimpleCommand, program: &str, at: usize) -> bool {
    if command.writes_to.iter().any(|w| names_a_gh_file(w, true)) {
        return true;
    }
    let reader = reader_arguments(command);
    let endpoint = gh_api::endpoint(command);
    let bare = WRITERS.contains(&program);
    command.words.iter().enumerate().any(|(n, word)| {
        if n == at || endpoint == Some(n) || reader.is_some_and(|start| n >= start) {
            return false;
        }
        let value = word.split_once('=').map(|(_, v)| v);
        std::iter::once(word.as_str())
            .chain(value)
            .any(|w| names_a_gh_file(w, bare))
    })
}

/// Whether path *word* ends in a component named gh. A bare name counts
/// only when *bare*; a URL never does.
pub(in crate::hook::guard) fn names_a_gh_file(word: &str, bare: bool) -> bool {
    if word.contains("://") {
        return false;
    }
    let pathy = word.contains(['/', '\\']);
    let leaf = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let leaf = leaf.split(':').next().unwrap_or(leaf);
    let leaf = leaf.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    let leaf = leaf.strip_suffix(".exe").unwrap_or(&leaf);
    (pathy || bare) && NAMES.contains(&leaf)
}

//! The exceptions file in a Bash command: ANY MENTION REFUSES, read or write.
//!
//! Split out of `guard.rs` at its size ceiling.
//!
//! **NO READ IS LET THROUGH, and that is a decision, not an omission** (#64).
//! Ledger 1590 asked for `cp ~/.claude/<file> <elsewhere>` to be allowed as a
//! read. Every version of that allowance was a question about how the shell
//! would parse the line, and four security reviews found a new way to make
//! the guard's answer differ from the shell's: a symlinked directory, a `HOME`
//! reassigned earlier on the line, an unknown wrapper, a split redirection. A
//! false refusal of a read costs a retry with the Read tool; a false allowance
//! of a write costs the decision file the guard exists to protect. So any
//! command whose text names the file refuses. Read it with the Read tool.

use crate::hook::EXCEPTIONS_FILE;
use crate::install::shellword::SimpleCommand;

/// Whether *line* names the exceptions file, or may expand into it.
///
/// The text is taken with quotes and backslashes removed and case folded, so
/// `yadgar-hook-exception\s.json`, `'yadgar-hook-'exceptions.json` and a
/// case-insensitive filesystem's `YADGAR-…` all count; a trailing `.`, space
/// or `:stream` suffix still contains the name. Then each word and redirect
/// target is checked for a shell expansion that may produce it (see
/// [`may_be_the_exceptions_file`]).
pub(super) fn names_the_exceptions_file(line: &str, commands: &[SimpleCommand]) -> bool {
    line_names_the_file(line)
        || commands.iter().any(|c| {
            c.words
                .iter()
                .chain(&c.writes_to)
                .any(|w| may_be_the_exceptions_file(w))
        })
}

/// Whether the file's name appears in *line*, quotes and backslashes removed,
/// ignoring ASCII case.
fn line_names_the_file(line: &str) -> bool {
    let plain: String = line
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    plain.to_ascii_lowercase().contains(EXCEPTIONS_FILE)
}

/// Whether *word* names the file, or may expand into it.
///
/// A word with a shell expansion in it (`$`, a glob, a brace list, a
/// backtick) cannot be decided from the line. It counts when it sits under a
/// `.claude` directory, or its leaf mentions `yadgar`, `hook` or `exception`:
/// `~/.claude/$F`, `~/.claude/*.json`, `yadgar-hook-exception?.json`,
/// `*-hook-exceptions.json`.
fn may_be_the_exceptions_file(word: &str) -> bool {
    if is_the_exceptions_file(word) {
        return true;
    }
    if !word.contains(['$', '*', '?', '[', '{', '`']) {
        return false;
    }
    let mut parts = word.rsplit(['/', '\\']);
    let leaf = parts.next().unwrap_or(word).to_ascii_lowercase();
    ["yadgar", "hook", "exception"]
        .iter()
        .any(|fragment| leaf.contains(fragment))
        || parts.any(|p| p.eq_ignore_ascii_case(".claude"))
}

/// Whether any path component of *word* reaches the file: for the Edit,
/// Write and NotebookEdit paths, which are exact.
///
/// IGNORING ASCII CASE, because macOS and Windows filesystems do by default;
/// and on Windows, ignoring trailing dots and spaces and any `:stream` suffix
/// (`<file>::$DATA` writes the file's main stream).
pub(super) fn is_the_exceptions_file(word: &str) -> bool {
    word.split(['/', '\\', '\n']).any(|part| {
        let part = part.split(':').next().unwrap_or(part);
        part.trim_end_matches(['.', ' '])
            .eq_ignore_ascii_case(EXCEPTIONS_FILE)
    })
}

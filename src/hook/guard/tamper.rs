//! Writes to the exceptions file: which commands change it, and which only
//! read it.
//!
//! Split out of `guard.rs` at its size ceiling. The seam is the rule: the
//! `hook_config_tamper` refusal is the only one that asks "is this file a
//! destination?", and everything that answers it lives here.
//!
//! **WHEN IN DOUBT, IT REFUSES.** The one read this lets through is narrow
//! (ledger 1590): `cp` of the REAL file, by its canonical path, with plain
//! copy options, to a destination that is not the file. Everything else that
//! names the file in a writer's operands refuses, including what the shell
//! might expand into it.

use crate::hook::EXCEPTIONS_FILE;

/// Commands that write a file named on their command line.
///
/// A NAMED SET, and therefore an incomplete one — said plainly rather than
/// implied. Bash can reach a file by a hundred routes and this covers the ones
/// an agent actually uses; `Edit`, `Write` and `NotebookEdit` are exact, and
/// that is where the incident happened. `install` and `ln` are here rather
/// than parsed like `cp`: naming the file either way refuses.
const WRITERS: &[&str] = &[
    "tee", "sed", "mv", "rm", "truncate", "install", "ln", "rsync", "chmod", "chown", "shred",
    "patch", "sponge", "python", "python3", "perl", "ruby", "node", "jq",
    // Not writers of bytes, but of a FIFO or device node in the file's place:
    // a reader that blocked on one would hang until the hook timed out.
    "mkfifo", "mknod",
];

/// Commands that run ANOTHER command on arguments this line cannot see in
/// full: `xargs` appends stdin, `find -exec` substitutes paths. They refuse
/// whenever the file is named anywhere on the line.
pub(super) const INDIRECT: &[&str] = &["xargs", "find", "parallel"];

/// `cp` options a read of the real file may carry: they copy content and
/// nothing else. Any other option (`-s`, `-l`, `--remove-destination`, ...)
/// with the file as a source refuses.
const CP_PLAIN_SHORT: &[char] = &['p', 'v', 'i', 'n', 'T'];
const CP_PLAIN_LONG: &[&str] = &["--preserve", "--verbose", "--interactive", "--no-clobber"];

/// The `cp` options that take a value.
const CP_VALUE_SHORT: &[char] = &['t', 'S'];
const CP_VALUE_LONG: &[&str] = &["--target-directory", "--suffix"];

/// The spellings of the real file's path this recognises as the REAL file.
/// A source named like the file anywhere else may be anybody's content.
const CANONICAL: &[&str] = &["~/.claude/", "$HOME/.claude/", "${HOME}/.claude/"];

pub(super) fn writes_the_exceptions_file(
    program: &str,
    args: &[String],
    writes_to: &[String],
) -> bool {
    if writes_to.iter().any(|f| may_be_the_exceptions_file(f)) {
        return true;
    }
    if program == "cp" {
        return copy_writes_the_exceptions_file(args);
    }
    if program == "dd" {
        // `if=` is read; only `of=` is written.
        return args
            .iter()
            .filter_map(|a| a.strip_prefix("of="))
            .any(may_be_the_exceptions_file);
    }
    WRITERS.contains(&program) && args.iter().any(|a| may_be_the_exceptions_file(a))
}

/// Whether a `cp` line may write the exceptions file.
///
/// A destination that may be the file refuses. A source named like the file
/// refuses unless it is the real file by a [`CANONICAL`] path and every option
/// is a plain copy option: copying the real file anywhere writes the file's own
/// content, and into ~/.claude (or a link to it) cp refuses "the same file".
/// Whether a destination is a directory is never guessed.
fn copy_writes_the_exceptions_file(args: &[String]) -> bool {
    let mut target_dir: Option<&str> = None;
    let mut operands: Vec<&str> = Vec::new();
    let mut plain = true;
    let mut words = args.iter();
    let mut options_done = false;
    while let Some(word) = words.next() {
        let word = word.as_str();
        if options_done || !word.starts_with('-') || word == "-" {
            operands.push(word);
        } else if word == "--" {
            options_done = true;
        } else if let Some(long) = word.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            let name = format!("--{name}");
            if CP_VALUE_LONG.contains(&name.as_str()) {
                let value = inline.or_else(|| words.next().map(String::as_str));
                if name == "--target-directory" {
                    target_dir = value;
                }
            } else if !CP_PLAIN_LONG.contains(&name.as_str()) {
                plain = false;
            }
        } else {
            // A short cluster: a value-taking letter consumes the rest of the
            // cluster, or the next word when the cluster ends there.
            let cluster = &word[1..];
            for (at, c) in cluster.char_indices() {
                if CP_VALUE_SHORT.contains(&c) {
                    let rest = &cluster[at + c.len_utf8()..];
                    let value = if rest.is_empty() {
                        words.next().map(String::as_str)
                    } else {
                        Some(rest)
                    };
                    if c == 't' {
                        target_dir = value;
                    }
                    break;
                }
                if !CP_PLAIN_SHORT.contains(&c) {
                    plain = false;
                }
            }
        }
    }
    let (sources, destination) = match target_dir {
        Some(dir) => (&operands[..], Some(dir)),
        None => match operands.split_last() {
            Some((last, rest)) => (rest, Some(*last)),
            None => (&operands[..], None),
        },
    };
    if destination.is_some_and(may_be_the_exceptions_file) {
        return true;
    }
    let named: Vec<&&str> = sources
        .iter()
        .filter(|s| may_be_the_exceptions_file(s))
        .collect();
    !named.is_empty() && !(plain && named.iter().all(|s| is_canonical(s)))
}

/// Whether *word* is the real file by one of its [`CANONICAL`] spellings.
fn is_canonical(word: &str) -> bool {
    CANONICAL
        .iter()
        .any(|prefix| word.strip_prefix(prefix) == Some(EXCEPTIONS_FILE))
}

/// Whether *word* names the file, or may expand into it.
///
/// A word with a shell expansion in it (`$`, a glob, a brace list, a
/// backtick) cannot be decided from the line. It counts when it sits under a
/// `.claude` directory or its leaf starts like the file: `~/.claude/$F`,
/// `~/.claude/yadgar-hook-*.json`, `yadgar-hook-exception?.json`.
pub(super) fn may_be_the_exceptions_file(word: &str) -> bool {
    if is_the_exceptions_file(word) {
        return true;
    }
    if !word.contains(['$', '*', '?', '[', '{', '`']) {
        return false;
    }
    let mut parts = word.rsplit(['/', '\\']);
    let leaf = parts.next().unwrap_or(word);
    let stem = EXCEPTIONS_FILE.split('-').next().unwrap_or(EXCEPTIONS_FILE);
    leaf.starts_with(stem) || parts.any(|p| p == ".claude")
}

pub(super) fn is_the_exceptions_file(word: &str) -> bool {
    word.split(['/', '\\', '\n'])
        .any(|part| part == EXCEPTIONS_FILE)
}

/// Whether the file is named anywhere in *line*, with quotes and backslashes
/// taken out, for the [`INDIRECT`] commands whose real arguments arrive later.
pub(super) fn line_names_the_file(line: &str) -> bool {
    let plain: String = line
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    plain.contains(EXCEPTIONS_FILE)
}

//! Writes to the exceptions file: which commands change it, and which only
//! read it.
//!
//! Split out of `guard.rs` at its size ceiling. The seam is the rule: the
//! `hook_config_tamper` refusal is the only one that asks "is this file a
//! destination?", and everything that answers it lives here.

use crate::hook::EXCEPTIONS_FILE;

/// Commands that write a file named on their command line.
///
/// A NAMED SET, and therefore an incomplete one — said plainly rather than
/// implied. Bash can reach a file by a hundred routes and this covers the ones
/// an agent actually uses; `Edit`, `Write` and `NotebookEdit` are exact, and
/// that is where the incident happened.
const WRITERS: &[&str] = &[
    "tee", "sed", "mv", "rm", "truncate", "chmod", "chown", "shred", "patch", "sponge", "python",
    "python3", "perl", "ruby", "node", "jq",
    // Not writers of bytes, but of a FIFO or device node in the file's place:
    // a reader that blocked on one would hang until the hook timed out.
    "mkfifo", "mknod",
];

/// Commands that READ every operand but one, and write only that one: the
/// last operand, or the directory `-t` / `--target-directory` names.
///
/// Apart from [`WRITERS`] because naming the file as a SOURCE is a read
/// (ledger 1590): `cp ~/.claude/yadgar-hook-exceptions.json /tmp/backup.json`
/// copies a human's decision somewhere else and changes nothing.
const COPIERS: &[&str] = &["cp", "install", "ln"];

/// The short options of [`COPIERS`] that take a value, and their long forms.
const COPIER_VALUE_SHORT: &[char] = &['t', 'S', 'm', 'o', 'g'];
const COPIER_VALUE_LONG: &[&str] = &[
    "--target-directory",
    "--suffix",
    "--mode",
    "--owner",
    "--group",
];

pub(super) fn writes_the_exceptions_file(
    program: &str,
    args: &[String],
    writes_to: &[String],
) -> bool {
    if writes_to.iter().any(|f| is_the_exceptions_file(f)) {
        return true;
    }
    if COPIERS.contains(&program) {
        return copy_writes_the_exceptions_file(args);
    }
    if program == "dd" {
        // `if=` is read; only `of=` is written.
        return args
            .iter()
            .filter_map(|a| a.strip_prefix("of="))
            .any(is_the_exceptions_file);
    }
    WRITERS.contains(&program) && args.iter().any(|a| is_the_exceptions_file(a))
}

/// Whether a `cp` / `install` / `ln` line writes the exceptions file.
///
/// It does when the DESTINATION is the file. It also does when a source is
/// named like the file and the destination is a directory: a copy into a
/// directory keeps the source's name. Whether a bare destination is a
/// directory cannot be known from the line, so a trailing `/`, `.`, `..`, `~`,
/// a `.claude` leaf and `-t` count as one; any other destination is taken as
/// the file it names.
fn copy_writes_the_exceptions_file(args: &[String]) -> bool {
    let mut target_dir: Option<&str> = None;
    let mut operands: Vec<&str> = Vec::new();
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
            if COPIER_VALUE_LONG.contains(&name.as_str()) {
                let value = inline.or_else(|| words.next().map(String::as_str));
                if name == "--target-directory" {
                    target_dir = value;
                }
            }
        } else {
            // A short cluster: the first value-taking letter consumes the rest
            // of the cluster, or the next word when the cluster ends there.
            let cluster = &word[1..];
            if let Some(at) = cluster.find(|c| COPIER_VALUE_SHORT.contains(&c)) {
                let rest = &cluster[at + 1..];
                let value = if rest.is_empty() {
                    words.next().map(String::as_str)
                } else {
                    Some(rest)
                };
                if cluster[at..].starts_with('t') {
                    target_dir = value;
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
    let Some(destination) = destination else {
        return false;
    };
    if target_dir.is_none() && is_the_exceptions_file(destination) {
        return true;
    }
    let into_a_directory = target_dir.is_some() || names_a_directory(destination);
    into_a_directory && sources.iter().any(|s| is_the_exceptions_file(s))
}

/// Whether a destination word visibly names a directory rather than a file.
fn names_a_directory(word: &str) -> bool {
    if word.ends_with('/') || word.ends_with('\\') {
        return true;
    }
    let leaf = word.rsplit(['/', '\\']).next().unwrap_or(word);
    matches!(leaf, "." | ".." | "~" | ".claude")
}

pub(super) fn is_the_exceptions_file(word: &str) -> bool {
    word.split(['/', '\\', '\n'])
        .any(|part| part == EXCEPTIONS_FILE)
}

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
//!
//! **WHAT A TEXT CHECK CANNOT SEE**, stated rather than implied. A command can
//! reach the file without its name, or a glob that can match it under
//! `.claude`, ever appearing on the line:
//!
//! * a path built in the same call: `D=~/.claude; f=yadgar; rm $D/$f*` is
//!   caught, because a variable set on the line takes its values there, and so
//!   is a wildcard directory on a line that spells `.claude` anywhere; a name
//!   built by `printf`, `base64 -d`, `rev`, `tr` or `xxd` and piped to `sh`,
//!   or by string concatenation inside an interpreter, is not;
//! * THE ONE GAP THIS CHECK ACCEPTS: a directory that is ONLY a variable
//!   set outside the line, or only a wildcard, on a line that never spells
//!   `.claude`, with a leaf that may name any file: `rm $D/$f` or
//!   `rm $D/*.json` with `D` from the environment, or `rm */*` under
//!   `dotglob`. Refusing those refused every `$a/$b` and `*/x/*`, dozens of
//!   ordinary commands a day. A leaf only the file is likely to match
//!   (`rm $D/y*.json`) still refuses. Setting `D` takes an earlier call that
//!   edits the user's profile, which names `.claude` in that call;
//! * a command that acts on the directory rather than the name:
//!   `cd ~/.claude && rm y*`, `rm -rf ~/.claude/`, `tar -x -C ~/.claude`,
//!   `find ~/.claude -name 'y*' -delete`;
//! * a link made with one name and written through another (`ln -s` then an
//!   `Edit` of the link).
//!
//! **KEPT ON PURPOSE**: a quoted pattern refuses as a glob does, so
//! `find . -not -path './.claude/*'` refuses; once the quotes are gone it
//! reads exactly as `rm ./.claude/*`.
//!
//! `Edit`, `Write` and `NotebookEdit` on the file's path are exact.

use crate::hook::EXCEPTIONS_FILE;
use crate::install::shellword::SimpleCommand;

mod expand;
use expand::{Bindings, EXPANSION};

/// Whether *line* names the exceptions file, or may expand into it.
///
/// The text is taken with quotes and backslashes removed and case folded, so
/// `yadgar-hook-exception\s.json`, `'yadgar-hook-'exceptions.json` and a
/// case-insensitive filesystem's `YADGAR-…` all count; a trailing `.`, space
/// or `:stream` suffix still contains the name. Then each word and redirect
/// target is checked for a shell expansion that may produce it (see
/// [`may_be_the_exceptions_file`]).
pub(super) fn names_the_exceptions_file(line: &str, commands: &[SimpleCommand]) -> bool {
    if line_names_the_file(line) {
        return true;
    }
    let line = Line {
        bindings: expand::bindings(commands),
        names_claude: plain(line).contains(".claude"),
    };
    commands.iter().enumerate().any(|(at, c)| {
        // `$(…)/y*` unquoted: the scanner ends one command at `$`, scans the
        // substitution as commands of its own, and starts another with
        // `/y*`, which begins with the substitution's output. The guard
        // cannot tell where the substitution ends, so after a word ending in
        // `$`, the first word of EVERY later command may begin with output.
        let after_substitution = commands[..at]
            .iter()
            .any(|p| p.words.last().is_some_and(|w| w.ends_with('$')));
        c.words
            .iter()
            .chain(&c.writes_to)
            .enumerate()
            .any(|(n, w)| {
                let word = if n == 0 && after_substitution {
                    format!("{EXPANSION}{w}")
                } else {
                    w.clone()
                };
                may_be_the_exceptions_file(&word, at, &line)
            })
    })
}

/// What the whole line tells a check of one of its words.
struct Line {
    /// The variables the line sets (see [`expand::bindings`]).
    bindings: Bindings,
    /// Whether `.claude` appears anywhere in the line's text. Then a wildcard
    /// directory may be it: `D=$(echo ~/.claude); rm $D/y*` builds the path in
    /// a command substitution, which the scanner splits off as its own
    /// command, so `$D` has no value the guard can read.
    names_claude: bool,
}

/// *line* with quotes and backslashes removed, ASCII case folded.
fn plain(line: &str) -> String {
    line.chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Whether the file's name appears in *line*, quotes and backslashes removed,
/// ignoring ASCII case.
fn line_names_the_file(line: &str) -> bool {
    plain(line).contains(EXCEPTIONS_FILE)
}

/// Whether *word* names the file, or a shell expansion in it could produce
/// the file under a `.claude` directory.
///
/// A variable set on the line takes the values it is set to there; every
/// other expansion stands for `*` (see [`expand`]). Brace lists are expanded,
/// and the word is cut at `/` into path components, each GLOB-MATCHED against
/// `.claude` and the file name, ignoring case: `~/.c*/y*`,
/// `~/.c[l]aude/*.json`, `~/{.claude,}/*.json` and `D=~/.claude; rm $D/$a`
/// all count; `~/.claude/*.md`, `'yadgar.*'` and `$YADGAR_URL` do not. A `*`
/// is let match a leading dot, unlike the shell by default, because `dotglob`
/// can. The word refuses when [`reaches_the_file`] holds. *at* is the index
/// of the command the word is in.
fn may_be_the_exceptions_file(word: &str, at: usize, line: &Line) -> bool {
    if is_the_exceptions_file(word) {
        return true;
    }
    if !word.contains(['$', '*', '?', '[', OPEN_BRACE, '`', EXPANSION]) {
        return false;
    }
    let Some(words) = expand::substitute(word, &line.bindings, at) else {
        // Too many values to check: refuse rather than guess.
        return true;
    };
    words.iter().any(|w| {
        if is_the_exceptions_file(w) {
            return true;
        }
        let Some(alternatives) = brace_expand(w) else {
            // Too many alternatives to check: refuse rather than guess.
            return true;
        };
        alternatives
            .iter()
            .any(|alt| reaches_the_file(&expand::collapse(alt), line.names_claude))
    })
}

/// Whether the path *word*, its expansions collapsed, may be the file in a
/// `.claude` directory.
///
/// * The last component must glob-match the file name.
/// * Some earlier component, ANY earlier one, must be able to match
///   `.claude`, and something must be EVIDENCE that the path means it:
///   - that component cannot match every name in [`ANY_DIRS`]: `.c*`,
///     `[.]claude`, `[!x]*` and `.claud?` are evidence, `*`, `$D` and `**`
///     are not;
///   - or the leaf cannot match any name in [`ANY_FILES`]: `y*.json` and
///     `*-exceptions.json` are evidence, `*`, `*.json` and `$f` are not;
///   - or *names_claude*: the line spells `.claude` somewhere else.
///
///   Without evidence, every `$a/$b` and `*/x/*` was a write of the file.
///
/// Any earlier component, and not only the leaf's parent: `.claude/s/y*`
/// reaches the file when `s` is a link back to `.claude`, and a `.` or `..`
/// or an expansion in between may climb back to it as well.
fn reaches_the_file(word: &str, names_claude: bool) -> bool {
    let parts: Vec<&str> = word.split(['/', '\\']).collect();
    let Some((leaf, dirs)) = parts.split_last() else {
        return false;
    };
    let leaf = as_glob(leaf);
    if !glob_matches(&leaf, EXCEPTIONS_FILE) {
        return false;
    }
    let leaf_is_evidence = !ANY_FILES.iter().any(|name| glob_matches(&leaf, name));
    dirs.iter().any(|dir| {
        let glob = as_glob(dir);
        glob_matches(&glob, ".claude")
            && (names_claude
                || leaf_is_evidence
                || ANY_DIRS.iter().any(|name| !glob_matches(&glob, name)))
    })
}

/// Directory names unlike `.claude` and unlike each other: a pattern that
/// matches every one of them says nothing about which directory it names.
const ANY_DIRS: &[&str] = &["0", "x", "src", "node_modules"];

/// File names unlike the exceptions file: a leaf pattern that matches one of
/// them is as likely to name any file.
const ANY_FILES: &[&str] = &["0", "x", "a.json", "settings.json", "README.md"];

/// *part* as a glob pattern: an expansion may hold anything, so it is `*`.
fn as_glob(part: &str) -> String {
    part.replace(EXPANSION, "*")
}

/// The brace characters, named: a literal `{` in the code would unbalance the
/// size-ceiling hook's brace count.
const OPEN_BRACE: char = '\u{7b}';
const CLOSE_BRACE: char = '\u{7d}';
const OPEN_BYTE: u8 = 0x7b;
const CLOSE_BYTE: u8 = 0x7d;

/// The most brace alternatives one word is expanded into before refusing.
const MAX_ALTERNATIVES: usize = 256; // ADR-0569-EXCEPTION(CB): bound on expanding untrusted input, not a per-install knob.

/// *word* with every `{a,b}` list expanded, or `None` past
/// [`MAX_ALTERNATIVES`].
fn brace_expand(word: &str) -> Option<Vec<String>> {
    let mut done = Vec::new();
    let mut pending = vec![word.to_string()];
    while let Some(w) = pending.pop() {
        match first_brace_list(&w) {
            None => done.push(w),
            Some((open, close, items)) => {
                for item in items {
                    pending.push(format!("{}{}{}", &w[..open], item, &w[close + 1..]));
                }
            }
        }
        if done.len() + pending.len() > MAX_ALTERNATIVES {
            return None;
        }
    }
    Some(done)
}

/// The first `{…}` holding a top-level comma: its byte span and its items.
fn first_brace_list(word: &str) -> Option<(usize, usize, Vec<String>)> {
    let bytes = word.as_bytes();
    for (open, &b) in bytes.iter().enumerate() {
        if b != OPEN_BYTE {
            continue;
        }
        let (mut depth, mut start, mut items) = (0usize, open + 1, Vec::new());
        for (i, &c) in bytes.iter().enumerate().skip(open) {
            match c {
                OPEN_BYTE => depth += 1,
                CLOSE_BYTE => {
                    depth -= 1;
                    if depth == 0 {
                        if items.is_empty() {
                            break; // `{x}` with no comma is not a list.
                        }
                        // ONE HOLDING WHITESPACE is read as one expansion:
                        // unquoted, a space or newline ends the word, so the
                        // list came from quotes, where the shell expands no
                        // brace, or has a quoted item, `{.claude," "}`, where
                        // it does. Python and Rust in a heredoc body are full
                        // of `{a, b}`, and expanding each refused the line at
                        // the bound; the expansion covers both readings.
                        if bytes[open..i].iter().any(u8::is_ascii_whitespace) {
                            return Some((open, i, vec![EXPANSION.to_string()]));
                        }
                        items.push(word[start..i].to_string());
                        return Some((open, i, items));
                    }
                }
                b',' if depth == 1 => {
                    items.push(word[start..i].to_string());
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    None
}

/// Whether glob *pattern* (`*`, `?`, `[…]`, `[!…]`) matches *name*, ignoring
/// ASCII case.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let n: Vec<char> = name.to_ascii_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if pi < p.len() && (p[pi] == '?' || class_or_literal(&p, pi, n[ni])) {
            pi = advance(&p, pi);
            ni += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Whether the pattern element at pi* (a literal or a `[…]` class) matches *c*.
fn class_or_literal(p: &[char], pi: usize, c: char) -> bool {
    if p[pi] != '[' {
        return p[pi] == c;
    }
    let Some(end) = p[pi + 1..]
        .iter()
        .skip(1)
        .position(|&x| x == ']')
        .map(|n| pi + n + 2)
    else {
        return p[pi] == c; // An unclosed `[` is a literal.
    };
    let body = &p[pi + 1..end];
    let (negate, body) = match body.first() {
        Some('!' | '^') => (true, &body[1..]),
        _ => (false, body),
    };
    let mut hit = false;
    let mut k = 0;
    while k < body.len() {
        if k + 2 < body.len() && body[k + 1] == '-' {
            hit |= body[k] <= c && c <= body[k + 2];
            k += 3;
        } else {
            hit |= body[k] == c;
            k += 1;
        }
    }
    hit != negate
}

/// The index after the pattern element at pi*.
fn advance(p: &[char], pi: usize) -> usize {
    if p[pi] == '[' {
        if let Some(n) = p[pi + 1..].iter().skip(1).position(|&x| x == ']') {
            return pi + n + 3;
        }
    }
    pi + 1
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glob_matcher_follows_shell_globs_ignoring_case() {
        assert!(glob_matches(".c*", ".claude"));
        assert!(glob_matches(".c[l]aude", ".CLAUDE"));
        assert!(glob_matches(".claud?", ".claude"));
        assert!(glob_matches("*", ".claude"), "a leading dot is matchable");
        assert!(!glob_matches(".c[!l]aude", ".claude"));
        assert!(!glob_matches("*.md", EXCEPTIONS_FILE));
        assert!(glob_matches("y*-[a-z]ook-*.json", EXCEPTIONS_FILE));
    }

    #[test]
    fn brace_lists_expand_and_a_huge_one_refuses() {
        let mut alts = brace_expand("~/.c{l,}aude/{a,b}").unwrap();
        alts.sort();
        assert_eq!(
            alts,
            ["~/.caude/a", "~/.caude/b", "~/.claude/a", "~/.claude/b"]
        );
        assert_eq!(brace_expand("{x}").unwrap(), ["{x}"]);
        assert!(brace_expand(&"{a,b}".repeat(10)).is_none());
        assert_eq!(
            brace_expand("f(a, {b, c})").unwrap(),
            ["f(a, \0)"],
            "a list holding whitespace is one expansion, not a product"
        );
    }

    #[test]
    fn a_wildcard_directory_is_no_evidence_of_claude() {
        assert!(reaches_the_file("~/.c*/y*", false));
        assert!(reaches_the_file("~/.claude/\0", false));
        assert!(reaches_the_file("~/.claude/x/\0", false));
        assert!(reaches_the_file("~/.claude/s/y*", false), "s may be a link");
        assert!(reaches_the_file("~/[!x]*/y*", false));
        assert!(reaches_the_file("~/[.0]*/y*", false));
        assert!(!reaches_the_file("\0/\0", false));
        assert!(!reaches_the_file("*/node_modules/*", false));
        assert!(!reaches_the_file("?*/*.json", false));
        assert!(
            reaches_the_file("\0/y*.json", false),
            "a leaf only the file is likely to match"
        );
        assert!(reaches_the_file("*/y*.json", false));
        assert!(
            reaches_the_file("\0/y*", true),
            "evidence when the line spells .claude"
        );
    }
}

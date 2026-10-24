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
//!   caught only while a component can still match `.claude`; a name built by
//!   `printf`, `base64 -d`, `rev`, `tr` or `xxd` and piped to `sh`, or by
//!   string concatenation inside an interpreter, is not;
//! * a command that acts on the directory rather than the name:
//!   `cd ~/.claude && rm y*`, `rm -rf ~/.claude/`, `tar -x -C ~/.claude`,
//!   `find ~/.claude -name 'y*' -delete`;
//! * a link made with one name and written through another (`ln -s` then an
//!   `Edit` of the link).
//!
//! **TWO RELAXATIONS, AND ONLY TWO** (ledger 1701, ADR-0969). The check
//! below refused agents' ordinary commands. Two shapes are relaxed, each
//! under conditions in `tamper/allow.rs` that fall back to this check on
//! anything else: the body of a quoted-delimiter heredoc written by `cat`
//! to a plain file, on a line that runs nothing that could execute it; and
//! `$NAME` for a variable set only by literal top-level assignments before
//! its first use. Separately, brace-list items holding whitespace collapse
//! into one, which changes no answer (see [`first_brace_list`]) but keeps
//! heredoc code under the alternatives bound. One case is refused that was
//! not: an unquoted `$(…)` whose output starts a path, whether as an
//! argument (`rm $(echo ~/.claude)/y*`) or a redirect target
//! (`echo x > $(echo ~/.claude)/y*`).
//!
//! **A THIRD RELAXATION** (yaadgaar #402, ADR-1005), under the same rule
//! (ADR-0971: a listed shape with a bypass test): the one endpoint operand
//! of a plain `gh api` call is a URL path, not a file (`tamper/gh_api.rs`).
//!
//! A refusal names WHY (#404): the line names the file, or the first word
//! the check could not prove, quoted, so an agent knows what to reword.
//!
//! `Edit`, `Write`, `MultiEdit` and `NotebookEdit` on the file's path are
//! exact.

use crate::hook::EXCEPTIONS_FILE;
use crate::install::shellword::{shell_commands, SimpleCommand};

mod allow;
mod bin_dirs;
mod gh_api;
mod protected;
mod redirects;
mod shadow;
pub(super) use bin_dirs::is_before_gh;
pub(super) use protected::{bash_touches_a_protected_path, is_protected_path};
pub(super) use shadow::names_a_gh_file;

/// Whether a Bash *line* shadows gh or writes before it on PATH
/// (ADR-1020): see `tamper/shadow.rs` and `tamper/bin_dirs.rs`. A data
/// heredoc's body is not commands, as for every tamper check, and a `>|`,
/// `>!` or `>&file` redirect reads as `>` (`tamper/redirects.rs`).
pub(super) fn shadows_or_plants_gh(
    line: &str,
    commands: &[SimpleCommand],
    env: &super::GuardEnv,
) -> bool {
    let base = allow::data_heredocs_removed(line);
    let stripped = redirects::plain(base.as_deref().unwrap_or(line)).or(base);
    let scanned;
    let (line, commands) = match &stripped {
        Some(stripped) => {
            scanned = shell_commands(stripped);
            (stripped.as_str(), scanned.as_slice())
        }
        None => (line, commands),
    };
    // A compound command's opener is not its program: the scanner keeps
    // `do`, `then`, `!` or an open brace as the first word, where `executed` would read
    // it as the program and skip what follows.
    let commands: Vec<SimpleCommand> = commands.iter().map(without_openers).collect();
    shadow::shadows_gh(line, &commands) || bin_dirs::writes_before_gh(&commands, env)
}

/// *command* without the openers (`runs::OPENERS`) before its first word.
fn without_openers(command: &SimpleCommand) -> SimpleCommand {
    let skip = command
        .words
        .iter()
        .take_while(|w| super::runs::OPENERS.contains(&w.as_str()))
        .count();
    SimpleCommand {
        words: command.words[skip..].to_vec(),
        writes_to: command.writes_to.clone(),
    }
}

/// Whether *line* names the exceptions file, or may expand into it.
///
/// The text is taken with quotes and backslashes removed and case folded, so
/// `yadgar-hook-exception\s.json`, `'yadgar-hook-'exceptions.json` and a
/// case-insensitive filesystem's `YADGAR-…` all count; a trailing `.`, space
/// or `:stream` suffix still contains the name. Then each word and redirect
/// target is checked for a shell expansion that may produce it (see
/// [`may_be_the_exceptions_file`]).
///
/// `Some` says WHY, for the refusal (yaadgaar #404): the line names the
/// file, or the first word the check could not prove does not expand into
/// it, so an agent knows which word to reword.
pub(super) fn names_the_exceptions_file(line: &str, commands: &[SimpleCommand]) -> Option<String> {
    if line_names_the_file(line) {
        return Some("The line names the file.".into());
    }
    // RELAXATION 1: a data heredoc's body is not commands.
    let stripped = allow::data_heredocs_removed(line);
    let scanned;
    let (line, commands) = match &stripped {
        Some(stripped) => {
            scanned = shell_commands(stripped);
            (stripped.as_str(), scanned.as_slice())
        }
        None => (line, commands),
    };
    // RELAXATION 2: a proven literal variable is its literal.
    let literals = allow::Literals::of(line);
    // `$(…)/y*` unquoted: the scanner ends one command at `$`, scans the
    // substitution as commands of its own, and starts another with `/y*`,
    // which begins with the substitution's output. After a word ending in
    // `$`, the first word of every later command is read as `*` + word.
    let mut after_substitution = false;
    for c in commands {
        // RELAXATION 3: the endpoint of `gh api` is a URL path (`gh_api.rs`).
        let endpoint = gh_api::endpoint(c);
        for (n, w) in c.words.iter().chain(&c.writes_to).enumerate() {
            if endpoint == Some(n) {
                continue;
            }
            let word = if n == 0 && after_substitution {
                format!("*{w}")
            } else {
                w.clone()
            };
            let Some(alternatives) = literals.substitute(&word) else {
                // Too many values to check: refuse rather than guess.
                return Some(unproven(&word, "has too many values to check"));
            };
            if alternatives.iter().any(|a| may_be_the_exceptions_file(a)) {
                return Some(unproven(&word, "may expand into it"));
            }
        }
        after_substitution |= c.words.iter().chain(&c.writes_to).any(|w| w.ends_with('$'));
    }
    None
}

/// The longest stretch of a word quoted back in a refusal. A BOUND, not a
/// knob: a command can be 64 KiB, and a refusal is read by a person.
const MAX_QUOTED: usize = 120; // ADR-0569-EXCEPTION(CB): bound on echoing untrusted input, not a per-install knob.

/// The refusal's detail for *word*: quoted, control characters dropped, cut
/// at [`MAX_QUOTED`] characters.
fn unproven(word: &str, why: &str) -> String {
    let printable: String = word.chars().filter(|c| !c.is_control()).collect();
    let cut: String = printable.chars().take(MAX_QUOTED).collect();
    let more = if printable.chars().count() > MAX_QUOTED {
        "…"
    } else {
        ""
    };
    format!(
        "The guard could not prove that the word `{cut}{more}` does not name the file: it {why}. \
         Reword it with a literal path, or put the command in a script file."
    )
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

/// Whether *word* names the file, or a shell expansion in it could produce
/// the file under a `.claude` directory.
///
/// Brace lists are expanded and each path component is GLOB-MATCHED (ignoring
/// case) against `.claude` and the file name, with a `$`-expansion or backtick
/// standing for `*`: `~/.c*/y*`, `~/.c[l]aude/*.json`, `~/{.claude,}/*.json`
/// and `$D/$a$b` all count. A word refuses only when some component before
/// the last can match `.claude` and the last can match the file; a glob that
/// cannot reach the file there (`~/.claude/*.md`, `'yadgar.*'`, `$YADGAR_URL`)
/// does not. A `*` is let match a leading dot, unlike the shell by default,
/// because `dotglob` and a variable can.
fn may_be_the_exceptions_file(word: &str) -> bool {
    if is_the_exceptions_file(word) {
        return true;
    }
    if !word.contains(['$', '*', '?', '[', OPEN_BRACE, '`']) {
        return false;
    }
    let Some(alternatives) = brace_expand(word) else {
        // Too many alternatives to check: refuse rather than guess.
        return true;
    };
    alternatives.iter().any(|alt| {
        let parts: Vec<String> = alt.split(['/', '\\']).map(as_glob).collect();
        let Some((leaf, dirs)) = parts.split_last() else {
            return false;
        };
        glob_matches(leaf, EXCEPTIONS_FILE) && dirs.iter().any(|d| glob_matches(d, ".claude"))
    })
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
                        items.push(word[start..i].to_string());
                        return Some((open, i, one_spaced_item(items)));
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

/// *part* as a glob pattern: each `$name`, `${…}`, `$(…)` or backtick span
/// becomes `*`, since it may expand to anything.
fn as_glob(part: &str) -> String {
    let chars: Vec<char> = part.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '$' => {
                out.push('*');
                i += 1;
                match chars.get(i) {
                    Some(&OPEN_BRACE) => i = closing(&chars, i, OPEN_BRACE, CLOSE_BRACE),
                    Some('(') => i = closing(&chars, i, '(', ')'),
                    _ => {
                        while chars
                            .get(i)
                            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                        {
                            i += 1;
                        }
                    }
                }
            }
            '`' => {
                out.push('*');
                i = chars[i + 1..]
                    .iter()
                    .position(|&c| c == '`')
                    .map_or(chars.len(), |n| i + n + 2);
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The index just past the bracket that closes the one at *open*.
fn closing(chars: &[char], open: usize, left: char, right: char) -> usize {
    let mut depth = 0;
    for (i, &c) in chars.iter().enumerate().skip(open) {
        if c == left {
            depth += 1;
        } else if c == right {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
    }
    chars.len()
}

/// *items* with every item that holds whitespace and no `/` replaced by
/// ONE `" "` item.
///
/// THIS CHANGES NO ANSWER. Such an item lands inside one path component,
/// and a component holding whitespace can match neither `.claude` nor the
/// file name, whatever else it holds; so every such item gives the same
/// answer as `" "`. What it changes is the count: Python and Rust in a
/// heredoc body are full of `{a, b}`, and expanding each multiplied the
/// alternatives past the bound, which refuses.
fn one_spaced_item(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut spaced = false;
    for item in items {
        if item.contains(char::is_whitespace) && !item.contains(['/', '\\']) {
            if !spaced {
                out.push(" ".to_string());
                spaced = true;
            }
        } else {
            out.push(item);
        }
    }
    out
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
            brace_expand("{a b,c d,e}").unwrap().len(),
            2,
            "items holding whitespace collapse into one"
        );
        assert!(brace_expand(&"{ a, b}".repeat(10)).is_some());
    }

    #[test]
    fn an_expansion_stands_for_anything() {
        assert_eq!(as_glob("$D"), "*");
        assert_eq!(as_glob("${a}x$(b)`c`"), "*x**");
    }
}

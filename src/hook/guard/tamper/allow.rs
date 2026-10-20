//! The two narrow relaxations of the exceptions-file check (ledger 1701).
//!
//! Everything else is checked exactly as before this change. A relaxation
//! applies only to a line whose every command is on a short list; any other
//! command, operator or expansion leaves the line to the old check. The one
//! assumption: a command on a list is the program its name says, and not a
//! shell function from the user's profile.
//!
//! * [`data_heredocs_removed`]: the body of a `cat > FILE <<'EOF'` heredoc,
//!   whose delimiter is QUOTED, is data written to a file; the shell expands
//!   nothing in it. On a line that runs nothing that could execute that file,
//!   the body is not checked as commands. Its text is still checked for the
//!   file's name. This is the PR-body heredoc the ticket reported.
//! * [`Literals`]: a variable set ONLY by top-level, unconditional
//!   `NAME=/a/literal/path` assignments, all written before its first use,
//!   holds one of those literals wherever it is read, so `$NAME` is that
//!   literal rather than `*`. This is `S=/tmp/s; … $S/pr/…`, the scratch-file
//!   loop the ticket reported.

use std::collections::{HashMap, HashSet};

use super::MAX_ALTERNATIVES;
use crate::install::shellword::{shell_commands_with_separators, SimpleCommand};

/// Commands with no option that executes a program or reads one by path, as
/// checked one by one: `mkdir`, `ls`, `wc` (`--files0-from` reads a file
/// list but runs nothing), `cat`, `head`, `tail`, `cut`, `basename`,
/// `dirname`, `echo`, `true`. `sort` (`-o` writes) and `uniq` (second
/// operand writes) are deliberately absent. `cd` is here for the data-heredoc
/// line, which runs no substitution, but NOT for the literal relaxation,
/// where it would move `$PWD`. `git` and `gh` are handled apart.
const PLAIN_HEREDOC: &[&str] = &[
    "cd", "mkdir", "ls", "wc", "cat", "head", "tail", "cut", "basename", "dirname", "echo", "true",
];

/// The same, less `cd`: the literal relaxation reads `$PWD`, so a `cd` would
/// change a value it may resolve.
const PLAIN_LITERAL: &[&str] = &[
    "mkdir", "ls", "wc", "cat", "head", "tail", "cut", "basename", "dirname", "echo", "true",
];

/// The `git` subcommands a data-heredoc line may run: each reads or writes
/// the repository and runs no arbitrary program. `rebase --exec`,
/// `difftool`/`mergetool`, `filter-branch`, `bisect run`, `submodule` and
/// the like are absent, as is any `-c`/`-C`/`--exec*`/`--git-dir`/GIT_* that
/// redirects what runs.
const GIT_SUBCOMMANDS: &[&str] = &[
    "fetch", "push", "status", "rev-list", "log", "diff", "show", "add", "commit", "checkout",
    "switch", "branch",
];

/// The `gh pr` verbs a data-heredoc line may run.
const GH_PR_VERBS: &[&str] = &["create", "edit", "comment"];

/// *line* with the bodies of its data heredocs removed, or `None` when it
/// holds no heredoc, a heredoc that is not data, or a command that could run
/// a file the heredoc wrote.
///
/// A data heredoc is the only heredoc on its line, has a quoted delimiter
/// (`<<'EOF'` or `<<"EOF"`), and is fed to `cat` whose output goes to one
/// plain `.md` or `.txt` path with no dot directory (`.git`, a profile) and
/// no `..` in it: a file nothing runs as code by its name. The rest of the line may run
/// only [`PLAIN`] commands, `git` with none of [`GIT_RUNS_OTHER_CODE`], and
/// `gh` only as `gh pr …`. No `$(…)`, backtick or process substitution.
pub(super) fn data_heredocs_removed(line: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut lines = line.split('\n');
    let mut found = false;
    while let Some(text) = lines.next() {
        out.push(text);
        let Some(delimiter) = heredoc_delimiter(text)? else {
            continue;
        };
        found = true;
        while lines.next()? != delimiter {}
    }
    let stripped = out.join("\n");
    // The executed part (bodies removed) carries NO expansion or glob: a
    // parser differential over `$`, a substitution or a glob cannot then
    // make the guard read a different command than the shell runs. The
    // tokens the shape needs — quotes, `<`, `>`, `|`, `&`, `;` — stay.
    if stripped
        .chars()
        .any(|c| HEREDOC_FORBIDDEN.contains(c) || !c.is_ascii())
    {
        return None;
    }
    let plain = shell_commands_with_separators(&stripped)
        .iter()
        .all(|(c, _)| heredoc_line_command(c));
    (found && plain).then_some(stripped)
}

/// Characters barred from the executed part of a data-heredoc line: every
/// expansion, substitution, glob and comment introducer. `<`, `>`, `|`,
/// `&`, `;`, quotes and `=` stay, since the shape needs them.
const HEREDOC_FORBIDDEN: &str = "$`*?[]()~!#\\\t";

/// The quoted delimiter of the heredoc *text* opens: `Some(None)` for no
/// heredoc, and `None` for one that is not data (see
/// [`data_heredocs_removed`]).
fn heredoc_delimiter(text: &str) -> Option<Option<String>> {
    let ops: Vec<usize> = text
        .match_indices("<<")
        .map(|(i, _)| i)
        .filter(|&i| !text[i + 2..].starts_with('<') && (i == 0 || !text[..i].ends_with('<')))
        .collect();
    match ops.as_slice() {
        [] => return Some(None),
        [_] => {}
        _ => return None,
    }
    // `<<-` strips leading tabs and is not modelled.
    if text[ops[0] + 2..].starts_with('-') {
        return None;
    }
    let rest = text[ops[0] + 2..].trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let end = rest[1..].find(quote)? + 1;
    let delimiter = &rest[1..end];
    // The word ends at the closing quote: `<<"E"OF` is the delimiter `EOF`,
    // not `E`, so anything but whitespace, end or a redirect after the quote
    // means this is not a quoted-delimiter heredoc the guard can read.
    if rest[end + 1..]
        .chars()
        .next()
        .is_some_and(|c| !c.is_whitespace() && c != '<' && c != '>')
    {
        return None;
    }
    if delimiter.is_empty()
        || !delimiter
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let commands = shell_commands_with_separators(text);
    let (command, _) = commands.last()?;
    let file = command.writes_to.first()?;
    let data = command.words.first().is_some_and(|w| w == "cat")
        && command.writes_to.len() == 1
        && !file.contains(['$', '`', '*', '?', '[', '\u{7b}', '~'])
        && !file
            .split('/')
            .any(|part| part.starts_with('.') && part != "." && part != "..")
        && !file.split('/').any(|part| part == "..")
        && (file.ends_with(".md") || file.ends_with(".txt"));
    data.then(|| Some(delimiter.to_string()))
}

fn heredoc_line_command(command: &SimpleCommand) -> bool {
    let words: Vec<&str> = command.words.iter().map(String::as_str).collect();
    match words.first() {
        // `2>&1` scans as a word, a redirect, a separator and a word.
        None => true,
        Some(w) if w.chars().all(|c| c.is_ascii_digit()) => words.len() == 1,
        Some(&"git") => git_is_safe(&words[1..]),
        Some(&("gh" | "gh-personal")) => {
            words.get(1) == Some(&"pr") && words.get(2).is_some_and(|v| GH_PR_VERBS.contains(v))
        }
        Some(first) => PLAIN_HEREDOC.contains(first),
    }
}

/// Whether a `git` invocation (its words after `git`) is a plain repository
/// command: no option that redirects what runs, and its first non-option
/// word is an allowlisted subcommand.
fn git_is_safe(args: &[&str]) -> bool {
    let no_redirect = !args.iter().any(|w| {
        matches!(*w, "-c" | "-C" | "-x")
            || w.starts_with("--exec")
            || w.starts_with("--git-dir")
            || w.starts_with("--work-tree")
            || w.starts_with("--config-env")
            || w.starts_with("--namespace")
            || w.starts_with("GIT_")
    });
    let subcommand = args.iter().find(|w| !w.starts_with('-'));
    no_redirect && subcommand.is_some_and(|w| GIT_SUBCOMMANDS.contains(w))
}

/// Whether *c* may appear in a line the literal relaxation reads: letters,
/// digits, space, newline, the path punctuation `_./=:@,+-`, and the few
/// operators the shape needs — `;` and `|` to separate commands, `>` to
/// redirect, and the `$ { }` of a reference. No quote, backslash, backtick,
/// glob (`* ? [ ]`), `&`, `(` / `)`, `<`, `~`, `!`, `#`, tab or non-ASCII:
/// each could make a dangerous line parse unlike how this reads it. `|` is
/// allowed because each pipe stage is a subshell that sets no variable and
/// whose command is still checked.
fn is_conservative(c: char) -> bool {
    c.is_ascii_alphanumeric() || " \n_./=:@,+-;|${}>".contains(c)
}

/// The literal values of the variables a line sets only by proven literal
/// assignments, all before the variable's first use.
pub(super) struct Literals {
    values: HashMap<String, Vec<String>>,
}

impl Literals {
    /// The proven literal variables of *line*: none at all unless the line
    /// passes the conservative charset gate (so it holds no `$(…)`, backtick,
    /// quote or heredoc) and every command is a [`PLAIN_LITERAL`] command, a
    /// `for` loop, or a literal assignment.
    pub(super) fn of(line: &str) -> Self {
        let none = Self {
            values: HashMap::new(),
        };
        // CONSERVATIVE CHARSET over the WHOLE raw command. A quote, a
        // backslash, a glob, a brace, a `(` or `)`, an operator, `<`/`>`,
        // `~`, `!`, `#`, tab or non-ASCII byte leaves the whole line to
        // `main`. `$(…)` and backticks hold a `(` or a backtick, so a span —
        // and any assignment inside one — can never appear here.
        if line.contains("<<") || line.chars().any(|c| !is_conservative(c)) {
            return none;
        }
        let commands = shell_commands_with_separators(line);
        if !commands.iter().all(|(c, _)| literal_line_command(c)) {
            return none;
        }
        let mut values: HashMap<String, Vec<String>> = HashMap::new();
        let mut disproven: HashSet<String> = HashSet::new();
        let mut parens = 0i32;
        let mut nesting = 0i32;
        for (at, (command, before)) in commands.iter().enumerate() {
            parens += before
                .iter()
                .map(|op| i32::from(op == "(") - i32::from(op == ")"))
                .sum::<i32>();
            let after = commands.get(at + 1).and_then(|(_, b)| b.first());
            let pure = command.writes_to.is_empty()
                && command
                    .words
                    .iter()
                    .all(|w| literal_assignment(w).is_some());
            let proven = pure
                && parens == 0
                && nesting == 0
                && before.last().is_none_or(|op| op == ";" || op == "\n")
                && after.is_none_or(|op| op != "|" && op != "&");
            for word in &command.words {
                if let Some((name, value)) = literal_assignment(word) {
                    if proven {
                        values
                            .entry(name.to_string())
                            .or_default()
                            .push(value.to_string());
                    } else {
                        disproven.insert(name.to_string());
                    }
                }
            }
            let words = after_do(&command.words);
            let first = words.first().map(String::as_str);
            if first == Some("for") {
                if let Some(name) = words.get(1) {
                    disproven.insert(name.clone());
                }
            }
            nesting += i32::from(first == Some("for")) - i32::from(first == Some("done"));
        }
        // A3: names the shell maintains itself never resolve.
        values.retain(|name, _| {
            !SHELL_MAINTAINED.contains(&name.as_str())
                && !disproven.contains(name)
                && assigned_before_read(line, name)
        });
        Self { values }
    }

    /// *word* with every `$NAME` / `${NAME}` replaced by each of its proven
    /// literals, or `None` past [`MAX_ALTERNATIVES`].
    ///
    /// ALL OR NOTHING. Every `$` in the word must begin a PROVEN `$NAME` /
    /// `${NAME}`. If any does not — a plain variable the line does not fix, a
    /// positional or special parameter (`$1`, `$@`), a `$(…)` command
    /// substitution, or a `${…}` that is not a plain name — the word is
    /// returned UNCHANGED, so the check on `main` decides it exactly. A
    /// proven literal directory does not make a word safe while such an
    /// expansion remains in it: it may expand to `../.claude/…` and climb
    /// back to the file.
    pub(super) fn substitute(&self, word: &str) -> Option<Vec<String>> {
        let chars: Vec<char> = word.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' {
                match reference(&chars, i) {
                    Some((name, end)) if self.values.contains_key(&name) => i = end,
                    _ => return Some(vec![word.to_string()]),
                }
            } else {
                i += 1;
            }
        }
        let mut alternatives = vec![String::new()];
        let mut i = 0;
        while i < chars.len() {
            let Some((name, end)) = reference(&chars, i) else {
                alternatives.iter_mut().for_each(|a| a.push(chars[i]));
                i += 1;
                continue;
            };
            let values = &self.values[&name];
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
}

fn after_do(words: &[String]) -> &[String] {
    let skip = words.iter().take_while(|w| *w == "do").count();
    &words[skip..]
}

fn literal_line_command(command: &SimpleCommand) -> bool {
    let words = after_do(&command.words);
    let Some(first) = words.first() else {
        return true;
    };
    words.iter().all(|w| literal_assignment(w).is_some())
        || matches!(first.as_str(), "for" | "done")
        || PLAIN_LITERAL.contains(&first.as_str())
}

/// Names bash and zsh maintain themselves, so an assignment on the line does
/// not fix their value where it is read.
const SHELL_MAINTAINED: &[&str] = &[
    "_", "PWD", "OLDPWD", "SHLVL", "IFS", "RANDOM", "SECONDS", "LINENO", "REPLY", "PATH", "HOME",
];

/// The NAME and value of a `NAME=value` word whose value is a plain literal
/// path: no expansion, glob, quote, tilde or space.
fn literal_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let named = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let literal = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./+:@%,-".contains(c));
    (named && literal).then_some((name, value))
}

/// Whether every occurrence of *name* as a token in *line* is `$name`,
/// `${name}` or a word-initial `name=`, and every assignment comes before
/// the first read.
fn assigned_before_read(line: &str, name: &str) -> bool {
    let c: Vec<char> = line.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let word_char = |x: char| x.is_ascii_alphanumeric() || x == '_';
    let mut read_seen = false;
    for i in 0..c.len() {
        let hit = c[i..].starts_with(&n)
            && (i == 0 || !word_char(c[i - 1]))
            && c.get(i + n.len()).is_none_or(|&x| !word_char(x));
        if !hit {
            continue;
        }
        let before = i.checked_sub(1).map(|k| c[k]);
        let read = before == Some('$')
            || (before == Some('\u{7b}')
                && i >= 2
                && c[i - 2] == '$'
                && c.get(i + n.len()) == Some(&'\u{7d}'));
        let assigned = before.is_none_or(|b| b.is_whitespace() || b == ';')
            && c.get(i + n.len()) == Some(&'=');
        if read {
            read_seen = true;
        } else if !assigned || read_seen {
            return false;
        }
    }
    true
}

/// The NAME of a plain `$NAME` or `${NAME}` at *at*, and the index after it.
fn reference(chars: &[char], at: usize) -> Option<(String, usize)> {
    if chars[at] != '$' {
        return None;
    }
    let braced = chars.get(at + 1) == Some(&'\u{7b}');
    let start = at + 1 + usize::from(braced);
    let len = chars[start..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
        .count();
    if len == 0 || chars[start].is_ascii_digit() {
        return None;
    }
    let name: String = chars[start..start + len].iter().collect();
    let end = start + len;
    if !braced {
        return Some((name, end));
    }
    (chars.get(end) == Some(&'\u{7d}')).then(|| (name, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_literal_assigned_before_every_read_is_proven() {
        let l = Literals::of("S=/tmp/s; mkdir -p $S/pr; ls ${S}/x");
        assert_eq!(l.substitute("$S/pr").unwrap(), ["/tmp/s/pr"]);
        for line in [
            "ls $S; S=/tmp",
            "S=$HOME; ls $S",
            "false && S=/tmp; ls $S",
            "S=/tmp; read S; ls $S",
            "S=/tmp; for S in a; do ls $S; done",
            "S=/tmp; grep x f; ls $S",
            "S=/tmp | cat; ls $S",
        ] {
            assert_eq!(
                Literals::of(line).substitute("$S").unwrap(),
                ["$S"],
                "{line}"
            );
        }
    }

    #[test]
    fn one_excluded_character_turns_off_the_literal_relaxation() {
        // Each is the proven shape with one exotic character added; the whole
        // line then falls back to the check on `main`, and nothing resolves.
        // A reported parser-differential: `'$S'` and `\$S` are literal in the
        // shell but this would otherwise substitute them.
        for exotic in [
            "S=/tmp; ls '$S'/y",   // single quote
            "S=/tmp; ls \"$S\"/y", // double quote
            "S=/tmp; ls \\$S/y",   // backslash
            "S=/tmp; ls `pwd`/$S", // backtick
            "S=/tmp; ls $S/*",     // glob star
            "S=/tmp; ls $S/y?",    // glob question
            "S=/tmp; ls $S/[a]",   // glob class
            "S=/tmp; ls $S/~x",    // tilde
            "S=/tmp; ls $S/x & y", // background
            "S=/tmp; (ls $S)",     // subshell paren
            "S=/tmp\tls $S",       // tab
            "S=/tmp; ls $S/café",  // non-ascii
            "S=/tmp; ls $S/x # c", // comment
        ] {
            assert_eq!(
                Literals::of(exotic).substitute("$S").unwrap(),
                ["$S"],
                "{exotic}"
            );
        }
    }

    #[test]
    fn a_heredoc_is_data_only_for_cat_into_a_plain_file_on_a_plain_line() {
        let data = "cat > /tmp/b.md <<'EOF'\nx `y/` z\nEOF\ngh pr create --body-file /tmp/b.md";
        assert_eq!(
            data_heredocs_removed(data).as_deref(),
            Some("cat > /tmp/b.md <<'EOF'\ngh pr create --body-file /tmp/b.md")
        );
        for line in [
            // M11: an unquoted delimiter; M12: a dot directory; `<<-`.
            "cat > /tmp/b.md <<EOF\nx\nEOF",
            "cat > /tmp/.h/x.md <<'EOF'\nx\nEOF",
            "cat > /tmp/b.md <<-'EOF'\nx\nEOF",
            "cat > /tmp/b <<EOF\nx\nEOF",
            "bash <<'EOF'\nx\nEOF",
            "cat > /tmp/b.sh <<'EOF'\nx\nEOF\nbash /tmp/b.sh",
            "cat > .git/hooks/pre-push <<'EOF'\nx\nEOF\ngit push",
            "cat > /tmp/x.sh <<'EOF'\nx\nEOF",
            "cat > /tmp/h/pre-push <<'EOF'\nx\nEOF\ngit --git-dir=/tmp/h push",
            "cat > /tmp/b <<'EOF'\nx\n",
        ] {
            assert_eq!(data_heredocs_removed(line), None, "{line}");
        }
    }
}

//! The shell grammar, in ONE place: how a command string is written and read.
//!
//! Split out of `hooks.rs` along the seam that module's own scars keep pointing
//! at. **A second parser is a second set of rules to disagree with the first**,
//! and this file has already cost that twice: `health.rs` grew a `first_word`
//! that returned `/opt/don` for a quoted path with an apostrophe, so `verify`
//! reported a healthy install as a missing binary. Quoting and splitting are one
//! round trip and belong in one file, where the pair can be read together.
//!
//! Not a shell: no expansion, no operators, no globbing. Enough to write argv[0]
//! so a shell reads it back whole, and to read argv[0..2] back off a string
//! somebody may have written by hand.

/// Quote a path for a command string that a shell will parse.
///
/// POSIX single-quoting: right on two of the three shells this string can
/// reach, and that is a limit written down rather than a claim about Windows.
///
/// > The `command` string is passed to a shell: `sh -c` on macOS and Linux, Git
/// > Bash on Windows, or PowerShell when Git Bash isn't installed.
/// > — <https://code.claude.com/docs/en/hooks>, "Exec form and shell form"
///
/// Git for Windows is optional, so which of the two runs there is the person's
/// install state and nothing an installer can know. `sh -c` and Git Bash parse
/// this as written. **PowerShell does not, and its doubled `''` escape would
/// not fix that**: a quoted string in command position is an EXPRESSION there,
/// so `'C:\…\yaadgaar.exe' hook prompt-recall` runs nothing under ANY escaping
/// without the call operator `&`. The quote style is not what breaks; the
/// shell-form contract is, so platform-conditional quoting buys nothing.
///
/// The same page documents a `shell` field (`"bash"` / `"powershell"`) that
/// would pin the dialect and remove the ambiguity. Not used here, and for a
/// weaker version of the reason below: no version floor is established for it,
/// where the exec form's is.
///
/// **The documented fix is the exec form, and it is a version floor rather than
/// an edit.** `args: string[]` beside `command` spawns the binary with no shell
/// and no quoting anywhere, added in Claude Code v2.1.139 — "so path
/// placeholders never need quoting"
/// (<https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md>). It
/// costs every older client: those ignore `args` and run `command` as shell
/// form, which is then the bare binary with no `hook <name>` after it — twelve
/// registrations that launch and do nothing. That floor decides which clients
/// this installer supports, so it is not decided here.
pub fn shell_quote(text: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Split a command string the way a shell would, enough to read argv[0..2].
///
/// Not a shell: no expansion, no globbing. It exists so that identity survives a
/// quoted path — `'/opt/my tools/yaadgaar' hook prompt-recall` is yadgar's, and
/// a naive `split_whitespace` would call it somebody else's.
pub fn shell_split(cmd: &str) -> Vec<String> {
    scan(cmd)
        .into_iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w),
            _ => None,
        })
        .collect()
}

/// One simple command a shell would run: its argv, and what it writes into.
#[derive(Debug, PartialEq, Eq)]
pub struct SimpleCommand {
    /// argv, with quotes removed and redirections taken out.
    pub words: Vec<String>,
    /// Files this command redirects its output INTO.
    ///
    /// Kept apart from `words` because reading a file and writing to it are
    /// different acts, and a guard that cannot tell them apart refuses `cat` on
    /// the very file it is only meant to protect from writes.
    pub writes_to: Vec<String>,
}

/// Split a command LINE into the separate commands a shell would run from it.
///
/// `cd infra && terraform apply` is TWO commands, and `terraform` is the command
/// word of the second one. A matcher that looked at the line as one string saw
/// `terraform` in the middle of it and had no way to say whether that was a
/// command being run or a word being mentioned — so it answered both questions
/// the same way and got both wrong.
pub fn shell_commands(line: &str) -> Vec<SimpleCommand> {
    let mut out = Vec::new();
    let mut current = SimpleCommand {
        words: Vec::new(),
        writes_to: Vec::new(),
    };
    let mut pending: Option<Token> = None;

    for token in scan(line) {
        match token {
            Token::Word(w) => match pending.take() {
                Some(Token::Into) => current.writes_to.push(w),
                // A file read FROM is an argument like any other.
                _ => current.words.push(w),
            },
            Token::Separator => {
                pending = None;
                if !current.words.is_empty() || !current.writes_to.is_empty() {
                    out.push(std::mem::replace(
                        &mut current,
                        SimpleCommand {
                            words: Vec::new(),
                            writes_to: Vec::new(),
                        },
                    ));
                }
            }
            redirect => pending = Some(redirect),
        }
    }
    if !current.words.is_empty() || !current.writes_to.is_empty() {
        out.push(current);
    }
    out
}

/// What the scanner found. The one place the shell's grammar is decided.
enum Token {
    Word(String),
    /// `;` `&` `&&` `||` `|` `(` `)` or a newline: one command ends here.
    Separator,
    /// `>` or `>>`: the next word is a file the command writes into.
    Into,
    /// `<`: the next word is a file the command reads from.
    From,
}

/// The single pass over a command string: quoting, escaping and operators.
///
/// ONE scanner, because a second is a second set of rules to disagree with the
/// first — this file has already cost that twice.
///
/// It is not a shell and does not try to be. Two known limits, written down
/// rather than discovered later: `2>&1` scans as the word `2`, a redirect, a
/// separator and the word `1`, so a caller sees two commands where a shell sees
/// one; and `$(…)` is not substitution, so the words inside it are read as
/// though they were typed at that point.
fn scan(cmd: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars().peekable();

    macro_rules! flush {
        () => {
            if started {
                out.push(Token::Word(std::mem::take(&mut current)));
                started = false;
            }
        };
    }

    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    started = true;
                }
            }
            (None, '>') => {
                flush!();
                // `>>` appends; for the question this answers it is the same act.
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
                out.push(Token::Into);
            }
            (None, '<') => {
                flush!();
                out.push(Token::From);
            }
            (None, c) if matches!(c, ';' | '|' | '&' | '(' | ')' | '\n') => {
                flush!();
                // `&&` and `||` separate exactly as their single forms do here.
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                out.push(Token::Separator);
            }
            (None, c) if c.is_whitespace() => flush!(),
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    // Not `flush!`: the macro clears `started`, which nothing reads after the
    // loop, and the compiler is right to say so.
    if started {
        out.push(Token::Word(current));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_path_survives_being_read_back() {
        assert_eq!(
            shell_split("'/opt/my tools/yaadgaar' hook prompt-recall"),
            ["/opt/my tools/yaadgaar", "hook", "prompt-recall"]
        );
    }

    #[test]
    fn what_is_written_is_what_is_read_back() {
        // The round trip is the whole reason both live in this file.
        for path in [
            "/usr/local/bin/yaadgaar",
            "/opt/my tools/yaadgaar",
            "/home/don't/yaadgaar",
        ] {
            let written = format!("{} hook prompt-recall", shell_quote(path));
            assert_eq!(shell_split(&written), [path, "hook", "prompt-recall"]);
        }
    }

    #[test]
    fn a_line_of_several_commands_is_several_commands() {
        // The failure this exists for: `terraform` is the command word of the
        // SECOND command here, and a matcher looking at the whole line as one
        // string cannot say that.
        let commands = shell_commands("cd infra && terraform apply");
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].words, ["cd", "infra"]);
        assert_eq!(commands[1].words, ["terraform", "apply"]);
    }

    #[test]
    fn every_separator_a_shell_honours_separates() {
        for line in [
            "cd infra; terraform",
            "cd infra && terraform",
            "cd infra || terraform",
            "cat plan | terraform",
            "cd infra\nterraform",
        ] {
            let commands = shell_commands(line);
            assert_eq!(
                commands.last().map(|c| c.words.first().map(String::as_str)),
                Some(Some("terraform")),
                "{line} did not end in a terraform command"
            );
        }
    }

    #[test]
    fn a_quoted_separator_separates_nothing() {
        // `gh pr comment --body "a; b"` is one command, and treating the `;`
        // inside a quoted argument as a separator would invent a command called
        // `b` that nobody ran.
        let commands = shell_commands(r#"gh pr comment --body "a; b""#);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].words.last().unwrap(), "a; b");
    }

    #[test]
    fn a_file_written_into_is_not_an_argument() {
        let commands = shell_commands("echo x > notes.md");
        assert_eq!(commands[0].words, ["echo", "x"]);
        assert_eq!(commands[0].writes_to, ["notes.md"]);
    }

    #[test]
    fn a_file_read_from_is_an_argument_and_not_a_write() {
        let commands = shell_commands("grep -q x < notes.md");
        assert_eq!(commands[0].writes_to, Vec::<String>::new());
        assert!(commands[0].words.contains(&"notes.md".to_string()));
    }

    #[test]
    fn an_appending_redirect_is_still_a_write() {
        let commands = shell_commands("echo x >> notes.md");
        assert_eq!(commands[0].writes_to, ["notes.md"]);
    }

    #[test]
    fn a_command_at_the_end_of_a_line_is_still_a_command() {
        // No trailing space, no trailing separator. The substring matcher this
        // replaces looked for "terraform " and this line has no space after it.
        assert_eq!(shell_commands("terraform")[0].words, ["terraform"]);
    }
}

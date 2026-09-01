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
/// Not a shell: no expansion, no operators. It exists so that identity survives
/// a quoted path — `'/opt/my tools/yadgar' hook prompt-recall` is yadgar's, and
/// a naive `split_whitespace` would call it somebody else's.
pub fn shell_split(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars();
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
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

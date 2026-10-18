//! What a simple command actually runs: the program behind the wrappers, and
//! the command substitutions inside a line.
//!
//! Split out of `guard.rs` at its size ceiling. The seam is the question: every
//! rule asks this module what runs, and none of them is answered here.

use crate::install::shellword::SimpleCommand;

/// The program a simple command runs, and the arguments it gets.
///
/// `sudo`, `env` and friends are stripped because they do not change what runs
/// — `sudo terraform apply` is terraform — and a leading `FOO=1` is an
/// assignment rather than a program.
pub(super) fn executed(command: &SimpleCommand) -> Option<(&str, &[String])> {
    let mut rest = command.words.as_slice();
    loop {
        // Environment assignments precede the program: `TF_LOG=trace terraform`.
        while rest
            .first()
            .is_some_and(|w| w.contains('=') && !w.starts_with('-'))
        {
            rest = &rest[1..];
        }
        let wrapper = program_name(rest.first()?);
        if !TRANSPARENT.contains(&wrapper) {
            return Some((wrapper, &rest[1..]));
        }
        rest = &rest[1..];
        // The wrapper's own options. A value-taking one consumes the word after
        // it, and skipping that word is the difference between reading
        // `sudo -u ops terraform` as terraform and reading it as `ops`.
        while let Some(flag) = rest.first().filter(|w| w.starts_with('-')).cloned() {
            rest = &rest[1..];
            if takes_a_value(wrapper, &flag) && !flag.contains('=') {
                rest = rest.get(1..).unwrap_or(&[]);
            }
        }
    }
}

/// The text of every outermost `$(...)` and backtick substitution in
/// *line*, quoted or not; nested ones are part of their outer text.
///
/// Inside single quotes these are literal text to a shell; they are taken
/// anyway, because reading too much here can only refuse more.
pub(super) fn substitutions(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
            let start = i + 2;
            let mut depth = 1;
            let mut j = start;
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let end = if depth == 0 { j - 1 } else { j };
            found.push(chars[start..end].iter().collect());
            // OUTERMOST ONLY, so the scan stays linear; a nested one is found
            // when its outer text is judged as a line.
            i = j;
        } else if chars[i] == '`' {
            let start = i + 1;
            let end = chars[start..]
                .iter()
                .position(|&c| c == '`')
                .map_or(chars.len(), |n| start + n);
            found.push(chars[start..end].iter().collect());
            i = end + 1;
        } else {
            i += 1;
        }
    }
    found
}

/// Does this wrapper's option consume the word after it?
///
/// Per wrapper, because the same letter means different things: `env -i`
/// ignores the environment and takes nothing, while `env -u` names a variable
/// to unset. Getting that backwards eats the program name and the guard sees
/// something harmless where terraform was about to run.
fn takes_a_value(wrapper: &str, flag: &str) -> bool {
    match wrapper {
        "sudo" | "doas" => matches!(flag, "-u" | "-g" | "-p" | "-C" | "-h" | "-r" | "-t" | "-U"),
        "env" => matches!(flag, "-u" | "-S" | "-C"),
        "nice" | "ionice" => matches!(flag, "-n" | "-c" | "-p"),
        "stdbuf" => matches!(flag, "-i" | "-o" | "-e"),
        "time" => matches!(flag, "-f" | "-o"),
        _ => false,
    }
}

/// The name a shell would resolve, without its directory or its extension.
///
/// BOTH SEPARATORS, because this client ships to Windows and
/// `C:\tools\terraform.exe` spells its path the other way.
fn program_name(word: &str) -> &str {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    base.strip_suffix(".exe").unwrap_or(base)
}

/// The value of an option given either as `-c script` or as `-cscript`.
pub(super) fn value_of<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, arg)| {
        if arg == option {
            args.get(i + 1).map(String::as_str)
        } else {
            arg.strip_prefix(option).filter(|v| !v.is_empty())
        }
    })
}

/// Wrappers that run something else without changing what it is.
const TRANSPARENT: &[&str] = &[
    "sudo", "doas", "env", "nohup", "time", "command", "nice", "ionice", "stdbuf", "setsid",
    "exec", // Multi-call binaries: `busybox cp` runs cp.
    "busybox", "toybox",
];

pub(super) const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash"];

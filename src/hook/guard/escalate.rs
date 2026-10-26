//! Protected mode's two Bash rules (ADR-1028), which replace the
//! exceptions-file text rule there.
//!
//! The protected file is root-owned, so a write as the user fails at the
//! kernel. What is left is a write AS ROOT: an agent whose session can reuse
//! a cached `sudo` timestamp. So in protected mode:
//!
//! * **Escalation, FAIL CLOSED.** Three security reviews each found a new
//!   spelling a shape-by-shape denylist missed (a glob, a case, a multi-call
//!   binary, a script on stdin). So the rule is no longer about shapes:
//!   - a line that MENTIONS an escalator anywhere ([`NAMES`], ignoring
//!     case, quotes and backslashes; `su` as a whole word) refuses, a
//!     `grep sudo` included: an agent never needs to;
//!   - a program word that may expand into one (`$S`, a glob, a brace list)
//!     refuses;
//!   - a shell or interpreter whose script the guard cannot see refuses:
//!     no script argument (stdin, a heredoc), `-s` or `-`, a `<` redirect,
//!     or the target of a pipe; inline code is refused for every program
//!     by `guard/inline.rs`.
//!
//!   The caller recurses into `sh -c`, `eval` and substitutions.
//! * **The protected path.** A line whose words may name `/etc/yaadgaar`
//!   refuses, a read included: matched literally after `//` and `/./` are
//!   collapsed, and component by component as a glob.
//!
//! **THIS IS NOT THE DEFENCE, and does not pretend to be.** A script file,
//! or a payload downloaded and run from a file, is not visible to any text
//! check. The defence is that sudo cannot be reused without a person:
//! `guard status` and `guard lock` check the sudoers timestamp settings and
//! say so loudly (`lock/sudo.rs`).

use super::runs::{executed, executed_through, program_name};
use super::tamper::{brace_expand, glob_matches};
use crate::install::shellword::{shell_commands, SimpleCommand};

/// Names whose mention anywhere in a line refuses (lower case).
const NAMES: &[&str] = &[
    "sudo",
    "doas",
    "pkexec",
    "run0",
    "systemd-run",
    "machinectl",
    "busybox",
    "toybox",
];

/// `su`, refused as a whole word only: as a substring it is in too many
/// ordinary words (`result`, `issue`) to refuse on.
const SU: &str = "su";

/// Shells and interpreters, whose script the guard must be able to see.
const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "mksh",
    "ash",
    "fish",
    "perl",
    "ruby",
    "node",
    "nodejs",
    "php",
    "lua",
    "tclsh",
    "osascript",
    "rscript",
    "pwsh",
    "powershell",
    "expect",
];

/// The protected-mode refusal for a Bash line, if any: *raw* as the
/// agent sent it, *line* with its continuations joined, scanned as
/// *commands*.
pub(super) fn refusal(raw: &str, line: &str, commands: &[SimpleCommand]) -> Option<&'static str> {
    let toward = commands.iter().any(super::deferred::changes_toward_it);
    if names_the_protected_dir(line) || toward {
        return Some(super::reasons::PROTECTED_FILE);
    }
    // A backslash-newline joins words the agent split (`s\<newline>udo`):
    // refused outright rather than modelled.
    let escalation = raw.contains("\\\n") || escalates(line, commands);
    escalation.then_some(super::reasons::ESCALATE)
}

/// Whether *line* (scanned as *commands*) escalates privilege, or may.
fn escalates(line: &str, commands: &[SimpleCommand]) -> bool {
    let plain = plain(line);
    if NAMES.iter().any(|n| plain.contains(n)) {
        return true;
    }
    let words = plain.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    if words.into_iter().any(|w| w == SU) {
        return true;
    }
    let program_may = |c: &SimpleCommand| {
        let mut hit = false;
        executed_through(c, &mut |w| hit |= may_expand_to_one(w) || is_named(w));
        hit
    };
    commands.iter().any(|c| {
        program_may(c)
            || undetermined(c)
            || hides_its_script(c)
            || super::inline::runs_inline_code(c)
            || super::deferred::defers_code(c)
            || super::deferred::replaces_tokens(c)
    }) || pipes_into_one(line)
        || redirects_into_one(line, commands)
        || super::deferred::has_brace_expansion(line)
}

/// Whether the guard cannot tell what *command* runs: a wrapper with no
/// program after it (`xargs`, `env -i`, `nice` read theirs from stdin or
/// run a default), a placeholder (`xargs -I{} {}`), or `find -exec {}`.
/// FAIL CLOSED in protected mode: what cannot be determined refuses.
fn undetermined(command: &SimpleCommand) -> bool {
    let mut seen = 0usize;
    let Some((program, args)) = executed_through(command, &mut |_| seen += 1) else {
        return seen > 0;
    };
    let placeholder = |w: &str| w.is_empty() || w.contains(['{', '$', '`']);
    // `}` alone is a group's closer; as the program of anything longer it
    // is what `{}` reads as once the scanner strips the opening brace.
    let closer = program == "}" && command.words.len() == 1;
    if placeholder(program) || (program == "}" && !closer) {
        return true;
    }
    program_name(program) == "find"
        && args.windows(2).any(|p| {
            ["-exec", "-execdir", "-ok", "-okdir"].contains(&p[0].as_str()) && placeholder(&p[1])
        })
}

/// Whether a line running a shell or interpreter holds an unquoted `<`:
/// the scanner reads `bash < f`, `bash <<EOF` and `bash <(curl …)` as a
/// script argument, but each feeds it a script the guard cannot see. An
/// interpreter reading a data file from stdin is refused too: accepted.
fn redirects_into_one(line: &str, commands: &[SimpleCommand]) -> bool {
    let runs_one = commands
        .iter()
        .filter_map(executed)
        .any(|(p, _)| is_interpreter(p));
    let bytes = line.as_bytes();
    let (mut single, mut double, mut i) = (false, false, 0);
    while runs_one && i < bytes.len() {
        match bytes[i] {
            b'\\' if !single => i += 1,
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'<' if !single && !double => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// *line* with quotes and backslashes removed, `//` and `/./` collapsed,
/// lower case.
fn plain(line: &str) -> String {
    let stripped: String = line
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    normalised(&stripped.to_ascii_lowercase())
}

/// *word* with `//` and `/./` collapsed, as path resolution reads it.
pub(super) fn normalised(word: &str) -> String {
    let mut out = word.to_string();
    loop {
        let next = out.replace("//", "/").replace("/./", "/");
        if next == out {
            return out;
        }
        out = next;
    }
}

/// Whether *word*, as a program or wrapper, names an escalator after its
/// directory, ignoring case: checked on the resolved word, not only on the
/// line's text.
fn is_named(word: &str) -> bool {
    let word = normalised(&word.to_ascii_lowercase());
    let name = program_name(&word);
    NAMES.contains(&name) || name == SU
}

/// Whether *word*, as a program or wrapper, is a glob that may match an
/// escalator (`su?o`, `s[u]do`). An expansion or a brace list is refused
/// earlier, by [`undetermined`].
fn may_expand_to_one(word: &str) -> bool {
    let word = normalised(&word.to_ascii_lowercase());
    let name = program_name(&word);
    name.contains(['*', '?', '[']) && NAMES.iter().chain(&[SU]).any(|e| glob_matches(name, e))
}

pub(super) fn is_interpreter(program: &str) -> bool {
    let name = program_name(program).to_ascii_lowercase();
    INTERPRETERS.contains(&name.as_str()) || name.starts_with("python")
}

/// Whether *command* runs a shell or interpreter on a script the guard
/// cannot read: none given (stdin, a heredoc), `-s` or `-`. An inline
/// `-c`/`-e` script is refused by the inline-code rule (`guard/inline.rs`).
fn hides_its_script(command: &SimpleCommand) -> bool {
    let Some((program, args)) = executed(command) else {
        return false;
    };
    if !is_interpreter(program) {
        return false;
    }
    if args.iter().any(|a| a == "-" || a == "-s") {
        return true;
    }
    args.iter().all(|a| a.starts_with('-'))
}

/// Whether an unquoted `|` (or `|&`) in *line* feeds a shell or an
/// interpreter: `curl … | sh`, `| env bash -s`, `| xargs sh -c …`.
fn pipes_into_one(line: &str) -> bool {
    let bytes = line.as_bytes();
    let (mut single, mut double, mut i) = (false, false, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if !single => i += 1,
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'|' if !single && !double => {
                if bytes.get(i + 1) == Some(&b'|') {
                    i += 1;
                } else {
                    let rest = line[i + 1..].trim_start_matches('&');
                    let fed = shell_commands(rest);
                    // NO PROGRAM is no answer: refuse (fail closed).
                    let fed_one = fed
                        .first()
                        .is_none_or(|c| executed(c).is_none_or(|(p, _)| is_interpreter(p)));
                    if fed_one {
                        return true;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// The protected directory as a word may spell it: the platform's
/// [`crate::hook::mode::PROTECTED_DIR`], or `/etc/yaadgaar`, which on macOS
/// reaches it through the `/etc` symlink.
pub(super) fn names_the_protected_dir(line: &str) -> bool {
    let plain: String = line
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect::<String>()
        .to_ascii_lowercase();
    let plain = normalised(&plain);
    let literal = [crate::hook::mode::PROTECTED_DIR, "/etc/yaadgaar"]
        .iter()
        .any(|dir| plain.contains(dir));
    literal || plain.split_whitespace().any(may_name_the_dir)
}

/// Whether some brace alternative of *word* has a component that may be
/// `etc` followed by one that may be `yaadgaar`, as globs, at least one of
/// the two spelling a letter of its own. A bare `*/*` is NOT read as the
/// directory: that is the #431 false positive again, and a write there as
/// the user fails at the kernel anyway.
fn may_name_the_dir(word: &str) -> bool {
    let Some(alternatives) = brace_expand(word) else {
        return true;
    };
    let lettered = |p: &str| p.chars().any(|c| c.is_ascii_alphanumeric());
    alternatives.iter().any(|alt| {
        let parts: Vec<&str> = alt.split('/').collect();
        parts.windows(2).any(|p| {
            (lettered(p[0]) || lettered(p[1]))
                && glob_matches(p[0], "etc")
                && glob_matches(p[1], "yaadgaar")
        })
    })
}

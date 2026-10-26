//! Protected mode refuses INLINE CODE (ADR-1028, fourth security review).
//!
//! Every interpreter has its own way to run a program from an argument,
//! and each review found one the escalation rule did not model (awk's
//! `system()`, gdb's `-ex`, vim's `+!cmd`, emacs's `--eval`, `env -S`). So
//! the guard stops modelling them: in protected mode, an inline-code option
//! refuses for EVERY program, wrappers included, except a short allowlist
//! of programs that cannot execute anything and for which these letters
//! mean something else (`grep -e`, `wc -c`, `git -C`). awk's program text,
//! sed's `e` command spelled with a byte escape, and vi's `+cmd` are inline
//! code too. Put the code in a file and run the file by name.
//!
//! **Best effort, and not the control.** A script file, a downloaded
//! payload, or an interpreter this list does not know is not visible to it.
//! The control is ownership plus sudo settings (`lock/sudo.rs`).

use super::runs::{executed, program_name};
use crate::install::shellword::SimpleCommand;

/// Options that hand a program code to run, compared exactly.
const INLINE: &[&str] = &[
    "-c",
    "-e",
    "-E",
    "-S",
    "-ex",
    "--eval",
    "--cmd",
    "--command",
    "--eval-command",
    "--exec",
    "--split-string",
    "-Command",
    "-command",
    "-EncodedCommand",
    "-encodedcommand",
    "-ec",
];

/// Long inline options that may carry their value after `=`.
const INLINE_LONG: &[&str] = &[
    "--eval=",
    "--cmd=",
    "--command=",
    "--eval-command=",
    "--exec=",
    "--split-string=",
];

/// Programs that execute nothing, for which [`INLINE`]'s letters are data.
const NO_EXEC: &[&str] = &[
    "grep", "egrep", "fgrep", "rg", "wc", "head", "tail", "cut", "uniq", "jq", "ls", "diff", "cmp",
    "tr", "stat", "du", "date", "printf", "echo", "cat", "set", "test", "[", "[[", "ulimit",
];

/// Programs whose first operand is a program: awk's text.
const AWKS: &[&str] = &["awk", "gawk", "mawk", "nawk"];

/// vi's family, which runs `+cmd` as an ex command.
const VIS: &[&str] = &["vi", "vim", "nvim", "ex", "view", "gvim", "rvim", "vimdiff"];

/// Whether *command* hands any program inline code.
pub(super) fn runs_inline_code(command: &SimpleCommand) -> bool {
    // No program after the wrappers (`env -S '…'` consumes its own code) is
    // refused as undetermined (`guard/escalate.rs`).
    let Some((program, args)) = executed(command) else {
        return false;
    };
    let name = program_name(program).to_ascii_lowercase();
    let name = name.as_str();
    // The wrappers before the program are judged too: `env -S '…'`.
    let at = command.words.len() - args.len();
    let wrappers = &command.words[..at.saturating_sub(1)];
    if wrappers.iter().any(|w| is_inline(w)) {
        return true;
    }
    if AWKS.contains(&name) {
        return !args.iter().any(|a| a == "-f" || a.starts_with("--file"));
    }
    if VIS.contains(&name) && args.iter().any(|a| a.starts_with('+')) {
        return true;
    }
    if name == "sed" {
        return args.iter().any(|a| has_a_byte_escape(a));
    }
    if NO_EXEC.contains(&name) {
        return false;
    }
    let interpreter = super::escalate::is_interpreter(program);
    args.iter()
        .any(|a| is_inline(a) || (interpreter && carries_a_code_flag(a)))
}

/// Whether short-option cluster *word* carries `c`, `e` or `E` anywhere:
/// `-xc`, `-le`, `-Sc`, `-cpass`. FAIL CLOSED: a letter that is really an
/// option's value is still read as the flag.
fn carries_a_code_flag(word: &str) -> bool {
    let Some(cluster) = word.strip_prefix('-').filter(|c| !c.starts_with('-')) else {
        return false;
    };
    cluster
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .any(|c| matches!(c, 'c' | 'e' | 'E'))
}

fn is_inline(word: &str) -> bool {
    INLINE.contains(&word) || INLINE_LONG.iter().any(|o| word.starts_with(o))
}

/// A byte escape (backslash then `x`, `o`, `d`, `c` or a digit): sed's way
/// to spell a command no text check reads.
fn has_a_byte_escape(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|w| w[0] == b'\\' && matches!(w[1], b'x' | b'o' | b'd' | b'c' | b'0'..=b'9'))
}

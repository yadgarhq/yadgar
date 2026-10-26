//! Protected mode, last text-layer round (ADR-1028, sixth security review):
//! brace expansion, code stored to run later, replacement tokens and a cd
//! toward the protected directory.
//! All FAIL CLOSED; all best effort. The control is ownership plus sudo
//! settings (`lock/sudo.rs`); further text-layer gaps are a follow-up.

use super::runs::{executed, program_name};
use crate::install::shellword::SimpleCommand;

/// Whether *line* holds an unquoted brace expansion, `{a,b}` or `{x..y}`.
/// Not expanded: the guard does not model what it produces. `${…}` is a
/// parameter expansion and `{}` a placeholder, so neither counts here.
pub(super) fn has_brace_expansion(line: &str) -> bool {
    let bytes = line.as_bytes();
    let (mut single, mut double, mut i) = (false, false, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if !single => i += 1,
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'{' if !single && !double && (i == 0 || bytes[i - 1] != b'$') => {
                let inner: Vec<u8> = bytes[i + 1..]
                    .iter()
                    .take_while(|&&b| b != b'}' && !b.is_ascii_whitespace())
                    .copied()
                    .collect();
                let closed = bytes.get(i + 1 + inner.len()) == Some(&b'}');
                let listy = inner.contains(&b',') || inner.windows(2).any(|w| w == b"..");
                if closed && listy {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Variables whose value a shell later runs or sources.
const DEFERRED_VARS: &[&str] = &[
    "PROMPT_COMMAND",
    "BASH_ENV",
    "ENV",
    "PS0",
    "PS1",
    "PS2",
    "PS3",
    "PS4",
];

/// git keys whose value git later runs, by prefix or by suffix.
const GIT_KEY_PREFIXES: &[&str] = &[
    "alias.",
    "core.pager",
    "core.editor",
    "core.sshcommand",
    "core.hookspath",
    "core.fsmonitor",
    "credential.helper",
    "diff.external",
    "sequence.editor",
    "gpg.program",
    "pager.",
];
const GIT_KEY_SUFFIXES: &[&str] = &[
    ".cmd",
    ".command",
    ".driver",
    ".smudge",
    ".clean",
    ".process",
    ".textconv",
    ".helper",
    ".program",
];

/// Whether *command* stores code that runs later: `trap`, `bind -x`,
/// `complete -C`, an assignment to a variable a shell runs, or a git key
/// whose value git runs.
pub(super) fn defers_code(command: &SimpleCommand) -> bool {
    let assigns = command.words.iter().any(|w| {
        w.split_once('=')
            .is_some_and(|(name, _)| DEFERRED_VARS.contains(&name))
    });
    if assigns {
        return true;
    }
    let Some((program, args)) = executed(command) else {
        return false;
    };
    match program_name(program) {
        "trap" => true,
        "bind" => args.iter().any(|a| a == "-x"),
        "complete" | "compgen" => args.iter().any(|a| a == "-C"),
        "git" => args.iter().any(|a| {
            let key = a.split('=').next().unwrap_or(a).to_ascii_lowercase();
            GIT_KEY_PREFIXES.iter().any(|p| key.starts_with(p))
                || (key.contains('.') && GIT_KEY_SUFFIXES.iter().any(|s| key.ends_with(s)))
        }),
        _ => false,
    }
}

/// Whether *command* runs a program the guard cannot name in advance
/// through a replacement token: `xargs` given `-I`, `-i`, `-J` or
/// `--replace` (in any short cluster), or `parallel` at all.
pub(super) fn replaces_tokens(command: &SimpleCommand) -> bool {
    let mut names: Vec<usize> = Vec::new();
    let words = &command.words;
    let at = |w: &str| {
        let p = w.as_ptr() as usize;
        words.iter().position(|x| {
            let start = x.as_ptr() as usize;
            (start..=start + x.len()).contains(&p)
        })
    };
    let executed = super::runs::executed_through(command, &mut |w| names.extend(at(w)));
    if executed.is_some_and(|(p, _)| program_name(p).eq_ignore_ascii_case("parallel")) {
        return true;
    }
    names.iter().enumerate().any(|(k, &i)| {
        if !program_name(&words[i]).eq_ignore_ascii_case("xargs") {
            return false;
        }
        let end = names.get(k + 1).copied().unwrap_or(words.len());
        words[i + 1..end].iter().any(|o| {
            o.starts_with("--replace")
                || o.strip_prefix('-')
                    .filter(|c| !c.starts_with('-'))
                    .is_some_and(|c| c.contains(['I', 'i', 'J']))
        })
    })
}

/// Whether *command* is a `cd`, `pushd` or `popd` toward the protected
/// directory: into `/etc`, or through a `yaadgaar` path component.
pub(super) fn changes_toward_it(command: &SimpleCommand) -> bool {
    let Some((program, args)) = executed(command) else {
        return false;
    };
    if !matches!(program_name(program), "cd" | "pushd" | "popd") {
        return false;
    }
    args.iter().any(|a| {
        let path = super::escalate::normalised(&a.to_ascii_lowercase());
        let path = path.trim_end_matches('/');
        path == "/etc" || path.split('/').any(|c| c == "yaadgaar")
    })
}

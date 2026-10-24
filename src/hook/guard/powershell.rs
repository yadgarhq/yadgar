//! The PowerShell tool (yaadgaar #227, ADR-1005): judged at least as
//! strictly as Bash.
//!
//! PowerShell is not parsed. Its text is turned into a line the Bash rules
//! can read, and judged by every one of them; and on top of that, any token
//! that names a terraform binary refuses, wherever it stands. That is
//! stricter than Bash on purpose: PowerShell reaches a program through more
//! routes than this guard could list (`&`, `Start-Process`, `iex`,
//! `cmd /c`, `wsl`, a variable holding the name), and every one of them
//! leaves the name in the text.
//!
//! * A backtick is PowerShell's escape character, so backticks are dropped:
//!   `` exc`eptions `` reads `exceptions`.
//! * A backslash is a path separator, not an escape, so it reads as `/`.
//! * A command whose text is hidden from the guard — `-EncodedCommand` (or
//!   any prefix PowerShell accepts for it) handed to `powershell` or `pwsh`,
//!   or a base64 decode — refuses, because nothing in it can be judged.
//!
//! **WHAT THIS DOES NOT SEE**: a name built at run time from pieces
//! (`'terra' + 'form'`, `-join`, `[char]` codes), exactly as Bash's
//! `eval "$CMD"` is not seen.

use super::reasons::{PS_UNJUDGEABLE, SHADOW, TERRAFORM};
use super::{any_rule_on, bash, Decision, GuardEnv, Policy, TERRAFORM_BINARIES};

/// Judge one PowerShell command.
pub(super) fn judge(command: &str, policy: &Policy, env: &GuardEnv) -> Decision {
    if any_rule_on(policy) && hides_its_text(command) {
        return Decision::Deny(PS_UNJUDGEABLE.into());
    }
    if policy.hook_config_tamper && shadows_gh(command) {
        return Decision::Deny(SHADOW.into());
    }
    let plain = as_bash_text(command);
    if policy.terraform_family && names_a_terraform_binary(&plain) {
        return Decision::Deny(TERRAFORM.into());
    }
    bash(&plain, 0, policy, env)
}

/// *command* as a line the Bash rules read: backticks dropped, backslashes
/// read as `/`.
fn as_bash_text(command: &str) -> String {
    command
        .chars()
        .filter(|&c| c != '`')
        .map(|c| if c == '\\' { '/' } else { c })
        .collect()
}

/// Whether any token of *text* ends in a terraform binary, ignoring case,
/// its directory and an executable suffix.
fn names_a_terraform_binary(text: &str) -> bool {
    text.to_ascii_lowercase()
        .split(|c: char| c.is_whitespace() || ";|&(){}[]'\",=+@$<>!#".contains(c))
        // The last path component: `C:/tools/terraform.exe` names the binary,
        // `terraform/main.tf` names a file.
        .map(|t| t.rsplit(['/', ':']).next().unwrap_or(t))
        .map(|t| {
            [".exe", ".cmd", ".bat", ".ps1", ".com"]
                .iter()
                .find_map(|s| t.strip_suffix(s))
                .unwrap_or(t)
        })
        .any(|t| TERRAFORM_BINARIES.contains(&t))
}

/// Whether *command* hands PowerShell a script the guard cannot read.
fn hides_its_text(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    if lower.contains("frombase64string") {
        return true;
    }
    let words: Vec<&str> = lower
        .split_whitespace()
        .map(|w| w.trim_matches(['\'', '"']))
        .collect();
    let launches = words.iter().any(|w| {
        let name = w.rsplit(['/', '\\']).next().unwrap_or(w);
        let name = name.strip_suffix(".exe").unwrap_or(name);
        matches!(name, "powershell" | "pwsh")
    });
    launches
        && words.iter().any(|w| {
            w.strip_prefix('-')
                .or_else(|| w.strip_prefix('/'))
                .is_some_and(|f| {
                    !f.is_empty()
                        && ("encodedcommand".starts_with(f)
                            || "encodedarguments".starts_with(f)
                            || matches!(f, "ec" | "ea"))
                })
        })
}

/// The PowerShell spellings of shadowing gh (ADR-1020), which the Bash
/// reading does not see: an alias cmdlet naming gh, an assignment to
/// `$env:PATH`, and `SetEnvironmentVariable` on `Path`. A `function gh`
/// reads as Bash and is caught there.
fn shadows_gh(command: &str) -> bool {
    let plain: String = command
        .chars()
        .filter(|c| !matches!(c, '`' | '\'' | '"'))
        .collect::<String>()
        .to_ascii_lowercase();
    let tokens: Vec<&str> = plain
        .split(|c: char| c.is_whitespace() || c == ';' || c == '|')
        .filter(|t| !t.is_empty())
        .collect();
    let aliases = tokens.iter().enumerate().any(|(i, t)| {
        matches!(*t, "set-alias" | "new-alias" | "sal" | "nal")
            && tokens[i + 1..]
                .iter()
                .any(|a| matches!(*a, "gh" | "gh-personal"))
    });
    let assigns = plain.match_indices("$env:path").any(|(at, m)| {
        let rest = &plain[at + m.len()..];
        let op = rest.trim_start();
        !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            && (op.starts_with('=') || op.starts_with("+="))
            && !op.starts_with("==")
    });
    let sets = plain
        .match_indices("setenvironmentvariable")
        .any(|(at, m)| {
            let rest = plain[at + m.len()..].trim_start();
            rest.strip_prefix('(').is_some_and(|r| {
                let r = r.trim_start();
                r.starts_with("path") && !r[4..].starts_with(|c: char| c.is_ascii_alphanumeric())
            })
        });
    aliases || assigns || sets
}

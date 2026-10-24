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

use std::path::Path;

use super::reasons::{PS_UNJUDGEABLE, TERRAFORM};
use super::{any_rule_on, bash, Decision, Policy, TERRAFORM_BINARIES};

/// Judge one PowerShell command.
pub(super) fn judge(command: &str, policy: &Policy, config_dir: Option<&Path>) -> Decision {
    if any_rule_on(policy) && hides_its_text(command) {
        return Decision::Deny(PS_UNJUDGEABLE.into());
    }
    let plain = as_bash_text(command);
    if policy.terraform_family && names_a_terraform_binary(&plain) {
        return Decision::Deny(TERRAFORM.into());
    }
    bash(&plain, 0, policy, config_dir)
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

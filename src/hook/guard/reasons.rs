//! The reasons, which a person reads.
//!
//! Split out of `guard.rs` at its size ceiling. Each rule's reason names the
//! rule and the ADR that sets its scope (ADR-1005), so an agent that is
//! refused can find the decision rather than guess at it.

/// The longest command judged, in bytes. A BOUND, not a knob: a real
/// command is a few hundred bytes, and one this long is refused rather than
/// risk the hook timing out.
pub(super) const MAX_COMMAND_BYTES: usize = 64 * 1024; // ADR-0569-EXCEPTION(CB): bound on parsing untrusted input, not a per-install knob.

pub(super) const TOO_LONG: &str = "refused: this command is too long for the guard to check \
     in time, so it was not checked. Split it, or put it in a script file.";

pub(super) const UNREADABLE: &str =
    "refused: the hook payload is not a tool call this guard can read \
     (no string `tool_name`, or a Bash or PowerShell call with no string `command`), so this \
     tool call was not checked. Retry it; if this repeats, run `yaadgaar verify`.";

pub(super) const UNPLACED: &str =
    "refused: this edit carries no string `file_path` or `notebook_path`, \
     so the guard cannot tell which file it changes, and it was not checked. \
     Retry it; if this repeats, run `yaadgaar verify`.";

pub(super) const HOOK_BYPASS: &str = "Refused under the No Hook Bypass rule (ADR-1005). \
     A commit that skips hooks is a commit whose checks did not run. \
     Hook failure is signal, not an obstacle — fix the cause and re-run.";

pub(super) const TERRAFORM: &str = "Refused under the No Terraform rule (ADR-1005). \
     terraform, tofu and tfp must never execute here, by any mechanism — \
     including a wrapper such as timeout or xargs, a fresh container, a nix run, \
     or a CI trigger. Write the command into MIGRATION_NOTES.md and hand it over instead.";

pub(super) const DIGGER: &str = "Refused under the No Terraform rule, digger clause (ADR-1005). \
     A digger comment runs terraform against remote state through the \
     orchestrator. This is refused even when explicitly instructed: put the \
     comment in MIGRATION_NOTES.md for a human to post.";

pub(super) const EXCEPTIONS: &str = "Refused under the exceptions-file tamper rule \
     (ADR-0971, ADR-1005). The hook exceptions file is a durable, human-only decision. An agent \
     once edited it to allow its own push, then reverted the file to conceal \
     that. So the guard refuses any Bash command naming the file, a read \
     included, and any edit of it. To read it, use the Read tool.";

pub(super) const PROTECTED: &str = "Refused under the guard-registration tamper rule (ADR-1005). \
     Claude Code settings files (`.claude/settings.json`, `.claude/settings.local.json`) \
     and `.git/hooks` register and run the hooks that enforce the standing rules, so an \
     agent may not edit, overwrite, move, link or delete them. Reading them (the Read tool, \
     `cat`, `jq`, `grep`) is allowed. Settings and hook changes come from nix or \
     `yaadgaar install`: write the change into MIGRATION_NOTES.md for a person.";

pub(super) const TOO_DEEP: &str = "Refused under the nesting rule (ADR-1005). This command \
     nests shells, `eval`, `env -S` or command substitutions more levels deep than the \
     guard reads, so what runs at the bottom was not checked. Flatten it, or put it in \
     a script file.";

pub(super) const PS_UNJUDGEABLE: &str = "Refused under the PowerShell rule (ADR-1005). This \
     PowerShell command hides its text from the guard (`-EncodedCommand` or a base64 \
     decode), so it was not checked. Pass the command as plain text.";

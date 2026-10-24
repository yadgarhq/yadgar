//! The standing refusals: what a tool call may not be allowed to do.
//!
//! Each rule exists because something happened, and each is written as a
//! question about the CALL rather than about its text.
//!
//! ## Position, not substring
//!
//! The matcher this replaces asked `haystack.contains(needle)` and therefore
//! answered two different questions the same way. It refused
//! `grep -r 'terraform ' docs/`, which runs grep. It refused a `Write` of
//! `MIGRATION_NOTES.md` containing the words `digger apply` — which is precisely
//! what the standing rule instructs a person to do instead of posting one, so
//! the guard forbade its own remedy. And it allowed
//! `git commit -n -m x`, `git -c core.hooksPath=/dev/null commit`,
//! `cd infra && terraform`, a bare `terraform` at the end of a line,
//! `docker exec tf-local-plan tofu`, and
//! `gh pr comment 12 --body "digger unlock"`.
//!
//! So every rule below asks one of two questions, and never confuses them:
//!
//! * **Does this command EXECUTE X?** — decided on the resolved command word of
//!   each simple command in the line, after the wrappers that do not change what
//!   runs are stripped off. `terraform` in argument position is a file name.
//! * **Does this text MENTION X?** — asked only where mentioning is the harm.
//!   Posting `digger apply` as a comment runs terraform against remote state
//!   through the orchestrator, so there the text IS the act; but only when
//!   something in the line posts it.
//!
//! ## What this does not decide, and will not pretend to
//!
//! * `bash deploy.sh`, where the script runs terraform. Undecidable from the
//!   command line, and no amount of matching changes that. A `-c` script is
//!   read, because it is present in the command; a file is not.
//! * `gh pr comment --body-file MIGRATION_NOTES.md`, where the digger command is
//!   in a file this never opens. Reading the file would make the guard depend on
//!   the working directory at hook time.
//! * `make apply`, `just plan`, a shell alias, `eval "$CMD"`. All reach the same
//!   binaries through a name this cannot resolve.
//!
//! Everything in that list fails OPEN for the terraform, commit and digger
//! rules: this runs on the critical path of somebody's session, and a guard
//! that refuses what it cannot understand is a guard that gets switched off.
//!
//! **What the guard reads, it reads to the end** (ADR-1005, yaadgaar #229):
//! a wrapper with an operand (`timeout 5`, `xargs -n1`) is read through to
//! the program it runs, and nesting past [`MAX_NESTING`] refuses instead of
//! being skipped. `command -v terraform` is refused with it, a known
//! over-refusal: `command` is a wrapper.
//!
//! **PowerShell and MultiEdit** are judged too (#227): see
//! `guard/powershell.rs`. **The guard's own registration** — Claude Code
//! settings files and `.git/hooks` — is protected from agent edits and Bash
//! writes (#240): see `guard/tamper/protected.rs`.
//!
//! **The exceptions file is the exception** (#64): any Bash command that names
//! it refuses, read or write, because every attempt to let a read through was
//! bypassed. `Edit`, `Write` and `NotebookEdit` on that path are exact. What a
//! text check cannot see is listed in `guard/tamper.rs`.

use serde_json::Value;

use std::path::Path;

use super::{Decision, Policy};
use crate::install::shellword::{shell_commands, SimpleCommand};

mod powershell;
mod reasons;
mod runs;
mod tamper;
use reasons::*;
use runs::{
    executed, long_takes_a_value, shell_scripts, substitution_depth, substitutions, SHELLS,
};
use tamper::{
    bash_touches_a_protected_path, is_protected_path, is_the_exceptions_file,
    names_the_exceptions_file,
};

/// Refuse a tool call that the standing rules forbid.
///
/// The matcher covers Bash AND the editing tools, and that breadth was earned:
/// an agent once used Edit rather than Bash to add itself to the push
/// allowlist, pushed to master, then reverted the file to conceal it — and a
/// Bash-only matcher never even routed the call to the guard.
///
/// *config_dir* is the `CLAUDE_CONFIG_DIR` the hook process received, whose
/// settings files are protected like `~/.claude`'s. A PARAMETER, not read
/// here, so every path is reachable from a test.
pub fn pre_tool_guard(payload: &Value, policy: &Policy, config_dir: Option<&Path>) -> Decision {
    // A PAYLOAD OF THE WRONG SHAPE REFUSES (ledger 1589). It used to read as
    // tool "" with command "", which allows: JSON that is not a tool call is a
    // call the guard cannot judge, exactly like JSON that does not parse.
    let Some(tool) = payload.get("tool_name").and_then(Value::as_str) else {
        return Decision::Deny(UNREADABLE.into());
    };
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    let shell = matches!(tool, "Bash" | "PowerShell");
    if shell && !input.get("command").is_some_and(Value::is_string) {
        return Decision::Deny(UNREADABLE.into());
    }
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    match tool {
        // TOO LONG TO CHECK QUICKLY refuses: a hook that times out is no
        // decision at all, so the call would run unchecked.
        _ if shell && field("command").len() > MAX_COMMAND_BYTES => Decision::Deny(TOO_LONG.into()),
        "Bash" => bash(&field("command"), 0, policy, config_dir),
        "PowerShell" => powershell::judge(&field("command"), policy, config_dir),

        // ONLY THE PATH. An edit executes nothing, so its CONTENT is text a
        // person wrote and not a command anybody ran. Reading the content is
        // what made the guard refuse writing a digger command into
        // MIGRATION_NOTES.md — the exact action the rule it enforces asks for.
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            // AN EDIT THE GUARD CANNOT PLACE REFUSES: no string `file_path` or
            // `notebook_path` used to read as "" and allow.
            let placed = ["file_path", "notebook_path"]
                .iter()
                .any(|k| input.get(*k).is_some_and(Value::is_string));
            if !placed {
                return Decision::Deny(UNPLACED.into());
            }
            let path = [field("file_path"), field("notebook_path")].join("\n");
            if policy.hook_config_tamper && is_the_exceptions_file(&path) {
                return Decision::Deny(EXCEPTIONS.into());
            }
            if policy.hook_config_tamper && is_protected_path(&path, config_dir) {
                return Decision::Deny(PROTECTED.into());
            }
            Decision::Allow
        }

        _ => Decision::Allow,
    }
}

/// The value of `env -S` / `env --split-string`, when *words* carry one.
fn env_split_string(words: &[String]) -> Option<&str> {
    let at = words.iter().position(|w| w == "env")?;
    let mut rest = words[at + 1..].iter();
    while let Some(word) = rest.next() {
        // `getopt`-style, as `runs.rs` reads wrappers: `-iS 'x'` and
        // `-vS'x'` close a cluster with `-S`, `-u NAME` and `-C DIR` take a
        // value, and a long option may be any prefix of its name.
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = long.split_once('=').unzip();
            let name = name.unwrap_or(long);
            if !name.is_empty() && "split-string".starts_with(name) {
                return value.or_else(|| rest.next().map(String::as_str));
            }
            if long_takes_a_value(word, &["--unset", "--chdir"]) {
                rest.next();
            }
            continue;
        }
        let flags = word.strip_prefix('-')?;
        let Some((at, letter)) = flags.char_indices().find(|(_, l)| "SuC".contains(*l)) else {
            continue;
        };
        let attached = &flags[at + letter.len_utf8()..];
        let value = if attached.is_empty() {
            rest.next().map(String::as_str)
        } else {
            Some(attached)
        };
        if letter == 'S' {
            return value;
        }
    }
    None
}

/// How many nested `-c` scripts to follow.
///
/// Bounded rather than trusted: a payload can nest `sh -c` inside `sh -c` as
/// deeply as it likes, and a guard that recurses on it hangs the session it
/// exists to protect. Past the bound the guard REFUSES (ADR-1005): it used to
/// skip what it did not read, which let `sh -c` three deep run terraform.
/// The exceptions-file rule needs no recursion: it refuses on the whole
/// line's text.
const MAX_NESTING: usize = 3; // ADR-0569-EXCEPTION(CB): bound on recursive -c unwrapping, not a per-install knob.

/// Whether any refusal is switched on at all.
fn any_rule_on(policy: &Policy) -> bool {
    policy.terraform_family || policy.git_commit_flags || policy.hook_config_tamper
}

/// The refusal for *script*, run one level below *depth*: its own verdict,
/// or [`TOO_DEEP`] when that level is past [`MAX_NESTING`].
fn nested(
    script: &str,
    depth: usize,
    policy: &Policy,
    config_dir: Option<&Path>,
) -> Option<String> {
    if depth >= MAX_NESTING {
        let unread = !script.trim().is_empty() && any_rule_on(policy);
        return unread.then(|| TOO_DEEP.to_string());
    }
    match bash(script, depth + 1, policy, config_dir) {
        Decision::Deny(reason) => Some(reason),
        Decision::Allow => None,
    }
}

fn bash(line: &str, depth: usize, policy: &Policy, config_dir: Option<&Path>) -> Decision {
    // A BACKSLASH-NEWLINE IS A LINE CONTINUATION: the shell deletes the pair
    // before it reads words, so `exc\<newline>eptions` is one word. Removed
    // here, before any check reads the line.
    let joined = line.replace("\\\n", "");
    let line = joined.as_str();
    let commands = shell_commands(line);
    let inner = substitutions(line);

    // THE EXCEPTIONS FILE: ANY MENTION REFUSES, READ OR WRITE. A text
    // denylist cannot safely let through a command that names the file:
    // every allowance (a `cp` from it, a read with `cat`) was a parsing
    // question the shell could answer differently. See `tamper.rs`.
    if policy.hook_config_tamper {
        if let Some(why) = names_the_exceptions_file(line, &commands) {
            return Decision::Deny(format!("{EXCEPTIONS} {why}"));
        }
    }
    if policy.hook_config_tamper && bash_touches_a_protected_path(line, &commands, config_dir) {
        return Decision::Deny(PROTECTED.into());
    }

    // Asked over the WHOLE line, because this is the one rule where mentioning
    // is the act. It is still gated on something in the line doing the posting.
    // Each rule is skipped only when the exceptions file switched its id off;
    // see `policy.rs` for which id covers which rule.
    if policy.terraform_family && posts_a_digger_command(line, &commands) {
        return Decision::Deny(DIGGER.into());
    }

    // A command substitution runs a command. The scanner reads an unquoted
    // `$(...)` as though typed in place, but not a quoted one or a backtick
    // pair, so each is judged here as its own line (security review of the
    // 1590 change): `echo "$(cp x ~/.claude/<file>)"` writes the file.
    // Substitutions nested past the bound refuse here, in one pass, rather
    // than after every level above them has been scanned.
    if any_rule_on(policy) && depth + substitution_depth(line) > MAX_NESTING {
        return Decision::Deny(TOO_DEEP.into());
    }
    for inner in &inner {
        if let Some(reason) = nested(inner, depth, policy, config_dir) {
            return Decision::Deny(reason);
        }
    }

    for command in &commands {
        // BEFORE resolving the program: `env -S '<command>'` resolves to no
        // program at all, and used to be skipped as allowed.
        if let Some(split) = env_split_string(&command.words) {
            if let Some(reason) = nested(split, depth, policy, config_dir) {
                return Decision::Deny(reason);
            }
        }
        let Some((program, args)) = executed(command) else {
            continue;
        };
        if policy.terraform_family && runs_terraform(program, args) {
            return Decision::Deny(TERRAFORM.into());
        }
        if policy.git_commit_flags && skips_hooks(program, args) {
            return Decision::Deny(HOOK_BYPASS.into());
        }
        // `eval` runs its arguments as a line, exactly as `sh -c` runs its
        // script.
        if program == "eval" {
            if let Some(reason) = nested(&args.join(" "), depth, policy, config_dir) {
                return Decision::Deny(reason);
            }
        }
        // A `-c` script is part of the command, so it is read. The rule is about
        // what runs, and `sh -c 'terraform apply'` runs terraform.
        if SHELLS.contains(&program) {
            for script in shell_scripts(args) {
                if let Some(reason) = nested(script, depth, policy, config_dir) {
                    return Decision::Deny(reason);
                }
            }
        }
    }
    Decision::Allow
}

// ── terraform, by any route ──────────────────────────────────────────────────

const TERRAFORM_BINARIES: &[&str] = &["terraform", "tofu", "tfp"];
const CONTAINER_TOOLS: &[&str] = &["docker", "podman", "nerdctl", "kubectl", "ctr", "crictl"];
const NIX_TOOLS: &[&str] = &["nix", "nix-shell", "nix-build"];
/// The plan container. Executing anything inside it is executing `tfp`.
const PLAN_CONTAINER: &str = "tf-local-plan";

fn runs_terraform(program: &str, args: &[String]) -> bool {
    if TERRAFORM_BINARIES.contains(&program) {
        return true;
    }
    // A container or a nix invocation reaches the same binary. The rule is about
    // what runs, not about which word was typed.
    if CONTAINER_TOOLS.contains(&program) {
        let enters = args
            .iter()
            .any(|a| matches!(a.as_str(), "exec" | "run" | "create" | "start" | "attach"));
        let target = args
            .iter()
            .any(|a| names_terraform(a) || a.contains(PLAN_CONTAINER));
        if enters && target {
            return true;
        }
    }
    if NIX_TOOLS.contains(&program) && args.iter().any(|a| names_terraform(a)) {
        return true;
    }
    false
}

/// Does this argument NAME a terraform binary, rather than contain the letters?
///
/// Split on the separators an image reference, a flake reference and a path use,
/// so `hashicorp/terraform:1.5` and `nixpkgs#terraform` match while
/// `docs/terraform-notes.md` and `terraforming` do not.
fn names_terraform(arg: &str) -> bool {
    arg.split(['/', '#', ':', '=', '\\', '@'])
        .any(|part| TERRAFORM_BINARIES.contains(&part))
}

// ── commits whose checks did not run ─────────────────────────────────────────

fn skips_hooks(program: &str, args: &[String]) -> bool {
    if program != "git" {
        return false;
    }
    if args
        .iter()
        .any(|a| a == "--no-verify" || a == "--no-gpg-sign")
    {
        return true;
    }
    if args.iter().enumerate().any(|(i, a)| {
        let setting = if a == "-c" {
            args.get(i + 1).map(String::as_str)
        } else {
            a.strip_prefix("-c").filter(|v| !v.is_empty())
        };
        setting.is_some_and(disables_hooks)
    }) {
        return true;
    }
    // `-n` IS `--no-verify` for `git commit`, and it is `--dry-run` for
    // `git push`. So the letter is read only where it means the dangerous thing.
    matches!(git_subcommand(args), Some(("commit", rest)) if rest.iter().any(|a| carries_no_verify(a)))
}

/// A one-invocation config override that turns the checks off.
///
/// Worse than the flag, not better: nothing in the repository records that hooks
/// were skipped, because the setting never lands in a config file.
fn disables_hooks(setting: &str) -> bool {
    let setting = setting.to_ascii_lowercase();
    if setting.starts_with("core.hookspath=") {
        return true;
    }
    matches!(
        setting.strip_prefix("commit.gpgsign="),
        Some("false" | "0" | "no" | "off")
    )
}

/// The subcommand, and everything after it.
///
/// git's own options come BEFORE the subcommand and some of them take a value,
/// so `git -c x=y commit` has `commit` as its third word and `git log -n 5` has
/// no `commit` at all.
fn git_subcommand(args: &[String]) -> Option<(&str, &[String])> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if !arg.starts_with('-') {
            return Some((arg.as_str(), &args[i + 1..]));
        }
        // The global options that consume the next word.
        let takes_a_value = matches!(
            arg.as_str(),
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path"
        );
        i += if takes_a_value { 2 } else { 1 };
    }
    None
}

/// Does this short-option cluster carry `-n`?
///
/// `git commit -nm x` is `--no-verify` with a message. `git commit -mn x` is a
/// message that happens to be the letter n — the scan stops at the first option
/// that takes a value, because everything after it in the token IS that value.
fn carries_no_verify(token: &str) -> bool {
    let Some(letters) = token.strip_prefix('-') else {
        return false;
    };
    if letters.starts_with('-') {
        return false;
    }
    for c in letters.chars() {
        if c == 'n' {
            return true;
        }
        // `git commit`'s short options that swallow the rest of the token.
        if "mcCFutS".contains(c) || !c.is_ascii_alphabetic() {
            return false;
        }
    }
    false
}

// ── a digger comment ─────────────────────────────────────────────────────────

/// Every verb the standing rule names.
///
/// `unlock` and `lock` are here because the guard this replaces listed only
/// apply, plan and destroy — and the rule has always named five.
const DIGGER_VERBS: &[&str] = &["apply", "plan", "destroy", "unlock", "lock"];
const FORGE_CLIENTS: &[&str] = &["gh", "glab", "hub", "curl", "wget", "http", "httpie"];

fn posts_a_digger_command(line: &str, commands: &[SimpleCommand]) -> bool {
    let mut runs_digger = false;
    let mut posts = false;
    for (program, args) in commands.iter().filter_map(executed) {
        if program == "digger" && args.iter().any(|a| DIGGER_VERBS.contains(&a.as_str())) {
            runs_digger = true;
        }
        if FORGE_CLIENTS.contains(&program) {
            posts = true;
        }
    }
    runs_digger || (posts && mentions_a_digger_command(line))
}

/// Two adjacent words: `digger` and one of its verbs.
///
/// Word-wise rather than `contains`, so it survives the quoting a comment body
/// arrives in and does not fire on `digger-notes.md`.
fn mentions_a_digger_command(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    words
        .windows(2)
        .any(|pair| pair[0] == "digger" && DIGGER_VERBS.contains(&pair[1].as_str()))
}

#[cfg(test)]
mod tests;

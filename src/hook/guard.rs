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
//! **The exceptions file is the exception** (#64): any Bash command that names
//! it refuses, read or write, because every attempt to let a read through was
//! bypassed. `Edit`, `Write` and `NotebookEdit` on that path are exact. What a
//! text check cannot see is listed in `guard/tamper.rs`.

use serde_json::Value;

use super::{Decision, Policy};
use crate::install::shellword::{shell_commands, SimpleCommand};

mod runs;
mod tamper;
use runs::{executed, substitutions, value_of, SHELLS};
use tamper::{is_the_exceptions_file, names_the_exceptions_file};

/// Refuse a tool call that the standing rules forbid.
///
/// The matcher covers Bash AND the editing tools, and that breadth was earned:
/// an agent once used Edit rather than Bash to add itself to the push
/// allowlist, pushed to master, then reverted the file to conceal it — and a
/// Bash-only matcher never even routed the call to the guard.
pub fn pre_tool_guard(payload: &Value, policy: &Policy) -> Decision {
    // A PAYLOAD OF THE WRONG SHAPE REFUSES (ledger 1589). It used to read as
    // tool "" with command "", which allows: JSON that is not a tool call is a
    // call the guard cannot judge, exactly like JSON that does not parse.
    let Some(tool) = payload.get("tool_name").and_then(Value::as_str) else {
        return Decision::Deny(UNREADABLE.into());
    };
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    if tool == "Bash" && !input.get("command").is_some_and(Value::is_string) {
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
        "Bash" if field("command").len() > MAX_COMMAND_BYTES => Decision::Deny(TOO_LONG.into()),
        "Bash" => bash(&field("command"), 0, policy),

        // ONLY THE PATH. An edit executes nothing, so its CONTENT is text a
        // person wrote and not a command anybody ran. Reading the content is
        // what made the guard refuse writing a digger command into
        // MIGRATION_NOTES.md — the exact action the rule it enforces asks for.
        "Edit" | "Write" | "NotebookEdit" => {
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
        if word == "-S" || word == "--split-string" {
            return rest.next().map(String::as_str);
        }
        if let Some(value) = word.strip_prefix("--split-string=") {
            return Some(value);
        }
        if let Some(value) = word.strip_prefix("-S").filter(|v| !v.is_empty()) {
            return Some(value);
        }
        if !word.starts_with('-') {
            return None;
        }
    }
    None
}

/// How many nested `-c` scripts to follow.
///
/// Bounded rather than trusted: a payload can nest `sh -c` inside `sh -c` as
/// deeply as it likes, and a guard that recurses on it hangs the session it
/// exists to protect. Two is every real case seen; deeper fails open for the
/// terraform and commit rules. The exceptions-file rule needs no recursion: it
/// refuses on the whole line's text.
const MAX_NESTING: usize = 2; // ADR-0569-EXCEPTION(CB): bound on recursive -c unwrapping, not a per-install knob.

fn bash(line: &str, depth: usize, policy: &Policy) -> Decision {
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
    if policy.hook_config_tamper && names_the_exceptions_file(line, &commands) {
        return Decision::Deny(EXCEPTIONS.into());
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
    if depth < MAX_NESTING {
        for inner in &inner {
            if let Decision::Deny(reason) = bash(inner, depth + 1, policy) {
                return Decision::Deny(reason);
            }
        }
    }

    for command in &commands {
        // BEFORE resolving the program: `env -S '<command>'` resolves to no
        // program at all, and used to be skipped as allowed.
        if depth < MAX_NESTING {
            if let Some(split) = env_split_string(&command.words) {
                if let Decision::Deny(reason) = bash(split, depth + 1, policy) {
                    return Decision::Deny(reason);
                }
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
        if program == "eval" && depth < MAX_NESTING {
            if let Decision::Deny(reason) = bash(&args.join(" "), depth + 1, policy) {
                return Decision::Deny(reason);
            }
        }
        // A `-c` script is part of the command, so it is read. The rule is about
        // what runs, and `sh -c 'terraform apply'` runs terraform.
        if SHELLS.contains(&program) && depth < MAX_NESTING {
            if let Some(script) = value_of(args, "-c") {
                if let Decision::Deny(reason) = bash(script, depth + 1, policy) {
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

// ── the reasons, which a person reads ────────────────────────────────────────

/// The longest Bash command judged, in bytes. A BOUND, not a knob: a real
/// command is a few hundred bytes, and one this long is refused rather than
/// risk the hook timing out.
const MAX_COMMAND_BYTES: usize = 64 * 1024; // ADR-0569-EXCEPTION(CB): bound on parsing untrusted input, not a per-install knob.

const TOO_LONG: &str = "refused: this Bash command is too long for the guard to check \
     in time, so it was not checked. Split it, or put it in a script file.";

const UNREADABLE: &str = "refused: the hook payload is not a tool call this guard can read \
     (no string `tool_name`, or a Bash call with no string `command`), so this \
     tool call was not checked. Retry it; if this repeats, run `yaadgaar verify`.";

const UNPLACED: &str = "refused: this edit carries no string `file_path` or `notebook_path`, \
     so the guard cannot tell which file it changes, and it was not checked. \
     Retry it; if this repeats, run `yaadgaar verify`.";

const HOOK_BYPASS: &str = "A commit that skips hooks is a commit whose checks did not run. \
     Hook failure is signal, not an obstacle — fix the cause and re-run.";

const TERRAFORM: &str = "terraform, tofu and tfp must never execute here, by any mechanism — \
     including a fresh container, a nix run, or a CI trigger. Write the \
     command into MIGRATION_NOTES.md and hand it over instead.";

const DIGGER: &str = "A digger comment runs terraform against remote state through the \
     orchestrator. This is refused even when explicitly instructed: put the \
     comment in MIGRATION_NOTES.md for a human to post.";

const EXCEPTIONS: &str = "The hook exceptions file is a durable, human-only decision. An agent \
     once edited it to allow its own push, then reverted the file to conceal \
     that. So the guard refuses any Bash command naming the file, a read \
     included, and any edit of it. To read it, use the Read tool.";

#[cfg(test)]
mod tests;

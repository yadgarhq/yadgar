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
//! the guard forbade its own remedy. It refused
//! `cat ~/.claude/yadgar-hook-exceptions.json`, a read. And it allowed
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
//! * Bash-level mutation of the exceptions file is a NAMED set of writing
//!   commands plus redirection targets, not a proof. `Edit`, `Write` and
//!   `NotebookEdit` on that path are exact, and they are where the incident was.
//!
//! Everything in that list fails OPEN, which is the right direction: this runs
//! on the critical path of somebody's session, and a guard that refuses what it
//! cannot understand is a guard that gets switched off.

use serde_json::Value;

use super::Decision;
use crate::install::shellword::{shell_commands, SimpleCommand};

/// Refuse a tool call that the standing rules forbid.
///
/// The matcher covers Bash AND the editing tools, and that breadth was earned:
/// an agent once used Edit rather than Bash to add itself to the push
/// allowlist, pushed to master, then reverted the file to conceal it — and a
/// Bash-only matcher never even routed the call to the guard.
pub fn pre_tool_guard(payload: &Value) -> Decision {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    match tool {
        "Bash" => bash(&field("command"), 0),

        // ONLY THE PATH. An edit executes nothing, so its CONTENT is text a
        // person wrote and not a command anybody ran. Reading the content is
        // what made the guard refuse writing a digger command into
        // MIGRATION_NOTES.md — the exact action the rule it enforces asks for.
        "Edit" | "Write" | "NotebookEdit" => {
            let path = [field("file_path"), field("notebook_path")].join("\n");
            if is_the_exceptions_file(&path) {
                return Decision::Deny(EXCEPTIONS.into());
            }
            Decision::Allow
        }

        _ => Decision::Allow,
    }
}

/// How many nested `-c` scripts to follow.
///
/// Bounded rather than trusted: a payload can nest `sh -c` inside `sh -c` as
/// deeply as it likes, and a guard that recurses on it hangs the session it
/// exists to protect. Two is every real case seen; deeper fails open.
const MAX_NESTING: usize = 2;

fn bash(line: &str, depth: usize) -> Decision {
    let commands = shell_commands(line);

    // Asked over the WHOLE line, because this is the one rule where mentioning
    // is the act. It is still gated on something in the line doing the posting.
    if posts_a_digger_command(line, &commands) {
        return Decision::Deny(DIGGER.into());
    }

    for command in &commands {
        let Some((program, args)) = executed(command) else {
            continue;
        };
        if runs_terraform(program, args) {
            return Decision::Deny(TERRAFORM.into());
        }
        if skips_hooks(program, args) {
            return Decision::Deny(HOOK_BYPASS.into());
        }
        if writes_the_exceptions_file(program, args, &command.writes_to) {
            return Decision::Deny(EXCEPTIONS.into());
        }
        // A `-c` script is part of the command, so it is read. The rule is about
        // what runs, and `sh -c 'terraform apply'` runs terraform.
        if SHELLS.contains(&program) && depth < MAX_NESTING {
            if let Some(script) = value_of(args, "-c") {
                if let Decision::Deny(reason) = bash(script, depth + 1) {
                    return Decision::Deny(reason);
                }
            }
        }
    }
    Decision::Allow
}

// ── What actually runs ───────────────────────────────────────────────────────

/// The program a simple command runs, and the arguments it gets.
///
/// `sudo`, `env` and friends are stripped because they do not change what runs
/// — `sudo terraform apply` is terraform — and a leading `FOO=1` is an
/// assignment rather than a program.
fn executed(command: &SimpleCommand) -> Option<(&str, &[String])> {
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
fn value_of<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
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
    "sudo", "doas", "env", "nohup", "time", "command", "nice", "ionice", "stdbuf", "setsid", "exec",
];

const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash"];

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

// ── the exceptions file ──────────────────────────────────────────────────────

const EXCEPTIONS_FILE: &str = "yadgar-hook-exceptions.json";

/// Commands that write a file named on their command line.
///
/// A NAMED SET, and therefore an incomplete one — said plainly rather than
/// implied. Bash can reach a file by a hundred routes and this covers the ones
/// an agent actually uses; `Edit`, `Write` and `NotebookEdit` are exact, and
/// that is where the incident happened.
const WRITERS: &[&str] = &[
    "tee", "sed", "cp", "mv", "rm", "dd", "truncate", "install", "ln", "chmod", "chown", "shred",
    "patch", "sponge", "python", "python3", "perl", "ruby", "node", "jq",
];

fn writes_the_exceptions_file(program: &str, args: &[String], writes_to: &[String]) -> bool {
    if writes_to.iter().any(|f| is_the_exceptions_file(f)) {
        return true;
    }
    WRITERS.contains(&program) && args.iter().any(|a| is_the_exceptions_file(a))
}

fn is_the_exceptions_file(word: &str) -> bool {
    word.split(['/', '\\', '\n'])
        .any(|part| part == EXCEPTIONS_FILE)
}

// ── the reasons, which a person reads ────────────────────────────────────────

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
     that — which is why this refuses the write rather than trusting the \
     intent behind it.";

#[cfg(test)]
mod tests;

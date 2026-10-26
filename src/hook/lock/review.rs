//! What `guard lock` shows a person before it installs, and whether it may
//! go on without asking.
//!
//! **The source is the user's copy, which an agent can write** until the
//! lock is done. So `lock` never installs unseen: it prints the exact
//! content, the change against the current protected file, and every guard
//! the content switches off, then asks at a terminal. Without a terminal it
//! refuses, unless `--yes` is given AND the content changes nothing that
//! matters: it is what the protected file already holds, or it switches no
//! guard off.

use super::install::Plan;
use crate::hook::Policy;

/// Whether `lock` may install *plan*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Consent {
    Proceed,
    Ask,
    Refuse(String),
}

/// The guards *content* switches off, as a person reads them.
pub fn switched_off(content: &str) -> Vec<&'static str> {
    let Ok(policy) = Policy::parse(content) else {
        return Vec::new();
    };
    let mut off = Vec::new();
    if !policy.terraform_family {
        off.push("terraform_family: terraform, tofu, tfp and digger comments are NOT refused");
    }
    if !policy.git_commit_flags {
        off.push("git_commit_flags: commits that skip hooks or signing are NOT refused");
    }
    if !policy.hook_config_tamper {
        off.push(
            "hook_config_tamper: edits of the exceptions file, Claude Code settings, git hooks \
             and gh shadowing are NOT refused, nor privilege escalation in protected mode",
        );
    }
    off
}

/// What `lock` prints before it asks: the content, the change, the guards.
pub fn review_lines(plan: &Plan, target: &str) -> Vec<String> {
    let mut lines = vec![format!(
        "content to install into {target} (from {}):",
        plan.source.display()
    )];
    lines.extend(plan.content.lines().map(|l| format!("    {l}")));
    match &plan.current {
        None => lines.push(format!("{target} does not exist yet.")),
        Some(Ok(text)) if *text == plan.content => {
            lines.push(format!("{target} already holds exactly this content."));
        }
        Some(Ok(text)) => {
            lines.push(format!("change against the current {target}:"));
            lines.extend(
                text.lines()
                    .filter(|l| !plan.content.lines().any(|n| n == *l))
                    .map(|l| format!("  - {l}")),
            );
            lines.extend(
                plan.content
                    .lines()
                    .filter(|l| !text.lines().any(|o| o == *l))
                    .map(|l| format!("  + {l}")),
            );
        }
        Some(Err(why)) => lines.push(format!(
            "the current file does not verify ({why}); it is replaced."
        )),
    }
    let off = switched_off(&plan.content);
    if off.is_empty() {
        lines.push("guards switched off: none — every guard stays on.".into());
    } else {
        lines.push("guards switched off:".into());
        lines.extend(off.iter().map(|g| format!("  {g}")));
    }
    lines
}

/// Whether to go on: *tty* is whether stdin is a terminal, *yes* the flag.
pub fn consent(tty: bool, yes: bool, plan: &Plan) -> Consent {
    let unchanged = matches!(&plan.current, Some(Ok(text)) if *text == plan.content);
    let harmless = unchanged || switched_off(&plan.content).is_empty();
    if yes && harmless {
        return Consent::Proceed;
    }
    if tty {
        return Consent::Ask;
    }
    Consent::Refuse(if yes {
        "--yes installs only content that the protected file already holds or that switches \
         no guard off; this content switches guards off, so run `sudo yaadgaar guard lock` at \
         a terminal and confirm it"
            .into()
    } else {
        "stdin is not a terminal, so nobody can confirm the content: run `sudo yaadgaar guard \
         lock` at a terminal"
            .into()
    })
}

/// Whether *answer*, typed at the prompt, confirms.
pub fn confirms(answer: &str) -> bool {
    matches!(answer.trim(), "y" | "yes" | "Y" | "YES")
}

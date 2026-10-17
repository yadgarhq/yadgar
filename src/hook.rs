//! `yaadgaar hook <name>` — every hook, one binary (D76).
//!
//! **One binary is the point, not a convenience.** The Python client shipped a
//! directory of hook scripts, and nix's home-manager kept its own copies of the
//! same handlers. The two diverged on `project_id` and the capture pipeline was
//! dead for six days while every signal read healthy. A single binary at one
//! path cannot have a second copy to drift from.
//!
//! Handlers are keyed on NAME, not on event: `SessionStart` carries two
//! registrations wanting different behaviour, so an event-keyed dispatcher could
//! not serve the registrations at all. [`crate::install::hooks::MANAGED_HOOKS`]
//! is the one list; this dispatches on the same strings, so a handler that is
//! registered and a handler that exists cannot drift apart unnoticed.
//!
//! ## Fail open, without exception
//!
//! Every handler swallows its errors and allows. A hook runs on the critical
//! path of somebody's session: a panic here does not degrade yadgar, it breaks
//! the agent. The only thing that may ever exit non-zero is
//! [`pre_tool_guard`], and only on a positively matched dangerous command —
//! never on an error, never on uncertainty.
//!
//! ## How a refusal reaches the agent client
//!
//! A refusal is exit status [`REFUSED_EXIT_CODE`] plus the reason as plain text
//! on STDERR, and nothing on stdout. That is the documented contract for a
//! blocking hook: "Exit with code 2 to block the action ... The message that
//! comes with the block is your hook's stderr." (Claude Code hooks reference,
//! "Exit code 2".) The same page advises one approach per hook, either exit
//! codes alone or exit 0 with JSON, so no JSON is printed.
//!
//! The earlier shape printed a `hookSpecificOutput` JSON to stdout and left
//! stderr empty. The block held, because status 2 blocks on its own, but the
//! agent was shown no reason: measured on a real home, a refusal arrived as
//! "No stderr output". Exit 2 also fails closed if the client ever changes how
//! it parses stdout, which exit 0 plus JSON would not.

use std::io::Read as _;
use std::path::Path;

use serde_json::Value;

mod guard;
mod policy;

pub use guard::pre_tool_guard;
pub use policy::{Policy, EXCEPTIONS_FILE};

/// The exit status a positively matched refusal leaves.
///
/// TWO, and only ever from [`pre_tool_guard`]. Claude Code treats 2 as a
/// blocking error; every other non-zero status is a hook that merely failed, is
/// reported as a non-blocking error, and lets the tool run.
pub const REFUSED_EXIT_CODE: i32 = 2; // ADR-0569-EXCEPTION(CB): Claude Code's own contract — exit 2 is the status it treats as a blocking refusal.

/// What a handler decided. Anything but [`Decision::Deny`] lets the tool run.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Proceed. The overwhelming majority, including every error path.
    Allow,
    /// Refuse, with a reason the agent shows the person.
    Deny(String),
}

/// Run one hook. Never returns an error: the caller allows regardless.
///
/// The payload is a PARAMETER rather than read here, so every path through this
/// function is reachable from a test. A `run` that read stdin itself could only
/// be exercised by spawning a process, which is why it never was. *exceptions*
/// is the path of the exceptions file, for the same reason: `None` when the
/// home directory could not be found, which leaves every guard on.
pub fn run(name: &str, payload: &Value, exceptions: Option<&Path>) -> Decision {
    match name {
        // The one handler, and the one that can refuse. Purely local — it
        // inspects the command it was handed and needs nothing from the
        // gateway. The exceptions file is read here and nowhere else, so a
        // stale entry for any other name never opens it.
        "pre-tool-guard" => pre_tool_guard(payload, &Policy::load(exceptions)),

        // Any other name is an entry an older install left behind: up to 0.1.4
        // `install` registered eleven more handlers, all of them inert, and they
        // keep firing until the next `yaadgaar install` strips them. Allow,
        // silently, and let `verify` be the thing that names them.
        _ => Decision::Allow,
    }
}

/// The hook payload the agent client writes to this process's stdin.
///
/// Separate from [`run`] so the dispatch is testable without a process.
pub fn read_stdin() -> Value {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return Value::Null;
    }
    serde_json::from_str(&buf).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash(cmd: &str) -> Value {
        json!({"tool_name": "Bash", "tool_input": {"command": cmd}})
    }

    #[test]
    fn the_guard_refuses_through_the_dispatcher_and_not_only_in_isolation() {
        // `run` was never called by anything, so eight passing tests of
        // `pre_tool_guard` read as coverage of a deny path that could not
        // refuse. This is the end-to-end seam: name in, reason out.
        let decision = run("pre-tool-guard", &bash("git commit --no-verify -m x"), None);
        let Decision::Deny(reason) = decision else {
            panic!("the dispatcher allowed a command the guard refuses");
        };
        assert!(
            !reason.trim().is_empty(),
            "a refusal needs a reason on stderr"
        );
    }

    /// The handlers 0.1.4 and earlier registered beside the guard, all inert.
    const RETIRED: &[&str] = &[
        "pre-compact-drain",
        "session-start-context",
        "post-compact-rehydrate",
        "post-tool-capture",
        "block-reflect",
        "prompt-recall",
        "stop-checkpoint",
        "session-end-capture",
        "instructions-loaded",
        "subagent-start",
        "file-changed",
    ];

    #[test]
    fn an_entry_an_older_install_left_behind_allows_through_the_dispatcher() {
        // Until the next `install` strips them, these keep firing. They must do
        // nothing, even on a call the guard would refuse.
        for name in RETIRED {
            assert_eq!(
                run(name, &bash("terraform apply"), None),
                Decision::Allow,
                "{name}"
            );
        }
    }

    #[test]
    fn a_guard_switched_off_in_the_exceptions_file_allows_through_the_dispatcher() {
        // The file is read by `run`, from the path it is handed — not only by
        // `Policy` in isolation.
        let dir = std::env::temp_dir().join(format!("yaadgaar-hook-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(EXCEPTIONS_FILE);
        std::fs::write(&path, r#"{"disabled_guards":["git_commit_flags"]}"#).unwrap();
        let call = bash("git commit --no-verify -m x");
        assert_eq!(run("pre-tool-guard", &call, Some(&path)), Decision::Allow);
        assert!(matches!(
            run("pre-tool-guard", &bash("terraform apply"), Some(&path)),
            Decision::Deny(_)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_a_blocking_status_is_ever_left_by_a_refusal() {
        // Any other non-zero status is read as a hook that merely failed: it is
        // reported as a non-blocking error and the tool RUNS.
        assert_eq!(REFUSED_EXIT_CODE, 2);
    }

    #[test]
    fn every_registered_handler_is_a_real_handler_in_the_dispatcher() {
        // The drift this pins: a handler registered in settings.json that this
        // binary does not dispatch fires on every session and does nothing,
        // silently. Every registration must reach a handler that can refuse.
        for spec in crate::install::hooks::MANAGED_HOOKS {
            assert!(
                matches!(
                    run(spec.name, &bash("terraform apply"), None),
                    Decision::Deny(_)
                ),
                "{} is registered but the dispatcher does not run it",
                spec.name
            );
        }
    }
}

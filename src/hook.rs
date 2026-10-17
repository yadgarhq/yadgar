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
//! A `Decision` that nothing serialises refuses nothing, so the wire format is
//! part of this module rather than an afterthought at the call site. Claude
//! Code reads a hook's STDOUT as JSON and maps it before it looks at the exit
//! code — verified against the installed client, `claude-code-2.1.251`, whose
//! hook-result reducer contains, literally:
//!
//! ```text
//! switch(e.hookSpecificOutput.hookEventName){case"PreToolUse":
//!   if(e.hookSpecificOutput.permissionDecision)
//!     switch(e.hookSpecificOutput.permissionDecision){ …
//!       case"deny": O.permissionBehavior="deny",
//!         O.blockingError={blockingError:
//!           e.hookSpecificOutput.permissionDecisionReason||e.reason
//!           ||"Blocked by hook",command:t};break;
//! ```
//!
//! and, on the same reducer, the event-agnostic older form:
//!
//! ```text
//! if(e.decision)switch(e.decision){ …
//!   case"block": O.permissionBehavior="deny",
//!     O.blockingError={blockingError:e.reason||"Blocked by hook",command:t};
//! ```
//!
//! Two details from the same source decide the shape [`refusal`] emits:
//!
//! * The reducer **throws** when `hookSpecificOutput.hookEventName` is not the
//!   event that fired — `Hook returned incorrect event name: expected '…' but
//!   got '…'`. A second literal for the event name here would turn every
//!   refusal into a silent allow, which is exactly D76's dead-pipeline shape
//!   inside the one handler written to refuse. So the event is read from
//!   [`crate::install::MANAGED_HOOKS`], never written twice.
//! * Stdout is parsed BEFORE any exit-code branch, and status 2 is separately a
//!   blocking error (`if(nr.status===2&&!xn.blockingError)` — it fills in from
//!   stderr only when the JSON did not already refuse). So a refusal both prints
//!   the JSON and exits [`REFUSED_EXIT_CODE`]: the JSON carries the reason, and
//!   the status blocks even if the JSON is somehow lost.

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

/// The JSON a refusal writes to stdout, in the form the agent client honours.
///
/// See the module note for the citation. The event name is looked up rather
/// than written: a mismatch is thrown on, not ignored.
pub fn refusal(name: &str, reason: &str) -> String {
    match event_for(name) {
        Some(event) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": event,
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
        // A handler this binary can refuse from but cannot name an event for
        // should be impossible — [`run`] only refuses from a registered
        // handler. If it ever happens, the event-agnostic form still blocks,
        // where guessing an event name would throw and allow.
        None => serde_json::json!({ "decision": "block", "reason": reason }),
    }
    .to_string()
}

/// The event key the registration for *name* fires on.
fn event_for(name: &str) -> Option<&'static str> {
    crate::install::MANAGED_HOOKS
        .iter()
        .find(|h| h.name == name)
        .map(|h| h.event)
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
    fn a_refusal_is_emitted_in_the_form_the_agent_client_honours() {
        // The shape, pinned against literals read off claude-code-2.1.251's own
        // hook-result reducer. Getting any one of the three keys wrong is not a
        // degraded refusal, it is no refusal at all.
        let v: Value =
            serde_json::from_str(&refusal("pre-tool-guard", "because it would delete master"))
                .unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecisionReason"],
            "because it would delete master"
        );
    }

    #[test]
    fn the_event_a_refusal_names_is_the_one_its_registration_fires_on() {
        // The reducer THROWS on a mismatch — `Hook returned incorrect event
        // name` — and a thrown reducer refuses nothing. So this is not a
        // cosmetic assertion: a second literal for the event name is a silent
        // allow, which is D76's dead-pipeline shape inside the one handler
        // written to refuse.
        let registered = crate::install::MANAGED_HOOKS
            .iter()
            .find(|h| h.name == "pre-tool-guard")
            .expect("the guard is registered");
        let v: Value = serde_json::from_str(&refusal("pre-tool-guard", "no")).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["hookEventName"], registered.event,
            "the refusal names an event the registration does not fire on"
        );
    }

    #[test]
    fn a_handler_with_no_registration_still_blocks() {
        // Guessing an event name would throw and therefore ALLOW. The
        // event-agnostic form is the one the same reducer maps without ever
        // comparing an event.
        let v: Value = serde_json::from_str(&refusal("not-a-registered-handler", "no")).unwrap();
        assert_eq!(v["decision"], "block");
        assert_eq!(v["reason"], "no");
        assert!(v["hookSpecificOutput"].is_null());
    }

    #[test]
    fn the_guard_refuses_through_the_dispatcher_and_not_only_in_isolation() {
        // `run` was never called by anything, so eight passing tests of
        // `pre_tool_guard` read as coverage of a deny path that could not
        // refuse. This is the end-to-end seam: name in, wire format out.
        let decision = run("pre-tool-guard", &bash("git commit --no-verify -m x"), None);
        let Decision::Deny(reason) = decision else {
            panic!("the dispatcher allowed a command the guard refuses");
        };
        let v: Value = serde_json::from_str(&refusal("pre-tool-guard", &reason)).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
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

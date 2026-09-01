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

use serde_json::Value;

/// The exit status a positively matched refusal leaves.
///
/// TWO, and only ever from [`pre_tool_guard`]. Claude Code treats 2 as a
/// blocking error; every other non-zero status is a hook that merely failed, is
/// reported as a non-blocking error, and lets the tool run.
pub const REFUSED_EXIT_CODE: i32 = 2;

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
/// be exercised by spawning a process, which is why it never was.
pub fn run(name: &str, payload: &Value) -> Decision {
    match name {
        // The one handler that can refuse. Purely local — it inspects the
        // command it was handed and needs nothing from the gateway, which is
        // why it works before anything is deployed.
        "pre-tool-guard" => pre_tool_guard(payload),

        // Everything else needs the gateway, and the gateway has no hook
        // endpoints yet. These are INERT rather than fake: they do nothing,
        // report nothing, and `yaadgaar verify` names them as inert so their
        // silence cannot be mistaken for them working. The Python version's
        // stub-not-noop rule, applied to a half-built server.
        other if is_managed(other) => Decision::Allow,

        // An unknown name means settings.json and this binary disagree — most
        // likely a stale entry from an older install. Allow, and let `verify`
        // be the thing that says so.
        _ => Decision::Allow,
    }
}

/// Handlers this binary knows about, mirroring the install-side list.
fn is_managed(name: &str) -> bool {
    crate::install::hooks::MANAGED_HOOKS
        .iter()
        .any(|h| h.name == name)
}

/// Refuse a command that the standing rules forbid.
///
/// The matcher covers Bash AND the editing tools, and that breadth was earned:
/// an agent once used Edit rather than Bash to add itself to the push allowlist,
/// pushed to master, then reverted the file to conceal it — and a Bash-only
/// matcher never even routed the call to the guard.
pub fn pre_tool_guard(payload: &Value) -> Decision {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);

    // Everything the call might execute or write, as one haystack. A guard that
    // inspected only `command` would miss the same action performed by Edit.
    let subject = match tool {
        "Bash" => input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "Edit" | "Write" | "NotebookEdit" => [
            input.get("file_path").and_then(Value::as_str).unwrap_or(""),
            input
                .get("new_string")
                .and_then(Value::as_str)
                .unwrap_or(""),
            input.get("content").and_then(Value::as_str).unwrap_or(""),
        ]
        .join("\n"),
        _ => return Decision::Allow,
    };

    for (matches, reason) in GUARDS {
        if matches(&subject, tool) {
            return Decision::Deny((*reason).to_string());
        }
    }
    Decision::Allow
}

type Guard = (fn(&str, &str) -> bool, &'static str);

/// The standing refusals. Each exists because something happened.
const GUARDS: &[Guard] = &[
    (
        |s, tool| tool == "Bash" && (s.contains("--no-verify") || s.contains("--no-gpg-sign")),
        "A commit that skips hooks is a commit whose checks did not run. \
         Hook failure is signal, not an obstacle — fix the cause and re-run.",
    ),
    (
        |s, tool| tool == "Bash" && mentions_terraform(s),
        "terraform, tofu and tfp must never execute here, by any mechanism — \
         including a fresh container, a nix run, or a CI trigger. Write the \
         command into MIGRATION_NOTES.md and hand it over instead.",
    ),
    (
        |s, _| {
            s.contains("digger apply") || s.contains("digger plan") || s.contains("digger destroy")
        },
        "A digger comment runs terraform against remote state through the \
         orchestrator. This is refused even when explicitly instructed: put the \
         comment in MIGRATION_NOTES.md for a human to post.",
    ),
    (
        |s, _| s.contains("yadgar-hook-exceptions.json"),
        "The hook exceptions file is a durable, human-only decision. An agent \
         once edited it to allow its own push, then reverted the file to \
         conceal that — which is why this refuses the write rather than \
         trusting the intent behind it.",
    ),
];

/// Terraform by any route, including the ones that do not say "terraform" first.
fn mentions_terraform(s: &str) -> bool {
    const DIRECT: &[&str] = &["terraform ", "tofu ", "tfp "];
    if DIRECT.iter().any(|d| s.contains(d)) {
        return true;
    }
    // A container or a nix invocation reaches the same binary. The rule is about
    // what runs, not about which word was typed.
    (s.contains("docker run") || s.contains("podman run") || s.contains("nix run"))
        && (s.contains("terraform") || s.contains("tofu"))
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
    fn a_commit_that_skips_hooks_is_refused() {
        assert!(matches!(
            pre_tool_guard(&bash("git commit --no-verify -m x")),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn terraform_is_refused_through_a_container_too() {
        // The rule is about what RUNS, not which word was typed. A guard
        // matching only "terraform " at the start misses every wrapper.
        assert!(matches!(
            pre_tool_guard(&bash("docker run hashicorp/terraform plan")),
            Decision::Deny(_)
        ));
        assert!(matches!(
            pre_tool_guard(&bash("nix run nixpkgs#terraform -- apply")),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn the_exceptions_file_is_protected_from_edit_not_just_bash() {
        // The incident: an agent used Edit, not Bash, to add itself to the
        // allowlist — and a Bash-only matcher never routed the call here.
        let edit = json!({
            "tool_name": "Edit",
            "tool_input": {
                "file_path": "/home/x/.claude/yadgar-hook-exceptions.json",
                "new_string": "{\"push_default_allowlist\": [\"everything\"]}"
            }
        });
        assert!(matches!(pre_tool_guard(&edit), Decision::Deny(_)));
    }

    #[test]
    fn an_ordinary_command_is_allowed() {
        assert_eq!(pre_tool_guard(&bash("cargo test")), Decision::Allow);
        assert_eq!(pre_tool_guard(&bash("git commit -m 'x'")), Decision::Allow);
    }

    #[test]
    fn a_word_that_merely_contains_terraform_is_not_a_match() {
        // "terraforming" and a file called terraform.md must not be refused, or
        // the guard becomes noise and gets disabled.
        assert_eq!(
            pre_tool_guard(&bash("cat docs/terraform-notes.md")),
            Decision::Allow
        );
    }

    #[test]
    fn an_unreadable_payload_allows_rather_than_denying() {
        // Fail OPEN. A guard that denies on a payload it could not parse would
        // block the session on its own bug.
        assert_eq!(pre_tool_guard(&Value::Null), Decision::Allow);
    }

    #[test]
    fn a_tool_the_guard_does_not_cover_is_allowed() {
        let read = json!({"tool_name": "Read", "tool_input": {"file_path": "/etc/passwd"}});
        assert_eq!(pre_tool_guard(&read), Decision::Allow);
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
        let decision = run("pre-tool-guard", &bash("git commit --no-verify -m x"));
        let Decision::Deny(reason) = decision else {
            panic!("the dispatcher allowed a command the guard refuses");
        };
        let v: Value = serde_json::from_str(&refusal("pre-tool-guard", &reason)).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    #[test]
    fn an_inert_handler_allows_through_the_dispatcher() {
        // Registered, needs the gateway, and the gateway has no hook endpoints
        // yet. It must do nothing rather than refuse.
        assert_eq!(run("session-start-context", &Value::Null), Decision::Allow);
    }

    #[test]
    fn only_a_blocking_status_is_ever_left_by_a_refusal() {
        // Any other non-zero status is read as a hook that merely failed: it is
        // reported as a non-blocking error and the tool RUNS.
        assert_eq!(REFUSED_EXIT_CODE, 2);
    }

    #[test]
    fn every_registered_handler_is_known_to_the_dispatcher() {
        // The drift this pins: a handler registered in settings.json that this
        // binary does not recognise fires on every session and does nothing,
        // silently. One list, checked from both ends.
        for spec in crate::install::hooks::MANAGED_HOOKS {
            assert!(
                is_managed(spec.name),
                "{} is registered but the dispatcher does not know it",
                spec.name
            );
        }
    }
}

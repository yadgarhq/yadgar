//! The `yaadgaar hook <name>` process: read the payload, decide, and exit with
//! the status Claude Code honours.
//!
//! Out of `main.rs` because this is the one place the binary's exit status IS
//! its answer, and every path through it has to end in either silence or a
//! blocking refusal — a panic included (ledger 1570).

use std::io::Write as _;
use std::panic::AssertUnwindSafe;

use super::{dispatch, read_stdin, Decision, REFUSED_EXIT_CODE};
use crate::install;

/// Run one hook handler and answer the agent client in the format it honours.
///
/// EXITS rather than returns on a refusal, and the asymmetry is the point: a
/// handler that fails open leaves no trace, and a handler that refuses is the
/// only thing in this binary that ever exits non-zero.
pub fn run(name: &str) -> anyhow::Result<()> {
    let decision = contain_panic(|| {
        // NOT `home()?`. An error here exits 1, which Claude Code reads as a
        // hook that merely failed — and lets the tool run. A home that cannot
        // be found is `None`, and `None` leaves every guard on.
        let exceptions = dirs::home_dir().map(|h| install::Layout::new(&h).exceptions());
        dispatch(name, read_stdin(), exceptions.as_deref())
    });
    match decision {
        // Silence. An allow that printed would add a line to the transcript on
        // every single tool call.
        Decision::Allow => Ok(()),
        Decision::Deny(reason) => refuse(name, &reason),
    }
}

/// Run *decide*, and turn a panic inside it into a refusal.
///
/// A panic left status 101 — a non-blocking error to Claude Code, so the tool
/// RAN unchecked (ledger 1570). Reading stdin and the exceptions file happens
/// inside *decide*, so a panic there refuses too. `catch_unwind` needs the
/// default `panic = "unwind"`; a test pins that `Cargo.toml` never sets
/// `abort`, under which this would catch nothing.
pub fn contain_panic(decide: impl FnOnce() -> Decision) -> Decision {
    std::panic::catch_unwind(AssertUnwindSafe(decide)).unwrap_or_else(|payload| {
        let what = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        Decision::Deny(format!(
            "refused: the hook panicked ({what}), so this tool call was not checked"
        ))
    })
}

/// Write *reason* to stderr and exit with the blocking status.
///
/// STDERR ONLY, and stdout stays empty. On exit 2 Claude Code feeds stderr to
/// the model — but it ALSO still reads stdout, and a valid JSON object there
/// that makes a blocking decision would replace the stderr reason with its own
/// (Claude Code hooks reference, "Exit code 2"). One channel carries the
/// reason, so nothing is printed to stdout.
///
/// FLUSHED BEFORE THE EXIT, and not for tidiness. `process::exit` runs no
/// destructors, so an unflushed line is simply lost — and a lost reason still
/// blocks (status 2 is a blocking error on its own) while the agent sees "No
/// stderr output". The refusal would work and explain nothing.
fn refuse(name: &str, reason: &str) -> ! {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "yaadgaar {name}: {reason}");
    let _ = err.flush();
    drop(err);
    std::process::exit(REFUSED_EXIT_CODE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_while_deciding_is_a_refusal_that_names_it() {
        let Decision::Deny(reason) = contain_panic(|| panic!("a planted panic")) else {
            panic!("a panic was let through as an allow");
        };
        assert!(reason.contains("a planted panic"), "{reason}");
        let owned = contain_panic(|| panic!("{}", String::from("formatted")));
        assert!(matches!(owned, Decision::Deny(r) if r.contains("formatted")));
    }

    #[test]
    fn a_decision_that_does_not_panic_passes_through_unchanged() {
        assert_eq!(contain_panic(|| Decision::Allow), Decision::Allow);
    }

    #[test]
    fn the_crate_never_aborts_on_panic() {
        // Under `panic = "abort"` `catch_unwind` catches nothing, and a
        // panicking guard would exit 134 — not a blocking status.
        let manifest = include_str!("../../Cargo.toml");
        assert!(!manifest.contains("panic = \"abort\""), "Cargo.toml aborts");
    }
}

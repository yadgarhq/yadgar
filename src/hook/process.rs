//! The `yaadgaar hook <name>` process: read the payload, decide, and exit with
//! the status Claude Code honours.
//!
//! Out of `main.rs` because this is the one place the binary's exit status IS
//! its answer, and every path through it has to end in either silence or a
//! blocking refusal — a panic included (ledger 1570).

use std::io::Write as _;
use std::panic::AssertUnwindSafe;
use std::sync::Mutex;

use super::{dispatch, read_stdin, Decision, REFUSED_EXIT_CODE};
use crate::install;

// A pin by `cfg`, not by searching `Cargo.toml` for a string. The string search
// missed `panic="abort"`, a profile in `.cargo/config.toml` and
// `RUSTFLAGS=-C panic=abort` (ledger 1576 (b)); the compiler sees all three.
#[cfg(panic = "abort")]
compile_error!("the pre-tool guard needs panic=unwind: catch_unwind catches nothing under abort");

/// The panic hook is process-global. One lock keeps two contained panics (only
/// ever concurrent in tests) from restoring each other's hook.
static HOOK_LOCK: Mutex<()> = Mutex::new(());

/// The hook name, when this invocation is `yaadgaar hook <name>`.
///
/// Read from the raw arguments, before clap, for the reason `main` gives: the
/// hook path must not depend on anything that can fail outside its containment.
/// `None` for any other shape, including a bare `hook` — clap's usage error
/// (exit 2) is the right answer to that.
pub fn hook_name(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<String> {
    args.next()?; // argv[0]
    if args.next()? != "hook" {
        return None;
    }
    Some(args.next()?.to_string_lossy().into_owned())
}

/// Run one hook handler and answer the agent client in the format it honours.
///
/// EXITS rather than returns on a refusal, and the asymmetry is the point: a
/// handler that fails open leaves no trace, and a handler that refuses is the
/// only thing in this binary that ever exits non-zero.
///
/// *init* is the caller's setup (tracing), run INSIDE the containment so a panic
/// there refuses too (ledger 1576 (d)).
pub fn run(name: &str, init: impl FnOnce()) -> anyhow::Result<()> {
    match decide(name, init) {
        // Silence. An allow that printed would add a line to the transcript on
        // every single tool call.
        Decision::Allow => Ok(()),
        Decision::Deny(reason) => refuse(name, &reason),
    }
}

/// [`run`] without the exit, so a test can read the decision.
fn decide(name: &str, init: impl FnOnce()) -> Decision {
    contain_panic(|| {
        init();
        // NOT `home()?`. An error here exits 1, which Claude Code reads as a
        // hook that merely failed — and lets the tool run. A home that cannot
        // be found is `None`, and `None` leaves every guard on.
        let exceptions = dirs::home_dir().map(|h| install::Layout::new(&h).exceptions());
        dispatch(name, read_stdin(), exceptions.as_deref())
    })
}

/// Run *decide*, and turn a panic inside it into a refusal.
///
/// A panic left status 101 — a non-blocking error to Claude Code, so the tool
/// RAN unchecked (ledger 1570). Reading stdin and the exceptions file happens
/// inside *decide*, so a panic there refuses too. `catch_unwind` needs the
/// default `panic = "unwind"`; a test pins that `Cargo.toml` never sets
/// `abort`, under which this would catch nothing — the `compile_error!` above
/// refuses to build that.
///
/// SILENT: the default hook has already written `thread 'main' panicked at` to
/// stderr by the time the unwind is caught, and on exit 2 Claude Code hands
/// stderr to the model, so the noise would surround the reason. The reason
/// below carries the message (ledger 1576 (c)). The previous hook is restored.
pub fn contain_panic(decide: impl FnOnce() -> Decision) -> Decision {
    let _held = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    contain_panic_locked(decide)
}

fn contain_panic_locked(decide: impl FnOnce() -> Decision) -> Decision {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(AssertUnwindSafe(decide));
    std::panic::set_hook(previous);
    caught.unwrap_or_else(|payload| {
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    static NOISE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn a_contained_panic_prints_nothing_and_the_previous_hook_comes_back() {
        // Ledger 1576 (c): the default hook wrote `thread 'main' panicked at`
        // to stderr before the unwind was caught, and on exit 2 that line
        // reaches the model around the refusal reason.
        let _held = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let outer = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {
            NOISE.fetch_add(1, Ordering::SeqCst);
        }));
        let denied = contain_panic_locked(|| panic!("planted"));
        let during = NOISE.load(Ordering::SeqCst);
        let _ = std::panic::catch_unwind(|| panic!("outside the guard"));
        let after = NOISE.load(Ordering::SeqCst);
        std::panic::set_hook(outer);
        assert!(matches!(denied, Decision::Deny(_)));
        assert_eq!(during, 0, "the hook ran for a contained panic");
        assert_eq!(after, 1, "the previous hook was not restored");
    }

    #[test]
    fn a_panic_in_the_setup_before_dispatch_is_a_refusal_too() {
        // Ledger 1576 (d): tracing init sat outside the containment.
        let Decision::Deny(reason) = decide("pre-tool-guard", || panic!("init broke")) else {
            panic!("a panic in setup was let through as an allow");
        };
        assert!(reason.contains("init broke"), "{reason}");
    }

    #[test]
    fn hook_name_is_read_from_the_raw_arguments_only_for_the_hook_shape() {
        let args = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let name = |v: &[&str]| hook_name(args(v).into_iter());
        assert_eq!(
            name(&["yaadgaar", "hook", "pre-tool-guard"]).as_deref(),
            Some("pre-tool-guard")
        );
        // A trailing flag is not clap's to judge any more.
        assert_eq!(
            name(&["yaadgaar", "hook", "pre-tool-guard", "--help"]).as_deref(),
            Some("pre-tool-guard")
        );
        assert_eq!(name(&["yaadgaar", "hook"]), None);
        assert_eq!(name(&["yaadgaar", "serve"]), None);
        assert_eq!(name(&["yaadgaar"]), None);
    }
}

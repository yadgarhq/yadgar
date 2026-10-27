//! The `yaadgaar hook <name>` process: read the payload, decide, and exit with
//! the status Claude Code honours.
//!
//! Out of `main.rs` because this is the one place the binary's exit status IS
//! its answer, and every path through it has to end in either silence or a
//! deliberate refusal — a panic included (ledger 1570).

use std::io::Write as _;
use std::panic::AssertUnwindSafe;
use std::sync::Mutex;

use super::{dispatch, read_stdin, session_start, Decision, REFUSED_EXIT_CODE};

// A pin by `cfg`, not by searching `Cargo.toml` for a string. The string search
// missed `panic="abort"`, a profile in `.cargo/config.toml` and
// `RUSTFLAGS=-C panic=abort` (ledger 1576 (b)); the compiler sees all three.
#[cfg(panic = "abort")]
compile_error!(
    "the hook handlers need panic=unwind: catch_unwind catches nothing under abort, \
     and a panic in session-start or write-back must not stop the session"
);

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

/// A DEBUG-BUILD FAULT for `tests/hook_exit.rs`: a panic in the hook's setup,
/// before dispatch, must be contained like any other (ledger 1576 (d)). Called
/// from `init_tracing`, the setup step. `debug_assertions` is off in every
/// release build, so no shipped binary reads the variable.
pub fn planted_setup_fault() {
    #[cfg(debug_assertions)]
    if std::env::var_os("YAADGAAR_TEST_PANIC_IN_SETUP").is_some() {
        panic!("planted panic in the hook's setup");
    }
}

/// Does [`run`] answer *name* with its own handler, rather than [`dispatch`]?
pub fn routes(name: &str) -> bool {
    name == session_start::NAME
}

/// Run one hook handler and answer the agent client in the format it honours.
///
/// EXITS rather than returns on a refusal, and the asymmetry is the point: a
/// handler that allows leaves no trace, and a write-back refusal is the only
/// thing in this binary that ever exits non-zero from a hook.
///
/// *init* is the caller's setup (tracing), run INSIDE the containment so a panic
/// there is contained too (ledger 1576 (d)).
pub fn run(name: &str, init: impl FnOnce()) -> anyhow::Result<()> {
    if routes(name) {
        return answer(&session_start_output(init));
    }
    match decide(name, init) {
        // Silence. An allow that printed would add a line to the transcript on
        // every single tool call.
        Decision::Allow => Ok(()),
        Decision::Deny(reason) => refuse(name, &reason),
    }
}

/// What `session-start` prints: its context, nothing when the session is out
/// of scope, or its notice when it panicked.
///
/// Never a refusal. A panic here, *init*'s included, becomes the same
/// fail-safe notice, so the session still starts and the agent is told why.
fn session_start_output(init: impl FnOnce()) -> Option<String> {
    contain(
        || {
            init();
            session_start::run()
        },
        |what| Some(session_start::panicked(&what)),
    )
}

/// Print *output* to stdout, flushed, and leave exit status 0. `None` writes
/// NOT ONE BYTE — an empty line would still be output.
fn answer(output: &Option<String>) -> anyhow::Result<()> {
    let Some(output) = output else {
        return Ok(());
    };
    let mut out = std::io::stdout().lock();
    // A failed write has nobody left to tell; the session starts regardless.
    let _ = writeln!(out, "{output}");
    let _ = out.flush();
    Ok(())
}

/// [`run`] without the exit, so a test can read the decision.
///
/// A panic ALLOWS (see `hook.rs`): the write-back handlers fail open, and a
/// retired handler judges nothing.
fn decide(name: &str, init: impl FnOnce()) -> Decision {
    contain(
        || {
            init();
            dispatch(name, read_stdin())
        },
        |_| Decision::Allow,
    )
}

/// Run *work*, and turn a panic inside it into `on_panic(message)`.
///
/// A panic left status 101, which Claude Code reads as a non-blocking error
/// (ledger 1570); the handlers here answer a panic on purpose instead.
/// `session-start` answers with its fail-safe notice and exit 0, because a
/// panic there must not stop the session. `catch_unwind` needs the default
/// `panic = "unwind"`; under `abort` it would catch nothing, and the
/// `compile_error!` above refuses to build that.
///
/// SILENT: the default hook would write `thread 'main' panicked at` to stderr
/// before the unwind is caught, and Claude Code shows hook stderr to the agent.
/// The previous hook is restored.
pub fn contain<T>(work: impl FnOnce() -> T, on_panic: impl FnOnce(String) -> T) -> T {
    let _held = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    contain_locked(work, on_panic)
}

fn contain_locked<T>(work: impl FnOnce() -> T, on_panic: impl FnOnce(String) -> T) -> T {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(AssertUnwindSafe(work));
    std::panic::set_hook(previous);
    caught.unwrap_or_else(|payload| {
        let what = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        on_panic(what)
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
    use crate::hook::write_back;

    #[test]
    fn a_panic_is_answered_with_its_message() {
        let what = contain(|| panic!("a planted panic"), |what| what);
        assert!(what.contains("a planted panic"), "{what}");
        let owned = contain(|| panic!("{}", String::from("formatted")), |what| what);
        assert!(owned.contains("formatted"), "{owned}");
    }

    #[test]
    fn work_that_does_not_panic_passes_through_unchanged() {
        assert_eq!(
            contain(|| Decision::Allow, |_| Decision::Deny(String::new())),
            Decision::Allow
        );
    }

    /// Set in the child process only: it makes the test below plant the panic.
    const CHILD: &str = "YAADGAAR_TEST_CONTAIN_PANIC_CHILD";

    #[test]
    fn a_panic_does_not_reach_stderr_through_the_default_hook() {
        // Ledger 1576 (c): the default hook wrote `thread 'main' panicked at`
        // to stderr before the unwind was caught, and Claude Code shows hook
        // stderr to the agent.
        //
        // A CHILD PROCESS, because only a real stderr shows what the default
        // hook prints: the test harness captures a panic message in-process.
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(contain(|| panic!("x"), |what| what), "x");
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "--nocapture",
                "--test-threads=1",
                "hook::process::tests::a_panic_does_not_reach_stderr_through_the_default_hook",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "the child failed: {out:?}");
        assert!(!stderr.contains("panicked at"), "{stderr}");
    }

    #[test]
    fn the_previous_hook_comes_back_after_a_contained_panic() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        let _held = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let outer = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {
            SEEN.fetch_add(1, Ordering::SeqCst);
        }));
        contain_locked(|| panic!("planted"), |_| ());
        let _ = std::panic::catch_unwind(|| panic!("outside the containment"));
        std::panic::set_hook(outer);
        assert_eq!(SEEN.load(Ordering::SeqCst), 1, "hook not restored");
    }

    #[test]
    fn a_panic_in_a_handler_or_its_setup_allows() {
        // The deliberate fail-open (plan, handler `task-update`): TaskCompleted
        // has no matcher, so a deny-on-panic would block every completion. A
        // retired handler judges nothing, so it allows on a panic too.
        for name in write_back::NAMES.iter().chain(&["pre-tool-guard"]) {
            assert_eq!(
                decide(name, || panic!("planted")),
                Decision::Allow,
                "{name}"
            );
        }
    }

    #[test]
    fn hook_name_is_read_from_the_raw_arguments_only_for_the_hook_shape() {
        let args = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let name = |v: &[&str]| hook_name(args(v).into_iter());
        assert_eq!(
            name(&["yaadgaar", "hook", "task-update"]).as_deref(),
            Some("task-update")
        );
        // A trailing flag is not clap's to judge any more.
        assert_eq!(
            name(&["yaadgaar", "hook", "task-update", "--help"]).as_deref(),
            Some("task-update")
        );
        assert_eq!(name(&["yaadgaar", "hook"]), None);
        assert_eq!(name(&["yaadgaar", "serve"]), None);
        assert_eq!(name(&["yaadgaar"]), None);
    }
}

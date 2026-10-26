//! The hook registrations, and the merge that must not eat other tools' hooks.
//!
//! Every entry invokes `yaadgaar hook <name>` — one binary, no scripts on disk.
//! That is D76's central point: the Python version shipped a directory of hook
//! scripts, nix's home-manager kept its own copies of the same handlers, the two
//! diverged on `project_id`, and the capture pipeline was dead for six days
//! while every signal still read healthy. A single binary has no second copy to
//! diverge from, and renaming a handler is not a filesystem migration.
//!
//! [`MANAGED_HOOKS`] is the one list. `hook.rs` dispatches on the same `name`
//! strings, so a handler that is registered and a handler that exists cannot
//! drift apart without the compiler or [`crate::install::verify`] noticing.

use std::path::Path;

use serde_json::{json, Value};

use super::jsonfile::ensure_object;
use super::shellword::{shell_quote, shell_split};

/// The command's own name, and the identity of a yadgar-managed entry.
///
/// `yaadgaar`, NOT `yadgar`, and the doubled vowels are not a typo. The Python
/// client this replaces already owns `yadgar` on PATH; two projects installing
/// the same executable name collide there and cannot coexist, so the
/// transitional client differs in BOTH the distribution and the executable.
/// `[[bin]] name` in `Cargo.toml` says the same word, and the two must not
/// drift: if they do, every hook this installer writes names a command that
/// does not exist AND is invisible to its own [`strip`], [`merge`] and
/// [`crate::install::verify`] — a dead pipeline while every signal reads
/// healthy, which is the whole of D76.
pub const BINARY_NAME: &str = "yaadgaar";

/// The subcommand every hook entry invokes.
const HOOK_VERB: &str = "hook";

/// The one key in `settings.json` that yadgar creates itself.
///
/// Named because [`crate::install::jsonfile::write_or_prune`] has to be TOLD
/// which key in a file is structural: emptying that one is emptying the file,
/// and every other key there is a name somebody chose. Reading a key somebody
/// chose as structure is what deleted a `settings.json` holding nothing but
/// its owner's `permissions`.
pub const HOOKS_KEY: &str = "hooks";

/// One registration in `settings.json`.
pub struct HookSpec {
    /// The Claude Code event key.
    pub event: &'static str,
    /// The argument passed to `yaadgaar hook`, and the handler's name.
    pub name: &'static str,
    /// Claude Code's tool/source matcher. Empty means "every one".
    pub matcher: &'static str,
    /// Claude Code's `async` flag: fire and forget, the session does not wait.
    /// No current registration sets it; kept so a handler that must not block
    /// (a compaction drain, say) can come back without a schema change.
    pub fire_and_forget: bool,
}

/// THREE registrations: the session-start task listing and the two write-back
/// handlers.
///
/// `session-start` (ADR-0992) prints the project's open tasks into the session
/// before its first prompt. Matcher `""` is every source — startup, resume,
/// clear and compact — so a compacted session is told its tasks again. It is
/// synchronous, because the listing must be context before the prompt, and it
/// bounds its own wait (see `hook::session_start::DEADLINE`).
///
/// `task-update` (`PreToolUse`, matcher `TaskUpdate`) and `task-completed`
/// (`TaskCompleted`, which takes no matcher) write an agent's edit to a
/// mirrored task back to yaadgaar before it lands (ADR-1000). Both are
/// synchronous: a refusal must come before the change, or it is no refusal.
///
/// Up to 0.1.4 this list held twelve. Eleven of them needed hook endpoints the
/// gateway does not have, so they did nothing — while still spawning this
/// binary on every tool call, prompt, stop and compaction, beside the memory
/// engine's own hooks, and turning into eleven "command not found" errors per
/// event the day the binary path went stale. They come back one at a time, each
/// with the gateway endpoint that gives it something to do.
///
/// Nothing else is needed to retire the eleven: [`merge`] takes EVERY hook of
/// ours out of every event ("ours" is the invocation shape, see [`is_managed`],
/// not a handler name here) and puts fresh entries back only under events this
/// list names. So an `install` over 0.1.4 removes all twelve and writes these.
///
/// Up to 0.1.31 the list also held `pre-tool-guard`, a `PreToolUse` agent
/// guard. It was removed: it is harness control, not memory, so it is out of
/// this client's scope. The same rule retires its entry. `PreToolUse` is still
/// an event this list names, and [`place`] takes every hook of ours out of it
/// before the fresh `task-update` entry goes back, so an `install` over 0.1.31
/// leaves no `pre-tool-guard` entry behind.
pub const MANAGED_HOOKS: &[HookSpec] = &[
    HookSpec {
        event: "PreToolUse",
        name: crate::hook::write_back::TASK_UPDATE,
        matcher: "TaskUpdate",
        fire_and_forget: false,
    },
    HookSpec {
        event: "SessionStart",
        name: crate::hook::session_start::NAME,
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "TaskCompleted",
        name: crate::hook::write_back::TASK_COMPLETED,
        matcher: "",
        fire_and_forget: false,
    },
];

/// The command string written into `settings.json` for one spec.
pub fn command_for(binary: &Path, name: &str) -> String {
    format!(
        "{} {HOOK_VERB} {name}",
        shell_quote(&binary.to_string_lossy())
    )
}

/// Is this command string one of yadgar's own?
///
/// Identity is the SHAPE of the invocation — a binary called `yadgar` with
/// `hook` as its first argument — and deliberately not the full command string.
/// Keying on the full string would make a yadgar entry written by a previous
/// install, from a different path, look foreign: it would be preserved beside
/// the fresh one and both would fire. Keying on the shape collapses stale
/// entries and still leaves every foreign entry alone, including a foreign
/// command that merely mentions yadgar somewhere in a path.
pub fn is_managed(cmd: &str) -> bool {
    let argv = shell_split(cmd);
    argv.len() >= 2 && argv[1] == HOOK_VERB && binary_stem(&argv[0]) == Some(BINARY_NAME)
}

/// The executable's own name in a command, without directories or extension.
///
/// Splits on BOTH separators before asking `Path`, because `Path` splits on `\`
/// only when compiled for Windows. Without this, `C:\Program Files\yaadgaar.exe`
/// has no directory separators at all as far as a Linux build is concerned and
/// comes back whole, so identity would flip with the platform doing the READING
/// rather than the writing — and a `settings.json` is routinely read somewhere
/// else than it was written (WSL beside the Windows install, synced dotfiles, a
/// file pasted into a bug report). An install that does not recognise its own
/// hooks appends beside them instead of replacing them.
///
/// `Path::file_stem` still does the extension half, on a name with no
/// separators left in it, where it means the same thing on every platform.
pub(super) fn binary_stem(argv0: &str) -> Option<&str> {
    let name = argv0.rsplit(['/', '\\']).next()?;
    Path::new(name).file_stem()?.to_str()
}

/// The handler name a managed command dispatches, if it has one.
pub fn managed_name(cmd: &str) -> Option<String> {
    let argv = shell_split(cmd);
    is_managed(cmd).then(|| argv.get(2).cloned()).flatten()
}

/// The path a command actually runs: argv[0], with the quoting taken back off.
///
/// Through [`shell_split`], because a SECOND parser is a second set of rules to
/// disagree with the first. The one it replaces read up to the next quote
/// character, so `'/opt/don'\''t/yaadgaar' hook prompt-recall` — what
/// [`shell_quote`] emits for a path containing an apostrophe — came back as
/// `/opt/don`, and `verify` reported a healthy install as a missing binary.
pub fn command_path(cmd: &str) -> String {
    shell_split(cmd).into_iter().next().unwrap_or_default()
}

/// EVERY command string in a hook entry, in the order they will fire.
///
/// **`hooks` is an ARRAY and reading only `hooks[0]` is a real defect**, not a
/// simplification. Claude Code's schema allows several hooks under one matcher,
/// so an entry can hold a foreign command first and one of ours second. Judging
/// such an entry by its first hook alone calls the whole entry foreign: the
/// reinstall appends a second copy of our hook and the capture fires twice, the
/// uninstall skips the entry and leaves a live yadgar hook behind, and `verify`
/// — which reads through this same function — reports neither. That is the D76
/// failure mode exactly, a doubled-or-dead pipeline behind healthy signals.
pub fn entry_commands(entry: &Value) -> Vec<&str> {
    let Some(hooks) = entry.get("hooks").and_then(Value::as_array) else {
        return Vec::new();
    };
    hooks
        .iter()
        .filter_map(|hook| hook.get("command").and_then(Value::as_str))
        .collect()
}

/// Register every managed hook, preserving every foreign one.
///
/// **Never `hooks[event] = [...]`.** That is how the Python version silently
/// discarded other tools' entries under the same key — on this machine, nix
/// writes a `SessionStart` entry and a caveman-mode `PostToolUse` one, and a
/// hard assignment deletes both without a word. Take out only what yadgar
/// owns, put the fresh entries where ours sat (see [`place`]), and leave the
/// rest exactly where it was.
///
/// Idempotent, and positionally stable: our entry is replaced AT THE INDEX it
/// already holds, so a second install writes back the same bytes. Removing it
/// and appending moved it behind any foreign entry that followed it, which
/// reordered the file under the other tool's feet. A first install appends. A
/// stale duplicate of ours collapses into the first one's position. An event
/// key we register is never removed and re-added either: that would move it to
/// the end of the `hooks` object.
///
/// Ours always goes into its OWN entry, even when a foreign entry already
/// carries the same matcher. Do not "tidy" that by appending into the existing
/// entry: sharing an entry is what couples our registration to somebody else's,
/// and an uninstall then has to edit a structure it does not own instead of
/// deleting one it does.
pub fn merge(settings: &mut Value, binary: &Path) {
    let Some(hooks) = ensure_object(settings, HOOKS_KEY) else {
        return;
    };
    // Events we still register are placed in situ below; every other event is a
    // retired one, and stripping it is the migration off an older install.
    let live: Vec<&str> = MANAGED_HOOKS.iter().map(|spec| spec.event).collect();
    strip_managed(hooks, &live);
    for event in live
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        let fresh = MANAGED_HOOKS
            .iter()
            .filter(|spec| spec.event == event)
            .map(|spec| entry_for(binary, spec))
            .collect();
        if let Some(array) = list.as_array_mut() {
            place(array, fresh);
        }
    }
}

/// Put *fresh* where our first entry in *array* sits, or at the end if none.
///
/// Our hooks come out of every entry that holds one. An entry left empty by
/// that goes; one still holding a foreign hook stays, and the fresh entries go
/// after it, so no foreign entry changes index relative to another.
fn place(array: &mut Vec<Value>, fresh: Vec<Value>) {
    let mut kept = Vec::with_capacity(array.len() + fresh.len());
    let mut at = None;
    for mut entry in array.drain(..) {
        let gone = strip_entry(&mut entry);
        if gone == 0 || survives(&entry) {
            kept.push(entry);
        }
        if gone > 0 && at.is_none() {
            at = Some(kept.len());
        }
    }
    let at = at.unwrap_or(kept.len());
    kept.splice(at..at, fresh);
    *array = kept;
}

/// Does this entry still hold a hook after ours came out of it?
///
/// Counted on the RAW `hooks` array, not on `entry_commands`: an http, prompt
/// or agent hook has no `command`, and counting commands called an entry
/// holding one of those empty and deleted it.
fn survives(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| !hooks.is_empty())
}

/// Remove every managed hook entry; returns how many were removed.
///
/// Reaches for the key rather than [`ensure_object`], and the difference is a
/// defect rather than a style: `ensure_object` CREATES `"hooks": {}` when it is
/// absent, so an uninstall on a machine with no `settings.json` wrote one — an
/// installer leaving behind a file that never existed, while removing nothing.
/// It also made "did this change anything?" unanswerable by comparison, because
/// the answer was always yes.
pub fn strip(settings: &mut Value) -> usize {
    let Some(hooks) = settings.get_mut(HOOKS_KEY).and_then(Value::as_object_mut) else {
        return 0;
    };
    strip_managed(hooks, &[])
}

/// Strip yadgar's hooks from every event, dropping what ends up empty.
///
/// Works one HOOK at a time rather than one entry at a time, so an entry
/// holding a foreign command beside ours keeps the foreign one and loses only
/// ours. An entry is dropped only when this function emptied it — an entry that
/// arrived with `"hooks": []` is somebody else's oddity and is left exactly as
/// it was found.
///
/// An emptied event key is residue this installer created; leaving
/// `"PreCompact": []` behind after an uninstall is the same class of untidiness
/// as leaving the hooks themselves. A key that still holds foreign entries is
/// never removed, because it is not empty.
///
/// Events named in *except* are skipped: [`merge`] places those in situ.
///
/// Returns how many hooks were removed — hooks, not entries, because one
/// registration is one hook and that is what the CLI reports.
fn strip_managed(hooks: &mut serde_json::Map<String, Value>, except: &[&str]) -> usize {
    let mut removed = 0;
    let mut emptied = Vec::new();
    for (event, list) in hooks.iter_mut() {
        if except.contains(&event.as_str()) {
            continue;
        }
        let Some(array) = list.as_array_mut() else {
            continue;
        };
        let before = array.len();
        array.retain_mut(|entry| {
            let gone = strip_entry(entry);
            removed += gone;
            // Keep the entry unless we took the last thing in it.
            gone == 0 || survives(entry)
        });
        if array.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        // `shift_remove`: `remove` is a swap_remove under `preserve_order`.
        hooks.shift_remove(&event);
    }
    removed
}

/// Remove yadgar's hooks from one entry; returns how many went.
fn strip_entry(entry: &mut Value) -> usize {
    let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
        return 0;
    };
    let before = hooks.len();
    hooks.retain(|hook| {
        !hook
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(is_managed)
    });
    before - hooks.len()
}

fn entry_for(binary: &Path, spec: &HookSpec) -> Value {
    let mut hook = json!({ "type": "command", "command": command_for(binary, spec.name) });
    if spec.fire_and_forget {
        hook["async"] = json!(true);
    }
    json!({ "matcher": spec.matcher, "hooks": [hook] })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paths a shell would mangle, and the reason quoting exists at all.
    ///
    /// A space is the DEFAULT case on Windows (`C:\Program Files\…`), and an
    /// apostrophe is the one that breaks a naive quoter, so both are pinned.
    const AWKWARD_PATHS: &[&str] = &[
        "/opt/my tools/yaadgaar",
        "/opt/don't/yaadgaar",
        r"C:\Program Files\yaadgaar\yaadgaar.exe",
        r"C:\Users\some one\yaadgaar.exe",
    ];

    #[test]
    fn a_command_written_for_an_awkward_path_is_still_read_back_as_ours() {
        // The failure this prevents: identity is what stops a reinstall
        // doubling the capture and an uninstall leaving it live, and it runs
        // through a quote-and-split round trip. A path with a space or an
        // apostrophe that does not survive that round trip is a machine where
        // every install adds one more copy of every hook.
        for path in AWKWARD_PATHS {
            let cmd = command_for(Path::new(path), "prompt-recall");
            assert!(is_managed(&cmd), "{path} -> {cmd}");
            assert_eq!(
                managed_name(&cmd).as_deref(),
                Some("prompt-recall"),
                "{path} -> {cmd}"
            );
            assert_eq!(command_path(&cmd), *path, "{path} -> {cmd}");
        }
    }

    #[test]
    fn a_foreign_command_at_an_awkward_path_is_left_alone() {
        // The other half: quoting that swallowed somebody else's command would
        // be worse than quoting that failed to recognise our own.
        for path in AWKWARD_PATHS {
            let cmd = format!("{} run --now", shell_quote(path));
            assert!(!is_managed(&cmd), "{cmd}");
        }
        assert!(!is_managed("'/opt/my tools/other' hook prompt-recall"));
    }

    #[test]
    fn a_path_is_quoted_for_the_shell_that_actually_parses_it() {
        // Pins the CONVENTION, which the round-trip tests above cannot. They
        // quote and split through this crate's own pair, so swapping both to
        // PowerShell's doubled `''` would keep them green while every hook
        // string in `settings.json` changed. This one names the bytes, so the
        // next person to reach for platform-conditional quoting lands on the
        // citation over `shell_quote` first.
        assert_eq!(
            command_for(
                Path::new(r"C:\Program Files\yaadgaar\yaadgaar.exe"),
                "prompt-recall"
            ),
            r"'C:\Program Files\yaadgaar\yaadgaar.exe' hook prompt-recall"
        );
        assert_eq!(
            command_for(Path::new("/opt/don't/yaadgaar"), "stop-checkpoint"),
            r"'/opt/don'\''t/yaadgaar' hook stop-checkpoint"
        );
        // An ordinary path is not quoted at all. Quoting everything would parse
        // identically and make every settings.json diff noisier than it is.
        assert_eq!(
            command_for(Path::new("/usr/local/bin/yaadgaar"), "prompt-recall"),
            "/usr/local/bin/yaadgaar hook prompt-recall"
        );
    }

    #[test]
    fn an_unquoted_path_is_still_recognised() {
        // What a person editing settings.json by hand writes.
        assert!(is_managed("/usr/local/bin/yaadgaar hook stop-checkpoint"));
        assert_eq!(
            command_path("/usr/local/bin/yaadgaar hook stop-checkpoint"),
            "/usr/local/bin/yaadgaar"
        );
    }

    #[test]
    fn every_hook_in_an_entry_is_read_not_only_the_first() {
        // The defect this pins, at the level of the one function that had it.
        let entry = json!({ "matcher": "", "hooks": [
            { "type": "command", "command": "/usr/bin/other-tool run" },
            { "type": "command", "command": "/usr/local/bin/yaadgaar hook stop-checkpoint" }
        ]});
        assert_eq!(
            entry_commands(&entry),
            vec![
                "/usr/bin/other-tool run",
                "/usr/local/bin/yaadgaar hook stop-checkpoint"
            ]
        );
    }

    #[test]
    fn a_fire_and_forget_entry_carries_async_true_and_an_ordinary_one_omits_it() {
        // No registration sets `fire_and_forget` today, so this pins the shape
        // directly: `async` is a key Claude Code reads, present and true, and
        // absent rather than false on every other entry.
        let spec = HookSpec {
            event: "PreCompact",
            name: "pre-compact-drain",
            matcher: "",
            fire_and_forget: true,
        };
        assert_eq!(
            entry_for(Path::new("/usr/local/bin/yaadgaar"), &spec).to_string(),
            r#"{"matcher":"","hooks":[{"type":"command","command":"/usr/local/bin/yaadgaar hook pre-compact-drain","async":true}]}"#
        );
        let ordinary = HookSpec {
            fire_and_forget: false,
            ..spec
        };
        assert!(
            entry_for(Path::new("/usr/local/bin/yaadgaar"), &ordinary)["hooks"][0]
                .get("async")
                .is_none()
        );
    }

    #[test]
    fn an_entry_that_arrived_empty_is_not_treated_as_one_we_emptied() {
        // `strip` drops an entry only when it took the last hook out of it. An
        // entry somebody else wrote with no hooks in it is their oddity, and
        // deleting it would be this installer editing what it does not own.
        let mut settings = json!({ "hooks": { "Stop": [ { "matcher": "", "hooks": [] } ] } });
        assert_eq!(strip(&mut settings), 0);
        assert_eq!(
            settings["hooks"]["Stop"].as_array().map(Vec::len),
            Some(1),
            "{settings:#?}"
        );
    }
}

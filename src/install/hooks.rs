//! The hook registrations, and the merge that must not eat other tools' hooks.
//!
//! Every entry invokes `yadgar hook <name>` — one binary, no scripts on disk.
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

/// One registration in `settings.json`.
pub struct HookSpec {
    /// The Claude Code event key.
    pub event: &'static str,
    /// The argument passed to `yadgar hook`, and the handler's name.
    pub name: &'static str,
    /// Claude Code's tool/source matcher. Empty means "every one".
    pub matcher: &'static str,
    /// Claude Code's `async` flag: fire and forget, the session does not wait.
    pub fire_and_forget: bool,
}

/// Twelve registrations across ten event keys.
///
/// Two events carry two entries each and that is deliberate: `SessionStart`
/// fires once normally and once after a compaction (matcher `compact`), and the
/// two need different handlers because a rehydrate is not a fresh start.
/// `PostToolUse` has a generic capture plus a narrow matcher for yadgar's own
/// block writes.
pub const MANAGED_HOOKS: &[HookSpec] = &[
    HookSpec {
        event: "PreCompact",
        name: "pre-compact-drain",
        matcher: "",
        // The one fire-and-forget entry: a compaction must not block on a drain.
        fire_and_forget: true,
    },
    HookSpec {
        event: "SessionStart",
        name: "session-start-context",
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "SessionStart",
        name: "post-compact-rehydrate",
        matcher: "compact",
        fire_and_forget: false,
    },
    HookSpec {
        event: "PostToolUse",
        name: "post-tool-capture",
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "PostToolUse",
        name: "block-reflect",
        matcher: "mcp__yadgar__block_(create|update|delete|replace|append)",
        fire_and_forget: false,
    },
    HookSpec {
        event: "UserPromptSubmit",
        name: "prompt-recall",
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "PreToolUse",
        // Edit/Write/NotebookEdit are in the matcher because of a real incident:
        // an agent used Edit, not Bash, to add itself to the hook exceptions
        // file, pushed to a protected branch, then reverted the file to conceal
        // it. A Bash-only matcher never even routed that call to the guard.
        name: "pre-tool-guard",
        matcher: "Bash|Edit|Write|NotebookEdit",
        fire_and_forget: false,
    },
    HookSpec {
        event: "Stop",
        name: "stop-checkpoint",
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "SessionEnd",
        name: "session-end-capture",
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "InstructionsLoaded",
        name: "instructions-loaded",
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "SubagentStart",
        name: "subagent-start",
        matcher: "",
        fire_and_forget: false,
    },
    HookSpec {
        event: "FileChanged",
        name: "file-changed",
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
fn binary_stem(argv0: &str) -> Option<&str> {
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
/// hard assignment deletes both without a word. Strip only what yadgar owns,
/// append fresh, leave the rest exactly where it was.
///
/// Idempotent: a second install strips what the first one wrote and re-appends
/// it, so re-running produces byte-identical output rather than duplicates.
///
/// Ours always goes into its OWN entry, even when a foreign entry already
/// carries the same matcher. Do not "tidy" that by appending into the existing
/// entry: sharing an entry is what couples our registration to somebody else's,
/// and an uninstall then has to edit a structure it does not own instead of
/// deleting one it does.
pub fn merge(settings: &mut Value, binary: &Path) {
    let Some(hooks) = ensure_object(settings, "hooks") else {
        return;
    };
    strip_managed(hooks);
    for spec in MANAGED_HOOKS {
        let list = hooks.entry(spec.event).or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        if let Some(array) = list.as_array_mut() {
            array.push(entry_for(binary, spec));
        }
    }
}

/// Remove every managed hook entry; returns how many were removed.
pub fn strip(settings: &mut Value) -> usize {
    let Some(hooks) = ensure_object(settings, "hooks") else {
        return 0;
    };
    strip_managed(hooks)
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
/// Returns how many hooks were removed — hooks, not entries, because one
/// registration is one hook and that is what the CLI reports.
fn strip_managed(hooks: &mut serde_json::Map<String, Value>) -> usize {
    let mut removed = 0;
    let mut emptied = Vec::new();
    for (event, list) in hooks.iter_mut() {
        let Some(array) = list.as_array_mut() else {
            continue;
        };
        let before = array.len();
        array.retain_mut(|entry| {
            let gone = strip_entry(entry);
            removed += gone;
            // Keep the entry unless we took the last thing in it.
            gone == 0 || !entry_commands(entry).is_empty()
        });
        if array.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        hooks.remove(&event);
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

/// Quote a path for a command string that a shell will parse.
fn shell_quote(text: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Split a command string the way a shell would, enough to read argv[0..2].
///
/// Not a shell: no expansion, no operators. It exists so that identity survives
/// a quoted path — `'/opt/my tools/yadgar' hook prompt-recall` is yadgar's, and
/// a naive `split_whitespace` would call it somebody else's.
fn shell_split(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    started = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
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

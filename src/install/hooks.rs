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
const BINARY_NAME: &str = "yadgar";

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
    argv.len() >= 2
        && argv[1] == HOOK_VERB
        && Path::new(&argv[0])
            .file_stem()
            .is_some_and(|s| s == BINARY_NAME)
}

/// The handler name a managed command dispatches, if it has one.
pub fn managed_name(cmd: &str) -> Option<String> {
    let argv = shell_split(cmd);
    is_managed(cmd).then(|| argv.get(2).cloned()).flatten()
}

/// The command string of a hook entry, if it has the expected shape.
pub fn entry_command(entry: &Value) -> Option<&str> {
    entry
        .get("hooks")?
        .as_array()?
        .first()?
        .get("command")?
        .as_str()
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

/// Strip yadgar's entries from every event, dropping keys that end up empty.
///
/// An emptied key is residue this installer created; leaving `"PreCompact": []`
/// behind after an uninstall is the same class of untidiness as leaving the
/// hooks themselves. A key that still holds foreign entries is never removed,
/// because it is not empty.
fn strip_managed(hooks: &mut serde_json::Map<String, Value>) -> usize {
    let mut removed = 0;
    let mut emptied = Vec::new();
    for (event, list) in hooks.iter_mut() {
        let Some(array) = list.as_array_mut() else {
            continue;
        };
        let before = array.len();
        array.retain(|entry| !entry_command(entry).is_some_and(is_managed));
        removed += before - array.len();
        if array.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        hooks.remove(&event);
    }
    removed
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

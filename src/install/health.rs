//! `yadgar verify` — the only thing in the system that can see hook drift.
//!
//! The daemon cannot read `~/.claude/settings.json`, so no server-side signal
//! will ever report that the hooks are gone, pointing at a deleted path, or
//! firing twice. **Two separate incidents featured a dead pipeline while every
//! signal read healthy**, which is the whole reason this exists and the whole
//! reason it must be able to fail: a check that only ever prints reassuring
//! things is indistinguishable from no check.
//!
//! Safe to run unattended, on a timer: it reads files and nothing else. No
//! writes, no repair, no subprocess, no network. A health check that heals what
//! it finds is a health check that never reports anything.

use std::fmt;
use std::path::Path;

use serde_json::Value;

use super::{command, hooks, jsonfile, mcp, rules, Layout, MANAGED_HOOKS};

/// One way the installed state differs from what `install` would write.
#[derive(Debug)]
pub enum Drift {
    Unreadable {
        path: String,
        detail: String,
    },
    HookMissing {
        event: String,
        name: String,
    },
    HookDuplicated {
        event: String,
        name: String,
        count: usize,
    },
    HookUnknown {
        event: String,
        name: String,
    },
    CommandMissing {
        where_: String,
        path: String,
    },
    CommandEphemeral {
        where_: String,
        path: String,
        reason: String,
    },
    McpMissing,
    McpWrongShape {
        detail: String,
    },
    McpCarriesCredential,
    RulesMissing {
        path: String,
    },
    ReferenceMissing {
        path: String,
    },
}

impl fmt::Display for Drift {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, detail } => write!(f, "{path} cannot be read: {detail}"),
            Self::HookMissing { event, name } => {
                write!(f, "hook {event}/{name} is not registered")
            }
            Self::HookDuplicated { event, name, count } => {
                write!(
                    f,
                    "hook {event}/{name} is registered {count} times — it fires {count} times"
                )
            }
            Self::HookUnknown { event, name } => {
                write!(
                    f,
                    "hook {event}/{name} is registered but this version has no such handler"
                )
            }
            Self::CommandMissing { where_, path } => {
                write!(f, "{where_} points at {path}, which does not exist")
            }
            Self::CommandEphemeral {
                where_,
                path,
                reason,
            } => {
                write!(
                    f,
                    "{where_} points at {path}, which will not last: {reason}"
                )
            }
            Self::McpMissing => write!(f, "the yadgar MCP server is not registered"),
            Self::McpWrongShape { detail } => write!(f, "the yadgar MCP entry is wrong: {detail}"),
            Self::McpCarriesCredential => write!(
                f,
                "the yadgar MCP entry carries an Authorization header — a token at rest, \
                 left over from an older install. Re-run `yaadgaar install`."
            ),
            Self::RulesMissing { path } => write!(f, "the rules file {path} is missing"),
            Self::ReferenceMissing { path } => {
                write!(
                    f,
                    "{path} does not reference the yadgar rules file on its first line"
                )
            }
        }
    }
}

/// Report drift, and fail if there is any.
///
/// Prints its own report and returns `Err` when anything is wrong, so the
/// obvious `install::verify(&home)?` in `main` exits non-zero by construction —
/// rather than by somebody remembering to check a return value, which is the
/// same class of mistake as a check nobody schedules.
pub fn verify(home: &Path) -> anyhow::Result<()> {
    let found = drift(home);
    for line in report_lines(&found) {
        println!("{line}");
    }
    if found.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{} problem(s) with the yadgar install; run `yaadgaar install` to repair",
        found.len()
    )
}

/// The report `verify` prints, one line per finding — or one line saying so.
///
/// Separated from the printing so a test can read what a scheduled run puts in
/// front of somebody, which is otherwise only observable by capturing stdout.
///
/// Every line names `verify`, because `verify` is what produced it. They all
/// said `install` — on a machine where nothing was installed at all, which is
/// the case this command exists for, so the first thing somebody ever read from
/// it named the wrong command. The two also mean different things by a line:
/// install reports what it WROTE, verify reports what it FOUND.
pub(super) fn report_lines(found: &[Drift]) -> Vec<String> {
    let label = format!("{} verify", hooks::BINARY_NAME);
    if found.is_empty() {
        return vec![format!(
            "{label}: OK — {} hooks, MCP entry, rules file.",
            MANAGED_HOOKS.len()
        )];
    }
    found
        .iter()
        .map(|item| format!("{label}: DRIFT — {item}"))
        .collect()
}

/// Everything wrong with the installed state, as data.
///
/// Never returns an error: an unreadable settings file is itself drift, and a
/// scheduled check that panics reports nothing at all.
pub fn drift(home: &Path) -> Vec<Drift> {
    let layout = Layout::new(home);
    let mut found = Vec::new();
    check_hooks(&layout, &mut found);
    check_mcp(&layout, &mut found);
    check_rules(&layout, &mut found);
    found
}

fn check_hooks(layout: &Layout, found: &mut Vec<Drift>) {
    let settings = match jsonfile::load(&layout.settings()) {
        Ok(v) => v,
        Err(e) => {
            found.push(Drift::Unreadable {
                path: layout.settings().display().to_string(),
                detail: e.to_string(),
            });
            return;
        }
    };
    let registered = registered_hooks(&settings);

    for spec in MANAGED_HOOKS {
        let count = registered
            .iter()
            .filter(|(event, name, _)| event == spec.event && name == spec.name)
            .count();
        match count {
            0 => found.push(Drift::HookMissing {
                event: spec.event.to_string(),
                name: spec.name.to_string(),
            }),
            1 => {}
            n => found.push(Drift::HookDuplicated {
                event: spec.event.to_string(),
                name: spec.name.to_string(),
                count: n,
            }),
        }
    }

    for (event, name, path) in &registered {
        if !MANAGED_HOOKS
            .iter()
            .any(|s| s.event == event && s.name == name)
        {
            found.push(Drift::HookUnknown {
                event: event.clone(),
                name: name.clone(),
            });
        }
        check_command(path, &format!("hook {event}/{name}"), found);
    }
}

/// Every yadgar-managed hook in the file, as `(event, handler, binary path)`.
fn registered_hooks(settings: &Value) -> Vec<(String, String, String)> {
    let Some(events) = settings.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (event, list) in events {
        for entry in list.as_array().unwrap_or(&Vec::new()) {
            // EVERY hook in the entry, not only the first. A second yadgar
            // registration sitting behind a foreign hook in the same entry is
            // exactly the doubled pipeline this check exists to notice.
            for cmd in hooks::entry_commands(entry) {
                let Some(name) = hooks::managed_name(cmd) else {
                    continue;
                };
                out.push((event.clone(), name, hooks::command_path(cmd)));
            }
        }
    }
    out
}

fn check_mcp(layout: &Layout, found: &mut Vec<Drift>) {
    let config = match jsonfile::load(&layout.mcp_config()) {
        Ok(v) => v,
        Err(e) => {
            found.push(Drift::Unreadable {
                path: layout.mcp_config().display().to_string(),
                detail: e.to_string(),
            });
            return;
        }
    };
    let Some(entry) = config
        .get("mcpServers")
        .and_then(|s| s.get(mcp::SERVER_KEY))
    else {
        found.push(Drift::McpMissing);
        return;
    };
    // A credential in this file is the one thing that must never be here, and
    // verify is the only thing that would ever notice a legacy one.
    if entry.get("headers").is_some() {
        found.push(Drift::McpCarriesCredential);
    }
    if entry.get("type").and_then(Value::as_str) != Some("stdio") {
        found.push(Drift::McpWrongShape {
            detail: "it is not a stdio server".to_string(),
        });
    }
    match entry.get("command").and_then(Value::as_str) {
        Some(path) => check_command(path, "the MCP entry", found),
        None => found.push(Drift::McpWrongShape {
            detail: "it names no command".to_string(),
        }),
    }
}

/// A registered path is drift when it is gone, or when it was never going to last.
///
/// Deliberately NOT compared against this process's own path: verify may legitimately
/// run from a second copy of the binary, and a check that cries wolf on a
/// scheduled run is a check people turn off.
fn check_command(path: &str, where_: &str, found: &mut Vec<Drift>) {
    let candidate = Path::new(path);
    if !candidate.exists() {
        found.push(Drift::CommandMissing {
            where_: where_.to_string(),
            path: path.to_string(),
        });
        return;
    }
    if let Some(reason) = command::ephemeral_reason(candidate) {
        found.push(Drift::CommandEphemeral {
            where_: where_.to_string(),
            path: path.to_string(),
            reason,
        });
    }
}

fn check_rules(layout: &Layout, found: &mut Vec<Drift>) {
    if !layout.rules().exists() {
        found.push(Drift::RulesMissing {
            path: layout.rules().display().to_string(),
        });
    }
    // ANYWHERE in the file, not on the first line, and the difference matters on
    // exactly the machine that made this module careful. Where `CLAUDE.md` is a
    // nix symlink, `install` refuses and says to add the line to whatever
    // generates it — and whatever generates it puts the line where it likes.
    // Insisting on the first line would report drift forever at somebody who did
    // precisely what they were told, which is how a scheduled check becomes a
    // check nobody reads. The reference still goes FIRST when yadgar writes it;
    // an import is read wherever it sits.
    // An UNREADABLE CLAUDE.md is its own drift and must not be reported as a
    // missing reference: the two have different fixes, and telling somebody to
    // re-run `install` against a file nothing can read sends them in a circle.
    let reference = rules::reference_line(&layout.rules());
    match rules::has_reference(&layout.claude_md(), &reference) {
        Ok(true) => {}
        Ok(false) => found.push(Drift::ReferenceMissing {
            path: layout.claude_md().display().to_string(),
        }),
        Err(e) => found.push(Drift::Unreadable {
            path: layout.claude_md().display().to_string(),
            detail: e.to_string(),
        }),
    }
}

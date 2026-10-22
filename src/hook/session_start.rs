//! `yaadgaar hook session-start` — the open tasks, in front of the agent before
//! its first prompt (ADR-0992).
//!
//! ADR-0992 moves task tracking for `yadgarhq/*` projects to yaadgaar, and makes
//! a SessionStart hook that prints the open tasks a precondition of the cutover:
//! the memory engine stops seeding its own task list for those projects, and
//! without this hook a session would start knowing of no tasks at all.
//!
//! ## What reaches the agent
//!
//! ONE JSON OBJECT ON STDOUT, exit 0, every time:
//! `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":…}}`.
//! Claude Code adds `additionalContext` to the session's context (Claude Code
//! hooks reference, "SessionStart" decision control). Stderr on exit 0 is shown
//! to nobody, so the failure notice goes through the same channel as the list:
//! a notice the agent never sees cannot tell it to ask the user.
//!
//! ## It never stops a session
//!
//! This handler cannot refuse anything. Not logged in, no project, a gateway
//! that is down, slow, refusing or answering garbage, and a panic all end in
//! the same place: a short notice that the tasks could not be loaded, that the
//! agent must ask the user, and — on ADR-0992's terms — never a fall back to
//! another ledger. The exit status is 0 in every case.
//!
//! **THE CREDENTIAL NEVER APPEARS IN THE OUTPUT.** It travels in the
//! `Authorization` header via [`crate::proxy::ToolCaller`] and nowhere else, and
//! a config that cannot be read is reported by its existence, not its contents.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::{Config, ConfigError};
use crate::printable::printable;
use crate::proxy::{Outcome, ToolCaller};

/// The handler name, as `settings.json` invokes it and `MANAGED_HOOKS` lists it.
pub const NAME: &str = "session-start";

/// How long the whole listing may take, every page included.
///
/// TWELVE SECONDS, and the floor is the gateway's own cold path, not a guess:
/// an attestation cache miss costs up to 5 s and the task dial up to 4 s (see
/// the arithmetic beside `proxy::REQUEST_TIMEOUT`), so anything under ~9 s fails
/// the first session after the gateway's cache goes cold — on a healthy
/// gateway. The 3 s above that covers the TLS handshake and task's own answer.
/// It stays far under Claude Code's 60 s default hook timeout, and it is paid
/// only when the gateway is slow: a refused connection fails at once.
pub const DEADLINE: Duration = Duration::from_secs(12); // ADR-0569-EXCEPTION(CC): bounds the session-start wait; coupled to the gateway's cold tools/call path, stated above.

/// How many tasks are listed by name. The count covers the rest.
const SHOWN: usize = 10;

/// task-db's own maximum page (D56), so the count takes as few calls as it can.
const PAGE_SIZE: u64 = 500;

/// Pages read before the count is reported as a lower bound instead.
const MAX_PAGES: usize = 20;

/// A title longer than this is cut, so one task cannot fill the context.
const TITLE_MAX: usize = 120;

/// The statuses asked for, and the order they are shown in.
const SHOWN_ORDER: [&str; 3] = ["in_progress", "blocked", "open"];

/// One task, in the summary shape the gateway answers with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Row {
    pub number: i64,
    pub status: String,
    pub title: String,
}

/// Every open task the listing reached.
#[derive(Debug, PartialEq, Eq)]
pub struct Listing {
    pub rows: Vec<Row>,
    /// `false` when [`MAX_PAGES`] stopped the paging: the count is then a floor.
    pub complete: bool,
}

/// Why the tasks could not be loaded. Each says so in words a person can act on.
#[derive(Debug, thiserror::Error)]
pub enum Failure {
    #[error("not logged in (run `yaadgaar login`)")]
    NotLoggedIn,
    /// The path only: the file's contents may hold the credential.
    #[error("the saved login in {0} could not be read (run `yaadgaar login` again)")]
    BadConfig(String),
    #[error(
        "no project is named for {0}: no .yadgar/project-id was found walking up from it, \
         and it has no usable `origin` remote"
    )]
    NoProject(String),
    #[error("{0}")]
    Gateway(String),
    #[error("the gateway did not answer within {0} s")]
    TimedOut(u64),
}

/// The JSON object Claude Code reads from a SessionStart hook's stdout.
pub fn hook_output(context: &str) -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })
    .to_string()
}

/// The line that names the ledger. Said on success AND on failure, so an agent
/// that cannot load the tasks still knows where they live.
fn ledger_line(project: &str) -> String {
    format!(
        "Tasks for {} live in yaadgaar (mcp__yaadgaar__*).",
        printable(project)
    )
}

/// The context text for a listing that loaded.
pub fn render(project: &str, listing: &Listing) -> String {
    let mut rows = listing.rows.clone();
    // STABLE, so the gateway's own order (oldest first) holds within a status.
    rows.sort_by_key(|r| rank(&r.status));
    let count = |s: &str| rows.iter().filter(|r| r.status == s).count();
    let total = if listing.complete {
        format!("{} open tasks", rows.len())
    } else {
        format!(
            "At least {} open tasks (the listing stopped after {MAX_PAGES} pages)",
            rows.len()
        )
    };
    let mut out = vec![
        ledger_line(project),
        format!(
            "{total}: {} in_progress, {} blocked, {} open.",
            count("in_progress"),
            count("blocked"),
            count("open")
        ),
    ];
    if !rows.is_empty() {
        out.push(format!(
            "The first {} (in_progress, then blocked, then open):",
            rows.len().min(SHOWN)
        ));
    }
    for row in rows.iter().take(SHOWN) {
        out.push(format!(
            "- #{} [{}] {}",
            row.number,
            printable(&row.status),
            title(&row.title)
        ));
    }
    out.join("\n")
}

/// Where *status* sorts; anything unexpected goes last rather than vanishing.
fn rank(status: &str) -> usize {
    SHOWN_ORDER
        .iter()
        .position(|s| *s == status)
        .unwrap_or(SHOWN_ORDER.len())
}

/// A title made safe for the context: one line, no escapes, bounded length.
///
/// [`printable`] escapes a line break as `\n` text, so a title cannot forge a
/// second task line or an instruction on a line of its own.
fn title(raw: &str) -> String {
    let safe = printable(raw);
    if safe.chars().count() <= TITLE_MAX {
        return safe;
    }
    let cut: String = safe.chars().take(TITLE_MAX).collect();
    format!("{cut}…")
}

/// The context text when the tasks could not be loaded.
pub fn notice(project: Option<&str>, failure: &Failure) -> String {
    let mut out = Vec::new();
    if let Some(project) = project {
        out.push(ledger_line(project));
    }
    out.push(format!(
        "The open tasks could not be loaded from yaadgaar: {failure}."
    ));
    out.push(
        "Ask the user how to proceed before you track any work. Do not record tasks \
         in any other ledger instead."
            .to_string(),
    );
    out.join("\n")
}

/// Every open task for the caller's project, within *deadline*.
pub async fn list(caller: &ToolCaller, deadline: Duration) -> Result<Listing, Failure> {
    tokio::time::timeout(deadline, pages(caller))
        .await
        .unwrap_or(Err(Failure::TimedOut(deadline.as_secs())))
}

/// Follow `next_page_token` until it runs out or [`MAX_PAGES`] is reached.
async fn pages(caller: &ToolCaller) -> Result<Listing, Failure> {
    let mut rows = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut arguments = json!({
            "statuses": ["in_progress", "open", "blocked"],
            "fields": "summary",
            "page_size": PAGE_SIZE,
        });
        if let Some(t) = &token {
            arguments["page_token"] = json!(t);
        }
        let (mut page, next) = page(caller.call("find_tasks", arguments).await)?;
        rows.append(&mut page);
        match next {
            Some(next) => token = Some(next),
            None => {
                return Ok(Listing {
                    rows,
                    complete: true,
                })
            }
        }
    }
    Ok(Listing {
        rows,
        complete: false,
    })
}

/// One `find_tasks` answer: its rows, and the token for the next page if any.
fn page(outcome: Outcome) -> Result<(Vec<Row>, Option<String>), Failure> {
    let body = match outcome {
        Outcome::Answered(body) => body,
        Outcome::Unreachable(e) => {
            return Err(Failure::Gateway(format!(
                "the gateway could not be reached ({})",
                printable(&e)
            )))
        }
        Outcome::Rejected(reqwest::StatusCode::UNAUTHORIZED, _) => {
            return Err(Failure::Gateway(
                "the gateway refused the saved credential (run `yaadgaar login` again)".into(),
            ))
        }
        Outcome::Rejected(status, detail) => {
            let detail = detail.map(|d| format!(": {}", printable(&d)));
            return Err(Failure::Gateway(format!(
                "the gateway answered {status}{}",
                detail.unwrap_or_default()
            )));
        }
    };
    let malformed = || Failure::Gateway("the gateway's answer was not a task list".into());
    let value: Value = serde_json::from_str(&body).map_err(|_| malformed())?;
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        return Err(Failure::Gateway(format!(
            "the gateway refused find_tasks: {}",
            printable(message)
        )));
    }
    let result = value.get("result").ok_or_else(malformed)?;
    if result.get("isError") == Some(&Value::Bool(true)) {
        let text = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(Failure::Gateway(format!(
            "find_tasks failed: {}",
            printable(text)
        )));
    }
    let content = result.get("structuredContent").ok_or_else(malformed)?;
    let rows: Vec<Row> = serde_json::from_value(content.get("tasks").cloned().unwrap_or_default())
        .map_err(|_| malformed())?;
    let next = content
        .get("next_page_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    Ok((rows, next))
}

/// The context text, from everything the process read.
///
/// Every input is a PARAMETER — the payload, the config, the process's working
/// directory — so no test reads the real config or calls the live gateway.
/// The working directory is the payload's `cwd` (the session's directory, per
/// the hook contract), falling back to the process's own.
pub fn context_text(
    payload: Result<Value, String>,
    config: Result<Config, ConfigError>,
    process_cwd: Option<PathBuf>,
    deadline: Duration,
) -> String {
    let config = match config {
        Ok(config) => config,
        Err(ConfigError::Missing) => return notice(None, &Failure::NotLoggedIn),
        Err(_) => {
            let dir = printable(&crate::config::base_dir().display().to_string());
            return notice(None, &Failure::BadConfig(dir));
        }
    };
    let cwd = payload
        .ok()
        .and_then(|p| p.get("cwd").and_then(Value::as_str).map(PathBuf::from))
        .or(process_cwd)
        .unwrap_or_default();
    let client = match crate::trust::client(config.ca_pem(), Some(deadline)) {
        Ok(client) => client,
        Err(e) => return notice(None, &Failure::Gateway(printable(&e.to_string()))),
    };
    let caller = ToolCaller::new(client, config, &cwd);
    let Some(project) = caller.project().map(str::to_string) else {
        return notice(None, &Failure::NoProject(shown(&cwd)));
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let listing = match runtime {
        Ok(rt) => rt.block_on(list(&caller, deadline)),
        Err(e) => Err(Failure::Gateway(format!("could not start: {e}"))),
    };
    match listing {
        Ok(listing) => render(&project, &listing),
        Err(failure) => notice(Some(&project), &failure),
    }
}

fn shown(path: &Path) -> String {
    printable(&path.display().to_string())
}

/// The whole hook, as the process runs it: read everything, answer once.
pub fn run() -> String {
    let context = context_text(
        super::read_stdin(),
        Config::load(),
        std::env::current_dir().ok(),
        DEADLINE,
    );
    hook_output(&context)
}

/// The answer when the hook panicked: the same notice, never a refusal.
pub fn panicked(what: &str) -> String {
    let failure = Failure::Gateway(format!("the hook failed ({})", printable(what)));
    hook_output(&notice(None, &failure))
}

#[cfg(test)]
#[path = "tests/session_start.rs"]
mod tests;

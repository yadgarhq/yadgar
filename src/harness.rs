//! The agent harness's own task list, and what this client may do to it.
//!
//! yaadgaar is the source of truth for a project's tasks (ADR-1000). Claude
//! Code keeps a task list of its own, on disk, in a format it does not
//! document. Before this client writes one byte there, it checks that the list
//! still has the shape that was recorded from a real Claude Code run. A list
//! that does not match is left alone, and the session gets the banner only.
//!
//! The plan is `plans/harness-task-sync.md` in `yadgarhq/docs`. Built so far:
//!
//! - car 0, [`shape`]: the recorded shape and the check against it. The
//!   canary (`make canary`) runs a real `claude` and feeds what it wrote to
//!   the same check, through `yaadgaar harness check-shape`;
//! - car 1, [`adapter`], [`claude_code`] and [`scope`]: one adapter per
//!   harness, the Claude Code probe, and `harness.scope`. Read-only: the
//!   session-start banner gains one line saying what a sync WOULD do;
//! - car 2, [`plan`], [`record`], [`state`] and [`sync`]: the SessionStart
//!   sync. yaadgaar tasks in scope are written into the Claude Code list,
//!   updated, and deleted when they go terminal;
//! - car 3, [`write_back`]: an agent's `TaskUpdate` on a mirrored entry is
//!   written back to yaadgaar (the hooks are `hook::write_back`).

pub mod adapter;
pub mod claude_code;
pub mod plan;
pub mod record;
pub mod scope;
pub mod shape;
pub mod state;
pub mod sync;
pub mod write_back;

use std::path::PathBuf;

/// What the hook reads from its process environment, read ONCE at the
/// boundary. Every function below takes this rather than reading the
/// environment itself, so no test mutates the process-wide environment and no
/// test reads the real `~/.claude`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    /// `CLAUDE_CONFIG_DIR`.
    pub claude_config_dir: Option<PathBuf>,
    /// The home directory.
    pub home: Option<PathBuf>,
    /// `CLAUDE_CODE_TASK_LIST_ID`: a named list shared by every session that
    /// sets it.
    pub task_list_id: Option<String>,
    /// `CLAUDE_CODE_ENABLE_TODO_TOOLS`.
    pub enable_todo_tools: Option<String>,
    /// `CLAUDE_CODE_ENABLE_TASKS`.
    pub enable_tasks: Option<String>,
    /// `YAADGAAR_HARNESS_SYNC`: `off` stops every write.
    pub sync_switch: Option<String>,
    /// `XDG_STATE_HOME`: where the sync state lives (else `~/.local/state`).
    pub state_home: Option<PathBuf>,
}

impl Env {
    /// The real process environment.
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self {
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
            home: dirs::home_dir(),
            task_list_id: var("CLAUDE_CODE_TASK_LIST_ID"),
            enable_todo_tools: var("CLAUDE_CODE_ENABLE_TODO_TOOLS"),
            enable_tasks: var("CLAUDE_CODE_ENABLE_TASKS"),
            sync_switch: var(SYNC_SWITCH),
            state_home: std::env::var_os("XDG_STATE_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }
}

/// The kill switch. `off` stops every write to the harness list.
pub const SYNC_SWITCH: &str = "YAADGAAR_HARNESS_SYNC";

/// `yaadgaar harness ...`: checks on the agent harness's own task list.
#[derive(Debug, clap::Subcommand)]
pub enum HarnessCommand {
    /// Check a Claude Code task list directory against the recorded shape.
    ///
    /// Read-only. Exit 0 when every entry matches, and 1 with the first entry
    /// that does not. The canary runs this on the directory a real `claude`
    /// wrote; a person can run it on their own list to see why it is not
    /// synced.
    CheckShape {
        /// The list directory, e.g. `~/.claude/tasks/<session id>`.
        dir: PathBuf,
    },
}

/// Run one `harness` subcommand, printing its report.
pub fn run(command: HarnessCommand) -> anyhow::Result<()> {
    match command {
        HarnessCommand::CheckShape { dir } => {
            let shape = shape::recorded().map_err(anyhow::Error::msg)?;
            let listing = shape::check_dir(&dir, &shape)?;
            println!("{}", summary(&listing));
            Ok(())
        }
    }
}

/// One line saying what a directory that passed holds.
fn summary(listing: &shape::Listing) -> String {
    let tasks = match listing.ids.len() {
        1 => "1 task".to_string(),
        n => format!("{n} tasks"),
    };
    let hwm = listing.highwatermark.map_or_else(
        || "no highwatermark".to_string(),
        |h| format!("highwatermark {h}"),
    );
    let lock = if listing.lock {
        ".lock present"
    } else {
        "no .lock"
    };
    format!("matches the recorded shape: {tasks}, {hwm}, {lock}")
}

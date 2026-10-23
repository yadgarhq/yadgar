//! The agent harness's own task list, and what this client may do to it.
//!
//! yaadgaar is the source of truth for a project's tasks (ADR-1000). Claude
//! Code keeps a task list of its own, on disk, in a format it does not
//! document. Before this client writes one byte there, it checks that the list
//! still has the shape that was recorded from a real Claude Code run. A list
//! that does not match is left alone, and the session gets the banner only.
//!
//! The plan is `plans/harness-task-sync.md` in `yadgarhq/docs`. This module
//! holds car 0: the recorded shape and the check against it. The canary
//! (`make canary`) runs a real `claude` and feeds what it wrote to the same
//! check, through `yaadgaar harness check-shape`.

pub mod shape;

use std::path::PathBuf;

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

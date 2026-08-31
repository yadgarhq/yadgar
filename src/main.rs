//! The whole client surface, one binary (D76).
//!
//! Every subcommand lives here and dispatches into a module; there is no second
//! executable and no directory of scripts. That is not tidiness — the Python
//! client shipped hook scripts that home-manager then kept its own copies of,
//! the two diverged on `project_id`, and the capture pipeline was dead for six
//! days while every signal read healthy. A single binary at one stable path has
//! no second copy to diverge from.
//!
//! WHAT THIS FILE IS TODAY: the CLI surface and the module layout, and nothing
//! else. Every arm reports that its module is unwritten and names the file that
//! will own it. Fixing the surface first is deliberate — the subcommand names,
//! the argument shapes and the module boundaries are what everything else is
//! written against, and settling them in one place is cheaper than reconciling
//! five branches that each guessed.

mod config;
mod hook;
mod install;
mod login;
mod proxy;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "yaadgaar",
    version,
    about = "Log in once, then serve MCP to your agent by proxying to the gateway."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Obtain a credential from the gateway and register the MCP server.
    ///
    /// One command is the whole setup (D75): it asks for the gateway address and
    /// your credentials, stores both together, and writes the server entry into
    /// the agent client's USER-level configuration — once per machine, never per
    /// repository.
    Login,

    /// Run as a local stdio MCP server, forwarding to the gateway.
    ///
    /// This is what the agent spawns, not something to run by hand. It knows no
    /// tools: `tools/list` is forwarded and its answer returned verbatim, so a
    /// tool added at the gateway needs no client release (D75).
    Serve,

    /// Register hooks, the rules reference and the MCP entry, then check them.
    Install,

    /// Remove exactly what `install` added, and nothing else.
    Uninstall,

    /// Check that the installed environment is still what `install` left.
    ///
    /// Scheduled rather than remembered (D76). The daemon cannot see
    /// `~/.claude/settings.json`, so no server-side signal can ever report hook
    /// drift — and a check nobody runs is indistinguishable from no check.
    Verify,

    /// Dispatch one agent hook event.
    ///
    /// `settings.json` invokes `yadgar hook <event>` and this does the HTTP with
    /// the credential already in config. A hook is a dispatch, not a program.
    Hook {
        /// The event name the agent client passed, e.g. `SessionStart`.
        event: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // STDERR, and this is load-bearing rather than a preference. Under `serve`,
    // stdout IS the MCP transport: one log line written there is a frame the
    // agent cannot parse, and the failure reads as a broken protocol rather than
    // a misdirected logger.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // A default, because an unset RUST_LOG enables nothing at all.
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Each arm names the file that will own it. Deliberately `bail!` rather than
    // `todo!`: somebody who runs an unfinished subcommand should get a sentence,
    // not a panic and a backtrace.
    match Cli::parse().command {
        Command::Login => unwritten("login", "src/login.rs"),
        Command::Serve => unwritten("serve", "src/proxy.rs"),
        Command::Install => unwritten("install", "src/install.rs"),
        Command::Uninstall => unwritten("uninstall", "src/install.rs"),
        Command::Verify => unwritten("verify", "src/install.rs"),
        Command::Hook { event } => unwritten(&format!("hook {event}"), "src/hook.rs"),
    }
}

fn unwritten(command: &str, module: &str) -> anyhow::Result<()> {
    anyhow::bail!("`yadgar {command}` is not implemented yet — it will live in {module}")
}

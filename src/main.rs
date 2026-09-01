//! The whole client surface, one binary (D76).
//!
//! Every subcommand lives here and dispatches into a module; there is no second
//! executable and no directory of scripts. That is not tidiness — the Python
//! client shipped hook scripts that home-manager then kept its own copies of,
//! the two diverged on `project_id`, and the capture pipeline was dead for six
//! days while every signal read healthy. A single binary at one stable path has
//! no second copy to diverge from.

mod config;
mod hook;
mod install;
mod login;
mod proxy;

use std::io::Write as _;

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
    /// Obtain a credential from the gateway and store it with the address.
    ///
    /// It asks for the gateway address and your credentials and stores BOTH
    /// TOGETHER (D72) — a token without the address it belongs to fails at
    /// connect time rather than at login, by which point the person has
    /// forgotten which deployment they logged into.
    ///
    /// It does NOT register the MCP entry: `install` owns every file the agent
    /// client reads, so there is one command to undo and one to check. Claiming
    /// otherwise here would be a promise this arm does not keep.
    Login,

    /// Run as a local stdio MCP server, forwarding to the gateway.
    ///
    /// This is what the agent spawns. It knows no tools: `tools/list` is
    /// forwarded and its answer returned verbatim, so a tool added at the
    /// gateway needs no client release (D75).
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

    /// Dispatch one agent hook.
    ///
    /// `settings.json` invokes `yaadgaar hook <name>`, and the argument is the
    /// HANDLER NAME rather than the event: `SessionStart` carries two
    /// registrations wanting different behaviour, so an event-keyed dispatcher
    /// could not serve the registrations at all.
    Hook {
        /// The handler name from `install::MANAGED_HOOKS`, e.g.
        /// `pre-tool-guard`.
        name: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // STDERR, and this is load-bearing rather than a preference. Under `serve`,
    // stdout IS the MCP transport: a single log line written there is a frame
    // the agent cannot parse, and the failure looks like a broken protocol
    // rather than a misdirected logger.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // A default, because an unset RUST_LOG enables nothing at all.
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Command::Serve => proxy::serve(config::Config::load()?).await,

        Command::Install => report("installed", install::install(&home()?)?),
        Command::Uninstall => report("removed", install::uninstall(&home()?)?),
        // `verify` prints its own report and returns `Err` on drift, so this
        // exits non-zero without anyone remembering to check a return value.
        Command::Verify => install::verify(&home()?),

        Command::Login => {
            let dir = config::base_dir();
            let config = login::login(&dir).await?;
            println!(
                "logged in to {} — the credential is stored in {}",
                config.gateway_url(),
                dir.display()
            );
            println!("run `yaadgaar install` to register the hooks and the MCP entry");
            Ok(())
        }

        Command::Hook { name } => dispatch_hook(&name),
    }
}

/// Run one hook handler and answer the agent client in the format it honours.
///
/// EXITS rather than returns on a refusal, and the asymmetry is the point: a
/// handler that fails open leaves no trace, and a handler that refuses is the
/// only thing in this binary that ever exits non-zero.
fn dispatch_hook(name: &str) -> anyhow::Result<()> {
    match hook::run(name, &hook::read_stdin()) {
        // Silence. An allow that printed would add a line to the transcript on
        // every single tool call.
        hook::Decision::Allow => Ok(()),
        hook::Decision::Deny(reason) => {
            let body = hook::refusal(name, &reason);
            // FLUSHED BEFORE THE EXIT, and not for tidiness. `process::exit` runs
            // no destructors, so an unflushed line is simply lost — and a lost
            // body still blocks (status 2 is a blocking error on its own) while
            // reporting "No stderr output" as the reason. The refusal would work
            // and explain nothing.
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{body}");
            let _ = out.flush();
            drop(out);
            std::process::exit(hook::REFUSED_EXIT_CODE);
        }
    }
}

/// The home directory every managed path is derived from.
///
/// Read HERE and passed down, never read inside `install` — a module that reads
/// `$HOME` on its own is a module whose tests write into the real `~/.claude`.
fn home() -> anyhow::Result<std::path::PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine the home directory"))
}

/// Say what was touched, by path.
///
/// Named files rather than a count: the point of the report is that somebody can
/// go and look, and "3 hooks installed" tells them nothing about where.
fn report(verb: &str, s: install::Summary) -> anyhow::Result<()> {
    println!("{} {} hook(s) in {}", verb, s.hooks, s.settings.display());
    println!("{verb} the MCP entry in {}", s.mcp_config.display());
    println!("{verb} the rules file {}", s.rules.display());
    if s.claude_md_changed {
        println!("{verb} the reference line in CLAUDE.md");
    }
    Ok(())
}

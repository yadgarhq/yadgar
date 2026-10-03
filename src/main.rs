//! The whole client surface, one binary (D76).
//!
//! Every subcommand lives here and dispatches into a module; there is no second
//! executable and no directory of scripts. That is not tidiness — the Python
//! client shipped hook scripts that home-manager then kept its own copies of,
//! the two diverged on `project_id`, and the capture pipeline was dead for six
//! days while every signal read healthy. A single binary at one stable path has
//! no second copy to diverge from.

mod config;
mod enrolment;
mod hook;
mod install;
mod login;
mod project;
mod proxy;
mod scheme;
mod trust;

#[cfg(test)]
mod testserver;

use std::io::Write as _;

use anyhow::Context as _;
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
    Login {
        /// The gateway address, so this is not asked for at a prompt.
        #[arg(long)]
        gateway: Option<String>,

        /// The username, so this is not asked for at a prompt.
        #[arg(long)]
        username: Option<String>,

        /// Read the password from stdin instead of a concealed prompt
        /// (ledger 638), for a machine with no terminal to answer one.
        ///
        /// Requires `--gateway` and `--username` as well: without them, the
        /// two prompts this flag does not remove would read stdin themselves
        /// — the gateway address and the username would consume the very
        /// lines meant for the password.
        ///
        /// Refused when stdin is a terminal rather than a pipe or a file —
        /// unlike `docker login`, which accepts one. This client hides a
        /// typed password everywhere else; a real terminal read through this
        /// flag would echo it in plain sight instead.
        #[arg(long = "password-stdin", requires_all = ["gateway", "username"])]
        password_stdin: bool,
    },

    /// Redeem the enrolment token an admin gave you: set a password, learn your
    /// username, and store the gateway address the token already names.
    ///
    /// A SEPARATE COMMAND rather than a first run of `login`, and the token is
    /// why: it already carries the gateway address and, on a private
    /// deployment, the CA to trust for it (D73). `login` would have to ask for
    /// an address the blob already knows, from somebody who has never met this
    /// deployment and cannot check the answer. It also SETS a password rather
    /// than presenting one, so it asks twice — the admin never learns it, and a
    /// typo is a lockout with nobody left to ask.
    Enrol {
        /// The base64 blob, exactly as it was handed over.
        ///
        /// It is not a secret in the credential sense — it is single-use and
        /// expires in 24 hours — but it does reach the shell history, so
        /// `enrol` reads it from stdin when neither this nor `--token-file` is
        /// given.
        ///
        /// CONFLICTS WITH `--token-file`: two sources for the same blob is a
        /// contradiction the caller must resolve, not a precedence for this
        /// binary to silently pick one side of.
        #[arg(conflicts_with = "token_file")]
        token: Option<String>,

        /// Read the blob from this file instead of the argument or stdin.
        #[arg(long = "token-file")]
        token_file: Option<std::path::PathBuf>,

        /// Read the new password from stdin instead of a concealed prompt
        /// (ledger 638); there is no repeat prompt to compare it against.
        ///
        /// Requires the token as an argument or via `--token-file`: the
        /// interactive "paste the enrolment token" fallback also reads
        /// stdin, and this flag has already claimed stdin for the password.
        ///
        /// Refused when stdin is a terminal — see `login --password-stdin`
        /// for why.
        #[arg(long = "password-stdin")]
        password_stdin: bool,
    },

    /// Run as a local stdio MCP server, forwarding to the gateway.
    ///
    /// This is what the agent spawns. It knows no tools: `tools/list` is
    /// forwarded and its answer returned verbatim, so a tool added at the
    /// gateway needs no client release (D75).
    Serve,

    /// Register hooks, the rules reference and the MCP entry.
    ///
    /// Says what it CHANGED and nothing about what it did not, so a repair and
    /// a no-op do not read alike. It does not run `verify` afterwards, and the
    /// help text used to say it did.
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
                //
                // The marker sits on the READ ITSELF, so `git grep ADR-0569-EXCEPTION`
                // lands on the line that takes the fallback rather than on prose near it.
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")), // ADR-0569-EXCEPTION(LIB): `tracing_subscriber`'s own convention for an absent RUST_LOG, same disposition as the seven service binaries.
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Command::Serve => proxy::serve(config::Config::load()?).await,

        Command::Install => {
            // ADR-0511 mints the install id HERE. It is separate from the four
            // files `install` manages — it lives in yadgar's own config rather
            // than in anything the agent client reads — so it is minted beside
            // the install and not inside it, and a machine that has not logged
            // in yet simply has nothing to mint into.
            mint_instance();
            report(
                "installed",
                "nothing to do — the agent environment is already registered.",
                install::install(&home()?)?,
            )
        }
        Command::Uninstall => report(
            "removed",
            "nothing to do — none of yadgar's registrations were there.",
            install::uninstall(&home()?)?,
        ),
        // `verify` prints its own report and returns `Err` on drift, so this
        // exits non-zero without anyone remembering to check a return value.
        Command::Verify => install::verify(&home()?),

        Command::Login {
            gateway,
            username,
            password_stdin,
        } => {
            let dir = config::base_dir();
            let config = login::login(&dir, gateway, username, password_stdin).await?;
            println!(
                "logged in to {} — the credential is stored in {}",
                config.gateway_url(),
                dir.display()
            );
            println!("run `yaadgaar install` to register the hooks and the MCP entry");
            Ok(())
        }

        Command::Enrol {
            token,
            token_file,
            password_stdin,
        } => {
            let dir = config::base_dir();
            let blob = match blob_source(token, token_file, password_stdin)? {
                BlobSource::Argument(blob) => blob,
                BlobSource::File(path) => std::fs::read_to_string(&path).with_context(|| {
                    format!("could not read the enrolment token from {}", path.display())
                })?,
                BlobSource::StdinPrompt => {
                    // Read from stdin so the blob need not reach shell history.
                    print!("Paste the enrolment token: ");
                    std::io::stdout().flush()?;
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line
                }
            };
            let config = login::enrol(&dir, &blob, password_stdin).await?;
            // THE USERNAME IS SAID HERE AND NOWHERE ELSE. `auth/enrol` is the
            // only place the deployment ever tells a person what they are
            // called, and they need it to log in on any other machine.
            println!(
                "enrolled with {} as {}",
                config.gateway_url(),
                config.username().unwrap_or("(the gateway named nobody)")
            );
            println!("the credential is stored in {}", dir.display());
            println!("run `yaadgaar install` to register the hooks and the MCP entry");
            Ok(())
        }

        Command::Hook { name } => dispatch_hook(&name),
    }
}

/// Where `enrol`'s blob comes from, decided from the CLI's own arguments alone
/// (ledger 638) — no file is read and no stdin is touched.
///
/// PURE, for the reason every other CLI-argument rule in this binary is: the
/// decision is exercisable without a file, a pipe, or a terminal.
///
/// **THE ARGUMENT AND `--token-file` NEVER ARRIVE TOGETHER.** Clap's own
/// `conflicts_with` on `token` refuses that combination with a usage error
/// before this function is ever called, so choosing between them is not this
/// function's problem to solve — unlike `--password-stdin`, which is a
/// boolean with nothing to conflict with and has to be checked here.
#[derive(Debug)]
enum BlobSource {
    Argument(String),
    File(std::path::PathBuf),
    /// The pre-existing fallback: print a prompt, then read one line of stdin.
    ///
    /// NEVER CHOSEN when `password_stdin` is set — stdin has exactly one
    /// reader, and `--password-stdin` is already that reader.
    StdinPrompt,
}

fn blob_source(
    token: Option<String>,
    token_file: Option<std::path::PathBuf>,
    password_stdin: bool,
) -> anyhow::Result<BlobSource> {
    match (token, token_file) {
        (Some(token), None) => Ok(BlobSource::Argument(token)),
        (None, Some(file)) => Ok(BlobSource::File(file)),
        // Unreachable through the CLI (clap's `conflicts_with` refuses it
        // first), kept so this function stays total for whatever calls it
        // directly, e.g. its own tests.
        (Some(token), Some(_)) => Ok(BlobSource::Argument(token)),
        (None, None) if password_stdin => anyhow::bail!(
            "--password-stdin needs the enrolment token as an argument or via \
             --token-file; stdin is reserved for the password"
        ),
        (None, None) => Ok(BlobSource::StdinPrompt),
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

/// Mint this install's id, if there is a config to mint it into.
///
/// BEST EFFORT, and deliberately silent. A person who runs `install` before
/// `login` has no config yet, and failing the install over an id that `serve`
/// will mint anyway would refuse the whole agent environment for a header. The
/// id is minted exactly once whichever of the two gets there first.
fn mint_instance() {
    if let Ok(mut config) = config::Config::load() {
        if let Err(e) = config.ensure_instance() {
            tracing::warn!("could not record this install's id: {e}");
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

/// Say what was touched, by path — and NOTHING about what was not.
///
/// Named files rather than a count: the point of the report is that somebody can
/// go and look, and "3 hooks installed" tells them nothing about where.
///
/// Every line is gated on that file having actually changed. A second install
/// printed all four unconditionally — over files a test proves do not move a
/// byte on a reinstall — so a repair and a no-op read exactly alike, and the
/// person cannot tell whether anything was wrong. The `CLAUDE.md` line was
/// already gated, which is the whole argument that the others can be.
///
/// *nothing* is said when no line was: a command that prints nothing at all
/// reads as a command that did not run.
fn report(verb: &str, nothing: &str, s: install::Summary) -> anyhow::Result<()> {
    for line in report_lines(verb, nothing, &s) {
        println!("{line}");
    }
    Ok(())
}

/// The lines [`report`] prints, as data.
///
/// Separated from the printing for the same reason `health::report_lines` was:
/// while this was one function ending in `println!`, NOTHING covered it.
/// Deleting the `settings_changed` gate so its line always prints, INVERTING
/// ALL FOUR GATES — the whole of the fix this report exists for, exactly
/// reversed — and deleting the "nothing to do" block were each measured
/// against the suite, and each left it green. The report is what somebody
/// reads to tell a repair from a no-op, and a report nothing can read is a
/// report nothing can check.
fn report_lines(verb: &str, nothing: &str, s: &install::Summary) -> Vec<String> {
    let mut lines = Vec::new();
    if s.settings_changed {
        lines.push(format!(
            "{} {} hook(s) in {}",
            verb,
            s.hooks,
            s.settings.display()
        ));
    }
    if s.mcp_changed {
        lines.push(format!(
            "{verb} the MCP entry in {}",
            s.mcp_config.display()
        ));
    }
    if s.rules_changed {
        lines.push(format!("{verb} the rules file {}", s.rules.display()));
    }
    if s.claude_md_changed {
        lines.push(format!("{verb} the reference line in CLAUDE.md"));
    }
    if lines.is_empty() {
        lines.push(nothing.to_string());
    }
    lines
}

#[cfg(test)]
mod tests;

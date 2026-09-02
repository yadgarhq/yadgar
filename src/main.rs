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
mod trust;

#[cfg(test)]
mod testserver;

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
        /// `enrol` reads it from stdin when the argument is omitted.
        token: Option<String>,
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
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
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

        Command::Enrol { token } => {
            let dir = config::base_dir();
            let blob = match token {
                Some(blob) => blob,
                None => {
                    // Read from stdin so the blob need not reach shell history.
                    print!("Paste the enrolment token: ");
                    std::io::stdout().flush()?;
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line
                }
            };
            let config = login::enrol(&dir, &blob).await?;
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
mod tests {
    use super::*;

    /// Sets exactly one of the four "this file changed" flags.
    type RaiseOneFlag = fn(&mut install::Summary);

    /// A summary with every flag off, and paths that name themselves.
    fn summary() -> install::Summary {
        install::Summary {
            hooks: 12,
            settings: std::path::PathBuf::from("/home/x/.claude/settings.json"),
            mcp_config: std::path::PathBuf::from("/home/x/.claude.json"),
            rules: std::path::PathBuf::from("/home/x/.claude/yadgar-rules.md"),
            settings_changed: false,
            mcp_changed: false,
            rules_changed: false,
            claude_md_changed: false,
        }
    }

    #[test]
    fn a_run_that_changed_nothing_says_so_and_nothing_else() {
        // Two failures at once. A command that prints NOTHING AT ALL reads as a
        // command that did not run, and deleting that block was invisible. A
        // gate deleted so its line always prints makes a no-op claim work
        // nobody did, which is the whole of what this report is for.
        let lines = report_lines("installed", "nothing to do", &summary());
        assert_eq!(lines, vec!["nothing to do".to_string()]);
    }

    #[test]
    fn every_line_is_gated_on_its_own_file_having_changed() {
        // ONE FLAG AT A TIME, so no gate can be deleted, inverted, or wired to
        // another flag without a failure. Inverting all four together was
        // measured and left the suite green: the fix this PR exists for, fully
        // reversed and undetected, because nothing read the report.
        let cases: [(RaiseOneFlag, &str); 4] = [
            (|s| s.settings_changed = true, "12 hook(s)"),
            (|s| s.mcp_changed = true, "the MCP entry"),
            (|s| s.rules_changed = true, "the rules file"),
            (|s| s.claude_md_changed = true, "the reference line"),
        ];
        for (set, expected) in cases {
            let mut s = summary();
            set(&mut s);
            let lines = report_lines("removed", "nothing to do", &s);
            assert_eq!(lines.len(), 1, "{expected}: {lines:#?}");
            assert!(lines[0].contains(expected), "{lines:#?}");
            assert!(lines[0].starts_with("removed "), "{lines:#?}");
        }
    }

    #[test]
    fn a_run_that_changed_everything_names_every_file_by_path() {
        // The point of the report is that somebody can go and look, so the
        // paths are the payload — "3 hooks installed" says nothing about where.
        // And the "nothing to do" line must not appear beside them.
        let mut s = summary();
        s.settings_changed = true;
        s.mcp_changed = true;
        s.rules_changed = true;
        s.claude_md_changed = true;
        let lines = report_lines("installed", "nothing to do", &s);
        assert_eq!(lines.len(), 4, "{lines:#?}");
        assert!(
            lines[0].contains("/home/x/.claude/settings.json"),
            "{lines:#?}"
        );
        assert!(lines[1].contains("/home/x/.claude.json"), "{lines:#?}");
        assert!(
            lines[2].contains("/home/x/.claude/yadgar-rules.md"),
            "{lines:#?}"
        );
        assert!(lines[3].contains("CLAUDE.md"), "{lines:#?}");
        assert!(
            !lines.iter().any(|l| l.contains("nothing to do")),
            "{lines:#?}"
        );
    }
}

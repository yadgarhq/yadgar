//! What the commands actually PRINT, read off the built binary's stdout.
//!
//! **An extraction made for testability is not wired by being extracted.** Both
//! reports in this client were pulled out of their `println!` so a test could
//! read them, and both were then tested at the wrong end: `health::report_lines`
//! had its text pinned while deleting `verify`'s loop over it — so `verify`
//! printed NOTHING AT ALL — left the suite green, and `main::report` was one
//! private function ending in `println!` that nothing covered at all. A unit
//! test on the lines proves the lines; only the process proves the caller.
//!
//! So this runs the real binary, with `HOME` pointed at a scratch directory.
//! `install` is deliberately NOT exercised here: it goes through
//! `resolve_durable_command`, which judges the path of the binary running the
//! test — green in an ordinary checkout, refused inside `.claude/worktrees` —
//! and a test whose colour depends on where somebody cloned the repository is
//! worse than no test. `uninstall` and `verify` call neither, and between them
//! they reach every line either report can print.
//!
//! The fixtures are literal TEXT rather than built through the crate: this is a
//! binary-only package with no library target, so nothing here can call
//! `hooks::merge`. Writing the JSON by hand is also the more honest fixture —
//! it is what is actually on a machine, rather than what this crate believes it
//! wrote there.
//!
//! **`#![cfg(unix)]`, and the reason is a destroyed machine rather than a
//! platform difference.** These are the only tests in this repository that go
//! through `main::home()`, and therefore through `dirs::home_dir()`. On Unix
//! that reads `$HOME`, so the binary can be pointed at a scratch directory. On
//! Windows it is `known_folder_profile()` — `SHGetKnownFolderPath` with
//! `FOLDERID_Profile` — which reads NEITHER `HOME` nor `USERPROFILE`. Running
//! these there would run a real `yaadgaar uninstall` against the developer's own
//! `~/.claude`: their hooks stripped, their MCP entry removed, their rules file
//! deleted. `install/tests/mod.rs` states the invariant these would break — a
//! test that writes into somebody's live settings has already failed at the
//! thing this module is for — and it held until now only because every other
//! test takes the home as a parameter.
//!
//! Setting `USERPROFILE` as well would not help and would be worse: it buys
//! nothing against the known-folder API, and leaves a test that LOOKS hermetic.
//! What both reports SAY is platform-independent and covered by the
//! `report_lines` unit tests, which run everywhere; only the wiring is gated.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh scratch home directory, with `.claude` already in it.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("yaadgaar-cli-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).expect("scratch home");
    dir
}

/// Run the built binary with *home* as `HOME`, and give back what it said.
fn run(home: &Path, subcommand: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .arg(subcommand)
        // HOME, not the current directory: `main::home()` is the one place that
        // reads it, and everything below is derived from what it returns.
        .env("HOME", home)
        .output()
        .expect("the binary under test did not run")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout was not UTF-8")
}

/// Everything `install` writes, written by hand.
///
/// The hook command matches `hooks::is_managed` — a binary whose stem is
/// `yaadgaar`, with `hook` as its first argument — and the reference line is
/// the absolute `@` import `rules::reference_line` builds.
fn seed_an_installed_home(home: &Path) {
    let claude = home.join(".claude");
    let rules = claude.join("yadgar-rules.md");
    std::fs::write(
        claude.join("settings.json"),
        r#"{"model":"opus","hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"/usr/local/bin/yaadgaar hook stop-checkpoint"}]}]}}"#,
    )
    .unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"numStartups":41,"mcpServers":{"yadgar":{"type":"stdio","command":"/usr/local/bin/yaadgaar","args":["serve"]}}}"#,
    )
    .unwrap();
    std::fs::write(&rules, "# the rules yadgar owns\n").unwrap();
    std::fs::write(
        claude.join("CLAUDE.md"),
        format!("@{}\n# my own instructions\n", rules.display()),
    )
    .unwrap();
}

#[test]
fn verify_prints_its_report_and_exits_non_zero() {
    // The wiring `health::report_lines` was extracted FOR. Deleting the loop in
    // `verify` — so a scheduled check prints nothing whatsoever and only its
    // exit status says anything — left the whole suite green.
    let home = scratch("verify-drift");
    std::fs::write(home.join(".claude").join("CLAUDE.md"), "# mine\n").unwrap();

    let output = run(&home, "verify");
    let said = stdout_of(&output);

    assert!(
        said.lines()
            .any(|l| l.starts_with("yaadgaar verify: DRIFT")),
        "verify reported nothing on a machine with no install: {said:?}"
    );
    assert!(
        said.contains("not registered"),
        "the findings themselves went unprinted: {said:?}"
    );
    assert!(
        !output.status.success(),
        "verify found drift and exited zero"
    );
}

#[test]
fn an_uninstall_that_removed_nothing_says_so_out_loud() {
    // A command that prints NOTHING AT ALL reads as a command that did not run.
    // Deleting the `nothing` block was measured and invisible.
    let home = scratch("uninstall-nothing");

    let output = run(&home, "uninstall");
    let said = stdout_of(&output);

    assert_eq!(
        said.trim_end(),
        "nothing to do — none of yadgar's registrations were there.",
        "an uninstall that removed nothing did not say so"
    );
    assert!(output.status.success(), "an uninstall of nothing failed");
}

#[test]
fn an_uninstall_names_every_file_it_actually_touched() {
    // The four gated lines, through the process that prints them. Inverting all
    // four gates — the fix this PR exists for, exactly reversed — left the
    // suite green, because nothing read a line of this report.
    let home = scratch("uninstall-reports");
    seed_an_installed_home(&home);

    let output = run(&home, "uninstall");
    let said = stdout_of(&output);
    let lines: Vec<&str> = said.lines().collect();

    assert_eq!(lines.len(), 4, "{said:?}");
    assert!(lines[0].starts_with("removed 1 hook(s) in "), "{said:?}");
    assert!(lines[0].ends_with("settings.json"), "{said:?}");
    assert!(
        lines[1].starts_with("removed the MCP entry in "),
        "{said:?}"
    );
    assert!(lines[1].ends_with(".claude.json"), "{said:?}");
    assert!(lines[2].starts_with("removed the rules file "), "{said:?}");
    assert!(lines[2].ends_with("yadgar-rules.md"), "{said:?}");
    assert_eq!(lines[3], "removed the reference line in CLAUDE.md");
    assert!(
        !said.contains("nothing to do"),
        "an uninstall that removed four things also claimed it did nothing: {said:?}"
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_uninstall_leaves_the_configs_of_a_machine_it_was_installed_on() {
    // The destructive probe, through the BINARY rather than through the library
    // — which is how it was found. Neither file below holds a scalar anywhere,
    // and the vacancy rule hunted for scalars, so an uninstall deleted both:
    // somebody's permissions configuration and a third party's MCP server.
    let home = scratch("uninstall-keeps-configs");
    let claude = home.join(".claude");
    std::fs::write(
        claude.join("settings.json"),
        r#"{"permissions":{"allow":[],"deny":[],"ask":[]},"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"/usr/local/bin/yaadgaar hook stop-checkpoint"}]}]}}"#,
    )
    .unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"other-server":{},"yadgar":{"type":"stdio","command":"/usr/local/bin/yaadgaar","args":["serve"]}}}"#,
    )
    .unwrap();

    let output = run(&home, "uninstall");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings = std::fs::read_to_string(claude.join("settings.json"))
        .expect("somebody's permissions configuration was deleted");
    assert!(settings.contains("permissions"), "{settings}");
    assert!(!settings.contains("stop-checkpoint"), "{settings}");

    let config = std::fs::read_to_string(home.join(".claude.json"))
        .expect("a third party's MCP registration was deleted");
    assert!(config.contains("other-server"), "{config}");
    assert!(!config.contains("\"yadgar\""), "{config}");
}

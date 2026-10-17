//! Unit tests for `main.rs`'s own pure helpers.
//!
//! Split out of `main.rs` when the file passed its size ceiling. Named
//! `tests.rs` deliberately, matching every other split like this one in this
//! crate (`src/login/tests/mod.rs`): a file under this name is exempt from
//! the line-count gate, and an inline `mod tests { ... }` in a production
//! file is not.

use super::*;

#[test]
fn token_file_is_used_when_no_argument_is_given() {
    let path = std::path::PathBuf::from("/some/token/file");
    assert!(matches!(
        blob_source(None, Some(path.clone()), false).unwrap(),
        BlobSource::File(f) if f == path
    ));
}

#[test]
fn no_flags_at_all_falls_back_to_the_interactive_prompt() {
    assert!(matches!(
        blob_source(None, None, false).unwrap(),
        BlobSource::StdinPrompt
    ));
}

#[test]
fn password_stdin_without_a_token_source_is_refused_before_reading_anything() {
    // Stdin has exactly one reader. Falling back to the "paste the token"
    // prompt here would make `--password-stdin` read the token instead of
    // the password, silently.
    let err = blob_source(None, None, true).expect_err("nothing to enrol with");
    assert!(
        err.to_string().contains("--token-file"),
        "the refusal did not say how to supply the token: {err}"
    );
}

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
        notices: Vec::new(),
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

#[cfg(unix)]
fn scratch_path_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "yadgar-path-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
#[test]
fn install_says_so_when_claude_code_is_not_on_path() {
    // Ledger 1547: on a machine with no Claude Code, `install` created
    // `~/.claude` and registered into it without a word, so the person had
    // no hint that nothing would ever read those files. One line, not a
    // failure: installing before Claude Code is a legitimate order.
    let empty = scratch_path_dir("empty");
    let note = claude_code_note(Some(empty.as_os_str()));
    let note = note.expect("an empty PATH has no Claude Code on it");
    assert!(note.contains("claude"), "{note}");
    assert!(note.contains("PATH"), "{note}");
    assert!(
        claude_code_note(None).is_some(),
        "an unset PATH has nothing on it"
    );
    std::fs::remove_dir_all(&empty).ok();
}

#[cfg(unix)]
#[test]
fn install_says_nothing_extra_when_claude_code_is_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let empty = scratch_path_dir("other");
    let bin = scratch_path_dir("bin");
    let claude = bin.join("claude");
    std::fs::write(&claude, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A PATH of several entries, the match in the LAST one.
    let path = std::env::join_paths([&empty, &bin]).unwrap();
    assert_eq!(claude_code_note(Some(&path)), None);
    std::fs::remove_dir_all(&empty).ok();
    std::fs::remove_dir_all(&bin).ok();
}

#[cfg(unix)]
#[test]
fn a_non_executable_file_named_claude_is_not_claude_code() {
    // What a shell would refuse to run is not on PATH in any useful sense.
    let bin = scratch_path_dir("noexec");
    std::fs::write(bin.join("claude"), "not a program").unwrap();
    assert!(claude_code_note(Some(bin.as_os_str())).is_some());
    std::fs::remove_dir_all(&bin).ok();
}

#[test]
fn a_notice_is_printed_and_replaces_the_nothing_to_do_line() {
    // A symlinked `CLAUDE.md` without the reference line: everything else is
    // already installed, and the one thing left is for the person to do. Saying
    // "nothing to do" beside that would be false; saying nothing would hide it.
    let mut s = summary();
    s.notices
        .push("add this line: @/home/x/.claude/yadgar-rules.md".to_string());
    let lines = report_lines("installed", "nothing to do", &s);
    assert_eq!(
        lines,
        vec!["add this line: @/home/x/.claude/yadgar-rules.md".to_string()]
    );

    s.rules_changed = true;
    let lines = report_lines("installed", "nothing to do", &s);
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert!(lines[0].contains("the rules file"), "{lines:#?}");
    assert!(lines[1].contains("add this line"), "{lines:#?}");
}

#[test]
fn issue_enrolment_never_prints_the_token_unless_asked_where_to_put_it() {
    // Ledger 1552: the token is a single-use secret. With neither sink named
    // the command is a usage error, before anything is sent or minted.
    let parse = |args: &[&str]| Cli::try_parse_from(args).map(|_| ());
    let base = ["yaadgaar", "admin", "issue-enrolment", "--user-id", "u-1"];
    assert!(parse(&base).is_err());
    assert!(parse(&[&base[..], &["--stdout"]].concat()).is_ok());
    assert!(parse(&[&base[..], &["--out", "/tmp/t"]].concat()).is_ok());
    assert!(parse(&[&base[..], &["--out", "/tmp/t", "--stdout"]].concat()).is_err());
}

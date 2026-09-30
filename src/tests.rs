//! Unit tests for `main.rs`'s own pure helpers.
//!
//! Split out of `main.rs` when the file passed its size ceiling. Named
//! `tests.rs` deliberately, matching every other split like this one in this
//! crate (`src/login/tests/mod.rs`): a file under this name is exempt from
//! the line-count gate, and an inline `mod tests { ... }` in a production
//! file is not.

use super::*;

#[test]
fn a_token_argument_wins_over_token_file() {
    assert!(matches!(
        blob_source(
            Some("the-argument".to_string()),
            Some(std::path::PathBuf::from("/ignored")),
            false
        )
        .unwrap(),
        BlobSource::Argument(t) if t == "the-argument"
    ));
}

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

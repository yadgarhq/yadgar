//! The exceptions-file check's reported false positives (yaadgaar #404, #402,
//! #267) and shape (c), under ADR-0971 / ADR-1005: a refusal is relaxed only
//! through a listed shape, and each shape ships with a bypass test.

use super::*;

fn exceptions_reason(command: &str) -> String {
    let reason = reason_for(&bash_call(command));
    assert!(
        reason.starts_with(EXCEPTIONS),
        "{command}: refused under another rule: {reason}"
    );
    reason
}

/// #404 command 1. `$S` is not proven on this line: `source` runs a file
/// that may set it, and `$(…)` turns the literal relaxation off. So the
/// refusal is intended, and its message names the word it could not prove.
const ISSUE_404_SCRATCH_LOOP: &str = "S=/tmp/claude-1000/x/scratchpad; cd /home/u/wt; source $S/env.sh; mkdir -p $S/bak; for f in src/hook/write_back.rs src/harness/sync.rs; do cp $f $S/bak/$(echo $f | tr / _); done";

#[test]
fn an_unprovable_word_stays_refused_and_the_refusal_names_it() {
    let reason = exceptions_reason(ISSUE_404_SCRATCH_LOOP);
    assert!(reason.contains("`$S/bak/$`"), "{reason}");
    // A line that names the file says so, rather than quoting a word.
    let reason = exceptions_reason("cat ~/.claude/yadgar-hook-exceptions.json");
    assert!(reason.contains("names the file"), "{reason}");
}

#[test]
fn the_named_word_is_bounded_and_printable() {
    let long = format!("cp x $D/{}/$f", "a".repeat(400));
    let reason = exceptions_reason(&long);
    assert!(reason.len() < EXCEPTIONS.len() + 300, "{}", reason.len());
    let reason = exceptions_reason("cp x $D/\u{1b}x/$f");
    assert!(!reason.contains('\u{1b}'), "{reason:?}");
}

#[test]
fn issue_404_command_2_is_allowed() {
    // Its `...` placeholders filled in; allowed on main already.
    let command = "cd /home/u/wt; python3 - <<'EOF'\np='README.md'\ns=open(p).read()\nopen(p,'w').write(s.replace('a','b'))\nEOF\nS=/tmp/s; source $S/env.sh; cargo fmt; git add -A; (cargo clippy && cargo test | grep x) 2>&1 | tail -12; timeout 1200 pre-commit run --all-files >$S/pc.log 2>&1; echo pc=$?; grep -A15 Failed $S/pc.log | head -40";
    assert_eq!(guard(&bash_call(command)), Decision::Allow);
}

/// #402: the endpoint of `gh api` is a URL path, never a file.
const GH_API_ENDPOINTS: &[&str] = &[
    "gh api repos/$r/git/refs/tags/$t",
    "gh-personal api repos/$r/git/ref/tags/$t --jq .object.sha",
    "for r in a:b c:d; do gh api repos/yadgarhq/${r%%:*}/git/ref/tags/${r#*:}; done",
    "for r in a b; do for t in v1 v2; do gh api repos/$r/git/refs/tags/$t; done; done",
    "gh api -X GET --paginate /repos/$r/tags",
    "gh api -H 'Accept: application/json' repos/{owner}/{repo}/git/refs/tags/$t",
    // The third report: a scratch file whose name holds `tags`.
    "S=/tmp/s; for r in a b; do echo $r >> $S/repo-tags.txt; done",
];

#[test]
fn a_gh_api_endpoint_is_not_a_file() {
    let refused: Vec<&&str> = GH_API_ENDPOINTS.iter().filter(|c| denies(c)).collect();
    assert!(refused.is_empty(), "refused: {refused:#?}");
}

/// Bypasses through the gh-api shape: each reaches the file through a word
/// that is not the endpoint, or through a command that is not plain `gh api`.
const GH_API_BYPASSES: &[&str] = &[
    "gh api repos/$r/x > $D/$f",
    "gh api repos/$r/x --input $D/$f",
    "gh api repos/$r/x; rm $D/$f",
    "command gh api $D/$f",
    "sudo gh api repos/$r/x $D/$f",
    "gh api $D/$f",
    "gh api ~/.claude/$f",
    "gh api $(echo ~/.claude)/y*",
    "gh api repos/`echo ~/.claude`/$f",
    "gh api --unknown-flag $D/$f repos/$r",
    "gh api repos/$r/x repos/$D/$f",
    "gh api repos/x/y | tee ~/.claude/yadgar-hook-exceptions.json",
    "gh api repos/~/.claude/yadgar-hook-exceptions.json",
];

#[test]
fn the_gh_api_shape_does_not_open_a_bypass() {
    let allowed: Vec<&&str> = GH_API_BYPASSES.iter().filter(|c| !denies(c)).collect();
    assert!(allowed.is_empty(), "allowed: {allowed:#?}");
}

#[test]
fn issue_267_a_heredoc_of_rust_test_code_is_allowed() {
    // Already fixed by ledger 1701's brace collapse: no new shape is needed.
    let command = "cat > src/hook/guard/tests/x.rs <<'EOF'\nfn a() {\n    let s = format!(\"{home}/{name}\", home = h, name = n);\n    for (a, b) in [(1, 2), (3, 4)] { assert_eq!(f(a, b), Some(vec![a, b])); }\n}\nEOF\ncargo test --quiet x";
    assert_eq!(guard(&bash_call(command)), Decision::Allow);
    // Not relaxed: `python3 -` runs its heredoc body, so a glob there is
    // still read (the bypass a `python3 -` shape would open).
    assert!(denies(
        "python3 - <<'EOF'\nimport os\nos.system(\"rm ~/.c*/y*\")\nEOF"
    ));
}

#[test]
fn shape_c_stays_refused_under_claude_worktrees() {
    // ADR-1005: a `$(…)` or unproven `$var` leaf under a literal directory.
    for command in [
        "cp x .claude/worktrees/$(echo agent-a)",
        "cp x /home/u/repo/.claude/worktrees/agent-a/$f",
        "echo x > .claude/worktrees/$W",
    ] {
        exceptions_reason(command);
    }
}

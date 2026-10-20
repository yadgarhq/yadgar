//! The exceptions-file rule on real commands it refused by mistake, and on
//! every way of writing the file a person could think of.
//!
//! The false refusals were measured: on one day the installed guard refused 46
//! ordinary Bash commands with the exceptions-file reason, and none of them
//! named the file. Three causes, one fixture each below:
//!
//! * a dir component that is ONLY an expansion or a wildcard (`$S/$f`,
//!   `*/node_modules/*`) was taken to stand for `.claude`, and a leaf of the
//!   same kind for the file;
//! * a backtick or `$(…)` span was cut at its `/` before it was read as one
//!   expansion, so Markdown like `` `adr/` `` in a heredoc body became `*/*`;
//! * Python or Rust `{a, b}` in a heredoc body was brace-expanded, and the
//!   product passed the bound at which the guard refuses rather than guess.

use super::*;

/// (a) A python heredoc that edits a test file. Its body is Rust doc comments
/// and Markdown, and nothing in it names the exceptions file.
const PYTHON_HEREDOC: &str = r#"cd /home/u/git/yadgar && python3 - <<'EOF'
p='src/install/tests/coexistence.rs'
s=open(p).read()
marker="/// Make *store* unreadable (mode 000)"
new='''/// A `settings.json` text exactly as `write_atomic` writes it, so a no-op
fn a_changed_entry_of_ours_is_replaced_at_its_index_and_foreign_ones_stay() {
    let seeded = settings_text(serde_json::json!([foreign_entry(), stale, foreign_entry()]));
    let (after, changed) = install_over("idem-update", &seeded);
        after,
            foreign_entry(),
            own_entry(GUARD_MATCHER),
}
fn a_first_install_appends_after_the_foreign_entries() {
    let (after, changed) = install_over("idem-first", &seeded);
        after,
        settings_text(serde_json::json!([foreign_entry(), own_entry(GUARD_MATCHER)]))
}
fn duplicate_entries_of_ours_collapse_into_the_first_one_position() {
        own_entry("Bash"),
        foreign_entry(),
    let (after, _) = install_over("idem-dupes", &seeded);
        after,
        settings_text(serde_json::json!([own_entry(GUARD_MATCHER), foreign_entry()]))
}

'''
s=s.replace(marker,new+marker,1)
open(p,'w').write(s)
EOF
timeout 900 cargo test --offline install::tests::coexistence 2>&1 | tail -40"#;

/// (b) `git fetch`, a heredoc writing a PR body, a push and `pr create`.
const PR_BODY_HEREDOC: &str = r#"cd /home/u/git/docs; git fetch origin --prune 2>&1|tail -1; git rev-list --count HEAD..origin/main
cat > /tmp/s/pr.md <<'EOF'
## What

Exports one decision record to `adr/`. The exporter writes `adr/LEDGER.md`
and the files under `adr/`; nothing else changes.

## Risk

Docs only. If wrong, revert this commit and re-run the exporter.
EOF
git push -u origin chore/adr-export 2>&1|tail -2
gh-personal pr create --repo yadgarhq/docs --base main --head chore/adr-export --title "docs: export to adr/" --body-file /tmp/s/pr.md"#;

/// (c) A `for` loop running `git show` into scratch files.
const GIT_SHOW_LOOP: &str = "S=/tmp/s; mkdir -p $S/pr; for f in $(git diff --name-only origin/main...02ba810 -- 'adr/ADR-*'); do git show 02ba810:$f > $S/pr/$(basename $f | cut -c1-8).md; done; ls $S/pr | wc -l";

const MUST_BE_ALLOWED: &[&str] = &[
    PYTHON_HEREDOC,
    PR_BODY_HEREDOC,
    GIT_SHOW_LOOP,
    // Markdown inline code holding a `/`.
    "cat > /tmp/b.md <<'EOF'\nNo `local/` prefix; only `kubernetes.io/cluster/*` tags.\nEOF",
    "cat > /tmp/b.md <<'EOF'\nSplit on `/` only, as in `a/b`.\nEOF",
    // A loop variable takes the loop's words, not anything.
    "for w in agent-a agent-b; do git -C .claude/worktrees/$w status; done",
    // A `bash -c` script that starts with an assignment is ONE word, `S=…`,
    // to the line outside it; read as a binding, `$S` in it expanded into
    // copies of the whole script, past the bound.
    "bash -c 'S=/tmp/s; W=/tmp/w; T=$S/live; rm -rf $T; mkdir -p $T/apps $T/scripts/gates; cd $W; for f in apps/a.yaml scripts/pin.json scripts/gates/r.sha256; do git show 36059be:$f > $T/$f; done; TOKEN=$(gh auth token) timeout 600 python3 scripts/pin.py --repo $T --check --evidence $S/ev.json > $S/body.md; echo exit=$?'",
    // A loop variable set from another one, on a line that spells `.claude`.
    "cd ~/x/.claude/worktrees/a && for d in infra apps; do for f in $d/*; do cat $f; done; done",
];

const MUST_BE_DENIED: &[&str] = &[
    // Every writer, on the literal path.
    "echo x >> ~/.claude/yadgar-hook-exceptions.json",
    "sed -i 's/a/b/' ~/.claude/yadgar-hook-exceptions.json",
    "perl -pi -e 's/a/b/' ~/.claude/yadgar-hook-exceptions.json",
    "tee -a ~/.claude/yadgar-hook-exceptions.json < /tmp/x",
    "install /tmp/x ~/.claude/yadgar-hook-exceptions.json",
    "mv -f /tmp/x ~/.claude/yadgar-hook-exceptions.json",
    // A heredoc into it, and a heredoc that is a script naming it.
    "cat > ~/.claude/yadgar-hook-exceptions.json <<'EOF'\n{}\nEOF",
    "cat <<EOF > \"$HOME/.claude/yadgar-hook-exceptions.json\"\n{}\nEOF",
    "python3 - <<'EOF'\nopen('/home/x/.claude/yadgar-hook-exceptions.json','w')\nEOF",
    "bash <<'EOF'\nrm ~/.c*/y*\nEOF",
    // Quoted paths, and paths through a variable.
    "cp /tmp/x \"/home/x/.claude/yadgar-hook-exceptions.json\"",
    "cp /tmp/x \"${HOME}/.claude/yadgar-hook-exceptions.json\"",
    "mv /tmp/x \"$HOME\"/.claude/yadgar-hook-exceptions.json",
    "cp /tmp/x \"$CLAUDE_CONFIG_DIR/yadgar-hook-exceptions.json\"",
    "cp /tmp/x \"$HOME/.claude/$F\"",
    "cp /tmp/x $CLAUDE_CONFIG_DIR/$F",
    "tee ${CLAUDE_DIR}/y*.json",
    // A variable set earlier on the same line, or by a loop.
    "F=~/.claude/yadgar-hook-exceptions.json; : > $F",
    "D=~/.claude; rm $D/*",
    "D=$HOME/.claude; cp /tmp/x $D/$F",
    "export D=~/.claude && cp /tmp/x \"$D/$F\"",
    "for d in ~/.claude; do rm $d/y*; done",
    "D=$(echo ~/.claude); rm $D/y*.json",
    // A command substitution that produces the directory.
    "rm \"$(echo ~/.claude)\"/y*.json",
    "rm `dirname ~/.claude/settings.json`/y*.json",
    // An expansion after `.claude` may hold `../`, and so may a literal `..`.
    "cp /tmp/x ~/.claude/sub/$F",
    "rm ~/.claude/x/../y*.json",
    "rm ~/.claude//y*.json",
    // Bracket classes spelling the directory.
    "rm ~/[.]claude/*.json",
    "rm ~/[.][c][l][a][u][d][e]/*.json",
    "rm ~/.claude/*",
    // DELIBERATE OVER-REFUSAL: a quoted pattern reads the same as an unquoted
    // glob once the shell removes the quotes, and `rm ./.claude/*` is a write.
    "find . -not -path './.claude/*'",
    // KEPT ON PURPOSE: `worktrees` may be a link back to `.claude`.
    "find . -name '*.md' -not -path '*/.claude/worktrees/*'",
    // KEPT, NO WEAKENING: a wildcard directory may be `.claude` under
    // `dotglob` or a program's own globbing (`find -path`), and a directory
    // after it may be a link back. Refused before this change, and still.
    "find . -name '*.json' -not -path '*/node_modules/*'",
    "find . -maxdepth 4 -iname 'pull_request_template*' -not -path '*/.venv/*'",
    "grep -n '^## \\|NEEDS-HUMAN.\\*\\*' body.md",
    "ls docs/runbooks/x/*/*",
];

/// Security review of this change: each one was refused before it, and a
/// first version of it let each through.
const BYPASSES: &[&str] = &[
    // A bracket class that matches `.claude` and also a one-letter name.
    "rm ~/[.0]*/y*.json",
    "rm ~/[!x]*/y*.json",
    // A directory between `.claude` and the leaf may be a link back to it.
    "ln -s . ~/.claude/s; rm ~/.claude/s/y*.json",
    "rm ~/.claude/s/y*.json",
    "rm ~/.claude/x/*",
    "rm ~/.claude/./y*.json",
    // A DECOY: a binding on the line that does not take effect where the
    // variable is read, so the variable keeps its value from the environment.
    "F=x true; cp /tmp/x ~/.claude/$F",
    "git -c F=x status; cp /tmp/x ~/.claude/$F",
    "(F=x); cp /tmp/x ~/.claude/$F",
    "false && F=x; cp /tmp/x ~/.claude/$F",
    "cp /tmp/x ~/.claude/$F; F=x",
    "cp /tmp/x ~/.claude/$F; for F in x; do :; done",
    "for F in; do :; done; cp /tmp/x ~/.claude/$F",
    "cat > /tmp/n <<'EOF'\nF=x\nEOF\ncp /tmp/x ~/.claude/$F",
    // A brace list with a quoted item holding whitespace is still expanded.
    "rm ~/{.claude,\" \"}/y*.json",
    "rm ~/.c{l,\" \"}aude/y*.json",
    // Command substitution, a nested shell, a subshell, `eval`.
    "rm $(echo ~/.claude)/y*.json",
    "D=~/.claude bash -c 'rm $D/y*'",
    "bash -c 'D=~/.claude; rm $D/y*'",
    "(D=~/.claude; rm $D/y*)",
    "D=~/.claude; eval \"rm $D/y*\"",
    "cd ~ && rm .claude/y*",
    // A wildcard directory with a leaf only the file is likely to match.
    "rm $D/y*.json",
    "rm */y*.json",
    "rm ?*/y*.json",
    "cp /tmp/x $D/*-exceptions.json",
    // Independent review of the second version. M1: a whitespace brace list
    // must keep the evidence of the items that hold none.
    "rm ~/{.*,\" \"}/*.json",
    "rm ~/{.cla?de,\"a b\"}/*.json",
    "rm ~/{.c*,' '}/*",
    "rm ~/{.c,\" \"}*/*.json",
    // M2: a span is not the literal text `.claude`.
    "rm ~/.${X:-claude}/*.json",
    "rm ~/.${X:-claude}/y*.json",
    "rm ${HOME}/.${N:-claude}/*",
    // M3: the body of a loop writes its variable another way.
    "for D in ~; do D+=/.cl; D+=aude; rm $D/y*.json; done",
    "for D in /tmp; do read D; rm $D/y*.json; done <<< ~/.claude",
    "for D in /tmp; do printf -v D %s/.cl%s ~ aude; rm $D/y*.json; done",
    // M4: a loop with no `done`, in a subshell or in a pipeline.
    "( for D in /tmp; { :; } ); rm $D/y*.json",
    "for D in /tmp; { :; } | cat; rm $D/y*.json",
    // M5: the directory built inside the call.
    "D=~; D+=/.cl; D+=aude; rm $D/*.json",
    "printf -v D %s/.cl%s ~ aude; rm $D/*.json",
    "D=$(printf %s/.cl%s ~ aude); rm $D/*.json",
    "D=$(echo ~/.cl?ude); rm $D/*.json",
    // M6: a nested shell's positional parameters.
    "sh -c 'rm $0/*.json' ~/.claude",
    "bash -c 'rm $1/*.json' _ ~/.c?aude",
    // M7: ANSI-C quoting.
    "rm ~/$'\\x2eclaude'/*.json",
    "rm ~/$'\\056claude'/*.json",
    // Found by the differential run: a backtick span the scanner cut at a
    // space, opening in one word and closing in the next.
    "rm ~/.claude/`echo y`*",
    "rm `echo ~/.claude`/`echo y`*",
    // Allowed before this change too: a substitution as a redirect target.
    "echo x > $(echo ~/.claude)/y*.json",
    // A quoted backtick does not open a span.
    "echo '`'; rm `echo ~/.claude`/y*.json",
    "echo \"`echo ~/.claude`\"/y*.json; rm `echo ~/.claude`/`echo y`*",
    // dotglob, and a `cd` that changes `$PWD`.
    "rm ~/[!y]*/*.json",
    "cd ~/.cl?ude; rm $PWD/*.json",
    // Variables the line does not set: with no environment to resolve them
    // against (these tests give the guard none), they refuse.
    "rm $D/$f",
    "rm $D/*.json",
    "rm */*",
    "cp /tmp/x $D/${f}",
    "rm $D/*s.json",
    "rm $PWD/*.json",
    // Decoys: an assignment in a heredoc body, after a comment, onto a
    // readonly name, or inside `eval`.
    "cat <<'EOF'\nD=/tmp\nEOF\nrm $D/*.json",
    "true # ; D=/tmp\nrm $D/*.json",
    "readonly D; D=/tmp; rm $D/*.json",
    "eval 'D=\"x y\"'; rm $D/*.json",
];

#[test]
fn ordinary_commands_that_name_no_exceptions_file_are_allowed() {
    let refused: Vec<&&str> = MUST_BE_ALLOWED.iter().filter(|c| denies(c)).collect();
    assert!(
        refused.is_empty(),
        "refused {} command(s) that do not name the exceptions file: {refused:#?}",
        refused.len()
    );
}

#[test]
fn every_way_of_naming_the_exceptions_file_is_refused() {
    let allowed: Vec<&&str> = MUST_BE_DENIED
        .iter()
        .chain(BYPASSES)
        .filter(|c| !matches!(guard(&bash_call(c)), Decision::Deny(r) if r == EXCEPTIONS))
        .collect();
    assert!(
        allowed.is_empty(),
        "not refused as the exceptions file: {allowed:#?}"
    );
}

/// The agent's shell, for the tests that resolve a variable the line does
/// not set: safe values, and a working directory outside `.claude`.
fn safe_env() -> Environment {
    Environment::of(
        &[
            ("FILES", "a.rs b.rs"),
            ("OUT", "/tmp/out"),
            ("SRC", "/tmp/src"),
            ("DST", "/tmp/dst"),
            ("f", "notes.json"),
            ("r", "repo"),
            ("GITHUB_WORKSPACE", "/w"),
            ("DIR", "sub"),
            ("D", "/tmp/x"),
            ("HOME", "/home/u"),
        ],
        Some("/home/u/proj"),
    )
}

/// The same, with `D` and the working directory inside `.claude`, and `f`
/// the file's name.
fn claude_env() -> Environment {
    Environment::of(
        &[
            ("D", "/home/u/.claude"),
            ("f", crate::hook::EXCEPTIONS_FILE),
            ("HOME", "/home/u"),
        ],
        Some("/home/u/.claude"),
    )
}

fn refuses_in(command: &str, env: &Environment) -> bool {
    matches!(
        pre_tool_guard_in(&bash_call(command), &Policy::ALL_ON, env),
        Decision::Deny(r) if r == EXCEPTIONS
    )
}

/// Each refused with no environment, and decided by the variables' values.
const RESOLVED: &[&str] = &[
    "for f in $FILES; do git show HEAD:$f > $OUT/$f; done",
    "cp $SRC/$f $DST/$f",
    "cd $GITHUB_WORKSPACE/$DIR",
    "gh api repos/acme/$r/contents/$f --jq .content",
    "rm $D/$f",
    "rm $D/*.json",
    "cp /tmp/x $D/${f}",
    "rm $D/*s.json",
    "rm $PWD/*.json",
];

#[test]
fn a_variable_the_line_does_not_set_is_decided_by_its_real_value() {
    let refused: Vec<&&str> = RESOLVED
        .iter()
        .filter(|c| refuses_in(c, &safe_env()))
        .collect();
    assert!(refused.is_empty(), "refused with safe values: {refused:#?}");
    let into_claude = [
        "rm $D/$f",
        "rm $D/*.json",
        "cp /tmp/x $D/${f}",
        "rm $D/*s.json",
        "rm $PWD/*.json",
    ];
    let allowed: Vec<&&str> = into_claude
        .iter()
        .filter(|c| !refuses_in(c, &claude_env()))
        .collect();
    assert!(
        allowed.is_empty(),
        "allowed pointing into .claude: {allowed:#?}"
    );
    for command in RESOLVED {
        assert!(
            denies(command),
            "with no environment, {command} must refuse"
        );
    }
}

#[test]
fn what_no_environment_can_decide_still_refuses() {
    for command in [
        "rm */*",
        "rm $CLAUDE_CONFIG_DIR/*.json",
        "rm $UNSET_HERE/*.json",
        "cd /; rm $PWD/*.json",
    ] {
        assert!(refuses_in(command, &safe_env()), "allowed: {command}");
    }
}

#[test]
fn too_many_alternatives_refuse_at_both_bounds() {
    let words: Vec<String> = (0..20).map(|i| format!("w{i}")).collect();
    let few = format!("for v in {}; do echo $v/$v; done", words[..3].join(" "));
    let many = format!("for v in {}; do echo $v/$v; done", words.join(" "));
    assert!(!refuses_in(&few, &safe_env()));
    assert!(
        refuses_in(&many, &safe_env()),
        "past the substitution bound"
    );
    assert!(!refuses_in("echo {a,b}/x", &safe_env()));
    let lists = format!("echo {}/x", "{a,b}".repeat(9));
    assert!(refuses_in(&lists, &safe_env()), "past the brace bound");
}

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

/// (c) A `for` loop running `git show` into scratch files, SIMPLIFIED to the
/// part that can be relaxed: its real form writes `$S/pr/$(basename …).md`,
/// and a `$(…)` leaf cannot be ruled out of climbing back to the file with
/// `..`, so the real (c) stays refused. A scratch write to a proven-literal
/// path is relaxed.
const SCRATCH_LOOP: &str =
    "S=/tmp/s; for n in a b; do echo $n; done; wc -l $S/pr/body.md; ls $S/pr";

const MUST_BE_ALLOWED: &[&str] = &[
    PYTHON_HEREDOC,
    PR_BODY_HEREDOC,
    SCRATCH_LOOP,
    // Markdown inline code holding a `/`.
    "cat > /tmp/b.md <<'EOF'\nNo `local/` prefix; only `kubernetes.io/cluster/*` tags.\nEOF",
    "cat > /tmp/b.md <<'EOF'\nSplit on `/` only, as in `a/b`.\nEOF",
    // A scratch directory set as a literal at the top of the line.
    "S=/tmp/s; mkdir -p $S/pr; wc -l ${S}/pr/body.md",
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
    // Fourth review. Each spells the file's name with a brace list, so only
    // the matcher, and not the line-text check, can refuse it.
    // A1: an assignment inside `$(…)` is never proven, and the charset gate
    // covers span contents.
    "S=/tmp/s; X=$( S=/home/u/.claude; rm $S/* ); echo x > $S/yadgar-hook-{exceptions,z}.json",
    "S=/tmp/s; X=$( S=/home/u/.claude ); echo x > $S/yadgar-hook-{exceptions,z}.json",
    // A2: text after a heredoc's closing quote is part of the delimiter.
    "cat > /tmp/b.md <<\"E\"OF\nx\nEOF\nrm ~/.c*/y*\nE",
    "cat > /tmp/b.md <<'E'OF\nx\nEOF\nrm ~/.c*/y*\nE",
    // A3: names the shell maintains itself.
    "_=/tmp; echo /home/u/.claude; echo x > $_/yadgar-hook-{exceptions,z}.json",
    "PWD=/tmp; echo x > $PWD/yadgar-hook-{exceptions,z}.json",
    "OLDPWD=/tmp; echo x > $OLDPWD/yadgar-hook-{exceptions,z}.json",
    "S=/tmp; cd /home/u/.claude; echo x > $S/../.claude/yadgar-hook-{exceptions,z}.json",
    // A4: a git subcommand outside the allowlist on a data-heredoc line.
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\ngit rebase --exec 'bash /tmp/b.md' HEAD~1",
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\ngit difftool -x 'bash /tmp/b.md'",
    // A git subcommand off the allowlist with no redirect flag (submodule,
    // bisect run, filter-branch all run arbitrary code).
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\ngit submodule foreach bash /tmp/b.md",
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\ngit bisect run bash /tmp/b.md",
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\ngit -c core.pager='bash /tmp/b.md' log",
    // A5: `sort` and `uniq` are off the list; gh only creates, edits or
    // comments on a PR.
    "S=/tmp; sort -o $S/x $S/yadgar-hook-{exceptions,z}.json",
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\ngh pr view",
    // Mutation killers.
    "D=/home/u/.claude; echo x > $D/yadgar-hook-{exceptions,z}.json",
    "ls | S=/tmp; echo x > $S/yadgar-hook-{exceptions,z}.json",
    "for n in; do S=/tmp; done; echo x > $S/yadgar-hook-{exceptions,z}.json",
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\nbash /tmp/b.md",
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\necho `bash /tmp/b.md`",
    "bash > /tmp/o.md <<'EOF'\nrm ~/.c*/y*\nEOF",
    "bash <<'A'; cat > /tmp/b.md <<'B'\nrm ~/.c*/y*\nA\nB",
    "cat > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF\ngit -c 'core.pager=bash /tmp/b.md' log",
    "rm ~/{\"x /.claude\",z}/y*.json",
    // Mutation killers needing a glob leaf under a wrongly-proven safe dir
    // (the exact-name cases above cannot catch a mis-proof of the directory).
    // M6: a `for` variable also assigned as a literal is still disproven.
    "S=/tmp; for S in a b; do :; done; rm $S/y*.json",
    // M22: an assignment inside a loop body is not proven.
    "for n in a; do S=/tmp; done; rm $S/y*.json",
    // M11: an unquoted-delimiter heredoc body is read as commands.
    "cat > /tmp/b.md <<EOF\nrm ~/.c*/y*\nEOF",
    // M16: only `cat` writes a heredoc body verbatim; another command does not.
    "wc > /tmp/b.md <<'EOF'\nrm ~/.c*/y*\nEOF",
    // M17: a second heredoc on the line is not modelled.
    "cat > /tmp/a.md <<'EOF'\nx\nEOF\ncat <<EOF2\nrm ~/.c*/y*\nEOF2",
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
    // Variables the line does not set are unknown (read as `*`), so a path
    // built from them refuses.
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
    // `local` outside a function fails; a function called in a loop body
    // assigns the loop's variable; `$PWD` may not be the payload's `cwd`.
    "local D=/tmp; rm $D/*.json",
    "f() { D=~/.claude; }; for D in /tmp; do f; rm $D/*.json; done",
    "for D in /tmp; do for D in ~/.claude; do :; done; rm $D/*.json; done",
    "D=(~/.claude); rm $D/*.json",
    // An assignment in argument position holding whitespace, run by a
    // wrapper that keeps it in the line's shell.
    "D=/tmp; command export \"D=a ~/.claude\"; rm $D/*.json",
    // A literal directory does not make a word safe while another reference
    // in it stays unproven: $f from the environment may be `../.claude/<file>`.
    "D=/tmp; echo x > $D/$f",
    "D=/tmp; echo x > ${D}/${f}",
    "f=x; echo x > $D/$f",
    "D=/tmp; cat a > $D/$f",
    // A positional or special parameter, and a loop variable, in the path:
    // each is unmodelled and may hold `../.claude/<file>`.
    "f=x; echo x > $1/$f",
    "f=x; echo x > $0/$f",
    "D=/tmp; for n in a b; do echo x > $D/$n; done",
    // The real (c): a `$(…)` leaf under a proven dir cannot be ruled out of
    // climbing back to the file with `..`.
    "S=/tmp/s; mkdir -p $S/pr; for f in $(git diff --name-only HEAD -- 'adr/*'); do git show HEAD:$f > $S/pr/$(basename $f | cut -c1-8).md; done",
    // Literal-variable relaxation is self-protecting: a literal that IS the
    // file's name, or a literal directory that is `.claude`, is substituted
    // and then refused by the exact check.
    "f=yadgar-hook-exceptions.json; echo x > /home/u/.claude/$f",
    "D=/home/u/.claude; f=yadgar-hook-exceptions.json; cp a $D/$f",
    "D=/home/u/.claude; echo x > $D/yadgar-hook-exceptions.json",
    // Third review: any write the expander does not model makes the name
    // unknown. A nameref, a `${D:=…}` default, a trap, `eval` behind a
    // wrapper, and zsh's own writers.
    "D=/tmp; declare -n R=D; R=~/.claude; rm $D/*.json",
    "D=/tmp; typeset -n R; R=D; rm $D/*.json",
    "D=; : ${D:=~/.claude}; rm $D/*.json",
    "D=/tmp; trap 'read D' DEBUG; rm $D/*.json <<< ~/.claude",
    "D=/tmp; command eval 'read D'; rm $D/*.json",
    "D=/tmp; vared D; rm $D/*.json",
    "D=/tmp; print -v D ~/.claude; rm $D/*.json",
    "D=/tmp; foreach D (~/.claude) rm $D/*.json; end",
    "D=/tmp; wait -p D; rm $D/*.json",
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

/// Refused: each is a branch where a value in the agent's shell cannot be
/// known for certain, so nothing is relaxed.
const UNDECIDABLE: &[&str] = &[
    "rm */*",
    "rm $CLAUDE_CONFIG_DIR/*.json",
    "rm $UNSET_HERE/*.json",
    "cd /; rm $PWD/*.json",
    // The payload's `cwd` is not proven to follow a `cd` from an earlier call.
    "rm $PWD/*.json",
    // `select` and `for NAME` with no `in` set the variable from input and
    // from the positional parameters.
    "select D in ~/.claude; do rm $D/*.json; done",
    "for D; do rm $D/*.json; done",
    // An inner loop over the same name leaves its last word behind.
    "for D in /tmp; do for D in ~/.claude; do :; done; rm $D/*.json; done",
    // An array assignment: `$D` is its first element.
    "D=(~/.claude); rm $D/*.json",
];

#[test]
fn what_cannot_be_known_still_refuses() {
    let allowed: Vec<&&str> = UNDECIDABLE.iter().filter(|c| !denies(c)).collect();
    assert!(allowed.is_empty(), "allowed: {allowed:#?}");
}

/// Shapes that look like an allowlisted one but are not: each falls back to
/// the check on `main`, which refuses it.
const NOT_RELAXED: &[&str] = &[
    // A value from the environment, or not a plain literal.
    "cp $SRC/$f $DST/$f",
    "S=$HOME/.claude; rm $S/*.json",
    "S=~/.c*; rm $S/*.json",
    "S=/tmp/s; rm $S/../$X/*.json",
    // An assignment the line may not run, or that something else rewrites.
    "false && S=/tmp; rm $S/*",
    "(S=/tmp); rm $S/*",
    "S=/tmp | cat; rm $S/*",
    "S=/tmp; read S; rm $S/*",
    "S=/tmp; eval x; rm $S/*",
    "S=/tmp; grep -q x f; rm $S/*",
    "S=/tmp; for S in ~/.claude; do rm $S/*; done",
    "S=/tmp; f; rm $S/*",
    // A heredoc whose body may run: a shell or interpreter reads it, or a
    // command on the line could run the file it is written to.
    "cat > /tmp/x.sh <<'EOF'\nrm ~/.c*/y*\nEOF\nbash /tmp/x.sh",
    "bash <<'EOF'\nrm ~/.c*/y*\nEOF",
    "cat > .git/hooks/pre-push <<'EOF'\nrm ~/.c*/y*\nEOF\ngit push",
    "cat <<EOF > /tmp/b.md\n`echo ~/.claude`/y*\nEOF",
    // A data heredoc only into a `.md` or `.txt` file outside any dot
    // directory: a profile, a hook or a script may be run by a later call.
    "cat > /home/u/.zshenv <<'EOF'\nrm ~/.c*/y*\nEOF",
    "cat > /tmp/x.sh <<'EOF'\nrm ~/.c*/y*\nEOF",
    "cat > /tmp/.hooks/pre-push <<'EOF'\nrm ~/.c*/y*\nEOF",
    // Work in a `.claude/worktrees/…` checkout, and a wildcard directory.
    "for w in agent-a agent-b; do git -C .claude/worktrees/$w status; done",
    "find . -name '*.json' -not -path '*/node_modules/*'",
];

#[test]
fn a_shape_outside_the_allowlist_is_checked_as_on_main() {
    let allowed: Vec<&&str> = NOT_RELAXED.iter().filter(|c| !denies(c)).collect();
    assert!(
        allowed.is_empty(),
        "relaxed outside the allowlist: {allowed:#?}"
    );
}

#[test]
fn too_many_alternatives_refuse_at_both_bounds() {
    assert!(!denies("echo {a,b}/x"));
    let lists = format!("echo {}/x", "{a,b}".repeat(9));
    assert!(denies(&lists), "past the brace bound");
    let mut many = String::new();
    for i in 0..300 {
        many.push_str(&format!("S=/tmp/{i}; "));
    }
    assert!(!denies(&format!("{many}ls /tmp")));
    assert!(
        denies(&format!("{many}ls $S/$S")),
        "past the substitution bound"
    );
}

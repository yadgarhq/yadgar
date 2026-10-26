//! An unquoted `$(…)` and the PATH directories before gh (ADR-1020, after
//! yadgar#92).
//!
//! The scanner ends a command at the `$` of `$(` and starts another after
//! the `)`. On #92 every later command's first word was read as the rest of
//! the substitution's argument, so the lone `}` closing a group after
//! `b=$(x);` became `$()}`, and a brace after a substitution refuses. Now a
//! command continues the argument only when the separators before it are
//! the `)` closing the substitution. Main's reading stays as a floor, so
//! the only refusal given up is that lone `}`.
//!
//! The environment is a literal, so no test here depends on the PATH of
//! the machine that runs it.

use std::path::PathBuf;

use super::shadow::{bash_calls, not_refused_as, refused};
use super::*;

fn env() -> GuardEnv {
    GuardEnv {
        home: Some(PathBuf::from("/home/x")),
        before_gh: vec![
            PathBuf::from("/run/wrappers/bin"),
            PathBuf::from("/home/x/.local/bin"),
        ],
        ..GuardEnv::default()
    }
}

fn judge(call: &Value) -> Decision {
    pre_tool_guard(call, &Policy::ALL_ON, &env())
}

/// A group after a substitution that closed with a `;`: nothing continues
/// the substitution's argument.
const GROUP_AFTER_A_SUBSTITUTION: &[&str] = &[
    "b=$(x); { echo z; }",
    "for f in a; do b=$(basename $f); { echo z; }; done",
];

#[test]
fn a_group_after_a_closed_substitution_is_not_refused() {
    let over = refused(&bash_calls(GROUP_AFTER_A_SUBSTITUTION), judge);
    assert!(over.is_empty(), "over-refused: {over:#?}");
}

/// Each writes into a protected directory, or onto `<output>}`, through
/// the argument a substitution starts.
const CONTINUED: &[&str] = &[
    // The `}` right after the `)` is part of the same argument.
    "cp a $(x)}",
    // Separators inside the substitution do not end the argument.
    "cp a $(x; y)/.local/bin/t",
    // The scanner reads `))` as one separator.
    "$(a $(b))/.local/bin/t",
    "cp a $(a $(b))/.local/bin/t",
    "cp a $(a $(b)) ~/.local/bin/t",
    // A word after the substitution, spaced or not, is the outer command's.
    "cp a $(x) ~/.local/bin/t",
    "cp $(which t) ~/.local/bin/t",
    // A substitution opened as a redirect target.
    "echo x > $(echo ~)/.local/bin/t",
    // Whether an earlier substitution fed a reader says nothing about this one.
    "cat $(x)/a; cp q $(y)/.local/bin/t",
    // A continued word is not a program, so its arguments are not a reader's.
    "cp q $(x)cat ~/.local/bin/t",
    // A piece split off at a brace runs on from the piece before it: to a
    // shell the brace is one more argument of `cp`.
    "cp q $(x)/a { /run/wrappers/bin/t",
    "cp q $(x)/a { ~/.local/bin/t",
];

#[test]
fn a_substitution_continues_its_argument_up_to_the_closing_paren() {
    let wrong = not_refused_as(&bash_calls(CONTINUED), SHADOW, judge);
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
}

/// Refused on main, whose reading of a substitution is sticky, where
/// counting parens reads a `)` that is not one: a `case` arm, a `#`
/// comment, one `)` too many. Main's reading stays as a floor.
const MISCOUNTED: &[&str] = &[
    "cp a $(x #)\n)/.local/bin/t",
    "cp a $(x #)\n)}",
    "cp a $(case x in a) echo;; esac)/.local/bin/t",
    "cp a $(case x in a) echo;; esac)}",
    "cp a $(x))/.local/bin/t",
    "x=$(y); case $v in a) .local/bin/t;; esac",
    "x=$(y); .local/bin/t",
    "cat $(x) cp ~/.local/bin/t",
];

#[test]
fn a_miscounted_paren_keeps_the_refusal_main_made() {
    let wrong = not_refused_as(&bash_calls(MISCOUNTED), SHADOW, judge);
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
}

/// Whether the command reads is decided on its whole argument list, the
/// words after the substitution included.
const WRITE_FLAG_AFTER: &[&str] = &[
    "git log $(x)--output=/run/wrappers/bin/t",
    "git log $(x) --output=/run/wrappers/bin/t",
    "rg $(x)--pre=/run/wrappers/bin/t foo",
    "rg $(x) --pre=/run/wrappers/bin/t foo",
];

#[test]
fn a_write_option_after_a_substitution_is_not_a_read() {
    let wrong = not_refused_as(&bash_calls(WRITE_FLAG_AFTER), SHADOW, judge);
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
    let over = refused(&bash_calls(&["git log $(x) -- a", "rg $(x) foo"]), judge);
    assert!(over.is_empty(), "over-refused: {over:#?}");
}

/// A substitution whose output a writing command gets, holding a
/// protected directory: the output may be that directory.
const THE_WHOLE_DIRECTORY: &[&str] = &[
    "cp a $(echo /run/wrappers/bin)/tool",
    "cp a $(echo /run/wrappers/bin)",
    "cp a $(echo /run/wrappers/bin/tool)",
    "cp a $(cat $(echo ~/.local/bin))/t",
    r#"cp a "$(echo /run/wrappers/bin)"/tool"#,
    r#"cp a "`echo /run/wrappers/bin`"/tool"#,
    "D=$(echo ~/.local/bin)",
];

#[test]
fn a_substitution_that_yields_a_protected_directory_is_refused() {
    let wrong = not_refused_as(&bash_calls(THE_WHOLE_DIRECTORY), SHADOW, judge);
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
}

/// Reads, runs, and writes elsewhere next to a substitution.
const NOT_A_WRITE: &[&str] = &[
    "ls $(echo /run/wrappers/bin)",
    "ls $(echo ~)/.local/bin",
    "cat $(x)/.local/bin/t",
    "cp /tmp/x $(pwd)/target/t",
    "cp a $(pwd)/t",
    "echo $(( 1 + 2 )); ~/.local/bin/t --version",
    "for f in $(ls); do echo $f; done",
];

#[test]
fn reads_and_runs_next_to_a_substitution_are_not_refused() {
    let over = refused(&bash_calls(NOT_A_WRITE), judge);
    assert!(over.is_empty(), "over-refused: {over:#?}");
}

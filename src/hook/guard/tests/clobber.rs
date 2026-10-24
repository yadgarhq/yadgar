//! A clobbering or dup-spelled redirect writes its file as `>` does
//! (ADR-1020): `>|` and `>!` (zsh) override noclobber, and `>&word` with a
//! word that is not a descriptor writes stdout and stderr into *word*.
//! The scanner reads the `|` or `&` as a separator, so the target used to
//! land in program position and both new rules missed it. A command that
//! is only a redirect (`> gh`, and the tail of `&> gh`) writes its file too.

use super::*;

/// The refusing shapes, each with `{dir}` for a directory before gh.
const CLOBBERING: &[&str] = &[
    "echo x >| /tmp/bin/gh",
    "echo x >|gh",
    "echo x >>| /tmp/bin/gh",
    "echo x >! /tmp/bin/gh",
    "echo x >!gh",
    "echo x >>!gh",
    "echo x >&/tmp/bin/gh",
    "echo x >&gh",
    "echo x 1>|gh",
    "echo x &>|gh",
    "echo x &>gh",
    "echo x &> /tmp/bin/gh",
    "> /tmp/bin/gh",
    "sh -c 'echo x >| /tmp/bin/gh'",
    "echo x >| {dir}/tool",
    "echo x >|{dir}/tool",
    "echo x >! {dir}/tool",
    "echo x >>!{dir}/tool",
    "echo x >&{dir}/tool",
    "echo x >& {dir}/tool",
    "bash -c 'echo x >| {dir}/tool'",
];

/// Descriptor duplication and closing write no file.
const DUPLICATING: &[&str] = &[
    "cargo build 2>&1 | tail -3",
    "echo x >&2",
    "echo x 1>&2",
    "exec 3>&-",
    "cmd > /tmp/log 2>&1",
    "echo x >| /tmp/log",
    "echo x >! /tmp/log",
];

#[test]
fn a_clobbering_redirect_onto_gh_or_a_dir_before_it_is_refused() {
    let m = Machine::new("clobber", Some(0o755));
    let dir = m.dir("home/.local/bin");
    let lines: Vec<String> = CLOBBERING
        .iter()
        .map(|l| l.replace("{dir}", &dir))
        .collect();
    let calls: Vec<Value> = lines.iter().map(|l| bash_call(l)).collect();
    let wrong = not_refused_as(&calls, SHADOW, |c| m.judge(c));
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
}

#[test]
fn descriptor_duplication_is_not_refused() {
    let m = Machine::new("dup", Some(0o755));
    let wrong = refused(&bash_calls(DUPLICATING), |c| m.judge(c));
    assert!(wrong.is_empty(), "over-refused: {wrong:#?}");
}

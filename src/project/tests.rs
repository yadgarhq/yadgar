//! The Python client's rule, asserted case by case.
//!
//! These are not "does it look right" tests. Each one is a shape the Python
//! derivation handles and a hand-written parser gets wrong, and a disagreement
//! on any of them files the same repository's memories under two keys.

use super::*;

#[test]
fn an_https_remote_loses_its_scheme_its_host_and_its_dot_git() {
    // The ordinary case, and the one that pins that the HOST IS EXCLUDED. A key
    // of `github.com/yadgarhq/yadgar` is well-formed, plausible, and disagrees
    // with every row the Python client ever wrote.
    assert_eq!(
        normalise_remote("https://github.com/yadgarhq/yadgar.git"),
        "yadgarhq/yadgar"
    );
    assert_eq!(
        normalise_remote("https://github.com/yadgarhq/yadgar"),
        "yadgarhq/yadgar"
    );
}

#[test]
fn an_scp_style_ssh_remote_resolves_to_the_same_key_as_its_https_twin() {
    // The same repository cloned two ways must key identically, or one person's
    // memories are invisible to the next.
    assert_eq!(
        normalise_remote("git@github.com:yadgarhq/yadgar.git"),
        normalise_remote("https://github.com/yadgarhq/yadgar.git")
    );
    assert_eq!(
        normalise_remote("ssh://git@github.com/yadgarhq/yadgar.git"),
        "yadgarhq/yadgar"
    );
}

#[test]
fn a_bare_ssh_alias_is_a_host_and_not_a_path() {
    // What this machine's git actually produces after an `insteadOf`:
    // `codeberg-agent:owner/repo`, with no `user@` at all. Reading it as a path
    // keys every codeberg repository under `codeberg-agent:owner/repo`.
    assert_eq!(
        normalise_remote("codeberg-agent:openfantasy/toaster.git"),
        "openfantasy/toaster"
    );
}

#[test]
fn a_nested_namespace_stays_one_opaque_path() {
    // §16.9. Splitting on the last `/` collapses a group with thirty
    // subprojects into thirty collisions, and the test above would still pass.
    assert_eq!(
        normalise_remote("https://gitlab.example/group/sub/repo.git"),
        "group/sub/repo"
    );
}

#[test]
fn a_repository_named_after_a_domain_keeps_its_suffix() {
    // ONLY a trailing `.git` is stripped. A `.git` anywhere else — or a
    // `.io` mistaken for one — renames somebody's repository.
    assert_eq!(
        normalise_remote("https://github.com/m-agahi/yadgar.io"),
        "m-agahi/yadgar.io"
    );
    assert_eq!(
        normalise_remote("git@github.com:m-agahi/yadgar.io"),
        "m-agahi/yadgar.io"
    );
    // A path with a slash BEFORE a colon is not an SSH remote.
    assert_eq!(normalise_remote("m-agahi/yadgar.io"), "m-agahi/yadgar.io");
}

#[test]
fn the_key_is_lowercased() {
    // Git hosts are case-insensitive on the owner and case-preserving on the
    // clone URL, so two clones of one repository differ only in casing.
    assert_eq!(
        normalise_remote("https://github.com/YadgarHQ/Yadgar.git"),
        "yadgarhq/yadgar"
    );
}

#[test]
fn insteadof_rewrites_are_applied_before_the_url_is_read() {
    // The rewrite is what makes this machine's codeberg remotes resolve at all,
    // and it is invisible to `remote.origin.url` — git applies it at transport
    // time, so a client that reads the raw value derives a different key from
    // the one the Python client derives on the same machine.
    let rules = insteadof_rules_from(
        "url.codeberg-agent:.insteadof git@codeberg.org:\nurl.https://x/.insteadof x:",
    );
    assert_eq!(
        apply_insteadof(&rules, "git@codeberg.org:openfantasy/toaster.git"),
        "codeberg-agent:openfantasy/toaster.git"
    );
    assert_eq!(
        normalise_remote(&apply_insteadof(
            &rules,
            "git@codeberg.org:openfantasy/toaster.git"
        )),
        "openfantasy/toaster"
    );
}

#[test]
fn a_rewrite_table_that_chases_its_own_tail_terminates() {
    // Legal git configuration. A client that hangs on startup is worse than one
    // that reports the wrong key, because nothing at all works and there is no
    // message to read.
    let mut rules = BTreeMap::new();
    rules.insert("alpha:".to_string(), "beta:".to_string());
    rules.insert("beta:".to_string(), "alpha:".to_string());
    let out = apply_insteadof(&rules, "alpha:owner/repo");
    assert!(out.ends_with("owner/repo"), "{out}");
}

#[test]
fn a_line_that_is_not_an_insteadof_rule_is_ignored() {
    let rules = insteadof_rules_from("user.name Max\nurl..insteadof nothing\nnot a pair\n");
    assert!(rules.is_empty(), "{rules:?}");
}

#[test]
fn a_project_id_file_overrides_the_remote_outright() {
    // The documented override AND the escape hatch: a monorepo subproject and a
    // fresh checkout with no remote have no other way to name themselves.
    // `sentinel/override-not-derivable` cannot be derived from any remote this
    // tree has, so a derivation that ignored the file cannot produce it.
    let root = crate::testserver::scratch_dir("project-file");
    let deep = root.join("packages").join("inner");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::create_dir_all(root.join(".yadgar")).unwrap();
    std::fs::write(
        root.join(".yadgar").join("project-id"),
        "sentinel/override-not-derivable\n",
    )
    .unwrap();

    // Found by WALKING UP, from a directory two levels below the file.
    assert_eq!(
        derive(&deep).as_deref(),
        Some("sentinel/override-not-derivable")
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_empty_project_id_file_falls_through_to_the_remote_rather_than_being_skipped() {
    // THE DIVERGENCE THAT MATTERS. `_walk_project_id_file` returns the FIRST
    // file it finds, stripped, and `mint_project_id`'s `if override:` is falsy
    // for `""` — so Python stops walking and uses the git remote. While this
    // kept walking, an empty file was TRANSPARENT and a GRANDPARENT's file won
    // instead: two clients, two keys, one repository, reachable with `touch`.
    //
    // The ancestor below holds a key nothing else could produce, so a walk that
    // reads past the empty file lands on it and fails here.
    let root = crate::testserver::scratch_dir("project-empty-file");
    let inner = root.join("inner");
    std::fs::create_dir_all(inner.join(".yadgar")).unwrap();
    std::fs::create_dir_all(root.join(".yadgar")).unwrap();
    std::fs::write(
        root.join(".yadgar").join("project-id"),
        "sentinel/the-ancestor-must-not-win\n",
    )
    .unwrap();
    std::fs::write(inner.join(".yadgar").join("project-id"), "   \n").unwrap();

    // The walk stopped at the nearest file, which says nothing.
    assert_eq!(project_id_file(&inner).as_deref(), Some(""));
    // So the ancestor is never consulted, and with no git remote under the
    // temp directory the answer is nothing at all.
    assert_eq!(derive(&inner), None);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_remote_with_no_path_keeps_its_host_as_python_does() {
    // Degenerate, and still a disagreement: `host_end >= 0` is false in Python
    // when there is no slash, so `stripped` stays as the host. Yielding `""`
    // here omitted the header where Python sent a key.
    assert_eq!(normalise_remote("https://github.com"), "github.com");
    assert_eq!(normalise_remote("ssh://gitserver"), "gitserver");
}

#[test]
fn a_directory_with_no_file_and_no_remote_names_itself_nothing() {
    // ADR-0227: no `local/<basename>`, no `"global"`, no guess. A plausible
    // wrong key is worse than none, because nothing downstream can tell it from
    // a real one. The scratch directory has no `.yadgar/project-id`; it sits
    // under the system temp directory, which is not a git repository.
    let dir = crate::testserver::scratch_dir("project-none");
    assert_eq!(derive(&dir), None);
    std::fs::remove_dir_all(&dir).ok();
}

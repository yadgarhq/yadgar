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

// ---------------------------------------------------------------------------
// The canonical-form matrix (`docs/plans/project-validation.md`, stage 0)
// ---------------------------------------------------------------------------

/// The canonical form for a git address, one row per shape.
///
/// **THE SPEC IS THE PLAN'S, NOT THIS TABLE'S.**
/// `plans/project-validation.md` in `yadgarhq/docs` states the derivation in
/// steps — `insteadOf` to a fixed point, strip the transport, strip ALL
/// trailing slashes, strip ONE trailing `.git`, strip trailing slashes again,
/// fold to lowercase, host EXCLUDED and nested namespaces NOT collapsed, and a
/// monorepo subpath never derived from a remote (ledger 881 adds the two
/// slash-stripping steps; see below). This table asserts that spec against
/// literal inputs. It does not restate it: a spec written twice is a spec that
/// drifts, and the plan is the copy that governs.
///
/// **EVERY EXPECTATION IS A LITERAL.** Not one is computed by the code under
/// test, and not one compares two calls of `normalise_remote` against each
/// other. A matrix whose expected column is produced by the derivation passes
/// for every derivation, including a broken one — the estate's most repeated
/// defect, and the reason the plan names fixture discipline in the same
/// paragraph as the table.
///
/// **WHY THIS MATTERS AT ALL**: `git@github.com:yadgarhq/docs.git`,
/// `https://github.com/yadgarhq/docs` and the trailing-`.git` variant are three
/// strings for one repository. Without a pinned form one repository silently
/// becomes three projects, each with its own `task_counter` sequence, and D52
/// says no later pass can merge them back.
///
/// **PYTHON PARITY IS NOT PROVED HERE AND STAYS INFERRED, which the plan asks
/// for explicitly.** Stage 0 says the matrix runs "in BOTH clients (Rust
/// `yadgar-client`, Python in `yadgarhq/yadgar`)", and that sentence cannot be
/// satisfied as written: the directory `yadgar-client` IS the repository
/// `yadgarhq/yadgar`, and this repository holds no Python source at all.
/// `pyproject.toml` says so in as many words — "There is no Python source in
/// this repository and there is no importable module" (D75) — and the Python
/// client stays on PyPI under the name `yadgar` only "for the transition"
/// (ADR-0505). The twenty repositories of `yadgarhq` hold no other client.
/// So this table asserts the SPEC, and the module header's claim to be a
/// transcription of `mint_project_id` remains a transcription rather than a
/// measurement. Anyone who can reach that Python source should run this same
/// table against it; until then the parity leg of stage 0 is open.
///
/// **THE FIRST NINE ROWS ARE THE PLAN'S OWN MINIMUM**, in its order. The rest
/// are the forge shapes it asks for — github, gitlab, bitbucket, codeberg,
/// azure; ssh and https; with and without `.git` — plus three shapes that pin a
/// boundary rather than a forge, each annotated where it sits.
const CANONICAL_FORM_MATRIX: &[(&str, &str)] = &[
    // The plan's table, rows 1-9.
    ("git@github.com:yadgarhq/docs.git", "yadgarhq/docs"),
    ("https://github.com/yadgarhq/docs", "yadgarhq/docs"),
    ("https://github.com/yadgarhq/docs.git", "yadgarhq/docs"),
    // A PORT IN AN `ssh://` URL. The host is dropped up to the first `/`, so the
    // `:22` goes with it; a parser splitting on the colon instead would keep
    // `22/yadgarhq/docs` and key the repository under a port number.
    ("ssh://git@github.com:22/yadgarhq/docs.git", "yadgarhq/docs"),
    ("https://gitlab.com/group/sub/repo.git", "group/sub/repo"),
    ("codeberg-agent:owner/repo", "owner/repo"),
    ("git@github.com:YadgarHQ/Docs.git", "yadgarhq/docs"),
    (
        "https://github.com/yadgarhq/yadgar.io",
        "yadgarhq/yadgar.io",
    ),
    (
        "https://dev.azure.com/org/project/_git/repo",
        "org/project/_git/repo",
    ),
    // Forge shapes, ssh and https, with and without `.git`.
    ("git@gitlab.com:group/sub/repo.git", "group/sub/repo"),
    ("ssh://git@gitlab.com/group/sub/repo", "group/sub/repo"),
    ("git@github.com:yadgarhq/docs", "yadgarhq/docs"),
    ("git@bitbucket.org:owner/repo.git", "owner/repo"),
    ("git@codeberg.org:owner/repo", "owner/repo"),
    ("git://github.com/yadgarhq/docs.git", "yadgarhq/docs"),
    // USERINFO AND A PORT IN AN `https://` URL, which the plan's step 2 names
    // explicitly: both belong to the transport and neither reaches the id.
    ("https://user@github.com/yadgarhq/docs.git", "yadgarhq/docs"),
    ("https://github.com:443/yadgarhq/docs.git", "yadgarhq/docs"),
    // Azure's ssh form, kept because the plan's Azure row is deliberately ugly
    // and deliberately in: the spec has NO forge-specific carve-outs, because
    // each carve-out is a divergence two clients must then keep in step for
    // ever. `v3` is part of the id, and an installation on Azure registers what
    // the derivation yields or places marker files.
    (
        "git@ssh.dev.azure.com:v3/org/project/repo",
        "v3/org/project/repo",
    ),
    // ONE TRAILING `.git`, NEVER TWO. A repository really named `docs.git` keeps
    // the name; a loop stripping the suffix until it stops matching renames it.
    (
        "https://github.com/yadgarhq/docs.git.git",
        "yadgarhq/docs.git",
    ),
    // OUTSIDE THE SEGMENT ALPHABET, PASSED THROUGH RATHER THAN COERCED. The plan
    // says a remote path with characters outside `[A-Za-z0-9._-]` does not
    // canonicalise: the client sends what it derives and `project-db`'s
    // `validate` refuses it at registration, which is the correct loud failure.
    // A client that folded such a path into something registrable would be
    // minting an id nobody wrote.
    ("https://forge.example/Ünïcode/Repo", "ünïcode/repo"),
    // A TRAILING SLASH IS STRIPPED — ALL OF THEM, not one. Without this,
    // `git remote set-url origin https://github.com/yadgarhq/docs/` keys the
    // same repository under a second id: the exact "one repo becomes N
    // projects" failure the whole spec exists to prevent, one character wide.
    // Ledger 881 closes the gap the previous version of this row stated rather
    // than hid; the plan's six steps now carry a seventh.
    ("https://github.com/yadgarhq/docs/", "yadgarhq/docs"),
    // MULTIPLE TRAILING SLASHES, all stripped. Unlike a trailing `.git` — where
    // a repository can legitimately be named `docs.git`, so only ONE strip is
    // safe — an empty path segment never carries content. `docs`, `docs/` and
    // `docs//` name the same repository, so every one of them collapses.
    ("https://github.com/yadgarhq/docs//", "yadgarhq/docs"),
    // TRAILING SLASH OUTSIDE `.git`. The slash must be stripped BEFORE the
    // `.git` suffix is checked, or `docs.git/` never matches `strip_suffix(
    // ".git")` at all and keeps both the suffix and the slash.
    ("https://github.com/yadgarhq/docs.git/", "yadgarhq/docs"),
    // TRAILING SLASH INSIDE `.git` — REACHABLE, not hypothetical: pre-fix,
    // `normalise_remote` already turns this into `yadgarhq/docs/` (the `.git`
    // suffix strips clean, exposing the slash the `.git`-strip does not know to
    // remove), so this shape hit the pre-fix bug through a second path. Closing
    // it requires trimming trailing slashes AFTER the `.git` strip too, not only
    // before.
    ("https://github.com/yadgarhq/docs/.git", "yadgarhq/docs"),
    // A BARE-SLASH-ONLY REMOTE. The grammar permits it: an `insteadOf` alias
    // with nothing after the colon (`after_ssh_host`) yields exactly `"/"`.
    // Stripped to `""`, which `project-db`'s `validate` refuses at registration
    // — the correct loud failure, same shape as the out-of-alphabet row above.
    ("codeberg-agent:/", ""),
];

/// Every row of [`CANONICAL_FORM_MATRIX`], asserted one at a time.
///
/// **THE LENGTH IS ASSERTED FIRST, and that is not ceremony.** A loop over a
/// table is a check that cannot fail when the table is empty, and a row deleted
/// by a careless edit is a shape nothing checks any more with no test turning
/// red. The estate has met the empty-collection form of this four times in a
/// week — a glob matching nothing, a shallow-clone guard, an `&&`-chained hook —
/// so a table-driven test states its own size.
#[test]
fn the_canonical_form_matrix_holds_row_by_row() {
    assert_eq!(
        CANONICAL_FORM_MATRIX.len(),
        25,
        "the table is the test: a row lost to an edit is a shape nobody checks any more"
    );
    for (input, expected) in CANONICAL_FORM_MATRIX {
        assert_eq!(
            normalise_remote(input),
            *expected,
            "canonical form of {input:?}"
        );
    }
}

/// A MONOREPO SUBPATH IS NEVER DERIVED FROM THE REMOTE — it enters only through
/// a `.yadgar/project-id` marker, whose trimmed contents win outright (D53, and
/// step 6 of the spec).
///
/// This is the row of the matrix that cannot be a string pair, because the claim
/// is about PRECEDENCE between two sources rather than about one input. So the
/// fixture is a real repository with a real `origin`, and the two values are
/// both asserted: the marker's, which `derive` answers, and the remote's, which
/// it does not.
///
/// **THE REMOTE'S HOST IS A SENTINEL nothing on any machine rewrites.** `derive`
/// consults the machine's own `insteadOf` table on the remote path, so a
/// fixture naming a real forge would assert a different value on a machine
/// carrying a rewrite for it. `sentinel-forge` matches no rule anywhere, and the
/// marker short-circuits ahead of the remote in any case — belt and braces,
/// because the braces are what make the test hermetic and the belt is what makes
/// it true.
///
/// Correcting the brief that commissioned the plan, and worth keeping asserted:
/// the fallback is the origin of the repository ROOT, so the cwd's own subpath
/// never enters remote derivation. `yadgarhq/docs/plans` reaches the wire ONLY
/// the way it does here.
#[test]
fn a_monorepo_subpath_comes_from_the_marker_file_and_never_from_the_remote() {
    let root = crate::testserver::scratch_dir("project-monorepo-subpath");
    let deep = root.join("plans").join("drafts");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::create_dir_all(root.join(".yadgar")).unwrap();
    std::fs::write(
        root.join(".yadgar").join("project-id"),
        "yadgarhq/docs/plans\n",
    )
    .unwrap();

    let remote = "git@sentinel-forge:yadgarhq/docs.git";
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "fixture@example.invalid"],
        &["config", "user.name", "fixture"],
        &["remote", "add", "origin", remote],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    // THE REMOTE IS REACHABLE AND SAYS SOMETHING ELSE. Without this the test
    // would pass against a fixture with no remote at all, which is the
    // single-source case another test already covers.
    assert_eq!(
        origin_remote(&repository_root(&deep)).as_deref(),
        Some(remote),
        "the fixture's own origin"
    );
    assert_eq!(
        normalise_remote(remote),
        "yadgarhq/docs",
        "the remote alone yields the repository, never the subpath"
    );

    // And the marker wins, from two levels below it.
    assert_eq!(derive(&deep).as_deref(), Some("yadgarhq/docs/plans"));

    std::fs::remove_dir_all(&root).ok();
}

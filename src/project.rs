//! `x-yadgar-project` — which workspace the person is sitting in.
//!
//! CONTEXT, not identity (ADR-0511). The gateway resolves WHO the caller is
//! from the bearer token via `iam.ResolveCredential` and mints the `Scope`
//! itself; it cannot resolve WHERE they are, because no credential can carry a
//! working directory. So this half stays caller-supplied, and this module is
//! how the caller supplies it.
//!
//! **THE RULE IS THE PYTHON CLIENT'S, TRANSCRIBED RATHER THAN INVENTED.** Both
//! clients write into the same `project_id` namespace, so a Rust client that
//! derived `yadgar` where the Python one derived `yadgarhq/yadgar` would file
//! the same repository's memories under two keys and neither would find the
//! other's. The source is `yadgar/core/hooks/_identity_mint.py::mint_project_id`
//! and the pure readers it composes in `yadgar/core/identity.py`:
//!
//! 1. `.yadgar/project-id`, walked UP from the working directory to the
//!    filesystem root. Its trimmed contents win outright. It is the documented
//!    override and the escape hatch for a tree with no usable remote.
//! 2. Otherwise `git config remote.origin.url` at the repository root, with
//!    `insteadOf` rewrites applied to a fixed point, the scheme and host
//!    stripped, ONE trailing `.git` stripped, and the result lowercased.
//!
//! **THE HOST IS EXCLUDED AND NESTED NAMESPACES ARE NOT COLLAPSED** (§16.9):
//! `group/sub/repo` stays one opaque path. Splitting on the last `/` would
//! collapse every subproject of a group into one key, so a group with thirty
//! repositories would all collide.
//!
//! **A REPOSITORY NAMED `yadgar.io` KEEPS ITS `.io`.** Only a trailing `.git`
//! is stripped, never a suffix mid-path.
//!
//! **NOTHING IS GUESSED WHEN NEITHER SOURCE RESOLVES.** ADR-0227 deleted every
//! fallback in the Python client — no `local/<basename>`, no `"global"` — for
//! the reason it gives: "a fallback that cannot fail is worse than an error,
//! because it manufactures a plausible-looking wrong answer". Here that means
//! `None`, and `None` means the header is OMITTED rather than sent empty. The
//! proxy still serves: a directory with no identity is a real place to run an
//! agent, and the gateway is entitled to decide what a call without a project
//! may do. What this module must never do is send a key it made up.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The project key for *cwd*, or `None` when nothing resolves it.
pub fn derive(cwd: &Path) -> Option<String> {
    // AN EMPTY FILE FALLS THROUGH TO THE REMOTE, exactly as `mint_project_id`
    // does: it reads the first `.yadgar/project-id` it finds and its
    // `if override:` is falsy for `""`. The file says "nothing", not "nothing
    // is derivable".
    let key = match project_id_file(cwd).filter(|id| !id.is_empty()) {
        Some(override_id) => override_id,
        None => {
            let remote = origin_remote(&repository_root(cwd))?;
            normalise_remote(&apply_insteadof(&insteadof_rules(), &remote))
        }
    };
    // WHETHER IT CAN BE SENT IS NOT DECIDED HERE. This module answers "what is
    // this workspace called"; `Context::discover` decides what may go in a
    // header, and it applies the same rule to the install id — which comes out
    // of a file a person can hand-edit and can be just as unsendable.
    (!key.is_empty()).then_some(key)
}

/// Walk UP from *start* looking for `.yadgar/project-id`.
///
/// An ancestor's file overrides remote derivation, which is what makes a
/// monorepo subproject and a fresh checkout with no remote workable at all.
///
/// **THE FIRST FILE FOUND ENDS THE WALK, EVEN WHEN IT IS EMPTY**, and the
/// caller then falls through to the git remote. That is what
/// `_walk_project_id_file` does — it returns the first file's stripped contents
/// unconditionally, and `mint_project_id`'s `if override:` is falsy for `""` —
/// so an empty file means "I have nothing to say, use the remote".
///
/// While this kept walking, an empty file was TRANSPARENT and a grandparent's
/// file won instead. Python would have used the git remote and this would have
/// used the ancestor: two clients, two keys, one repository — the exact
/// split-namespace failure the transcription exists to prevent, reachable by
/// nothing more exotic than `touch .yadgar/project-id`.
fn project_id_file(start: &Path) -> Option<String> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    for dir in start.ancestors() {
        let candidate = dir.join(".yadgar").join("project-id");
        if candidate.is_file() {
            // `.ok()?` rather than `continue`: a file that exists and cannot be
            // read is not the same as one that is not there, and reading past
            // it would quietly use a different key than the one somebody wrote.
            let body = std::fs::read_to_string(&candidate).ok()?;
            return Some(body.trim().to_string());
        }
    }
    None
}

/// Reduce a git remote URL to its `owner/repo` — or `group/sub/repo` — form.
///
/// Three shapes, and the middle one is the one a hand-written parser gets
/// wrong. `git@host:path` and a bare `alias:path` both strip up to the first
/// colon, but ONLY when nothing before that colon is a `/`: a real remote URL
/// never has a slash before the host colon, while a path such as
/// `m-agahi/yadgar.io` does — and mistaking it for an SSH remote would eat the
/// owner.
fn normalise_remote(url: &str) -> String {
    let mut s = url.trim().to_string();

    if let Some(rest) = after_scheme(&s) {
        // `scheme://host/path` — drop the host as well as the scheme.
        //
        // A URL WITH NO PATH KEEPS THE HOST, which looks wrong and is what
        // Python does: `host_end = stripped.find("/")` followed by `if host_end
        // >= 0` leaves `stripped` as the host when there is no slash. Producing
        // `""` here instead meant the header was OMITTED where Python sent a
        // key — a disagreement on a degenerate remote, but a disagreement, and
        // the whole value of transcribing this is that there are none.
        s = match rest.find('/') {
            Some(slash) => rest[slash + 1..].to_string(),
            None => rest.to_string(),
        };
    } else if let Some(rest) = after_ssh_host(&s) {
        s = rest.to_string();
    }

    // A TRAILING `.git` ONLY, never mid-path.
    if let Some(stripped) = s.strip_suffix(".git") {
        s = stripped.to_string();
    }
    s.to_lowercase()
}

/// `scheme://` where scheme is `[A-Za-z][A-Za-z0-9+.-]*`, and what follows it.
fn after_scheme(s: &str) -> Option<&str> {
    let (scheme, rest) = s.split_once("://")?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
    .then_some(rest)
}

/// `[user@]host:` — scp-style, and the bare-alias form an `insteadOf` produces.
///
/// A bare SSH alias matters here rather than being an edge case: this machine's
/// git rewrites codeberg remotes to `codeberg-agent:owner/repo`, which has no
/// `user@` at all, and reading that as a path would key every such repository
/// under `codeberg-agent:owner/repo`.
fn after_ssh_host(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let prefix = &s[..colon];
    // A real remote URL never has a `/` before the host colon.
    if prefix.is_empty() || prefix.contains('/') {
        return None;
    }
    // At most one `@`, and neither half may be empty.
    match prefix.split_once('@') {
        Some((user, host)) if user.is_empty() || host.is_empty() || host.contains('@') => None,
        _ => Some(&s[colon + 1..]),
    }
}

/// Apply `insteadOf` rewrites to *url* until a fixed point.
///
/// BOUNDED at sixteen passes. A table mapping `alpha` to `beta` and `beta` back
/// to `alpha` is legal git configuration and must not spin forever — a client
/// that hangs on startup is worse than one that reports the wrong key.
///
/// **WHEN TWO RULES BOTH PREFIX-MATCH, NEITHER CLIENT IMPLEMENTS GIT'S OWN
/// RULE, AND THEY DISAGREE WITH EACH OTHER.** git picks the LONGEST matching
/// source; Python iterates a `dict` in insertion order, and this iterates a
/// `BTreeMap` in sorted order, so with an overlapping table the two can rewrite
/// to different targets and derive different keys. Left as it is deliberately:
/// matching Python here means matching a bug, and matching git means
/// diverging from Python — the one thing the transcription exists to avoid — so
/// the honest move is to write it down rather than pick silently. Overlapping
/// `insteadOf` sources are rare and a single rule is unaffected. If it ever
/// bites, BOTH clients want git's longest-match rule, in one change.
fn apply_insteadof(rules: &BTreeMap<String, String>, url: &str) -> String {
    let mut current = url.to_string();
    for _ in 0..16 {
        let mut changed = false;
        for (target, source) in rules {
            if current.starts_with(source.as_str()) && &current != target {
                current = format!("{target}{}", &current[source.len()..]);
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    current
}

/// `url.<REWRITE>.insteadOf <SOURCE>` as git reports it.
fn insteadof_rules() -> BTreeMap<String, String> {
    let raw = git(&["config", "--get-regexp", r"^url\..*\.insteadof$"], None);
    insteadof_rules_from(raw.as_deref().unwrap_or_default())
}

/// Pure, so the table can be driven without a real gitconfig.
fn insteadof_rules_from(raw: &str) -> BTreeMap<String, String> {
    let mut rules = BTreeMap::new();
    for line in raw.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let key = key.to_lowercase();
        let Some(target) = key
            .strip_prefix("url.")
            .and_then(|k| k.strip_suffix(".insteadof"))
        else {
            continue;
        };
        if !target.is_empty() {
            // The KEY is lowercased by git itself; the VALUE is not, and must
            // not be — it is matched against a URL as a prefix.
            rules.insert(target.to_string(), value.to_string());
        }
    }
    rules
}

/// The repository root for *cwd*, or *cwd* itself when there is none.
///
/// THE ONE HARDENED CALL, exactly as in the Python source: it is the first git
/// invocation in an untrusted directory, and it is the one whose `.git/config`
/// has not yet been shown to be anybody's.
fn repository_root(cwd: &Path) -> PathBuf {
    git_with(&["rev-parse", "--show-toplevel"], Some(cwd), Hardening::On)
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn origin_remote(root: &Path) -> Option<String> {
    git(&["config", "remote.origin.url"], Some(root))
}

/// Run git, or return `None` for every way that can fail.
///
/// No git, no repository, no `origin` — all collapse to `None` so the caller has
/// one branch rather than three. The client runs on laptops, and a machine
/// without git in `PATH` must still serve.
///
/// **ONE DELIBERATE DIVERGENCE FROM THE PYTHON SOURCE, stated rather than
/// hidden.** `_origin_remote` and `_resolve_project_root` both pass
/// `timeout=2`; there is no timeout here. `std::process` has none, so matching
/// it means a hand-rolled polling loop on the startup path — code no test in
/// this repository could exercise without a fake `git` on `PATH`. Both commands
/// are local and touch no network, so the hang being guarded against is remote
/// enough that untested machinery is the worse trade. If a hang is ever
/// observed, the guard is the fix and this paragraph is the reason it was not
/// written first.
fn git(args: &[&str], cwd: Option<&Path>) -> Option<String> {
    git_with(args, cwd, Hardening::Off)
}

/// Whether this invocation runs with the config sources shut off.
///
/// **IT CANNOT BE ON EVERYWHERE, and that is a constraint rather than a
/// choice.** Hardening points `GIT_CONFIG_GLOBAL` at `/dev/null`, and the
/// user's global config is exactly where `url.<x>.insteadOf` lives — so
/// hardening [`insteadof_rules`] would read an empty table and silently stop
/// rewriting, changing the derived key on every machine that uses a rewrite.
/// The Python source draws the line in the same place, hardening
/// `_resolve_project_root` alone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hardening {
    On,
    Off,
}

fn git_with(args: &[&str], cwd: Option<&Path>, hardening: Hardening) -> Option<String> {
    let mut command = Command::new("git");
    if hardening == Hardening::On {
        // A hostile `.git/config` in a directory somebody was handed can set
        // `core.fsmonitor` or `core.sshCommand` and get arbitrary commands run
        // on the next git invocation. `serve` runs git in whatever directory an
        // agent was started in, so that directory is exactly as trusted as
        // whatever cloned it. Transcribed from `_git_safe_env` and
        // `_GIT_SAFE_ARGS`.
        command
            .args([
                "-c",
                "protocol.allow=never",
                "-c",
                "protocol.file.allow=never",
                "-c",
                "uploadpack.allowFilter=false",
            ])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
    }
    if let Some(cwd) = cwd {
        command.arg("-C").arg(cwd);
    }
    let out = command.args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests;

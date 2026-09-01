//! Resolving a command path durable enough to bake into somebody's settings.
//!
//! `settings.json` carries literal command strings, and they outlive the process
//! that wrote them by months. The Python version was poisoned three times by
//! baking a path that existed only inside an agent worktree or a temp venv: once
//! the worktree was cleaned every hook failed with "No such file or directory",
//! and nothing reported it.
//!
//! So an ephemeral path is REFUSED, not substituted. Substituting guesses at
//! which binary the person meant; refusing names the problem while they are
//! still standing at the terminal.

use std::path::{Path, PathBuf};

/// Agent worktrees are created and deleted per task — a path through one is
/// dead by the time the next session starts.
///
/// TWO DIRECTORY NAMES, not a substring with separators baked into it. The
/// substring form was `"/.claude/worktrees/"`, and it cannot match on Windows:
/// `C:\Users\x\repo\.claude\worktrees\w\yaadgaar.exe` spells its separators
/// the other way, so the refusal D76 records as having been poisoned three
/// times never fired there at all. This client ships to Windows.
const WORKTREE_MARKER: [&str; 2] = [".claude", "worktrees"];

/// This binary's own path, if it is somewhere that will still exist tomorrow.
pub fn resolve_durable_command() -> anyhow::Result<PathBuf> {
    // NOT canonicalised beyond what the OS gives us. A symlink in /tmp pointing
    // at a durable binary is still a doomed registration once /tmp is cleaned,
    // so the path that would be WRITTEN is the path that gets judged.
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("cannot determine this binary's own path: {e}"))?;
    if let Some(reason) = ephemeral_reason(&exe) {
        anyhow::bail!(
            "refusing to register {} in settings.json: {reason}.\n\
             Every hook would break the moment that path goes away, silently — \
             which is how the capture pipeline died three times before. \
             Install yaadgaar to a permanent location and run `yaadgaar install` from there.",
            exe.display(),
        );
    }
    Ok(exe)
}

/// Why *path* must not be written into a persistent config, or `None` if it may.
///
/// Filesystem reads only — no `git` subprocess. `verify` calls this too, and a
/// scheduled health check must not shell out on a machine where `git` may not
/// even be installed.
pub fn ephemeral_reason(path: &Path) -> Option<String> {
    if is_inside_agent_worktree(path) {
        return Some(
            "it is inside an agent worktree (.claude/worktrees), which is deleted \
             when the agent that made it finishes"
                .to_string(),
        );
    }
    for root in temp_roots() {
        if path.starts_with(&root) {
            return Some(format!(
                "it is under the temporary directory {}, whose contents are not \
                 promised to survive a reboot",
                root.display()
            ));
        }
    }
    linked_worktree_reason(path)
}

/// Does *path* run through a `.claude/worktrees` directory?
///
/// Compares DIRECTORY NAMES rather than searching for a substring, so the
/// answer does not depend on which way the separators lean.
fn is_inside_agent_worktree(path: &Path) -> bool {
    let names = directory_names(path);
    names
        .windows(2)
        .any(|pair| pair[0] == WORKTREE_MARKER[0] && pair[1] == WORKTREE_MARKER[1])
}

/// The path's components, split on the separators of BOTH platforms.
///
/// `Path::components` splits on `\` only when compiled for Windows, so a
/// Windows path examined anywhere else arrives as one long component and every
/// component-wise check silently answers "no". Splitting on both is also what
/// makes the Windows shape testable on the Linux CI this repository has — and a
/// Windows defect that only a Windows runner can catch is a defect nobody
/// catches, which is how this module came to have no Windows coverage at all.
///
/// The cost is a Unix directory named literally `.claude\worktrees` reading as
/// two components. That is a refusal carrying an explicit message, which is the
/// side of this trade to be wrong on.
fn directory_names(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .flat_map(|name| name.split('\\').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

fn temp_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir(), PathBuf::from("/tmp")];
    if let Ok(real) = std::fs::canonicalize(std::env::temp_dir()) {
        roots.push(real);
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Detects a linked git worktree ANYWHERE, not only under `.claude/worktrees`.
///
/// A linked worktree's `.git` is a FILE containing `gitdir: …`; a normal
/// checkout's is a directory. That one bit is the whole test, and it needs no
/// subprocess. The search stops at the first `.git` found, because that is the
/// repository the binary belongs to — a `.git` further up would be a different
/// repo's and says nothing about this path.
fn linked_worktree_reason(path: &Path) -> Option<String> {
    let mut dir = path.parent();
    while let Some(current) = dir {
        let git = current.join(".git");
        match std::fs::symlink_metadata(&git) {
            Ok(meta) if meta.is_file() => {
                return Some(format!(
                    "it is inside a linked git worktree ({}), which is removed when \
                     that worktree is pruned",
                    current.display()
                ));
            }
            Ok(_) => return None, // a real checkout — durable enough
            Err(_) => {}
        }
        dir = current.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_path_is_refused() {
        // The failure this prevents: an install run from inside an agent
        // worktree bakes that worktree's path into settings.json, and every
        // hook on the machine dies the moment the worktree is cleaned up. Three
        // separate occurrences in the Python version.
        let reason = ephemeral_reason(Path::new(
            "/home/someone/repo/.claude/worktrees/feat-x/target/debug/yadgar",
        ));
        assert!(reason.unwrap().contains("worktree"));
    }

    #[test]
    fn a_windows_worktree_path_is_refused_too() {
        // The failure this prevents: the marker was the substring
        // "/.claude/worktrees/", so on Windows — a platform this client ships
        // to — it never matched, and the refusal that D76 records as having
        // been poisoned three times silently did not exist there.
        //
        // NOT #[cfg(windows)]-gated, deliberately. CI here is Linux, and a
        // Windows check only a Windows runner can reach is a check nobody runs.
        let reason = ephemeral_reason(Path::new(
            r"C:\Users\someone\repo\.claude\worktrees\feat-x\target\debug\yaadgaar.exe",
        ));
        assert!(reason.unwrap().contains("worktree"));
    }

    #[test]
    fn an_ordinary_windows_install_path_is_accepted() {
        // The other half, and the reason the check compares directory names
        // rather than searching anywhere in the string for the two words.
        assert!(ephemeral_reason(Path::new(r"C:\Program Files\yaadgaar\yaadgaar.exe")).is_none());
    }

    #[test]
    fn a_temp_path_is_refused() {
        let reason = ephemeral_reason(&std::env::temp_dir().join("venv/bin/yadgar"));
        assert!(reason.unwrap().contains("temporary"));
    }

    #[test]
    fn an_ordinary_install_path_is_accepted() {
        // The other half of the rule: refusing everything would be just as
        // broken as refusing nothing.
        assert!(ephemeral_reason(Path::new("/usr/local/bin/yadgar")).is_none());
    }
}

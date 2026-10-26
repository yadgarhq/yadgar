//! The guard's own registration: Claude Code settings files and `.git/hooks`
//! (yaadgaar #240, ADR-1005).
//!
//! An agent that can edit `settings.json` can deregister the guard, and one
//! that can write `.git/hooks` can replace the checks a commit runs. So:
//!
//! * `Edit`, `Write`, `MultiEdit` and `NotebookEdit` on such a path refuse.
//!   The path is exact, after `.` and `..` are taken out lexically.
//! * A Bash word that names such a path, or may expand into one under the
//!   same glob reading as the exceptions file, refuses — unless the word is
//!   an argument of a command on [`READERS`]. A redirect onto such a path
//!   refuses whatever the command is.
//!
//! **THE READER ALLOWANCE IS AN ALLOWLIST SHAPE** (ADR-0971): a command is a
//! reader only when its resolved program is on [`READERS`] and it carries
//! none of the options that make that program write or run something else.
//! Anything else that names a protected path refuses.
//!
//! Protected:
//!
//! * `settings.json` and `settings.local.json` directly inside ANY `.claude`
//!   directory — the user's, and every project's, since any settings scope
//!   can set `disableAllHooks` — and directly inside `$CLAUDE_CONFIG_DIR`,
//!   compared as a whole path, never by its basename;
//! * `.git/hooks` and everything below it;
//! * a plugin's `hooks` directory and everything below it, and
//!   `installed_plugins.json`, below `.claude/plugins/` or
//!   `$CLAUDE_CONFIG_DIR/plugins/`, and a submodule's hooks below
//!   `.git/modules/` (ADR-1020): see [`more_hooks`].
//!
//! The installer is not locked out: `yaadgaar install` writes these files
//! from the binary, which is not a tool call, and the command line that runs
//! it names none of them.
//!
//! **WHAT THIS DOES NOT SEE**, as for the exceptions file: a command that
//! acts on a directory (`cd ~/.claude && rm settings.json`, `cp -r x .git/`),
//! a path built inside an interpreter, `git config core.hooksPath`, an
//! edit of `.git/config`, and a plugin's hooks declared inline in its
//! `plugin.json` rather than in `hooks/hooks.json`.

use std::path::Path;

use super::{allow, as_glob, brace_expand, glob_matches, OPEN_BRACE};
use crate::hook::guard::git_subcommand;
use crate::hook::guard::runs::executed;
use crate::install::shellword::{shell_commands, SimpleCommand};

const SETTINGS_FILES: &[&str] = &["settings.json", "settings.local.json"];

/// Programs that only read the files they are given. `less` (`-o` writes a
/// log), `bat` (`--pager` runs a command), `sed` (`-i`, `w`), `sort` (`-o`)
/// and `file` (`-C`) are deliberately absent. `git` is handled apart.
const READERS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "jq",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ls",
    "stat",
    "wc",
    "diff",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "readlink",
    "realpath",
    "test",
    "[",
    "echo",
    "printf",
];

/// The `git` subcommands that only read the work tree.
const GIT_READS: &[&str] = &[
    "diff",
    "log",
    "show",
    "status",
    "blame",
    "ls-files",
    "ls-tree",
    "check-ignore",
];

/// Whether an Edit/Write/MultiEdit/NotebookEdit *path* is protected.
pub(in crate::hook::guard) fn is_protected_path(path: &str, config_dir: Option<&Path>) -> bool {
    let config = config_dir.map(|c| normalized(&c.to_string_lossy()));
    path.split('\n')
        .any(|p| protected_components(&normalized(p), config.as_deref()))
}

/// Whether a Bash *line* writes, moves, links or deletes a protected path,
/// or names one outside a reading command.
pub(in crate::hook::guard) fn bash_touches_a_protected_path(
    line: &str,
    commands: &[SimpleCommand],
    config_dir: Option<&Path>,
) -> bool {
    // No word of a line with no separator holds two components, and no
    // literal value it proves does either: see [`may_be_protected`].
    if !line.contains(['/', '\\', OPEN_BRACE]) {
        return false;
    }
    // A data heredoc's body is not commands (`tamper/allow.rs`).
    let stripped = allow::data_heredocs_removed(line);
    let scanned;
    let (line, commands) = match &stripped {
        Some(stripped) => {
            scanned = shell_commands(stripped);
            (stripped.as_str(), scanned.as_slice())
        }
        None => (line, commands),
    };
    let literals = allow::Literals::of(line);
    let config = config_dir.map(|c| normalized(&c.to_string_lossy()));
    // As in `names_the_exceptions_file`: after a word ending in `$`, the next
    // command's first word starts with a substitution's output.
    let mut after_substitution = false;
    for c in commands {
        // Only a reader's OWN arguments are skipped. An assignment or a
        // wrapper word before it is checked like any other word:
        // `GIT_EXTERNAL_DIFF='rm .git/hooks/x' git diff` runs that command.
        let reader_args = reader_arguments(c);
        for (n, w) in c.words.iter().chain(&c.writes_to).enumerate() {
            if reader_args.is_some_and(|start| n >= start && n < c.words.len()) {
                continue;
            }
            let word = if n == 0 && after_substitution {
                format!("*{w}")
            } else {
                w.clone()
            };
            let Some(alternatives) = literals.substitute(&word) else {
                return true;
            };
            if alternatives
                .iter()
                .any(|a| may_be_protected(a, config.as_deref()))
            {
                return true;
            }
        }
        after_substitution |= c.words.iter().chain(&c.writes_to).any(|w| w.ends_with('$'));
    }
    false
}

/// Where the arguments of *command*'s program start in its words, when
/// that program only reads the files it names: see [`READERS`].
pub(super) fn reader_arguments(command: &SimpleCommand) -> Option<usize> {
    let (program, args) = executed(command)?;
    let start = command.words.len() - args.len();
    let reads = match program {
        "rg" => !args.iter().any(|a| a.starts_with("--pre")),
        "git" => {
            // `-c` can set a pager or a diff driver that runs anything, and
            // `--output` writes a file.
            let redirected = args
                .iter()
                .any(|a| a == "-c" || a.starts_with("--config-env") || a.starts_with("--output"));
            !redirected && git_subcommand(args).is_some_and(|(sub, _)| GIT_READS.contains(&sub))
        }
        _ => READERS.contains(&program),
    };
    reads.then_some(start)
}

/// Whether *word* names a protected path, or a shell expansion in it may.
fn may_be_protected(word: &str, config: Option<&[String]>) -> bool {
    // ONE COMPONENT CANNOT BE PROTECTED: a settings file needs its directory
    // and a hook needs `.git/hooks`, and an expansion stands for one
    // component here as in the exceptions check. Checked first because most
    // words are one component, and this runs at every nesting level.
    if !word.contains(['/', '\\', OPEN_BRACE]) {
        return false;
    }
    // A path inside a larger word, such as an interpreter's code string.
    let plain = word.to_ascii_lowercase().replace('\\', "/");
    let named = [
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".git/hooks/",
    ]
    .iter()
    .any(|s| plain.contains(s))
        || plain.ends_with(".git/hooks")
        || plain
            .split_once(".git/modules/")
            .is_some_and(|(_, rest)| rest.contains("/hooks/") || rest.ends_with("/hooks"))
        || plain
            .split_once(".claude/plugins/")
            .is_some_and(|(_, rest)| {
                rest.contains("/hooks/")
                    || rest.ends_with("/hooks")
                    || rest.starts_with("installed_plugins.json")
            });
    let in_config = config.is_some_and(|c| {
        let dir = c.join("/");
        SETTINGS_FILES
            .iter()
            .any(|f| plain.contains(&format!("{dir}/{f}")))
    });
    if named || in_config || protected_components(&normalized(word), config) {
        return true;
    }
    if !word.contains(['$', '*', '?', '[', OPEN_BRACE, '`']) {
        return false;
    }
    let Some(alternatives) = brace_expand(word) else {
        return true; // Too many alternatives to check: refuse rather than guess.
    };
    alternatives.iter().any(|alt| {
        let parts: Vec<String> = alt.split(['/', '\\']).map(as_glob).collect();
        globs_may_be_protected(&parts, config)
    })
}

/// [`protected_components`] for components that are glob patterns.
fn globs_may_be_protected(parts: &[String], config: Option<&[String]>) -> bool {
    let mut path: Vec<&str> = Vec::new();
    for part in parts {
        match part.as_str() {
            "" | "." => {}
            ".." => match path.pop() {
                // `$X/..` climbs out of a directory that may be anything.
                Some(last) if last.contains(['*', '?', '[']) => return true,
                _ => {}
            },
            p => path.push(p),
        }
    }
    let hooks = path
        .windows(2)
        .any(|w| glob_matches(w[0], ".git") && glob_matches(w[1], "hooks"));
    let Some((leaf, dirs)) = path.split_last() else {
        return false;
    };
    let settings = SETTINGS_FILES.iter().any(|f| glob_matches(leaf, f))
        && (dirs.last().is_some_and(|d| glob_matches(d, ".claude"))
            || config.is_some_and(|c| glob_matches(&dirs.join("/"), &c.join("/"))));
    hooks || settings || more_hooks(&path, config, glob_matches)
}

/// Whether exact, normalized *components* are a protected path.
fn protected_components(components: &[String], config: Option<&[String]>) -> bool {
    let hooks = components
        .windows(2)
        .any(|w| w[0] == ".git" && w[1] == "hooks");
    let Some((leaf, dirs)) = components.split_last() else {
        return false;
    };
    let settings = SETTINGS_FILES.contains(&leaf.as_str())
        && (dirs.last().is_some_and(|d| d == ".claude") || config == Some(dirs));
    hooks || settings || more_hooks(components, config, |c, n| c == n)
}

/// The hook registration ADR-1020 adds, matched component by component
/// with *is* (exact, or a glob):
///
/// * a plugin's `hooks` directory and everything below it (`hooks.json`
///   and the scripts it runs) at any depth below `.claude/plugins/` or
///   `$CLAUDE_CONFIG_DIR/plugins/` — the real layouts are
///   `cache/<market>/<plugin>/<version>/` and
///   `marketplaces/<market>/plugins/<plugin>/` — and that directory's
///   `installed_plugins.json`, which says which plugins load;
/// * a submodule's hooks: `hooks` at or below `.git/modules/<name>/`, where
///   a name may hold `/` and submodules nest (`modules/a/modules/b/hooks`).
fn more_hooks<S: AsRef<str>>(
    path: &[S],
    config: Option<&[String]>,
    is: impl Fn(&str, &str) -> bool,
) -> bool {
    let at = |i: usize, name: &str| path.get(i).is_some_and(|p| is(p.as_ref(), name));
    let submodule = (0..path.len()).any(|i| {
        at(i, ".git") && at(i + 1, "modules") && (i + 3..path.len()).any(|k| at(k, "hooks"))
    });
    let n = path.len();
    // Below the plugins directory at *plugins*: `installed_plugins.json`
    // directly, or a `hooks` component after at least one plugin directory.
    let registers = |plugins: usize| {
        (n == plugins + 2 && at(plugins + 1, "installed_plugins.json"))
            || (plugins + 2..n).any(|h| at(h, "hooks"))
    };
    let in_claude = (0..n).any(|i| at(i, ".claude") && at(i + 1, "plugins") && registers(i + 1));
    let in_config = config.is_some_and(|c| {
        c.len() < n
            && c.iter().enumerate().all(|(i, d)| at(i, d))
            && at(c.len(), "plugins")
            && registers(c.len())
    });
    submodule || in_claude || in_config
}

/// *path*'s components, lowercased, with `.` dropped and `..` applied.
///
/// IGNORING ASCII CASE, trailing dots and spaces and a `:stream` suffix, as
/// `is_the_exceptions_file` does, for the filesystems that ignore them.
pub(super) fn normalized(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in path.split(['/', '\\']) {
        if part == ".." {
            out.pop();
            continue;
        }
        let part = part.split(':').next().unwrap_or(part);
        let part = part.trim_end_matches(['.', ' ']);
        if !part.is_empty() {
            out.push(part.to_ascii_lowercase());
        }
    }
    out
}

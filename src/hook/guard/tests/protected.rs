//! The guard's own registration (yaadgaar #240, ADR-1005): Claude Code
//! settings files and `.git/hooks` are refused to agent edits and to Bash
//! writes. Reads stay allowed, through a listed set of reading programs.

use std::path::Path;

use super::*;

fn edit(tool: &str, path: &str) -> Value {
    let key = if tool == "NotebookEdit" {
        "notebook_path"
    } else {
        "file_path"
    };
    json!({"tool_name": tool, "tool_input": {key: path, "content": "{}"}})
}

const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

const PROTECTED_PATHS: &[&str] = &[
    "/home/x/.claude/settings.json",
    "/home/x/.claude/settings.local.json",
    // A project's settings can switch every hook off (`disableAllHooks`).
    "/home/x/repo/.claude/settings.json",
    "/home/x/repo/.claude/settings.local.json",
    ".claude/settings.local.json",
    r"C:\Users\x\.claude\settings.json",
    "/home/x/.CLAUDE/Settings.JSON",
    "/home/x/.claude/settings.json.",
    "/home/x/.claude/./settings.json",
    "/home/x/.claude/agents/../settings.json",
    "/home/x/repo/.git/hooks/pre-commit",
    "/home/x/repo/.git/hooks",
    "/home/x/repo/src/../.git/hooks/pre-push",
    r"C:\repo\.git\hooks\pre-commit",
];

const UNPROTECTED_PATHS: &[&str] = &[
    "/home/x/repo/.vscode/settings.json",
    "/home/x/repo/settings.json",
    "/home/x/.claude/agents/settings.json",
    "/home/x/.claude/CLAUDE.md",
    "/home/x/repo/.claude/agents/reviewer.md",
    "/home/x/repo/githooks/pre-commit",
    "/home/x/repo/.git/hooks-notes.md",
    "/home/x/.config/claude-work/settings.json",
];

#[test]
fn an_edit_of_a_settings_file_or_a_git_hook_is_refused() {
    for tool in EDIT_TOOLS {
        for path in PROTECTED_PATHS {
            assert_eq!(
                guard(&edit(tool, path)),
                Decision::Deny(PROTECTED.into()),
                "{tool} {path}"
            );
        }
        for path in UNPROTECTED_PATHS {
            assert_eq!(guard(&edit(tool, path)), Decision::Allow, "{tool} {path}");
        }
    }
}

#[test]
fn the_claude_config_dir_settings_are_protected_by_its_full_path() {
    let cfg = Some(Path::new("/home/x/.config/claude-work"));
    let judge = |path: &str| pre_tool_guard(&edit("Write", path), &Policy::ALL_ON, cfg);
    for path in [
        "/home/x/.config/claude-work/settings.json",
        "/home/x/.config/claude-work/settings.local.json",
        "/home/x/.config/claude-work/./settings.json",
    ] {
        assert_eq!(judge(path), Decision::Deny(PROTECTED.into()), "{path}");
    }
    // The basename alone is not the directory.
    for path in [
        "/tmp/claude-work/settings.json",
        "/home/x/.config/claude-work/agents/settings.json",
    ] {
        assert_eq!(judge(path), Decision::Allow, "{path}");
    }
    let bash = |cmd: &str| pre_tool_guard(&bash_call(cmd), &Policy::ALL_ON, cfg);
    assert_eq!(
        bash("cp /tmp/x /home/x/.config/claude-work/settings.json"),
        Decision::Deny(PROTECTED.into())
    );
    assert_eq!(
        bash("cat /home/x/.config/claude-work/settings.json"),
        Decision::Allow
    );
    assert_eq!(
        bash("cp /tmp/x /tmp/claude-work/settings.json"),
        Decision::Allow
    );
}

/// Bash writes, moves, deletes and links onto a protected path, and the
/// bypasses of the reader allowance (ADR-0971: each relaxation ships with a
/// test that a write through its shape is still refused).
const BASH_WRITES: &[&str] = &[
    "echo '{}' > ~/.claude/settings.json",
    "echo '{}' >> .claude/settings.local.json",
    "cp /tmp/x ~/.claude/settings.json",
    "cp /tmp/x /home/x/repo/.claude/settings.local.json",
    "mv /tmp/x ~/.claude/settings.local.json",
    "mv ~/.claude/settings.json /tmp/x",
    "rm ~/.claude/settings.json",
    "tee .claude/settings.local.json",
    "sed -i 's/a/b/' ~/.claude/settings.json",
    "perl -pi -e 's/a/b/' ~/.claude/settings.json",
    "ln -sf /dev/null .git/hooks/pre-commit",
    "rm -rf .git/hooks",
    "chmod -x .git/hooks/pre-push",
    "cp /tmp/hook .git/hooks/pre-push",
    "install -m 755 /tmp/hook .git/hooks/commit-msg",
    "cp x $CLAUDE_CONFIG_DIR/settings.json",
    "cp x ~/.c*/settings.json",
    "cp x ~/.claude/settings.*",
    "cp x ~/.claude/{settings.json,y}",
    "D=~/.claude; cp x $D/settings.json",
    "rm $GIT_DIR/hooks/pre-commit",
    "sudo cp x ~/.claude/settings.json",
    "timeout 5 cp x ~/.claude/settings.json",
    "git checkout -- .claude/settings.json",
    "sh -c 'rm .git/hooks/pre-commit'",
    r#"echo "$(cp x ~/.claude/settings.json)""#,
    "jq '.hooks={}' ~/.claude/settings.json > /tmp/s && mv /tmp/s ~/.claude/settings.json",
    r#"python3 -c "open('/home/x/.claude/settings.json','w').write('{}')""#,
    // Reader allowance bypasses: a redirect, a writing option, a pipe stage.
    "cat /tmp/x > ~/.claude/settings.json",
    "jq . /tmp/x > .git/hooks/pre-commit",
    "cat /tmp/x | tee ~/.claude/settings.json",
    "rg --pre ./x hooks ~/.claude/settings.json",
    "rg --pre=./x hooks ~/.claude/settings.json",
    "grep x /tmp/y 2> ~/.claude/settings.json",
    "less -o ~/.claude/settings.json /tmp/x",
    "git -C /tmp checkout ~/.claude/settings.json",
    "git show HEAD:x > .claude/settings.json",
    // An assignment or wrapper word before a reader is not its argument.
    r#"GIT_EXTERNAL_DIFF="sh -c 'rm .git/hooks/pre-commit'" git diff"#,
    r#"env GIT_EXTERNAL_DIFF="sh -c 'rm .git/hooks/pre-commit'" git diff"#,
    r#"GIT_PAGER="cp x ~/.claude/settings.json" git log"#,
    r#"bat --paging=always --pager "cp x ~/.claude/settings.json" /tmp/f"#,
    "bat ~/.claude/settings.json",
];

#[test]
fn a_bash_write_onto_a_protected_path_is_refused() {
    let wrong: Vec<(&str, String)> = BASH_WRITES
        .iter()
        .filter_map(|c| match guard(&bash_call(c)) {
            Decision::Deny(r) if r == PROTECTED => None,
            other => Some((*c, format!("{other:?}"))),
        })
        .collect();
    assert!(wrong.is_empty(), "not refused as protected: {wrong:#?}");
}

/// Reading a protected file, or writing a file that only shares its name.
const BASH_READS: &[&str] = &[
    "cat ~/.claude/settings.json",
    "jq .hooks ~/.claude/settings.json",
    "jq -r '.hooks.PreToolUse[0].matcher' .claude/settings.local.json",
    "grep -n hooks ~/.claude/settings.json",
    "rg hooks ~/.claude/settings.json",
    "head -5 .claude/settings.local.json",
    "ls .git/hooks",
    "ls -la .git/hooks/",
    "cat .git/hooks/pre-commit",
    "stat .git/hooks/pre-push",
    "diff ~/.claude/settings.json /tmp/b.json",
    "git diff -- .claude/settings.json",
    "git log -p -- .claude/settings.json",
    "cat ~/.claude/settings.json | jq .hooks",
    "sudo -u ops cat ~/.claude/settings.json",
    "timeout 5 jq . .claude/settings.local.json",
    "cp x .vscode/settings.json",
    "echo '{}' > .vscode/settings.json",
    "cp /tmp/hook githooks/pre-commit",
    // A PR body that names the paths, written as data and posted from a file.
    "cat > /tmp/pr.md <<'EOF'\nRefuses edits of ~/.claude/settings.json and .git/hooks/pre-commit.\nEOF\ngh pr create --body-file /tmp/pr.md",
];

#[test]
fn reading_a_protected_path_is_allowed() {
    let refused: Vec<(&str, String)> = BASH_READS
        .iter()
        .filter_map(|c| match guard(&bash_call(c)) {
            Decision::Allow => None,
            other => Some((*c, format!("{other:?}"))),
        })
        .collect();
    assert!(refused.is_empty(), "refused a read: {refused:#?}");
}

#[test]
fn the_installer_is_not_locked_out() {
    // The guard judges tool calls. `yaadgaar install`, run by nix or a person,
    // writes settings.json from the binary, which no hook sees; and an agent
    // asked to run the installer names no protected path on its command line.
    for line in [
        "yaadgaar install",
        "yaadgaar uninstall",
        "yaadgaar verify",
        "uvx yaadgaar install",
        "nix run github:yadgarhq/yadgar -- install",
        "home-manager switch --flake .",
        "sudo nixos-rebuild switch --flake .",
    ] {
        assert_eq!(guard(&bash_call(line)), Decision::Allow, "{line}");
    }
    // The installer itself writes the very file the guard refuses an agent.
    let home = std::env::temp_dir().join(format!(
        "yaadgaar-guard-installer-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    crate::install::install_with(&home, Path::new("/usr/local/bin/yaadgaar")).unwrap();
    let settings = crate::install::Layout::new(&home).settings();
    let written = std::fs::read_to_string(&settings).unwrap();
    assert!(written.contains("pre-tool-guard"), "{written}");
    assert_eq!(
        guard(&edit("Edit", &settings.to_string_lossy())),
        Decision::Deny(PROTECTED.into())
    );
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn hook_config_tamper_switched_off_allows_the_protected_paths() {
    let off = Policy {
        hook_config_tamper: false,
        ..Policy::ALL_ON
    };
    assert_eq!(
        pre_tool_guard(&edit("Write", "/home/x/.claude/settings.json"), &off, None),
        Decision::Allow
    );
    assert_eq!(
        pre_tool_guard(&bash_call("rm .git/hooks/pre-commit"), &off, None),
        Decision::Allow
    );
}

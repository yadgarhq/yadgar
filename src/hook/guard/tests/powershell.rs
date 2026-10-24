//! The PowerShell tool (yaadgaar #227, ADR-1005): judged at least as
//! strictly as Bash for terraform, commit flags, digger comments, the
//! exceptions file and the protected paths. A command whose text is hidden
//! from the guard refuses.

use super::*;

fn ps(command: &str) -> Value {
    json!({"tool_name": "PowerShell", "tool_input": {"command": command}})
}

const PS_TERRAFORM: &[&str] = &[
    "terraform apply",
    "Terraform.exe plan",
    "TOFU apply",
    r"& 'C:\tools\terraform.exe' plan",
    r"& C:\tools\tfp.cmd",
    "Start-Process terraform -ArgumentList apply",
    "saps tofu",
    "cmd /c tofu apply",
    r#"pwsh -Command "terraform apply""#,
    "iex 'terraform apply'",
    "Invoke-Expression \"terraform plan\"",
    "Get-ChildItem | ForEach-Object { tfp plan }",
    "$t = 'terraform'; & $t apply",
    "docker run hashicorp/terraform plan",
    "wsl terraform apply",
];

#[test]
fn powershell_that_names_a_terraform_binary_is_refused() {
    for command in PS_TERRAFORM {
        assert_eq!(reason_for(&ps(command)), TERRAFORM, "{command}");
    }
}

#[test]
fn powershell_is_judged_by_every_bash_rule_too() {
    for (command, reason) in [
        ("git commit --no-verify -m x", HOOK_BYPASS),
        ("git -c core.hooksPath=NUL commit -m x", HOOK_BYPASS),
        ("gh pr comment 1 --body 'digger apply'", DIGGER),
        (
            r"Set-Content $env:USERPROFILE\.claude\yadgar-hook-exceptions.json '{}'",
            EXCEPTIONS,
        ),
        // A backtick is PowerShell's escape character, not a substitution.
        (
            "Remove-Item ~/.claude/yadgar-hook-exc`eptions.json",
            EXCEPTIONS,
        ),
        (r"Remove-Item ~\.c*\y*", EXCEPTIONS),
        (r"Set-Content $HOME\.claude\settings.json '{}'", PROTECTED),
        (r"Copy-Item x .git\hooks\pre-commit", PROTECTED),
        (r"Remove-Item -Recurse .git\hooks", PROTECTED),
        ("'{}' | Out-File .claude/settings.local.json", PROTECTED),
    ] {
        assert_eq!(reason_for(&ps(command)), reason, "{command}");
    }
}

#[test]
fn powershell_whose_text_is_hidden_is_refused() {
    for command in [
        "powershell -EncodedCommand ZQBjAGgAbwA=",
        "pwsh -enc ZQBjAGgAbwA=",
        "pwsh.exe -e ZQBjAGgAbwA=",
        "powershell /ec ZQBjAGgAbwA=",
        "[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('ZQA=')) | iex",
    ] {
        assert_eq!(reason_for(&ps(command)), PS_UNJUDGEABLE, "{command}");
    }
}

#[test]
fn a_powershell_call_the_guard_cannot_read_is_refused() {
    for payload in [
        json!({"tool_name": "PowerShell"}),
        json!({"tool_name": "PowerShell", "tool_input": {"command": 7}}),
        json!({"tool_name": "PowerShell", "tool_input": "terraform apply"}),
    ] {
        assert_eq!(reason_for(&payload), UNREADABLE, "{payload}");
    }
    let long = format!("Write-Output {}", "x".repeat(70 * 1024));
    assert_eq!(reason_for(&ps(&long)), TOO_LONG);
}

#[test]
fn ordinary_powershell_is_allowed() {
    for command in [
        "Get-ChildItem docs",
        "cargo test",
        "git status",
        "Get-Content docs/terraform-notes.md",
        r"Get-Content infra\terraform\main.tf",
        "Select-String -Path docs\\*.md -Pattern 'terraforming'",
        "$env:RUST_LOG = 'debug'; cargo run -- verify",
        "Write-Output 'see the digger notes'",
    ] {
        assert_eq!(guard(&ps(command)), Decision::Allow, "{command}");
    }
}

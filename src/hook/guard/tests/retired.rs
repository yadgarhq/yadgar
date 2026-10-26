//! Protected mode retires the exceptions-file text rule (ADR-1028), and
//! nothing else: every other refusal still holds, and default mode refuses
//! exactly what it refused before.

use super::*;

/// The guard's environment in protected mode: the exceptions file is
/// root-owned and verified, so naming it in a Bash command is no threat.
fn protected() -> GuardEnv {
    GuardEnv {
        exceptions_protected: true,
        ..GuardEnv::default()
    }
}

fn decide(command: &str, env: &GuardEnv) -> Decision {
    pre_tool_guard(&bash_call(command), &Policy::ALL_ON, env)
}

fn decide_ps(command: &str, env: &GuardEnv) -> Decision {
    let call = json!({"tool_name": "PowerShell", "tool_input": {"command": command}});
    pre_tool_guard(&call, &Policy::ALL_ON, env)
}

/// PowerShell's own ways to run what the guard cannot read (sixth review).
const PS_FAIL_CLOSED: &[&str] = &[
    "& $x",
    "& notepad.exe",
    "& ('i'+'d')",
    ". ./profile.ps1",
    "iex $s",
    "Invoke-Expression $s",
    "INVOKE-EXPRESSION 'x'",
    "Get-Item ('a'+'b')",
    "Start-Process pwsh -Verb RunAs",
    "Get-Content x; . $p",
];

#[test]
fn protected_mode_refuses_powershell_indirection() {
    for line in PS_FAIL_CLOSED {
        match decide_ps(line, &protected()) {
            Decision::Deny(r) => assert_eq!(r, PS_PROTECTED, "{line}"),
            Decision::Allow => panic!("protected mode allowed: {line}"),
        }
    }
    for line in [
        "Get-ChildItem .",
        "Get-Content ./a.txt",
        "Write-Output 'a.b'",
    ] {
        // The PowerShell tool is refused ENTIRELY in protected mode
        // (seventh security review): fail closed.
        assert!(
            matches!(decide_ps(line, &protected()), Decision::Deny(r) if r == PS_PROTECTED),
            "{line}"
        );
        assert_eq!(
            decide_ps(line, &GuardEnv::default()),
            Decision::Allow,
            "{line}"
        );
    }
    assert_eq!(decide_ps("iex $s", &GuardEnv::default()), Decision::Allow);
}

/// The two commands of yaadgaar #431, in the neutral form the
/// exceptions-file corpus records (every shape kept, no real paths):
/// refused under the text rule only because a loop variable might expand
/// into the file.
const CMD_431: &[&str] = &[
    r#"cd /home/x/work/repo-wt-1 && echo "diffstat:[$(git diff --stat origin/master -- clusters/)]"; S=/tmp/claude-1000/-home-x-work-repo/878decb7-de3f-49b7-8e50-c074e8b8fe30/scratchpad/base; rm -rf $S; mkdir -p $S; git archive origin/master clusters | tar -xf - -C $S; for d in clusters/*/*/apps/app-config; do a=$(kustomize build $S/$d | sha256sum); b=$(kustomize build $d | sha256sum); [ "$a" = "$b" ] && echo "same $d ${a:0:12}" || echo "DIFF $d"; done"#,
    concat!(
        r#"cd /home/x/work/repo-wt-2 && S=/tmp/claude-1000/-home-x-work-repo/878decb7-de3f-49b7-8e50-c074e8b8fe30/scratchpad; for d in clusters/*/*/apps/app-config; do n=$(echo $d|cut -d/ -f2,3|tr / _); kustomize build $d > $S/after/$n.yaml 2>/dev/null || echo FAIL $d; done; for f in $S/before/*.yaml; do b=$(basename $f); cmp -s $f $S/after/$b && echo "identical $b" || { echo "DIFF $b"; diff $f $S/after/$b; }; done; git diff; cat clusters/region-a/dev/apps/app-config/secrets/sec_generated.yaml"#,
        "\n",
        r#"echo ---sops; grep -rl "provider: sops" clusters/region-a/dev/flux-system | head -3; git ls-files clusters/region-a/dev/apps/app-config | grep -v enc | grep -i secret; ls clusters/region-a/dev/apps/app-config/secrets; grep -rn "sec_in_cluster" clusters/region-a/preprod/apps/app-config/kustomization.yaml; grep -rn "sops" clusters/region-a/preprod/flux-system/kustomizations/app-config.yaml"#
    ),
];

/// Smaller shapes the text rule refuses for naming, or possibly naming,
/// the USER'S copy. In protected mode that copy is not read, and the
/// protected file is root-owned, so these are no threat.
const TEXT_RULE_ONLY: &[&str] = &[
    "for d in a b; do cat $S/$d; done",
    "b=x; cmp -s $f $S/after/$b",
    "cat ~/.claude/yadgar-hook-exceptions.json",
];

/// The protected file by its literal path: refused in protected mode (a
/// literal match, no variable guessing), read or write. The Read tool reads it.
const PROTECTED_LITERAL: &[&str] = &[
    "cat /etc/yaadgaar/yadgar-hook-exceptions.json",
    "jq . '/etc/yaadgaar/yadgar-hook-exceptions.json'",
    "ls /ETC/YAADGAAR",
    "tee /etc/yaa\\dgaar/x",
    "sh -c 'cat /etc/yaadgaar/yadgar-hook-exceptions.json'",
    // Spellings a substring misses (security review).
    "cat //etc//yaadgaar/x",
    "cat /etc/./yaadgaar/x",
    "cat /etc/yaad*/x",
    "cat /e?c/yaadgaar/x",
    "cat /etc/yaad{gaar,x}/y",
    "cat /etc/[y]aadgaar/y",
    // A cd toward it (seventh security review).
    "cd /etc",
    "cd /etc/ && cat yaadgaar/x",
    "pushd /ETC",
    "cd yaadgaar",
    "cd ./YAADGAAR/",
    "cd //etc",
];

/// Privilege escalation, refused outright in protected mode: an agent with
/// a cached sudo timestamp could otherwise rewrite the root-owned file.
const ESCALATION: &[&str] = &[
    "sudo tee /etc/yaadgaar/yadgar-hook-exceptions.json",
    "sudo -n cp x /etc/yaadgaar/yadgar-hook-exceptions.json",
    "S=/etc/yaadgaar; sudo cp x $S/f",
    "sudo -n true",
    "sudo -u root -- sh",
    "doas sh",
    "env sudo -s",
    "env -i sudo -s",
    "/usr/bin/sudo -i",
    "command sudo x",
    "exec sudo x",
    "nice -n 5 sudo x",
    "timeout 5 sudo x",
    "xargs sudo cp",
    "find . -exec sudo cp {} /x ';'",
    "sudoedit f",
    "su -c x",
    "su",
    "pkexec x",
    "run0 x",
    "systemd-run --uid=0 x",
    "machinectl shell root@",
    "S=sudo; $S tee x",
    "echo \"$(sudo cat x)\"",
    "sh -c 'sudo x'",
    "eval sudo x",
    "true && sudo x",
    "for i in 1; do sudo x; done",
    // Glob, brace, tilde and path spellings of the program (security review).
    "/usr/bin/su?o -n true",
    "/usr/bin/s[u]do x",
    "/usr/bin/s* x",
    "/usr/bin/{sudo,x} y",
    "/usr/{bin/sudo,x} y",
    "~root/../usr/bin/sudo x",
    "//usr//bin/./sudo x",
    "/run/wrappers/bin/sudo x",
    // Programs that run another program (security review).
    "chroot / sudo x",
    "flock /tmp/l sudo x",
    "watch sudo x",
    "taskset 1 sudo x",
    "find . -execdir /usr/bin/sudo x ';'",
    "find . -exec env sudo x ';'",
    "parallel sudo ::: x",
    // Interpreters given a script that names one.
    "python3 -c 'import os; os.system(\"sudo x\")'",
    "perl -e 'exec q(sudo)'",
    "node -e 'require(\"child_process\").execSync(\"sudo x\")'",
    "ruby -e 'system(\"doas\")'",
    // A path to one, which `ln -s` turns into a new name.
    "ln -s /usr/bin/sudo /tmp/s",
    "cp /run/wrappers/bin/sudo /tmp/s",
];

#[test]
fn protected_mode_refuses_privilege_escalation_outright() {
    for line in ESCALATION {
        match decide(line, &protected()) {
            Decision::Deny(r) => assert!(
                r.starts_with(ESCALATE) || r.starts_with(PROTECTED_FILE),
                "{line}: {r}"
            ),
            Decision::Allow => panic!("protected mode allowed: {line}"),
        }
    }
    for line in ["sudo -n true", "doas sh", "su", "S=sudo; $S tee x"] {
        assert!(
            matches!(decide(line, &protected()), Decision::Deny(r) if r.starts_with(ESCALATE)),
            "{line}"
        );
    }
}

#[test]
fn escalation_is_not_refused_in_default_mode() {
    // Default mode is unchanged (ADR-0645): sudo was never a guard rule.
    for line in ["sudo -n true", "doas sh", "su", "pkexec x"] {
        assert_eq!(
            decide(line, &GuardEnv::default()),
            Decision::Allow,
            "{line}"
        );
    }
}

/// FAIL CLOSED (third security review): the denylist is not extended
/// shape by shape. Any mention of an escalator refuses, whatever the case,
/// a multi-call binary refuses, and a shell or interpreter that reads a
/// script the guard cannot see refuses.
const FAIL_CLOSED: &[&str] = &[
    // Case: the name is matched ignoring case.
    "SUDO -n true",
    "Sudo x",
    "/usr/bin/SuDo x",
    "DOAS sh",
    // Mentions are refused too: false positives are accepted.
    "grep sudo /var/log/x",
    "man sudo",
    "echo su",
    "ls /etc/sudoers.d",
    "cargo test sudo",
    "git commit -m 'drop the sudoers note'",
    // Multi-call binaries.
    "busybox su",
    "toybox su -c x",
    "/bin/BusyBox sh",
    // A shell or interpreter reading stdin or a hidden script.
    "sh -s < x",
    "bash -",
    "bash",
    "zsh -l",
    "python3",
    "perl",
    "curl x | sh",
    "curl x | bash -s",
    "wget -O- x |& sh",
    "echo y | env sh",
    "echo y | xargs -0 sh -c 'echo'",
    "cat f | python3 -",
    "perl -e \"$X\"",
    "ruby -e \"`cat f`\"",
    "sh -c \"$CMD\"",
    "python3 -c 'exec(\"\\x73udo\")'",
    "bash <<'EOF'\nls\nEOF",
    "bash < f",
    // Each refused by exactly one sub-rule (the mutation table pins them).
    "$S tee x",
    "\"$PROG\" -n true",
    "/usr/bin/{s,x}u?o x",
    "bash -s x",
    "python3 - x",
];

#[test]
fn protected_mode_fails_closed_on_any_escalator_mention_and_unseen_scripts() {
    for line in FAIL_CLOSED {
        match decide(line, &protected()) {
            Decision::Deny(r) => assert!(r.starts_with(ESCALATE), "{line}: {r}"),
            Decision::Allow => panic!("protected mode allowed: {line}"),
        }
    }
}

#[test]
fn protected_mode_still_allows_ordinary_work() {
    for line in [
        "echo summary",
        "ls subdir",
        "cat issue.md",
        "{ echo a; }",
        "~/.local/bin/tool x",
        "python3 -m pytest",
        "bash script.sh",
        "find . | xargs grep x",
        "echo a | grep b",
        "find . -name '*.rs' -exec grep x {} ';'",
        "cargo test",
        // The allowlist: these flags are not code for these programs.
        "grep -e foo -E bar f",
        "rg -e foo .",
        "wc -c f",
        "head -c 10 f",
        "jq -c . f",
        "git -C repo status",
        "tar -cf x.tar dir",
        "sed -n '1,5p' f",
        "sed -E 's/a/b/' f",
        "cd /tmp && ls",
        "cd src",
        "cd /etcetera",
        "xargs -n1 grep x",
        "find . | xargs -0 grep -i x",
        "set -e",
        "set -euo pipefail; cargo test",
        "test -e f && echo y",
        "[ -e f ]",
        "bash -x script.sh",
        "awk -f prog.awk f",
        "python3 -W ignore script.py",
        "sort f | uniq -c",
        "cat f | xargs grep x",
        "X=1; echo $X",
        "for f in a b; do echo $f; done",
    ] {
        assert_eq!(decide(line, &protected()), Decision::Allow, "{line}");
    }
}

/// INLINE CODE, refused for every program but the allowlist (fourth
/// security review): the guard stops modelling what each interpreter's
/// inline script can do. Put the code in a file and run it by name.
const INLINE_CODE: &[&str] = &[
    "python3 -c 'print(1)'",
    "sh -c 'ls | wc -l'",
    "bash -c ls",
    "node -e 1",
    "perl -E 'say 1'",
    "awk '{print $1}' f",
    "gawk 'BEGIN{system(\"id\")}'",
    "mawk -F: '{print}' f",
    "nawk 1 f",
    "expect -c 'spawn id'",
    "gdb -batch -ex 'call system(\"id\")'",
    "gdb --eval-command=run",
    "vim -c ':!id' f",
    "nvim +'!id' f",
    "ex +q f",
    "emacs --batch --eval '(shell-command \"id\")'",
    "env -S 'id -u'",
    "Rscript -e 'system(\"id\")'",
    "pwsh -Command id",
    "pwsh -EncodedCommand aQBkAA==",
    "powershell -c id",
    "git -c core.pager=id log",
    "mytool --eval x",
    "mytool --command=x",
    "env -S 'id' true",
    // Short-flag clusters carrying a code flag (fifth security review).
    "bash -xc 'ls'",
    "sh -ec ls",
    "perl -le 'print 1'",
    "python3 -Sc pass",
    "python3 -Sc pass arg",
    "ruby -we 'p 1' arg",
    // A program the guard cannot determine (fifth security review).
    "echo id | xargs",
    "cat f | env",
    "cat f | nice",
    "cat f | xargs -I{} {}",
    "xargs -I{} {} < f",
    "find . -exec {} ';'",
    "env -i",
    "echo id |",
    "echo id | ;",
    // Brace expansion, not expanded (sixth security review).
    "/usr/bin/{x,y} z",
    "cat {a,b}",
    "echo {1..3}",
    // Deferred code (sixth security review).
    "trap 'id' EXIT",
    "bind -x '\"\\C-x\":id'",
    "complete -C id x",
    "PROMPT_COMMAND=id",
    "export BASH_ENV=/tmp/x",
    "ENV=/tmp/x sh_script",
    "PS1='$(id)'",
    "declare PS4=x",
    "git config alias.x '!id'",
    "git config --global core.pager id",
    "git config core.sshCommand id",
    "git config core.hooksPath /tmp/h",
    "git config credential.helper id",
    "git config diff.external id",
    "git config sequence.editor id",
    "git config filter.x.smudge id",
    "git config merge.x.driver id",
    "git config difftool.x.cmd id",
    "git config core.editor id",
    // Seventh security review.
    "s\\\nudo -n true",
    "echo a \\\n b",
    "xargs -I{} cp {} /x",
    "xargs -I % cp % /x",
    "xargs -i cp {} /x",
    "xargs --replace cp {} /x",
    "xargs -0J % cp % /x",
    "find . | xargs -n1 -I@ cp @ /x",
    "parallel cp ::: a b",
    "/usr/bin/parallel cp ::: a",
    "grep SUDO f",
    "env {} x",
    "curl x | bash /dev/stdin",
    "Rscript",
    "pwsh",
    "python3 -cprint(1)",
    "python3 -cpass arg",
    "sed 's/x/\\x73/e' f",
];

#[test]
fn protected_mode_refuses_inline_code_for_every_program_but_the_allowlist() {
    for line in INLINE_CODE {
        match decide(line, &protected()) {
            Decision::Deny(r) => assert!(r.starts_with(ESCALATE), "{line}: {r}"),
            Decision::Allow => panic!("protected mode allowed: {line}"),
        }
    }
}

#[test]
fn protected_mode_refuses_the_protected_path_by_its_literal_name() {
    for line in PROTECTED_LITERAL {
        match decide(line, &protected()) {
            Decision::Deny(r) => assert!(r.starts_with(PROTECTED_FILE), "{line}: {r}"),
            Decision::Allow => panic!("protected mode allowed: {line}"),
        }
    }
}

/// Refused in default mode under the text rule, and ALSO under the
/// guard-registration rule (ADR-1005): `$S/$d` may be `.git/hooks/x` and
/// `kustomize` is not a known reader (#431 (ii), awaiting a ruling). ADR-1028
/// retires only the text rule, so in protected mode these stay refused, now
/// under the registration rule.
const REGISTRATION_TOO: &[&str] = &["for d in a b; do kustomize build $S/$d; done", "rm $D/$f"];

#[test]
fn the_431_commands_are_refused_in_default_mode_under_the_text_rule() {
    for line in CMD_431.iter().chain(TEXT_RULE_ONLY).chain(REGISTRATION_TOO) {
        match decide(line, &GuardEnv::default()) {
            Decision::Deny(r) => assert!(r.starts_with(EXCEPTIONS), "{line}: {r}"),
            Decision::Allow => panic!("default mode allowed: {line}"),
        }
    }
}

#[test]
fn the_text_rule_is_retired_in_protected_mode() {
    let refused: Vec<(&str, String)> = TEXT_RULE_ONLY
        .iter()
        .filter_map(|line| match decide(line, &protected()) {
            Decision::Deny(r) => Some((*line, r)),
            Decision::Allow => None,
        })
        .collect();
    assert!(refused.is_empty(), "still refused: {refused:#?}");
}

#[test]
fn the_431_commands_are_refused_in_protected_mode_only_under_the_registration_rule() {
    for line in CMD_431.iter().chain(REGISTRATION_TOO) {
        match decide(line, &protected()) {
            Decision::Deny(r) => assert!(r.starts_with(PROTECTED), "{line}: {r}"),
            Decision::Allow => panic!("allowed, so the registration rule moved: {line}"),
        }
    }
}

#[test]
fn the_text_rule_stays_retired_inside_nested_shells() {
    // `bash` recurses with the same environment, so a nested `sh -c` and
    // a substitution are judged in protected mode too.
    for line in [
        "echo \"$(cat ~/.claude/yadgar-hook-exceptions.json)\"",
        "echo \"$(cat $D/$f)\"",
    ] {
        assert!(
            matches!(decide(line, &GuardEnv::default()), Decision::Deny(_)),
            "{line}"
        );
        assert_eq!(decide(line, &protected()), Decision::Allow, "{line}");
    }
}

/// What protected mode must still refuse: every rule but the text rule.
const STILL_REFUSED: &[(&str, &str)] = &[
    ("terraform apply", TERRAFORM),
    ("git commit --no-verify -m x", HOOK_BYPASS),
    (r#"gh pr comment 1 --body "digger apply""#, DIGGER),
    ("echo '{}' > ~/.claude/settings.json", PROTECTED),
    ("rm .git/hooks/pre-commit", PROTECTED),
    ("alias gh=true", SHADOW),
    // Nesting without `-c`, which protected mode refuses as inline code.
    ("echo $(echo $(echo $(echo $(echo x))))", TOO_DEEP),
];

#[test]
fn protected_mode_still_refuses_every_other_rule() {
    for (line, why) in STILL_REFUSED {
        match decide(line, &protected()) {
            Decision::Deny(r) => assert!(r.starts_with(why), "{line}: {r}"),
            Decision::Allow => panic!("protected mode allowed: {line}"),
        }
    }
}

#[test]
fn protected_mode_still_refuses_an_edit_of_the_exceptions_file_by_path() {
    // The Edit rule is an exact path, not a text check: it never refused
    // anything harmless, and the user's copy is what the memory engine
    // still reads.
    for tool in ["Edit", "Write", "MultiEdit"] {
        let call = json!({"tool_name": tool, "tool_input": {
            "file_path": "/home/x/.claude/yadgar-hook-exceptions.json"}});
        assert!(
            matches!(pre_tool_guard(&call, &Policy::ALL_ON, &protected()), Decision::Deny(r) if r == EXCEPTIONS),
            "{tool}"
        );
    }
}

#[test]
fn hook_config_tamper_off_still_switches_the_text_rule_off_in_default_mode() {
    let off = Policy {
        hook_config_tamper: false,
        ..Policy::ALL_ON
    };
    let call = bash_call("cat ~/.claude/yadgar-hook-exceptions.json");
    assert_eq!(
        pre_tool_guard(&call, &off, &GuardEnv::default()),
        Decision::Allow
    );
}

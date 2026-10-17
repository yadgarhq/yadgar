//! Unit tests for the rules file and the `CLAUDE.md` reference line.

use super::*;

/// The tools the gateway serves, as `tools/list` names them.
///
/// **A TEST FIXTURE, NOT A REGISTRY.** This client holds no per-tool code
/// (D75): `tools/list` is forwarded and answered by the gateway alone. The
/// rules file is the one place this repository NAMES tools, in prose for
/// the agent, and that prose has to name tools that exist. So the names
/// live here, inside `#[cfg(test)]`, and nowhere in the shipped binary.
///
/// **IT PINS `assets/rules.md` TO A HAND-COPIED LIST, NOT TO THE LIVE
/// GATEWAY.** Nothing here can see what a deployment serves; a gateway that
/// renames a tool leaves this green. The rules therefore point the agent at
/// the `tools/list` descriptions as the authority and name these five as
/// the current set.
///
/// Source: `yadgarhq/gateway` `src/tools.rs` `SERVED` at `2b970b7`, and the
/// `tools/list` measured on a VM against chart v0.24.17 on 2026-10-17
/// (ledger 1543), which returned exactly these five. When the gateway
/// grows a tool, this list and `assets/rules.md` change together.
const GATEWAY_TOOLS: [&str; 5] = [
    "create_task",
    "read_task",
    "find_tasks",
    "edit_task",
    "transition_task",
];

/// Every `backticked` span in *text*.
fn backticked(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

#[test]
fn the_rules_name_every_tool_the_gateway_serves() {
    // Ledger 1543: the rules told the agent to call `task_list`,
    // `task_get` and `task_write` — none of which the gateway serves — so
    // an agent following its own instructions failed on every call.
    let spans = backticked(RULES_BODY);
    for tool in GATEWAY_TOOLS {
        assert!(
            spans.contains(&tool),
            "the rules never name `{tool}`, which the gateway serves"
        );
    }
}

#[test]
fn the_rules_name_no_tool_the_gateway_does_not_serve() {
    // A backticked identifier that LOOKS like a tool — about tasks,
    // memories, the wiki, or recall — and is not served is an instruction
    // the agent cannot follow. Status names and argument names
    // (`in_progress`, `expect_version`) carry none of these words.
    const TOOL_WORDS: [&str; 6] = ["task", "tasks", "memory", "memorize", "wiki", "recall"];
    for span in backticked(RULES_BODY) {
        let identifier =
            !span.is_empty() && span.chars().all(|c| c.is_ascii_lowercase() || c == '_');
        let tool_like = span.split('_').any(|w| TOOL_WORDS.contains(&w));
        if identifier && tool_like {
            assert!(
                GATEWAY_TOOLS.contains(&span),
                "the rules name `{span}`, which the gateway does not serve"
            );
        }
    }
}

#[test]
fn the_rules_name_the_command_that_writes_them() {
    // The executable is `yaadgaar`; `yadgar` on PATH is the Python client
    // this one coexists with (ADR-0505), so naming it sends a reader to the
    // wrong program.
    assert!(RULES_BODY.contains("`yaadgaar install`"));
    assert!(!RULES_BODY.contains("`yadgar install`"));
}

#[test]
fn the_reference_goes_first_and_the_rest_survives_byte_for_byte() {
    let dir = crate::install::tests::scratch("rules-reference");
    let path = dir.join("CLAUDE.md");
    std::fs::write(&path, "# Mine\r\n\r\nprose with no trailing newline").unwrap();
    assert!(ensure_reference(&path, "@/opt/yadgar-rules.md").unwrap());
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "@/opt/yadgar-rules.md\n# Mine\r\n\r\nprose with no trailing newline"
    );
}

#[test]
fn a_second_install_does_not_rewrite_the_file() {
    // The failure this prevents: an install that appends its line again, or
    // touches a file it does not own on every run.
    let dir = crate::install::tests::scratch("rules-idempotent");
    let path = dir.join("CLAUDE.md");
    std::fs::write(&path, "existing\n").unwrap();
    assert!(ensure_reference(&path, "@/opt/rules.md").unwrap());
    assert!(!ensure_reference(&path, "@/opt/rules.md").unwrap());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "@/opt/rules.md\nexisting\n"
    );
}

/// A second install must not TOUCH `CLAUDE.md`, not merely leave it equal.
///
/// `#[cfg(unix)]` for the reason `refusals.rs` states the rule for: this
/// needs an inode, which Windows has no equivalent of. The inode is the
/// discriminator rather than the mtime because it cannot flake on a clock
/// or on a filesystem's timestamp granularity — `write_atomic_text` renames
/// a fresh temp file over the target, so a rewrite changes the inode while
/// leaving every byte the same.
///
/// That is exactly why `installing_twice_is_byte_identical` cannot see
/// this: it compares BYTES, and the property here is that a file yadgar
/// does not own is not written to at all. Deleting the
/// `if desired == existing { return Ok(false) }` early return left the
/// suite green.
#[cfg(unix)]
#[test]
fn a_second_install_does_not_touch_the_file_it_does_not_own() {
    use std::os::unix::fs::MetadataExt;
    let dir = crate::install::tests::scratch("rules-untouched");
    let path = dir.join("CLAUDE.md");
    std::fs::write(&path, "# my own instructions\n").unwrap();

    assert!(ensure_reference(&path, "@/opt/rules.md").unwrap());
    let first = std::fs::metadata(&path).unwrap().ino();

    assert!(!ensure_reference(&path, "@/opt/rules.md").unwrap());

    assert_eq!(
        std::fs::metadata(&path).unwrap().ino(),
        first,
        "a second install rewrote a file it does not own — the bytes are \
         the same, and the file is not"
    );
}

#[test]
fn a_file_left_holding_only_blank_lines_is_removed_too() {
    // The residue this module set out to remove, in its nearest neighbour
    // case. A `CLAUDE.md` created by install and holding the reference line
    // plus one blank line came out of an uninstall as a one-byte file
    // holding `"\n"` — the emptied file D76 says must not be left behind,
    // one newline short of the case that was fixed.
    //
    // Whitespace is nothing in the sense this decision needs: there is no
    // prose in it to lose. ONE WORD of somebody's writing and the file is
    // written back instead, which the second half asserts.
    let dir = crate::install::tests::scratch("rules-blank-residue");

    let residue = dir.join("CLAUDE.md");
    std::fs::write(&residue, "@/opt/rules.md\n\n").unwrap();
    assert!(remove_reference(&residue, "@/opt/rules.md").unwrap());
    assert!(
        !residue.exists(),
        "a file holding one newline was left behind: {:?}",
        std::fs::read_to_string(&residue)
    );

    let theirs = dir.join("theirs.md");
    std::fs::write(&theirs, "@/opt/rules.md\n\nmine\n").unwrap();
    assert!(remove_reference(&theirs, "@/opt/rules.md").unwrap());
    assert_eq!(
        std::fs::read_to_string(&theirs).unwrap(),
        "\nmine\n",
        "somebody's own prose was deleted with the reference line"
    );
}

/// Pins `read` itself, not the pre-flight check that also catches this.
///
/// `install` refuses an unreadable `CLAUDE.md` twice over: once in
/// `check_reference_target`, before anything is written, and once here.
/// The end-to-end test can only ever see the first of them, so removing
/// this one — going back to `unwrap_or_default()` — was invisible through
/// the public path. Two independent defenses need two independent tests,
/// or one of them is decoration.
#[cfg(unix)]
#[test]
fn an_unreadable_file_is_an_error_rather_than_an_empty_one() {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::install::tests::scratch("rules-unreadable");
    let path = dir.join("CLAUDE.md");
    let theirs = "# my own instructions\n";
    std::fs::write(&path, theirs).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
    if std::fs::read_to_string(&path).is_ok() {
        // Root, or a filesystem that ignores the mode: nothing to test.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }

    let outcome = ensure_reference(&path, "@/opt/yadgar-rules.md");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(outcome.is_err(), "an unreadable file was treated as empty");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        theirs,
        "the person's instructions were overwritten"
    );
}

#[test]
fn a_reference_further_down_is_left_where_it_is_not_duplicated() {
    // `verify` accepts the line anywhere, and so does install: moving it
    // to the top rewrote a file this client does not own, against where
    // the person (or nix) chose to put it.
    let dir = crate::install::tests::scratch("rules-move");
    let path = dir.join("CLAUDE.md");
    std::fs::write(&path, "first\n@/opt/rules.md\nlast\n").unwrap();
    assert!(!ensure_reference(&path, "@/opt/rules.md").unwrap());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "first\n@/opt/rules.md\nlast\n"
    );
}

#[test]
fn the_rules_name_the_server_key_and_its_tool_prefix() {
    // Two servers on one machine: `yadgar` (the memory engine) and
    // `yaadgaar` (this client). Rules that say only "Yadgar" leave the
    // model to guess which server's tools they mean.
    assert!(RULES_BODY.contains("`yaadgaar`"), "server key not named");
    assert!(
        RULES_BODY.contains("`mcp__yaadgaar__"),
        "tool prefix not named"
    );
}

#[test]
fn the_rules_name_all_three_task_systems() {
    // Claude Code's own todo tools, the memory engine's task tools, and
    // this client's — all live in one session on a machine that runs both
    // servers. The engine's tools REQUIRE `project`; this client's take
    // none. Rules that name only one system tell the model the wrong
    // thing about the other two.
    for name in [
        "`TaskCreate`",
        "`TaskList`",
        "`mcp__yadgar__task_*`",
        "`mcp__yaadgaar__*`",
    ] {
        assert!(RULES_BODY.contains(name), "the rules never name {name}");
    }
    let engine = RULES_BODY
        .split("`mcp__yadgar__task_*`")
        .nth(1)
        .expect("engine tools named");
    let engine_paragraph = engine.split("\n\n").next().unwrap();
    // `project_id` is the engine's required argument; `project` is only an
    // optional override there.
    assert!(
        engine_paragraph.contains("`project_id`"),
        "the rules do not say the engine's task tools require `project_id`: {engine_paragraph}"
    );
    assert!(
        !engine_paragraph.contains("require a `project` argument"),
        "the rules still say the engine requires `project`: {engine_paragraph}"
    );
}

#[test]
fn has_reference_matches_the_exact_line_only() {
    let dir = crate::install::tests::scratch("rules-exact");
    let path = dir.join("CLAUDE.md");
    std::fs::write(
        &path,
        "<!-- @/opt/rules.md -->\n# @/opt/rules.md\n@/opt/rules.md.bak\n",
    )
    .unwrap();
    assert!(!has_reference(&path, "@/opt/rules.md").unwrap());
    std::fs::write(&path, "x\n  @/opt/rules.md  \ny\n").unwrap();
    assert!(has_reference(&path, "@/opt/rules.md").unwrap());
}

#[test]
fn the_rules_say_to_ask_once_when_no_ledger_is_named_for_deferred_work() {
    // Ledger 1565: both ledgers tell the model to file deferred work, and these
    // rules say "not both" — so with both present and nothing naming one, the
    // model had no rule to pick by.
    assert!(
        RULES_BODY.replace('\n', " ").contains(
            "If both ledgers are present and CLAUDE.md does not say which to use for deferred work, ask the person once."
        ),
        "no ask-once rule for deferred work"
    );
}

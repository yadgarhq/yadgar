# Local targets only. CI does not run make; it runs the shared yadgarhq/actions
# workflows. Nothing here may be wired into CI.

.PHONY: canary canary-record

# The Claude Code task-format canary (ADR-1000). Runs a REAL `claude -p` on a
# throwaway task list and checks what it wrote against
# tests/fixtures/claude-code/task-shape-v1.json. Needs `claude` logged in, and
# spends a few cents per run. Run it on the operator's machine after every
# Claude Code upgrade, and before any car that writes the harness list ships.
canary:
	scripts/canary-claude-code-tasks

# The same, and also rewrite tests/fixtures/claude-code/hook-payloads.jsonl
# from the run. Commit the result only in a reviewed change.
canary-record:
	scripts/canary-claude-code-tasks --record

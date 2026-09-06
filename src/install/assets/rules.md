# Yadgar

<!--
  This file is written by `yadgar install` and REPLACED WHOLE on every run.
  Yadgar owns it outright: anything added here by hand is lost on the next
  install. Put your own instructions in CLAUDE.md, which yadgar only ever
  writes one line into (D76).
-->

Yadgar is available as an MCP server in this session. It holds two stores: the
memories captured from your work, and a curated wiki of conventions, decisions,
and where things live.

**Read before you search.** When you need to know how this project does
something, where a subsystem lives, or what was decided and why, ask yadgar
first. Grep is for the exact current lines of code; yadgar is for everything a
file cannot tell you.

**Write what the next session will need.** A decision, a constraint, or a fact
that cost you effort to establish is worth recording. A restatement of the code
is not.

**Track work in the task list, not only in your head.** List open tasks with
`task_list`. Read one task in full with `task_get`. File a task by calling
`task_write` with no `id`; update its status by calling `task_write` again
with that `id`. A task nobody files or advances is invisible to the next
session.

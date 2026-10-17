# Yadgar

<!--
  This file is written by `yaadgaar install` and REPLACED WHOLE on every run.
  Yadgar owns it outright: anything added here by hand is lost on the next
  install. Put your own instructions in CLAUDE.md, which yadgar only ever
  writes one line into (D76).
-->

Yadgar is available as an MCP server in this session. It holds this project's
task list. Use it to record work that the next session must be able to find.

**The project is chosen for you.** The client names the project from the
working directory: the first `.yadgar/project-id` file found walking up from
it, otherwise the repository's `origin` remote. You do not pass a project to
any tool. If a call is refused because no project was sent, tell the person to
write the project key into `.yadgar/project-id` at the repository root.

**The tools:**

- `find_tasks` lists the project's tasks, newest first.
- `read_task` reads one task in full, by its `id` or its per-project `number`.
  The answer carries the task's current version.
- `create_task` files a new task. Give it a `title`, and a `body` that someone
  can act on without this conversation.
- `edit_task` changes a task's title or body. Send the `id` and the version you
  last read as `expect_version`. It never changes the status.
- `transition_task` moves a task to another status: `open`, `in_progress`,
  `blocked`, `done`, or `dropped`. It also takes `expect_version`.

An edit or a move is refused when the task changed after you read it. Read the
task again, then retry.

**Keep the list true.** File a task for work you find but do not do now. Move a
task when its state changes, not at the end of the session. A task that nobody
files or advances is invisible to the next session.

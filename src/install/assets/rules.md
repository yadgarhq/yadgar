# Yaadgaar task ledger

<!--
  This file is written by `yaadgaar install` and REPLACED WHOLE on every run.
  yaadgaar owns it outright: anything added here by hand is lost on the next
  install. Put your own instructions in CLAUDE.md, which yaadgaar only ever
  writes one line into (D76).
-->

The `yaadgaar` MCP server is available in this session. Its tools appear as
`mcp__yaadgaar__*`. It holds this project's task list on the yadgarhq gateway.
Use it to record work that the next session must be able to find.

**Up to three task systems can be live in one session. They are separate.**

- `mcp__yaadgaar__*` (this server): this project's shared task ledger on the
  yadgarhq gateway. Use it for work that must outlive this session. The rest
  of this file is about these tools only.
- `TaskCreate`, `TaskList` and the other Claude Code task tools: a checklist
  for this session only. Use them to track the steps of the work in hand. They
  are not kept after the session ends.
- `mcp__yadgar__task_*`, if present: a different product, the yadgar memory
  engine, with its own task list and its own rules. Its tools require a
  `project_id` argument; `project` there is only an optional override. Follow
  that server's own instructions for its tools. Nothing in this file applies to
  them.

Do not file the same task in both ledgers. Do not copy tasks from one ledger to
the other unless the person asks. If both ledgers are present and CLAUDE.md,
or a file it imports, does not say which to use for deferred work, ask the
person once. If you cannot ask (unattended run), follow whichever loaded
instructions name a ledger.

**For `mcp__yaadgaar__*` tools, the project is chosen for you.** The client
names the project once, from the directory the session was started in: the
first `.yadgar/project-id` file found walking up from it, otherwise the
repository's `origin` remote. You do not pass a project to these tools. The
project does not follow a `cd` during the session; to work on another project,
start a new session there. If a call is refused because no project was sent,
tell the person to write the project key into `.yadgar/project-id` at the
repository root.

**The tools.** The descriptions and argument schemas in the server's tool list
are the authority; read them. Today the tools are task tools, named here
without the `mcp__yaadgaar__` prefix:

- `find_tasks` lists the project's tasks, newest first, one page at a time.
  When the answer carries a `next_page_token`, pass it as `page_token` to get
  the next page.
- `read_task` reads one task in full, by its `id` or its per-project `number`.
  The answer carries the task's current version.
- `create_task` files a new task. Give it a `title`, and a `body` that someone
  can act on without this conversation.
- `edit_task` changes a task's title or body. Send the `id` and the version you
  last read as `expect_version`. It never changes the status.
- `transition_task` moves a task to another status: `open`, `in_progress`,
  `blocked`, `done`, or `dropped`. It also takes `expect_version`.

**When a call is refused, read the message.** If the task's version changed
after you read it, read the task again and retry once with the new version.
For any other refusal — a move the task's status does not allow, an edit with
neither a title nor a body, a status that does not exist — do not retry the
same call. Fix the request, or tell the person.

**Keep the list true.** File a task for work you find but do not do now. Move a
task when its state changes, not at the end of the session. A task that nobody
files or advances is invisible to the next session.

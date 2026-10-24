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

- `find_tasks` lists the project's tasks, oldest first (by `id`, a UUIDv7),
  one page at a time. To get only some statuses, pass `statuses`, for example
  `["in_progress", "open", "blocked"]`; omit it to get every status. Pass
  `fields: "summary"` to get only `id`, `number`, `title`, `status` and
  `version` for each task, with no body. Pass `fields: "index"` to get only
  `number`, `status` and `title`; a row from another project also carries
  `project_id`. Use `index` to scan a list, then read the one task you need
  for its `version`. When the answer carries a
  `next_page_token`, pass it as `page_token` to get the next page.
- `read_task` reads one task in full, by its `id` or its per-project `number`.
  The answer carries the task's current version.
- `create_task` files a new task. Give it a `title` of at most 128 code
  points, and a `body` that someone can act on without this conversation. To record the task's reference in
  another tracker, pass `legacy_ref`. Make it URN-shaped, for example
  `urn:yadgar:ledger:<project>:<id>`. The gateway stores it as given and
  returns it in `links`; it does not check the shape. To make a retry safe,
  pass your own `idempotency_key`. The same key with the same arguments
  returns the first call's task. The same key with different arguments is
  refused. Keys compare case-insensitively. A replay returns the first call's
  version, which can be stale: read the task again before you edit it.
- `edit_task` changes a task's title or body. Send the `id` (or the `number`)
  and the version you last read as `expect_version`. It never changes the status. A title has at
  most 128 code points; a longer title is refused. To detect a body that was
  cut off in transit, send `expect_body_sha256` with the new `body`: the
  SHA-256 of that body as 64 lowercase hex characters. The gateway refuses the
  edit when the digest does not match.
- `transition_task` moves a task to another status: `open`, `in_progress`,
  `blocked`, `done`, or `dropped`. It also takes `expect_version`.
- `read_task`, `edit_task` and `transition_task` each take either the `id` or
  the `number` of the task, never both. A `number` is the task's number in the
  caller's own project.

This section describes gateway version 0.12.27. A newer gateway can serve
more; its tool list is the authority.

**When a call is refused, read the message.** If the task's version changed
after you read it, read the task again and retry once with the new version.
For any other refusal — a move the task's status does not allow, an edit with
neither a title nor a body, a status that does not exist — do not retry the
same call. Fix the request, or tell the person.

**Keep the list true.** File a task for work you find but do not do now. Move a
task when its state changes, not at the end of the session. A task that nobody
files or advances is invisible to the next session.

# yadgar — the client

One binary. `yaadgaar login` obtains a credential, `yaadgaar serve` is the MCP
server your agent spawns, and everything else manages the machine it runs on
(D75, D76).

The command is `yaadgaar`, and the doubled vowels are not a typo. The Python
client this replaces already owns `yadgar` on PATH, so the transitional client
differs in both the PyPI project and the executable — the reasoning is written
over `BINARY_NAME` in `src/install/hooks.rs`.

Decisions are recorded in [`yadgarhq/docs`](https://github.com/yadgarhq/docs) —
D75 (the proxy that knows no tools), D76 (one binary, and a rules file that is
referenced rather than spliced), D72 (the credential), D16 (why the gateway is
the only thing that speaks MCP inward).

## What it is

A proxy, and deliberately nothing more. `tools/list` is forwarded to the gateway
and its answer returned verbatim; so is `tools/call`. **There is no per-tool code
in this repository at all** — not a match arm, not a registry, not a feature
flag.

That is the boundary rather than an economy. A list the client asserts is a list
the client is trusted to filter honestly; forwarding makes the gateway's answer
the only answer, and the gateway resolves identity before it replies. It is also
what keeps releases uncoupled: this binary lives on people's laptops and the
gateway does not, so a client that enumerated tools would make the tool set move
at the speed of the slowest machine.

When the gateway is unreachable at spawn, the last tool list is served from cache
and calls fail with a plain "gateway unreachable" — because an empty list is
indistinguishable from yadgar not being installed, and the agent would silently
lose its tasks with nothing to report.

## Commands

| Command                             | What it does                                                                                                                                                                                                                                                                                                                                    |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `yaadgaar login`                    | Asks for the gateway address and your credentials once and stores both together. It registers nothing — `install` owns every file the agent client reads.                                                                                                                                                                                       |
| `yaadgaar enrol`                    | Redeems an admin's enrolment token: sets your password, prints your username, and stores the gateway address and credential. Replaces `login` on the first machine.                                                                                                                                                                             |
| `yaadgaar whoami`                   | Prints who your saved credential names: your `user_id`, your account segment, and your private project prefix `local/<account_segment>/`. It never prints the credential.                                                                                                                                                                       |
| `yaadgaar serve`                    | The stdio MCP server the agent spawns. Not run by hand.                                                                                                                                                                                                                                                                                         |
| `yaadgaar install`                  | Registers the hooks, the rules reference and the MCP entry, in the agent client's **user-level** config — once per machine, never per repository.                                                                                                                                                                                               |
| `yaadgaar uninstall`                | Removes what `install` added, and nothing else.                                                                                                                                                                                                                                                                                                 |
| `yaadgaar verify`                   | Reports drift in the installed environment. Scheduled, not remembered — the daemon cannot see `~/.claude/settings.json`, so nothing server-side can ever notice.                                                                                                                                                                                |
| `yaadgaar hook <event>`             | What `settings.json` invokes. A hook is a dispatch, not a program.                                                                                                                                                                                                                                                                              |
| `yaadgaar admin issue-enrolment`    | Issues an enrolment token for an existing user (`--user-id`): a first enrolment or a password reset, sent with your own administrator credential. Writes the token only where you name, with `--out <new file>` (0600) or `--stdout`. `--revoke-existing` revokes the user's existing credentials on redemption (a reset after a lost machine). |
| `yaadgaar admin revoke-credentials` | Revokes every live credential a user holds, now (`--user-id yadgar:user:<uuidv7>`), sent with your own administrator credential. Prints the count revoked and the count of outstanding enrolments voided. It does not change the password or lock the account. You cannot revoke your own credentials, and it asks no confirmation.             |
| `yaadgaar admin list-credentials`   | Lists a user's live credentials (`--user-id`), one row each: id, created, expires, label. Follows every page. Never lists a token.                                                                                                                                                                                                              |
| `yaadgaar admin revoke-credential`  | Revokes one credential (`--user-id`, `--credential-id` from `list-credentials`), sent with your own administrator credential. You may revoke your own.                                                                                                                                                                                          |

`enrol` (first machine) or `login` (any other), then `install`, is the whole
setup, and `verify` is the one to schedule. The project key is read once, from
the directory the session was started in (D53) — see
[Getting started](#getting-started) — so one registration serves every
repository.

### Unattended provisioning

`login` and `enrol` are interactive by default: three prompts each, and
`rpassword` reads the password straight off the controlling terminal rather
than stdin. A machine with no terminal to answer those prompts — a VM image,
a container build, a config-management run — cannot use either command as
they stand. Both take flags that remove every prompt:

| Flag                  | Commands         | What it replaces                                                                                                        |
| --------------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `--gateway <addr>`    | `login`          | The "Gateway address" prompt.                                                                                           |
| `--username <name>`   | `login`          | The "Username" prompt.                                                                                                  |
| `--password-stdin`    | `login`, `enrol` | The concealed password prompt (`enrol`'s repeat prompt too — there is nothing to compare a single stdin value against). |
| `--token-file <path>` | `enrol`          | The positional token argument or the "paste the enrolment token" prompt.                                                |

A fully unattended login:

```bash
printf '%s\n' "$PASSWORD" | yaadgaar login --gateway https://gateway.yadgar.internal:18443 \
  --username someone --password-stdin
```

Add `--ca-file /path/to/ca.pem` when the deployment uses a private CA and this
machine never enrolled. The CA is trusted in addition to the system roots and
stored with the credential.

A fully unattended enrolment:

```bash
printf '%s\n' "$NEW_PASSWORD" | yaadgaar enrol --token-file /path/to/token --password-stdin
```

**`printf '%s\n'`, not `echo`.** Under `dash` (Debian/Ubuntu's `/bin/sh`) and
some other shells, a bare `echo` interprets backslash escapes in its
argument by default — a password containing `\n`, `\t`, or similar is
silently truncated or rewritten before it ever reaches this client.
`printf '%s\n'` never interprets its argument at all.

**`--password-stdin` requires `--gateway` and `--username` on `login`.**
Without them, the prompts those two flags remove would still read stdin
themselves — consuming the very lines meant for the password. `enrol`
similarly requires the token as the positional argument or via
`--token-file` when `--password-stdin` is given: the interactive "paste the
enrolment token" fallback also reads stdin, and `--password-stdin` has
already claimed it for the password.

**The password is read as exactly one line: the first, with its trailing
line ending stripped.** It is never `trim`med — a password can legitimately
start, end, or consist of whitespace — and an empty result is refused.

**`--password-stdin` refuses a terminal**, unlike `docker login`, which
accepts one. `rpassword` is used everywhere else in this client precisely so
a password is never echoed and never reaches scrollback; reading one off a
real terminal through this flag would type it in plain sight instead of
concealing it, which is the exact leak this flag exists to close. Redirect
input from a file or a pipe.

Nothing about the interactive path changes when none of these flags are
given.

### Resetting a password

An administrator issues a new enrolment token; the person redeems it and
chooses the new password. The administrator never learns it.

1. Find the user's id. A new user gets one from the gateway's
   `/admin/create-user`; the gateway has no lookup by username.
2. Issue the token with your own administrator credential:

   ```bash
   yaadgaar admin issue-enrolment --user-id "$USER_ID" --out reset.token --revoke-existing
   ```

   `--out` writes a new 0600 file and refuses an existing one. Leave out
   `--revoke-existing` to keep the user's other machines logged in.

3. Hand `reset.token` over out of band. The person runs:

   ```bash
   yaadgaar enrol --token-file reset.token
   ```

**Revocation happens at redemption, not at issue.** The user's old
credentials keep working until the person runs `enrol` with the token. The
token is single-use and expires in 24 hours.

### Revoking a leaked credential now

To kill a leaked token at once, without waiting for a reset, revoke every
credential the user holds:

```bash
yaadgaar admin revoke-credentials --user-id "$USER_ID"
```

The id must be the canonical `yadgar:user:<lowercase uuidv7>`; anything else
is refused before a request is made. The command prints the number of
credentials revoked. A count of 0 prints a warning: the user held nothing
live, or the revocation already happened. It logs the user out of every
machine, and they can log in again, because the password is unchanged and the
account is not locked. It asks no confirmation. The gateway refuses (403) a
non-administrator, the bootstrap token, and an administrator targeting
themselves, and answers 404 for an unknown user.

Revoke-all also voids the user's outstanding enrolment tokens (ADR-0972 (5)).
The command prints that count too, as `voided N outstanding enrolments`. A
gateway older than gateway#127 does not report it, and then the line is absent.

### Revoking one credential

To log out one machine and keep the others, list the user's credentials and
revoke one by its id:

```bash
yaadgaar admin list-credentials --user-id "$USER_ID"
yaadgaar admin revoke-credential --user-id "$USER_ID" --credential-id "$CREDENTIAL_ID"
```

`list-credentials` prints one tab-separated row per live credential on stdout:
the id, the creation time, the expiry, and the label the machine gave at login.
The expiry is `never` for a credential minted before expiry existed. A count
goes to stderr. An unknown user gives an empty list, not an error. A control
character in any field the gateway sends, as in its error text, prints
escaped, as `\u{1b}`, so it cannot drive the terminal.

`revoke-credential` prints `revoked credential <id>`. It refuses a receipt that
names another user or another credential. The gateway answers 404 `no such
credential for this user` for an unknown id and for another user's id alike.
Unlike `revoke-credentials`, an administrator may revoke their own credential
here (ADR-0972 (2)). Both commands need gateway#127 or later; an older gateway
answers 404 and the command says so.

### Finding your private project prefix

A private project path is `local/<account_segment>/…` (ADR-0954). Ask the
gateway for your segment rather than guessing it:

```bash
yaadgaar whoami
```

```text
user_id:         yadgar:user:<uuid>
account_segment: <uuid>
private prefix:  local/<uuid>/
expires_at:      2027-01-19T08:00:00Z
```

`expires_at` is the expiry the gateway gave at `login` or `enrol`, stored in
the config (ADR-0972 (3)). It is `unknown` for a config written before this
field, or from a gateway older than gateway#128.

It uses the saved login and calls the gateway's `GET /auth/whoami`
(ADR-0959, gateway 0.12.17 or later). It exits non-zero when there is no saved
login, when the gateway refuses the credential (401: it is invalid or revoked,
so run `yaadgaar login` again; after the stored expiry it names the expiry
instead), when the gateway cannot be reached, when the
gateway answers 404 (it is older than 0.12.17 and does not serve the route),
when the gateway names no account segment (the dev trusted-headers path), and
when the segment is not one path segment (it holds a `/`, whitespace or a
control character).

### Credential expiry

A credential from `login` or `enrol` expires (ADR-0972 (3); 90 days by
default). Both commands store the expiry the gateway answers, and `whoami`
shows it. The password does not expire, so the fix is always `yaadgaar login`.

- A 401 after the stored expiry names the expiry and says to run
  `yaadgaar login`. This holds in `serve`, `whoami` and every `admin`
  command. Before the expiry, or with no stored expiry, the gateway's own
  reason is shown as before.
- A credential minted before expiry existed has none and keeps working.
- `enrol` can answer that the password is set but no credential was issued,
  because the account changed during the redemption (HTTP 409, ADR-0977). The
  password is then set and nothing is stored: run `yaadgaar login`.

## Installing

```bash
pipx install yaadgaar
```

`pip install yaadgaar` installs the same wheel, which holds only the binary.
To move to a newer release, `pipx install --force yaadgaar==<version>`.

## Getting started

1. **Enrol**, on the first machine, with the enrolment token an admin gave you.
   `enrol` sets your password, prints your username, and stores the gateway
   address, the credential and, on a private deployment, the CA to trust — the
   same config `login` writes. No `login` is needed afterwards.

   ```bash
   yaadgaar enrol --token-file /path/to/token
   ```

   Unattended, with the new password on stdin (see [Unattended provisioning](#unattended-provisioning)):

   ```bash
   printf '%s\n' "$NEW_PASSWORD" | yaadgaar enrol --token-file /path/to/token --password-stdin
   ```

   On any further machine, `yaadgaar login` with that username and password.
   For a deployment with a private CA, pass the CA's PEM file with
   `--ca-file /path/to/ca.pem`; `login` stores it with the credential, as
   `enrol` stores the CA its token carries.

2. **Install**, once per machine. This registers yadgar with Claude Code at
   user level: one hook in `~/.claude/settings.json` (the `PreToolUse` guard,
   `yaadgaar hook pre-tool-guard`; an install over 0.1.4 or older removes the
   eleven inert hook entries those versions wrote, and never touches another
   tool's hooks), the MCP entry in
   `~/.claude.json` under the key `yaadgaar` (tools appear as `mcp__yaadgaar__*`;
   an existing entry under that key that this client did not write makes install
   refuse, and the separate `yadgar` key is never touched), and the rules file
   `~/.claude/yadgar-rules.md`, referenced from `~/.claude/CLAUDE.md` by the
   line `@<home>/.claude/yadgar-rules.md` (absolute path, matched exactly). If
   that line is already anywhere in `CLAUDE.md`, install leaves the file alone;
   otherwise it adds the line at the top. If `CLAUDE.md` is a symlink (for
   example nix-managed), install never edits it: when the line is missing it
   prints the exact line to add to whatever generates the file, and finishes
   the rest of the install. If `claude` is not on PATH it says so and registers
   anyway.

   ```bash
   yaadgaar install
   ```

3. **Name the project.** Each call carries a project key, read once when the
   agent starts `yaadgaar serve`, from the directory the session starts in: the first `.yadgar/project-id` file
   found walking up from it, otherwise the git `origin` remote reduced to
   `owner/repo`. With neither, no key is sent and the gateway refuses calls
   that need one; the client adds a reason to that refusal that names
   `.yadgar/project-id`. A deployment whose gateway runs project validation in
   enforcing mode also refuses a key that is not registered or that does not
   resolve, and this client has no command that registers one. The client does
   not check the key itself. It sends the key it found, and the gateway decides.
   The refusal reaches the agent as a tool error that holds the gateway's own
   message: `yadgar gateway answered <status>: <message>`. The client adds
   nothing to that message when it did send a key. Ask the deployment's admin
   to register the key, or write the registered key into `.yadgar/project-id`.

   ```bash
   mkdir -p .yadgar && printf '%s\n' 'owner/repo' > .yadgar/project-id
   ```

4. **Verify**, now and on a schedule. It exits non-zero when the installed
   environment has drifted from what `install` left.

   ```bash
   yaadgaar verify
   ```

The config lives in `~/.config/yaadgaar/` (the OS config directory on macOS
and Windows; `YAADGAAR_CONFIG_DIR` overrides it). On Linux and macOS,
`config.json` and its directory are owner-only; on Windows the client sets no
permissions and relies on the profile directory's own.

Up to 0.1.4 the config lived in `~/.config/yadgar/`, which is also where the
yadgar memory engine keeps its `config.yaml` and `secrets.env`. This client no
longer reads, writes or moves anything there. If it finds its old
`config.json` or `tools-cache.json` there and no new config, `install`,
`verify` and `serve` print a hint: run `yaadgaar login` (or `yaadgaar enrol`)
again. A new login mints a new install id.

The guard reads `disabled_guards` from `~/.claude/yadgar-hook-exceptions.json`,
the file the memory engine's own guard reads, and never writes it. It honours
three ids, spelled as the engine spells them: `terraform_family` (terraform,
tofu, tfp and digger comments), `git_commit_flags` (commits that skip hooks or
signing) and `hook_config_tamper` (writes to that file, and agent writes to
Claude Code's `settings.json` / `settings.local.json`, to plugin
`hooks/hooks.json` files, to `.git/hooks` and submodule hooks, and lines that
shadow `gh` or write into a PATH directory before it).
Every other id is ignored. A missing, unreadable or malformed file leaves
every guard on.

The guard judges the `Bash`, `PowerShell`, `Edit`, `Write`, `MultiEdit` and
`NotebookEdit` tools (ADR-1005). It reads terraform through wrappers such as
`timeout` and `xargs`, and refuses nesting deeper than it reads. It refuses
a line that shadows `gh` — a function or alias, a PATH assignment, a file
named `gh`, or a write into a PATH directory that comes before the real `gh`
when the hook runs (ADR-1020). It judges
tool calls only: `yaadgaar install`, run by nix or a person, still writes
`settings.json`.

## Packaging

Rust, shipped to PyPI as wheels via maturin. The language follows the rest of the
system; the channel follows the people already using it, and a `pip`-installable command
is not a thing a rewrite gets to take away. A plain GitHub-release binary
was rejected for exactly that reason.

The wheel carries the compiled binary and no Python at all — `bindings = "bin"`
in `pyproject.toml` — so `import yaadgaar` is not a thing and is not meant to be.

A wheel is per-platform, so there are **six**: Linux, macOS and Windows, each on
x86_64 and aarch64. Every one is built on a runner of its own architecture rather
than cross-compiled, and `release.yaml` refuses to publish unless all six are
present — shipping the four that built would leave two platforms silently
installing an older version.

## Development

```bash
cargo build
cargo test
pre-commit run --all-files
```

There is no `Containerfile` and no chart here: this repository is not a service,
and what it ships is a wheel. CI is the shared workflow in
[`yadgarhq/actions`](https://github.com/yadgarhq/actions), which detects that and
skips the image, chart and proto stages (D62).

To build a wheel locally:

```bash
maturin build --release   # → target/wheels/, for THIS machine only
```

The other five targets are built by `.github/workflows/release.yaml`, which is
this repository's one piece of local CI. The reason it is not in
[`yadgarhq/actions`](https://github.com/yadgarhq/actions) with everything else is
written at the top of that file.

### The Claude Code task-format canary

The client will mirror yaadgaar tasks into Claude Code's own task list
(ADR-1000, `plans/harness-task-sync.md` in `yadgarhq/docs`). Claude Code does
not document that list's file format, so the format is recorded in
`tests/fixtures/claude-code/task-shape-v1.json`, and nothing writes to a list
that does not match it (`yaadgaar harness check-shape <dir>` runs the check).

`make canary` checks the recording against the real thing. It runs `claude -p`
on a throwaway list (`CLAUDE_CODE_TASK_LIST_ID=yaadgaar-canary-<random>`) with
capture hooks from a temporary `--settings` file, then checks the files it
wrote, the `TaskUpdate` and `TaskCompleted` hook payloads, and the order those
hooks fire in. It needs `claude` logged in and costs a few cents, so it runs on
the operator's machine and never in CI. Run it after every Claude Code upgrade.
A red canary means the format moved: update the fixture and the check together,
in one reviewed change. `make canary-record` also rewrites
`tests/fixtures/claude-code/hook-payloads.jsonl` from the run.

Which tasks are mirrored is `harness.scope`, set in `harness.json` beside
`config.json` (`{"scope": "active"}`): `all-open` (every task not done or
dropped, the default per ADR-1000), `active` (in_progress and blocked), or
`none`. The file may also hold `completion_ttl_secs` (below). Any other key, a
typo included, is refused and named in the banner, never read as the default. The
task tools count as present when `CLAUDE_CODE_ENABLE_TODO_TOOLS` is exactly `1`
(the documented value), or the model has them by default.

In a Claude Code session whose list passes the probe, `session-start` writes
each task in scope into the session's task list, keyed by its URN (never by its
yaadgaar number), updates the ones yaadgaar changed, and deletes the ones that
went done, dropped or out of scope. It never touches a task file it did not
write. What it owns is recorded outside the Claude Code tree, in
`<state>/harness/claude-code/<list id>.json`, where `<state>` is
`$YAADGAAR_STATE_DIR`, else `$XDG_STATE_HOME/yaadgaar`, else
`~/.local/state/yaadgaar` (the directory is `0700`, the files `0600`). The same
`<state>` holds the per-project registry record the memory engine reads
(`<state>/registry/<key>.json`), so one variable moves both. The banner
gains one line with the counts, or the reason the list was not synced.

An agent's edit to a mirrored task is written back to yaadgaar before it lands:
`yaadgaar hook task-update` (`PreToolUse`, matcher `TaskUpdate`) sends a subject
edit as `edit_task` and a `pending`/`in_progress` change as `transition_task`,
with the version last seen, and refuses (exit 2) what yaadgaar does not take. A
completion is closed by `yaadgaar hook task-completed` (`TaskCompleted`), and
only when the same session's same prompt marked it through `task-update` within
`completion_ttl_secs` (default 600). A state file or `harness.json` that cannot
be read refuses an edit to a mirrored task, with the reason, rather than let it
bypass write-back.

`harness.scope = none` and the kill switch `YAADGAAR_HARNESS_SYNC=off` both stop
the sync. Neither leaves a stale mirror behind: the pass that sees either one
only DELETES the files it owns, and writes nothing new. A list it never wrote
to, or a session whose list fails the probe, is not touched at all.

The recorded format (Claude Code 2.1.296):

- `<CLAUDE_CONFIG_DIR or ~/.claude>/tasks/<CLAUDE_CODE_TASK_LIST_ID or session id>/`
  holds one `<N>.json` per task, a 0-byte `.lock`, and `.highwatermark`, a
  decimal integer written when a task is deleted. Ids are never reused.
- A task file holds `id` (the file name, as a string), `subject`,
  `description`, `status` (`pending`, `in_progress` or `completed`), `blocks`
  and `blockedBy` (arrays of ids), and optionally `activeForm`, `owner` and
  `metadata` (an object, stored as given).
- `TaskUpdate` with `status: "deleted"` removes the file.
- For one completing `TaskUpdate`, the hooks fire as `PreToolUse`, then
  `TaskCompleted`, then `PostToolUse`. `PreToolUse` and `TaskCompleted` share
  the same `prompt_id`; `TaskCompleted` carries no `tool_use_id`.

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

| Command                          | What it does                                                                                                                                                                                                                          |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `yaadgaar login`                 | Asks for the gateway address and your credentials once and stores both together. It registers nothing — `install` owns every file the agent client reads.                                                                             |
| `yaadgaar enrol`                 | Redeems an admin's enrolment token: sets your password, prints your username, and stores the gateway address and credential. Replaces `login` on the first machine.                                                                   |
| `yaadgaar serve`                 | The stdio MCP server the agent spawns. Not run by hand.                                                                                                                                                                               |
| `yaadgaar install`               | Registers the hooks, the rules reference and the MCP entry, in the agent client's **user-level** config — once per machine, never per repository.                                                                                     |
| `yaadgaar uninstall`             | Removes what `install` added, and nothing else.                                                                                                                                                                                       |
| `yaadgaar verify`                | Reports drift in the installed environment. Scheduled, not remembered — the daemon cannot see `~/.claude/settings.json`, so nothing server-side can ever notice.                                                                      |
| `yaadgaar hook <event>`          | What `settings.json` invokes. A hook is a dispatch, not a program.                                                                                                                                                                    |
| `yaadgaar admin issue-enrolment` | Issues an enrolment token for an existing user (`--user-id`): a first enrolment or a password reset, sent with your own administrator credential. Writes the token only where you name, with `--out <new file>` (0600) or `--stdout`. |

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

## Installing

```bash
pipx install yaadgaar
```

Measured with `pipx install yaadgaar==0.1.2` on a fresh Debian VM. `pip install
yaadgaar` installs the same wheel, which holds only the binary. To move to a
newer release, `pipx install --force yaadgaar==<version>`.

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
   `.yadgar/project-id`. A deployment that enforces project registration also
   refuses a key it does not know, and this client has no command that
   registers one.

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
signing) and `hook_config_tamper` (writes to that file). Every other id is
ignored. A missing, unreadable or malformed file leaves every guard on.

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

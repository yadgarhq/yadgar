# yadgar — the client

One binary. `yadgar login` obtains a credential, `yadgar serve` is the MCP server
your agent spawns, and everything else manages the machine it runs on (D75, D76).

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
lose memory and tasks with nothing to report.

## Commands

| Command               | What it does                                                                                                                                                                                     |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `yadgar login`        | Asks for the gateway address and your credentials once, stores both together, and registers the MCP server in the agent client's **user-level** config — once per machine, never per repository. |
| `yadgar serve`        | The stdio MCP server the agent spawns. Not run by hand.                                                                                                                                          |
| `yadgar install`      | Hooks, the rules reference, the MCP entry, and a health check.                                                                                                                                   |
| `yadgar uninstall`    | Removes what `install` added, and nothing else.                                                                                                                                                  |
| `yadgar verify`       | Reports drift in the installed environment. Scheduled, not remembered — the daemon cannot see `~/.claude/settings.json`, so nothing server-side can ever notice.                                 |
| `yadgar hook <event>` | What `settings.json` invokes. A hook is a dispatch, not a program.                                                                                                                               |

`login` is the whole setup. `project_id` is derived from the working directory at
call time (D53), so one registration serves every repository and a fresh checkout
works immediately.

## Installing

```bash
pipx install yadgar
yadgar login
```

Rust, shipped to PyPI as wheels via maturin. The language follows the rest of the
system; the channel follows the people already using it, and `pipx install
yadgar` is not a thing a rewrite gets to take away. A plain GitHub-release binary
was rejected for exactly that reason.

The wheel carries the compiled binary and no Python at all — `bindings = "bin"`
in `pyproject.toml` — so `import yadgar` is not a thing and is not meant to be.

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

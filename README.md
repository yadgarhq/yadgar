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
lose memory and tasks with nothing to report.

## Commands

| Command                 | What it does                                                                                                                                                     |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `yaadgaar login`        | Asks for the gateway address and your credentials once and stores both together. It registers nothing — `install` owns every file the agent client reads.        |
| `yaadgaar serve`        | The stdio MCP server the agent spawns. Not run by hand.                                                                                                          |
| `yaadgaar install`      | Registers the hooks, the rules reference and the MCP entry, in the agent client's **user-level** config — once per machine, never per repository.                |
| `yaadgaar uninstall`    | Removes what `install` added, and nothing else.                                                                                                                  |
| `yaadgaar verify`       | Reports drift in the installed environment. Scheduled, not remembered — the daemon cannot see `~/.claude/settings.json`, so nothing server-side can ever notice. |
| `yaadgaar hook <event>` | What `settings.json` invokes. A hook is a dispatch, not a program.                                                                                               |

`login` then `install` is the whole setup, and `verify` is the one to schedule.
`project_id` is derived from the working directory at call time (D53), so one
registration serves every repository and a fresh checkout works immediately.

## Installing

```bash
pip install --no-cache-dir yaadgaar
yaadgaar login
yaadgaar install
```

To upgrade, the same line with `--upgrade`:

```bash
pip install --no-cache-dir --upgrade yaadgaar
```

**`--no-cache-dir` is not optional while the only releases are prereleases, and
it is the whole of the problem.** pip caches the simple-index page it fetched for
a project. A machine that has resolved `yaadgaar` once keeps serving itself that
page and cannot see anything published since — so an upgrade reports success
against a version that is already stale, rather than failing.

Measured on a fresh Debian VM with `0.1.0a2` already on PyPI. The VM's own `curl`
of the simple index showed all six `a2` wheels at the same moment pip insisted
only `a1` existed:

| Command                                          | What it did                                               |
| ------------------------------------------------ | --------------------------------------------------------- |
| `pipx upgrade yaadgaar`                          | "already at latest version 0.1.0a1"                       |
| `pipx install --force --pip-args=--pre yaadgaar` | installed `a1` again                                      |
| `pipx install --force yaadgaar==0.1.0a2`         | "Could not find a version that satisfies the requirement" |
| `pip install --no-cache-dir yaadgaar==0.1.0a2`   | worked, first try                                         |

Every row is the same cache. The third one is worth reading twice: an EXACT
version that PyPI was serving at that moment came back as no such version, which
is what a stale index page looks like from the inside.

**No `--pre` and no version pin are needed**, and this is measured rather than
reasoned — a bare `pip download --no-deps --no-cache-dir yaadgaar` against a
fresh index fetches `0.1.0a2`, and `pip install --no-cache-dir --upgrade
--dry-run yaadgaar` resolves to `Would install yaadgaar-0.1.0a2`. PEP 440 falls
back to prereleases when a requirement matches nothing else, so pinning would
only fix this README to a version that goes stale on the next release. `--pre`
widens a requirement and does not refetch anything, which is why it changed
nothing on the VM.

One command does NOT take that fallback: `pip index versions yaadgaar` reports
"No matching distribution found" while `pip index versions --pre yaadgaar` lists
both. It is a diagnostic, not an install, and it is the one place `--pre` earns
its keep here.

Once a stable release exists, all of this goes away and `pipx install yaadgaar` /
`pipx upgrade yaadgaar` are the lines. Until then pipx's own upgrade path is the
one measured failing above; the pipx equivalent,
`pipx install --force --pip-args="--no-cache-dir" yaadgaar`, is **untested** — the
`pip` lines are the ones that were actually run.

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

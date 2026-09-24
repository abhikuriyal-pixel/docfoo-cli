# DocFoo CLI

Barebones, scriptable DocFoo for the shell and for agents (Hermes/Slack).

> **Status:** Stage 1.2 — skeleton + bundled Pi sidecar. `version`, `model` and
> `auth` work; every other command reports the stage that will land. See
> `PLAN.md` for the full architecture and stage breakdown.

## Build

```bash
cargo build
./target/debug/docfoo version
```

The model sidecar (`docfoo-agent`) is a Bun-compiled completion service built
from `sidecar/`:

```bash
cd sidecar
bun install
./build.sh          # build.ps1 on Windows
```

The CLI finds it next to the `docfoo` binary. For development you can instead
point at the TypeScript source:

```bash
export DOCFOO_SIDECAR_BIN=/path/to/sidecar/docfoo-agent   # explicit binary
export DOCFOO_SIDECAR_TS=/path/to/sidecar/main.ts         # run with bun
```

## Usage (current)

```bash
docfoo version [--verbose] [--json]
docfoo model --list [--provider PROVIDER] [--json]
docfoo model --get [SLOT]                 # SLOT: chat | scan | scan-analysis | kg
docfoo model --set SLOT provider/model
docfoo auth --status [--provider PROVIDER] [--json]
docfoo auth --set PROVIDER [--key KEY]    # prompts on a TTY when --key is omitted
docfoo auth --logout PROVIDER
```

Global flags: `--workspace DIR`, `--json`, `--format markdown|slack|json`,
`--quiet`, `--verbose`, `--no-color`.

The workspace resolves in this order:

1. `--workspace DIR`
2. `$DOCFOO_WORKSPACE`
3. `~/.docfoo` (`%USERPROFILE%\.docfoo` on Windows)

`DOCFOO_AGENT_DIR` and `DOCFOO_MODELS_DIR` override the derived agent and native
models directories. The layout matches the desktop app's `db/` folder, so you
can point `--workspace` at an existing DocFoo library. Model slots and provider
credentials are shared with the app (`model-selection.json`, Pi `auth.json`).

## Output contract

Every command emits a `docfoo.cli/1` JSON envelope with `--json`:

```json
{
  "ok": true,
  "schema": "docfoo.cli/1",
  "command": "version",
  "workspace": "/home/abhishek/.docfoo",
  "data": { "cliVersion": "0.1.0" }
}
```

Errors use the same envelope with `"ok": false` and an `error` object carrying a
stable `code` (`usage`, `not_found`, `not_implemented`, `io`, `json`, `error`).
Human mode writes errors to stderr. Exit codes: `0` success, `1` runtime error,
`2` usage error.

## Sidecar protocol

One JSON object per line on stdin/stdout. Requests: `complete`, `cancel`,
`models`, `auth_status`, `auth_set`, `auth_logout`, `ping`. Responses are tagged
with the request id (`complete_response`, `stream_delta`, `models_response`,
`auth_status_response`, `auth_set_response`, `auth_logout_response`, `pong`).
See `PLAN.md` §3.4 for the full shapes.

## Roadmap

See `PLAN.md`. Stage 1.3 adds `kg --index/--query/--status` with rich JSON and
Slack output; Stage 2 adds read-only resources and notes.

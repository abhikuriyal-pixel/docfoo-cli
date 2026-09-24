# DocFoo CLI

Barebones, scriptable DocFoo for the shell and for agents (Hermes/Slack).

> **Status:** Stage 1.1 — skeleton. The command surface and JSON output envelope
> are in place; `version` works, and every other command reports the stage that
> will land. See `PLAN.md` for the full architecture and stage breakdown.

## Build

```bash
cargo build
./target/debug/docfoo version
```

## Usage (current)

```bash
docfoo version [--verbose] [--json]
docfoo help
```

Global flags: `--workspace DIR`, `--json`, `--format markdown|slack|json`,
`--quiet`, `--verbose`, `--no-color`.

The workspace resolves in this order:

1. `--workspace DIR`
2. `$DOCFOO_WORKSPACE`
3. `~/.docfoo` (`%USERPROFILE%\.docfoo` on Windows)

`DOCFOO_AGENT_DIR` and `DOCFOO_MODELS_DIR` override the derived agent and native
models directories. The layout matches the desktop app's `db/` folder, so you
can point `--workspace` at an existing DocFoo library.

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

## Roadmap

See `PLAN.md`. Stage 1.2 adds the bundled Pi sidecar plus `model` and `auth`;
Stage 1.3 adds `kg --index/--query/--status` with rich JSON and Slack output.

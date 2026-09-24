# DocFoo CLI

Barebones, scriptable DocFoo for the shell and for agents (Hermes/Slack).

> **Status:** Stages 1–2 complete — skeleton, bundled Pi sidecar, `model`/`auth`,
> `kg --index/--query/--status` with rich JSON and Slack output, and read-only
> `resources`/`notes`. Stages 3–6 (backup/collections, scan/setup, packaging)
> are pending. See `PLAN.md` for the architecture and stage breakdown.

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

### Knowledge graph

```bash
# Build or refresh the whole-library graph (progress on stderr)
docfoo kg --index

# Scope a build to one resources folder
docfoo kg --index --scope papers/ml

# Show built graphs and counts
docfoo kg --status [--scope DIR] [--json]

# Ask a question: one-shot final answer, no agent loop
docfoo kg --query "How are the National Education Policy 2020 and the NCF connected?"

# Machine-readable envelope (answer + citations + figures + sources)
docfoo kg --query "What is axial cross-attention?" --scope single_column_test --json

# Slack-ready output: figures become MEDIA: lines, Sources section appended,
# optional [[hermes:final]] sentinel for a patched Hermes to relay verbatim
docfoo kg --query "..." --format slack --hermes-final [--plain-tables] [--quote-sources]

# Persist the turn to an app-compatible kg-chats/ entry
docfoo kg --query "..." --save
```

Query flags: `--scope DIR`, `--model provider/model`, `--reasoning off|minimal|…|max`,
`--stream` (synthesis deltas to stderr), `--save`, `--hermes-final`,
`--plain-tables`, `--no-sources`, `--quote-sources`, `--max-chars N`.

### Resources and notes (read-only)

```bash
# One level of the library (folders summarized: files, md, figures, size)
docfoo resources --list [--rel DIR] [--figures] [--json]

# Full recursive tree with children and figure paths
docfoo resources --list --tree [--rel DIR] [--json]

# Read a line window (numbered by default, like the agent tools)
docfoo resources --read card/content.md [--offset N] [--limit N] [--plain] [--json]

# Only the ready-to-paste figure markdown lines
docfoo resources --read card/content.md --figures

# Headings with line numbers and per-section figure/table counts
docfoo resources --outline card/content.md [--json]

# Case-insensitive substring search with optional context
docfoo resources --search "query" [--rel DIR|FILE] [--context 0-5] [--limit N] [--json]

# Notes (read-only)
docfoo notes --list [--resource card/content.md] [--json]
docfoo notes --read NOTE_ID [--json]
```

`--read` output is line-numbered (`NNNNN | text`) so citations stay exact; the
JSON envelope also carries `text` (plain), `nextOffset`, and the window's
figures. Nothing in this stage writes to the workspace.

Global flags: `--workspace DIR`, `--json`, `--format markdown|slack|json`,
`--quiet`, `--verbose`, `--no-color`.

## Workspace

The workspace resolves in this order:

1. `--workspace DIR`
2. `$DOCFOO_WORKSPACE`
3. `~/.docfoo` (`%USERPROFILE%\.docfoo` on Windows)

`DOCFOO_AGENT_DIR` and `DOCFOO_MODELS_DIR` override the derived agent and native
models directories. The layout matches the desktop app's `db/` folder, so you
can point `--workspace` at an existing DocFoo library. Model slots, provider
credentials and KG settings are shared with the app (`model-selection.json`,
Pi `auth.json`, `kg-settings.json`).

## Output contract

Every command emits a `docfoo.cli/1` JSON envelope with `--json`:

```json
{
  "ok": true,
  "schema": "docfoo.cli/1",
  "command": "kg.query",
  "workspace": "/home/abhishek/.docfoo",
  "data": {
    "answer_markdown": "…[card/content.md:75-89]…",
    "citations": [{ "file": "card/content.md", "line_start": 75, "line_end": 89 }],
    "figures": [{ "path": "card/assets/f.png", "abs_path": "/…/f.png" }],
    "sources": [{ "doc": "card/content.md", "start_line": 75, "end_line": 89 }]
  }
}
```

Errors use the same envelope with `"ok": false` and an `error` object carrying a
stable `code` (`usage`, `not_found`, `not_implemented`, `io`, `json`, `error`).
Human mode writes errors to stderr. Exit codes: `0` success, `1` runtime error,
`2` usage error.

## Roadmap

See `PLAN.md`. Stage 3 adds local `backup`/`restore` and community
`collections --download`; Stage 4 `scan` plus `setup`; Stage 5 packaging,
`update`, and `docs/HERMES.md`.

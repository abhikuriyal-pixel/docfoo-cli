# DocFoo CLI

Barebones, scriptable DocFoo for the shell and for agents (Hermes/Slack).

> **Status:** All stages complete (1-6), 160 tests green, clippy clean. See
> `PLAN.md` for the architecture and `docs/HERMES.md` for the Hermes/Slack
> integration (plugin, `dfq` trigger routing, rich Slack rendering,
> persistent sidecar, custom providers).

## Build

```bash
cargo build                   # debug: target/debug/docfoo
cargo build --release         # release: target/release/docfoo
./scripts/acceptance.sh       # offline build + test + smoke checks
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

The visualizer frontend is dependency-free ES modules embedded into the
binary; its pure modules are tested with Node 22+:

```bash
node --test crates/docfoo-cli/tests/js/*.test.mjs
```

On Linux/WSL the sidecar can run as a persistent systemd user service
(`scripts/docfoo-sidecar.service`); the CLI then connects to
`<workspace>/.agent/sidecar.sock` instead of spawning a process per command.
Scan support provisions its native deps once with `docfoo setup`
(optionally `--from /path/to/DocFoo/models`).

## Usage (current)

```bash
docfoo version [--verbose] [--json]
docfoo model --list [--provider PROVIDER] [--refresh] [--json]
docfoo model --get [SLOT]                 # SLOT: chat | scan | scan-analysis | kg
docfoo model --set SLOT provider/model
docfoo auth --status [--provider PROVIDER] [--json]
docfoo auth --set PROVIDER [--key KEY]    # prompts on a TTY when --key is omitted
docfoo auth --logout PROVIDER
```

`model --refresh` re-fetches Pi's model catalogs over the network before
listing (useful once the sidecar daemon has been running for a while).

### Knowledge graph

```bash
# Build or refresh the whole-library graph (progress on stderr)
docfoo kg --index

# Scope a build to one resources folder
docfoo kg --index --scope papers/ml

# Show built graphs and counts
docfoo kg --status [--scope DIR] [--json]

# Open the live visualizer (loopback browser page; Ctrl-C stops the server)
docfoo kg --vis [--scope DIR] [--port N] [--no-open]

# Ask a question: one-shot final answer, no agent loop
docfoo kg --query "How are the National Education Policy 2020 and the NCF connected?"

# Machine-readable envelope (answer + citations + figures + sources)
docfoo kg --query "What is axial cross-attention?" --scope single_column_test --json

# Slack-ready output: `$…$` math -> Unicode, [doc.md:75-89] citations ->
# inline-code chips, figures -> MEDIA: lines; compact Sources unless
# --no-sources; --hermes-final adds the [[hermes:final]] sentinel
docfoo kg --query "..." --format slack --hermes-final [--no-sources] [--plain-tables]

# Persist the turn to an app-compatible kg-chats/ entry
docfoo kg --query "..." --save
```

Query flags: `--scope DIR`, `--model provider/model`, `--reasoning off|minimal|…|max`,
`--stream` (synthesis deltas to stderr), `--save`, `--hermes-final`,
`--plain-tables`, `--no-sources`, `--quote-sources`, `--max-chars N`.

`kg --vis` serves an embedded page on `127.0.0.1` (ephemeral port by default;
`--port N` pins one) and opens the default browser unless `--no-open` is given.
It reproduces the desktop app's knowledge-graph window: the same deterministic
canvas layout and query choreography (seed halos, traversal hop trails,
escalation banners, evidence strip), a chat composer that streams the answer
with citations, inline figures and GFM tables, a scope dropdown over every
built graph, a session-only model picker, a fixed reasoning cycle, and a
replay transport for the last turn. Queries are ephemeral — nothing is written
to `kg-chats/` unless you use `kg --query --save`.

### Resources and notes

```bash
# Serve the library as a read-only browser (cards with random figure covers,
# document reader with outline, highlight notes)
docfoo resources --vis [--rel DIR] [--port N] [--no-open]

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

# Notes (read-only from the CLI; the browser adds, edits and deletes them)
docfoo notes --list [--resource card/content.md] [--json]
docfoo notes --read NOTE_ID [--json]
```

`resources --vis` serves an embedded page on `127.0.0.1` (ephemeral port by
default; `--port N` pins one) and opens the default browser unless `--no-open`
is given. It reproduces the desktop app's resource browser and reader: cards
with a random figure as the cover (the die button re-rolls every cover, folders
collage up to four), breadcrumb drill-down, document tabs, an outline panel,
text/figure sliders (20px / 65%), and general Markdown through the same
marked + DOMPurify + KaTeX pipeline as the desktop reader (GFM tables,
centered figures, OCR math, escaped-tag and spaced-path repairs; code blocks
are plain monospace).
The page never changes resource files — no create, rename, delete, move or
edit. Selecting text offers **Note**: notes are written in the desktop app's
`notes.json` schema, so they round-trip with the desktop app, and can be
edited and deleted from the notes panel. Images open in a lightbox; other
files are listed as inert cards.

`--read` output is line-numbered (`NNNNN | text`) so citations stay exact; the
JSON envelope also carries `text` (plain), `nextOffset`, and the window's
figures. Nothing in this stage writes to the workspace.

### Backup, restore and collections

```bash
# Create a docfoo-backup v1 zip (app-compatible)
docfoo backup [--out FILE] [--json]

# Restore a backup (asks for confirmation unless --yes)
docfoo restore FILE [--yes] [--json]

# Community collections
docfoo collections --list [--json]
docfoo collections --info ID|NAME [--type resource|kg] [--json]
docfoo collections --download ID|NAME [--type resource|kg] [--name NAME] [--force] [--json]
```

The backup format is identical to the desktop app's, so backups move in both
directions. Restore stages the archive, snapshots the live data, swaps it in
atomically, and keeps a journal so an interrupted restore rolls back on the
next run.

### Scan and setup

```bash
# Provision scan's native dependencies (Linux downloads pinned, checksummed
# ONNX Runtime 1.28.0 + PDFium 151.0.7881.0; --from copies the layout model
# from a local DocFoo models/ folder and, on Windows, its DLLs)
docfoo setup [--check] [--from DIR] [--layout-model-url URL] [--force] [--json]

# OCR a PDF or image into resources/<destination>/<stem>/
docfoo scan FILE [--parallel N] [--text_model KEY] [--figure_model KEY]
                [--output DEST] [--pages 1,2,3] [--prompt TEXT]
                [--analysis-prompt TEXT] [--no-figures] [--json]
```

`--figure_model` defaults to `--text_model`; `--no-figures` disables figure and
table analysis (asset crops are still saved). The effective native paths honor
`DOCFOO_LAYOUT_MODEL`, `DOCFOO_ORT_DLL` and `DOCFOO_PDFIUM_DLL`, and
`DOCFOO_MODELS_DIR` overrides the models cache location.

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

## Install and update

```bash
# Linux/WSL: install the latest release into ~/.local/bin
./install.sh                 # or --local to build from this checkout

# Private repo: export a token first (the same is needed for `docfoo update`)
export GITHUB_TOKEN="$(gh auth token)"

# Windows: build from source and keep the pair together
cargo build --release
cd sidecar && ./build.ps1    # produces docfoo-agent.exe next to docfoo.exe

# Check for or install a newer release
docfoo update --check
docfoo update

# Shell completions
docfoo completions bash        # bash | zsh | fish | powershell | elvish
```

Release artifacts are `docfoo-cli-<version>-linux-x64.tar.gz` and
`docfoo-cli-<version>-windows-x64.zip` plus `.sha256` sidecars; `scripts/release.sh`
and `scripts/release.ps1` build them locally. Set `DOCFOO_REPO` to your GitHub
`owner/repo` before releasing.

## Providers and models

Model slots live in `<workspace>/model-selection.json` and are shared with the
desktop app. Credentials go through Pi (`<workspace>/.agent/auth.json`, or
`docfoo auth --set PROVIDER --key KEY`); custom OpenAI-compatible providers
can be declared in `<workspace>/.agent/models.json` with an environment key
(`"apiKey": "$MY_API_KEY"`). The sidecar also bundles Inception and InferX as
local provider modules compiled into `docfoo-agent` (no `models.json` entry
needed; their catalogs refresh from `/v1/models`). On Linux/WSL the systemd
sidecar reads key overrides
from `~/.config/systemd/user/docfoo-sidecar.env`. See `docs/HERMES.md`
("Custom providers") for a worked example.

## Hermes (Slack)

See [`docs/HERMES.md`](docs/HERMES.md) for the full guide. Install the
`docfoo_plugin` Hermes plugin once so a `--hermes-final` tool result becomes
the final Slack answer with no second model pass:

```bash
python3 hermes/install.py      # Linux/WSL
python hermes\install.py       # Windows
```

The plugin lives outside the Hermes repo (`~/.hermes/plugins/docfoo_plugin/`),
so `hermes update` never wipes it. See [`hermes/README.md`](hermes/README.md).

## Roadmap

All six plan stages are implemented. `scripts/acceptance.sh` runs the offline
acceptance checks (build, tests, fresh-workspace smoke tests, vis frontend
module tests when Node is present) and an optional live KG query when
`DOCFOO_ACCEPTANCE_WORKSPACE` and `DOCFOO_ACCEPTANCE_QUERY` are set.

Possible follow-ups (out of v1 scope): `docfoo ask` (multi-step agent mode),
Koofr cloud backup push/pull, community uploads, OAuth provider logins,
macOS/arm64 builds.

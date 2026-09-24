# DocFoo CLI — Implementation Plan

> **Status:** Stages 1–4 complete: skeleton, bundled Pi sidecar, `model`/`auth`,
> `kg --index/--query/--status`, read-only `resources`/`notes`, local
> `backup`/`restore`, community `collections --download`, and `scan` +
> `setup`; 120 tests green. Stages 5–6 pending. See §7 for the breakdown.
> **Companion project:** `C:\Users\abhik\Documents\Test\DocFoo` (the Tauri desktop app).
> **Primary consumer:** Hermes (Slack agent) running in WSL, which invokes `docfoo` commands
> through its `terminal` tool.

---

## 1. Goal

A barebones, scriptable version of DocFoo's core, usable from a shell and easy for Hermes to
drive:

- `docfoo kg --query "How is apples connected to mangoes?"` returns the **final, rich answer**
  (markdown, figures, tables, exact citations) — no second model pass.
- `docfoo scan <pdf|img> --parallel 4 --text_model X --figure_model Y` builds a resource card.
- Read-only access to resources and notes; local backup/restore; community collection downloads.
- Model selection and provider auth shared with the desktop app's workspace.
- Runs on Linux/WSL and Windows; installs with one script; self-updates.

Out of scope (v1): calendar, podcasts, Google Drive, subagents/skills, the Buddy chat UI, Ask
Buddy, KG chat history browsing, community uploads, Koofr cloud backup, OAuth provider logins.

---

## 2. Locked decisions

| # | Area | Decision |
|---|------|----------|
| 1 | Stack | **Rust CLI + bundled Pi sidecar.** The CLI links `docfoo-kg`/`docfoo-ocr`; a stripped Bun sidecar embeds Pi's `ModelRuntime` and answers model-completion requests over JSONL. |
| 2 | Pi | Sidecar is a **completion service only** (no agent loop, no sessions, no RPC mode). Bundled compiled binary; `DOCFOO_SIDECAR_TS` is a dev fallback that runs the TS source under Bun. |
| 3 | Workspace | **Configurable; default CLI-local.** Default `~/.docfoo` (Linux) / `%USERPROFILE%\.docfoo` (Windows); `--workspace` / `DOCFOO_WORKSPACE` can point at the desktop app's `db/`. Same on-disk layout as the app. |
| 4 | Ask mode | **None.** `docfoo kg --query` is inherently one-shot/final (retrieval + one synthesis call). No `docfoo ask`. |
| 5 | KG | **Index + query + status.** `docfoo kg --index`, `--query`, `--status`. |
| 6 | Writes | **Resources/notes are read-only** in v1. |
| 7 | Cloud | **Local backup/restore + community collection downloads.** No Koofr, no uploads. |
| 8 | Output | Default human markdown; `--json` envelope; `--format slack` (Slack-ready mrkdwn, `MEDIA:` figures, sources section). |
| 9 | Hermes | CLI-side only: Slack-ready payload + opt-in `[[hermes:final]]` sentinel. The ~10-line Hermes patch is documented in `docs/HERMES.md`, not implemented here. |
| 10 | Scan deps | **`docfoo setup` auto-provisions** Linux `libonnxruntime.so` + `libpdfium.so` + `PP-DocLayoutV3.onnx`, checksummed, cached in `~/.cache/docfoo/models`. |
| 11 | Platforms | **Linux/WSL x64 + Windows x64.** |
| 12 | Model/auth | Shared workspace: `model-selection.json` + Pi `auth.json`; `docfoo model --get/--set/--list`, `docfoo auth --status/--set/--logout`. |
| 13 | Install | `install.sh` + versioned release tarballs; `docfoo update [--check]`. |
| 14 | Code reuse | **Duplicate thin Tauri glue** in the CLI crate; heavy crates reused via path deps. |
| 15 | Repo coupling | `DocFoo_CLI` references `../DocFoo/crates/docfoo-kg` and `docfoo-ocr` via path; release builds vendor the crates into the tarball. |
| 16 | KG history | **Ephemeral** by default; `--save` writes an app-compatible `kg-chats/` entry. |

---

## 3. Architecture

### 3.1 Components

```
┌─ docfoo (Rust binary) ─────────────────────────────────────────────┐
│ clap command tree                                                   │
│ workspace + config (model-selection.json, kg-settings.json)         │
│ kg/scan/resources/notes/backup/collections/model/auth/setup/update  │
│ sidecar client (JSONL, pending registry, cancel, restart)           │
└───────────────┬──────────────────────────────┬─────────────────────┘
                │ path deps                     │ stdin/stdout JSONL
┌───────────────▼──────────────┐   ┌────────────▼─────────────────────┐
│ docfoo-kg   (reused crate)   │   │ docfoo-agent (Bun sidecar)       │
│ docfoo-ocr  (reused crate)   │   │ Pi ModelRuntime + completeSimple │
│  · build::run / query::retrieve │ │ + streamSimple, auth, catalogs  │
└──────────────────────────────┘   └──────────────────────────────────┘
```

- The CLI is a normal Rust binary (no Tauri, no GUI).
- All provider access goes through the sidecar, so provider keys, catalogs, `models.json`
  custom providers, and `auth.json` behave exactly like the desktop app.
- The sidecar is **not** a Pi agent: it never creates a session, never runs tools, and never
  touches `runRpcMode`. Startup is fast.

### 3.2 Runtime flow — `docfoo kg --query`

```
1. Resolve workspace (--workspace / DOCFOO_WORKSPACE / ~/.docfoo).
2. Load kg-settings.json (IndexSettings/QueryTunables, clamped) and graphs/<scope>/graph.json.
3. Spawn/reuse docfoo-agent (lazy; only if the command needs models).
4. Build AgentChatClient (implements docfoo_kg::llm::ChatClient) over the sidecar.
5. docfoo_kg::query::retrieve(query, &kg, &tunables, llm, decision, sink, cancel, on_delta)
   → streams synthesis deltas, expands [S1] → [doc:start-end].
6. Build the output envelope:
   · answer_markdown (verbatim)
   · citations[]   parsed from the answer with the same regex as src/lib/citations.ts
   · figures[]     parsed from ![](...), resolved to absolute paths under resources/
   · sources[]     from QueryOutcome.trace.evidence_sections (doc/start_line/end_line/…)
   · timings, model, scope, stale
7. Render: markdown (default) | JSON (--json) | Slack (--format slack, optional sentinel).
```

`docfoo kg --index` mirrors `src-tauri/src/kg/mod.rs::kg_index` (corpus walk → `build::run`),
with progress to stderr. `docfoo kg --status` lists built graphs (port of `kg_list_graphs`)
plus source freshness (later, port of `src-tauri/src/kg/source_status.rs`).

### 3.3 Workspace layout

Same shape as the app's `db/` so `--workspace` can point at the app's data:

```
~/.docfoo/                        # DOCFOO_WORKSPACE
├── resources/                    # read-only in v1
│   └── <card>/{content.md, assets/}
├── graphs/top-level.json         # + graphs/<scope>/graph.json
├── notes.json                    # read-only
├── notes-images/                 # read-only
├── model-selection.json          # CLI + app shared
├── kg-settings.json              # CLI + app shared
├── .agent/                       # DOCFOO_AGENT_DIR (default <workspace>/.agent)
│   ├── auth.json                 # Pi credentials
│   ├── models.json               # custom providers/overrides
│   ├── models-store.json         # cached catalogs
│   ├── current-kg-graph.json     # optional compatibility pointer
│   └── secrets.json              # fallback only
├── kg-chats/                     # only with --save
└── tmp/                          # downloads, staging
```

Native deps live outside the workspace in `~/.cache/docfoo/models/` (Linux) or
`%LOCALAPPDATA%\docfoo\models` (Windows); `DOCFOO_MODELS_DIR` overrides.
`DOCFOO_LAYOUT_MODEL` / `DOCFOO_ORT_DLL` / `DOCFOO_PDFIUM_DLL` overrides are honored as today.

### 3.4 Sidecar protocol

One JSON object per line, both directions. Request ids correlate responses.

**CLI → sidecar**

```json
{"type":"complete","requestId":"c1","model":"provider/model","messages":[…],
 "maxTokens":6000,"reasoning":"off","stream":true,"responseFormat":{…}}
{"type":"cancel","requestId":"c1"}
{"type":"models","requestId":"m1"}
{"type":"auth_status","requestId":"a1"}
{"type":"auth_set","requestId":"a2","provider":"openrouter","key":"…"}
{"type":"auth_logout","requestId":"a3","provider":"openrouter"}
{"type":"ping","requestId":"p1"}
```

**Sidecar → CLI**

```json
{"type":"ready","version":"0.1.0"}
{"type":"stream_delta","requestId":"c1","text":"…"}
{"type":"complete_response","requestId":"c1","success":true,"text":"…"}
{"type":"models_response","requestId":"m1","success":true,"providers":[…]}
{"type":"auth_status_response","requestId":"a1","success":true,"providers":[…]}
{"type":"auth_set_response","requestId":"a2","success":true}
{"type":"pong","requestId":"p1"}
{"type":"fatal","message":"…"}
```

Sidecar implementation notes:

- `ModelRuntime.create({ authPath, modelsPath, modelsStorePath, allowModelNetwork: true,
  refreshOnCreate: true })` with paths under the workspace `.agent/`.
- Reuse the completion logic from `DocFoo/agent/model-query.ts` (`buildContext`, model
  resolution, maxTokens clamp, reasoning, `responseFormat` only for
  `api === "openai-completions"`, stream/non-stream, abort). Copy it into `sidecar/complete.ts`
  and replace the Pi command/emit plumbing with the stdin loop.
- `auth_set` = `runtime.login(provider, "api_key", interaction)` where `interaction.prompt`
  returns the supplied key and `notify` is a no-op. `auth_logout` = `runtime.logout(provider)`.
  OAuth-only providers return a clear "use the desktop app for OAuth login" error.
- `models` = `runtime.getAvailable()` + `runtime.getProviders()` + `getProviderAuthStatus()`
  (port of `DocFoo/agent/providers.ts::list`).
- Pin `@earendil-works/pi-coding-agent@0.86.1` and `@earendil-works/pi-ai@0.86.1` (same as
  the app). Run the same `dedupe-pi-ai` step as `agent/build.bat`.
- Build: `bun build --compile ./main.ts --outfile docfoo-agent` with `--target=bun-linux-x64`
  and `--target=bun-windows-x64`.

---

## 4. Command surface

Global flags (available on every command): `--workspace DIR`, `--json`, `--format
markdown|slack|json`, `--quiet`, `--verbose`, `--no-color`.

| Command | Flags | Notes |
|---|---|---|
| `docfoo kg --query Q` | `--scope DIR`, `--model KEY`, `--reasoning off\|minimal\|…\|max`, `--save`, `--stream`, `--hermes-final`, `--plain-tables`, `--no-sources`, `--quote-sources`, `--max-chars N` | One-shot final answer. Defaults: scope `""` (top-level graph), model `kgModelKey`. |
| `docfoo kg --index` | `--scope DIR`, `--fresh`, `--model KEY`, `--reasoning LEVEL` | Writes `graphs/<scope>/graph.json`; progress on stderr. |
| `docfoo kg --status` | `--scope DIR` | Built graphs, active scope, doc/section counts, source staleness. |
| `docfoo scan <file>` | `--parallel N` (4), `--text_model KEY`, `--figure_model KEY`, `--output DEST`, `--pages 1,2,3`, `--prompt TEXT`, `--analysis-prompt TEXT`, `--no-figures` | `--figure_model` defaults to `--text_model`; `--no-figures` disables analysis. |
| `docfoo resources --list` | `--tree`, `--figures` | Default one level; `--tree` recursive. |
| `docfoo resources --read REL` | `--offset N`, `--limit N`, `--numbered` | Numbered lines make citations easy. |
| `docfoo resources --outline REL` | | Headings + line numbers + figure/table counts. |
| `docfoo resources --search Q` | `--limit N`, `--context N` | Returns file/line/text/image hits. |
| `docfoo notes --list` | `--resource REL` | |
| `docfoo notes --read ID` | | |
| `docfoo backup` | `--out FILE` | Default `docfoo-backup-<ts>.zip` in cwd. |
| `docfoo restore <FILE>` | `--yes` | Same zip format as the app (`docfoo-backup` v1). |
| `docfoo collections --list` | | Community index. |
| `docfoo collections --info ID` | | |
| `docfoo collections --download ID\|NAME` | `--type resource\|kg`, `--name NAME`, `--force` | Installs into `resources/` or `graphs/`. |
| `docfoo model --get [SLOT]` | SLOT = `chat\|scan\|scan-analysis\|kg` (default `kg`) | |
| `docfoo model --set SLOT KEY` | | Validates against the sidecar catalog before writing. |
| `docfoo model --list` | `--provider P` | Providers + models + auth status. |
| `docfoo auth --status` | `--provider P` | Never prints key values. |
| `docfoo auth --set PROVIDER` | `--key KEY` (prompt on TTY if omitted) | API keys only in v1. |
| `docfoo auth --logout PROVIDER` | | |
| `docfoo setup` | `--check`, `--from DIR`, `--force` | Provision native deps + layout model. |
| `docfoo version` | `--verbose` | CLI + sidecar versions, workspace path. |
| `docfoo update` | `--check` | `--check` prints availability; bare command self-updates when a release manifest exists. |
| `docfoo help [command]` | | |

Exit codes: `0` success, `1` runtime error, `2` usage error. Errors go to stderr; with
`--json` a machine-readable error envelope goes to stdout.

---

## 5. Output contract

### 5.1 JSON envelope (stable, versioned)

All commands emit the same top-level shape; `data` is command-specific:

```json
{
  "ok": true,
  "schema": "docfoo.cli/1",
  "command": "kg.query",
  "workspace": "/home/abhishek/.docfoo",
  "data": { }
}
```

`kg.query` data:

```json
{
  "query": "How is apples connected to mangoes?",
  "scope": "",
  "model": "openrouter/inception/mercury-2.5",
  "answer_markdown": "…[multi_column_1/content.md:77-81].\n\n![…](multi_column_1/assets/figure_2_p03_02.png)\n\n| A | B |\n|---|---|\n| 1 | 2 |",
  "citations": [
    {"file": "multi_column_1/content.md", "line_start": 77, "line_end": 81, "label": "multi_column_1:77-81"}
  ],
  "figures": [
    {"path": "multi_column_1/assets/figure_2_p03_02.png",
     "abs_path": "/home/abhishek/.docfoo/resources/multi_column_1/assets/figure_2_p03_02.png",
     "markdown": "![](multi_column_1/assets/figure_2_p03_02.png)"}
  ],
  "tables": ["| A | B |\n|---|---|\n| 1 | 2 |"],
  "sources": [
    {"doc": "multi_column_1/content.md", "start_line": 77, "end_line": 81,
     "section": "[SC1] Atomic formatting", "chars": 1234}
  ],
  "stale": null,
  "saved": false,
  "timings": {"totalSecs": 1.2}
}
```

Other `data` shapes:

- `kg.index`: `entities, relations, sections, topics, docs, extracted, skipped, elapsedSecs, graphPath`
- `kg.status`: `built: ["", "papers/ml"], active, graphPath, docs, sections`
- `resources.list`: recursive `ResourceEntry` objects (`name, rel, kind, size, files, modified, children, figures`)
- `resources.read`: `rel, startLine, endLine, totalLines, text, numberedText, figures[]`
- `resources.outline`: `rel, totalLines, sections:[{line, heading, figures, tables}]`
- `resources.search`: `hits:[{file, line, text, image?, markdown?}]`
- `notes.list`: `notes.json` values verbatim, filtered by resource
- `scan`: `outDir, rel, md, pages, figures, chars, partial, failedPage, cancelled`
- `backup`: `file, bytes, resources, sessions, notes, createdAt`
- `restore`: `file, resources, sessions, notes, restoredAt`
- `collections.list`: `items[]` from the community API
- `collections.download`: `id, type, name, rel, files, bytes`
- `model.get/set/list`, `auth.status/set/logout`, `setup`, `version`, `update`: small data objects

### 5.2 `--format slack`

1. **Tables**: GFM pipe tables are preserved (Hermes renders them as native Slack table blocks
   when `platforms.slack.extra.rich_blocks: true`). `--plain-tables` converts each table to
   labeled bullet lines for the default flat-mrkdwn path.
2. **Figures**: every `![alt](path)` is replaced by `MEDIA:/absolute/path` on its own line,
   resolved relative to `<workspace>/resources/`. Caption lines that follow are kept. Missing
   files leave the markdown untouched.
3. **Citations**: inline `[doc:line]` tokens are kept. A compact `Sources:` section is appended
   by default (`--no-sources` disables). `--quote-sources` adds the exact text lines under each
   source (read from the resource). `--max-chars N` bounds the whole output (default: no bound).
4. **Sentinel**: `--hermes-final` prepends `[[hermes:final]]` as the first line. The rest of
   stdout is the answer Hermes should post verbatim.

### 5.3 Streaming and progress

- `kg --query --stream`: synthesis deltas to **stderr**; final answer to **stdout**.
- `kg --index` / `scan`: human progress lines to stderr; `--quiet` suppresses; final result
  (or JSON envelope) to stdout.
- Ctrl-C sets the pipeline's cancel flag and aborts in-flight sidecar requests.

---

## 6. Repository layout

```
DocFoo_CLI/
├── Cargo.toml                     # workspace; members: crates/docfoo-cli
├── PLAN.md
├── README.md
├── install.sh
├── docs/
│   ├── HERMES.md                  # integration guide + sentinel patch
│   └── RELEASE.md
├── sidecar/
│   ├── package.json               # pi-ai / pi-coding-agent 0.86.1
│   ├── main.ts                    # stdin JSONL loop
│   ├── complete.ts                # adapted from DocFoo/agent/model-query.ts
│   ├── providers.ts               # auth status/set/logout + model list
│   ├── build.sh
│   └── build.ps1
├── crates/docfoo-cli/
│   ├── Cargo.toml                 # path deps: ../../../DocFoo/crates/docfoo-kg, docfoo-ocr
│   └── src/
│       ├── main.rs
│       ├── cli.rs                 # clap tree
│       ├── workspace.rs
│       ├── config.rs              # model-selection.json, kg-settings.json
│       ├── output.rs              # envelope + markdown/json/slack renderers
│       ├── sidecar.rs             # process + pending registry + cancel/restart
│       ├── util/                  # paths, atomic write, zip safety, http
│       ├── kg/                    # paths, settings, index.rs, query.rs, decision.rs
│       ├── scan/                  # completion client, destination, progress
│       ├── resources/             # tree, read, outline, search
│       ├── notes/                 # read-only notes
│       ├── backup/                # ported zip.rs
│       ├── collections/           # client + extract/install
│       └── commands/              # thin clap → feature mapping
└── tests/
    ├── fixtures/                  # tiny workspace, graph, resource cards, zips
    └── fake-sidecar/              # test binary with canned responses
```

Release tarball (per platform):

```
docfoo-0.1.0-linux-x64.tar.gz
├── docfoo            # Rust binary
├── docfoo-agent      # Bun-compiled sidecar
├── install.sh
└── README.md
```

---

## 7. Stages

Each stage is independently buildable, testable, and useful. Stages 2–5 do not depend on each
other after Stage 1.

### Stage 1 — Foundation + KG vertical slice (primary value)

**Status:** 🟢 done (1.1–1.3)

**Goal:** `docfoo kg --index` and `docfoo kg --query` produce rich, Hermes-ready answers in a
WSL workspace, using the bundled Pi sidecar for models.

#### 1.1 Skeleton

Deliverables:

- Cargo workspace + `docfoo-cli` crate; `clap` command tree for every command in §4 (stubs for
  commands not yet implemented, returning "not implemented yet").
- `workspace.rs`: resolve `--workspace` → `DOCFOO_WORKSPACE` → `~/.docfoo`; derive agent dir,
  models dir, resources, graphs, tmp; create dirs lazily.
- `config.rs`: read/write `model-selection.json` (`chatModelKey`, `scanModelKey`,
  `scanAnalysisModelKey`, `kgModelKey`; port validation from `src-tauri/src/config/mod.rs`).
- `output.rs`: envelope types, error envelope, markdown/JSON renderers, exit codes.
- `version`, `help`; global flags.
- `README.md` quick start.

Reference: `src-tauri/src/config/mod.rs` (model prefs), `src-tauri/src/core/workspace.rs`.

Tests: workspace resolution precedence; model-selection round-trip; JSON envelope golden.

Acceptance: `cargo build`; `docfoo version --verbose`; `docfoo help`; `docfoo kg --query x`
fails with a clear "sidecar not built" error.

#### 1.2 Sidecar + model/auth

Deliverables:

- `sidecar/`: `main.ts` stdin loop, `complete.ts` (adapted `model-query.ts`), `providers.ts`
  (auth status/set/logout, model list), `package.json` pinned to Pi 0.86.1, build scripts.
- `sidecar.rs`: lazy spawn of `docfoo-agent` next to the binary (or `DOCFOO_SIDECAR_BIN`;
  `DOCFOO_SIDECAR_TS` dev fallback via Bun); pending-request registry keyed by `requestId`;
  delta routing; cancel; restart-on-crash with the app's 1/2/4s backoff pattern; fail all
  pending on exit.
- `model --get/--set/--list`; `auth --status/--set/--logout`.
- `AgentChatClient`-equivalent plumbing (`CompletionRequest` → sidecar) used by later stages.

Reference: `agent/model-query.ts`, `agent/providers.ts`, `src-tauri/src/core/model_bridge.rs`
(backoff/lifecycle ideas), `src-tauri/src/config/mod.rs` (secrets fallback if ever needed).

Tests: fake sidecar binary with canned responses; spawn/cancel/timeout/restart; model set
validation; auth status never leaks keys.

Acceptance: `docfoo model --list` prints providers; `docfoo auth --set openrouter` stores a key
that survives a restart (verify via `auth --status`, not by printing); `docfoo model --set kg
<provider/model>` writes the workspace file.

#### 1.3 KG index + query

Deliverables:

- `kg/paths.rs`: graph path rules (top-level vs scoped), `kg --status` listing.
- `kg/settings.rs`: port of `src-tauri/src/kg/settings.rs` (load/parse/clamp/migrate
  `kg-settings.json`).
- `kg/index.rs`: port of `src-tauri/src/kg/mod.rs::kg_index` minus Tauri: corpus walk via
  `docfoo_kg::corpus::load_documents`, `build::run` with `IndexOptions`, progress mapping,
  cancel flag.
- `kg/query.rs`: port of `src-tauri/src/kg/query.rs` minus Tauri: load graph + settings,
  `query::retrieve`, collect `trace.evidence_sections` as sources, build envelope.
- `kg/decision.rs`: `DecisionClient` over the same sidecar client for Jev concept routing
  (port of `src-tauri/src/kg/decision.rs`), disabled when unconfigured.
- `commands/kg.rs`: `--index`, `--query`, `--status` with all flags.
- Rich output: citation regex identical to `src/lib/citations.ts`; figure extraction; Slack
  renderer; `--hermes-final`; `--save` writes `kg-chats/kg-<uuid>/{meta,kg-history}.json`
  (port of `src-tauri/src/kg/chats.rs`).
- Ctrl-C cancellation.

Reference: `crates/docfoo-kg/src/{build.rs,query/mod.rs,query/synthesis.rs}`,
`src-tauri/src/kg/{mod.rs,query.rs,bridge.rs,decision.rs,chats.rs,settings.rs}`,
`src/lib/citations.ts` (regex + label rules), `agent/kg-tool.ts` (answer shape).

Tests:

- Fixture workspace with 2 tiny resource cards + prebuilt `graphs/top-level.json`.
- Fake sidecar returns canned synthesis with `[S1]` tags, a figure line, and a table.
- Assert: answer expansion, citations (start/end), figure absolute paths, sources, JSON schema,
  Slack `MEDIA:` conversion, sentinel placement, `--save` round-trip.
- Index test with a fake sidecar returning schema-locked extraction JSON; assert graph written
  and incremental re-run skips unchanged sections.

Acceptance (the Stage 1 demo):

```bash
export DOCFOO_WORKSPACE=/mnt/c/Users/abhik/Documents/Test/DocFoo/db   # or a WSL-local copy
docfoo kg --status
docfoo kg --query "How is apples connected to mangoes?" --json | jq .
docfoo kg --query "How is apples connected to mangoes?" --format slack --hermes-final
```

The Slack output contains the answer, `MEDIA:/abs/path` figure lines, inline citations, and a
`Sources:` section — ready for Hermes to relay verbatim.

### Stage 2 — Resources & notes (read-only)

**Status:** 🟢 done

Deliverables:

- `resources/`: port `src-tauri/src/resources/tree.rs` (ResourceEntry classification, sorting,
  hidden-file rules) and the read/outline/search logic from `agent/read-tools.ts`
  (`list_resources`, `read_resource`, `outline_resource`, `search_resources`).
- `notes/`: read `notes.json` and filter by resource; expose note images paths.
- `commands/resources.rs`, `commands/notes.rs` with all flags in §4.
- Numbered-line output for `--read`; figure markdown lines for `--figures`.

Tests: fixture tree assertions; outline/search hits; notes filtering; JSON shapes.

Acceptance: `docfoo resources --list --json`, `--outline`, `--search` and `docfoo notes --list`
work against the app's real `db/` workspace without writing anything.

### Stage 3 — Backup/restore + collections download

**Status:** 🟢 done

Deliverables:

- `backup/`: port `src-tauri/src/backup/zip.rs` (`create_backup_zip`, `restore_from_zip`,
  staging/rollback/quota/journal) + needed `common/util` helpers. Format stays
  `docfoo-backup` v1.
- `collections/`: port `src-tauri/src/collections/client.rs` (list/info/download with the Koofr
  `Referer` handling), `zip_util.rs::extract`, and `api.rs::collections_download` install rules
  (resource → `resources/<name>` with suffixing; KG → `graphs/<components>/graph.json`).
- `commands/backup.rs`, `commands/restore.rs`, `commands/collections.rs`.
- `--yes` confirmation for restore; `--force` for overwrite.

Tests: fixture workspace → zip → fresh workspace restore equality; malformed/manifest-invalid
zips rejected; mock HTTP server for `/items` and a fixture collection zip for both types.

Acceptance: round-trip against the app's backup file; `docfoo collections --list` and
`--download` install a resource and a graph.

### Stage 4 — Scan + setup

**Status:** 🟢 done

Deliverables:

- `setup.rs`: `--check` validation; `--from DIR` copies `PP-DocLayoutV3.onnx`; downloads pinned
  Linux `libonnxruntime.so` (ONNX Runtime **1.28.0**, matching the bundled Windows DLL) and
  `libpdfium.so` (PDFium **151.0.7881.0**, bblanchon/pdfium-binaries) with SHA-256 checksums;
  cache in `~/.cache/docfoo/models`; `--force` re-downloads. Env overrides still win.
- `scan/`: `SidecarCompletionClient` implementing `docfoo_ocr::CompletionClient`; destination
  rules ported from `src-tauri/src/scan/mod.rs`; progress mapping to stderr; Ctrl-C cancel.
- `commands/scan.rs`: `--parallel`, `--text_model` (default `scanModelKey`), `--figure_model`
  (default = `--text_model`), `--output`, `--pages`, `--prompt`, `--analysis-prompt`,
  `--no-figures`. Uses `run_import` from `docfoo-ocr` with `models_dir` from setup.
- Optional `warm_layout()` pre-flight on first run.

Tests: destination sanitization/uniqueness; flag→`ImportOptions` mapping (figure model fallback);
completion client contract; `setup --check` on missing/present files. Full OCR is validated
manually with the real model (CI stays native-lib-free).

Acceptance (WSL): `docfoo setup` then `docfoo scan sample.pdf --parallel 4 --text_model
<key> --figure_model <key>` produces `resources/sample/content.md` + `assets/`, matching the
app's output shape.

### Stage 5 — Packaging, update, Hermes integration docs

Deliverables:

- `install.sh`: detects Linux x64/WSL; installs `docfoo` + `docfoo-agent` to `~/.local/bin`;
  runs `docfoo setup` (skippable); `--local` builds from source; verifies SHA-256.
- Release tarball layout + `scripts/release.sh`/`.ps1` that vendors the two crates and builds
  both platform artifacts.
- `docfoo update --check` (GitHub releases API) and self-update when a release manifest exists
  (download, checksum, atomic replace of `docfoo` + `docfoo-agent`).
- `docs/HERMES.md`: the sentinel patch, a skill/instruction template for Hermes, the
  `platforms.slack.extra.rich_blocks: true` recommendation, and example prompts.
- `README.md` polish, shell completions (`clap_complete`).

Acceptance: `curl … | sh` install on WSL; `docfoo update --check` prints the version;
`docs/HERMES.md` instructions are copy-pasteable.

### Stage 6 — Hardening & acceptance

- Windows build verification (native libs from `DocFoo/models`, `%LOCALAPPDATA%` cache).
- Error-message pass (missing graph, missing auth, missing native libs, unknown model).
- `--quiet`/`--verbose` behavior, no secrets in logs, no absolute paths in answers (except
  `abs_path` fields and `MEDIA:` lines).
- End-to-end acceptance script for Hermes: index → query → slack output → sentinel.
- Performance sanity: sidecar cold start, KG query latency vs the desktop app.

---

## 8. Reuse map (DocFoo references)

| CLI piece | DocFoo source |
|---|---|
| KG build/query engine | `crates/docfoo-kg/src/{build.rs, query/}` (path dep, unchanged) |
| OCR pipeline | `crates/docfoo-ocr/src/` (path dep, unchanged) |
| Model bridge semantics | `src-tauri/src/core/model_bridge.rs` |
| Completion request shape | `agent/model-query.ts` |
| Provider/auth/model list | `agent/providers.ts` |
| KG index driver | `src-tauri/src/kg/mod.rs` |
| KG query driver + sources | `src-tauri/src/kg/query.rs` |
| KG decision (Jev) | `src-tauri/src/kg/decision.rs` |
| KG chat persistence (`--save`) | `src-tauri/src/kg/chats.rs` |
| KG settings | `src-tauri/src/kg/settings.rs` + `crates/docfoo-kg/src/tunables.rs` |
| Citation parsing/labels | `src/lib/citations.ts` |
| Resource tree | `src-tauri/src/resources/tree.rs` |
| Resource read/outline/search | `agent/read-tools.ts`, `agent/resource-format.ts` |
| Notes read | `src-tauri/src/resources/notes.rs` |
| Scan destination/progress | `src-tauri/src/scan/mod.rs` |
| Backup zip | `src-tauri/src/backup/zip.rs` |
| Collections client/install | `src-tauri/src/collections/{client.rs,zip_util.rs,api.rs}` |
| Model preferences | `src-tauri/src/config/mod.rs` |
| Workspace rules | `src-tauri/src/core/workspace.rs` |

---

## 9. Risks & mitigations

| Risk | Mitigation |
|---|---|
| `docfoo-kg`/`docfoo-ocr` API drift in the sibling repo | Path deps pinned to the current `Cargo.lock`; a `scripts/vendor.sh` freezes the exact revisions into the release tarball. |
| Bun sidecar binary size / antivirus flags on Windows | Same `--compile` approach as the app (already shipping ~122 MB); document the trade-off; keep `DOCFOO_SIDECAR_TS` fallback. |
| Native OCR libs mismatch (`ort` ABI, PDFium) | Pin ONNX Runtime 1.28.0 and PDFium 151.0.7881.0 to match the bundled Windows DLLs; `setup --check` validates loadability. |
| `/mnt/c` I/O slowness when pointing at the Windows workspace | Document `--workspace` trade-off; recommend a WSL-local copy for indexing/scan; reads still work. |
| Hermes terminal output truncation/spill for long answers | `--format slack` is compact; `Sources:` is short by default; `--max-chars` optional; sentinel patch reads the tool result before any downstream summarization. |
| Hermes rich_blocks off → raw pipe tables | `--plain-tables` converts to bullet lines; docs recommend enabling `rich_blocks`. |
| No public URL for `PP-DocLayoutV3.onnx` | `setup --from DIR` copies from a local DocFoo checkout; `--layout-model-url` override; docs point at the app's `models/`. |
| Sidecar crash mid-query | Pending requests fail fast with a clear error; one automatic restart per command; no silent hangs (300s ceiling like the app). |
| Secrets in `--json` output | Auth commands only ever report `configured/source/label`; key values are write-only. |

---

## 10. Hermes integration notes

### 10.1 What the CLI provides

- `docfoo kg --query Q --format slack --hermes-final` prints a complete Slack-ready message:
  answer markdown, `MEDIA:/abs/path` figures, inline citations, `Sources:` section, and the
  `[[hermes:final]]` sentinel on the first line.
- Hermes can run it through its `terminal` tool; the answer is final by construction — the CLI
  performs exactly one synthesis call.

### 10.2 The ~10-line Hermes patch (documented, not implemented here)

Hermes' loop always calls the model again after a tool round
(`agent/turn_tool_round.py` → `"continue"`). To give a tool result final-answer semantics,
patch `run_tool_round()` right after `agent._execute_tool_calls(...)` (around line 152) and
after the `_incremental_persistence_failed` check:

```python
_FINAL_SENTINEL = "[[hermes:final]]"
for _msg in reversed(messages):
    if _msg.get("role") != "tool":
        break
    _content = _msg.get("content")
    if isinstance(_content, str) and _content.lstrip().startswith(_FINAL_SENTINEL):
        final_response = _content.lstrip()[len(_FINAL_SENTINEL):].lstrip("\n")
        _turn_exit_reason = "tool_final"
        append_message(messages, {"role": "assistant", "content": final_response})
        if agent.stream_delta_callback:
            with suppress(Exception):
                agent.stream_delta_callback(final_response)
                agent.stream_delta_callback(None)
        return _verdict("break")
```

This mirrors Pi's `terminate: true` behavior. Without the patch, use the relay convention: a
Hermes skill instructs the model to run the command and output its stdout verbatim.

### 10.3 Hermes configuration

- Enable native Slack table rendering:
  `platforms.slack.extra.rich_blocks: true` (read by
  `plugins/platforms/slack/block_kit.py`; falls back to plain mrkdwn if rendering fails).
- Keep `display.tool_progress` off so tool chatter doesn't compete with the answer.
- Hermes already supports `MEDIA:/absolute/path` uploads from responses; the sentinel patch
  routes CLI output through the same delivery path.

---

## 11. Out of scope / future

- `docfoo ask` (full agent loop) — revisit if Hermes ever needs multi-step resource reasoning.
- Koofr cloud backup (`backup cloud push/pull`), community uploads.
- OAuth provider logins in the CLI (use the desktop app or `pi` CLI).
- Podcasts, calendar, Google tools, subagents/skills, notebooks.
- macOS builds, arm64 Linux.
- `docfoo kg --visualize`, KG chat history browsing, interactive TUI.

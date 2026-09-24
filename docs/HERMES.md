# DocFoo CLI × Hermes (Slack)

This guide wires the `docfoo` CLI into Hermes Agent so a Slack question can be
answered from the user's DocFoo knowledge graph with figures, tables and exact
citations — ideally with **no second model pass** over the answer.

## 1. What the CLI gives Hermes

`docfoo kg --query` is a one-shot final answer: retrieval + exactly one
synthesis call. Nothing else calls a model. The output can be made
Slack-ready in one command:

```bash
docfoo kg --query "How are the National Education Policy 2020 and the NCF connected?" \
  --format slack --hermes-final
```

- `--format slack` keeps GFM tables (native Slack table blocks when rich
  rendering is on), turns local figures into `MEDIA:/absolute/path` lines, and
  appends a compact `Sources:` section.
- `--hermes-final` prepends the `[[hermes:final]]` sentinel so a patched
  Hermes can return the rest verbatim.
- `--quote-sources` adds the exact cited lines under each source (verbose).
- `--plain-tables` converts tables to bullet lines if Slack rich blocks are
  off.
- `--json` emits the machine envelope (`answer_markdown`, `citations`,
  `figures`, `tables`, `sources`) if Hermes prefers structured data.

Other commands Hermes can use: `resources --outline/--read/--search`,
`notes --list`, `collections --download`, `scan`, `model`, `auth`, `setup`.

## 2. Recommended Hermes configuration

Enable native Slack tables and keep tool chatter out of the answer:

```yaml
platforms:
  slack:
    extra:
      rich_blocks: true
display:
  slack:
    tool_progress: false
```

`rich_blocks` makes `plugins/platforms/slack/block_kit.py` render GFM pipe
tables as native `table` blocks; if rendering fails it falls back to plain
text, so it is safe to enable.

## 3. The relay convention (no Hermes changes)

If you do not patch Hermes, add an instruction so the model does not rewrite
the answer. Example skill/instruction text:

> When the user asks a question about their DocFoo library, run
> `docfoo kg --query "<question>" --format slack --hermes-final` with the
> terminal tool. The command's stdout is the complete, Slack-ready answer.
> Output it **verbatim** as your final message: do not paraphrase, summarize,
> translate, reorder, or add commentary. Keep `MEDIA:` lines exactly where
> they are. If the command fails, report the error text.

With the plugin below this routing is injected automatically and the turn ends
right after the CLI call, so this text is only the fallback for environments
where plugins cannot load. Without either, the model still relays the content
but costs one extra turn.

## 4. The `docfoo_plugin` integration

The plugin is the entire Hermes integration — no skill install, no
`soul.md`/persona edit, no core patch. At load time it registers three things:

1. **A direct trigger route** — the only query path. Any message containing
   the word `dofoq` (case-insensitive: `dofoq`, `Dofoq`, `DoFoq`, …) is
   intercepted by a `pre_gateway_dispatch` hook and answered by the CLI
   directly. The message never reaches the model, so the only latency left is
   docfoo's synthesis (~6s). The reply goes through the platform adapter with
   the same typing indicator, markdown and `MEDIA:` figure handling as a
   normal answer; authorization is still enforced.
2. **A `run_tool_round` wrapper** so a tool result carrying the
   `[[hermes:final]]` sentinel ends the turn with that text — used whenever
   Hermes is explicitly asked to run docfoo. It unwraps Hermes' terminal
   envelope `{"output": "...", "exit_code": 0, "error": null}`.
3. **An advertised skill** (`~/.hermes/skills/docfoo/SKILL.md`, installed by
   the same command) that tells the model `docfoo` is on PATH
   (`~/.local/bin/docfoo`) with workspace `~/.docfoo`, how to inspect and
   configure it, and never to search the filesystem or run `kg --query` on its
   own.

There is deliberately **no system prompt section**: the model is never told to
query docfoo on its own, so ordinary mentions ("change docfoo's model", "scan
this pdf with docfoo") go to Hermes as normal.

Install it once (cross-platform, pure Python):

```bash
python3 hermes/install.py \
  [--workspace DIR] [--model PROVIDER/MODEL] [--scope RESOURCE] [--bin PATH]
python3 hermes/install.py --check
python3 hermes/install.py --uninstall
```

- `--workspace` — the DocFoo library (the desktop app's `db/` folder works).
- `--model` — provider/model for kg queries, e.g.
  `opencode-go/muse-spark-1.2-contributor`.
- `--scope` — default `--scope` for kg queries (omit for the whole library).
- `--bin` — `docfoo` executable name or absolute path. Use an absolute path
  (`/home/<user>/.local/bin/docfoo`) so the gateway's service PATH cannot hide
  the binary.

The installer writes `<HERMES_HOME>/plugins/docfoo_plugin/` (default
`~/.hermes`) and runs `hermes plugins enable docfoo_plugin
--no-allow-tool-override`. Restart the Hermes gateway afterwards. No core
Hermes files are modified.

**Why a plugin:** `hermes update` is git-based and autostashes/switches
branches on a dirty tree, so a source patch would need re-application (and can
conflict) on every update. User plugins live outside the repo and load at
startup, so this survives updates automatically. If a future Hermes renames
`run_tool_round`, the wrapper stops applying and the sentinel is simply
ignored — the turn still completes, just with a paraphrase.

### Build and install the CLI in WSL/Linux

Hermes in WSL needs native Linux binaries: a Windows `docfoo.exe` can still
run through interop, but its `MEDIA:` paths and sidecar would be Windows-side.
In the WSL shell:

```bash
cd /mnt/c/Users/<you>/Documents/Test/DocFoo_CLI

# 1. CLI (release)
CARGO_TARGET_DIR=$HOME/.cache/docfoo-target cargo build --release
install -m 755 $HOME/.cache/docfoo-target/release/docfoo ~/.local/bin/docfoo

# 2. Sidecar — build with a *Linux* bun. The `bun` on PATH inside WSL is
#    often the Windows npm shim, which cross-builds an .exe.
curl -fsSL -o /tmp/bun.zip \
  https://github.com/oven-sh/bun/releases/download/bun-v1.3.11/bun-linux-x64.zip
python3 -c "import zipfile; zipfile.ZipFile('/tmp/bun.zip').extractall('/tmp/bun')"
install -m 755 /tmp/bun/bun-linux-x64/bun ~/.local/bin/bun
mkdir -p ~/.cache/docfoo-sidecar
(cd sidecar && tar --exclude=node_modules -cf - .) \
  | (cd ~/.cache/docfoo-sidecar && tar -xf -)
cd ~/.cache/docfoo-sidecar && rm -rf node_modules
~/.local/bin/bun install
~/.local/bin/bun build --compile ./main.ts --outfile docfoo-agent
install -m 755 docfoo-agent ~/.local/bin/docfoo-agent

# 3. Credentials (stored in Pi's auth.json; no env var needed at query time)
~/.local/bin/docfoo --workspace /mnt/c/.../db auth --set opencode-go --key "$OPENCODE_GO_API_KEY"
```

The CLI finds `docfoo-agent` next to itself. `scripts/release.sh` builds the
same pair plus a tarball when run on a Linux host (it forces the Linux sidecar
target).

### Manual fallback (no plugin system)

If plugins cannot load, patch `agent/turn_tool_round.py` in `run_tool_round()`
right after `agent._execute_tool_calls(...)` and the
`_incremental_persistence_failed` check (around line 160), and re-apply after
every `hermes update`:

```python
    # --- DocFoo CLI: a tool result marked [[hermes:final]] IS the answer ---
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

- `append_message` and `suppress` are already imported/defined in that module.
- The patch ends the turn after the tool batch, so Hermes never paraphrases
  the KG answer. Hermes still makes the *first* model call that chooses the
  terminal tool; only the second (rewrite) call is removed.
- Without the patch/plugin, `--hermes-final` is harmless: the sentinel is just
  the first line of the text.

## 5. Figures and media

Hermes already uploads `MEDIA:/absolute/path` files found in an assistant
message. `--format slack` resolves each `![](card/assets/figure.png)` against
`<workspace>/resources/` and replaces it with a `MEDIA:` line only when the
file exists; otherwise the markdown is left untouched.

Because Slack renders an uploaded image separately from the text, keep the
answer's caption line (`*Figure 2 — …*`) in place — the CLI preserves it.

## 6. Example Slack questions → commands

| Slack question | Command Hermes runs |
|---|---|
| "How is X connected to Y?" | `docfoo kg --query "How is X connected to Y?" --format slack --hermes-final` |
| "What does the paper say about Z?" | `docfoo kg --query "..." --scope papers/ml --format slack --hermes-final` |
| "Search my library for 'attention'" | `docfoo resources --search "attention" --json` |
| "Outline the UniverSat paper" | `docfoo resources --outline single_column_test/content.md --json` |
| "Read lines 75–105 of that paper" | `docfoo resources --read single_column_test/content.md --offset 75 --limit 31 --json` |
| "Save a backup" | `docfoo backup --out /tmp/docfoo-backup.zip --json` |

For a graph that does not exist yet, run `docfoo kg --index [--scope DIR]`
first (this uses the model configured for the `kg` slot).

## 7. Troubleshooting

- **`no knowledge graph … run docfoo kg --index first`** — index the scope
  once (`docfoo kg --index`), or download a shared graph with
  `docfoo collections --download <id>`.
- **`no model selected`** — `docfoo model --set kg provider/model`, or pass
  `--model` per query. `docfoo auth --status` shows which providers are
  configured; `docfoo auth --set <provider>` stores an API key.
- **Rate limits / `429`** — the provider rejected the request; switch models
  or retry. The CLI reports the provider's own message.
- **Missing scan dependencies** — `docfoo setup` (Linux downloads pinned
  ONNX Runtime + PDFium; the layout model comes from
  `--from /path/to/DocFoo/models`).
- **Long output truncated by Hermes** — the terminal tool spills long outputs
  to a file. Keep `--format slack` (compact by default), drop
  `--quote-sources`, or cap with `--max-chars N`.
- **Tables render as raw pipes** — enable
  `platforms.slack.extra.rich_blocks: true`, or pass `--plain-tables`.

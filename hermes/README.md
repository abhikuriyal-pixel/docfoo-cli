# Hermes integration (`docfoo_plugin`)

The plugin is the entire Hermes integration — no skill install, no
`soul.md`/persona edit, no core patch. It registers:

- an **advertised skill** (`<hermes-home>/skills/docfoo/SKILL.md`) that tells
  Hermes the CLI is on PATH (`~/.local/bin/docfoo`) with workspace `~/.docfoo`
  and how to inspect/configure it — so it runs `docfoo` instead of searching
  the filesystem;
- a **`run_tool_round` wrapper** so a tool result carrying the
  `[[hermes:final]]` sentinel ends the turn with that text — Hermes never
  makes the second (paraphrase) model call. It unwraps Hermes' terminal
  envelope `{"output": "...", "exit_code": 0, "error": null}`;
- a **direct trigger route** (see below): messages containing the word
  `dofoq` (any case) skip the model entirely.

## Install (once per machine)

```bash
python3 hermes/install.py \
  --workspace /path/to/DocFoo/db \
  --model opencode-go/muse-spark-1.2-contributor \
  --scope single_column_1 \
  --bin /home/<user>/.local/bin/docfoo
```

Flags: `--workspace` (library / app `db/`), `--model` (kg query model),
`--scope` (default kg scope; omit for the whole library), `--reasoning`
(kg thinking level; `off` is fastest), `--bin` (absolute path recommended for
the gateway service), `--trigger`, `--hermes-home DIR`.

```bash
python3 hermes/install.py --check       # exit 0 when installed + show config
python3 hermes/install.py --uninstall   # disable + remove
python3 hermes/install.py --no-enable   # copy only (enable manually)
```

The installer writes `<HERMES_HOME>/plugins/docfoo_plugin/` (default
`~/.hermes`) and the skill `<HERMES_HOME>/skills/docfoo/SKILL.md`, then runs
`hermes plugins enable docfoo_plugin --no-allow-tool-override`. **Restart the
Hermes gateway afterwards** so a running process picks up the new plugin.

## Direct trigger route (no model call)

Any incoming message containing the word **dofoq** (any case: `Dofoq`, `DoFoq`,
`DOFOQ`) is intercepted by a `pre_gateway_dispatch` hook and answered by the
CLI directly: the message never reaches the model, so the only latency is
docfoo's synthesis (~6s) instead of synthesis + Hermes' tool-selection call
(~12-19s). The reply is delivered through the platform adapter — typing
indicator, markdown, `MEDIA:` figures — and authorization is still checked.
Messages starting with `/` are left to the normal command registry.

```
dofoq what is the Universal Patch Encoder?
```

Normal mentions of docfoo ("can you change docfoo's model?", "scan and index
this pdf with docfoo") do **not** trigger the route — they go to Hermes like
any other message. The trigger is configurable with `--trigger`.

## Why a plugin instead of a source patch

`hermes update` is git-based and autostashes/switches branches on a dirty
working tree, so a patch to `agent/turn_tool_round.py` would be stashed or
conflict on every update. User plugins live outside the repo and are loaded at
startup, so this needs **no re-application**. If a future Hermes renames
`run_tool_round`, the wrapper stops applying and the sentinel is simply
ignored — the turn still completes, just with a paraphrase.

## Verify

```bash
hermes plugins list | grep docfoo_plugin
hermes -z "According to my documents, what is the Universal Patch Encoder?" --yolo
```

The output should be the CLI's markdown verbatim (inline
`[resource/content.md:lines]` citations, `MEDIA:/abs/path` lines, a `Sources:`
section). Confirm the loop skipped the second model call by comparing the last
assistant message with the terminal tool's `output` in
`~/.hermes/state.db` — they are byte-identical.

See `docs/HERMES.md` for the full Slack guide, the WSL build steps, and
example commands.
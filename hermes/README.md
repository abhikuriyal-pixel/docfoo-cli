# Hermes integration (`docfoo_plugin`)

Makes a DocFoo CLI tool result the **final** Slack answer instead of letting
Hermes paraphrase it. When the CLI is called with `--hermes-final`, stdout
starts with `[[hermes:final]]`; the plugin wraps `run_tool_round` so that
result ends the turn — the same `terminate` behavior DocFoo's Buddy uses.

## Install (once per machine)

```bash
# Linux / WSL
python3 hermes/install.py

# Windows
python hermes\install.py
```

The installer writes `<HERMES_HOME>/plugins/docfoo_plugin/` (default
`~/.hermes`) and runs `hermes plugins enable docfoo_plugin
--no-allow-tool-override`. Restart the Hermes gateway afterwards.

```bash
python3 hermes/install.py --check       # exit 0 when installed
python3 hermes/install.py --uninstall   # disable + remove
python3 hermes/install.py --no-enable   # copy only (enable manually)
```

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
# then from a Hermes session: run a docfoo kg query via the terminal tool
# and confirm the final Slack message is the CLI output, not a rewrite.
```

See `docs/HERMES.md` for the full Slack guide and example commands.

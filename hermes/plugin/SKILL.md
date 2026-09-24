---
name: docfoo
description: Inspect and configure the docfoo CLI — models, auth, resources, backups, scanning. docfoo is on PATH; never search the filesystem for it.
---

# DocFoo CLI

`docfoo` is installed at `~/.local/bin/docfoo` (on PATH). Its workspace —
`model-selection.json`, `resources/`, `graphs/` — is `~/.docfoo`. Run it
directly; never search the filesystem for it.

## Configuration (models, auth)

```bash
docfoo model --get kg --json          # one slot: chat, scan, scan-analysis, kg
docfoo model --list --json            # model catalog for configured providers
docfoo model --set kg opencode-go/muse-spark-1.2-contributor
docfoo auth --status --json
```

All four slots at once: `cat ~/.docfoo/model-selection.json` (keys
`chatModelKey`, `kgModelKey`, `scanModelKey`, `scanAnalysisModelKey`).

## Document Q&A

Never run `docfoo kg --query` on your own. Document questions reach the CLI
only when the user prefixes the message with `dofoq`, and that path is handled
outside the agent. Run `kg --query` only if the user explicitly asks you to.

## Browse the library

```bash
docfoo resources --list --json
docfoo resources --outline <resource>/content.md --json
docfoo resources --read <resource>/content.md --offset 75 --limit 31 --json
docfoo resources --search "attention" --json
docfoo notes --list --json
```

## Maintenance

```bash
docfoo kg --status --json
docfoo backup --out /tmp/docfoo-backup.zip --json
docfoo restore /tmp/docfoo-backup.zip --json
docfoo collections --list --json
docfoo collections --download <id> --json
docfoo scan <file.pdf|image.png> --parallel 4 --json
```

## Rules

- Configuration, auth and library questions: run `docfoo` from PATH. Do not
  search the filesystem, and do not run `ls`/`find` to locate it.
- Never run `docfoo kg --query` unless the user explicitly asks.
- If a command fails, report its error text.

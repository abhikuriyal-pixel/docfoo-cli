---
name: docfoo
description: DocFoo CLI reference — query the user's document library and use the other library commands.
---

# DocFoo CLI

`docfoo` works on the user's DocFoo library (a workspace directory holding
`resources/`, `graphs/` and notes). The binaries are installed; no setup is
needed.

## Ask a question (main path)

```bash
docfoo [--workspace DIR] kg --query "<question>" --format slack --hermes-final [--model PROVIDER/MODEL]
```

- stdout is the final, Slack-ready answer: figures as `MEDIA:/abs/path`
  lines, GFM tables, inline citations and a `Sources:` section. Relay it
  verbatim.
- `--scope <dir>` limits retrieval to one resource folder.
- `--json` returns the machine envelope instead (`answer_markdown`,
  `citations`, `figures`, `tables`, `sources`).
- Without `--hermes-final` the answer is plain markdown (no sentinel).

If no graph exists yet, index once: `docfoo kg --index [--scope DIR]`.

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
docfoo model --get kg --json
docfoo auth --status --json
docfoo scan <file.pdf|image.png> --parallel 4 --json
```

## Rules

- Questions about the user's documents: always run `docfoo kg --query`; never
  answer from memory or earlier context.
- Output the CLI result verbatim; do not add commentary or reorder sections.
- Keep `MEDIA:` lines exactly as they are.
- If a command fails, report its error text.

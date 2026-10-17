# DocFoo CLI quick reference

These examples describe the current CLI interfaces shown on the product page.
Installed releases can differ. Start with `docfoo --help` and
`docfoo kg --status --json`; check advertised capabilities before passing newer
snapshot, progress or request-local decision flags. The public release archive
may lag the source checkout used for the recorded demonstration.

## Workspace and models

The default library is `~/.docfoo` (the user profile's `.docfoo` folder on Windows).
Use `--workspace DIR` to select another library or an isolated workspace.
Keep the matching `docfoo` and `docfoo-agent` binaries together.

```sh
docfoo doctor
docfoo setup
docfoo model --list
docfoo auth --set PROVIDER
docfoo model --set scan provider/vision-model
docfoo model --set kg provider/answer-model
```

`doctor` is local/read-only. `setup` downloads native scan dependencies.
Provider/model names above are placeholders, not literal catalog entries.
Scanning, indexing and synthesis use the endpoints you configure; provider
charges and data handling apply. Never put credentials in command examples,
query metadata or source documents.

For a local OpenAI-compatible provider, merge the provider from
`../integrations/models.example.json` into `<workspace>/.agent/models.json`.
Use the exact model ID loaded by your server. For the recorded 16K-context
example, see `../integrations/kg-settings.example.json`. That example disables
hosted concept routing and bounds evidence delivery; it is not a universal
context preset. Configure server-side thinking separately if required.

## Scan, index and query

```sh
docfoo scan paper.pdf --output references
docfoo kg --index --scope references
docfoo kg --status --json
docfoo kg --query "What supports the main result?" --scope references --json --quiet
```

Scanning accepts PDFs and supported images, and produces Markdown plus saved
assets. Indexing discovers visible Markdown resources. Scope is a folder
relative to the library's resources directory. Rerunning the same scan source
and destination resumes checkpointed work. Re-index affected scopes after
changing or restoring resources.

Query overrides:

```sh
docfoo kg --query "YOUR QUESTION" --model PROVIDER/MODEL --reasoning off --json
docfoo kg --query "YOUR QUESTION" --no-decision-routing --json
docfoo kg --query "YOUR QUESTION" --json --snapshot
docfoo kg --query "YOUR QUESTION" --json --progress
```

`--snapshot` needs `kg.query.snapshot.v1`; `--progress` needs
`kg.query.progress.v1`. Check these in `kg --status --json`.
Snapshots are bounded (1,000 nodes / 4,000 edges / 500 frames); they are not
an exhaustive internal execution log. `replayError` does not invalidate a
successfully generated answer.

Hosted concept routing is independent of the answer model. Request-local
`--decision-provider openrouter|opencode|typesafe` and `--decision-model ID`
need `kg.decision-model/1`. Credentials come from the selected provider's
environment. Router failure is non-fatal and falls back to the existing term
bridge / lexical path. `--no-decision-routing` wins over routing overrides.

## Inspect the original library

```sh
docfoo resources --list --tree --figures --json
docfoo resources --outline references/Paper/content.md --json
docfoo resources --read references/Paper/content.md --offset 20 --limit 40 --json
docfoo resources --search "technical term" --rel references --context 2 --json
docfoo notes --list --json
```

These resource/note reads are read-only. Resource paths must stay relative to
the library and cannot escape it or traverse symlinks. Source reads are bounded
and line-numbered. Use exact paths returned in the query metadata rather than
inventing filenames.

## Structured output

`--json` uses the stable `docfoo.cli/1` envelope. A query's `data` includes
`answer_markdown`, `citations`, `figures`, `tables`, `sources`, `routing`,
`droppedCount`, timings and optionally `replay`. Code and equations remain in
Markdown and source sections; there is no invented top-level code-snippet
field. Figure absolute paths, when returned by the CLI, are local paths—not
public URLs.

Results go to stdout; diagnostics/progress go to stderr. Exit codes are 0
(success), 1 (runtime failure) and 2 (invalid usage). A scan's partial-result
contract requires inspecting per-file `ok`, `partial` and `cancelled`, not just
the process exit code. Do not parse the human-readable renderer as an API.

## Library management

```sh
docfoo backup --out backups/library.zip
docfoo restore backups/library.zip --yes
docfoo collections --list --json
docfoo update --check
```

Backups are unencrypted. They include supported library/chat metadata, not
graph databases, credentials or native models. Close other library writers
before restore and rebuild affected graph scopes afterward. Keep backups
outside the content being backed up. Restore is journaled and staged, not one
atomic whole-workspace swap.

## Support

Public documentation and downloads:
https://github.com/abhikuriyal-pixel/docfoo-cli

Issues: https://github.com/abhikuriyal-pixel/docfoo-cli/issues

Email: docfoo.work@gmail.com

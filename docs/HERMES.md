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

This costs one extra model turn (the model decides to relay), but the content
is passed through unchanged. For the fastest path, patch Hermes as below.

## 4. True terminate semantics (the ~10-line Hermes patch)

Hermes' loop always calls the model again after a tool round
(`agent/turn_tool_round.py` returns `"continue"`), so a tool result normally
becomes context rather than the final message. The CLI's
`[[hermes:final]]` sentinel exists so a small patch can end the turn with the
tool output.

Edit `~/.hermes/hermes-agent/agent/turn_tool_round.py` in `run_tool_round()`,
right after `agent._execute_tool_calls(...)` and the
`_incremental_persistence_failed` check (around line 160):

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

Notes:

- `append_message` and `suppress` are already imported/defined in that module.
- The patch ends the turn after the tool batch, so Hermes never paraphrases
  the KG answer. Hermes still makes the *first* model call that chooses the
  terminal tool; only the second (rewrite) call is removed.
- After patching, restart the Hermes gateway. Keep the relay instruction from
  §3 as a fallback for older sessions.
- Without the patch, `--hermes-final` is harmless: the sentinel is just the
  first line of the text.

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

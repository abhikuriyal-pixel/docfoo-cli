# DocFoo vs Agentic search — selected Luna capture

The user selected the **first gpt-6-luna pair**. The page displays
**8.8149914 s DocFoo**, excluding outer CLI startup, and **21.3220343 s agentic
search**. The original **9.0886889 s** DocFoo wall time is retained in the data. This is not the fastest of four Luna pairs,
not an average, and not a general performance or answer-quality benchmark.
Both full outputs are preserved in `recorded.json` and `*-original.md`.

## Matched setup

- Exact same multi-hop question, model `openai-codex/gpt-6-luna`, low thinking,
  SSE transport and 6,000-token per-call output cap.
- Existing Papers graph backed up with SQLite's read-only backup interface;
  byte-identical scanned-resource copies. No reindexing or source changes.
  The graph build UID, counts and resource hashes are in the capture.
- DocFoo: ordinary retrieval defaults, unlimited evidence delivery, 57 sections,
  no auxiliary Jev routing, one writer call with 53,857 input tokens.
- Agentic: a fresh in-memory Pi SDK session with read-only `find`, `grep`,
  `read` and `ls`, scoped to those same raw resources. No graph, shell, web,
  previous conversation, extensions or skills. Five model calls, eight file-tool
  calls; some tools ran concurrently.
- End-to-end measurements include CLI/sidecar startup for DocFoo and in-memory
  session setup for the agent. Authentication, snapshot preparation, scanning
  and prior graph indexing are excluded. The two methods have different setup
  overhead and answer lengths. Neither is billed by a public page visitor.
- Token usage is Pi's completed provider-message usage. Input/cache counts and
  all model calls are retained. Costs are catalog estimates, not subscription
  billing. No auth tokens, hidden reasoning or absolute private paths are public.

## What the playback does — and does not show

The runs happened separately and are aligned at t=0 for side-by-side replay.
The displayed query durations are the replay endpoints; DocFoo's outer CLI
startup interval is removed from its timeline. File-tool starts
and durations are actual recorded offsets.

First answer-text offsets are **approximate**: each is reconstructed as
`displayed duration - final model-call duration + first-text latency` (about
2.74 s for DocFoo and 14.91 s for the agent). The interval includes small finalization
work, because absolute model-request starts were not separately timestamped.

Original streaming chunks and individual token timestamps were **not** recorded.
The page therefore reveals the answer uniformly between its approximate first
text and measured completion. This text reveal is illustrative, not a faithful
per-token latency trace or a live query. Actual file-tool events and model-waiting
periods are distinguished; the agent is not shown executing a tool while it was
actually waiting for its next model response.

Complete answers appear on load and reload; scrolling into view does not start
playback or clear them. Query is always visible. Play replay starts the optional
finite 1x playback, which has Pause, Replay and Show complete answers controls.
It pauses off-screen and in a hidden tab. Reduced-motion, print, no-JavaScript
and missing-playback-data views show both complete formatted responses without
animation. The site makes no model calls or network requests during playback.

## Display edits

Only the agent's paragraph-prefix labels were removed: **Baseline.**,
**Longformer.**, **BigBird.**, **FlashAttention.**, and **Recommendation:**.
`agentic-display.md` records this display version. Other wording, emphasis,
paragraphs and equations are retained. DocFoo's answer is unchanged.

Equations are built as native MathML. Citations open exact scanned line windows,
with Markdown-file fallbacks. Displaying an agent citation adds the collection
prefix internally to locate the same scanned file; the original downloadable
answer remains unchanged. These links verify the cited source text's identity,
not whether the model's claim is correctly supported by it.

DocFoo's final sentence claims file/line citations are unavailable. That sentence
is retained as returned; DocFoo expanded its evidence tags into the displayed
file/line citations after generation.

Regeneration uses `tools/comparison_data.py` and `tools/prepare-data.py`. The
private capture harness, library snapshots and credentials are never published.

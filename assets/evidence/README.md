# Sources and recorded queries

The examples use an unchanged snapshot of the existing `Papers` graph and
resources in the local DocFoo library. No source document was synthesized,
rewritten or re-indexed for the page.

## Source provenance

- Graph: `~/.docfoo/graphs/Papers/graph.sqlite`.
- Resources: `~/.docfoo/resources/Papers/`.
- Capture workspace: a read-only SQLite backup and copies under the page's
  private work directory. The original library and its settings are unchanged.
- Snapshot, build ID and resource SHA-256 hashes are in `recorded.json`.
- Public assets include exact cited line windows, selected original image
  crops and query replays, not a mirror of the entire library or its database.

## Query selectors

1. **Figures:** How does FlashAttention use tiling and recomputation to compute
   exact attention? Show its original Figure 1.
2. **Tables:** How do self-attention and recurrent layers compare in sequential
   operations and maximum path length? Include the original comparison table.
3. **Comparison:** How does Longformer's sliding-window attention differ from
   its global attention? Include the original attention-pattern figures.
4. **Ambiguity:** How can the calculation be unchanged if the big square array
   is never kept?

All answer writers use `unsloth/Qwen3.5-4B-GGUF:Q4_K_M` through llama.cpp at
`http://127.0.0.1:8080/v1`. Each capture contains one local completion.
Temperature is 0, thinking is off and the output cap is 2,048 tokens.
The server context is 16,128 tokens; evidence is limited to 18,000 characters.
Source-section omissions and timings are retained in the raw query metadata.
These recordings are examples, not benchmark measurements.

The first three captures explicitly disable hosted decision routing. The fourth
uses OpenRouter System One, `typesafe/jev-1.13`, to select concepts before the
same local model writes the answer. Its selections and probabilities are real
recorded values, not an illustrative route or an answer-accuracy score.

The browser performs no model calls. Switching selectors changes which
pre-rendered recording is visible.

## Response excerpts

Short visible excerpts are selected verbatim from the model output, not
rewritten. Each selector includes the complete rendered response in an
expandable section. Equations are rendered at build time as native MathML from
the unchanged LaTeX expressions; no browser math service is called.
`recorded.json` retains all unedited answer Markdown, citations, tables, figure
references, retrieval metadata and bounded snapshots.
The selected excerpt strings are also included in the small bundled UI data,
so validation can compare them directly with the original answer.

Query 2 returns a Markdown comparison table. The separately linked source crop
is stored evidence for checking that table; it is not misrepresented as a
returned image directive. Query 3 returns the original sliding-window and global
attention image references. Query 4's hosted concept decision is distinguished
from its local answer writer.

## Original assets

All image files are copied byte-for-byte, with hashes in `assets.json`:

- FlashAttention Figure 1 — Dao, Fu, Ermon, Rudra and Ré (2022):
  https://arxiv.org/abs/2205.14135 .
- Attention Is All You Need Table 1 — Vaswani et al. (2017):
  https://arxiv.org/abs/1706.03762 .
- Longformer Figure 2(b) and Figure 2(d) — Beltagy, Peters and Cohan (2020):
  https://arxiv.org/abs/2004.05150 .
- Big Bird Table 1 — Zaheer et al. (2020):
  https://arxiv.org/abs/2007.14062 .

Algorithm 1 is the stored fenced Markdown block at FlashAttention lines 135–155.
`algorithm-read.json` is an actual CLI resource-read result for that window;
only its absolute workspace field is redacted. The algorithm is not rebuilt by
a model for the page. Source-viewer text is copied from original stored line
windows without rewriting. It may contain OCR errors; original PDFs are linked
above for verification.

Source material remains attributable to its authors/rights holders. The site's
MIT license applies to site code, not to the cited documents.

## Reproduction and limits

```sh
python landing-page/tools/capture-demo.py --only flash
python landing-page/tools/capture-demo.py --only transformer
python landing-page/tools/capture-demo.py --only longformer
python landing-page/tools/capture-demo.py --only routing
python landing-page/tools/prepare-data.py
```

The exact local model must be loaded on port 8080. The last capture needs
`OPENROUTER_API_KEY` in the environment. The script does not read credential
files, put secrets in process arguments or save a key to disk. A loopback-only
sampling gateway fixes request settings but never edits responses.

Query wording and context budgets affect retrieval. DocFoo omits whole queued
sections rather than clipping them mid-block, and reports those omissions.
Generated answers and citations still require source review for important
claims. No clinical validation, universal retrieval success or complete OCR
accuracy is claimed.

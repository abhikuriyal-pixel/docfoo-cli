# Retrieval methodology comparison

An architectural comparison of the retrieval paths implemented in the project's
benchmark adapters, covering candidate selection, evidence units and provenance.
No benchmark scores or latency claims are included.

## Feature definitions

- **Embedding-free:** the compared indexing/query configuration does not need
  an embedding model or vector index. GraphRAG global search uses community
  reports at query time; its standard indexed pipeline here still configures
  embedding models. The table describes that pipeline, not a claim that every
  global-search request performs a vector lookup.
- **Whole source sections:** complete stored Markdown sections are selected and
  delivered as evidence. This is different from character chunks, community
  reports or PDF page/tree units. DocFoo can omit whole queued sections to meet
  a context budget; it reports those omissions.
- **Source-line spans:** source file and exact stored section line ranges are
  returned as structured provenance. Page citations, document IDs and report
  labels are useful but are not the same line-span contract.
- **Jev concept routing:** an independent System One decision model scores a
  query against indexed concepts when lexical seeding is weak. This is separate
  from an LLM generating search keywords or writing the final answer.

A check means that feature is built into the compared path. A dash means it is
not used by that path. Each row names its configured retrieval method; feature
checks use the definitions above.

## DocFoo

Term/entity/concept search uses SQLite FTS5/BM25. A rare-token locator can find
source sections directly, and topic guides plus bounded relation traversal
connect relevant concepts. The evidence-delivery stage deduplicates and ranks
complete source sections. Source paths and line spans travel with the query
result. Optional Jev routing is independent of the generative writer.

The result combines technical-term retrieval and source-preserving evidence in
one local workflow, without an embedding service or separate graph server.

Evidence: `benchmarks/frameworks/docfoo_fw.py`,
`crates/docfoo-kg/src/query/seeds.rs`, `query/deliver.rs`, `store/search.rs`,
`crates/docfoo-cli/src/kg/query.rs`, `kg/decision.rs`.

## Microsoft GraphRAG — standard index, local/global search

The adapter builds GraphRAG's standard index with completion and embedding
models. Local search constructs entity/relation/text-unit context. Global
search uses community-report context through its native query API.

**DocFoo difference:** it delivers original stored sections and source-line
provenance instead of assembling the answer's evidence from text units or
community reports.

Evidence: `benchmarks/frameworks/graphrag_fw.py`,
`benchmarks/tools/graphrag_query.py`.

## LightRAG — hybrid, local and global graph modes

The adapter configures an embedding function and the native LightRAG graph
query modes. Native keyword extraction and graph/vector retrieval return
chunks and references under entity/relation/context budgets. LightRAG's naive
mode is a vector-only variant; it is not the graph mode described in the row.

**DocFoo difference:** a direct rare-term section locator, independent concept
decision routing and complete section delivery are part of its query path,
rather than relying on the keyword/vector-selected chunk set as final evidence.

Evidence: `benchmarks/frameworks/lightrag_fw.py`.

## Neo4j GraphRAG — vector/Cypher adapter

The adapter uses the official package's graph builder and chunk embeddings.
VectorCypherRetriever finds vector-similar chunks, then traverses nearby entities
and relations in Neo4j. The retrieved payload is chunk text plus neighborhood
context. This is an architecture comparison of the implemented adapter, not a
runtime result: the local benchmark job did not provision the Neo4j service.

**DocFoo difference:** local SQLite relation retrieval without a vector-first
candidate stage or a separate Neo4j server, plus section-level evidence and
source-line provenance.

Evidence: `benchmarks/frameworks/neo4j_fw.py`.

## PageIndex — native local PDF-tree agent

The adapter creates the native PDF tree index and uses PageIndex's local
tool-calling document-QA agent with page/document citations. It does not use
embedding retrieval in this path. Source discovery happens through agent
selection and reading of PDF tree/page units.

**DocFoo difference:** bounded term/concept/graph retrieval delivers source
sections directly; an iterative PDF-tree browsing agent is not required for
that selection stage. Ambiguity handling can use a separate decision model.

Evidence: `benchmarks/frameworks/pageindex_fw.py`.

## Vector RAG baseline

The adapter embeds the shared character chunks, uses cosine-similarity top-k
retrieval, and sends those chunks to answer synthesis.

**DocFoo difference:** exact-term lookup and graph relationships add named
concept context beyond similarity ranking, and the writer receives complete
selected sections.

Evidence: `benchmarks/frameworks/naive_vector.py`.

## BM25 RAG baseline

The adapter lexically ranks shared character chunks, then sends the top-k
chunk context to answer synthesis. It does not require embeddings.

**DocFoo difference:** relation traversal and probabilistic concept routing
extend lexical search; the delivery unit is a source section rather than a
character chunk.

Evidence: `benchmarks/frameworks/bm25.py`.

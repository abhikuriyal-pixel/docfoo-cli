//! docfoo-kg — DocFoo's knowledge-graph indexer.
//!
//! A Rust port of the kg_demo Graph-RAG *build* path (no embeddings, DD-1):
//! markdown resources are parsed into whole sections, an LLM extracts
//! schema-locked entities/relations per section, everything merges into a
//! provenance-complete graph, and the SQLite store ([`store`]) is the
//! on-disk representation. Incremental sync (DD-16)
//! makes re-indexing cheap: unchanged sections skip, edited re-extract,
//! deleted GC.
//!
//! Module map (kg_demo origin in parentheses):
//! - [`config`]  — build constants (`config.py`, build half)
//! - [`text`]    — stemmer, slug, tokenizer, PRNG (`kg.py` helpers)
//! - [`bm25`]    — Okapi BM25 (`rank_bm25.BM25Okapi`)
//! - [`llm`]     — the ChatClient seam + JSON repair (`llm.py`)
//! - [`decision`] — the typed System One classifier seam (Jev concept routing)
//! - [`extract`] — parsing + extraction prompts/validation (`extractor.py`)
//! - [`graph`]   — storage, searches, noise floor (`kg.py`)
//! - [`build`]     — orchestration: waves, sync, topics, checkpoints (`build.py`)
//! - [`corpus`]    — DocFoo-specific: resources walk + doc tags
//! - [`tunables`]  — every user-tunable knob with kg_demo defaults
//! - [`expand`]    — DD-25 relation-driven query expansion (`expansion.py`)
//! - [`query`]     — retrieval + streaming synthesis (`pipeline.py`)

pub mod build;
pub mod config;
pub mod corpus;
pub mod decision;
pub mod expand;
pub mod extract;
pub mod graph;
pub mod llm;
pub mod query;
pub mod reader;
pub mod store;
#[cfg(test)]
pub mod test_support;
pub mod text;
pub mod tunables;

#[derive(Debug, thiserror::Error)]
pub enum KgError {
    #[error("preflight failed: {0}")]
    Preflight(String),
    #[error("extraction failed: {0}")]
    Extraction(String),
    #[error("LLM call failed: {0}")]
    Llm(#[from] llm::LlmError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store error: {0}")]
    Store(#[from] store::StoreError),
    #[error("indexing was cancelled")]
    Cancelled,
}

impl KgError {
    /// Map an LLM failure: a user cancellation surfaces as the dedicated
    /// cancelled error (the shell reports it as a clean stop), everything
    /// else stays a plain LLM failure.
    pub fn from_llm(e: llm::LlmError) -> Self {
        match e {
            llm::LlmError::Cancelled => KgError::Cancelled,
            other => KgError::Llm(other),
        }
    }
}

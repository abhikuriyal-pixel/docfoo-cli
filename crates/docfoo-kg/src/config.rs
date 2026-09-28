//! Build-path knobs — ported from kg_demo `config.py`.
//!
//! Only the constants the *indexing* (build) path needs live here; query-time
//! knobs (seed pools, hop depths, budgets) join when the retrieval pipeline
//! is ported. Every value matches kg_demo so extraction behaviour is
//! identical to the reference implementation.

/// Closed entity vocabulary (schema-locked at decode time — DD-2).
pub const ENTITY_TYPES: [&str; 8] = [
    "DEVICE",
    "MATERIAL",
    "CONCEPT",
    "PROCESS",
    "PERSON",
    "ORGANIZATION",
    "EVENT",
    "METRIC",
];

/// Closed relation vocabulary (schema-locked at decode time — DD-2).
pub const RELATIONS: [&str; 9] = [
    "CAUSES",
    "ENABLES",
    "PART_OF",
    "USES",
    "TRADES_OFF_WITH",
    "INVENTED_BY",
    "DEVELOPED_BY",
    "COMPETES_WITH",
    "MEASURES",
];

/// Known-entity glossary budget per extraction call: exact name matches are
/// guaranteed inside it, the rest is filled by BM25. Indexing runs on large
/// context models, so a generous list is cheap and prevents duplicates.
pub const GLOSSARY_K: usize = 60;

/// Glossary BM25 fill: the section's rarest tokens seed the FTS candidate
/// query, keeping the scored candidate set small even on 1M-entity graphs.
pub const GLOSSARY_QUERY_TERMS: usize = 12;

/// A seed token whose document frequency exceeds this is too common to narrow
/// the candidate set; a section made only of such tokens skips the BM25 fill
/// (exact matches still apply) instead of scoring the whole corpus.
pub const GLOSSARY_MAX_DF: usize = 2_000;

/// Sections longer than this are split at `###` subheadings, never truncated
/// (DD-5).
pub const MAX_SECTION_CHARS: usize = 6000;

/// Fragments shorter than this are noise and dropped at parse time.
pub const MIN_SECTION_CHARS: usize = 200;

/// Extraction output ceiling per section.
pub const MAX_ENTITIES_PER_SECTION: usize = 12;

/// When a section is list-heavy, only this many load-bearing items.
pub const PRIORITY_ENTITY_COUNT: usize = 8;

/// Answer room for one extraction call. The schema bounds the output (at most
/// 12 entities plus their relations), so this is roughly ten times the
/// realistic answer. On providers where thinking shares the response ceiling,
/// pi clamps the thinking budget to leave at least 1024 answer tokens.
pub const EXTRACT_MAX_TOKENS: u32 = 16_384;

/// Parallel extractions per wave. Indexing runs on cloud models, so this is
/// never clamped for local endpoints.
pub const BUILD_CONCURRENCY: usize = 8;

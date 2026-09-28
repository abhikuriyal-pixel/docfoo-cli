//! Corpus parsing + schema-constrained extraction — ported from kg_demo
//! `extractor.py`.
//!
//! Pipeline: markdown -> sections (`##` headings, noise-skip rules,
//! split-don't-truncate), then one schema-locked JSON extraction call per
//! section with a known-entity glossary. Off-vocabulary labels are made
//! undecodable by the response schema (DD-2); whatever slips past is still
//! validated and counted here.

use crate::llm::{extract_json, ChatClient};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Runtime extraction vocabulary + prompt knobs, resolved from `IndexSettings`
/// once per build. Replaces the compile-time `config::ENTITY_TYPES` /
/// `config::RELATIONS` / entity-count constants at extraction time.
pub struct Vocab {
    pub types: Vec<String>,
    pub relations: Vec<String>,
    pub max_entities: usize,
    pub priority: usize,
    pub extra_prompt: String,
}

impl Vocab {
    pub fn from_index_settings(s: &crate::tunables::IndexSettings) -> Self {
        Vocab {
            types: s.entity_types.clone(),
            relations: s.relations.clone(),
            max_entities: s.max_entities_per_section,
            priority: s.priority_entity_count,
            extra_prompt: s.index_prompt.trim().to_string(),
        }
    }
}

/// Drop a trailing bracketed type marker from an entity name when the group
/// is one of the vocabulary types: `UNIVERSAT [PROCESS]` becomes `UNIVERSAT`.
/// Parentheticals that are not types are preserved, so
/// `Universal Patch Encoder (UPE)` stays intact. Loops so stacked markers
/// (`Name (DEVICE) [DEVICE]`) are removed too.
pub fn strip_type_markers(name: &str, types: &[String]) -> String {
    let re = regex::Regex::new(r"^(.*?)\s*[\[(]\s*([^\[\]()]+?)\s*[\])]\s*$")
        .expect("static regex");
    let mut out = name.trim().to_string();
    loop {
        let Some(caps) = re.captures(&out) else { break };
        let inner = caps[2].trim().to_uppercase();
        if !types.iter().any(|t| t == &inner) {
            break;
        }
        let prefix = caps[1].trim();
        if prefix.is_empty() {
            break; // never strip the whole name
        }
        out = prefix.to_string();
    }
    out
}

/// Titles that never carry substantive technical content.
const SKIP_TITLES: [&str; 13] = [
    "table of contents",
    "contents",
    "see also",
    "references",
    "recommended reading",
    "review questions",
    "abbreviations",
    "course support",
    "supplemental documents",
    "useful web sites",
    "mailing list",
    "simulation tools",
    "designing for performance",
];

const SKIP_EXACT_TITLES: [&str; 2] = ["problems", "index"];

/// One parsed corpus section (pre-extraction).
#[derive(Clone, Debug)]
pub struct Section {
    pub title: String,
    pub text: String,
    /// Chapter number derived from heading numbering (DD-6 deterministic
    /// topics); subsections inherit it.
    pub chapter: Option<String>,
    /// Document this section belongs to (file rel-path basename in DocFoo).
    pub source_doc: String,
    /// 1-based file line of the first content line of this section text.
    pub start_line: usize,
    /// 1-based file line of the last content line of this section text (inclusive).
    pub end_line: usize,
}

/// A known-entity hint handed to an extraction call so names are reused
/// instead of minted twice.
#[derive(Clone, Debug)]
pub struct GlossaryEntry {
    pub name: String,
    pub etype: String,
    pub id: String,
}

/// Validated entity ready for the graph (id is `{TYPE}_{slug}`).
#[derive(Clone, Debug)]
pub struct RawEntity {
    pub id: String,
    pub name: String,
    pub etype: String,
    pub desc: String,
}

/// Validated relation whose endpoints are resolved entity ids.
#[derive(Clone, Debug, PartialEq)]
pub struct RawRelation {
    pub source: String,
    pub target: String,
    pub rel: String,
}

/// One concise glossary concept for the section (exactly one per extracted
/// section): the routing label, a one-sentence summary, and the lexical
/// aliases — including lay synonyms — that let the BM25 prefilter find the
/// concept behind a vague question.
#[derive(Clone, Debug, PartialEq)]
pub struct RawConcept {
    pub name: String,
    pub summary: String,
    pub terms: Vec<String>,
}

/// Caps for the concept fields; the extraction model is asked for terse
/// output and anything longer is trimmed at validation time.
const CONCEPT_NAME_MAX: usize = 120;
const CONCEPT_SUMMARY_MAX: usize = 400;
const CONCEPT_TERMS_MAX: usize = 12;
const CONCEPT_TERM_MAX: usize = 60;

/// Per-call drop accounting (feeds the end-of-build summary).
#[derive(Default, Clone, Debug)]
pub struct ExtractStats {
    pub dropped_entities: u32,
    pub dropped_relations: u32,
    pub bad_type: u32,
    pub empty_name: u32,
    pub off_vocab: u32,
    pub unknown_source: u32,
    pub unknown_target: u32,
}

/// Outcome of one section's extraction. `Extracted` means the model produced a
/// usable payload (empty lists are a valid extraction). `Skipped` means no
/// payload was produced at all; the build path must leave the section un-hashed
/// so the next run retries it instead of recording it as done.
#[derive(Clone, Debug)]
pub enum ExtractOutcome {
    Extracted {
        entities: Vec<RawEntity>,
        relations: Vec<RawRelation>,
        concept: Option<RawConcept>,
        stats: ExtractStats,
    },
    Skipped {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn is_skipped(title: &str) -> bool {
    let low = title.trim().to_lowercase();
    if SKIP_EXACT_TITLES.contains(&low.as_str()) {
        return true;
    }
    SKIP_TITLES.iter().any(|s| low.contains(s))
}

/// Split an oversized section at `###` subheadings (line-chunk fallback),
/// producing parts that never exceed MAX_SECTION_CHARS — content loss is
/// structurally impossible (DD-5).
fn split_long(
    title: &str,
    lines: Vec<(usize, String)>,
    max_chars: usize,
) -> Vec<(String, String, usize, usize)> {
    let max = max_chars;
    let finalize = |piece: Vec<(usize, String)>| -> Option<(String, usize, usize)> {
        // trim blank edges, then span = first/last remaining line numbers
        let mut slice: &[(usize, String)] = &piece[..];
        while let Some(f) = slice.first() {
            if f.1.trim().is_empty() { slice = &slice[1..]; } else { break; }
        }
        while let Some(l) = slice.last() {
            if l.1.trim().is_empty() { slice = &slice[..slice.len() - 1]; } else { break; }
        }
        if slice.is_empty() { return None; }
        let text = slice.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n");
        Some((text, slice.first().unwrap().0, slice.last().unwrap().0))
    };

    let text = lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n");
    if text.len() <= max {
        return finalize(lines)
            .map(|(text, start, end)| vec![(title.to_string(), text, start, end)])
            .unwrap_or_default();
    }
    let sub_re = regex::Regex::new(r"^###\s").expect("static regex");

    // break into blocks at ### boundaries
    let mut blocks: Vec<Vec<(usize, String)>> = vec![];
    let mut cur: Vec<(usize, String)> = vec![];
    for line in &lines {
        if sub_re.is_match(&line.1) && !cur.is_empty() {
            blocks.push(std::mem::take(&mut cur));
        }
        cur.push(line.clone());
    }
    if !cur.is_empty() { blocks.push(cur); }

    let block_len = |b: &[(usize, String)]| b.iter().map(|(_, l)| l.len() + 1).sum::<usize>();
    let mut pieces: Vec<Vec<(usize, String)>> = vec![];
    let mut piece: Vec<(usize, String)> = vec![];
    let mut plen = 0;
    for b in blocks {
        let blen = block_len(&b);
        if blen > max {
            if !piece.is_empty() {
                pieces.push(std::mem::take(&mut piece));
                plen = 0;
            }
            // oversized single block: hard-chunk line-wise (never truncate)
            let mut chunk: Vec<(usize, String)> = vec![];
            let mut clen = 0;
            for line in b {
                if clen >= max {
                    pieces.push(std::mem::take(&mut chunk));
                    clen = 0;
                }
                clen += line.1.len() + 1;
                chunk.push(line);
            }
            if !chunk.is_empty() { pieces.push(chunk); }
            continue;
        }
        if !piece.is_empty() && plen + blen > max {
            pieces.push(std::mem::take(&mut piece));
            plen = 0;
        }
        for line in b { piece.push(line); }
        plen += blen;
    }
    if !piece.is_empty() { pieces.push(piece); }
    if pieces.len() <= 1 {
        return finalize(lines)
            .map(|(text, start, end)| vec![(title.to_string(), text, start, end)])
            .unwrap_or_default();
    }
    let finalized: Vec<(String, usize, usize)> = pieces.into_iter().filter_map(finalize).collect();
    let n = finalized.len();
    finalized.into_iter().enumerate()
        .map(|(i, (text, start, end))| (format!("{title} (part {}/{n})", i + 1), text, start, end))
        .collect()
}

/// Stable content fingerprint for incremental sync (DD-16): first 16 hex
/// chars of sha256 over the exact section text.
pub fn section_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_string()
}

/// Parse a markdown document into sections at `##` headings. Chapter numbers
/// come from heading numbering when present (`8.3 ...`, `Chapter 8`,
/// bare `8. Title` wiki style); unnumbered sections inherit nothing and are
/// grouped by the LLM fallback later.
pub fn parse_sections(md_text: &str, source_doc: &str) -> Vec<Section> {
    let num_dot_num = regex::Regex::new(r"^(\d+)\.\d+").expect("static regex");
    let chapter_word = regex::Regex::new(r"(?i)^Chapter\s+(\d+)").expect("static regex");
    let bare_number = regex::Regex::new(r"^(\d+)\.\s+\S").expect("static regex");
    let heading = regex::Regex::new(r"^##\s+(.+)$").expect("static regex");

    let normalized = md_text.replace("\r\n", "\n");
    let mut sections: Vec<Section> = vec![];
    // (title, accumulated (line number, text) pairs, chapter at heading time)
    let mut current: Option<(String, Vec<(usize, String)>, Option<String>)> = None;

    fn flush(
        current: &Option<(String, Vec<(usize, String)>, Option<String>)>,
        source_doc: &str,
        out: &mut Vec<Section>,
    ) {
        let Some((title, lines, chapter)) = current else { return };
        // drop leading/trailing blank lines so spans match the stored text
        let mut slice: &[(usize, String)] = &lines[..];
        while let Some(first) = slice.first() {
            if first.1.trim().is_empty() { slice = &slice[1..]; } else { break; }
        }
        while let Some(last) = slice.last() {
            if last.1.trim().is_empty() { slice = &slice[..slice.len() - 1]; } else { break; }
        }
        if slice.is_empty() { return; }
        let text = slice.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n");
        if is_skipped(title) || text.chars().count() < crate::config::MIN_SECTION_CHARS {
            return;
        }
        for (piece_title, piece_text, start_line, end_line) in
            split_long(title, slice.to_vec(), crate::config::MAX_SECTION_CHARS)
        {
            out.push(Section {
                title: piece_title,
                text: piece_text,
                chapter: chapter.clone(),
                source_doc: source_doc.to_string(),
                start_line,
                end_line,
            });
        }
    }

    for (idx, line) in normalized.split('\n').enumerate() {
        let line_no = idx + 1;
        if let Some(m) = heading.captures(line) {
            flush(&current, source_doc, &mut sections);
            let title = m[1].trim().to_string();
            let chapter = if let Some(c) = num_dot_num.captures(&title) {
                Some(c[1].to_string())
            } else if let Some(c) = chapter_word.captures(&title) {
                Some(c[1].to_string())
            } else if let Some(c) = bare_number.captures(&title) {
                // bare 'N. Title' numbering (wiki compilations): the article
                // number IS the chapter — deterministic topics without LLM
                // grouping, which overflows on 120+ section documents
                Some(c[1].to_string())
            } else {
                current.as_ref().and_then(|(_, _, c)| c.clone()) // inherit
            };
            current = Some((title, Vec::new(), chapter));
        } else if let Some((_, lines, _)) = current.as_mut() {
            lines.push((line_no, line.to_string()));
        }
    }
    flush(&current, source_doc, &mut sections);
    sections
}

// ---------------------------------------------------------------------------
// Extraction calls
// ---------------------------------------------------------------------------

/// Schema-constrained response format: enum-locking `type` and `relation`
/// makes off-vocabulary labels undecodable tokens (DD-2).
fn extraction_response_format(vocab: &Vocab) -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "extraction",
            "schema": {
                "type": "object",
                "properties": {
                    "concept": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "summary": {"type": "string"},
                            "terms": {
                                "type": "array",
                                "items": {"type": "string"},
                                "maxItems": CONCEPT_TERMS_MAX,
                            },
                        },
                        "required": ["name", "summary", "terms"],
                        "additionalProperties": false,
                    },
                    "entities": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "type": {"type": "string", "enum": vocab.types},
                                "description": {"type": "string"},
                            },
                            "required": ["name", "type", "description"],
                            "additionalProperties": false,
                        },
                    },
                    "relations": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "source": {"type": "string"},
                                "relation": {"type": "string", "enum": vocab.relations},
                                "target": {"type": "string"},
                            },
                            "required": ["source", "relation", "target"],
                            "additionalProperties": false,
                        },
                    },
                },
                "required": ["concept", "entities", "relations"],
                "additionalProperties": false,
            },
        },
    })
}

fn prompt_body(known_block: &str, vocab: &Vocab) -> String {
    format!(
        "Extract key technical entities and relationships from the encyclopedia section below.\n\
{known_block}\n\
Rules:\n\
- SOURCE HYGIENE: the text may contain publisher/copyright boilerplate, course-supplement\n\
  advertisements, table-of-contents listings, page-number artifacts, garbled PDF characters\n\
  (e.g. \"PearsonAr\", \"EducaciA3n\"), figure/table reference numbers, review questions, and\n\
  problem statements. IGNORE ALL OF IT. Extract only substantive technical content:\n\
  concepts, structures, mechanisms, definitions, performance trade-offs, and their\n\
  cause-effect relationships. If a section is purely noise, return empty lists.\n\
- Extract AT MOST {max_entities} entities in TOTAL for this section. Never exceed this number.\n\
- If the section is long or list-heavy, extract only the {priority} most load-bearing items.\n\
- Each entity: {{\"name\": \"...\", \"type\": \"...\", \"description\": \"max ONE sentence\"}}. \
type must be exactly one of: {types}\n\
- Write the entity name alone. Never append the type or a bracketed type marker to the name.\n\
- Include important named phenomena, laws and theorems, operating conditions (stall, saturation, \
back-EMF, resonance, cogging, Ohm's law, De Morgan's laws...) as CONCEPT when explicitly \
discussed — they count toward the total.\n\
- Wire cause-and-effect chains between the extracted items: state what causes what and what \
enables what (e.g., stall CAUSES high current which ENABLES the overload protection to trip) \
using CAUSES, ENABLES or MEASURES.\n\
- Relations: pairs among those entities. Each: {{\"source\": \"...\", \"target\": \"...\", \"relation\": \"...\"}}.\n\
- relation must be exactly one of: {rels}\n\
- source/target names MUST be copied character-for-character from your entity names above.\n\
- If an entity you would extract matches one of the KNOWN ENTITIES above, reuse its exact name and type.\n\
- You MAY create relations between known entities and/or new entities freely; known-entity \
references are valid.\n\
- Also write ONE concise glossary concept covering what the WHOLE section is about:\n\
  {{\"name\": \"a few words\", \"summary\": \"ONE short sentence\", \"terms\": [\"up to 12 short search terms\"]}}.\n\
  The concept is the section's routing label, not a single entity: summarize the section's\n\
  subject. In \"terms\" include the EXACT names of the section's key mechanisms, methods and\n\
  entities as written in the text (e.g. \"sequential dimension collapse\", \"axial cross-attention\"),\n\
  plus the words a NON-EXPERT would type when searching for it (lay synonyms, everyday names,\n\
  symptom words) alongside the technical ones.\n\
- Stop generating as soon as the JSON object is complete. Do not add anything after the closing brace.\n\
Output ONLY raw JSON, no markdown, no commentary:\n\
{{\"concept\": {{\"name\": \"...\", \"summary\": \"...\", \"terms\": [\"...\"]}}, \"entities\": [...], \"relations\": [...]}}",
        known_block = known_block,
        max_entities = vocab.max_entities,
        priority = vocab.priority,
        types = vocab.types.join(", "),
        rels = vocab.relations.join(", "),
    )
}

/// The built-in extraction prompt with the runtime vocabulary substituted —
/// shown read-only in the Build KG popup (no glossary block, no extra
/// instructions: those are dynamic, added at call time).
pub fn builtin_prompt(vocab: &Vocab) -> String {
    prompt_body("", vocab)
}

fn build_messages(title: &str, text: &str, glossary: &[GlossaryEntry], vocab: &Vocab) -> Vec<Value> {
    let known_block = if glossary.is_empty() {
        String::new()
    } else {
        let lines: String = glossary
            .iter()
            .map(|e| format!("- {} (type: {})", e.name, e.etype))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\nKNOWN ENTITIES ALREADY IN THE KNOWLEDGE BASE \
             (reuse these EXACT names when the text refers to them):\n{lines}\n\n"
        )
    };
    let prompt = prompt_body(&known_block, vocab);
    let prompt = if vocab.extra_prompt.is_empty() {
        prompt
    } else {
        format!(
            "{prompt}\n\nADDITIONAL INSTRUCTIONS (from the user; follow these alongside the rules above):\n{}\n",
            vocab.extra_prompt
        )
    };
    vec![
        json!({"role": "system", "content": "You output only raw JSON. No prose."}),
        json!({"role": "user",
               "content": prompt + "\n\nSECTION TITLE: " + title + "\n\nSECTION TEXT:\n" + text}),
    ]
}

/// Internal result of one raw call: a parsed payload, or a skip with the
/// reason.
enum OnceOutcome {
    Parsed(Value),
    Skipped { reason: String },
}

/// First 80 characters of a reply, for skip reasons the user will read.
fn reply_snippet(raw: &str) -> String {
    let trimmed = raw.trim();
    let snippet: String = trimmed.chars().take(80).collect();
    if trimmed.chars().count() > 80 {
        format!("{snippet}...")
    } else {
        snippet
    }
}

/// One extraction call. Cloud models with a schema-locked output do not need a
/// parse retry: an unusable reply is a skipped section, and the next run
/// retries it.
fn extract_once(
    llm: &dyn ChatClient,
    title: &str,
    text: &str,
    glossary: &[GlossaryEntry],
    max_tokens: u32,
    vocab: &Vocab,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<OnceOutcome, crate::KgError> {
    let messages = build_messages(title, text, glossary, vocab);
    let rf = extraction_response_format(vocab);
    let raw = match llm.chat(&messages, max_tokens, Some(rf), cancel) {
        Ok(raw) => raw,
        // Reasoning modes can return no content; the section is skipped,
        // never a fatal error.
        Err(crate::llm::LlmError::EmptyResponse(message)) => {
            return Ok(OnceOutcome::Skipped {
                reason: format!("the model returned no content ({message})"),
            });
        }
        Err(e) => return Err(crate::KgError::from_llm(e)),
    };
    if raw.is_empty() {
        return Ok(OnceOutcome::Skipped {
            reason: "the model returned an empty reply".to_string(),
        });
    }
    match extract_json(&raw) {
        Some(v) if v.is_object() => Ok(OnceOutcome::Parsed(v)),
        _ => Ok(OnceOutcome::Skipped {
            reason: format!("the model reply was not valid JSON ({})", reply_snippet(&raw)),
        }),
    }
}

// ---------------------------------------------------------------------------
// Payload validation + resolution
// ---------------------------------------------------------------------------

/// Case-insensitive name map + slug map -> existing global entity ids.
fn glossary_maps(glossary: &[GlossaryEntry]) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut by_name = HashMap::new();
    let mut by_slug = HashMap::new();
    for e in glossary {
        if e.id.is_empty() || e.name.trim().is_empty() {
            continue;
        }
        by_name.entry(e.name.trim().to_lowercase()).or_insert_with(|| e.id.clone());
        by_slug.entry(crate::text::canonical_slug(e.name.trim())).or_insert_with(|| e.id.clone());
    }
    (by_name, by_slug)
}

/// Resolve a relation endpoint against this section's local entities first,
/// then the known-entity glossary; a glossary hit yields the EXISTING global
/// entity id (no duplicate node minted).
fn resolve_endpoint(
    raw: Option<&Value>,
    types: &[String],
    local_by_name: &HashMap<String, String>,
    local_by_slug: &HashMap<String, String>,
    glos_by_name: &HashMap<String, String>,
    glos_by_slug: &HashMap<String, String>,
) -> Option<String> {
    let text = raw?.as_str()?.trim();
    let stripped = strip_type_markers(text, types);
    let name = if stripped.is_empty() { text } else { stripped.as_str() };
    resolve_name(name, local_by_name, local_by_slug, glos_by_name, glos_by_slug)
}

fn resolve_name(
    name: &str,
    local_by_name: &HashMap<String, String>,
    local_by_slug: &HashMap<String, String>,
    glos_by_name: &HashMap<String, String>,
    glos_by_slug: &HashMap<String, String>,
) -> Option<String> {
    let low = name.trim().to_lowercase();
    if low.is_empty() {
        return None;
    }
    if let Some(hit) = local_by_name.get(&low) {
        return Some(hit.clone());
    }
    let slug = crate::text::canonical_slug(&low);
    if let Some(hit) = local_by_slug.get(&slug) {
        return Some(hit.clone());
    }
    if let Some(hit) = glos_by_name.get(&low) {
        return Some(hit.clone());
    }
    glos_by_slug.get(&slug).cloned()
}

/// Validate the payload's section concept. A missing or nameless concept is
/// `None` (the graph falls back to the section title); overlong fields are
/// trimmed rather than dropped so routing still has something to work with.
fn validate_concept(raw: Option<&Value>) -> Option<RawConcept> {
    let obj = raw?.as_object()?;
    let name = obj.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let summary = obj.get("summary").and_then(Value::as_str).unwrap_or_default().trim();
    let terms: Vec<String> = obj.get("terms").and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(CONCEPT_TERM_MAX).collect::<String>())
        .take(CONCEPT_TERMS_MAX)
        .collect();
    Some(RawConcept {
        name: name.chars().take(CONCEPT_NAME_MAX).collect(),
        summary: summary.chars().take(CONCEPT_SUMMARY_MAX).collect(),
        terms,
    })
}

/// Validate one extraction payload -> (concept, entities deduped by slug
/// within it, resolved relations).
fn parse_payload(
    data: &Value,
    stats: &mut ExtractStats,
    glossary: &[GlossaryEntry],
    vocab: &Vocab,
) -> (Option<RawConcept>, Vec<RawEntity>, Vec<RawRelation>) {
    let mut entities: Vec<RawEntity> = vec![];
    let mut index_by_slug: HashMap<String, usize> = HashMap::new();

    for e in data.get("entities").and_then(Value::as_array).into_iter().flatten() {
        if !e.is_object() {
            stats.dropped_entities += 1;
            stats.bad_type += 1;
            continue;
        }
        let raw_name = e["name"].as_str().unwrap_or_default().trim();
        let stripped = strip_type_markers(raw_name, &vocab.types);
        let name = if stripped.is_empty() { raw_name.to_string() } else { stripped };
        let etype = e["type"].as_str().unwrap_or_default().trim().to_uppercase();
        let desc = e["description"].as_str().unwrap_or_default().trim().to_string();
        if name.is_empty() {
            stats.dropped_entities += 1;
            stats.empty_name += 1;
            continue;
        }
        if !vocab.types.iter().any(|t| t == &etype) {
            stats.dropped_entities += 1;
            stats.bad_type += 1;
            continue;
        }
        let id = format!("{etype}_{}", crate::text::canonical_slug(&name));
        match index_by_slug.get(&id) {
            Some(&i) => {
                if desc.len() > entities[i].desc.len() {
                    entities[i].desc = desc;
                }
            }
            None => {
                index_by_slug.insert(id.clone(), entities.len());
                entities.push(RawEntity {
                    id,
                    name,
                    etype,
                    desc,
                });
            }
        }
    }

    let (glos_by_name, glos_by_slug) = glossary_maps(glossary);
    let mut local_by_name: HashMap<String, String> = HashMap::new();
    let mut local_by_slug: HashMap<String, String> = HashMap::new();
    for ent in &entities {
        local_by_name.entry(ent.name.to_lowercase()).or_insert_with(|| ent.id.clone());
        local_by_slug.entry(crate::text::canonical_slug(&ent.name)).or_insert_with(|| ent.id.clone());
    }

    let mut relations = vec![];
    for r in data.get("relations").and_then(Value::as_array).into_iter().flatten() {
        if !r.is_object() {
            stats.dropped_relations += 1;
            stats.off_vocab += 1;
            continue;
        }
        let rel = r["relation"].as_str().unwrap_or_default().trim().to_uppercase();
        if !vocab.relations.iter().any(|r| r == &rel) {
            stats.dropped_relations += 1;
            stats.off_vocab += 1;
            continue;
        }
        let src = resolve_endpoint(
            r.get("source"),
            &vocab.types,
            &local_by_name,
            &local_by_slug,
            &glos_by_name,
            &glos_by_slug,
        );
        if src.is_none() {
            stats.dropped_relations += 1;
            stats.unknown_source += 1;
            continue;
        }
        let tgt = resolve_endpoint(
            r.get("target"),
            &vocab.types,
            &local_by_name,
            &local_by_slug,
            &glos_by_name,
            &glos_by_slug,
        );
        if tgt.is_none() {
            stats.dropped_relations += 1;
            stats.unknown_target += 1;
            continue;
        }
        relations.push(RawRelation {
            source: src.unwrap(),
            target: tgt.unwrap(),
            rel,
        });
    }
    (validate_concept(data.get("concept")), entities, relations)
}

/// Full extraction path for one section: one call, one payload, one outcome.
pub fn extract_section(
    llm: &dyn ChatClient,
    title: &str,
    text: &str,
    glossary: &[GlossaryEntry],
    max_tokens: u32,
    vocab: &Vocab,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ExtractOutcome, crate::KgError> {
    let mut stats = ExtractStats::default();
    match extract_once(llm, title, text, glossary, max_tokens, vocab, cancel)? {
        OnceOutcome::Parsed(data) => {
            let (concept, entities, relations) =
                parse_payload(&data, &mut stats, glossary, vocab);
            Ok(ExtractOutcome::Extracted { entities, relations, concept, stats })
        }
        OnceOutcome::Skipped { reason } => Ok(ExtractOutcome::Skipped { reason }),
    }
}

// ---------------------------------------------------------------------------
// Topic grouping fallback (LLM, unnumbered titles only — DD-6)
// ---------------------------------------------------------------------------

const FALLBACK_RULES: [(&str, [&str; 6]); 5] = [
    (
        "History & Industry",
        ["history", "industry", "", "", "", ""],
    ),
    (
        "Physics & Materials",
        ["physics", "semiconductor", "material", "electron", "charge", ""],
    ),
    (
        "Circuits & Signals",
        ["circuit", "logic", "digital", "analog", "gate", "boolean"],
    ),
    (
        "Chips & Boards",
        ["ic ", "integrated circuit", "microprocessor", "pcb", "packaging", "printed circuit"],
    ),
    (
        "Applied Electronics",
        ["sensor", "opto", "power", "thermal", "noise", "emerging"],
    ),
];

fn fallback_group(titles: &[String]) -> HashMap<String, String> {
    titles
        .iter()
        .map(|t| {
            let low = t.to_lowercase();
            let topic = FALLBACK_RULES
                .iter()
                .find(|(_, kws)| kws.iter().any(|k| !k.is_empty() && low.contains(k)))
                .map(|(name, _)| name.to_string())
                .unwrap_or_else(|| "Other".to_string());
            (t.clone(), topic)
        })
        .collect()
}

fn canon_topic(name: &str) -> String {
    let re = regex::Regex::new(r"[^a-z0-9]+").expect("static regex");
    re.replace_all(&name.to_lowercase(), " ").trim().to_string()
}

/// Group loose (unnumbered) section titles into 6-10 broad topics via one
/// LLM call; leftovers fall to word-overlap scoring, then keyword rules.
pub fn group_topics(
    llm: &dyn ChatClient,
    titles: Vec<String>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<HashMap<String, String>, crate::KgError> {
    let listing: String = titles
        .iter()
        .map(|t| format!("- {t}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Group these encyclopedia section titles into 6-10 broad topics.\n\n\
Titles:\n{listing}\n\n\
Every title must be assigned to exactly one topic using its EXACT title text.\n\
Output ONLY raw JSON:\n{{\"topics\": [{{\"name\": \"Topic Name\", \"sections\": [\"exact title\", ...]}}]}}"
    );
    let raw = match llm.chat(
        &[json!({"role": "user", "content": prompt})],
        2000,
        None,
        cancel,
    ) {
        Ok(raw) => raw,
        // An empty reply (reasoning modes, transient server hiccups) still
        // leaves the rule-based fallback below usable.
        Err(crate::llm::LlmError::EmptyResponse(_)) => String::new(),
        Err(e) => return Err(crate::KgError::from_llm(e)),
    };
    if raw.is_empty() {
        // Reasoning modes may return no content under a tight token budget;
        // the rule-based fallback below handles ungrouped titles.
        return Ok(HashMap::new());
    }
    let data = extract_json(&raw);

    let mut mapping: HashMap<String, String> = HashMap::new();
    // canonical display name per normalized key, first occurrence wins
    // (insertion-ordered so downstream tie-breaks stay deterministic)
    let mut canon: HashMap<String, String> = HashMap::new();
    let mut topics_seen: Vec<String> = vec![];
    let mut normalize = |name: &str| -> String {
        let key = {
            let c = canon_topic(name);
            if c.is_empty() { "other".to_string() } else { c }
        };
        if let Some(existing) = canon.get(&key) {
            return existing.clone();
        }
        let display = name.trim().to_string();
        canon.insert(key, display.clone());
        topics_seen.push(display.clone());
        display
    };

    if let Some(data) = data.filter(|d| d.is_object()) {
        for tp in data.get("topics").and_then(Value::as_array).into_iter().flatten() {
            if !tp.is_object() {
                continue;
            }
            let name_raw = tp["name"].as_str().unwrap_or_default().trim();
            let name = normalize(if name_raw.is_empty() { "Other" } else { name_raw });
            for s in tp.get("sections").and_then(Value::as_array).into_iter().flatten() {
                if let Some(t) = s.as_str() {
                    let t = t.trim();
                    if titles.iter().any(|x| x == t) && !mapping.contains_key(t) {
                        mapping.insert(t.to_string(), name.clone());
                    }
                }
            }
        }
    }

    if mapping.len() < titles.len() {
        // leftovers: score by topic-word overlap, then keyword rules
        let leftovers: Vec<String> = titles
            .iter()
            .filter(|t| !mapping.contains_key(t.as_str()))
            .cloned()
            .collect();
        let kw_map = fallback_group(&leftovers);
        let word_re = regex::Regex::new(r"[a-z0-9]+").expect("static regex");
        for t in leftovers {
            let low = t.to_lowercase();
            let mut best: Option<String> = None;
            let mut best_score = -1i64;
            for tp in &topics_seen {
                let score: i64 = word_re
                    .find_iter(&tp.to_lowercase())
                    .map(|m| m.as_str())
                    .filter(|w| low.contains(w))
                    .count() as i64;
                if score > best_score {
                    best_score = score;
                    best = Some(tp.clone());
                }
            }
            let topic = match best {
                Some(tp) if best_score > 0 => tp,
                _ => kw_map.get(&t).cloned().unwrap_or_else(|| "Other".to_string()),
            };
            mapping.insert(t, topic.clone());
            if !topics_seen.contains(&topic) {
                topics_seen.push(topic);
            }
        }
    }
    Ok(mapping)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\
# Book

## 8.3 Cache Memory
Cache memory reduces average access time.
This paragraph discusses associativity and replacement policies at length.
More filler text follows to push the section over the two hundred character
minimum so that it survives the parse filter. Padding padding padding.

## contents
junk

## Problems
junk

## Short
tiny

## 9. Pipelining
Pipelines overlap instruction execution to improve throughput dramatically,
letting several instructions occupy different stages of the machine at once.
Deeper pipelines raise clock speeds but worsen branch mispredictions because
a flushed pipeline throws away far more in-flight work than a shallow one.

### 9.1 Hazards
Structural hazards stall the pipeline when hardware resources collide and
the dispatcher has nowhere to place a new instruction during that cycle.

## Appendix Notes
Loose unnumbered closing notes inherit the running chapter context when no
new heading number appears before them, which keeps appendix material tied
to the chapter it annotates instead of being dumped into a junk bucket.";

    #[test]
    fn parse_sections_skips_noise_and_short_fragments() {
        let secs = parse_sections(DOC, "book.md");
        let titles: Vec<&str> = secs.iter().map(|s| s.title.as_str()).collect();
        assert!(titles.contains(&"8.3 Cache Memory"));
        assert!(titles.contains(&"9. Pipelining"));
        assert!(titles.contains(&"Appendix Notes"));
        assert!(!titles.contains(&"contents"), "skip-title substring must drop");
        assert!(!titles.contains(&"Problems"), "exact skip title must drop");
        assert!(!titles.contains(&"Short"), "<200 chars must drop");

        let cache = secs.iter().find(|s| s.title == "8.3 Cache Memory").unwrap();
        // heading at line 2, blank line dropped -> first content line is 4
        assert_eq!(cache.start_line, 4);
        assert!(cache.end_line >= cache.start_line);
        let pipelining = secs.iter().find(|s| s.title == "9. Pipelining").unwrap();
        assert_eq!(pipelining.start_line, 19);
        assert!(pipelining.end_line < 28);
        let appendix = secs.iter().find(|s| s.title == "Appendix Notes").unwrap();
        assert_eq!(appendix.start_line, 29);
    }

    #[test]
    fn chapters_inherit_downward() {
        let secs = parse_sections(DOC, "book.md");
        let pipelining = secs.iter().find(|s| s.title == "9. Pipelining").unwrap();
        assert_eq!(pipelining.chapter.as_deref(), Some("9"));
        // ### subsections stay inside the parent's text (split_long uses
        // them only when a section grows oversized)
        assert!(pipelining.text.contains("Structural hazards"));
        // unnumbered sections inherit the running chapter number
        let appendix = secs.iter().find(|s| s.title == "Appendix Notes").unwrap();
        assert_eq!(appendix.chapter.as_deref(), Some("9"));
        let cache = secs.iter().find(|s| s.title == "8.3 Cache Memory").unwrap();
        assert_eq!(cache.chapter.as_deref(), Some("8"));
    }

    #[test]
    fn section_hash_is_stable_sha256_prefix() {
        assert_eq!(section_hash("hello"), "2cf24dba5fb0a30e");
        assert_ne!(section_hash("hello"), section_hash("hellp"));
    }

    #[test]
    fn split_long_breaks_at_subheadings_without_loss() {
        let body = "intro line\n".repeat(300) + "### sub heading\n" + &"detail line\n".repeat(300);
        let lines: Vec<(usize, String)> = body
            .lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.to_string()))
            .collect();
        let parts = split_long("T", lines, crate::config::MAX_SECTION_CHARS);
        assert!(parts.len() >= 2, "oversized section must split");
        assert!(parts.iter().all(|(t, _, _, _)| t.starts_with("T (part ")));
        // no content lost: every input line survives in exactly one part
        let mut collected: Vec<String> = parts
            .iter()
            .flat_map(|(_, x, _, _)| x.lines().map(str::to_string))
            .collect();
        let mut original: Vec<String> = body.lines().map(str::to_string).collect();
        collected.sort();
        original.sort();
        assert_eq!(collected, original);
        for (_, text, start_line, end_line) in &parts {
            assert!(*start_line <= *end_line);
            assert!(
                text.len() <= crate::config::MAX_SECTION_CHARS + 80,
                "parts must stay near the size cap"
            );
        }
        assert_eq!(parts.first().unwrap().2, 1);
        assert_eq!(parts.last().unwrap().3, body.lines().count());
    }

    #[test]
    fn split_long_parts_carry_disjoint_line_spans() {
        let body = "intro line\n".repeat(300) + "### sub heading\n" + &"detail line\n".repeat(300);
        let lines: Vec<(usize, String)> = body
            .lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.to_string()))
            .collect();
        let parts = split_long("T", lines, crate::config::MAX_SECTION_CHARS);
        assert!(parts.len() >= 2);
        for pair in parts.windows(2) {
            assert!(pair[0].2 <= pair[0].3);
            assert!(pair[0].3 < pair[1].2);
        }
    }

    #[test]
    fn parse_payload_validates_dedups_and_resolves() {
        let data = json!({
            "entities": [
                {"name": "Cache", "type": "CONCEPT", "description": "fast memory"},
                {"name": "cache", "type": "CONCEPT", "description": "faster memory — dup slug"},
                {"name": "Bad Thing", "type": "WIDGET", "description": "off vocab"},
                {"name": "", "type": "CONCEPT", "description": "empty"},
                {"name": "CPU", "type": "DEVICE", "description": "processor"}
            ],
            "relations": [
                {"source": "CPU", "relation": "USES", "target": "Cache"},
                {"source": "Cache", "relation": "LOVES", "target": "CPU"},
                {"source": "Ghost", "relation": "USES", "target": "CPU"},
                {"source": "Known", "relation": "USES", "target": "Cache"}
            ]
        });
        let glossary = vec![GlossaryEntry {
            name: "Known".into(),
            etype: "CONCEPT".into(),
            id: "CONCEPT_known_entity".into(),
        }];
        let mut stats = ExtractStats::default();
        let vocab = Vocab::from_index_settings(&crate::tunables::IndexSettings::default());
        let (concept, ents, rels) = parse_payload(&data, &mut stats, &glossary, &vocab);
        assert!(concept.is_none(), "no concept in the payload -> None (title fallback)");

        assert_eq!(ents.len(), 2, "dup slugs merge, invalid types drop");
        assert_eq!(ents[0].desc, "faster memory — dup slug", "longer description wins");
        assert_eq!(stats.bad_type, 1);
        assert_eq!(stats.empty_name, 1);

        assert_eq!(rels.len(), 2, "off-vocab and unknown-source drop");
        assert_eq!(stats.off_vocab, 1);
        assert_eq!(stats.unknown_source, 1);
        assert!(rels.contains(&RawRelation {
            source: "DEVICE_cpu".into(),
            target: "CONCEPT_cache".into(),
            rel: "USES".into(),
        }));
        assert!(rels.contains(&RawRelation {
            source: "CONCEPT_known_entity".into(),
            target: "CONCEPT_cache".into(),
            rel: "USES".into(),
        }), "glossary endpoint resolves to existing global id");
    }

    #[test]
    fn concept_validates_trims_and_accepts_missing() {
        let data = json!({
            "concept": {
                "name": "Power dissipation",
                "summary": "Heat generated by switching losses in power regulators",
                "terms": ["overheating", "gets hot", "", "thermal losses"],
            },
            "entities": [],
            "relations": [],
        });
        let mut stats = ExtractStats::default();
        let vocab = Vocab::from_index_settings(&crate::tunables::IndexSettings::default());
        let (concept, _, _) = parse_payload(&data, &mut stats, &[], &vocab);
        let concept = concept.expect("valid concept");
        assert_eq!(concept.name, "Power dissipation");
        assert_eq!(concept.terms, vec!["overheating", "gets hot", "thermal losses"]);

        // Nameless concepts are None — the graph falls back to the title.
        let bad = json!({"concept": {"name": "  ", "summary": "x", "terms": []}});
        assert!(validate_concept(bad.get("concept")).is_none());
        assert!(validate_concept(None).is_none());
    }

    #[test]
    fn type_markers_strip_only_vocabulary_types() {
        let types: Vec<String> = crate::config::ENTITY_TYPES.iter().map(|s| s.to_string()).collect();
        assert_eq!(strip_type_markers("UNIVERSAT [PROCESS]", &types), "UNIVERSAT");
        assert_eq!(strip_type_markers("UNIVERSAT (PROCESS)", &types), "UNIVERSAT");
        assert_eq!(
            strip_type_markers("Vision Transformer (ViT) [CONCEPT]", &types),
            "Vision Transformer (ViT)"
        );
        assert_eq!(
            strip_type_markers("Universal Patch Encoder (UPE)", &types),
            "Universal Patch Encoder (UPE)"
        );
        assert_eq!(strip_type_markers("[CONCEPT]", &types), "[CONCEPT]");
    }

    #[test]
    fn identity_canonicalizes_markers_and_plurals() {
        let data = json!({
            "entities": [
                {"name": "UNIVERSAT [PROCESS]", "type": "PROCESS", "description": "d1"},
                {"name": "Vision Transformers", "type": "CONCEPT", "description": "d2"},
                {"name": "Vision Transformer", "type": "CONCEPT", "description": "d3 is longer"}
            ],
            "relations": [
                {"source": "UNIVERSAT [PROCESS]", "relation": "USES", "target": "Vision Transformer"}
            ]
        });
        let mut stats = ExtractStats::default();
        let vocab = Vocab::from_index_settings(&crate::tunables::IndexSettings::default());
        let (_concept, ents, rels) = parse_payload(&data, &mut stats, &[], &vocab);

        assert!(ents.iter().any(|e| e.id == "PROCESS_universat"));
        let vision: Vec<&RawEntity> =
            ents.iter().filter(|e| e.id == "CONCEPT_vision_transformer").collect();
        assert_eq!(vision.len(), 1, "plural and singular collapse to one entity");
        assert_eq!(vision[0].desc, "d3 is longer");
        assert_eq!(rels[0].source, "PROCESS_universat");
        assert_eq!(rels[0].target, "CONCEPT_vision_transformer");
    }
}

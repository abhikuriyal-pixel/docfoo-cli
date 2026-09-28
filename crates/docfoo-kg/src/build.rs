//! Build orchestration — native SQLite rewrite of kg_demo `build.py`.
//!
//! Flow: preflight parse -> checkpoint load / fresh backup -> indexed sync
//! diff (DD-16) -> wave-batched parallel extraction whose results are written
//! straight into the store (one transaction per section, live FTS glossary)
//! -> topic derivation (deterministic chapters + per-document LLM grouping,
//! DD-6) -> noise-floor measurement (DD-17) -> finalize (BM25 stats, counts,
//! build uid, `derived_ready`). No whole-graph hydration, no per-wave BM25
//! refits, no final full rewrite.

use crate::extract::{self, GlossaryEntry, Section};
use crate::llm::ChatClient;
use crate::store::{KgStore, OpenMode, SectionInput};
use crate::KgError;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// One indexable document: markdown content plus its identity.
pub struct InputDoc {
    /// Rel-path under the resources root, e.g. `multi_column_1/content.md`.
    pub name: String,
    /// Short uppercase tag used in section titles/topics (`[MC1] 8.3 …`).
    pub tag: String,
    pub markdown: String,
    /// SHA-256 of the raw file bytes at load time; stored in the graph so a
    /// later check can warn when sources changed after the build.
    pub source_hash: String,
}

#[derive(Clone)]
pub struct IndexOptions {
    pub llm: std::sync::Arc<dyn ChatClient>,
    /// Parallel extractions per wave.
    pub workers: usize,
    /// Ignore any prior graph and start clean (backs the old one up first).
    pub fresh: bool,
    /// Where graph.sqlite is written.
    pub graph_path: PathBuf,
    /// Graph-shaping settings (vocabulary, prompt, entity budget).
    pub settings: crate::tunables::IndexSettings,
}

/// Progress reported to the caller (the shell turns these into UI events).
pub enum Progress {
    Started { docs: usize, total_sections: usize },
    Sync { added: usize, changed: usize, removed: usize, unchanged: usize },
    SectionDone { done: usize, total: usize, title: String, entities: usize, relations: usize },
    Warn(String),
    Phase(String),
}

#[derive(Debug, Default)]
pub struct IndexStats {
    pub entities: usize,
    pub relations: usize,
    pub sections: usize,
    pub topics: usize,
    pub docs: usize,
    pub extracted_this_run: usize,
    pub skipped_resume: usize,
    pub cross_doc_merges: usize,
    /// Entities merged into an existing slug because the model chose a
    /// different type for the same name.
    pub type_merges: usize,
    pub noise_floor: Option<f64>,
    pub elapsed_secs: f64,
}

// ---------------------------------------------------------------------------
// Incremental sync (DD-16)
// ---------------------------------------------------------------------------

/// Migrate pre-DD-16 bare `Chapter N` topics to `[TAG] Chapter N` using the
/// tag embedded in the section title; anything else passes through.
fn migrate_topic(title: &str, topic: Option<&str>) -> Option<String> {
    let topic = topic?;
    let bare_chapter = regex::Regex::new(r"^Chapter\s+(\d+)$").expect("static regex");
    let Some(caps) = bare_chapter.captures(topic.trim()) else {
        return Some(topic.to_string());
    };
    let tag_re = regex::Regex::new(r"^\[([^\]]+)\]").expect("static regex");
    match tag_re.captures(title) {
        Some(t) => Some(format!("[{}] Chapter {}", &t[1], &caps[1])),
        None => Some(topic.to_string()),
    }
}

/// What changed between the stored graph and the freshly parsed corpus.
struct SyncOutcome {
    added: Vec<Section>,
    changed: Vec<Section>,            // re-extract
    removed: Vec<(String, String)>,   // (title, source_doc) to GC
    unchanged: Vec<(String, String)>, // (doc, title)
    backfilled: usize,                // legacy sections adopted hash-less
}

/// Native sync diff (DD-16): compare the parsed corpus against stored section
/// hashes with two indexed queries instead of hydrating the model.
fn plan_sync(
    store: &KgStore,
    parsed: &[Section],
    scope_docs: &BTreeSet<String>,
) -> Result<SyncOutcome, KgError> {
    let stored = store.sections_in_scope(scope_docs)?;
    let by_key: HashMap<(&str, &str), &Section> = parsed
        .iter()
        .map(|s| ((s.source_doc.as_str(), s.title.as_str()), s))
        .collect();

    let mut out = SyncOutcome {
        added: vec![],
        changed: vec![],
        removed: vec![],
        unchanged: vec![],
        backfilled: 0,
    };
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for row in stored {
        let Some(p) = by_key.get(&(row.source_doc.as_str(), row.title.as_str())) else {
            out.removed.push((row.title, row.source_doc));
            continue;
        };
        seen.insert((row.source_doc.clone(), row.title.clone()));
        let want = extract::section_hash(&p.text);
        if row.content_hash == want {
            out.unchanged.push((row.source_doc, row.title));
        } else if row.content_hash.is_empty() && !row.retry_pending {
            store.update_section_hash(&row.title, &want)?; // legacy backfill
            out.unchanged.push((row.source_doc, row.title));
            out.backfilled += 1;
        } else {
            out.changed.push((*p).clone());
        }
    }
    for s in parsed {
        if !seen.contains(&(s.source_doc.clone(), s.title.clone())) {
            out.added.push(s.clone());
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Extraction waves
// ---------------------------------------------------------------------------

/// Outcome of one section's extraction work. Payload-level garbage is
/// repaired or dropped (empty result); a transport/format/auth failure sets
/// `fatal` instead, because that is a property of the endpoint, not of the
/// section — the wave aborts so it cannot be recorded as "done".
#[derive(Default)]
struct WaveResult {
    entities: Vec<extract::RawEntity>,
    relations: Vec<extract::RawRelation>,
    /// The section's concise glossary concept (query routing); None when the
    /// payload lacked one — the query path falls back to the section title.
    concept: Option<extract::RawConcept>,
    stats: extract::ExtractStats,
    fatal: Option<String>,
    /// Set when the model produced no usable payload. The section is stored
    /// without a content hash so the next run retries it.
    skipped: Option<String>,
}

fn extract_one(
    llm: &dyn ChatClient,
    sec: &Section,
    glossary: &[GlossaryEntry],
    cancel: &AtomicBool,
    settings: &crate::tunables::IndexSettings,
) -> WaveResult {
    if cancel.load(Ordering::Relaxed) {
        return WaveResult::default();
    }
    let vocab = extract::Vocab::from_index_settings(settings);
    match extract::extract_section(
        llm, &sec.title, &sec.text, glossary, crate::config::EXTRACT_MAX_TOKENS, &vocab, cancel,
    ) {
        Ok(extract::ExtractOutcome::Extracted { entities, relations, concept, stats }) => {
            WaveResult { entities, relations, concept, stats, fatal: None, skipped: None }
        }
        Ok(extract::ExtractOutcome::Skipped { reason }) => {
            WaveResult { skipped: Some(reason), ..Default::default() }
        }
        Err(e) => WaveResult { fatal: Some(e.to_string()), ..Default::default() },
    }
}

/// Per-wave accounting fed back to the caller after application.
#[derive(Default)]
struct WaveTotals {
    done: usize,
    skipped: usize,
    skip_reasons: Vec<String>,
    type_merges: usize,
    dropped_entities: u32,
    dropped_relations: u32,
    fatal: Option<String>,
}

/// Run extraction over `todo` in wave-sized chunks with parallel scoped
/// threads; glossaries are read from the live indexes at wave start, and
/// completed results are applied single-threaded in input order, one
/// transaction per section (per-section crash resume). Returns what was
/// applied before finishing or cancelling.
fn run_waves(
    store: &KgStore,
    todo: &[Section],
    workers: usize,
    llm: &dyn ChatClient,
    settings: &crate::tunables::IndexSettings,
    progress: impl FnMut(Progress) + Send,
    cancel: &AtomicBool,
) -> Result<WaveTotals, KgError> {
    let mut totals = WaveTotals::default();
    let total = todo.len();
    let workers = workers.max(1);
    // Workers report SectionDone the moment a section finishes (the UI must
    // see progress during a wave, not a burst after it); the Mutex keeps the
    // callback's FnMut invocation serial.
    let progress = Mutex::new(progress);

    for chunk_start in (0..total).step_by(workers) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(totals);
        }
        let chunk = &todo[chunk_start..(chunk_start + workers).min(total)];
        let done_count = AtomicUsize::new(chunk_start);

        // Wave-start glossaries: the live FTS + exact-name indexes answer
        // from the committed state, and every worker in the wave sees the
        // same view (the v1 per-wave fit, without the fit).
        let glossaries: Vec<Vec<GlossaryEntry>> = chunk
            .iter()
            .map(|sec| store.glossary(&sec.text, crate::config::GLOSSARY_K))
            .collect::<Result<_, _>>()?;

        // Workers share the read-only glossaries; results land in positional
        // slots so application order matches input order.
        let slots: Mutex<Vec<Option<WaveResult>>> =
            Mutex::new((0..chunk.len()).map(|_| None).collect());
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..workers.min(chunk.len()) {
                scope.spawn(|| loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= chunk.len() {
                        return;
                    }
                    let r = extract_one(llm, &chunk[i], &glossaries[i], cancel, settings);
                    let title = chunk[i].title.clone();
                    let entities = r.entities.len();
                    let relations = r.relations.len();
                    slots.lock().unwrap()[i] = Some(r);
                    let d = done_count.fetch_add(1, Ordering::SeqCst) + 1;
                    (progress.lock().unwrap())(Progress::SectionDone {
                        done: d,
                        total,
                        title,
                        entities,
                        relations,
                    });
                });
            }
        });
        let results = slots.into_inner().unwrap();

        // Any section-level extraction error is systemic (HTTP status,
        // unsupported output format, auth) — abort before applying so no
        // failed section is marked done and the caller reports the reason.
        if let Some(msg) = results.iter().flatten().find_map(|r| r.fatal.as_deref()) {
            totals.fatal = Some(msg.to_string());
            return Ok(totals);
        }

        // Single-threaded post-wave: one transaction per completed section,
        // in original order. A cancel mid-wave keeps the completed sections.
        for (i, slot) in results.into_iter().enumerate() {
            let Some(res) = slot else { continue };
            apply_section(store, &chunk[i], res, &mut totals)?;
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(totals);
        }
    }
    Ok(totals)
}

/// Write one extraction result into the store: entities (slug merge), the
/// section row, ordered joins, concept and relations in one transaction.
fn apply_section(
    store: &KgStore,
    sec: &Section,
    res: WaveResult,
    totals: &mut WaveTotals,
) -> Result<(), KgError> {
    let WaveResult { entities, relations, concept, stats, skipped, .. } = res;
    let tx = store.transaction()?;

    // A skipped section is stored with its text and provenance but no
    // content hash, so the resume scan picks it up on the next run.
    if let Some(reason) = skipped {
        let row = store.upsert_section(&SectionInput {
            title: &sec.title,
            text: &sec.text,
            source_doc: &sec.source_doc,
            start_line: sec.start_line,
            end_line: sec.end_line,
            content_hash: "",
            retry_pending: true,
            topic: None,
        })?;
        store.set_section_entities(row, &[])?;
        store.refresh_section_tokens(row)?;
        store.upsert_concept(row, None)?;
        totals.skipped += 1;
        totals.skip_reasons.push(format!("{}: {reason}", sec.title));
        tx.commit().map_err(crate::store::StoreError::from)?;
        return Ok(());
    }

    // Canonical slug merge: extraction mints ids from the type the model
    // chose, but a slug may only exist once in the graph. The first applied
    // entity wins; later ones merge into it (type_merges counts that).
    let mut remap: HashMap<String, String> = HashMap::new();
    let mut upserted: Vec<crate::store::UpsertedEntity> = Vec::new();
    for e in &entities {
        let entity = store.upsert_entity(&e.id, &e.name, &e.etype, &e.desc)?;
        if entity.entity_key != e.id {
            totals.type_merges += 1;
        }
        remap.insert(e.id.clone(), entity.entity_key.clone());
        if !upserted.iter().any(|u| u.entity_key == entity.entity_key) {
            upserted.push(entity);
        }
    }

    let hash = extract::section_hash(&sec.text);
    let row = store.upsert_section(&SectionInput {
        title: &sec.title,
        text: &sec.text,
        source_doc: &sec.source_doc,
        start_line: sec.start_line,
        end_line: sec.end_line,
        content_hash: &hash,
        retry_pending: false,
        topic: None,
    })?;
    for entity in &upserted {
        store.link_entity_section(entity.node, row)?;
        if !sec.source_doc.is_empty() {
            store.link_entity_doc(entity.node, &sec.source_doc)?;
        }
    }
    let entity_keys: Vec<String> = upserted.iter().map(|e| e.entity_key.clone()).collect();
    store.set_section_entities(row, &entity_keys)?;
    store.refresh_section_tokens(row)?;
    let concept = concept.map(|c| crate::graph::Concept {
        name: c.name,
        summary: c.summary,
        terms: c.terms,
    });
    store.upsert_concept(row, concept.as_ref())?;
    for r in &relations {
        let source = remap.get(&r.source).cloned().unwrap_or_else(|| r.source.clone());
        let target = remap.get(&r.target).cloned().unwrap_or_else(|| r.target.clone());
        store.insert_relation_if_known(&source, &target, &r.rel, row, Some(&sec.source_doc))?;
    }
    totals.dropped_entities += stats.dropped_entities;
    totals.dropped_relations += stats.dropped_relations;
    totals.done += 1;
    tx.commit().map_err(crate::store::StoreError::from)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Topic derivation (DD-6)
// ---------------------------------------------------------------------------

/// Structure-aware topics: numbered chapters cluster deterministically with
/// the doc tag baked into the name ('[CS] Chapter 3') so two numbered books
/// can never fuse their same-numbered chapters. Loose (unnumbered) titles
/// group per document via the LLM; documents untouched this run keep their
/// stored groups verbatim (a no-op build never reshuffles routing topology).
fn derive_topics(
    store: &KgStore,
    all_sections: &[Section],
    tag_by_doc: &HashMap<String, String>,
    regroup_docs: Option<&BTreeSet<String>>,
    llm: &dyn ChatClient,
    cancel: &AtomicBool,
) -> Result<(), KgError> {
    let mut chapter_map: HashMap<String, String> = HashMap::new();
    for s in all_sections {
        if let (Some(ch), Some(tag)) = (&s.chapter, tag_by_doc.get(&s.source_doc)) {
            chapter_map.insert(s.title.clone(), format!("[{tag}] Chapter {ch}"));
        }
    }

    let mut topic_map: HashMap<String, String> = HashMap::new();
    for (title, topic) in store.section_topics()? {
        if chapter_map.contains_key(&title) {
            continue;
        }
        if let Some(t) = migrate_topic(&title, topic.as_deref()) {
            topic_map.insert(title, t);
        }
    }
    topic_map.extend(chapter_map.clone());

    let mut loose_by_doc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in all_sections {
        if chapter_map.contains_key(&s.title) {
            continue;
        }
        if let Some(keep) = regroup_docs {
            if !keep.contains(&s.source_doc) {
                continue;
            }
        }
        loose_by_doc.entry(s.source_doc.clone()).or_default().push(s.title.clone());
    }
    for titles in loose_by_doc.values() {
        if titles.is_empty() {
            continue;
        }
        topic_map.extend(extract::group_topics(llm, titles.clone(), cancel)?);
    }
    store.apply_topics(&topic_map)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Index every input document into `opts.graph_path`. Per-section
/// transactions make it crash-safe: a cancelled or failed run resumes from
/// the last committed section.
pub fn run(
    docs: &[InputDoc],
    opts: &IndexOptions,
    mut progress: impl FnMut(Progress) + Send,
    cancel: &AtomicBool,
) -> Result<IndexStats, KgError> {
    let t0 = std::time::Instant::now();
    let graph_path = opts.graph_path.clone();
    let workers = opts.workers.max(1);

    // ---- preflight: parse corpus ----
    let mut all_sections: Vec<Section> = vec![];
    for doc in docs {
        let mut secs = extract::parse_sections(&doc.markdown, &doc.name);
        for s in &mut secs {
            s.title = format!("[{}] {}", doc.tag, s.title); // '[MC1] 8.3 Cache'
        }
        all_sections.extend(secs);
    }
    if all_sections.is_empty() {
        return Err(KgError::Preflight(
            "no sections parsed from resources — scan or add markdown documents first"
                .to_string(),
        ));
    }
    let total = all_sections.len();
    progress(Progress::Started { docs: docs.len(), total_sections: total });

    // ---- preflight: model label for the UI ----
    let label = opts.llm.label();
    if !label.is_empty() {
        progress(Progress::Phase(format!("Indexing with {label}")));
    }
    let llm = std::sync::Arc::clone(&opts.llm);
    if cancel.load(Ordering::Relaxed) {
        return Err(KgError::Cancelled);
    }

    // ---- checkpoint load / fresh backup ----
    let existed = graph_path.exists();
    if opts.fresh && existed {
        let stamp = timestamp();
        let backup = graph_path.with_file_name(format!("graph_backup_{stamp}.sqlite"));
        let existing = KgStore::open(&graph_path, OpenMode::ReadWrite)?;
        existing.backup_to(&backup)?;
        drop(existing);
        remove_store_files(&graph_path);
        progress(Progress::Warn(format!(
            "prior graph backed up to {}",
            backup.file_name().unwrap_or_default().to_string_lossy())));
    }
    let store = match KgStore::open(&graph_path, OpenMode::ReadWrite) {
        Ok(store) => store,
        Err(e) => {
            progress(Progress::Warn(format!(
                "checkpoint unreadable ({e}); starting clean")));
            remove_store_files(&graph_path);
            KgStore::open(&graph_path, OpenMode::ReadWrite)?
        }
    };
    // The graph is being rebuilt: block queries until finalize.
    store.set_meta(crate::store::META_DERIVED_READY, "0")?;

    // ---- indexed sync diff (DD-16) ----
    let scope_docs: BTreeSet<String> = docs.iter().map(|d| d.name.clone()).collect();
    let mut regroup_docs: Option<BTreeSet<String>> = None;
    let mut skipped = 0usize;
    let mut todo: Vec<Section> = all_sections.clone();

    if !opts.fresh {
        let sync = plan_sync(&store, &all_sections, &scope_docs)?;
        let mut gc_titles: Vec<String> = sync.removed.iter().map(|(t, _)| t.clone()).collect();
        gc_titles.extend(sync.changed.iter().map(|c| c.title.clone()));
        // only documents whose content actually changed get regrouped
        let mut affected: BTreeSet<String> = BTreeSet::new();
        for a in &sync.added {
            affected.insert(a.source_doc.clone());
        }
        for c in &sync.changed {
            affected.insert(c.source_doc.clone());
        }
        for (_, doc) in &sync.removed {
            affected.insert(doc.clone());
        }
        regroup_docs = Some(affected);

        let before = store.counts()?;
        store.remove_sections(&gc_titles)?;
        let after = store.counts()?;
        progress(Progress::Sync {
            added: sync.added.len(),
            changed: sync.changed.len(),
            removed: sync.removed.len(),
            unchanged: sync.unchanged.len(),
        });
        if sync.backfilled > 0 || before.entities > after.entities {
            progress(Progress::Warn(format!(
                "{} legacy hashes backfilled, GC freed {} relation(s) + {} entity(ies)",
                sync.backfilled,
                before.relations.saturating_sub(after.relations),
                before.entities.saturating_sub(after.entities))));
        }

        // Documents whose files vanished from Resources are evicted whole
        // (out-of-scope immunity still guards live documents).
        let vanished: BTreeSet<String> = store
            .section_docs()?
            .into_iter()
            .filter(|d| !scope_docs.contains(d))
            .collect();
        if !vanished.is_empty() {
            let titles = store.section_titles_for_docs(&vanished)?;
            let count = titles.len();
            let before = store.counts()?;
            store.remove_sections(&titles)?;
            let after = store.counts()?;
            progress(Progress::Warn(format!(
                "evicted {count} section(s) from {} removed document(s) \
                 (GC: {} relation(s), {} entity(ies))",
                vanished.len(),
                before.relations.saturating_sub(after.relations),
                before.entities.saturating_sub(after.entities))));
        }

        let unchanged: HashSet<(String, String)> = sync.unchanged.into_iter().collect();
        todo = all_sections
            .iter()
            .filter(|s| !unchanged.contains(&(s.source_doc.clone(), s.title.clone())))
            .cloned()
            .collect();
        skipped = total - todo.len();
    }

    if !todo.is_empty() {
        progress(Progress::Phase(format!("Indexing {} section{}", todo.len(),
            if todo.len() == 1 { "" } else { "s" })));
    } else if skipped > 0 {
        progress(Progress::Phase("Everything already indexed — refreshing structure".to_string()));
    }

    // ---- extraction waves ----
    let totals = run_waves(&store, &todo, workers, llm.as_ref(), &opts.settings,
                           &mut |p| progress(p),
                           cancel)?;
    if let Some(msg) = totals.fatal {
        // The failed wave was discarded before hashing, so those sections
        // re-extract on the next run; surface the real reason instead of
        // saving an empty graph and reporting success.
        return Err(KgError::Extraction(msg));
    }
    if totals.skipped > 0 {
        let sample: Vec<String> = totals.skip_reasons.iter().take(3).cloned().collect();
        progress(Progress::Warn(format!(
            "{} section(s) skipped and left for the next run: {}",
            totals.skipped, sample.join("; "))));
    }
    let cancelled = totals.done + totals.skipped < todo.len();
    if cancelled {
        // committed sections are the checkpoint; stop here and report it
        return Err(KgError::Cancelled);
    }
    // A graph with sections but no entities is a failed build (model ignored
    // the format, or every payload was dropped) — never report ok=true on it.
    let counts = store.counts()?;
    if counts.sections >= 2 && counts.entities == 0 {
        let detail = if totals.skipped > 0 {
            let sample: Vec<String> = totals.skip_reasons.iter().take(3).cloned().collect();
            format!(" ({} section(s) skipped: {})", totals.skipped, sample.join("; "))
        } else {
            " (the model returned no entities for any section)".to_string()
        };
        return Err(KgError::Extraction(format!(
            "no entities in the graph across {} section(s){detail} — check the extraction model and that it supports the requested output format",
            counts.sections
        )));
    }
    if totals.dropped_entities > 0 || totals.dropped_relations > 0 {
        progress(Progress::Warn(format!(
            "dropped invalid output this run: {} entity(ies), {} relation(s)",
            totals.dropped_entities, totals.dropped_relations)));
    }

    // ---- topics + hashes + floor + finalize ----
    progress(Progress::Phase("Grouping topics".to_string()));
    derive_topics(&store, &all_sections,
                  &docs.iter().map(|d| (d.name.clone(), d.tag.clone())).collect(),
                  regroup_docs.as_ref(), llm.as_ref(), cancel)?;
    // Record the raw-file fingerprints for staleness detection. Evicted
    // documents are pruned; out-of-scope docs of a shared graph keep theirs.
    for doc in docs {
        store.upsert_source_hash(&doc.name, &doc.source_hash)?;
    }
    store.prune_source_hashes()?;
    // The floor is corpus-level: a full build (or a build that extracted at
    // least 1% of sections) re-measures it; a small incremental patch reuses
    // the stored value so one changed document does not pay for 300 samples.
    let stored_floor = store.noise_floor()?;
    let floor_stale = stored_floor.is_none() || totals.done * 100 >= counts.sections.max(1);
    let noise_floor = if counts.entities == 0 {
        None
    } else if floor_stale {
        progress(Progress::Phase("Measuring the lexical noise floor".to_string()));
        measure_noise_floor(&store)?
    } else {
        stored_floor
    };
    store.finalize_build(noise_floor)?;
    finalize_stats(&store, docs.len(), skipped, totals.done, totals.type_merges, noise_floor, t0)
}

/// DD-17: median top exact-BM25 score of random queries built from the
/// entity corpus's common vocabulary. The same scorer answers queries, so the
/// escalation ratio stays self-calibrating.
fn measure_noise_floor(store: &KgStore) -> Result<Option<f64>, KgError> {
    let df = store.entity_df()?;
    if df.is_empty() {
        return Ok(None);
    }
    let mut common: Vec<String> = df
        .iter()
        .filter(|(_, &count)| count >= 20)
        .map(|(term, _)| term.clone())
        .collect();
    common.sort();
    if common.len() < 4 {
        // fall back to the most frequent tokens
        let mut by_freq: Vec<(String, usize)> = df.into_iter().collect();
        by_freq.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        common = by_freq.into_iter().take(10).map(|(term, _)| term).collect();
        if common.is_empty() {
            return Ok(None);
        }
    }
    let mut rng = crate::text::Rng::new(42);
    let mut tops: Vec<f64> = vec![];
    for _ in 0..300 {
        let picks = rng.sample_indices(4.min(common.len()), common.len());
        let q: Vec<String> = picks.iter().map(|&i| common[i].clone()).collect();
        let best = store
            .bm25_entity_search(&q.join(" "), 1)?
            .first()
            .map(|(_, score)| *score)
            .unwrap_or(0.0);
        if best > 0.0 {
            tops.push(best);
        }
    }
    if tops.is_empty() {
        return Ok(None);
    }
    tops.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Ok(Some(tops[tops.len() / 2]))
}

fn finalize_stats(store: &KgStore, docs: usize, skipped: usize, extracted: usize,
                  type_merges: usize, noise_floor: Option<f64>,
                  t0: std::time::Instant) -> Result<IndexStats, KgError> {
    let counts = store.counts()?;
    Ok(IndexStats {
        entities: counts.entities,
        relations: counts.relations,
        sections: counts.sections,
        topics: counts.topics,
        docs,
        extracted_this_run: extracted,
        skipped_resume: skipped,
        cross_doc_merges: store.cross_doc_merges()?,
        type_merges,
        noise_floor,
        elapsed_secs: t0.elapsed().as_secs_f64(),
    })
}

/// Remove a SQLite store and its journal/WAL sidecars.
fn remove_store_files(path: &std::path::Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut raw = path.as_os_str().to_os_string();
        raw.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(raw));
    }
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}{mth:02}{d:02}_{h:02}{m:02}{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::KnowledgeGraph;
    use crate::llm::{ChatClient, LlmError};
    use serde_json::Value;

    /// Replies with one fixed string to every call.
    struct FixedClient(String);

    impl ChatClient for FixedClient {
        fn chat(&self, _messages: &[Value], _max_tokens: u32,
                _response_format: Option<Value>, _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            Ok(self.0.clone())
        }

        fn chat_stream(&self, _messages: &[Value], _max_tokens: u32,
                       _on_delta: &mut dyn FnMut(&str), _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            Ok(self.0.clone())
        }
    }

    fn doc_markdown() -> String {
        let cache = "Cache memory reduces average access time and keeps the pipeline fed with instructions. ".repeat(4);
        let pipe = "Pipelines overlap instruction execution to improve throughput across the machine. ".repeat(4);
        format!("# Test\n\n## 1. Cache Memory\n\n{cache}\n\n## 2. Pipelining\n\n{pipe}")
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("docfoo-kg-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn load_graph(path: &std::path::Path) -> KnowledgeGraph {
        let store = KgStore::open(path, OpenMode::ReadOnly).expect("store opens");
        let mut kg = KnowledgeGraph::default();
        store.load_into(&mut kg).expect("store loads");
        kg
    }

    fn run_with(reply: &str, graph: &std::path::Path, fresh: bool) -> Result<IndexStats, KgError> {
        let docs = vec![InputDoc {
            name: "test.md".to_string(),
            tag: "T".to_string(),
            markdown: doc_markdown(),
            source_hash: "hash".to_string(),
        }];
        let opts = IndexOptions {
            llm: std::sync::Arc::new(FixedClient(reply.to_string())),
            workers: 2,
            fresh,
            graph_path: graph.to_path_buf(),
            settings: crate::tunables::IndexSettings::default(),
        };
        let cancel = AtomicBool::new(false);
        run(&docs, &opts, |_| {}, &cancel)
    }

    const GOOD_REPLY: &str =
        r#"{"entities":[{"name":"Cache","type":"CONCEPT","description":"fast memory"}],"relations":[]}"#;

    #[test]
    fn skipped_sections_are_not_marked_done_and_retry_next_run() {
        let dir = test_dir("skip");
        let graph = dir.join("graph.sqlite");

        let error = run_with("I cannot help with that.", &graph, true).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("no entities"), "got: {message}");
        assert!(message.contains("skipped"), "got: {message}");

        let stored = load_graph(&graph);
        assert_eq!(stored.sections.len(), 2);
        assert!(stored.entities.is_empty());
        assert!(
            stored.sections.values().all(|s| s.content_hash.is_empty() && s.retry_pending),
            "skipped sections must stay un-hashed and flagged for retry"
        );

        // The next run must re-extract both sections, not treat them as
        // legacy hash-less sections.
        let stats = run_with(GOOD_REPLY, &graph, false).expect("retry run succeeds");
        assert_eq!(stats.sections, 2);
        assert!(stats.entities >= 1);
        assert_eq!(stats.extracted_this_run, 2);
        assert_eq!(stats.skipped_resume, 0);

        let stored = load_graph(&graph);
        assert!(stored.sections.values().all(|s| !s.content_hash.is_empty() && !s.retry_pending));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn valid_empty_extraction_is_done_not_pending() {
        // The empty-graph guard still fails the build, but the sections were
        // extracted (hashed), not skipped, so they are not retried forever.
        let dir = test_dir("empty");
        let graph = dir.join("graph.sqlite");
        let error = run_with(r#"{"entities":[],"relations":[]}"#, &graph, true).unwrap_err();
        assert!(error.to_string().contains("no entities"), "got: {error}");

        let stored = load_graph(&graph);
        assert_eq!(stored.sections.len(), 2);
        assert!(stored.sections.values().all(|s| !s.content_hash.is_empty() && !s.retry_pending));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Replies with a queue of strings, one per call. workers = 1 keeps the
    /// call order deterministic.
    struct QueueClient(std::sync::Mutex<std::collections::VecDeque<String>>);

    impl QueueClient {
        fn new(replies: Vec<String>) -> Self {
            QueueClient(std::sync::Mutex::new(replies.into()))
        }
    }

    impl ChatClient for QueueClient {
        fn chat(&self, _messages: &[Value], _max_tokens: u32,
                _response_format: Option<Value>, _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            Ok(self.0.lock().unwrap().pop_front().unwrap_or_else(|| GOOD_REPLY.to_string()))
        }

        fn chat_stream(&self, _messages: &[Value], _max_tokens: u32,
                       _on_delta: &mut dyn FnMut(&str), _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            Ok(self.0.lock().unwrap().pop_front().unwrap_or_else(|| GOOD_REPLY.to_string()))
        }
    }

    fn run_with_queue(replies: Vec<String>, graph: &std::path::Path) -> Result<IndexStats, KgError> {
        let docs = vec![InputDoc {
            name: "test.md".to_string(),
            tag: "T".to_string(),
            markdown: doc_markdown(),
            source_hash: "hash".to_string(),
        }];
        let opts = IndexOptions {
            llm: std::sync::Arc::new(QueueClient::new(replies)),
            workers: 1,
            fresh: true,
            graph_path: graph.to_path_buf(),
            settings: crate::tunables::IndexSettings::default(),
        };
        let cancel = AtomicBool::new(false);
        run(&docs, &opts, |_| {}, &cancel)
    }

    #[test]
    fn type_mismatched_duplicates_merge_into_one_node() {
        let dir = test_dir("type-merge");
        let graph = dir.join("graph.sqlite");
        let replies = vec![
            r#"{"entities":[{"name":"UNIVERSAT","type":"CONCEPT","description":"a model"},{"name":"Foo","type":"CONCEPT","description":"a thing"}],"relations":[{"source":"UNIVERSAT","relation":"USES","target":"Foo"}]}"#.to_string(),
            r#"{"entities":[{"name":"UNIVERSAT","type":"PROCESS","description":"a longer description of the model"},{"name":"Bar","type":"CONCEPT","description":"another thing"}],"relations":[{"source":"UNIVERSAT","relation":"ENABLES","target":"Bar"}]}"#.to_string(),
        ];
        let stats = run_with_queue(replies, &graph).expect("build succeeds");
        assert_eq!(stats.type_merges, 1);

        let stored = load_graph(&graph);
        assert!(stored.entities.contains_key("CONCEPT_universat"));
        assert!(!stored.entities.contains_key("PROCESS_universat"));
        assert_eq!(
            stored.entities["CONCEPT_universat"].desc,
            "a longer description of the model"
        );
        assert_eq!(stored.relations.len(), 2);
        assert!(
            stored.relations.iter().all(|r| r.source == "CONCEPT_universat"),
            "relations from the PROCESS payload must remap to the surviving node"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cancelled build leaves committed per-section rows with
    /// `derived_ready=0`; the next run resumes, skips the already-hashed
    /// section and finalizes the derived indexes.
    #[test]
    fn resume_from_a_section_checkpoint_finishes_the_graph() {
        let dir = test_dir("resume-checkpoint");
        let graph = dir.join("graph.sqlite");

        let cache = "Cache memory reduces average access time and keeps the pipeline fed with instructions. "
            .repeat(4);
        let title = "[T] 1. Cache Memory";
        let store = KgStore::open(&graph, OpenMode::ReadWrite).unwrap();
        let row = store
            .upsert_section(&SectionInput {
                title,
                text: &cache,
                source_doc: "test.md",
                start_line: 0,
                end_line: 0,
                content_hash: &extract::section_hash(&cache),
                retry_pending: false,
                topic: None,
            })
            .unwrap();
        let entity = store
            .upsert_entity("CONCEPT_cache", "Cache", "CONCEPT", "fast memory")
            .unwrap();
        store.link_entity_section(entity.node, row).unwrap();
        store.link_entity_doc(entity.node, "test.md").unwrap();
        store
            .set_section_entities(row, &["CONCEPT_cache".to_string()])
            .unwrap();
        store.refresh_section_tokens(row).unwrap();
        store.upsert_concept(row, None).unwrap();
        store
            .set_meta(crate::store::META_DERIVED_READY, "0")
            .unwrap();
        assert!(!store.derived_ready().unwrap(), "checkpoints are not queryable");
        drop(store);

        let resume_reply = r#"{"entities":[
            {"name":"Cache","type":"CONCEPT","description":"fast memory near the cpu"},
            {"name":"Pipeline","type":"PROCESS","description":"overlaps instruction execution"},
            {"name":"Register","type":"DEVICE","description":"fastest storage in the processor"}
        ],"relations":[]}"#;
        let stats = run_with(resume_reply, &graph, false).expect("resume build succeeds");
        assert_eq!(stats.skipped_resume, 1, "the checkpointed section is not re-extracted");

        let stored = load_graph(&graph);
        assert!(stored.sections[title].text.contains("Cache memory reduces"));
        let store = KgStore::open(&graph, OpenMode::ReadOnly).unwrap();
        assert!(store.derived_ready().unwrap(), "final save rebuilds derived data");
        assert!(
            store.bm25_entity_search("cache memory", 5).unwrap()
                .iter().any(|(id, _)| id == "CONCEPT_cache"),
            "derived BM25 index works after resume"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

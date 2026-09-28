//! `KnowledgeGraph` -> SQLite rows and (via triggers) FTS.

use super::{StoreError, StoreResult};
use crate::graph::{concept_text, entity_text, KnowledgeGraph};
use crate::text::tokenize;
use rusqlite::{params, Connection};
use std::collections::{BTreeSet, HashMap};

pub(super) fn write_all(
    conn: &Connection,
    kg: &KnowledgeGraph,
    build_uid: &str,
) -> StoreResult<()> {
    write_meta(conn, kg, build_uid)?;
    // Sections first: provenance joins store `section_row`, and relation
    // anchors need the section row to exist. FTS indexes are filled by the
    // schema triggers, so there is no rebuild step.
    let row_of = write_sections(conn, kg)?;
    let node_of = write_entities(conn, kg, &row_of)?;
    write_relations(conn, kg, &node_of, &row_of)?;
    write_source_hashes(conn, kg)?;
    write_topics(conn, kg)?;
    Ok(())
}

fn write_meta(conn: &Connection, kg: &KnowledgeGraph, build_uid: &str) -> StoreResult<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )?;
    let mut put = |key: &str, value: String| -> StoreResult<()> {
        stmt.execute(params![key, value])?;
        Ok(())
    };
    put(super::META_SCHEMA_VERSION, super::SCHEMA_VERSION.to_string())?;
    put(super::META_BUILD_UID, build_uid.to_string())?;
    put(super::META_DERIVED_READY, "1".to_string())?;
    put(
        super::META_NOISE_FLOOR,
        serde_json::to_string(&kg.noise_floor).map_err(|e| StoreError::Data(e.to_string()))?,
    )?;
    put(super::META_ENTITY_COUNT, kg.entities.len().to_string())?;
    put(super::META_RELATION_COUNT, kg.relations.len().to_string())?;
    put(super::META_SECTION_COUNT, kg.sections.len().to_string())?;
    Ok(())
}

/// Section rows, ordered `section_entities`, one concept row per section and
/// the raw locator vocabulary. Returns `title -> row` for the provenance
/// joins.
fn write_sections<'a>(
    conn: &Connection,
    kg: &'a KnowledgeGraph,
) -> StoreResult<HashMap<&'a str, i64>> {
    let mut section_stmt = conn.prepare(
        "INSERT INTO sections(row, title, topic, text, source_doc, start_line, end_line,
                              content_hash, retry_pending, tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    let mut entity_stmt = conn.prepare(
        "INSERT INTO section_entities(section_row, entity_key, ord) VALUES (?1, ?2, ?3)",
    )?;
    let mut concept_stmt = conn.prepare(
        "INSERT INTO concepts(section_row, present, name, summary, terms, tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut locator_stmt = conn.prepare(
        "INSERT OR IGNORE INTO section_locator_terms(term, section_row) VALUES (?1, ?2)",
    )?;

    let mut row_of: HashMap<&str, i64> = HashMap::with_capacity(kg.sections.len());
    for (index, (title, info)) in kg.sections.iter().enumerate() {
        let row = index as i64;
        row_of.insert(title.as_str(), row);
        let tokens = tokenize(&kg.section_text(title, info));
        section_stmt.execute(params![
            row,
            title.as_str(),
            info.topic.as_deref(),
            info.text.as_str(),
            info.source_doc.as_str(),
            info.start_line as i64,
            info.end_line as i64,
            info.content_hash.as_str(),
            info.retry_pending as i64,
            tokens.join(" ")
        ])?;
        for (ord, entity_key) in info.entity_ids.iter().enumerate() {
            entity_stmt.execute(params![row, entity_key.as_str(), ord as i64])?;
        }
        // Raw section-text tokens back the rare-token locator (DD-21); the
        // locator_df triggers count them.
        for term in crate::query::deliver::locator_tokens(&info.text) {
            locator_stmt.execute(params![term, row])?;
        }

        // One concept row per section: `present` keeps Some(empty) distinct
        // from None, `tokens` carries the title fallback used by the corpus.
        let concept_tokens = tokenize(&concept_text(title, info));
        let (present, name, summary, terms) = match &info.concept {
            Some(concept) => (
                1i64,
                concept.name.as_str(),
                concept.summary.as_str(),
                serde_json::to_string(&concept.terms)
                    .map_err(|e| StoreError::Data(e.to_string()))?,
            ),
            None => (0i64, "", "", "[]".to_string()),
        };
        concept_stmt.execute(params![
            row,
            present,
            name,
            summary,
            terms.as_str(),
            concept_tokens.join(" ")
        ])?;
    }
    Ok(row_of)
}

/// Returns `entity_key -> node` for the relation endpoint check.
fn write_entities<'a>(
    conn: &Connection,
    kg: &'a KnowledgeGraph,
    row_of: &HashMap<&'a str, i64>,
) -> StoreResult<HashMap<&'a str, i64>> {
    let mut entity_stmt = conn.prepare(
        "INSERT INTO entities(node, entity_key, entity_slug, name, type, desc, tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut section_stmt = conn.prepare(
        "INSERT INTO entity_sections(node, section_row, ord) VALUES (?1, ?2, ?3)",
    )?;
    let mut doc_stmt =
        conn.prepare("INSERT INTO entity_docs(node, doc, ord) VALUES (?1, ?2, ?3)")?;
    let mut name_stmt = conn.prepare(
        "INSERT OR IGNORE INTO entity_name_terms(term, node, name_len) VALUES (?1, ?2, ?3)",
    )?;
    let mut df_stmt = conn.prepare(
        "INSERT INTO entity_df(term, df) VALUES (?1, 1)
         ON CONFLICT(term) DO UPDATE SET df = df + 1",
    )?;

    let mut node_of: HashMap<&str, i64> = HashMap::with_capacity(kg.entities.len());
    for (index, (key, entity)) in kg.entities.iter().enumerate() {
        let node = index as i64;
        node_of.insert(key.as_str(), node);
        let tokens = tokenize(&entity_text(entity));
        entity_stmt.execute(params![
            node,
            key.as_str(),
            crate::text::canonical_slug(&entity.name),
            entity.name.as_str(),
            entity.etype.as_str(),
            entity.desc.as_str(),
            tokens.join(" ")
        ])?;
        for (ord, section) in entity.sections.iter().enumerate() {
            let Some(&section_row) = row_of.get(section.as_str()) else {
                return Err(StoreError::Data(format!(
                    "entity {key:?} references unknown section {section:?}"
                )));
            };
            section_stmt.execute(params![node, section_row, ord as i64])?;
        }
        for (ord, doc) in entity.source_doc.iter().enumerate() {
            doc_stmt.execute(params![node, doc.as_str(), ord as i64])?;
        }
        // Exact-name index for the native glossary (duplicates are ignored).
        let name_terms: BTreeSet<String> = tokenize(&entity.name).into_iter().collect();
        let name_len = name_terms.len() as i64;
        for term in &name_terms {
            name_stmt.execute(params![term, node, name_len])?;
        }
        // Entity document frequencies (unique terms per entity).
        for term in tokens.iter().collect::<BTreeSet<_>>() {
            df_stmt.execute([term.as_str()])?;
        }
    }
    Ok(node_of)
}

fn write_relations(
    conn: &Connection,
    kg: &KnowledgeGraph,
    node_of: &HashMap<&str, i64>,
    row_of: &HashMap<&str, i64>,
) -> StoreResult<()> {
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO relations(rid, src, dst, rel, section_row, source_doc)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (rid, relation) in kg.relations.iter().enumerate() {
        let Some(&src) = node_of.get(relation.source.as_str()) else {
            return Err(StoreError::Data(format!(
                "relation references unknown source entity {:?}",
                relation.source
            )));
        };
        let Some(&dst) = node_of.get(relation.target.as_str()) else {
            return Err(StoreError::Data(format!(
                "relation references unknown target entity {:?}",
                relation.target
            )));
        };
        let Some(&section_row) = row_of.get(relation.section.as_str()) else {
            return Err(StoreError::Data(format!(
                "relation references unknown section {:?}",
                relation.section
            )));
        };
        stmt.execute(params![
            rid as i64,
            src,
            dst,
            relation.rel.as_str(),
            section_row,
            relation.source_doc.as_deref()
        ])?;
    }
    Ok(())
}

fn write_source_hashes(conn: &Connection, kg: &KnowledgeGraph) -> StoreResult<()> {
    let mut stmt = conn.prepare("INSERT INTO source_hashes(doc, hash) VALUES (?1, ?2)")?;
    for (doc, hash) in &kg.source_hashes {
        stmt.execute(params![doc.as_str(), hash.as_str()])?;
    }
    Ok(())
}

fn write_topics(conn: &Connection, kg: &KnowledgeGraph) -> StoreResult<()> {
    let mut stmt = conn.prepare("INSERT INTO topics(topic, section, ord) VALUES (?1, ?2, ?3)")?;
    for (topic, info) in &kg.topics {
        for (ord, section) in info.sections.iter().enumerate() {
            stmt.execute(params![topic.as_str(), section.as_str(), ord as i64])?;
        }
    }
    Ok(())
}

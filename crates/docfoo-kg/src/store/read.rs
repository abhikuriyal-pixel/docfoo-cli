//! SQLite rows -> `KnowledgeGraph`, preserving every list order.

use super::{StoreError, StoreResult};
use crate::graph::{Concept, Entity, KnowledgeGraph, Relation, SectionInfo};
use rusqlite::{Connection, OptionalExtension};

pub(super) fn meta(conn: &Connection, key: &str) -> StoreResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0))
        .optional()?)
}

pub(super) fn load_into(conn: &Connection, kg: &mut KnowledgeGraph) -> StoreResult<()> {
    *kg = KnowledgeGraph::default();
    load_entities(conn, kg)?;
    load_sections(conn, kg)?;
    load_relations(conn, kg)?;
    load_source_hashes(conn, kg)?;
    load_topics(conn, kg)?;
    kg.noise_floor = match meta(conn, super::META_NOISE_FLOOR)? {
        Some(raw) => serde_json::from_str::<Option<f64>>(&raw)
            .map_err(|e| StoreError::Data(format!("noise_floor: {e}")))?,
        None => None,
    };
    Ok(())
}

fn load_entities(conn: &Connection, kg: &mut KnowledgeGraph) -> StoreResult<()> {
    {
        let mut stmt = conn.prepare(
            "SELECT entity_key, name, type, desc FROM entities ORDER BY node",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Entity {
                id: row.get(0)?,
                name: row.get(1)?,
                etype: row.get(2)?,
                desc: row.get(3)?,
                sections: Vec::new(),
                source_doc: Vec::new(),
            })
        })?;
        for entity in rows {
            let entity = entity?;
            kg.entities.insert(entity.id.clone(), entity);
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT e.entity_key, s.title FROM entity_sections es
             JOIN entities e ON e.node = es.node
             JOIN sections s ON s.row = es.section_row
             ORDER BY es.node, es.ord",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let title: String = row.get(1)?;
            if let Some(entity) = kg.entities.get_mut(&key) {
                entity.sections.push(title);
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT e.entity_key, d.doc FROM entity_docs d
             JOIN entities e ON e.node = d.node
             ORDER BY d.node, d.ord",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let doc: String = row.get(1)?;
            if let Some(entity) = kg.entities.get_mut(&key) {
                entity.source_doc.push(doc);
            }
        }
    }
    Ok(())
}

fn load_sections(conn: &Connection, kg: &mut KnowledgeGraph) -> StoreResult<()> {
    {
        let mut stmt = conn.prepare(
            "SELECT title, topic, text, source_doc, start_line, end_line,
                    content_hash, retry_pending
             FROM sections ORDER BY row",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let title: String = row.get(0)?;
            kg.sections.insert(
                title,
                SectionInfo {
                    topic: row.get(1)?,
                    entity_ids: Vec::new(),
                    concept: None,
                    text: row.get(2)?,
                    source_doc: row.get(3)?,
                    start_line: row.get::<_, i64>(4)? as usize,
                    end_line: row.get::<_, i64>(5)? as usize,
                    content_hash: row.get(6)?,
                    retry_pending: row.get::<_, i64>(7)? != 0,
                },
            );
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT s.title, se.entity_key FROM section_entities se
             JOIN sections s ON s.row = se.section_row
             ORDER BY se.section_row, se.ord",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let title: String = row.get(0)?;
            let entity_key: String = row.get(1)?;
            if let Some(info) = kg.sections.get_mut(&title) {
                info.entity_ids.push(entity_key);
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT s.title, c.present, c.name, c.summary, c.terms FROM concepts c
             JOIN sections s ON s.row = c.section_row",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let title: String = row.get(0)?;
            let present: i64 = row.get(1)?;
            if present == 0 {
                continue;
            }
            let terms_raw: String = row.get(4)?;
            let terms: Vec<String> = serde_json::from_str(&terms_raw).unwrap_or_default();
            if let Some(info) = kg.sections.get_mut(&title) {
                info.concept = Some(Concept {
                    name: row.get(2)?,
                    summary: row.get(3)?,
                    terms,
                });
            }
        }
    }
    Ok(())
}

fn load_relations(conn: &Connection, kg: &mut KnowledgeGraph) -> StoreResult<()> {
    let mut stmt = conn.prepare(
        "SELECT s.entity_key, t.entity_key, r.rel, sec.title, r.source_doc
         FROM relations r
         JOIN entities s ON s.node = r.src
         JOIN entities t ON t.node = r.dst
         JOIN sections sec ON sec.row = r.section_row
         ORDER BY r.rid",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        kg.relations.push(Relation {
            source: row.get(0)?,
            target: row.get(1)?,
            rel: row.get(2)?,
            section: row.get(3)?,
            source_doc: row.get(4)?,
        });
    }
    Ok(())
}

fn load_source_hashes(conn: &Connection, kg: &mut KnowledgeGraph) -> StoreResult<()> {
    let mut stmt = conn.prepare("SELECT doc, hash FROM source_hashes")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        kg.source_hashes.insert(row.get(0)?, row.get(1)?);
    }
    Ok(())
}

fn load_topics(conn: &Connection, kg: &mut KnowledgeGraph) -> StoreResult<()> {
    let mut stmt = conn.prepare("SELECT topic, section FROM topics ORDER BY topic, ord")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let topic: String = row.get(0)?;
        let section: String = row.get(1)?;
        kg.topics
            .entry(topic)
            .or_default()
            .sections
            .push(section);
    }
    Ok(())
}

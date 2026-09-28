//! Packed binary scene projection for the KG visualizer.
//!
//! `build_scene` reads only the base rows the renderer needs (entity keys,
//! relation endpoints, degrees, meta) and returns one little-endian buffer the
//! frontend wraps in typed arrays. This replaces the JSON projection: no
//! `serde_json::Value` tree, no 1M-string array, no per-element validation,
//! and no multi-megabyte JSON text to serialize/parse per fetch.
//!
//! The format is versioned and mirrored by `decodeScene` in
//! `src/features/chat/kg-viz/projection.ts`; keep both in sync.
//!
//! ```text
//!  0  u32 magic                  SCENE_MAGIC
//!  4  u32 version                SCENE_VERSION
//!  8  u32 flags                  bit0 = ids sorted in JS UTF-16 order
//! 12  u32 node_count
//! 16  u32 edge_count             relation rows
//! 20  u32 top_count
//! 24  u32 hash_len               UTF-8 bytes of build_uid
//! 28  u32 type_count             unique entity type names (≤ 255)
//! 32  f64 noise_floor            0.0 when unset
//! 40  u32 degrees[node_count]
//!     u32 top[top_count]         degree desc, index asc
//!     u32 edges[2*edge_count]    `[a, b, a, b, …]` in rid order
//!     u32 id_offsets[node_count+1]  byte offsets into the id blob
//!     u8  node_types[node_count] type index per node (255 = untyped)
//!     u32 type_offsets[type_count+1]  byte offsets into the type blob
//!     u8  hash[hash_len]
//!     u8  ids[…]                 concatenated entity_key UTF-8
//!     u8  types[…]               concatenated type name UTF-8 (sorted)
//! ```

use super::{KgStore, StoreResult};
use rusqlite::OptionalExtension;

/// `"KGVS"` (KG visual scene).
pub const SCENE_MAGIC: u32 = 0x4B47_5653;
/// Bump whenever the layout below or the field semantics change.
pub const SCENE_VERSION: u32 = 2;
/// Fixed header size in bytes; every u32 array starts 4-byte aligned after it.
pub const SCENE_HEADER_BYTES: usize = 40;
/// `flags` bit 0: entity keys are sorted in JS UTF-16 code-unit order.
pub const SCENE_FLAG_IDS_SORTED: u32 = 1;
/// Label candidates shipped beside the community hubs (degree desc).
pub const SCENE_TOP_K: usize = 12;
/// `node_types` value for entities with no type (or a type past the cap).
pub const SCENE_TYPE_UNTYPED: u8 = 255;
/// Distinct type names the u8 index can address (indices 0..=254).
pub const SCENE_TYPE_LIMIT: usize = 255;

/// Are `ids` sorted in JavaScript string order (UTF-16 code-unit order)?
///
/// Rust `str` ordering compares UTF-8 bytes, which is code-point order; JS
/// compares UTF-16 code units. The two differ only around non-BMP characters
/// (a surrogate pair's lead unit 0xD800–0xDBFF sorts below U+E000–U+FFFF),
/// so the check has to be explicit: the frontend's binary search relies on it.
pub fn ids_sorted_js(ids: &[String]) -> bool {
    ids.windows(2)
        .all(|pair| pair[0].encode_utf16().cmp(pair[1].encode_utf16()).is_le())
}

/// Top-`k` node indices by degree desc, ties by ascending index.
fn top_degrees(degrees: &[u32], k: usize) -> Vec<u32> {
    if k == 0 {
        return Vec::new();
    }
    let mut top: Vec<(u32, u32)> = Vec::with_capacity(k + 1);
    for (i, &degree) in degrees.iter().enumerate() {
        let index = i as u32;
        if top.len() == k {
            let (worst_degree, worst_index) = top[k - 1];
            if degree < worst_degree || (degree == worst_degree && index > worst_index) {
                continue;
            }
        }
        let pos = top.partition_point(|&(td, ti)| td > degree || (td == degree && ti < index));
        top.insert(pos, (degree, index));
        if top.len() > k {
            top.truncate(k);
        }
    }
    top.into_iter().map(|(_, index)| index).collect()
}

/// Build the packed scene for `store`, keyed by `hash` (usually the store's
/// `build_uid`). Base tables only — sections/concepts/FTS are not touched.
pub fn build_scene(store: &KgStore, hash: &str) -> StoreResult<Vec<u8>> {
    let conn = store.connection();

    let mut ids: Vec<String> = Vec::new();
    let mut raw_types: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT entity_key, type FROM entities ORDER BY node")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
            raw_types.push(row.get(1).unwrap_or_default());
        }
    }
    let node_count = ids.len();
    if node_count > u32::MAX as usize {
        return Err(super::StoreError::Data(format!(
            "scene node count {node_count} exceeds u32"
        )));
    }

    let mut degrees = vec![0u32; node_count];
    {
        let mut stmt = conn.prepare(
            "SELECT node, COUNT(*) FROM (
                 SELECT src AS node FROM relations
                 UNION ALL
                 SELECT dst AS node FROM relations
             ) GROUP BY node",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let node: i64 = row.get(0)?;
            let count: i64 = row.get(1)?;
            if node >= 0 && (node as usize) < node_count {
                degrees[node as usize] = count.min(u32::MAX as i64) as u32;
            }
        }
    }

    let top = top_degrees(&degrees, SCENE_TOP_K);

    let edge_count = conn.query_row("SELECT COUNT(*) FROM relations", [], |row| {
        row.get::<_, i64>(0)
    })? as usize;
    let mut edges: Vec<u32> = Vec::with_capacity(edge_count * 2);
    {
        let mut stmt = conn.prepare("SELECT src, dst FROM relations ORDER BY rid")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            edges.push(row.get::<_, i64>(0)? as u32);
            edges.push(row.get::<_, i64>(1)? as u32);
        }
    }

    // Entity keys as one UTF-8 blob plus per-node byte offsets. The frontend
    // decodes a key only when it needs it (labels, popover, id lookup).
    let mut id_offsets: Vec<u32> = Vec::with_capacity(node_count + 1);
    let mut id_blob_len: usize = 0;
    for id in &ids {
        id_offsets.push(id_blob_len as u32);
        id_blob_len += id.len();
    }
    id_offsets.push(id_blob_len as u32);
    if id_blob_len > u32::MAX as usize {
        return Err(super::StoreError::Data(format!(
            "scene id blob {id_blob_len} exceeds u32"
        )));
    }

    // Entity types: one u8 index per node over a sorted unique-name table.
    // Sorting keeps the scene byte-identical across runs; empty names and
    // names past the 255-entry cap collapse to SCENE_TYPE_UNTYPED.
    let mut type_names: Vec<String> = raw_types
        .iter()
        .filter(|t| !t.is_empty())
        .cloned()
        .collect();
    type_names.sort_unstable();
    type_names.dedup();
    if type_names.len() > SCENE_TYPE_LIMIT {
        type_names.truncate(SCENE_TYPE_LIMIT);
    }
    let mut node_types: Vec<u8> = Vec::with_capacity(node_count);
    for raw in &raw_types {
        let index = if raw.is_empty() {
            None
        } else {
            type_names.binary_search(raw).ok()
        };
        node_types.push(match index {
            Some(pos) => pos as u8,
            None => SCENE_TYPE_UNTYPED,
        });
    }
    let mut type_offsets: Vec<u32> = Vec::with_capacity(type_names.len() + 1);
    let mut type_blob_len: usize = 0;
    for name in &type_names {
        type_offsets.push(type_blob_len as u32);
        type_blob_len += name.len();
    }
    type_offsets.push(type_blob_len as u32);

    let noise_floor: f64 = {
        let raw: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'noise_floor'", [], |row| {
                row.get(0)
            })
            .optional()?;
        raw.and_then(|value| serde_json::from_str::<Option<f64>>(&value).ok())
            .flatten()
            .unwrap_or(0.0)
    };

    let hash_bytes = hash.as_bytes();
    let flags = if ids_sorted_js(&ids) {
        SCENE_FLAG_IDS_SORTED
    } else {
        0
    };

    let total = SCENE_HEADER_BYTES
        + degrees.len() * 4
        + top.len() * 4
        + edges.len() * 4
        + id_offsets.len() * 4
        + node_types.len()
        + type_offsets.len() * 4
        + hash_bytes.len()
        + id_blob_len
        + type_blob_len;
    let mut out = Vec::with_capacity(total);

    let push_u32 = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
    push_u32(&mut out, SCENE_MAGIC);
    push_u32(&mut out, SCENE_VERSION);
    push_u32(&mut out, flags);
    push_u32(&mut out, node_count as u32);
    push_u32(&mut out, edge_count as u32);
    push_u32(&mut out, top.len() as u32);
    push_u32(&mut out, hash_bytes.len() as u32);
    push_u32(&mut out, type_names.len() as u32);
    out.extend_from_slice(&noise_floor.to_le_bytes());
    for degree in &degrees {
        push_u32(&mut out, *degree);
    }
    for index in &top {
        push_u32(&mut out, *index);
    }
    for edge in &edges {
        push_u32(&mut out, *edge);
    }
    for offset in &id_offsets {
        push_u32(&mut out, *offset);
    }
    out.extend_from_slice(&node_types);
    for offset in &type_offsets {
        push_u32(&mut out, *offset);
    }
    out.extend_from_slice(hash_bytes);
    for id in &ids {
        out.extend_from_slice(id.as_bytes());
    }
    for name in &type_names {
        out.extend_from_slice(name.as_bytes());
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{KnowledgeGraph, SectionInfo};
    use crate::store::{KgStore, OpenMode};
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "docfoo-scene-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::default();
        kg.add_entity("DEVICE_a", "A", "DEVICE", "x", "S1", None);
        kg.add_entity("CONCEPT_b", "B", "CONCEPT", "y", "S1", None);
        kg.add_entity("CONCEPT_iso", "Iso", "CONCEPT", "z", "S1", None);
        kg.add_relation("DEVICE_a", "CONCEPT_b", "USES", "S1", None);
        kg.add_relation("CONCEPT_b", "DEVICE_a", "ENABLES", "S1", None);
        kg.sections.insert(
            "S1".into(),
            SectionInfo {
                topic: Some("[CO] Chapter 3".into()),
                entity_ids: vec!["DEVICE_a".into(), "CONCEPT_b".into()],
                text: "t".into(),
                source_doc: "d.md".into(),
                ..Default::default()
            },
        );
        kg.noise_floor = Some(0.031);
        kg
    }

    fn u32_at(buf: &[u8], index: usize) -> u32 {
        let at = index * 4;
        u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn scene_round_trips_degrees_edges_and_ids() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("graph.sqlite");
        KgStore::write_full(&sample(), &path).unwrap();
        let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
        let hash = store.build_uid().unwrap();
        let buf = build_scene(&store, &hash).unwrap();

        assert_eq!(u32_at(&buf, 0), SCENE_MAGIC);
        assert_eq!(u32_at(&buf, 1), SCENE_VERSION);
        assert_eq!(u32_at(&buf, 2) & SCENE_FLAG_IDS_SORTED, SCENE_FLAG_IDS_SORTED);
        assert_eq!(u32_at(&buf, 3), 3, "node count");
        assert_eq!(u32_at(&buf, 4), 2, "edge count");
        assert_eq!(u32_at(&buf, 5), 3, "top count");
        assert_eq!(u32_at(&buf, 6) as usize, hash.len());
        assert_eq!(u32_at(&buf, 7), 2, "type count");
        let noise = f64::from_le_bytes(buf[32..40].try_into().unwrap());
        assert!((noise - 0.031).abs() < 1e-9);

        let n = 3usize;
        let e = 2usize;
        let top = 3usize;
        let type_count = 2usize;
        let hash_len = hash.len();
        let degrees_at = SCENE_HEADER_BYTES / 4;
        let top_at = degrees_at + n;
        let edges_at = top_at + top;
        let offsets_at = edges_at + e * 2;
        // ids: CONCEPT_b, CONCEPT_iso, DEVICE_a (BTreeMap order)
        assert_eq!(u32_at(&buf, degrees_at), 2);
        assert_eq!(u32_at(&buf, degrees_at + 1), 0);
        assert_eq!(u32_at(&buf, degrees_at + 2), 2);
        assert_eq!(u32_at(&buf, top_at), 0);
        assert_eq!(u32_at(&buf, top_at + 1), 2);
        assert_eq!(u32_at(&buf, top_at + 2), 1);
        assert_eq!(u32_at(&buf, edges_at), 2);
        assert_eq!(u32_at(&buf, edges_at + 1), 0);
        assert_eq!(u32_at(&buf, edges_at + 2), 0);
        assert_eq!(u32_at(&buf, edges_at + 3), 2);

        // node_types: CONCEPT=0, CONCEPT=0, DEVICE=1 (sorted type table).
        let node_types_at = offsets_at * 4 + (n + 1) * 4;
        assert_eq!(&buf[node_types_at..node_types_at + n], &[0, 0, 1]);
        // type_offsets: [0, 7, 13] over "CONCEPTDEVICE".
        let type_offsets_at = node_types_at + n;
        let u32_bytes = |at: usize| u32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
        assert_eq!(u32_bytes(type_offsets_at), 0);
        assert_eq!(u32_bytes(type_offsets_at + 4), 7);
        assert_eq!(u32_bytes(type_offsets_at + 8), 13);

        let blob_at = type_offsets_at + (type_count + 1) * 4 + hash_len;
        let ids_blob = "CONCEPT_bCONCEPT_isoDEVICE_a";
        assert_eq!(
            String::from_utf8(buf[blob_at..blob_at + ids_blob.len()].to_vec()).unwrap(),
            ids_blob
        );
        assert_eq!(
            String::from_utf8(buf[blob_at + ids_blob.len()..].to_vec()).unwrap(),
            "CONCEPTDEVICE"
        );
        assert_eq!(u32_at(&buf, offsets_at), 0);
        assert_eq!(u32_at(&buf, offsets_at + 1), 9);
        assert_eq!(u32_at(&buf, offsets_at + 2), 20);
        assert_eq!(u32_at(&buf, offsets_at + 3), ids_blob.len() as u32);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn js_sorted_flag_detects_surrogate_order_difference() {
        // Rust code-point order: U+FFFF < U+10000. JS UTF-16 order: the
        // surrogate pair starts with 0xD800 < 0xFFFF, so JS sees U+10000 first.
        let ids = vec!["\u{FFFF}".to_string(), "\u{10000}".to_string()];
        assert!(!ids_sorted_js(&ids));
        let plain = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert!(ids_sorted_js(&plain));
    }

    #[test]
    fn top_degrees_matches_sort_with_ties_by_index() {
        let degrees = [1u32, 5, 5, 0, 5, 0];
        assert_eq!(top_degrees(&degrees, 12), vec![1, 2, 4, 0, 3, 5]);
        assert_eq!(top_degrees(&degrees, 2), vec![1, 2]);
        assert!(top_degrees(&degrees, 0).is_empty());
    }
}

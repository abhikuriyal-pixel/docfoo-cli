//! DocFoo-specific corpus handling: turning the Resources folder into
//! indexable documents.
//!
//! A *file card* in DocFoo is one complete resource (e.g.
//! `multi_column_1/`) whose content is its markdown (`content.md`); assets and
//! cache folders are just storage. So every `*.md` under `db/resources`
//! counts as one document, keyed by its rel path — which covers both card
//! markdown and loose markdown added via "Add markdown".

use crate::build::InputDoc;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// One discovered markdown file.
#[derive(Clone, Debug)]
pub struct CorpusFile {
    /// Slash-separated path relative to the resources root.
    pub rel: String,
    pub markdown: String,
    /// SHA-256 of the raw file bytes at load time (staleness fingerprint).
    pub source_hash: String,
}

/// Recursively collect all markdown under `root`, sorted by rel path for
/// deterministic ordering everywhere downstream. Unreadable files are
/// skipped (they cannot be indexed anyway).
pub fn gather(root: &Path) -> Vec<CorpusFile> {
    let mut out = vec![];
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<CorpusFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("md")).unwrap_or(false) {
            let Ok(markdown) = std::fs::read_to_string(&path) else { continue };
            let Some(rel) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) else { continue };
            out.push(CorpusFile {
                rel: rel.replace('\\', "/"),
                source_hash: hash_bytes(markdown.as_bytes()),
                markdown,
            });
        }
    }
}

/// SHA-256 hex (64 chars) of raw bytes.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Streaming SHA-256 hex of a file's raw bytes — the staleness check hashes
/// sources without holding them all in memory.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Derive short uppercase doc tags (`[MC1]`, `[SC2]`, …) for each rel path,
/// deterministically deduped in listing order.
///
/// Rule: split the *card name* (parent folder, else file stem) on
/// non-alphanumerics; take the initial letter of up to two word-parts plus
/// trailing digits (`multi_column_1` -> `MC1`, `single_column_2` -> `SC2`);
/// a lone word contributes up to four characters (`note` -> `NOTE`,
/// `ocr` -> `OCR`). Collisions get numeric suffixes in sorted-path order.
pub fn tags_for(rels: &[String]) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    rels.iter()
        .map(|rel| {
            let base = derive_tag(rel);
            let n = counts.entry(base.clone()).or_insert(0);
            *n += 1;
            if *n == 1 { base } else { format!("{base}{n}") }
        })
        .collect()
}

fn derive_tag(rel: &str) -> String {
    let stem = rel.rsplit_once('.').map_or(rel, |(a, _)| a);
    let parts: Vec<&str> = stem.split('/').filter(|p| !p.is_empty()).collect();
    // Prefer the card folder name when nested (`multi_column_1/content.md`
    // describes itself through the card, not through every card's "ocr").
    let subject = if parts.len() >= 2 {
        parts[parts.len() - 2]
    } else {
        parts.first().copied().unwrap_or("")
    };

    let words: Vec<&str> = subject
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();

    let mut tag = String::new();
    if words.len() <= 1 {
        let w = words.first().copied().unwrap_or("");
        tag = w.chars().filter(char::is_ascii_alphanumeric)
            .take(4).collect::<String>().to_uppercase();
    } else {
        for w in words.iter().take(2) {
            if let Some(c) = w.chars().find(char::is_ascii_alphabetic) {
                tag.push(c.to_ascii_uppercase());
            }
        }
        // trailing digits give same-named cards their identity
        if let Some(last) = words.last() {
            let digits: String = last.chars().filter(char::is_ascii_digit).collect();
            tag.push_str(&digits);
        }
    }
    if tag.is_empty() { "DOC".to_string() } else { tag }
}

/// Convenience: gather + tag + wrap into build inputs.
pub fn load_documents(root: &Path) -> Vec<InputDoc> {
    let files = gather(root);
    let rels: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();
    let tags = tags_for(&rels);
    files.into_iter().zip(tags).map(|(f, tag)| InputDoc {
        name: f.rel,
        tag,
        markdown: f.markdown,
        source_hash: f.source_hash,
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_follow_the_card_identity_rule() {
        let rels = [
            "multi_column_1/content.md".to_string(),
            "multi_column_2/content.md".to_string(),
            "single_column_1/content.md".to_string(),
            "single_column_2/content.md".to_string(),
            "note.md".to_string(),
            "Study notes/sub.md".to_string(),
        ];
        assert_eq!(
            tags_for(&rels),
            ["MC1", "MC2", "SC1", "SC2", "NOTE", "SN"]
        );
    }

    #[test]
    fn colliding_tags_get_dedup_suffixes() {
        let rels = [
            "tools/note.md".to_string(),  // TOOL
            "toolx/note.md".to_string(),  // TOOL -> collides
            "trains/a.md".to_string(),    // TRAI (4-char cap)
        ];
        let tags = tags_for(&rels);
        assert_eq!(tags, ["TOOL", "TOOL2", "TRAI"]);
    }

    #[test]
    fn degenerate_names_fall_back_to_doc() {
        assert_eq!(tags_for(&["###/++.md".to_string()]), ["DOC"]);
    }

    #[test]
    fn source_hash_sees_any_byte_change() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abc "));
        assert_ne!(hash_bytes(b"a\r\nb"), hash_bytes(b"a\nb"));
    }

    #[test]
    fn hash_file_matches_hash_bytes() {
        let path = std::env::temp_dir().join(format!("docfoo-hash-{}.md", std::process::id()));
        std::fs::write(&path, b"line one\nline two\n").unwrap();
        assert_eq!(hash_file(&path).unwrap(), hash_bytes(b"line one\nline two\n"));
        let _ = std::fs::remove_file(&path);
    }
}

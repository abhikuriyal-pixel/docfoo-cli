//! Recursive resource tree — port of `src-tauri/src/resources/tree.rs`.
//!
//! Directories carry their children, so the real folder structure (e.g.
//! `doc/assets/figure_01.png`) is preserved. Folders report recursive totals
//! and the image rels under their `assets/` directory.

use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceEntry {
    /// File or folder name.
    pub name: String,
    /// Path relative to the resources root.
    pub rel: String,
    /// `dir`, `md`, `image`, `notebook` or `file`.
    pub kind: &'static str,
    /// Total bytes (folders: sum of everything inside).
    pub size: u64,
    /// Number of files inside (folders only).
    pub files: usize,
    /// Newest modification timestamp in unix seconds.
    pub modified: u64,
    /// Contents (folders only; empty for files).
    pub children: Vec<ResourceEntry>,
    /// Image rels under this folder's `assets/` directory.
    pub figures: Vec<String>,
}

/// Classify a file by extension.
pub fn resource_kind(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_lowercase());
    match ext.as_deref() {
        Some("md" | "markdown") => "md",
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") => "image",
        _ => "file",
    }
}

/// True when `rel` is a `.py` file inside a `notebooks/` folder.
fn is_notebook_rel(rel: &str) -> bool {
    let rel = rel.replace('\\', "/");
    if !rel.to_ascii_lowercase().ends_with(".py") {
        return false;
    }
    match rel.rsplit_once('/') {
        Some((parent, _)) => parent == "notebooks" || parent.ends_with("/notebooks"),
        None => false,
    }
}

fn file_kind(path: &Path, rel: &str) -> &'static str {
    if is_notebook_rel(rel) {
        return "notebook";
    }
    resource_kind(path)
}

fn collect_images(entry: &ResourceEntry, out: &mut Vec<String>) {
    if entry.kind == "image" {
        out.push(entry.rel.clone());
    } else if entry.kind == "dir" {
        for child in &entry.children {
            collect_images(child, out);
        }
    }
}

/// List everything under `root` as a tree. Every directory lists its
/// subfolders first (sorted), then its files (sorted).
pub fn scan_resources(root: &Path) -> Vec<ResourceEntry> {
    scan_level(root, "")
}

fn scan_level(dir: &Path, prefix: &str) -> Vec<ResourceEntry> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == ".podcast" {
            continue;
        }
        if entry.metadata().map(|meta| meta.is_dir()).unwrap_or(false) {
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();

    for name in dirs {
        let path = dir.join(&name);
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let children = scan_level(&path, &rel);
        let mut total = 0u64;
        let mut count = 0usize;
        let mut newest = 0u64;
        for child in &children {
            total += child.size;
            count += child.files + usize::from(child.kind != "dir");
            if child.modified > newest {
                newest = child.modified;
            }
        }
        let mut figures = Vec::new();
        for child in &children {
            if child.kind == "dir" && child.name == "assets" {
                collect_images(child, &mut figures);
            }
        }
        out.push(ResourceEntry {
            name,
            rel,
            kind: "dir",
            size: total,
            files: count,
            modified: newest,
            children,
            figures,
        });
    }

    for name in files {
        if name == ".podcast" {
            continue;
        }
        let path = dir.join(&name);
        let Some(meta) = path.metadata().ok() else {
            continue;
        };
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        out.push(ResourceEntry {
            name,
            rel: rel.clone(),
            kind: file_kind(&path, &rel),
            size: meta.len(),
            files: 0,
            modified: mtime(&meta),
            children: Vec::new(),
            figures: Vec::new(),
        });
    }
    out
}

fn mtime(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_preserves_structure_and_figures() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Book/assets")).unwrap();
        std::fs::write(temp.path().join("Book/content.md"), "hello").unwrap();
        std::fs::write(temp.path().join("Book/assets/figure_1.png"), b"png").unwrap();
        std::fs::write(temp.path().join("notes.md"), "note").unwrap();
        std::fs::create_dir_all(temp.path().join(".hidden")).unwrap();
        std::fs::write(temp.path().join(".hidden/x.md"), "hidden").unwrap();

        let tree = scan_resources(temp.path());
        assert_eq!(tree.len(), 2);
        let book = tree.iter().find(|entry| entry.name == "Book").unwrap();
        assert_eq!(book.kind, "dir");
        assert_eq!(book.files, 2);
        assert_eq!(book.figures, vec!["Book/assets/figure_1.png"]);
        assert_eq!(book.children.len(), 2);
        // hidden entries are skipped
        assert!(!tree.iter().any(|entry| entry.name == ".hidden"));
    }

    #[test]
    fn notebooks_are_classified() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("notebooks")).unwrap();
        std::fs::write(temp.path().join("notebooks/demo.py"), "print(1)").unwrap();
        let tree = scan_resources(temp.path());
        let notebooks = tree.iter().find(|entry| entry.name == "notebooks").unwrap();
        assert_eq!(notebooks.children[0].kind, "notebook");
    }
}

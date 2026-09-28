//! Collection zip validation + install — port of
//! `src-tauri/src/collections/zip_util.rs` (read path only).

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::kg::paths;
use crate::util::{sanitize_component, Quota, QuotaError, QuotaState};

/// Zip-bomb guards for shared collections (gallery items are small folders).
const COLLECTION_QUOTA: Quota = Quota {
    max_entries: 4_096,
    max_entry_bytes: 512 * 1024 * 1024,
    max_total_bytes: 2 * 1024 * 1024 * 1024,
    max_ratio: 1_000,
    ratio_min_bytes: 8 * 1024 * 1024,
};

fn quota_error(kind: QuotaError) -> String {
    match kind {
        QuotaError::TooManyEntries => {
            "This collection has too many files and was not installed.".to_string()
        }
        QuotaError::EntryTooLarge => {
            "This collection contains a file that is too large and was not installed.".to_string()
        }
        QuotaError::TotalTooLarge => {
            "This collection is too large and was not installed.".to_string()
        }
        QuotaError::CompressionRatio => {
            "This collection looks like a zip bomb and was not installed.".to_string()
        }
    }
}

fn collection_error() -> String {
    "Not a valid DocFoo collection zip.".to_string()
}

fn read_manifest(archive: &mut ::zip::ZipArchive<File>) -> Result<Value, String> {
    let mut text = String::new();
    let mut entry = archive.by_name("manifest.json").map_err(|_| collection_error())?;
    entry
        .read_to_string(&mut text)
        .map_err(|_| collection_error())?;
    let manifest = serde_json::from_str::<Value>(&text).map_err(|_| collection_error())?;
    if manifest["app"].as_str() != Some("docfoo") {
        return Err(collection_error());
    }
    Ok(manifest)
}

fn graph_components(rel: &str) -> Vec<String> {
    rel.replace('\\', "/")
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .map(|component| {
            let safe = sanitize_component(component);
            if matches!(safe.as_str(), "." | "..") {
                "shared".to_string()
            } else {
                safe
            }
        })
        .collect()
}

fn collection_entry_is_safe(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && !name.contains("..")
        && !Path::new(name).is_absolute()
        && !name
            .split('/')
            .any(|component| matches!(component, ".." | "."))
}

/// Install a downloaded collection zip. `force` replaces an existing resource
/// folder instead of suffixing; `name_override` renames a resource install.
/// Returns the installed name (a resource folder name, or a graph scope where
/// `""` means the top-level graph).
pub fn extract(
    zip_path: &Path,
    workspace: &Path,
    expect_kind: &str,
    force: bool,
    name_override: Option<&str>,
) -> Result<String, String> {
    let file = File::open(zip_path).map_err(|e| format!("could not open collection zip: {e}"))?;
    let mut archive =
        ::zip::ZipArchive::new(file).map_err(|_| "Not a valid DocFoo collection zip.".to_string())?;
    let manifest = read_manifest(&mut archive)?;
    if manifest["kind"].as_str() != Some(expect_kind) {
        return Err(collection_error());
    }

    match expect_kind {
        "resource" => {
            let mut quota = QuotaState::default();
            for index in 0..archive.len() {
                let entry = archive
                    .by_index(index)
                    .map_err(|_| "Unexpected file in collection zip.".to_string())?;
                let name = entry.name();
                if name == "manifest.json" {
                    continue;
                }
                if let Err(kind) =
                    COLLECTION_QUOTA.check(&mut quota, entry.size(), entry.compressed_size())
                {
                    return Err(quota_error(kind));
                }
                if entry.is_dir()
                    || !collection_entry_is_safe(name)
                    || !(name == "content.md"
                        || name.strip_prefix("assets/").is_some_and(|asset| {
                            !asset.is_empty()
                                && !asset.contains('/')
                                && asset != "."
                                && asset != ".."
                        }))
                {
                    return Err("Unexpected file in collection zip.".to_string());
                }
            }

            let requested = name_override
                .map(str::to_string)
                .unwrap_or_else(|| manifest["name"].as_str().unwrap_or("").to_string());
            let sanitized = sanitize_component(&requested);
            let name = if matches!(sanitized.as_str(), "." | "..") {
                "shared".to_string()
            } else {
                sanitized
            };
            let resources = workspace.join("resources");
            let installed_name = if force {
                let target = resources.join(&name);
                if target.exists() {
                    fs::remove_dir_all(&target).map_err(|e| {
                        format!("could not replace the existing resource: {e}")
                    })?;
                }
                name.clone()
            } else {
                let mut installed = name.clone();
                let mut n = 2;
                while resources.join(&installed).exists() {
                    installed = format!("{name} ({n})");
                    n += 1;
                }
                installed
            };
            let target = resources.join(&installed_name);
            fs::create_dir_all(&target)
                .map_err(|e| format!("could not create the resource folder: {e}"))?;

            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|_| "Unexpected file in collection zip.".to_string())?;
                let name = entry.name().to_string();
                if name == "manifest.json" {
                    continue;
                }
                let output = if name == "content.md" {
                    target.join("content.md")
                } else {
                    let asset = name.strip_prefix("assets/").unwrap_or_default();
                    let assets = target.join("assets");
                    fs::create_dir_all(&assets)
                        .map_err(|e| format!("could not create the assets folder: {e}"))?;
                    assets.join(asset)
                };
                let mut out = File::create(&output)
                    .map_err(|e| format!("could not write {}: {e}", output.display()))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| format!("could not write {}: {e}", output.display()))?;
            }
            Ok(installed_name)
        }
        "kg" => {
            let mut graph_count = 0;
            let mut quota = QuotaState::default();
            for index in 0..archive.len() {
                let entry = archive
                    .by_index(index)
                    .map_err(|_| "Unexpected file in collection zip.".to_string())?;
                let name = entry.name();
                if name == "manifest.json" {
                    continue;
                }
                if let Err(kind) =
                    COLLECTION_QUOTA.check(&mut quota, entry.size(), entry.compressed_size())
                {
                    return Err(quota_error(kind));
                }
                if name == "graph.json" {
                    return Err(
                        "This collection holds the old JSON graph format — ask the author to rebuild and share it with a current DocFoo."
                            .to_string(),
                    );
                }
                if entry.is_dir() || !collection_entry_is_safe(name) || name != "graph.sqlite" {
                    return Err("Unexpected file in collection zip.".to_string());
                }
                graph_count += 1;
            }
            if graph_count == 0 {
                return Err("Unexpected file in collection zip.".to_string());
            }

            let rel = manifest["rel"].as_str().unwrap_or("");
            let mut target = workspace.join("graphs");
            for component in graph_components(rel) {
                target.push(component);
            }
            fs::create_dir_all(&target)
                .map_err(|e| format!("could not create the graph folder: {e}"))?;
            let output = target.join(if rel.is_empty() {
                "top-level.sqlite"
            } else {
                "graph.sqlite"
            });
            // Never leave a stale WAL/journal next to the freshly installed file.
            paths::remove_store_files(&output);
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|_| "Unexpected file in collection zip.".to_string())?;
                if entry.name() == "graph.sqlite" {
                    let mut out = File::create(&output)
                        .map_err(|e| format!("could not write {}: {e}", output.display()))?;
                    std::io::copy(&mut entry, &mut out)
                        .map_err(|e| format!("could not write {}: {e}", output.display()))?;
                }
            }
            Ok(if rel.is_empty() {
                "top-level".to_string()
            } else {
                rel.to_string()
            })
        }
        _ => Err(collection_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = ::zip::ZipWriter::new(file);
        let options = ::zip::write::SimpleFileOptions::default()
            .compression_method(::zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn installs_a_resource_collection() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("item.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "manifest.json",
                    br#"{"app":"docfoo","kind":"resource","name":"Shared Book"}"#,
                ),
                ("content.md", b"hello"),
                ("assets/figure_1.png", b"png"),
            ],
        );
        let workspace = temp.path().join("ws");
        let name = extract(&zip_path, &workspace, "resource", false, None).unwrap();
        assert_eq!(name, "Shared Book");
        assert!(workspace.join("resources/Shared Book/content.md").is_file());
        assert!(workspace.join("resources/Shared Book/assets/figure_1.png").is_file());
    }

    #[test]
    fn suffixes_unless_forced() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("item.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "manifest.json",
                    br#"{"app":"docfoo","kind":"resource","name":"Shared"}"#,
                ),
                ("content.md", b"hello"),
            ],
        );
        let workspace = temp.path().join("ws");
        assert_eq!(extract(&zip_path, &workspace, "resource", false, None).unwrap(), "Shared");
        assert_eq!(
            extract(&zip_path, &workspace, "resource", false, None).unwrap(),
            "Shared (2)"
        );
        assert_eq!(
            extract(&zip_path, &workspace, "resource", true, None).unwrap(),
            "Shared"
        );
    }

    #[test]
    fn installs_a_kg_collection() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("item.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "manifest.json",
                    br#"{"app":"docfoo","kind":"kg","name":"Paper graph","rel":"papers/ml"}"#,
                ),
                ("graph.sqlite", b"SQLite format 3\0"),
            ],
        );
        let workspace = temp.path().join("ws");
        let name = extract(&zip_path, &workspace, "kg", false, None).unwrap();
        assert_eq!(name, "papers/ml");
        assert!(workspace.join("graphs/papers/ml/graph.sqlite").is_file());
    }

    #[test]
    fn rejects_the_retired_json_graph_format() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("item.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "manifest.json",
                    br#"{"app":"docfoo","kind":"kg","name":"Old graph","rel":""}"#,
                ),
                ("graph.json", b"{}"),
            ],
        );
        let workspace = temp.path().join("ws");
        let error = extract(&zip_path, &workspace, "kg", false, None).unwrap_err();
        assert!(error.contains("old JSON graph format"), "error: {error}");
    }

    #[test]
    fn rejects_unsafe_entries() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("item.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "manifest.json",
                    br#"{"app":"docfoo","kind":"resource","name":"Bad"}"#,
                ),
                ("../evil.md", b"boom"),
            ],
        );
        let workspace = temp.path().join("ws");
        assert!(extract(&zip_path, &workspace, "resource", false, None).is_err());
    }

    #[test]
    fn name_override_renames_the_resource() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("item.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "manifest.json",
                    br#"{"app":"docfoo","kind":"resource","name":"Original"}"#,
                ),
                ("content.md", b"hello"),
            ],
        );
        let workspace = temp.path().join("ws");
        let name = extract(&zip_path, &workspace, "resource", false, Some("Renamed")).unwrap();
        assert_eq!(name, "Renamed");
        assert!(workspace.join("resources/Renamed/content.md").is_file());
    }
}

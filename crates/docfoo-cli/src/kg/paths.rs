//! Graph path rules and scope validation.
//!
//! `""` (whole library) → `graphs/top-level.json`; `"papers/ml"` →
//! `graphs/papers/ml/graph.json`. Port of `src-tauri/src/kg/mod.rs` path rules.

use std::path::{Path, PathBuf};

use crate::error::{CliError, Result};
use crate::workspace::Workspace;

/// Normalize a `--scope` value and reject anything that escapes `resources/`.
pub fn normalize_scope(scope: &str) -> Result<String> {
    let raw = scope.trim();
    // Backslashes are path separators to `graph_path` (`replace('\\', "/")`),
    // so a scope like `papers\..\..` would escape on Unix where `Path` does
    // not treat `\` as a separator. Colons cover Windows drive paths
    // (`C:\evil`, `C:evil`), which are absolute/drive-relative on Windows and
    // harmless folder names elsewhere — reject both on every platform.
    if raw.starts_with('/')
        || raw.starts_with('\\')
        || raw.contains('\\')
        || raw.contains(':')
    {
        return Err(CliError::Usage(format!("invalid resource scope \"{scope}\"")));
    }
    let dir = raw.trim_end_matches('/').to_string();
    if dir.is_empty() {
        return Ok(String::new());
    }
    if is_safe_rel(&dir) {
        Ok(dir)
    } else {
        Err(CliError::Usage(format!("invalid resource scope \"{scope}\"")))
    }
}

fn is_safe_rel(rel: &str) -> bool {
    let path = Path::new(rel);
    if path.is_absolute() {
        return false;
    }
    path.components().all(|component| {
        matches!(component, std::path::Component::Normal(_))
    })
}

/// Path of the graph file for `scope`.
pub fn graph_path(workspace: &Workspace, scope: &str) -> PathBuf {
    let graphs = workspace.graphs_dir();
    if scope.is_empty() {
        graphs.join("top-level.json")
    } else {
        graphs
            .join(scope.replace('\\', "/"))
            .join("graph.json")
    }
}

/// Every built graph, as scopes (`""` = top level). Port of `kg_list_graphs`.
pub fn list_built(workspace: &Workspace) -> Vec<String> {
    let mut built = Vec::new();
    let root = workspace.graphs_dir();
    if root.join("top-level.json").is_file() {
        built.push(String::new());
    }
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if path.join("graph.json").is_file() {
                out.push(rel.clone());
            }
            walk(&path, &rel, out);
        }
    }
    walk(&root, "", &mut built);
    built.sort();
    built
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    fn workspace(root: &Path) -> Workspace {
        Workspace {
            root: root.to_path_buf(),
            agent_dir: root.join(".agent"),
            models_dir: root.join("models"),
        }
    }

    #[test]
    fn scopes_stay_inside_the_library() {
        assert_eq!(normalize_scope("papers/ml").unwrap(), "papers/ml");
        assert_eq!(normalize_scope("").unwrap(), "");
        assert_eq!(normalize_scope("  papers/ml  ").unwrap(), "papers/ml");
        assert!(normalize_scope("/papers/ml/").is_err());
        assert!(normalize_scope("..").is_err());
        assert!(normalize_scope("papers/../..").is_err());
        assert!(normalize_scope("C:\\evil").is_err());
        assert!(normalize_scope("C:evil").is_err());
        assert!(normalize_scope("papers\\..\\..").is_err());
    }

    #[test]
    fn graph_paths_follow_the_scope() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        assert_eq!(
            graph_path(&workspace, ""),
            temp.path().join("graphs").join("top-level.json")
        );
        assert_eq!(
            graph_path(&workspace, "papers/ml"),
            temp.path().join("graphs").join("papers/ml").join("graph.json")
        );
    }

    #[test]
    fn list_built_finds_both_levels() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        std::fs::create_dir_all(temp.path().join("graphs/papers/ml")).unwrap();
        std::fs::write(temp.path().join("graphs/top-level.json"), "{}").unwrap();
        std::fs::write(temp.path().join("graphs/papers/ml/graph.json"), "{}").unwrap();
        assert_eq!(list_built(&workspace), vec!["", "papers/ml"]);
    }
}

//! Optional `kg-chats/` persistence for `docfoo kg --query --save`.
//!
//! App-compatible shape (port of `src-tauri/src/kg/chats.rs`): a `kg-<id>`
//! folder with `meta.json` + `kg-history.json`, plus the
//! `.agent/current-kg-chat.json` pointer. The CLI only ever creates a new chat
//! when `--save` is passed; default queries are ephemeral.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::Result;
use crate::util::atomic_write_json;
use crate::workspace::Workspace;

pub struct SavedChat {
    pub id: String,
    pub dir: PathBuf,
}

/// Persist one Q&A turn in a fresh `kg-<id>` chat.
pub fn save_query(
    workspace: &Workspace,
    query: &str,
    answer: &str,
    sources: &Value,
    routing: &Value,
    total_secs: f64,
) -> Result<SavedChat> {
    let id = format!("kg-{}", session_id());
    let dir = workspace.kg_chats_dir().join(&id);
    std::fs::create_dir_all(&dir)?;
    let now = now_ms();
    let meta = json!({
        "id": id,
        "title": compact(query, 60),
        "created": now,
        "updated": now,
    });
    atomic_write_json(&dir.join("meta.json"), &meta)?;
    let history = json!({
        "entries": [{
            "q": query,
            "a": answer,
            "sources": sources,
            "routing": routing,
            "secs": total_secs,
            "ts": now,
            "trace": [],
        }]
    });
    atomic_write_json(&dir.join("kg-history.json"), &history)?;
    let pointer = workspace.agent_dir.join("current-kg-chat.json");
    atomic_write_json(&pointer, &json!({ "dir": dir.to_string_lossy() }))?;
    Ok(SavedChat { id, dir })
}

fn session_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let millis = now_ms();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{millis:x}{:x}{:x}", std::process::id(), seq)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn compact(text: &str, max: usize) -> String {
    let compact: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > max {
        compact.chars().take(max).collect::<String>() + "…"
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn save_query_writes_app_compatible_files() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = Workspace {
            root: temp.path().to_path_buf(),
            agent_dir: temp.path().join(".agent"),
            models_dir: temp.path().join("models"),
        };
        let saved = save_query(
            &workspace,
            "How is apples connected to mangoes?",
            "Answer [a/content.md:1].",
            &json!([{"doc":"a/content.md","start_line":1,"end_line":1}]),
            &json!({}),
            1.5,
        )
        .unwrap();
        assert!(saved.dir.join("meta.json").is_file());
        assert!(saved.dir.join("kg-history.json").is_file());
        let history: Value =
            serde_json::from_str(&std::fs::read_to_string(saved.dir.join("kg-history.json")).unwrap())
                .unwrap();
        assert_eq!(history["entries"][0]["q"], "How is apples connected to mangoes?");
        assert!(workspace.agent_dir.join("current-kg-chat.json").is_file());
    }
}

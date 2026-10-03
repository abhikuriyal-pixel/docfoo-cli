//! Knowledge-graph indexing and querying.
//!
//! Thin, Tauri-free ports of the desktop app's `src-tauri/src/kg` drivers. The
//! heavy lifting lives in the reused `docfoo-kg` crate; these modules own the
//! workspace paths, settings file, sidecar-backed `ChatClient`, Jev decision
//! bridge, and optional `kg-chats` persistence.

pub mod bridge;
pub mod chats;
pub mod decision;
pub mod index;
pub mod paths;
pub mod query;
pub mod settings;
pub mod snapshot;

//! Read-only resource library access.
//!
//! Ports of `src-tauri/src/resources/tree.rs` and the bounded read helpers in
//! `agent/read-tools.ts` + `agent/resource-format.ts`. Everything is a plain
//! function of the workspace so it can be unit-tested.

pub mod read;
pub mod tree;
pub mod vis;

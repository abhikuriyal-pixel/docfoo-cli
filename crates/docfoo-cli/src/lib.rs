//! docfoo — barebones DocFoo CLI.
//!
//! The crate is split into a library (everything testable) and the `docfoo`
//! binary (argument parsing + dispatch). Integration tests import this crate
//! directly; the sidecar client tests spawn the `fake-sidecar` fixture binary.

// Stage 1.3 ships modules ahead of their consumers. Remove this allow once
// Stages 2–5 land.
#![allow(dead_code)]

pub mod backup;
pub mod citations;
pub mod cli;
pub mod collections;
pub mod commands;
pub mod config;
pub mod error;
pub mod kg;
pub mod notes;
pub mod output;
pub mod render;
pub mod resources;
pub mod scan;
pub mod setup;
pub mod sidecar;
pub mod util;
pub mod workspace;

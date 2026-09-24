//! docfoo — barebones DocFoo CLI.
//!
//! The crate is split into a library (everything testable) and the `docfoo`
//! binary (argument parsing + dispatch). Integration tests import this crate
//! directly; the sidecar client tests spawn the `fake-sidecar` fixture binary.

// Stage 1.2 ships modules ahead of their consumers. Remove this allow once
// Stages 1.3–2 land.
#![allow(dead_code)]

pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod output;
pub mod sidecar;
pub mod util;
pub mod workspace;

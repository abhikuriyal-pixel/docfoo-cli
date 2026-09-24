//! CLI error type and process exit-code mapping.

use std::fmt;

/// One user-facing failure. `code()` is the stable machine-readable code used
/// in the JSON error envelope; `exit_code()` is the process status.
#[derive(Debug)]
pub enum CliError {
    /// Bad flags or arguments (exit 2).
    Usage(String),
    /// A requested file, resource, graph or note does not exist.
    NotFound(String),
    /// The command exists but its stage has not landed yet.
    NotImplemented(String),
    /// A general runtime failure.
    Message(String),
    /// Filesystem failure.
    Io(std::io::Error),
    /// JSON parse/serialize failure.
    Json(serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CliError>;

impl CliError {
    pub fn code(&self) -> &'static str {
        match self {
            CliError::Usage(_) => "usage",
            CliError::NotFound(_) => "not_found",
            CliError::NotImplemented(_) => "not_implemented",
            CliError::Message(_) => "error",
            CliError::Io(_) => "io",
            CliError::Json(_) => "json",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            _ => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(message)
            | CliError::NotFound(message)
            | CliError::NotImplemented(message)
            | CliError::Message(message) => write!(f, "{message}"),
            CliError::Io(error) => write!(f, "{error}"),
            CliError::Json(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        CliError::Io(error)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        CliError::Json(error)
    }
}

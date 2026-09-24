//! Output envelope and renderers.
//!
//! Every command produces a `serde_json::Value` payload. `--json` wraps it in
//! the stable `docfoo.cli/1` envelope; the default human renderer prints a
//! `message`/`text` field when present and pretty JSON otherwise. The real
//! markdown and Slack renderers land with the KG stage (1.3).

use serde::Serialize;
use serde_json::Value;

use crate::error::CliError;

pub const SCHEMA: &str = "docfoo.cli/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Markdown,
    Slack,
    Json,
}

impl OutputFormat {
    pub fn is_json(self) -> bool {
        matches!(self, OutputFormat::Json)
    }
}

#[derive(Serialize)]
pub struct Envelope<'a> {
    pub ok: bool,
    pub schema: &'static str,
    pub command: &'a str,
    pub workspace: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

#[derive(Serialize)]
pub struct ErrorInfo {
    pub code: &'static str,
    pub message: String,
}

/// Print a successful result.
pub fn success(
    format: OutputFormat,
    command: &str,
    workspace: &str,
    data: Value,
) -> Result<(), CliError> {
    if format.is_json() {
        let envelope = Envelope {
            ok: true,
            schema: SCHEMA,
            command,
            workspace,
            data: Some(data),
            error: None,
        };
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        print_human(&data);
    }
    Ok(())
}

/// Print a failure. JSON errors go to stdout as an envelope; human errors go to
/// stderr. The caller exits with `err.exit_code()`.
pub fn error(format: OutputFormat, command: &str, workspace: &str, err: &CliError) {
    if format.is_json() {
        let envelope = Envelope {
            ok: false,
            schema: SCHEMA,
            command,
            workspace,
            data: None,
            error: Some(ErrorInfo {
                code: err.code(),
                message: err.to_string(),
            }),
        };
        let rendered = serde_json::to_string_pretty(&envelope)
            .unwrap_or_else(|_| String::from("{\"ok\":false}"));
        println!("{rendered}");
    } else {
        eprintln!("error: {err}");
    }
}

fn print_human(data: &Value) {
    if let Some(message) = data.get("message").and_then(Value::as_str) {
        println!("{message}");
        return;
    }
    if let Some(text) = data.get("text").and_then(Value::as_str) {
        println!("{text}");
        return;
    }
    match serde_json::to_string_pretty(data) {
        Ok(pretty) => println!("{pretty}"),
        Err(_) => println!("{data}"),
    }
}

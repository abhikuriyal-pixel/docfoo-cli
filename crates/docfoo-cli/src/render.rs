//! Answer renderers: JSON envelope helpers, Slack mode, and extraction of
//! figures/tables from the answer markdown.
//!
//! The CLI never renders HTML; `answer_markdown` stays verbatim. Slack mode
//! only rewrites what Slack cannot use: local figure paths become `MEDIA:`
//! tags, and (optionally) GFM tables become bullet lines for the default flat
//! mrkdwn path. Sources are appended as a compact list.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Resolve an image path, repairing a wrong image extension when the
/// same-stem file exists (models sometimes write `.png` for a `.jpg`).
fn resolve_asset(resources_dir: &Path, path: &str) -> Option<std::path::PathBuf> {
    let direct = resources_dir.join(path);
    if direct.is_file() {
        return Some(direct);
    }
    let current = direct
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase)?;
    if !IMAGE_EXTS.contains(&current.as_str()) {
        return None;
    }
    IMAGE_EXTS
        .iter()
        .map(|ext| direct.with_extension(ext))
        .find(|candidate| candidate.is_file())
}

fn figure_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `![](path)` and `![](<path with spaces>)`; alt text is ignored.
    RE.get_or_init(|| {
        Regex::new(r"!\[[^\]]*\]\(\s*(?:<([^>]+)>|([^)]+))\s*\)").expect("figure regex")
    })
}

/// Figures referenced by the answer, resolved against `resources/`.
pub fn extract_figures(answer: &str, resources_dir: &Path) -> Vec<Value> {
    figure_re()
        .captures_iter(answer)
        .filter_map(|caps| {
            let path = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str().trim())
                .unwrap_or_default();
            if path.is_empty() {
                return None;
            }
            let absolute = resolve_asset(resources_dir, path);
            let resolved = absolute
                .as_ref()
                .and_then(|abs| abs.strip_prefix(resources_dir).ok())
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| path.to_string());
            Some(json!({
                "path": resolved,
                "abs_path": absolute.map(|abs| Value::String(abs.display().to_string())).unwrap_or(Value::Null),
                "markdown": caps[0].to_string(),
            }))
        })
        .collect()
}

/// GFM tables in the answer, as raw markdown blocks.
pub fn extract_tables(answer: &str) -> Vec<String> {
    let lines: Vec<&str> = answer.lines().collect();
    let mut tables = Vec::new();
    let mut index = 0;
    while index + 1 < lines.len() {
        if lines[index].contains('|') && is_table_separator(lines[index + 1]) {
            let start = index;
            index += 2;
            while index < lines.len() && lines[index].contains('|') && !lines[index].trim().is_empty() {
                index += 1;
            }
            tables.push(lines[start..index].join("\n"));
        } else {
            index += 1;
        }
    }
    tables
}

pub struct SlackOptions<'a> {
    /// Convert GFM tables to bullet lines (default flat-mrkdwn path).
    pub plain_tables: bool,
    /// Append the Sources section.
    pub include_sources: bool,
    /// Quote the exact lines under each source.
    pub quote_sources: bool,
    /// Bound the rendered body length.
    pub max_chars: Option<usize>,
    /// Prefix `[[hermes:final]]` so a patched Hermes returns it verbatim.
    pub hermes_final: bool,
    pub resources_dir: &'a Path,
}

/// Render an answer for Slack: `MEDIA:` figures, optional bullet tables and a
/// Sources section, then the optional `[[hermes:final]]` sentinel.
pub fn render_slack(answer: &str, sources: &[Value], options: &SlackOptions) -> String {
    let mut body = if options.plain_tables {
        convert_tables(answer)
    } else {
        answer.to_string()
    };
    body = convert_figures(&body, options.resources_dir);
    // Answers often start with blank lines from the synthesis stream; Slack
    // should not open with them.
    body = body.trim_start().to_string();
    if options.include_sources {
        body.push_str(&sources_section(sources, options));
    }
    if let Some(max_chars) = options.max_chars {
        body = truncate_chars(&body, max_chars);
    }
    if options.hermes_final {
        format!("[[hermes:final]]\n{body}")
    } else {
        body
    }
}

fn convert_figures(text: &str, resources_dir: &Path) -> String {
    figure_re()
        .replace_all(text, |caps: &regex::Captures| {
            let path = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str().trim())
                .unwrap_or_default();
            if path.is_empty()
                || path.starts_with("http://")
                || path.starts_with("https://")
                || path.starts_with("data:")
            {
                return caps[0].to_string();
            }
            let absolute = resolve_asset(resources_dir, path);
            match absolute {
                Some(absolute) => format!("MEDIA:{}", absolute.display()),
                None => caps[0].to_string(),
            }
        })
        .to_string()
}

fn convert_tables(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if index + 1 < lines.len()
            && lines[index].contains('|')
            && is_table_separator(lines[index + 1])
        {
            let header = parse_row(lines[index]);
            index += 2;
            while index < lines.len() && lines[index].contains('|') && !lines[index].trim().is_empty()
            {
                let row = parse_row(lines[index]);
                let mut parts = Vec::new();
                for (column, cell) in row.iter().enumerate() {
                    let label = header.get(column).map(String::as_str).unwrap_or("");
                    if label.is_empty() {
                        parts.push(cell.clone());
                    } else {
                        parts.push(format!("{label}: {cell}"));
                    }
                }
                out.push(format!("• {}", parts.join(" · ")));
                index += 1;
            }
            out.push(String::new());
        } else {
            out.push(lines[index].to_string());
            index += 1;
        }
    }
    out.join("\n").trim_end().to_string()
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return false;
    }
    let cells: Vec<&str> = trimmed
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect();
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let core = cell.trim_matches(':');
            !core.is_empty() && core.chars().all(|c| c == '-')
        })
}

fn parse_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn sources_section(sources: &[Value], options: &SlackOptions) -> String {
    let mut seen = HashSet::new();
    let mut lines = vec![String::new(), "Sources:".to_string()];
    for source in sources {
        let doc = source.get("doc").and_then(Value::as_str).unwrap_or_default();
        if doc.is_empty() {
            continue;
        }
        let start = source.get("start_line").and_then(Value::as_u64);
        let end = source.get("end_line").and_then(Value::as_u64);
        let label = match (start, end) {
            (Some(start), Some(end)) if end > start => format!("{doc}:{start}-{end}"),
            (Some(start), _) => format!("{doc}:{start}"),
            _ => doc.to_string(),
        };
        if !seen.insert(label.clone()) {
            continue;
        }
        lines.push(format!("• {label}"));
        if options.quote_sources {
            if let (Some(start), Some(end)) = (start, end) {
                let path = options.resources_dir.join(doc);
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let take = (end.saturating_sub(start) + 1) as usize;
                    for line in text
                        .lines()
                        .skip((start as usize).saturating_sub(1))
                        .take(take)
                    {
                        lines.push(format!("  > {line}"));
                    }
                }
            }
        }
    }
    let mut rendered = lines.join("\n");
    rendered.push('\n');
    rendered
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str("\n… (truncated)");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn figures_resolve_to_media_when_the_file_exists() {
        let temp = tempfile::tempdir().unwrap();
        let card = temp.path().join("Book/assets");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("figure_1.png"), b"png").unwrap();

        let answer = "Text\n\n![](Book/assets/figure_1.png)\n";
        let options = SlackOptions {
            plain_tables: false,
            include_sources: false,
            quote_sources: false,
            max_chars: None,
            hermes_final: false,
            resources_dir: temp.path(),
        };
        let rendered = render_slack(answer, &[], &options);
        assert!(rendered.contains("MEDIA:"), "rendered: {rendered}");
        assert!(rendered.contains("figure_1.png"));
        assert!(!rendered.contains("![]("), "image markdown should be replaced");
    }

    #[test]
    fn missing_figures_keep_their_markdown() {
        let temp = tempfile::tempdir().unwrap();
        let options = SlackOptions {
            plain_tables: false,
            include_sources: false,
            quote_sources: false,
            max_chars: None,
            hermes_final: false,
            resources_dir: temp.path(),
        };
        let rendered = render_slack("![](Book/assets/nope.png)", &[], &options);
        assert!(rendered.contains("![](Book/assets/nope.png)"));
    }

    #[test]
    fn tables_convert_to_bullets() {
        let answer = "Intro\n\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nEnd";
        let options = SlackOptions {
            plain_tables: true,
            include_sources: false,
            quote_sources: false,
            max_chars: None,
            hermes_final: false,
            resources_dir: Path::new("."),
        };
        let rendered = render_slack(answer, &[], &options);
        assert!(rendered.contains("• A: 1 · B: 2"), "rendered: {rendered}");
        assert!(rendered.contains("• A: 3 · B: 4"), "rendered: {rendered}");
        assert!(!rendered.contains("|---|"));
    }

    #[test]
    fn tables_are_preserved_by_default() {
        let answer = "| A | B |\n|---|---|\n| 1 | 2 |";
        let options = SlackOptions {
            plain_tables: false,
            include_sources: false,
            quote_sources: false,
            max_chars: None,
            hermes_final: false,
            resources_dir: Path::new("."),
        };
        let rendered = render_slack(answer, &[], &options);
        assert!(rendered.contains("| A | B |"));
    }

    #[test]
    fn sources_section_dedupes_and_quotes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("content.md"), "line one\nline two\nline three\n").unwrap();
        let sources = vec![
            json!({"doc":"content.md","start_line":2,"end_line":2}),
            json!({"doc":"content.md","start_line":2,"end_line":2}),
        ];
        let options = SlackOptions {
            plain_tables: false,
            include_sources: true,
            quote_sources: true,
            max_chars: None,
            hermes_final: false,
            resources_dir: temp.path(),
        };
        let rendered = render_slack("answer", &sources, &options);
        assert_eq!(rendered.matches("• content.md:2").count(), 1);
        assert!(rendered.contains("> line two"));
    }

    #[test]
    fn hermes_sentinel_is_prepended() {
        let options = SlackOptions {
            plain_tables: false,
            include_sources: false,
            quote_sources: false,
            max_chars: None,
            hermes_final: true,
            resources_dir: Path::new("."),
        };
        let rendered = render_slack("answer", &[], &options);
        assert!(rendered.starts_with("[[hermes:final]]\nanswer"));
    }

    #[test]
    fn extract_tables_finds_gfm_blocks() {
        let tables = extract_tables("before\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nafter");
        assert_eq!(tables.len(), 1);
        assert!(tables[0].starts_with("| A | B |"));
    }

    #[test]
    fn wrong_image_extension_is_repaired() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Book/assets")).unwrap();
        std::fs::write(temp.path().join("Book/assets/f.jpg"), b"jpg").unwrap();
        let figures = extract_figures("![](Book/assets/f.png)", temp.path());
        assert_eq!(figures[0]["path"], "Book/assets/f.jpg");
        assert!(figures[0]["abs_path"].as_str().unwrap().ends_with("f.jpg"));
    }

    #[test]
    fn extract_figures_reports_absolute_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Book/assets")).unwrap();
        std::fs::write(temp.path().join("Book/assets/f.png"), b"png").unwrap();
        let figures = extract_figures("![](Book/assets/f.png)", temp.path());
        assert_eq!(figures.len(), 1);
        assert!(figures[0]["abs_path"].as_str().unwrap().ends_with("f.png"));
    }
}

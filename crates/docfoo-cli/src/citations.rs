//! Citation parsing — `[<rel>:<line>]` / `[<rel>:<start>-<end>]`.
//!
//! Port of `DocFoo/src/lib/citations.ts`: same accepted syntax, same
//! "plausible resource path" guard, same display labels. The CLI extracts
//! tokens for the JSON envelope; the answer text keeps the inline citations
//! unchanged.

use std::sync::OnceLock;

use regex::Regex;

/// Hyphen, en dash, em dash, non-breaking hyphen (models copy book typography).
const DASHES: &str = "-–—‑";
/// Same set with the hyphen last: safe inside a `[...]` character class
/// (a leading/mid `-` after a class escape would be read as a range).
const DASH_CLASS: &str = "–—‑-";

fn single_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"\[([^\[\]]+?):(\d+)(?:[{DASHES}](\d+))?\]")).expect("citation regex")
    })
}

fn list_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"\[([^\[\]]+?):([\d,\s{DASH_CLASS}]+)\]")).expect("citation list regex")
    })
}

fn link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"\[([^\[\]]+?):(\d[\d,\s{DASH_CLASS}]*)\]\(([^)\n]+)\)"))
            .expect("citation link regex")
    })
}

fn title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\s+["'][^"']*["']$"#).expect("title regex"))
}

/// One citation found in assistant text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CitationToken {
    /// Resource rel path (e.g. "multi_column_1/content.md").
    pub file: String,
    /// 1-based start line.
    pub line_start: usize,
    /// 1-based end line (=== line_start for single-line citations).
    pub line_end: usize,
}

impl CitationToken {
    /// Short chip label: `card:12` or `card:12-14`.
    pub fn label(&self) -> String {
        let base = resource_display_name(&self.file);
        if self.line_end > self.line_start {
            format!("{base}:{}-{}", self.line_start, self.line_end)
        } else {
            format!("{base}:{}", self.line_start)
        }
    }

    /// Full title: rel path + lines.
    pub fn title(&self) -> String {
        if self.line_end > self.line_start {
            format!("{}:{}-{}", self.file, self.line_start, self.line_end)
        } else {
            format!("{}:{}", self.file, self.line_start)
        }
    }
}

/// A citation target must look like a resource file path.
fn is_plausible_resource_path(file: &str) -> bool {
    if file.is_empty() {
        return false;
    }
    if file.contains('/') {
        return true;
    }
    let lower = file.to_lowercase();
    [
        ".md", ".markdown", ".txt", ".html", ".htm", ".json", ".csv", ".pdf", ".png", ".jpg",
        ".jpeg", ".gif", ".webp", ".bmp",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension))
}

/// Human display name for a resource rel path. Scanned document cards store
/// their markdown as `content.md` (legacy `ocr.md`), so those show the
/// enclosing card (folder) name; every other file keeps its basename.
pub fn resource_display_name(rel: &str) -> String {
    let normalized = rel.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let Some(base) = parts.last() else {
        return rel.to_string();
    };
    if (*base == "content.md" || *base == "ocr.md") && parts.len() >= 2 {
        return parts[parts.len() - 2].to_string();
    }
    base.to_string()
}

/// Replace citation tokens with placeholders and record them, skipping fenced
/// code blocks (a `[path:12]` inside code is not a citation). Mirrors the
/// two-pass order of the web UI: markdown-link unwrap → single → comma list.
pub fn tokenize(text: &str) -> Vec<CitationToken> {
    let mut tokens = Vec::new();
    let mut in_fence = false;

    for line in text.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }

        let unwrapped = link_re()
            .replace_all(line, |caps: &regex::Captures| {
                let destination = title_re().replace(caps[3].trim(), "").to_string();
                if destination.is_empty() {
                    return caps[0].to_string();
                }
                if destination.contains(':') {
                    format!("[{destination}]")
                } else {
                    format!("[{destination}:{}]", caps[2].trim())
                }
            })
            .to_string();

        let replaced = single_re()
            .replace_all(&unwrapped, |caps: &regex::Captures| {
                let file = caps[1].trim();
                if !is_plausible_resource_path(file) {
                    return caps[0].to_string();
                }
                let start = caps[2].parse::<usize>().unwrap_or(0);
                let end = caps
                    .get(3)
                    .and_then(|m| m.as_str().parse::<usize>().ok())
                    .unwrap_or(start)
                    .max(start);
                let index = tokens.len();
                tokens.push(CitationToken {
                    file: file.to_string(),
                    line_start: start,
                    line_end: end,
                });
                format!("\u{27e6}cit:{index}\u{27e7}")
            })
            .to_string();

        list_re()
            .replace_all(&replaced, |caps: &regex::Captures| {
                let file = caps[1].trim();
                if !is_plausible_resource_path(file) {
                    return caps[0].to_string();
                }
                let parts: Vec<&str> = caps[2]
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .collect();
                if parts.is_empty() {
                    return caps[0].to_string();
                }
                let mut out = String::new();
                for part in parts {
                    let Some((start, end)) = parse_range(part) else {
                        continue;
                    };
                    let index = tokens.len();
                    tokens.push(CitationToken {
                        file: file.to_string(),
                        line_start: start,
                        line_end: end,
                    });
                    out.push_str(&format!("\u{27e6}cit:{index}\u{27e7}"));
                }
                if out.is_empty() {
                    caps[0].to_string()
                } else {
                    out
                }
            });
    }

    tokens
}

fn parse_range(part: &str) -> Option<(usize, usize)> {
    let mut pieces = part.split(|c| DASHES.contains(c));
    let start = pieces.next()?.trim().parse::<usize>().ok()?;
    let end = pieces
        .next()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(start)
        .max(start);
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_and_range_citations() {
        let tokens = tokenize("Fact [doc_a/content.md:59-61] and [doc_b/x.md:7].");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].file, "doc_a/content.md");
        assert_eq!(tokens[0].line_start, 59);
        assert_eq!(tokens[0].line_end, 61);
        assert_eq!(tokens[1].line_start, 7);
        assert_eq!(tokens[1].line_end, 7);
    }

    #[test]
    fn comma_list_expands_to_one_token_per_line() {
        let tokens = tokenize("See [doc/content.md:224, 235-240, 17].");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].line_start, 224);
        assert_eq!(tokens[1].line_start, 235);
        assert_eq!(tokens[1].line_end, 240);
        assert_eq!(tokens[2].line_start, 17);
    }

    #[test]
    fn unicode_dash_ranges() {
        let tokens = tokenize("See [doc/content.md:12–14] and [doc/content.md:20—22].");
        assert_eq!(tokens[0].line_end, 14);
        assert_eq!(tokens[1].line_end, 22);
    }

    #[test]
    fn markdown_link_citations_use_the_destination() {
        let tokens = tokenize("[content.md:353–361](Book/content.md:353-361)");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].file, "Book/content.md");
        assert_eq!(tokens[0].line_start, 353);
        assert_eq!(tokens[0].line_end, 361);
    }

    #[test]
    fn code_fences_are_ignored() {
        let tokens = tokenize("```\n[doc/content.md:12]\n```\n[doc/content.md:13]");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].line_start, 13);
    }

    #[test]
    fn implausible_paths_are_not_citations() {
        let tokens = tokenize("[note: 5] and [Section 3: 12]");
        assert!(tokens.is_empty());
    }

    #[test]
    fn labels_use_the_card_folder_for_content_md() {
        let token = CitationToken {
            file: "multi_column_1/content.md".to_string(),
            line_start: 77,
            line_end: 81,
        };
        assert_eq!(token.label(), "multi_column_1:77-81");
        assert_eq!(token.title(), "multi_column_1/content.md:77-81");
    }
}

//! Bounded read helpers — port of `agent/resource-format.ts` and the read
//! tools in `agent/read-tools.ts`.
//!
//! Every result is hard-bounded: by line count, byte budget, or both. The
//! structure-aware helpers (headings, figures, tables) let a caller navigate a
//! long document by section instead of reading it from the top.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::error::{CliError, Result};

/// Default lines returned by `resources --read` when no limit is given.
pub const PAGE_DEFAULT_LINES: usize = 200;
/// Hard cap for `--limit`.
pub const PAGE_MAX_LINES: usize = 4000;
/// Hard byte budget for one paged read (includes line-number prefixes).
pub const PAGE_MAX_BYTES: usize = 50 * 1024;
/// Hard byte budget for one outline result.
pub const OUTLINE_MAX_BYTES: usize = 40 * 1024;
pub const SEARCH_DEFAULT_LIMIT: usize = 25;
pub const SEARCH_MAX_LIMIT: usize = 50;
/// Hard byte budget for one search result.
pub const SEARCH_MAX_BYTES: usize = 40 * 1024;
/// Longest text kept per matched/context line.
pub const SEARCH_MAX_LINE_CHARS: usize = 200;
/// Maximum entries a one-level listing returns.
pub const LIST_MAX_ENTRIES: usize = 200;
/// How far above a figure/table caption we look for its image.
const SEARCH_CAPTION_IMAGE_RADIUS: usize = 20;
/// Files larger than this are not scanned for a line count.
const MAX_LINES_SCAN_BYTES: u64 = 2 * 1024 * 1024;

fn image_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)!\[[^\]]*\]\(\s*(?:<([^>]+)>|([^)]+?))(?:\s+["'][^"']*["'])?\s*\)"#)
            .expect("image link regex")
    })
}

fn image_ext_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\.(?:png|jpe?g|gif|webp|bmp)$").expect("image ext regex"))
}

fn heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*$").expect("heading regex"))
}

fn caption_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*(?:Figure|Table)\s+\d+(?:\.\d+)?\s+\S").expect("caption regex")
    })
}

pub fn is_markdown(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".md") || name.to_ascii_lowercase().ends_with(".markdown")
}

pub fn is_image(name: &str) -> bool {
    image_ext_re().is_match(name)
}

pub fn kind_of(name: &str) -> &'static str {
    if is_markdown(name) {
        "md"
    } else if is_image(name) {
        "image"
    } else {
        "file"
    }
}

/// Resolve a resource rel path under `root`, rejecting escapes. An empty rel
/// resolves to the root itself (used by one-level listings).
pub fn resolve_rel(root: &Path, rel: &str) -> Result<(String, PathBuf)> {
    let cleaned = rel.trim().replace('\\', "/");
    let cleaned = cleaned.trim_start_matches('/').to_string();
    if cleaned.is_empty() {
        return Ok((String::new(), root.to_path_buf()));
    }
    if cleaned
        .split('/')
        .any(|component| component == ".." || component.is_empty())
    {
        return Err(CliError::Usage(format!("invalid resource path \"{rel}\"")));
    }
    Ok((cleaned.clone(), root.join(cleaned)))
}

/// Read a text file as UTF-8.
pub fn read_text(abs: &Path) -> Result<String> {
    std::fs::read_to_string(abs).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CliError::NotFound(format!("resource not found: {}", abs.display()))
        } else {
            CliError::Io(error)
        }
    })
}

/// Split into lines the way the agent tools do (`\r?\n`).
pub fn split_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Paged reads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct PageSlice {
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub next_offset: Option<usize>,
    pub clipped: bool,
}

/// Normalize offset/limit into safe values (defaults: 1 / 200).
pub fn page_args(offset: Option<usize>, limit: Option<usize>) -> (usize, usize) {
    let offset = offset.unwrap_or(1).max(1);
    let limit = limit.unwrap_or(PAGE_DEFAULT_LINES).clamp(1, PAGE_MAX_LINES);
    (offset, limit)
}

/// Format lines `[offset, offset + limit)` with 1-based numbers
/// (`NNNNN | text`), stopping early when `PAGE_MAX_BYTES` is reached.
pub fn format_page(lines: &[String], offset: usize, limit: usize) -> PageSlice {
    let total_lines = lines.len();
    let from = offset.saturating_sub(1);
    let to = (from + limit).min(total_lines);
    let mut out: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    let mut end_line = offset.saturating_sub(1);
    let mut clipped = false;

    for (index, line) in lines.iter().enumerate().take(to).skip(from) {
        let prefix = format!("{:>5} | ", index + 1);
        let full = format!("{prefix}{line}");
        if bytes + full.len() + 1 > PAGE_MAX_BYTES {
            if out.is_empty() {
                let room = PAGE_MAX_BYTES.saturating_sub(bytes + prefix.len() + 4);
                if room > 0 {
                    out.push(format!("{prefix}{}…", clip_utf8(line, room)));
                    end_line = index + 1;
                    clipped = true;
                }
            }
            break;
        }
        bytes += full.len() + 1;
        out.push(full);
        end_line = index + 1;
    }

    PageSlice {
        text: out.join("\n"),
        start_line: if out.is_empty() { 0 } else { offset },
        end_line,
        total_lines,
        next_offset: if end_line < total_lines {
            Some(end_line + 1)
        } else {
            None
        },
        clipped,
    }
}

/// Trailing hint that tells the caller exactly how to continue paging.
pub fn page_note(page: &PageSlice) -> String {
    if let Some(next) = page.next_offset {
        let remaining = page.total_lines - page.end_line;
        return format!(
            "\n[... {remaining} more line{}; continue with offset {next} ...]",
            if remaining == 1 { "" } else { "s" }
        );
    }
    if page.clipped {
        "\n[long line clipped to fit the read budget]".to_string()
    } else {
        String::new()
    }
}

/// Header naming the full rel path so citations are never shortened.
pub fn page_header(rel: &str, page: &PageSlice) -> String {
    let range = if page.total_lines == 0 {
        "empty file".to_string()
    } else {
        format!(
            "lines {}-{} of {}",
            page.start_line, page.end_line, page.total_lines
        )
    };
    format!("File: {rel} ({range}). Cite lines as [{rel}:N].\n\n")
}

// ---------------------------------------------------------------------------
// Figures
// ---------------------------------------------------------------------------

/// Return the first figure path referenced on a markdown line.
pub fn image_of_line(line: &str) -> Option<String> {
    let caps = image_link_re().captures(line)?;
    let reference = caps
        .get(1)
        .or_else(|| caps.get(2))
        .map(|m| m.as_str().trim())
        .unwrap_or_default();
    if image_ext_re().is_match(reference) {
        Some(reference.to_string())
    } else {
        None
    }
}

/// Resource-relative path for a figure reference inside a markdown file:
/// `assets/x.png` in `book/content.md` → `book/assets/x.png`.
pub fn resource_image_path(file_rel: &str, image_ref: &str) -> String {
    let reference = image_ref
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string();
    match file_rel.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => format!("{dir}/{reference}"),
        _ => reference,
    }
}

/// Ready-to-paste markdown image line, percent-encoding each path segment.
pub fn figure_markdown(resource_rel: &str) -> String {
    let encoded: Vec<String> = resource_rel
        .replace('\\', "/")
        .split('/')
        .map(percent_encode_segment)
        .collect();
    format!("![]({})", encoded.join("/"))
}

fn percent_encode_segment(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.as_bytes() {
        let ch = *byte as char;
        // Same safe set as JavaScript's encodeURIComponent.
        if ch.is_ascii_alphanumeric()
            || matches!(ch, '-' | '_' | '.' | '~' | '!' | '*' | '\'' | '(' | ')')
        {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FigureLine {
    pub line: usize,
    pub markdown: String,
}

/// Exact markdown lines for every figure inside a read window.
pub fn figure_lines(
    rel: &str,
    lines: &[String],
    start_line: usize,
    end_line: usize,
    max: usize,
) -> Vec<FigureLine> {
    if start_line == 0 || end_line < start_line {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(end_line.min(lines.len()))
        .skip(start_line - 1)
    {
        if found.len() >= max {
            break;
        }
        if let Some(reference) = image_of_line(line) {
            found.push(FigureLine {
                line: index + 1,
                markdown: figure_markdown(&resource_image_path(rel, &reference)),
            });
        }
    }
    found
}

/// Human footer listing the window's figures.
pub fn figure_footer(rel: &str, lines: &[String], start_line: usize, end_line: usize) -> String {
    let found = figure_lines(rel, lines, start_line, end_line, 8);
    if found.is_empty() {
        return String::new();
    }
    let rows: Vec<String> = found
        .iter()
        .map(|figure| format!("line {}: {}", figure.line, figure.markdown))
        .collect();
    format!(
        "\n[figures in this window — copy exactly to show one]\n{}",
        rows.join("\n")
    )
}

/// The image belonging to a figure/table caption: the nearest image line
/// above it with only blank lines in between.
pub fn caption_image_above(lines: &[String], index: usize) -> Option<String> {
    let mut distance = 1;
    while distance <= SEARCH_CAPTION_IMAGE_RADIUS && index >= distance {
        let candidate = &lines[index - distance];
        if let Some(reference) = image_of_line(candidate) {
            let between = &lines[index - distance + 1..index];
            return if between.iter().all(|line| line.trim().is_empty()) {
                Some(reference)
            } else {
                None
            };
        }
        distance += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct OutlineEntry {
    pub line: usize,
    pub level: usize,
    pub heading: String,
    pub figures: usize,
    pub tables: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OutlineResult {
    pub entries: Vec<OutlineEntry>,
    pub heading_count: usize,
    pub figure_count: usize,
    pub table_count: usize,
    pub truncated: bool,
}

pub fn build_outline(lines: &[String]) -> OutlineResult {
    let mut entries: Vec<OutlineEntry> = Vec::new();
    let mut current: Option<usize> = None;
    let mut figures = 0usize;
    let mut tables = 0usize;
    let mut table_open = false;
    let mut heading_count = 0usize;
    let mut figure_count = 0usize;
    let mut table_count = 0usize;
    let mut bytes = 0usize;
    let mut truncated = false;

    for (index, line) in lines.iter().enumerate() {
        if let Some(caps) = heading_re().captures(line) {
            if let Some(current) = current {
                entries[current].figures = figures;
                entries[current].tables = tables;
            }
            heading_count += 1;
            figures = 0;
            tables = 0;
            table_open = false;
            let level = caps[1].len();
            let heading = caps[2].trim().to_string();
            let row = format!("{:>5} | {} {}\n", index + 1, "#".repeat(level), heading);
            if !truncated && bytes + row.len() <= OUTLINE_MAX_BYTES {
                entries.push(OutlineEntry {
                    line: index + 1,
                    level,
                    heading,
                    figures: 0,
                    tables: 0,
                });
                bytes += row.len();
                current = Some(entries.len() - 1);
            } else {
                truncated = true;
                current = None;
            }
            continue;
        }
        if image_of_line(line).is_some() {
            figure_count += 1;
            figures += 1;
        }
        let is_table = line.trim_start().starts_with('|');
        if is_table && !table_open {
            table_count += 1;
            tables += 1;
        }
        table_open = is_table;
    }
    if let Some(current) = current {
        entries[current].figures = figures;
        entries[current].tables = tables;
    }

    OutlineResult {
        entries,
        heading_count,
        figure_count,
        table_count,
        truncated,
    }
}

/// Render an outline as compact text (line numbers keep citations working).
pub fn format_outline(rel: &str, lines: &[String]) -> String {
    let outline = build_outline(lines);
    let header = format!(
        "Outline of {rel} — {} lines, {} headings, {} figures, {} tables.",
        lines.len(),
        outline.heading_count,
        outline.figure_count,
        outline.table_count
    );
    if outline.entries.is_empty() {
        return format!(
            "{header}\nNo headings found — use `resources --search` to find a passage, then `--read` with offset/limit around it."
        );
    }
    let rows: Vec<String> = outline
        .entries
        .iter()
        .map(|entry| {
            let mut badges = Vec::new();
            if entry.figures > 0 {
                badges.push(format!("figures: {}", entry.figures));
            }
            if entry.tables > 0 {
                badges.push(format!("tables: {}", entry.tables));
            }
            let suffix = if badges.is_empty() {
                String::new()
            } else {
                format!("  [{}]", badges.join(", "))
            };
            format!(
                "{:>5} | {} {}{suffix}",
                entry.line,
                "#".repeat(entry.level),
                entry.heading
            )
        })
        .collect();
    let tail = if outline.truncated {
        format!(
            "\n[Outline truncated at {} of {} headings — use `resources --search` to narrow down.]",
            outline.entries.len(),
            outline.heading_count
        )
    } else {
        String::new()
    };
    format!("{header}\n{}{tail}", rows.join("\n"))
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub file: String,
    pub line: usize,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
    pub context: usize,
    pub limit: usize,
}

/// Recursively list markdown files under `root/rel` as `(rel, abs)` pairs.
pub fn markdown_targets(root: &Path, rel: &str) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let base = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if is_markdown(&name) {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push((rel.to_string_lossy().replace('\\', "/"), path));
                }
            }
        }
    }
    walk(&base, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Case-insensitive substring search, capped by hit count and total bytes.
pub fn search_targets(
    targets: &[(String, PathBuf)],
    raw_query: &str,
    raw_context: Option<usize>,
    raw_limit: Option<usize>,
) -> SearchResult {
    let query = raw_query.trim().to_lowercase();
    let context = raw_context.unwrap_or(0).min(5);
    let limit = raw_limit
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    if query.is_empty() {
        return SearchResult {
            hits,
            truncated,
            context,
            limit,
        };
    }

    'scan: for (rel, abs) in targets {
        let Ok(content) = std::fs::read_to_string(abs) else {
            continue;
        };
        let lines = split_lines(&content);
        for (index, line) in lines.iter().enumerate() {
            if !line.to_lowercase().contains(&query) {
                continue;
            }
            if hits.len() >= limit {
                truncated = true;
                break 'scan;
            }
            let before: Vec<String> = if context > 0 {
                lines[index.saturating_sub(context)..index]
                    .iter()
                    .map(|line| clip_search_line(line))
                    .collect()
            } else {
                Vec::new()
            };
            let after: Vec<String> = if context > 0 {
                lines[index + 1..(index + 1 + context).min(lines.len())]
                    .iter()
                    .map(|line| clip_search_line(line))
                    .collect()
            } else {
                Vec::new()
            };
            let mut image_ref = lines[index.saturating_sub(context)
                ..(index + 1 + context).min(lines.len())]
                .iter()
                .find_map(|candidate| image_of_line(candidate));
            if image_ref.is_none() && caption_line_re().is_match(line) {
                image_ref = caption_image_above(&lines, index);
            }
            let mut hit = SearchHit {
                file: rel.clone(),
                line: index + 1,
                text: clip_search_line(line),
                image: None,
                markdown: None,
                before,
                after,
            };
            if let Some(reference) = image_ref {
                let resource_rel = resource_image_path(rel, &reference);
                hit.markdown = Some(figure_markdown(&resource_rel));
                hit.image = Some(resource_rel);
            }
            let size = serde_json::to_string(&hit).map(|text| text.len() + 1).unwrap_or(0);
            if bytes + size > SEARCH_MAX_BYTES {
                truncated = true;
                break 'scan;
            }
            hits.push(hit);
            bytes += size;
        }
    }
    SearchResult {
        hits,
        truncated,
        context,
        limit,
    }
}

/// Next-step hint when a search finds nothing.
pub fn search_hint(query: &str) -> String {
    let words: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|word| word.len() > 2)
        .take(3)
        .map(str::to_string)
        .collect();
    let suggestions = if words.is_empty() {
        "one distinctive word".to_string()
    } else {
        words.join(", ")
    };
    format!(
        "No line contains \"{query}\". Retry with a single keyword ({suggestions}), or use `resources --outline` to browse the document's sections."
    )
}

fn clip_search_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() > SEARCH_MAX_LINE_CHARS {
        trimmed.chars().take(SEARCH_MAX_LINE_CHARS).collect()
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------
// One-level listing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListingEntry {
    pub name: String,
    pub rel: String,
    pub kind: &'static str,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub figures: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub figure_paths: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Listing {
    pub entries: Vec<ListingEntry>,
    pub total: usize,
    pub truncated: bool,
}

/// Aggregate counts for a folder without returning its contents.
fn dir_stats(dir: &Path, prefix: &str) -> (u64, usize, usize, usize) {
    let mut size = 0u64;
    let mut files = 0usize;
    let mut md = 0usize;
    let mut figures = 0usize;
    let mut stack = vec![(dir.to_path_buf(), prefix.to_string())];
    while let Some((current, rel_prefix)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, format!("{rel_prefix}/{name}")));
                continue;
            }
            files += 1;
            if let Ok(meta) = path.metadata() {
                size += meta.len();
            }
            if is_markdown(&name) {
                md += 1;
            } else if is_image(&name) {
                figures += 1;
            }
        }
    }
    (size, files, md, figures)
}

fn collect_image_paths(dir: &Path, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), prefix.to_string())];
    while let Some((current, rel_prefix)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let rel = format!("{rel_prefix}/{name}");
            if path.is_dir() {
                stack.push((path, rel));
            } else if is_image(&name) {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

fn file_entry(abs: &Path, rel: &str) -> ListingEntry {
    let mut size = 0u64;
    if let Ok(meta) = abs.metadata() {
        size = meta.len();
    }
    let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
    let mut entry = ListingEntry {
        name,
        rel: rel.to_string(),
        kind: kind_of(rel),
        size,
        files: None,
        md: None,
        figures: None,
        lines: None,
        figure_paths: Vec::new(),
    };
    if !is_image(rel) && size <= MAX_LINES_SCAN_BYTES {
        if let Ok(text) = std::fs::read_to_string(abs) {
            entry.lines = Some(split_lines(&text).len());
        }
    }
    entry
}

/// One directory level, folders first (sorted), then files (sorted).
/// A missing root (`rel == ""`) lists as empty; a missing subfolder is an
/// error.
pub fn list_level(root: &Path, rel: &str, include_figure_paths: bool) -> Result<Listing> {
    let (rel, dir) = resolve_rel(root, rel)?;
    let read = match std::fs::read_dir(&dir) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && rel.is_empty() => {
            return Ok(Listing {
                entries: Vec::new(),
                total: 0,
                truncated: false,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CliError::NotFound(format!(
                "resource folder not found: {rel}"
            )));
        }
        Err(error) => return Err(CliError::Io(error)),
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if entry.metadata().map(|meta| meta.is_dir()).unwrap_or(false) {
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();

    let mut entries = Vec::new();
    for name in dirs {
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let child_abs = dir.join(&name);
        let (size, count, md, figures) = dir_stats(&child_abs, &child_rel);
        entries.push(ListingEntry {
            name,
            rel: child_rel.clone(),
            kind: "dir",
            size,
            files: Some(count),
            md: Some(md),
            figures: Some(figures),
            lines: None,
            figure_paths: if include_figure_paths {
                collect_image_paths(&child_abs, &child_rel)
            } else {
                Vec::new()
            },
        });
    }
    for name in files {
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        entries.push(file_entry(&dir.join(&name), &child_rel));
    }
    let total = entries.len();
    let truncated = total > LIST_MAX_ENTRIES;
    if truncated {
        entries.truncate(LIST_MAX_ENTRIES);
    }
    Ok(Listing {
        entries,
        total,
        truncated,
    })
}

fn clip_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paged_read_numbers_lines_and_reports_next_offset() {
        let lines: Vec<String> = (1..=10).map(|index| format!("line {index}")).collect();
        let page = format_page(&lines, 3, 4);
        assert_eq!(page.start_line, 3);
        assert_eq!(page.end_line, 6);
        assert_eq!(page.next_offset, Some(7));
        assert!(page.text.starts_with("    3 | line 3"));
        assert!(page_note(&page).contains("offset 7"));
    }

    #[test]
    fn offset_past_the_end_is_empty() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let page = format_page(&lines, 10, 5);
        assert_eq!(page.start_line, 0);
        assert_eq!(page.next_offset, None);
    }

    #[test]
    fn outline_counts_figures_and_tables() {
        let lines: Vec<String> = [
            "# Intro",
            "![](assets/figure_1.png)",
            "| A | B |",
            "|---|---|",
            "| 1 | 2 |",
            "## Next",
            "text",
        ]
        .iter()
        .map(|line| line.to_string())
        .collect();
        let outline = build_outline(&lines);
        assert_eq!(outline.heading_count, 2);
        assert_eq!(outline.figure_count, 1);
        assert_eq!(outline.table_count, 1);
        assert_eq!(outline.entries[0].figures, 1);
        assert_eq!(outline.entries[0].tables, 1);
        let rendered = format_outline("doc/content.md", &lines);
        assert!(rendered.contains("Outline of doc/content.md"));
    }

    #[test]
    fn search_finds_hits_and_images() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("doc")).unwrap();
        std::fs::write(
            temp.path().join("doc/content.md"),
            "Alpha line\n![](assets/figure_1.png)\nFigure 1. A chart\nbeta alpha\n",
        )
        .unwrap();
        let targets = vec![("doc/content.md".to_string(), temp.path().join("doc/content.md"))];
        let result = search_targets(&targets, "alpha", Some(1), None);
        assert_eq!(result.hits.len(), 2);
        assert_eq!(result.hits[0].line, 1);
        assert_eq!(result.hits[1].line, 4);
        assert_eq!(result.hits[0].image.as_deref(), Some("doc/assets/figure_1.png"));
        assert!(result.hits[1]
            .before
            .iter()
            .any(|line| line.contains("Figure")));
    }

    #[test]
    fn caption_image_is_found_above_the_caption() {
        let lines: Vec<String> = ["![](assets/f.png)", "", "Figure 1. A chart"]
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(
            caption_image_above(&lines, 2).as_deref(),
            Some("assets/f.png")
        );
        let prose: Vec<String> = ["![](assets/f.png)", "text", "Figure 1. A chart"]
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(caption_image_above(&prose, 2), None);
    }

    #[test]
    fn figure_markdown_encodes_spaces() {
        assert_eq!(
            figure_markdown("My Book/assets/figure 1.png"),
            "![](My%20Book/assets/figure%201.png)"
        );
    }

    #[test]
    fn list_level_summarizes_folders() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Book/assets")).unwrap();
        std::fs::write(temp.path().join("Book/content.md"), "hello").unwrap();
        std::fs::write(temp.path().join("Book/assets/f.png"), b"png").unwrap();
        std::fs::write(temp.path().join("root.md"), "root").unwrap();
        let listing = list_level(temp.path(), "", true).unwrap();
        assert_eq!(listing.entries.len(), 2);
        let book = listing
            .entries
            .iter()
            .find(|entry| entry.name == "Book")
            .unwrap();
        assert_eq!(book.kind, "dir");
        assert_eq!(book.files, Some(2));
        assert_eq!(book.md, Some(1));
        assert_eq!(book.figures, Some(1));
        assert_eq!(book.figure_paths, vec!["Book/assets/f.png"]);
    }

    #[test]
    fn resolve_rel_rejects_escapes() {
        let root = Path::new("/tmp/resources");
        assert!(resolve_rel(root, "../etc/passwd").is_err());
        assert!(resolve_rel(root, "").is_ok());
        assert_eq!(resolve_rel(root, "Book/content.md").unwrap().0, "Book/content.md");
    }
}

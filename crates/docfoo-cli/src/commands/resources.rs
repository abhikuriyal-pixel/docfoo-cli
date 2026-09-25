//! `docfoo resources` — read-only library access.

use serde_json::json;

use crate::cli::ResourcesArgs;
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::resources::{read, tree};
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &ResourcesArgs) -> Result<()> {
    if args.vis {
        if args.list
            || args.tree
            || args.figures
            || args.read.is_some()
            || args.outline.is_some()
            || args.search.is_some()
        {
            return Err(CliError::Usage(
                "--vis cannot be combined with --list, --read, --outline or --search".to_string(),
            ));
        }
        return crate::resources::vis::run(workspace, args, args.rel.as_deref().unwrap_or(""));
    }

    let root = workspace.resources_dir();
    let workspace_display = workspace.root.display().to_string();

    if args.list {
        return list(format, &root, &workspace_display, args);
    }
    if let Some(rel) = &args.read {
        return read_file(format, &root, &workspace_display, rel, args);
    }
    if let Some(rel) = &args.outline {
        return outline(format, &root, &workspace_display, rel);
    }
    if let Some(query) = &args.search {
        return search(format, &root, &workspace_display, query, args);
    }
    Err(CliError::Usage(
        "resources needs --vis, --list, --read, --outline or --search".to_string(),
    ))
}

fn list(
    format: OutputFormat,
    root: &std::path::Path,
    workspace: &str,
    args: &ResourcesArgs,
) -> Result<()> {
    let rel = args.rel.clone().unwrap_or_default();
    if args.tree {
        let tree = tree::scan_resources(root);
        let entries = if rel.is_empty() {
            tree
        } else {
            find_subtree(&tree, &rel).ok_or_else(|| {
                CliError::NotFound(format!("resource folder not found: {rel}"))
            })?
        };
        if format.is_json() {
            return output::success(
                format,
                "resources.list",
                workspace,
                json!({ "scope": rel, "tree": entries }),
            );
        }
        let mut out = String::new();
        render_tree(&entries, 0, &mut out);
        print!("{out}");
        return Ok(());
    }

    let listing = read::list_level(root, &rel, args.figures)?;
    if format.is_json() {
        return output::success(
            format,
            "resources.list",
            workspace,
            json!({
                "scope": rel,
                "entries": listing.entries,
                "total": listing.total,
                "truncated": listing.truncated,
            }),
        );
    }
    for entry in &listing.entries {
        println!("{}", format_listing_line(entry));
    }
    if listing.truncated {
        eprintln!(
            "[showing {} of {} entries — narrow the listing with --rel]",
            listing.entries.len(),
            listing.total
        );
    }
    Ok(())
}

fn read_file(
    format: OutputFormat,
    root: &std::path::Path,
    workspace: &str,
    rel: &str,
    args: &ResourcesArgs,
) -> Result<()> {
    if rel.trim().is_empty() {
        return Err(CliError::Usage("a resource path is required".to_string()));
    }
    let (rel, abs) = read::resolve_rel(root, rel)?;
    if !abs.is_file() {
        return Err(CliError::NotFound(format!("resource not found: {rel}")));
    }
    if !read::is_markdown(&rel) {
        return Err(CliError::Usage(
            "only markdown resources can be read".to_string(),
        ));
    }
    let text = read::read_text(&abs)?;
    let lines = read::split_lines(&text);

    if args.figures {
        let figures = read::figure_lines(&rel, &lines, 1, lines.len(), usize::MAX);
        if format.is_json() {
            return output::success(
                format,
                "resources.read",
                workspace,
                json!({ "rel": rel, "totalLines": lines.len(), "figures": figures }),
            );
        }
        for figure in &figures {
            println!("{}", figure.markdown);
        }
        return Ok(());
    }

    let (offset, limit) = read::page_args(args.offset, args.limit);
    if !lines.is_empty() && offset > lines.len() {
        return Err(CliError::Usage(format!(
            "offset {offset} is beyond the end of {rel} ({} lines total)",
            lines.len()
        )));
    }
    let page = read::format_page(&lines, offset, limit);
    let figures = read::figure_lines(&rel, &lines, page.start_line, page.end_line, 8);
    let plain_text = if page.start_line == 0 {
        String::new()
    } else {
        lines[page.start_line - 1..page.end_line].join("\n")
    };

    if format.is_json() {
        return output::success(
            format,
            "resources.read",
            workspace,
            json!({
                "rel": rel,
                "startLine": page.start_line,
                "endLine": page.end_line,
                "totalLines": page.total_lines,
                "nextOffset": page.next_offset,
                "clipped": page.clipped,
                "text": plain_text,
                "numberedText": page.text,
                "figures": figures,
            }),
        );
    }

    if args.plain {
        println!("{plain_text}");
    } else {
        print!("{}", read::page_header(&rel, &page));
        println!("{}", page.text);
    }
    let footer = read::figure_footer(&rel, &lines, page.start_line, page.end_line);
    if !footer.is_empty() {
        println!("{footer}");
    }
    let note = read::page_note(&page);
    if !note.is_empty() {
        println!("{note}");
    }
    Ok(())
}

fn outline(
    format: OutputFormat,
    root: &std::path::Path,
    workspace: &str,
    rel: &str,
) -> Result<()> {
    if rel.trim().is_empty() {
        return Err(CliError::Usage("a resource path is required".to_string()));
    }
    let (rel, abs) = read::resolve_rel(root, rel)?;
    if !abs.is_file() {
        return Err(CliError::NotFound(format!("resource not found: {rel}")));
    }
    if !read::is_markdown(&rel) {
        return Err(CliError::Usage(
            "only markdown resources can be outlined".to_string(),
        ));
    }
    let text = read::read_text(&abs)?;
    let lines = read::split_lines(&text);
    let outline = read::build_outline(&lines);
    if format.is_json() {
        return output::success(
            format,
            "resources.outline",
            workspace,
            json!({
                "rel": rel,
                "totalLines": lines.len(),
                "headingCount": outline.heading_count,
                "figureCount": outline.figure_count,
                "tableCount": outline.table_count,
                "truncated": outline.truncated,
                "sections": outline.entries,
            }),
        );
    }
    println!("{}", read::format_outline(&rel, &lines));
    Ok(())
}

fn search(
    format: OutputFormat,
    root: &std::path::Path,
    workspace: &str,
    query: &str,
    args: &ResourcesArgs,
) -> Result<()> {
    if query.trim().is_empty() {
        return Err(CliError::Usage("--search needs a query".to_string()));
    }
    let targets = match &args.rel {
        Some(rel) => {
            let (rel, abs) = read::resolve_rel(root, rel)?;
            if !abs.exists() {
                return Err(CliError::NotFound(format!("resource not found: {rel}")));
            }
            if abs.is_dir() {
                read::markdown_targets(root, &rel)
            } else {
                if !read::is_markdown(&rel) {
                    return Err(CliError::Usage(
                        "search can only look inside markdown resources".to_string(),
                    ));
                }
                vec![(rel, abs)]
            }
        }
        None => read::markdown_targets(root, ""),
    };
    let result = read::search_targets(&targets, query, args.context, args.limit);
    let total = result.hits.len();
    if format.is_json() {
        let mut data = json!({
            "query": query,
            "total": total,
            "truncated": result.truncated,
            "hits": result.hits,
        });
        if let Some(rel) = &args.rel {
            data["rel"] = json!(rel);
        }
        if total == 0 {
            data["hint"] = json!(read::search_hint(query));
        }
        return output::success(format, "resources.search", workspace, data);
    }
    if result.hits.is_empty() {
        eprintln!("{}", read::search_hint(query));
        return Ok(());
    }
    for hit in &result.hits {
        let before_len = hit.before.len();
        for (index, line) in hit.before.iter().enumerate() {
            println!("    {} | {line}", hit.line - before_len + index);
        }
        println!("{}:{}: {}", hit.file, hit.line, hit.text);
        for (index, line) in hit.after.iter().enumerate() {
            println!("    {} | {line}", hit.line + 1 + index);
        }
    }
    if result.truncated {
        eprintln!("[results truncated at {total} hits — narrow with --rel or refine the query]");
    }
    Ok(())
}

fn format_listing_line(entry: &read::ListingEntry) -> String {
    match entry.kind {
        "dir" => format!(
            "{}/  dir  {} files, {} md, {} figures, {}",
            entry.rel,
            entry.files.unwrap_or(0),
            entry.md.unwrap_or(0),
            entry.figures.unwrap_or(0),
            human_bytes(entry.size)
        ),
        kind => {
            let lines = entry
                .lines
                .map(|lines| format!(", {lines} lines"))
                .unwrap_or_default();
            format!(
                "{}  {kind}  {}{lines}",
                entry.rel,
                human_bytes(entry.size)
            )
        }
    }
}

fn render_tree(entries: &[tree::ResourceEntry], depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for entry in entries {
        match entry.kind {
            "dir" => out.push_str(&format!(
                "{indent}{}/  dir  {} files, {} figures, {}\n",
                entry.name,
                entry.files,
                entry.figures.len(),
                human_bytes(entry.size)
            )),
            kind => out.push_str(&format!(
                "{indent}{}  {kind}  {}\n",
                entry.name,
                human_bytes(entry.size)
            )),
        }
        render_tree(&entry.children, depth + 1, out);
    }
}

fn find_subtree(entries: &[tree::ResourceEntry], rel: &str) -> Option<Vec<tree::ResourceEntry>> {
    for entry in entries {
        if entry.rel == rel {
            return Some(entry.children.clone());
        }
        if let Some(found) = find_subtree(&entry.children, rel) {
            return Some(found);
        }
    }
    None
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

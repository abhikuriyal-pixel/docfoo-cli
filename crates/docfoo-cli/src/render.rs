//! Answer renderers: JSON envelope helpers, Slack mode, and extraction of
//! figures/tables from the answer markdown.
//!
//! The CLI never renders HTML; `answer_markdown` stays verbatim. Slack mode
//! only rewrites what Slack cannot use: `$…$` LaTeX becomes Unicode math,
//! `[doc.md:1-2]` citations become inline-code chips, local figure paths
//! become `MEDIA:` tags, and (optionally) GFM tables become bullet lines for
//! the default flat mrkdwn path. Sources are appended as a compact list unless
//! `--no-sources`.

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
    body = convert_math(&body);
    body = convert_stray_latex(&body);
    body = convert_citations(&body);
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

/// `[single_column_1/content.md:75-89]` -> `` `single_column_1/content.md:75-89` ``.
/// Slack renders inline code as a small chip, which visually separates
/// citations from prose.
fn citation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\[([A-Za-z0-9_][A-Za-z0-9_\-./\\ ,]*\.(?:md|pdf|txt|json|csv|html?)(?::[0-9\-, ]+)?)\]",
        )
        .expect("citation regex")
    })
}

fn convert_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in citation_re().find_iter(text) {
        // `[paper.md](url)` is markdown link text, not a citation.
        if text[found.end()..].starts_with('(') {
            continue;
        }
        out.push_str(&text[last..found.start()]);
        out.push('`');
        out.push_str(found.as_str().trim_matches(|ch| ch == '[' || ch == ']'));
        out.push('`');
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}

/// Escapes that also appear outside `$…$` (`58\%`, `\{…\}`).
fn convert_stray_latex(text: &str) -> String {
    text.replace("\\%", "%")
        .replace("\\&", "&")
        .replace("\\_", "_")
        .replace("\\#", "#")
        .replace("\\{", "{")
        .replace("\\}", "}")
        .replace("\\$", "$")
}

/// `$…$` / `$$…$$` -> readable Unicode. Slack has no math rendering, so
/// LaTeX commands become symbols and single-character scripts become
/// sub/superscripts; multi-character scripts keep `_`/`^`.
fn convert_math(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let display = i + 1 < chars.len() && chars[i + 1] == '$';
            let start = i + if display { 2 } else { 1 };
            let mut j = start;
            let end = loop {
                if j >= chars.len() {
                    break None;
                }
                if chars[j] == '$' {
                    if display {
                        if j + 1 < chars.len() && chars[j + 1] == '$' {
                            break Some(j);
                        }
                    } else {
                        break Some(j);
                    }
                }
                j += 1;
            };
            if let Some(end) = end {
                let inner: String = chars[start..end].iter().collect();
                out.push_str(latex_to_text(&inner).trim());
                i = end + if display { 2 } else { 1 };
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

const LATEX_SYMBOLS: &[(&str, &str)] = &[
    ("\\alpha", "α"),
    ("\\beta", "β"),
    ("\\gamma", "γ"),
    ("\\delta", "δ"),
    ("\\epsilon", "ε"),
    ("\\varepsilon", "ε"),
    ("\\zeta", "ζ"),
    ("\\eta", "η"),
    ("\\theta", "θ"),
    ("\\iota", "ι"),
    ("\\kappa", "κ"),
    ("\\lambda", "λ"),
    ("\\mu", "μ"),
    ("\\nu", "ν"),
    ("\\xi", "ξ"),
    ("\\pi", "π"),
    ("\\rho", "ρ"),
    ("\\sigma", "σ"),
    ("\\tau", "τ"),
    ("\\upsilon", "υ"),
    ("\\phi", "φ"),
    ("\\varphi", "φ"),
    ("\\chi", "χ"),
    ("\\psi", "ψ"),
    ("\\omega", "ω"),
    ("\\Gamma", "Γ"),
    ("\\Delta", "Δ"),
    ("\\Theta", "Θ"),
    ("\\Lambda", "Λ"),
    ("\\Xi", "Ξ"),
    ("\\Pi", "Π"),
    ("\\Sigma", "Σ"),
    ("\\Upsilon", "Υ"),
    ("\\Phi", "Φ"),
    ("\\Psi", "Ψ"),
    ("\\Omega", "Ω"),
    ("\\times", "×"),
    ("\\cdot", "·"),
    ("\\cdots", "⋯"),
    ("\\otimes", "⊗"),
    ("\\oplus", "⊕"),
    ("\\odot", "⊙"),
    ("\\circ", "∘"),
    ("\\rightarrow", "→"),
    ("\\leftarrow", "←"),
    ("\\Rightarrow", "⇒"),
    ("\\Leftarrow", "⇐"),
    ("\\leftrightarrow", "↔"),
    ("\\mapsto", "↦"),
    ("\\to", "→"),
    ("\\in", "∈"),
    ("\\notin", "∉"),
    ("\\subset", "⊂"),
    ("\\subseteq", "⊆"),
    ("\\supset", "⊃"),
    ("\\supseteq", "⊇"),
    ("\\cup", "∪"),
    ("\\cap", "∩"),
    ("\\setminus", "∖"),
    ("\\emptyset", "∅"),
    ("\\sum", "∑"),
    ("\\prod", "∏"),
    ("\\int", "∫"),
    ("\\partial", "∂"),
    ("\\nabla", "∇"),
    ("\\infty", "∞"),
    ("\\approx", "≈"),
    ("\\neq", "≠"),
    ("\\ne", "≠"),
    ("\\leqslant", "≤"),
    ("\\leq", "≤"),
    ("\\le", "≤"),
    ("\\geqslant", "≥"),
    ("\\geq", "≥"),
    ("\\ge", "≥"),
    ("\\pm", "±"),
    ("\\mp", "∓"),
    ("\\propto", "∝"),
    ("\\simeq", "≃"),
    ("\\sim", "∼"),
    ("\\equiv", "≡"),
    ("\\forall", "∀"),
    ("\\exists", "∃"),
    ("\\neg", "¬"),
    ("\\land", "∧"),
    ("\\lor", "∨"),
    ("\\infty", "∞"),
    ("\\langle", "⟨"),
    ("\\rangle", "⟩"),
    ("\\lceil", "⌈"),
    ("\\rceil", "⌉"),
    ("\\lfloor", "⌊"),
    ("\\rfloor", "⌋"),
    ("\\ldots", "…"),
    ("\\dots", "…"),
    ("\\vdots", "⋮"),
    ("\\ddots", "⋱"),
    ("\\prime", "′"),
    ("\\star", "⋆"),
    ("\\ast", "∗"),
    ("\\bullet", "•"),
    ("\\quad", " "),
    ("\\qquad", "  "),
    ("\\%", "%"),
    ("\\&", "&"),
    ("\\_", "_"),
    ("\\#", "#"),
    ("\\$", "$"),
    ("\\{", "{"),
    ("\\}", "}"),
    ("\\|", "‖"),
];

fn latex_to_text(latex: &str) -> String {
    let mut s = latex.to_string();
    for wrapper in [
        "\\text",
        "\\mathrm",
        "\\mathbf",
        "\\mathit",
        "\\operatorname",
        "\\mathcal",
    ] {
        s = map_group(&s, wrapper, "", "");
    }
    s = map_group(&s, "\\tag", "(", ")");
    s = replace_frac(&s);
    s = s.replace("\\sqrt", "√");
    // Longest commands first so `\to` never eats `\top`-like prefixes.
    let mut symbols: Vec<(&str, &str)> = LATEX_SYMBOLS.to_vec();
    symbols.sort_by_key(|(command, _)| std::cmp::Reverse(command.len()));
    for (command, symbol) in symbols {
        s = s.replace(command, symbol);
    }
    // After symbols: `\rightarrow` must win over `\right`.
    s = s.replace("\\left", "").replace("\\right", "");
    for spacing in ["\\,", "\\;", "\\:", "\\!", "\\ "] {
        s = s.replace(spacing, " ");
    }
    s = convert_scripts(&s);
    s = strip_backslashes(&s);
    s.replace(['{', '}'], "")
}

/// `\cmd{inner}` -> `prefix + inner + suffix` (nested groups included).
fn map_group(s: &str, command: &str, prefix: &str, suffix: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(position) = rest.find(command) {
        out.push_str(&rest[..position]);
        let after = &rest[position + command.len()..];
        if let Some(inner_rest) = after.strip_prefix('{') {
            if let Some(end) = matching_brace(inner_rest) {
                out.push_str(prefix);
                out.push_str(&map_group(&inner_rest[..end], command, prefix, suffix));
                out.push_str(suffix);
                rest = &inner_rest[end + 1..];
                continue;
            }
        }
        out.push_str(command);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Index of the `}` closing the group opened before `after_open`.
fn matching_brace(after_open: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (index, ch) in after_open.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn replace_frac(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(position) = rest.find("\\frac") {
        out.push_str(&rest[..position]);
        let after = &rest[position + 5..];
        let Some(numerator_rest) = after.strip_prefix('{') else {
            out.push_str("\\frac");
            rest = after;
            continue;
        };
        let Some(numerator_end) = matching_brace(numerator_rest) else {
            out.push_str("\\frac");
            rest = after;
            continue;
        };
        let after_numerator = &numerator_rest[numerator_end + 1..];
        let Some(denominator_rest) = after_numerator.strip_prefix('{') else {
            out.push_str("\\frac");
            rest = after;
            continue;
        };
        let Some(denominator_end) = matching_brace(denominator_rest) else {
            out.push_str("\\frac");
            rest = after;
            continue;
        };
        out.push_str(&format!(
            "({})/({})",
            replace_frac(&numerator_rest[..numerator_end]),
            replace_frac(&denominator_rest[..denominator_end])
        ));
        rest = &denominator_rest[denominator_end + 1..];
    }
    out.push_str(rest);
    out
}

fn convert_scripts(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let marker = chars[i];
        if marker == '^' || marker == '_' {
            i += 1;
            let (content, next) = read_group(&chars, i);
            i = next;
            let mapped: Option<String> = content
                .chars()
                .map(|ch| {
                    if marker == '^' {
                        to_superscript(ch)
                    } else {
                        to_subscript(ch)
                    }
                })
                .collect();
            match mapped {
                Some(mapped) => out.push_str(&mapped),
                None => {
                    out.push(marker);
                    if content.chars().count() <= 1
                        || content.chars().all(|ch| ch.is_alphanumeric())
                    {
                        out.push_str(&content);
                    } else {
                        out.push('(');
                        out.push_str(&content);
                        out.push(')');
                    }
                }
            }
        } else {
            out.push(marker);
            i += 1;
        }
    }
    out
}

fn read_group(chars: &[char], start: usize) -> (String, usize) {
    if start < chars.len() && chars[start] == '{' {
        let mut depth = 1usize;
        let mut index = start + 1;
        while index < chars.len() {
            match chars[index] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (chars[start + 1..index].iter().collect(), index + 1);
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
    if start < chars.len() {
        (chars[start].to_string(), start + 1)
    } else {
        (String::new(), start)
    }
}

fn to_superscript(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'a' => 'ᵃ',
        'b' => 'ᵇ',
        'c' => 'ᶜ',
        'd' => 'ᵈ',
        'e' => 'ᵉ',
        'f' => 'ᶠ',
        'g' => 'ᵍ',
        'h' => 'ʰ',
        'i' => 'ⁱ',
        'j' => 'ʲ',
        'k' => 'ᵏ',
        'l' => 'ˡ',
        'm' => 'ᵐ',
        'n' => 'ⁿ',
        'o' => 'ᵒ',
        'p' => 'ᵖ',
        'r' => 'ʳ',
        's' => 'ˢ',
        't' => 'ᵗ',
        'u' => 'ᵘ',
        'v' => 'ᵛ',
        'w' => 'ʷ',
        'x' => 'ˣ',
        'y' => 'ʸ',
        'z' => 'ᶻ',
        'A' => 'ᴬ',
        'B' => 'ᴮ',
        'D' => 'ᴰ',
        'E' => 'ᴱ',
        'G' => 'ᴳ',
        'H' => 'ᴴ',
        'I' => 'ᴵ',
        'J' => 'ᴶ',
        'K' => 'ᴷ',
        'L' => 'ᴸ',
        'M' => 'ᴹ',
        'N' => 'ᴺ',
        'O' => 'ᴼ',
        'P' => 'ᴾ',
        'R' => 'ᴿ',
        'T' => 'ᵀ',
        'U' => 'ᵁ',
        'V' => 'ⱽ',
        'W' => 'ᵂ',
        'α' => 'ᵅ',
        'β' => 'ᵝ',
        'γ' => 'ᵞ',
        'δ' => 'ᵟ',
        'ε' => 'ᵋ',
        'θ' => 'ᶿ',
        'ι' => 'ᶥ',
        'φ' => 'ᵠ',
        'χ' => 'ᵡ',
        _ => return None,
    })
}

fn to_subscript(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        'β' => 'ᵦ',
        'γ' => 'ᵧ',
        'ρ' => 'ᵨ',
        'φ' => 'ᵩ',
        'χ' => 'ᵪ',
        _ => return None,
    })
}

fn strip_backslashes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            if i + 1 < chars.len() && chars[i + 1] == '\\' {
                out.push('\n');
                i += 2;
                continue;
            }
            let mut end = i + 1;
            while end < chars.len() && chars[end].is_ascii_alphabetic() {
                end += 1;
            }
            if end == i + 1 {
                if end < chars.len() {
                    out.push(chars[end]);
                    i = end + 1;
                } else {
                    i += 1;
                }
            } else {
                out.extend(&chars[i + 1..end]);
                i = end;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
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
    fn math_becomes_readable_unicode() {
        let rendered = convert_math(
            r"Drop $|M|$ while $\text{UPE}\rightarrow\text{MLPs}$ costs $58\%$ and $f_p^m$; $$\text{ACA}_X^{\alpha}(t) = \text{FF}(x) \tag{3}$$",
        );
        assert!(!rendered.contains('$'), "dollar left: {rendered}");
        assert!(rendered.contains("|M|"), "{rendered}");
        assert!(rendered.contains("UPE→MLPs"), "{rendered}");
        assert!(rendered.contains("58%"), "{rendered}");
        assert!(rendered.contains("fₚᵐ"), "{rendered}");
        assert!(rendered.contains("ACA_X"), "{rendered}");
        assert!(rendered.contains('ᵅ'), "{rendered}");
        assert!(rendered.contains("(3)"), "{rendered}");
    }

    #[test]
    fn stray_escapes_outside_math_are_cleaned() {
        assert_eq!(convert_stray_latex(r"58\% and \{x\} \& \_"), "58% and {x} & _");
    }

    #[test]
    fn citations_become_inline_code_chips() {
        let rendered =
            convert_citations("Supported [single_column_1/content.md:75-89] and [notes/a.pdf:12].");
        assert_eq!(
            rendered,
            "Supported `single_column_1/content.md:75-89` and `notes/a.pdf:12`."
        );
        // markdown link text is left alone
        let link = convert_citations("See [paper.md](https://example.com).");
        assert_eq!(link, "See [paper.md](https://example.com).");
    }

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

//! Markdown assembly — exact port of pdfium_probe's `render_regions` +
//! its text helpers: every region is OCR'd independently and rendered at
//! its position in the reading order; optional asset descriptions are
//! supplied only for the image alt text.

use std::collections::HashMap;
use std::sync::Arc;

use image::RgbaImage;

use crate::analyze::{expand_asset_region, to_luma_pil};
use crate::error::{OcrError, Result};
use crate::layout::{Region, RegionKind};

/// Region labels that are never rendered (pdfium_probe `DROP_LABELS`).
const DROP_LABELS: [&str; 1] = ["reference"];

/// Labels rendered as plain text.
const TEXT_LABELS: [&str; 8] = [
    "text",
    "abstract",
    "content",
    "aside_text",
    "reference_content",
    "vertical_text",
    "footnote",
    "vision_footnote",
];

/// Labels rendered as `## heading`.
const HEADING_LABELS: [&str; 2] = ["doc_title", "paragraph_title"];

/// Captions rendered as plain text.
const CAPTION_LABELS: [&str; 1] = ["figure_title"];

/// Labels rendered as saved image assets (figures or tables).
const ASSET_LABELS: [&str; 3] = ["table", "image", "chart"];

/// A saved asset's semantic kind, used for its generic fallback alt text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Figure,
    Table,
}

impl AssetKind {
    #[must_use]
    pub fn generic_alt(self) -> &'static str {
        match self {
            Self::Figure => "Figure",
            Self::Table => "Table",
        }
    }

    #[must_use]
    fn file_prefix(self) -> &'static str {
        match self {
            Self::Figure => "figure",
            Self::Table => "table",
        }
    }
}

/// A final expanded/trimmed asset crop. The same encoded bytes are saved,
/// embedded and supplied to optional image analysis: JPEG q90 for opaque
/// crops (the scan norm), PNG only when the crop really has transparency.
#[derive(Debug, Clone)]
pub struct AssetCrop {
    pub region_index: usize,
    pub kind: AssetKind,
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
}

/// Per-document counters used for asset naming.
#[derive(Default)]
pub struct RenderCtx {
    pub table_n: u32,
    pub figure_n: u32,
}

pub(crate) fn is_asset_region(region: &Region) -> bool {
    ASSET_LABELS.contains(&region.label.as_str())
}

/// A region crop with its page, for OCR + asset extraction.
pub struct RegionJob<'a> {
    /// The region itself.
    pub region: &'a Region,
    /// The page bitmap the region belongs to.
    pub page: &'a RgbaImage,
}

// ---------------- text helpers (pdfium_probe ports) ----------------

/// Collapse whitespace runs to single spaces.
#[must_use]
pub fn normalize_text(text: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending = !out.is_empty();
        } else {
            if pending {
                out.push(' ');
                pending = false;
            }
            out.push(ch);
        }
    }
    out
}

/// Normalize an image-analysis response for direct use inside `![alt](…)`.
/// Whitespace is collapsed first; Markdown delimiters are escaped so model
/// text cannot close the alt text or change the surrounding image syntax.
#[must_use]
pub fn normalize_analysis_alt(raw: &str) -> String {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        if INLINE_MARKUP.contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

const INLINE_MARKUP: &str = "\\`*_{}[]<>#!|";

fn is_ordered_list_start(text: &str) -> bool {
    let rest = text.trim_start_matches(|c: char| c.is_ascii_digit());
    if let Some(first) = rest.chars().next() {
        if first == '.' || first == ')' {
            return rest.len() > 1
                && rest[1..].chars().next().is_some_and(|c| c.is_whitespace());
        }
    }
    false
}

/// Escape markdown special characters (pdfium_probe `escape_markdown`),
/// except that `**` pairs are kept intact — our OCR model emits real
/// bold markers, and escaping them would render as literal `\*\*`.
fn escape_markdown(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    if let Some(&first) = chars.first() {
        if first == '-' || first == '+' {
            out.push('\\');
            out.push(first);
            i = 1;
        } else if first.is_ascii() && first.is_ascii_digit() && is_ordered_list_start(text) {
            let mut sep = 1usize;
            while sep < chars.len() && chars[sep].is_ascii() && chars[sep].is_ascii_digit() {
                sep += 1;
            }
            out.push_str(&chars[..sep].iter().collect::<String>());
            if sep < chars.len() {
                out.push('\\');
                out.push(chars[sep]);
            }
            i = sep + 1;
        }
    }
    while i < chars.len() {
        let ch = chars[i];
        if ch == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            out.push('*');
            out.push('*');
            i += 2;
            continue;
        }
        if INLINE_MARKUP.contains(ch) {
            out.push('\\');
        }
        out.push(ch);
        i += 1;
    }
    out
}

const SUPERSCRIPT_MAP: [(char, char); 10] = [
    ('0', '\u{2070}'),
    ('1', '\u{00B9}'),
    ('2', '\u{00B2}'),
    ('3', '\u{00B3}'),
    ('4', '\u{2074}'),
    ('5', '\u{2075}'),
    ('6', '\u{2076}'),
    ('7', '\u{2077}'),
    ('8', '\u{2078}'),
    ('9', '\u{2079}'),
];

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Escape markdown but keep `$...$` math spans intact, folding glued
/// superscripts like `$^2$` into unicode superscripts (pdfium_probe
/// `escape_keep_math`).
fn escape_keep_math(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0usize;
    while i < n {
        if chars[i] == '$' {
            let (delim, len) = if i + 1 < n && chars[i + 1] == '$' {
                ("$$", 2usize)
            } else {
                ("$", 1usize)
            };
            let mut j = i + len;
            let mut found = None;
            while j + len <= n {
                if &chars[j..j + len].iter().collect::<String>() == delim {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(j) = found {
                let inner_t: String = chars[i + len..j].iter().collect::<String>().trim().to_string();
                let before = if i > 0 { Some(chars[i - 1]) } else { None };
                let after = if j + len < n { Some(chars[j + len]) } else { None };
                if !inner_t.is_empty() {
                    let sup_glued = delim == "$"
                        && inner_t.starts_with('^')
                        && (before.map(is_word_char).unwrap_or(false)
                            || after.map(is_word_char).unwrap_or(false));
                    if sup_glued {
                        let digits: String = inner_t.chars().filter(|c| c.is_ascii_digit()).collect();
                        if !digits.is_empty() {
                            let mapped: String = digits
                                .chars()
                                .map(|c| {
                                    SUPERSCRIPT_MAP
                                        .iter()
                                        .find(|(a, _)| *a == c)
                                        .map(|(_, b)| *b)
                                        .unwrap_or(c)
                                })
                                .collect();
                            out.push_str(&mapped);
                        } else {
                            out.push_str(inner_t.trim_start_matches('^'));
                        }
                        i = j + len;
                        continue;
                    }
                    out.push_str(delim);
                    out.push_str(&inner_t);
                    out.push_str(delim);
                    i = j + len;
                    continue;
                }
            }
            out.push_str("\\$");
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && chars[j] != '$' {
            j += 1;
        }
        let run: String = chars[i..j].iter().collect();
        out.push_str(&escape_markdown(&run));
        i = j;
    }
    out
}

/// Strip `$$`/`$` fences around a formula body (pdfium_probe
/// `strip_math_fences`).
fn strip_math_fences(ocr: &str) -> String {
    let t = ocr.trim();
    if t.starts_with("$$") && t.ends_with("$$") && t.len() > 4 {
        let inner = t[2..t.len() - 2].trim();
        if !inner.is_empty() {
            return inner.to_string();
        }
    }
    if t.starts_with('$') && t.ends_with('$') && t.len() > 2 {
        let inner = t[1..t.len() - 1].trim();
        if !inner.is_empty() {
            return inner.to_string();
        }
    }
    t.to_string()
}

/// Remove spurious `$` delimiters inside a display-formula body
/// (pdfium_probe `strip_inner_dollars`).
fn strip_inner_dollars(body: &str) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '$' && !(i > 0 && chars[i - 1] == '\\') {
            continue;
        }
        out.push(c);
    }
    out
}

/// Wrap a code body in a fenced block whose fence is longer than any run
/// inside it (pdfium_probe `fenced_code_block`).
fn fenced_code_block(code: &str) -> String {
    let mut longest_run = 0usize;
    let mut current = 0usize;
    for ch in code.chars() {
        if ch == '`' {
            current += 1;
            longest_run = longest_run.max(current);
        } else {
            current = 0;
        }
    }
    let fence = "`".repeat(longest_run.max(2) + 1);
    format!("{fence}\n{}\n{fence}", code.trim_end())
}

/// Leading digits of a string (for formula `\tag` attachment).
fn first_digits(ocr: &str) -> Option<String> {
    let mut it = ocr.chars().peekable();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            break;
        }
        it.next();
    }
    let mut digits = String::new();
    while let Some(c) = it.next() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            break;
        }
    }
    if digits.is_empty() { None } else { Some(digits) }
}

/// Strip markdown code fences the model may have wrapped output in.
fn strip_markdown_fences(raw: &str) -> String {
    let t = raw.trim();
    if t.starts_with("```") && t.ends_with("```") {
        let first = t.find('\n');
        let last = t.rfind('\n');
        if let (Some(f), Some(l)) = (first, last) {
            if f < l {
                return t[f + 1..l].trim().to_string();
            }
        }
        return t.trim_matches('`').trim().to_string();
    }
    t.to_string()
}

// ---------------- crops (pdfium_probe ports) ----------------

/// Crop a box out of a page image (coordinates truncated toward zero,
/// clamped to the page, exclusive right/bottom edge).
#[must_use]
pub fn crop_region(page: &RgbaImage, xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> RgbaImage {
    let (w, h) = (page.width() as i64, page.height() as i64);
    let x1 = (xmin as i64).clamp(0, w);
    let y1 = (ymin as i64).clamp(0, h);
    let x2 = (xmax as i64).clamp(0, w);
    let y2 = (ymax as i64).clamp(0, h);
    if x2 <= x1 || y2 <= y1 {
        return RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
    }
    image::imageops::crop_imm(page, x1 as u32, y1 as u32, (x2 - x1) as u32, (y2 - y1) as u32)
        .to_image()
}

/// PNG bytes of a region crop (pdfium_probe `png_bytes`).
pub fn png_bytes(img: &RgbaImage) -> Vec<u8> {
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .expect("png encode");
    buf
}

/// JPEG quality for opaque asset crops. q90 is visually lossless for scanned
/// figures and text, and roughly an order of magnitude smaller than the RGBA
/// PNG the same pixels would produce.
const ASSET_JPEG_QUALITY: u8 = 90;

/// Encode a final asset crop: JPEG q90 when every pixel is opaque (the scan
/// norm), PNG when the crop genuinely carries transparency. Returns the file
/// extension along with the bytes.
pub fn asset_bytes(img: &RgbaImage) -> (&'static str, Vec<u8>) {
    if img.pixels().any(|p| p.0[3] != 255) {
        return ("png", png_bytes(img));
    }
    use image::ImageEncoder;
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, ASSET_JPEG_QUALITY)
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .expect("jpeg encode");
    ("jpg", buf)
}

/// Downscale a crop to at most `max_side` with the fixed-point bilinear
/// (pdfium_probe `sample_capped`).
fn sample_capped(img: &RgbaImage, max_side: u32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= max_side {
        return img.clone();
    }
    let scale = f64::from(max_side) / f64::from(w.max(h));
    let nw = ((f64::from(w) * scale).round_ties_even() as u32).max(1);
    let nh = ((f64::from(h) * scale).round_ties_even() as u32).max(1);
    crate::preprocess::resize_fixed_point(img, nw, nh)
}

/// Max side for OCR'd region crops (pdfium_probe `MAX_SAMPLED_SIDE`).
pub const MAX_SAMPLED_SIDE: u32 = 768;

// ---------------- assembly (pdfium_probe render_regions port) ----------------

/// Build the OCR image for a region (equations at full resolution,
/// everything else sampled to [`MAX_SAMPLED_SIDE`]).
#[must_use]
pub fn region_image(job: &RegionJob) -> RgbaImage {
    let crop = crop_region(job.page, job.region.bbox[0] as f64, job.region.bbox[1] as f64, job.region.bbox[2] as f64, job.region.bbox[3] as f64);
    if job.region.kind == RegionKind::Equation {
        crop
    } else {
        sample_capped(&crop, MAX_SAMPLED_SIDE)
    }
}

/// Prepare the final expanded/trimmed PNG for every asset in a page. The
/// returned bytes are exactly the bytes written to disk and later embedded,
/// which keeps image analysis aligned with what the reader sees.
#[allow(clippy::too_many_arguments)]
pub fn prepare_asset_crops(
    page_no: u32,
    regions: &[Region],
    omitted: &std::collections::HashSet<usize>,
    page: &RgbaImage,
    ctx: &mut RenderCtx,
    assets_dir: &std::path::Path,
) -> Result<Vec<AssetCrop>> {
    let gray = to_luma_pil(page);
    let (page_w, page_h) = (f64::from(page.width()), f64::from(page.height()));
    let mut assets = Vec::new();
    for (ri, r) in regions.iter().enumerate() {
        if DROP_LABELS.contains(&r.label.as_str())
            || omitted.contains(&ri)
            || !is_asset_region(r)
        {
            continue;
        }
        let kind = if r.label == "table" {
            ctx.table_n += 1;
            AssetKind::Table
        } else {
            ctx.figure_n += 1;
            AssetKind::Figure
        };
        let n = match kind {
            AssetKind::Figure => ctx.figure_n,
            AssetKind::Table => ctx.table_n,
        };
        let [ex1, ey1, ex2, ey2] = expand_asset_region(r, regions, page_w, page_h, &gray);
        let crop = crop_region(page, ex1, ey1, ex2, ey2);
        let (ext, encoded) = asset_bytes(&crop);
        let name = format!("{}_{n}_p{page_no:02}_{n:02}.{ext}", kind.file_prefix());
        let bytes = Arc::new(encoded);
        std::fs::write(assets_dir.join(&name), bytes.as_ref())
            .map_err(|e| OcrError::Output(format!("could not save {name}: {e}")))?;
        assets.push(AssetCrop { region_index: ri, kind, name, bytes });
    }
    Ok(assets)
}

/// Render one page's markdown fragments (exact port of pdfium_probe
/// `render_regions`). `ocr` supplies the per-region transcription; assets
/// have already been prepared by [`prepare_asset_crops`]. Successful image
/// analyses replace only the generic Figure/Table alt text.
#[allow(clippy::too_many_arguments)]
pub fn render_page_fragments(
    regions: &[Region],
    omitted: &std::collections::HashSet<usize>,
    ocr: &dyn Fn(usize, &Region) -> Result<String>,
    assets: &[AssetCrop],
    asset_descriptions: &HashMap<usize, String>,
    assets_ref: &str,
) -> Result<Vec<String>> {
    let mut frags: Vec<String> = Vec::new();
    let mut pending_display: Option<String> = None; // awaiting \tag

    for (ri, r) in regions.iter().enumerate() {
        if DROP_LABELS.contains(&r.label.as_str()) || omitted.contains(&ri) {
            continue;
        }
        if r.label == "inline_formula" {
            // never OCR'd separately
            continue;
        }
        let text = ocr(ri, r)?;

        if is_asset_region(r) {
            pending_display = None;
            let asset = assets.iter().find(|asset| asset.region_index == ri).ok_or_else(|| {
                OcrError::Other(format!("asset region {ri} has no prepared crop"))
            })?;
            let alt = asset_descriptions
                .get(&ri)
                .filter(|description| !description.trim().is_empty())
                .map(String::as_str)
                .unwrap_or_else(|| asset.kind.generic_alt());
            frags.push(format!("![{alt}]({assets_ref}/{})", asset.name));
            continue;
        }
        if r.label == "display_formula" {
            pending_display = None;
            let body = strip_inner_dollars(&strip_math_fences(&text)).trim().to_string();
            if !body.is_empty() {
                pending_display = Some(format!("$$\n{body}\n$$"));
            }
            continue;
        }
        if r.label == "formula_number" {
            // attach as \tag to the pending display formula
            if let (Some(frag), Some(m)) = (&pending_display, first_digits(&text)) {
                let new_frag = format!("{}\\tag{{{m}}}\n$$", &frag[..frag.len() - 3]);
                pending_display = Some(new_frag);
            }
            continue;
        }
        // flush pending display formula
        if let Some(f) = pending_display.take() {
            frags.push(f);
        }
        if HEADING_LABELS.contains(&r.label.as_str()) {
            let text = normalize_text(&text);
            if text.is_empty() {
                continue;
            }
            frags.push(format!("## {}", escape_keep_math(&text)));
            continue;
        }
        if CAPTION_LABELS.contains(&r.label.as_str()) {
            let text = normalize_text(&text);
            if !text.is_empty() {
                frags.push(escape_keep_math(&text));
            }
            continue;
        }
        if r.label == "algorithm" {
            if !text.trim().is_empty() {
                frags.push(fenced_code_block(text.trim()));
            }
            continue;
        }
        if TEXT_LABELS.contains(&r.label.as_str()) {
            let text = normalize_text(&text);
            if !text.is_empty() {
                frags.push(escape_keep_math(&text));
            }
            continue;
        }
    }
    if let Some(f) = pending_display.take() {
        frags.push(f);
    }
    Ok(frags)
}

/// Merge consecutive bullet fragments into one bullet list (pdfium_probe
/// `group_bullets`).
#[must_use]
pub fn group_bullets(frags: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut bullets: Vec<String> = Vec::new();
    for f in frags {
        if f.starts_with("\\- ") || f.starts_with("- ") {
            let item = if f.starts_with("\\- ") { &f[2..] } else { &f[1..] };
            bullets.push(format!("- {item}"));
        } else {
            if !bullets.is_empty() {
                out.push(bullets.join("\n"));
                bullets.clear();
            }
            out.push(f);
        }
    }
    if !bullets.is_empty() {
        out.push(bullets.join("\n"));
    }
    out
}

/// Strip markdown fences a model may have wrapped a region in.
#[must_use]
pub fn clean_region_text(raw: &str) -> String {
    strip_markdown_fences(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_keeps_math_and_superscripts() {
        assert_eq!(escape_keep_math("a $b$ c"), "a $b$ c");
        assert_eq!(escape_keep_math("x$^2$"), "x\u{00B2}");
        assert_eq!(escape_keep_math("2.5x$^3$"), "2.5x\u{00B3}");
        assert_eq!(escape_keep_math("_i_"), "\\_i\\_");
        assert_eq!(escape_keep_math("A$_{ij}$"), "A$_{ij}$");
        assert_eq!(escape_keep_math("stray $ dollar"), "stray \\$ dollar");
        // bold pairs survive (the model emits real **bold** markers)
        assert_eq!(escape_keep_math("**bold** text"), "**bold** text");
        assert_eq!(escape_keep_math("**a** *b* c"), "**a** \\*b\\* c");
    }

    #[test]
    fn math_fences_and_tags() {
        assert_eq!(strip_math_fences("$$ x+y $$"), "x+y");
        assert_eq!(strip_math_fences("$x$"), "x");
        assert_eq!(strip_math_fences("plain"), "plain");
        assert_eq!(strip_inner_dollars("$a$ + $b$"), "a + b");
        assert_eq!(strip_inner_dollars("\\$5"), "\\$5");
        assert_eq!(first_digits("(11)"), Some("11".to_string()));
        assert_eq!(first_digits("abc"), None);
    }

    #[test]
    fn fenced_blocks_and_fences() {
        assert_eq!(fenced_code_block("a"), "```\na\n```");
        assert_eq!(fenced_code_block("has `` inside"), "```\nhas `` inside\n```");
        assert_eq!(clean_region_text("```\ncode\n```"), "code");
        assert_eq!(clean_region_text("plain"), "plain");
    }

    #[test]
    fn bullets_group() {
        // fragments come from escape_keep_math, which escapes the leading
        // dash ("\\- a"); group_bullets turns them into a clean list
        let frags = vec![
            "one".to_string(),
            "\\- a".to_string(),
            "\\- b".to_string(),
            "two".to_string(),
        ];
        let out = group_bullets(frags);
        // exact pdfium_probe behavior: escaped dash + f[2..] keeps the
        // space (their own md shows "-  We propose…")
        assert_eq!(out, vec!["one".to_string(), "-  a\n-  b".to_string(), "two".to_string()]);
    }

    #[test]
    fn heading_normalization() {
        assert_eq!(normalize_text("a   b\n\n c"), "a b c");
    }

    #[test]
    fn analysis_alt_normalizes_and_escapes_markdown_delimiters() {
        assert_eq!(
            normalize_analysis_alt("  A\n chart ](x) * value\t"),
            "A chart \\](x) \\* value"
        );
        assert_eq!(normalize_analysis_alt(" \n\t "), "");
    }

    #[test]
    fn asset_rendering_uses_generic_kind_fallbacks() {
        let regions = vec![
            Region {
                kind: RegionKind::Figure,
                label: "chart".to_string(),
                confidence: 1.0,
                bbox: [0.0, 0.0, 1.0, 1.0],
                order: 0,
            },
            Region {
                kind: RegionKind::Table,
                label: "table".to_string(),
                confidence: 1.0,
                bbox: [1.0, 1.0, 2.0, 2.0],
                order: 1,
            },
        ];
        let assets = vec![
            AssetCrop {
                region_index: 0,
                kind: AssetKind::Figure,
                name: "figure_1.png".to_string(),
                bytes: Arc::new(Vec::new()),
            },
            AssetCrop {
                region_index: 1,
                kind: AssetKind::Table,
                name: "table_1.png".to_string(),
                bytes: Arc::new(Vec::new()),
            },
        ];
        let omitted = std::collections::HashSet::new();
        let output = render_page_fragments(
            &regions,
            &omitted,
            &|_, _| Ok(String::new()),
            &assets,
            &HashMap::new(),
            "assets",
        )
        .unwrap();
        assert_eq!(
            output,
            vec![
                "![Figure](assets/figure_1.png)".to_string(),
                "![Table](assets/table_1.png)".to_string(),
            ]
        );
    }

    #[test]
    fn asset_rendering_only_replaces_alt_text() {
        let region = Region {
            kind: RegionKind::Figure,
            label: "image".to_string(),
            confidence: 1.0,
            bbox: [0.0, 0.0, 1.0, 1.0],
            order: 0,
        };
        let asset = AssetCrop {
            region_index: 0,
            kind: AssetKind::Figure,
            name: "figure_1.png".to_string(),
            bytes: Arc::new(Vec::new()),
        };
        let mut descriptions = HashMap::new();
        descriptions.insert(0, "A chart \\]\\(safe\\)".to_string());
        let output = render_page_fragments(
            &[region],
            &std::collections::HashSet::new(),
            &|_, _| Ok("caption remains".to_string()),
            &[asset],
            &descriptions,
            "assets",
        )
        .unwrap();
        assert_eq!(output, vec!["![A chart \\]\\(safe\\)](assets/figure_1.png)".to_string()]);
    }
}

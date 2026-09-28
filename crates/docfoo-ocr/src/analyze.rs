//! Reading-order analysis (exact port of pdfium_probe's `pipeline.rs`):
//! dedupe -> drop headers/footers/other -> model `order_seq` with a
//! bubble pass that fixes same-column vertical inversions. Also ports
//! `omitted_regions` (containment omission) and `expand_asset_region`
//! (content-driven, neighbor-bounded asset crop expansion), plus the
//! `layout_block` LLM-prompt helper.

use std::collections::HashSet;

use image::RgbaImage;

use crate::layout::{Region, RegionKind};

/// NMS IoU threshold for same-kind duplicate suppression.
const NMS_IOU: f32 = 0.5;

/// Kinds that never enter the reading order.
const DROP_KINDS: [RegionKind; 3] = [RegionKind::Header, RegionKind::Footer, RegionKind::Other];

/// Max pixels an asset crop may expand beyond the detection box (before
/// the content scan and neighbor bounds cut it short).
const ASSET_PAD: f64 = 60.0;

/// Extra pixels included past the last content pixel in an expanded crop.
const CONTENT_MARGIN: i64 = 3;

/// Luma threshold: pixels darker than this count as content.
const CONTENT_LEVEL: u8 = 245;

fn iou2(a: &Region, b: &Region) -> f32 {
    let [ax1, ay1, ax2, ay2] = a.bbox;
    let [bx1, by1, bx2, by2] = b.bbox;
    let x1 = ax1.max(bx1);
    let y1 = ay1.max(by1);
    let x2 = ax2.min(bx2);
    let y2 = ay2.min(by2);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let ua = (ax2 - ax1) * (ay2 - ay1) + (bx2 - bx1) * (by2 - by1) - inter;
    if ua > 0.0 { inter / ua } else { 0.0 }
}

fn area(r: &Region) -> f64 {
    f64::from(r.bbox[2] - r.bbox[0]) * f64::from(r.bbox[3] - r.bbox[1])
}

fn inside(a: &Region, b: &Region) -> bool {
    b.bbox[0] <= a.bbox[0]
        && b.bbox[1] <= a.bbox[1]
        && a.bbox[2] <= b.bbox[2]
        && a.bbox[3] <= b.bbox[3]
}

/// Full reading order (exact port of pdfium_probe `order_regions`):
/// dedupe same-kind IoU duplicates -> drop headers/footers/other -> sort
/// by the model's predicted `order_seq` (then ymin) -> bubble pass that
/// swaps adjacent x-overlapping pairs whose vertical order disagrees.
#[must_use]
pub fn order_regions(regions: Vec<Region>) -> Vec<Region> {
    // Dedupe: keep the highest-scoring region per same-kind near-duplicate
    // (stable: ties keep the earlier index).
    let mut order: Vec<usize> = (0..regions.len()).collect();
    order.sort_by(|&a, &b| {
        regions[b]
            .confidence
            .partial_cmp(&regions[a].confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept_idx: Vec<usize> = Vec::new();
    for i in order {
        let dup = kept_idx.iter().any(|&k| {
            regions[k].kind == regions[i].kind && iou2(&regions[k], &regions[i]) >= NMS_IOU
        });
        if !dup {
            kept_idx.push(i);
        }
    }
    kept_idx.sort_unstable();
    let mut kept: Vec<Region> = kept_idx.iter().map(|&i| regions[i].clone()).collect();
    kept.retain(|r| !DROP_KINDS.contains(&r.kind));
    kept.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then(
                a.bbox[1]
                    .partial_cmp(&b.bbox[1])
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    // Bubble pass: fix same-column vertical inversions in the model order.
    let n = kept.len();
    for _ in 0..n {
        let mut swapped = false;
        for i in 0..n.saturating_sub(1) {
            let (a, b) = (&kept[i], &kept[i + 1]);
            let ox = a.bbox[2].min(b.bbox[2]) - a.bbox[0].max(b.bbox[0]);
            if ox > 0.0 && a.bbox[1] > b.bbox[1] {
                kept.swap(i, i + 1);
                swapped = true;
            }
        }
        if !swapped {
            break;
        }
    }
    kept
}

/// Indices of regions fully contained in a larger region (exact port of
/// pdfium_probe `omitted_regions`); `"reference"`-labelled regions are
/// excluded from both the containment test and the omission set.
#[must_use]
pub fn omitted_regions(regions: &[Region]) -> HashSet<usize> {
    let rendered: Vec<usize> = (0..regions.len())
        .filter(|&i| regions[i].label != "reference")
        .collect();
    let mut omitted = HashSet::new();
    for (ai, a) in regions.iter().enumerate() {
        if a.label == "reference" {
            continue;
        }
        for &b in &rendered {
            if regions[b].bbox == a.bbox {
                // identity check: same box = same region
                continue;
            }
            if area(&regions[b]) > area(a) && inside(a, &regions[b]) {
                omitted.insert(ai);
                break;
            }
        }
    }
    omitted
}

// ---------------- asset crop expansion (pdfium_probe expand_asset_region) ----------------

/// PIL "L" conversion: `(19595*r + 38470*g + 7471*b + 0x8000) >> 16`.
#[must_use]
pub fn to_luma_pil(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x as u32, y as u32);
            let v = (u64::from(p[0]) * 19595
                + u64::from(p[1]) * 38470
                + u64::from(p[2]) * 7471
                + 0x8000)
                >> 16;
            out[y * w + x] = v as u8;
        }
    }
    out
}

/// Expand a figure/table detection box to the tight content boundary:
/// at most [`ASSET_PAD`] pixels on each side, further bounded by the
/// nearest neighbour region on that side, then trimmed to the last
/// content pixel (luma < [`CONTENT_LEVEL`]) plus [`CONTENT_MARGIN`].
/// Returns `[xmin, ymin, xmax, ymax]` (exact port of pdfium_probe
/// `expand_asset_region`).
#[must_use]
pub fn expand_asset_region(
    r: &Region,
    regions: &[Region],
    page_w: f64,
    page_h: f64,
    gray: &[u8],
) -> [f64; 4] {
    let mut allowed = [
        f64::from(r.bbox[1]).min(ASSET_PAD),            // top
        (page_h - f64::from(r.bbox[3])).min(ASSET_PAD), // bottom
        f64::from(r.bbox[0]).min(ASSET_PAD),            // left
        (page_w - f64::from(r.bbox[2])).min(ASSET_PAD), // right
    ];
    for o in regions {
        if o.bbox == r.bbox {
            continue;
        }
        let ox = r.bbox[2].min(o.bbox[2]) - r.bbox[0].max(o.bbox[0]);
        let oy = r.bbox[3].min(o.bbox[3]) - r.bbox[1].max(o.bbox[1]);
        if ox > 0.0 {
            if o.bbox[1] >= r.bbox[3] {
                allowed[1] = allowed[1].min(f64::from(o.bbox[1] - r.bbox[3]));
            } else if o.bbox[3] <= r.bbox[1] {
                allowed[0] = allowed[0].min(f64::from(r.bbox[1] - o.bbox[3]));
            } else if o.bbox[1] < r.bbox[1] && o.bbox[3] > r.bbox[1] {
                allowed[0] = 0.0; // neighbor overlaps the top edge
            } else if o.bbox[1] < r.bbox[3] && o.bbox[3] > r.bbox[3] {
                allowed[1] = 0.0; // neighbor overlaps the bottom edge
            }
        }
        if oy > 0.0 {
            if o.bbox[0] >= r.bbox[2] {
                allowed[3] = allowed[3].min(f64::from(o.bbox[0] - r.bbox[2]));
            } else if o.bbox[2] <= r.bbox[0] {
                allowed[2] = allowed[2].min(f64::from(r.bbox[0] - o.bbox[2]));
            } else if o.bbox[0] < r.bbox[0] && o.bbox[2] > r.bbox[0] {
                allowed[2] = 0.0; // neighbor overlaps the left edge
            } else if o.bbox[0] < r.bbox[2] && o.bbox[2] > r.bbox[2] {
                allowed[3] = 0.0; // neighbor overlaps the right edge
            }
        }
    }
    for a in allowed.iter_mut() {
        *a = (*a as i64).max(0) as f64;
    }
    let (x1, y1, x2, y2) = (r.bbox[0] as i64, r.bbox[1] as i64, r.bbox[2] as i64, r.bbox[3] as i64);
    let (pw, ph) = (page_w as usize, page_h as usize);

    fn last_content(
        gray: &[u8],
        pw: usize,
        ph: usize,
        axis: usize,
        start: i64,
        end: i64,
        lo: usize,
        hi: usize,
    ) -> Option<usize> {
        let s = start.max(0) as usize;
        let e = (end as usize).min(if axis == 0 { ph } else { pw });
        let mut found = None;
        for i in s..e {
            let has = if axis == 0 {
                gray[i * pw + lo..i * pw + hi]
                    .iter()
                    .any(|&p| p < CONTENT_LEVEL)
            } else {
                (lo..hi).any(|y| gray[y * pw + i] < CONTENT_LEVEL)
            };
            if has {
                found = Some(i);
            }
        }
        found
    }

    let mut ex = [0i64; 4]; // top, bottom, left, right
    if allowed[0] > 0.0 {
        if let Some(f) = last_content(
            gray,
            pw,
            ph,
            0,
            y1 - allowed[0] as i64,
            y1,
            x1 as usize,
            x2 as usize,
        ) {
            ex[0] = (allowed[0] as i64).min(y1 - f as i64 + CONTENT_MARGIN);
        }
    }
    if allowed[1] > 0.0 {
        if let Some(f) = last_content(
            gray,
            pw,
            ph,
            0,
            y2,
            y2 + allowed[1] as i64,
            x1 as usize,
            x2 as usize,
        ) {
            ex[1] = (allowed[1] as i64).min(f as i64 - y2 + 1 + CONTENT_MARGIN);
        }
    }
    if allowed[2] > 0.0 {
        if let Some(f) = last_content(
            gray,
            pw,
            ph,
            1,
            x1 - allowed[2] as i64,
            x1,
            y1 as usize,
            y2 as usize,
        ) {
            ex[2] = (allowed[2] as i64).min(x1 - f as i64 + CONTENT_MARGIN);
        }
    }
    if allowed[3] > 0.0 {
        if let Some(f) = last_content(
            gray,
            pw,
            ph,
            1,
            x2,
            x2 + allowed[3] as i64,
            y1 as usize,
            y2 as usize,
        ) {
            ex[3] = (allowed[3] as i64).min(f as i64 - x2 + 1 + CONTENT_MARGIN);
        }
    }
    [
        x1 as f64 - ex[2] as f64,
        y1 as f64 - ex[0] as f64,
        x2 as f64 + ex[3] as f64,
        y2 as f64 + ex[1] as f64,
    ]
}

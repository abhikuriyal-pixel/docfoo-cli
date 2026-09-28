//! Probe: render a page, detect layout, print the READING ORDER and save
//! the expanded figure crops exactly as the pipeline would.
//! `cargo run -p docfoo-ocr --example figprobe -- <pdf> <page>`

use std::path::PathBuf;

use docfoo_ocr::{analyze, preprocess, warm_layout};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pdf = PathBuf::from(&args[1]);
    let page_no: u32 = args[2].parse().expect("page");
    let models_dir = PathBuf::from("models");
    let _ = warm_layout(&models_dir).expect("layout ready");

    let bytes = std::fs::read(&pdf).expect("read pdf");
    let rast = docfoo_ocr::raster::PdfiumRasterizer::new(
        bytes,
        300,
        &PathBuf::from("models/pdfium.dll"),
    )
    .expect("rasterizer");
    let rgba = rast.render_page(page_no - 1).expect("render");
    let (w, h) = (rgba.width(), rgba.height());
    let max = w.max(h);
    let scale = 2048.0 / f64::from(max);
    let rgba = preprocess::resize_fixed_point(
        &rgba,
        ((f64::from(w) * scale).round().max(1.0)) as u32,
        ((f64::from(h) * scale).round().max(1.0)) as u32,
    );
    let rgb = image::DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
    let regions = docfoo_ocr::layout::acquire_detector(&PathBuf::from("models/PP-DocLayoutV3.onnx"))
        .expect("detector")
        .detect_image(&rgb)
        .expect("detect");
    let ordered = analyze::order_regions(regions);
    let omitted = analyze::omitted_regions(&ordered);
    for (idx, r) in ordered.iter().enumerate() {
        println!(
            "  [{idx}] {} kind={:?} box=[{:.1},{:.1},{:.1},{:.1}] om={}",
            r.label,
            r.kind,
            r.bbox[0],
            r.bbox[1],
            r.bbox[2],
            r.bbox[3],
            omitted.contains(&idx)
        );
    }
    let figures: Vec<usize> = ordered
        .iter()
        .enumerate()
        .filter(|(idx, r)| r.kind == docfoo_ocr::layout::RegionKind::Figure && !omitted.contains(idx))
        .map(|(idx, _)| idx)
        .collect();
    println!(
        "page {page_no}: {}x{} -> {}x{} | {} regions, {} figures ({} omitted)",
        w,
        h,
        rgba.width(),
        rgba.height(),
        ordered.len(),
        figures.len(),
        omitted.len()
    );
    for (k, &fi) in figures.iter().enumerate() {
        let f = &ordered[fi];
        let gray = analyze::to_luma_pil(&rgba);
        let [ex1, ey1, ex2, ey2] = analyze::expand_asset_region(
            f,
            &ordered,
            f64::from(rgba.width()),
            f64::from(rgba.height()),
            &gray,
        );
        println!(
            "  fig {}: label={} box=[{:.1},{:.1},{:.1},{:.1}] -> expanded=[{:.1},{:.1},{:.1},{:.1}]",
            k + 1,
            f.label,
            f.bbox[0],
            f.bbox[1],
            f.bbox[2],
            f.bbox[3],
            ex1,
            ey1,
            ex2,
            ey2
        );
    }
    let out = PathBuf::from("figprobe");
    std::fs::create_dir_all(&out).unwrap();
    rgba.save(out.join(format!("page_{page_no:02}.png"))).unwrap();
    for (k, &fi) in figures.iter().enumerate() {
        let f = &ordered[fi];
        let gray = analyze::to_luma_pil(&rgba);
        let [ex1, ey1, ex2, ey2] = analyze::expand_asset_region(
            f,
            &ordered,
            f64::from(rgba.width()),
            f64::from(rgba.height()),
            &gray,
        );
        let crop = docfoo_ocr::assemble::crop_region(&rgba, ex1, ey1, ex2, ey2);
        crop.save(out.join(format!("fig{k}.png"))).unwrap();
    }
    println!("crops in ./figprobe");
}

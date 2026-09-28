//! Compare downscale methods on a real 300-dpi page: Triangle (old) vs
//! fixed-point OpenCV-exact bilinear (new). Prints mean/max abs diff.

use std::time::Instant;

fn main() {
    let pdf_path = std::env::args().nth(1).expect("pdf path");
    let dll = std::env::args().nth(2).expect("pdfium dll");
    let bytes = std::fs::read(&pdf_path).expect("read pdf");
    let rasterizer =
        docfoo_ocr::raster::PdfiumRasterizer::new(bytes, 300, std::path::Path::new(&dll))
            .expect("rasterizer");
    let page = rasterizer.render_page(0).expect("render page 1");
    println!("rendered: {}x{}", page.width(), page.height());

    let (w, h) = (page.width(), page.height());
    let max = w.max(h);
    let scale = 2048.0 / f64::from(max);
    let nw = ((f64::from(w) * scale).round()).max(1.0) as u32;
    let nh = ((f64::from(h) * scale).round()).max(1.0) as u32;
    println!("downscale to {nw}x{nh}");

    let t = Instant::now();
    let triangle = image::imageops::resize(&page, nw, nh, image::imageops::FilterType::Triangle);
    println!("Triangle: {} ms", t.elapsed().as_millis());

    let t = Instant::now();
    let fixed = docfoo_ocr::resize_fixed_point(&page, nw, nh);
    println!("fixed-point: {} ms", t.elapsed().as_millis());

    // per-channel diff stats
    let a = triangle.as_raw();
    let b = fixed.as_raw();
    assert_eq!(a.len(), b.len());
    let mut sum: u64 = 0;
    let mut max_diff: u32 = 0;
    let mut over_16 = 0usize;
    let mut over_8 = 0usize;
    for (x, y) in a.iter().zip(b.iter()) {
        let d = (*x as i32 - *y as i32).unsigned_abs();
        sum += u64::from(d);
        max_diff = max_diff.max(d);
        if d > 16 {
            over_16 += 1;
        }
        if d > 8 {
            over_8 += 1;
        }
    }
    let n = a.len();
    println!("mean abs diff: {:.3} / 255", sum as f64 / n as f64);
    println!("max abs diff: {max_diff} / 255");
    println!("pixels with diff > 8: {:.2}%", 100.0 * over_8 as f64 / n as f64);
    println!("pixels with diff > 16: {:.2}%", 100.0 * over_16 as f64 / n as f64);

    // save both for visual comparison
    image::DynamicImage::ImageRgba8(triangle.clone()).save("C:/Users/abhik/AppData/Local/Temp/down-triangle.png").unwrap();
    image::DynamicImage::ImageRgba8(fixed.clone()).save("C:/Users/abhik/AppData/Local/Temp/down-fixed.png").unwrap();
    println!("saved to /tmp/down-*.png");

    // practical test: does the layout model detect the same regions on both?
    if std::path::Path::new("models/onnxruntime.dll").is_file() {
        let rgb_a = image::DynamicImage::ImageRgba8(triangle).to_rgb8();
        let rgb_b = image::DynamicImage::ImageRgba8(fixed).to_rgb8();
        let model = std::path::Path::new("models/PP-DocLayoutV3.onnx");
        let ort_dll = std::path::Path::new("models/onnxruntime.dll");
        docfoo_ocr::layout::ensure_onnx_runtime(ort_dll).expect("ort");
        let mut det = docfoo_ocr::layout::acquire_detector(model).expect("detector");
        let ra = det.detect_image(&rgb_a).expect("detect a");
        let rb = det.detect_image(&rgb_b).expect("detect b");
        println!("layout regions: triangle={} fixed={}", ra.len(), rb.len());
        let same = ra.len() == rb.len()
            && ra.iter().zip(rb.iter()).all(|(a, b)| {
                a.kind == b.kind
                    && (a.bbox[0] - b.bbox[0]).abs() <= 1.0
                    && (a.bbox[1] - b.bbox[1]).abs() <= 1.0
                    && (a.bbox[2] - b.bbox[2]).abs() <= 1.0
                    && (a.bbox[3] - b.bbox[3]).abs() <= 1.0
            });
        println!("identical detections (<=1px box shift): {same}");
        for (a, b) in ra.iter().zip(rb.iter()).take(4) {
            println!("  {}: [{:.0},{:.0},{:.0},{:.0}] vs [{:.0},{:.0},{:.0},{:.0}]",
                a.label, a.bbox[0], a.bbox[1], a.bbox[2], a.bbox[3],
                b.bbox[0], b.bbox[1], b.bbox[2], b.bbox[3]);
        }
    }
}

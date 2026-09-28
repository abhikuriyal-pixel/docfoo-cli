//! Measure session-creation time + one inference per optimization level.
//! `cargo run -p docfoo-ocr --example loadtest -- <model.onnx>`

use std::time::Instant;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

fn main() {
    let path = std::env::args().nth(1).expect("model path");
    let only = std::env::args().nth(2); // "default" | "Level1" | "disable"
    let dll = std::env::args().nth(3); // optional onnxruntime.dll to bind
    if let Some(dll) = &dll {
        let t = std::time::Instant::now();
        ort::init_from(dll).expect("init_from").commit();
        eprintln!("[ort] dll bound in {} ms", t.elapsed().as_millis());
    } else {
        ort::init().commit();
    }
    // a tiny 8x8 gray image
    let rgb = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([(x * 4) as u8, (y * 4) as u8, 128])
    });
    let blob = docfoo_ocr::preprocess_blob(&rgb);

    let cases: [(&str, Option<GraphOptimizationLevel>); 3] = [
        ("default(L3)", None),
        ("Level1", Some(GraphOptimizationLevel::Level1)),
        ("Disable", Some(GraphOptimizationLevel::Disable)),
    ];
    for (name, level) in cases {
        if only.as_deref().is_some_and(|o| !name.starts_with(o)) {
            continue;
        }
        let t = Instant::now();
        let mut b = Session::builder().expect("builder");
        if let Some(l) = level {
            b = b.with_optimization_level(l).expect("opt level");
        }
        let mut session = b.commit_from_file(&path).expect("commit");
        let create_ms = t.elapsed().as_millis();

        let t = Instant::now();
        let blob2 = docfoo_ocr::preprocess_blob(&rgb);
        let scale = ndarray::Array2::<f32>::from_shape_vec((1, 2), vec![800.0f32 / 64.0, 800.0 / 64.0]).unwrap();
        let shape = ndarray::Array2::<f32>::from_shape_vec((1, 2), vec![800.0f32, 800.0]).unwrap();
        let mut feed: Vec<(String, ort::session::SessionInputValue)> = Vec::new();
        for input in session.inputs() {
            let name_in = input.name().to_owned();
            let tensor = match name_in.as_str() {
                "image" => ort::value::Tensor::from_array(blob2.clone()).unwrap(),
                "im_shape" => ort::value::Tensor::from_array(shape.clone()).unwrap(),
                "scale_factor" => ort::value::Tensor::from_array(scale.clone()).unwrap(),
                other => panic!("unknown input {other}"),
            };
            feed.push((name_in, tensor.into()));
        }
        let _ = session.run(feed).expect("run");
        let infer_ms = t.elapsed().as_millis();
        let _ = blob;
        println!("{name:>12}: session create {create_ms:>6} ms | one inference {infer_ms:>5} ms");
    }
}

//! Image preprocessing for PP-DocLayoutV3 (port of owl-ocr's
//! `layout/preprocess.rs`): resize the page to 800x800 with the cv2
//! `INTER_LINEAR` fixed-point port and normalize to `[0, 1]`. The model
//! consumes plain `[0, 1]` pixels and is told the resize happened via the
//! `scale_factor` feed, so its boxes come out in original-page
//! coordinates.

use ndarray::Array4;

/// Square input size of the layout model.
pub(crate) const INPUT_SIZE: u32 = 800;

/// `INPUT_SIZE` as f32, for coordinate math.
pub(crate) const INPUT_SIZE_F32: f32 = 800.0;

/// Resize `rgb` to `dw` x `dh` with the cv2 `INTER_LINEAR` fixed-point
/// algorithm (OpenCV's fixed-point 8u path). Ported verbatim from
/// owl-ocr; verified 0-pixel difference against `cv2.resize` on real
/// pages.
#[allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub(crate) fn resize_bilinear_rgb(rgb: &image::RgbImage, dw: u32, dh: u32) -> image::RgbImage {
    let (sw, sh) = (rgb.width() as usize, rgb.height() as usize);
    let (dw, dh) = (dw as usize, dh as usize);
    let cn = 3usize;
    let scale_x = 1.0 / (dw as f64 / sw as f64);
    let scale_y = 1.0 / (dh as f64 / sh as f64);

    // Horizontal coefficients: source offset + weights per destination
    // column (OpenCV's `xofs` / `cbuf`).
    let mut xofs = vec![0i32; dw];
    let mut alpha = vec![0i32; dw * 2];
    for dx in 0..dw {
        let mut fx = ((dx as f64 + 0.5) * scale_x - 0.5) as f32;
        let mut sx = fx.floor() as i64;
        fx -= sx as f32;
        if sx < 0 {
            fx = 0.0;
            sx = 0;
        }
        if sx as usize >= sw - 1 {
            fx = 0.0;
            sx = (sw - 1) as i64;
        }
        xofs[dx] = sx as i32;
        // cvRound of the f32 product (saturate_cast<short> semantics).
        alpha[dx * 2] = ((((1.0f32 - fx) * 2048.0f32) as f64) + 0.5).floor() as i32;
        alpha[dx * 2 + 1] = (((fx * 2048.0f32) as f64) + 0.5).floor() as i32;
    }
    // Vertical coefficients: source offset + weights per destination row.
    let mut yofs = vec![0i32; dh];
    let mut beta = vec![0i32; dh * 2];
    for dy in 0..dh {
        let mut fy = ((dy as f64 + 0.5) * scale_y - 0.5) as f32;
        let mut sy = fy.floor() as i64;
        fy -= sy as f32;
        if sy < 0 {
            fy = 0.0;
            sy = 0;
        }
        if sy as usize >= sh - 1 {
            fy = 0.0;
            sy = (sh - 1) as i64;
        }
        yofs[dy] = sy as i32;
        beta[dy * 2] = ((((1.0f32 - fy) * 2048.0f32) as f64) + 0.5).floor() as i32;
        beta[dy * 2 + 1] = (((fy * 2048.0f32) as f64) + 0.5).floor() as i32;
    }

    let pixels = rgb.as_raw();
    let mut hbuf = vec![0i32; dw * cn * 2];
    let mut out = vec![0u8; dw * dh * cn];
    for dy in 0..dh {
        let sy0 = yofs[dy] as usize;
        let sy1 = (sy0 + 1).min(sh - 1);
        // Horizontal pass for both source rows.
        for (sy, slot) in [(sy0, 0usize), (sy1, 1usize)] {
            let row = &pixels[sy * sw * cn..(sy + 1) * sw * cn];
            let hb = &mut hbuf[slot * dw * cn..(slot + 1) * dw * cn];
            for dx in 0..dw {
                let sx = xofs[dx] as usize * cn;
                let (a0, a1) = (alpha[dx * 2], alpha[dx * 2 + 1]);
                // When `sx` is clamped to the last pixel, `a1` is 0 and
                // the second tap must not read past the row.
                let sx1 = (sx + cn).min(row.len() - cn);
                for c in 0..cn {
                    hb[dx * cn + c] = row[sx + c] as i32 * a0 + row[sx1 + c] as i32 * a1;
                }
            }
        }
        let (b0, b1) = (beta[dy * 2], beta[dy * 2 + 1]);
        let dst = &mut out[dy * dw * cn..(dy + 1) * dw * cn];
        for dx in 0..dw * cn {
            let v = (((b0 * (hbuf[dx] >> 4)) >> 16) + ((b1 * (hbuf[dw * cn + dx] >> 4)) >> 16) + 2)
                >> 2;
            dst[dx] = v as u8;
        }
    }
    // The buffer length is `dw * dh * 3` by construction, so the closure
    // below always stays in bounds.
    image::RgbImage::from_fn(dw as u32, dh as u32, |x, y| {
        let i = (y as usize * dw + x as usize) * cn;
        image::Rgb([out[i], out[i + 1], out[i + 2]])
    })
}

/// Build the NCHW `(1, 3, 800, 800)` float blob for the model from `rgb`,
/// resized with the cv2-exact `INTER_LINEAR` port.
pub fn image_to_blob(rgb: &image::RgbImage) -> Array4<f32> {
    let resized = resize_bilinear_rgb(rgb, INPUT_SIZE, INPUT_SIZE);
    let size = INPUT_SIZE as usize;
    let mut blob = Array4::<f32>::zeros((1, 3, size, size));
    for (x, y, pixel) in resized.enumerate_pixels() {
        let (x_i, y_i) = (x as usize, y as usize);
        blob[[0, 0, y_i, x_i]] = f32::from(pixel[0]) / 255.0;
        blob[[0, 1, y_i, x_i]] = f32::from(pixel[1]) / 255.0;
        blob[[0, 2, y_i, x_i]] = f32::from(pixel[2]) / 255.0;
    }
    blob
}

/// RGBA variant of the cv2-exact fixed-point bilinear resize (channel
/// count only changes the row stride — the per-channel math is identical
/// to [`resize_bilinear_rgb`]). Used for the max-side downscale of page
/// bitmaps: the integer path is an order of magnitude faster than the
/// image-crate filters in debug builds, and matches the reference's
/// INTER_LINEAR semantics.
#[allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn resize_fixed_point(rgba: &image::RgbaImage, dw: u32, dh: u32) -> image::RgbaImage {
    let (sw, sh) = (rgba.width() as usize, rgba.height() as usize);
    let (dw, dh) = (dw as usize, dh as usize);
    let cn = 4usize;
    let scale_x = 1.0 / (dw as f64 / sw as f64);
    let scale_y = 1.0 / (dh as f64 / sh as f64);

    let mut xofs = vec![0i32; dw];
    let mut alpha = vec![0i32; dw * 2];
    for dx in 0..dw {
        let mut fx = ((dx as f64 + 0.5) * scale_x - 0.5) as f32;
        let mut sx = fx.floor() as i64;
        fx -= sx as f32;
        if sx < 0 {
            fx = 0.0;
            sx = 0;
        }
        if sx as usize >= sw - 1 {
            fx = 0.0;
            sx = (sw - 1) as i64;
        }
        xofs[dx] = sx as i32;
        alpha[dx * 2] = ((((1.0f32 - fx) * 2048.0f32) as f64) + 0.5).floor() as i32;
        alpha[dx * 2 + 1] = (((fx * 2048.0f32) as f64) + 0.5).floor() as i32;
    }
    let mut yofs = vec![0i32; dh];
    let mut beta = vec![0i32; dh * 2];
    for dy in 0..dh {
        let mut fy = ((dy as f64 + 0.5) * scale_y - 0.5) as f32;
        let mut sy = fy.floor() as i64;
        fy -= sy as f32;
        if sy < 0 {
            fy = 0.0;
            sy = 0;
        }
        if sy as usize >= sh - 1 {
            fy = 0.0;
            sy = (sh - 1) as i64;
        }
        yofs[dy] = sy as i32;
        beta[dy * 2] = ((((1.0f32 - fy) * 2048.0f32) as f64) + 0.5).floor() as i32;
        beta[dy * 2 + 1] = (((fy * 2048.0f32) as f64) + 0.5).floor() as i32;
    }

    let pixels = rgba.as_raw();
    let mut hbuf = vec![0i32; dw * cn * 2];
    let mut out = vec![0u8; dw * dh * cn];
    for dy in 0..dh {
        let sy0 = yofs[dy] as usize;
        let sy1 = (sy0 + 1).min(sh - 1);
        for (sy, slot) in [(sy0, 0usize), (sy1, 1usize)] {
            let row = &pixels[sy * sw * cn..(sy + 1) * sw * cn];
            let hb = &mut hbuf[slot * dw * cn..(slot + 1) * dw * cn];
            for dx in 0..dw {
                let sx = xofs[dx] as usize * cn;
                let (a0, a1) = (alpha[dx * 2], alpha[dx * 2 + 1]);
                let sx1 = (sx + cn).min(row.len() - cn);
                for c in 0..cn {
                    hb[dx * cn + c] = row[sx + c] as i32 * a0 + row[sx1 + c] as i32 * a1;
                }
            }
        }
        let (b0, b1) = (beta[dy * 2], beta[dy * 2 + 1]);
        let dst = &mut out[dy * dw * cn..(dy + 1) * dw * cn];
        for dx in 0..dw * cn {
            let v = (((b0 * (hbuf[dx] >> 4)) >> 16) + ((b1 * (hbuf[dw * cn + dx] >> 4)) >> 16) + 2)
                >> 2;
            dst[dx] = v as u8;
        }
    }
    image::RgbaImage::from_raw(dw as u32, dh as u32, out).expect("buffer is dw*dh*4")
}

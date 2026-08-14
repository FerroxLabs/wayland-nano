pub mod heuristic;

/// Redaction is best-effort defense in depth. A decode/encode failure returns
/// the original bytes and must be logged by the integrating caller.
pub fn redact_png_best_effort(bytes: Vec<u8>) -> (Vec<u8>, bool) {
    let Ok(decoded) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) else {
        return (bytes, false);
    };
    let mut rgba = decoded.into_rgba8();
    let runs = heuristic::detect_password_field_runs(&rgba);
    if runs.is_empty() {
        return (bytes, false);
    }
    for (x0, y0, x1, y1) in runs {
        let width = x1 - x0 + 1;
        let height = y1 - y0 + 1;
        let crop = image::imageops::crop_imm(&rgba, x0, y0, width, height).to_image();
        let blurred = image::imageops::blur(&crop, 8.0);
        image::imageops::replace(&mut rgba, &blurred, i64::from(x0), i64::from(y0));
    }
    let mut output = Vec::new();
    if image::DynamicImage::ImageRgba8(rgba)
        .write_to(
            &mut std::io::Cursor::new(&mut output),
            image::ImageFormat::Png,
        )
        .is_err()
    {
        return (bytes, false);
    }
    (output, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Best-effort contract: a decode failure passes the original bytes
    /// through untouched and never blocks the op (redact is defense in
    /// depth beneath the gate, never load-bearing).
    #[test]
    fn undecodable_bytes_pass_through() {
        let garbage = b"not a png at all".to_vec();
        let (out, redacted) = redact_png_best_effort(garbage.clone());
        assert_eq!(out, garbage);
        assert!(!redacted);
    }

    /// A synthetic password band (a wide run of mid-gray rows) is
    /// blurred before the bytes leave the crate. The band is striped so
    /// the blur measurably perturbs every pixel in it.
    #[test]
    fn password_band_fixture_is_blurred() {
        let mut img = image::RgbaImage::from_pixel(64, 32, Rgba([255, 255, 255, 255]));
        for y in 10..22 {
            for x in 0..64 {
                let v = if x % 2 == 0 { 96 } else { 200 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (out, redacted) = redact_png_best_effort(png);
        assert!(redacted, "the synthetic band must be detected");
        let blurred = image::load_from_memory_with_format(&out, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        // Blur mixes the 96/200 stripes together.
        let px = blurred.get_pixel(32, 16);
        assert!(
            px[0] > 96 && px[0] < 200,
            "band pixel must be perturbed by the blur, got {px:?}"
        );
    }

    #[test]
    fn plain_screenshot_is_untouched() {
        // White (avg 255) sits outside the heuristic's 16..=240 band, so
        // a plain screenshot passes through byte-identical.
        let img = image::RgbaImage::from_pixel(64, 32, Rgba([255, 255, 255, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (out, redacted) = redact_png_best_effort(png.clone());
        assert!(!redacted);
        assert_eq!(out, png);
    }
}

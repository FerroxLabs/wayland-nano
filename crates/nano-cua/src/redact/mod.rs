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

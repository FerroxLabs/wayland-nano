use image::{Rgba, RgbaImage};
pub fn detect_password_field_runs(img: &RgbaImage) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    if w < 40 || h < 12 {
        return vec![];
    }
    let mut runs = Vec::new();
    let mut start = None;
    for y in 0..h {
        if row(img, y) {
            start.get_or_insert(y);
        } else if let Some(s) = start.take() {
            if y - s >= 8 {
                runs.push((0, s, w - 1, y - 1));
            }
        }
    }
    if let Some(s) = start {
        if h - s >= 8 {
            runs.push((0, s, w - 1, h - 1));
        }
    }
    runs
}
fn row(img: &RgbaImage, y: u32) -> bool {
    let (w, _) = img.dimensions();
    let mut hist = [0u32; 256];
    for x in 0..w {
        let Rgba([r, g, b, _]) = *img.get_pixel(x, y);
        hist[((u32::from(r) + u32::from(g) + u32::from(b)) / 3) as usize] += 1
    }
    hist[16..=240].iter().copied().max().unwrap_or(0) * 100 / w >= 30
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_password_band_is_found() {
        let mut i = RgbaImage::from_pixel(64, 32, Rgba([255, 255, 255, 255]));
        for y in 10..22 {
            for x in (0..64).step_by(3) {
                i.put_pixel(x, y, Rgba([96, 96, 96, 255]));
            }
        }
        assert!(!detect_password_field_runs(&i).is_empty());
    }
}

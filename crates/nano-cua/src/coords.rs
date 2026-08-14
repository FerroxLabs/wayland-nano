//! ONE coordinate authority (design Q6): all tool coordinates are
//! physical pixels of the primary display — the same space screenshots
//! capture. Out-of-range is a typed error, never a clamp.

use crate::{CuaError, CuaResult};

/// Logical (DPI-scaled) point → physical pixels at `scale`
/// (1.0 = 100%, 1.5 = 150%, ...). Rounds half away from zero.
pub fn logical_to_physical(x: i32, y: i32, scale: f64) -> (i32, i32) {
    (
        (f64::from(x) * scale).round() as i32,
        (f64::from(y) * scale).round() as i32,
    )
}

/// Physical pixel → the 0..=65535 normalized space `SendInput`'s
/// `MOUSEEVENTF_ABSOLUTE` expects. Endpoints are exact: 0 → 0 and
/// `width - 1` → 65535. Anything outside `0..width`/`0..height` is
/// `CoordinateOutOfRange` — clamping would silently click the wrong thing.
pub fn physical_to_normalized(x: i32, y: i32, width: i32, height: i32) -> CuaResult<(i32, i32)> {
    if width <= 0 || height <= 0 || x < 0 || y < 0 || x >= width || y >= height {
        return Err(CuaError::CoordinateOutOfRange);
    }
    Ok((
        ((i64::from(x) * 65_535) / i64::from(width - 1).max(1)) as i32,
        ((i64::from(y) * 65_535) / i64::from(height - 1).max(1)) as i32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-local inverse of `physical_to_normalized`.
    fn normalized_to_physical(nx: i32, ny: i32, width: i32, height: i32) -> (i32, i32) {
        (
            ((i64::from(nx) * i64::from(width - 1).max(1)) / 65_535) as i32,
            ((i64::from(ny) * i64::from(height - 1).max(1)) / 65_535) as i32,
        )
    }

    /// Design §7.1: physical↔normalized roundtrip across scale factors
    /// {1.0, 1.25, 1.5, 2.0} plus out-of-bounds rejection.
    #[test]
    fn mapping_roundtrips_across_scale_factors() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            // A logical point on a 1920x1080 logical display maps onto a
            // physical display of 1920*scale x 1080*scale and roundtrips
            // back to the same physical pixel (±1 for integer division).
            let (px, py) = logical_to_physical(960, 540, scale);
            let (w, h) = logical_to_physical(1920, 1080, scale);
            let (nx, ny) = physical_to_normalized(px, py, w, h).unwrap();
            let (bx, by) = normalized_to_physical(nx, ny, w, h);
            assert!((bx - px).abs() <= 1, "scale {scale}: {bx} vs {px}");
            assert!((by - py).abs() <= 1, "scale {scale}: {by} vs {py}");
            // Center stays center: the logical midpoint normalizes to the
            // midpoint of the 0..=65535 range at every scale (endpoint-
            // exact mapping biases toward the far edge by 65535/(2(w-1))
            // at most).
            assert!((nx - 32_768).abs() <= 32, "scale {scale}: nx={nx}");
            assert!((ny - 32_768).abs() <= 32, "scale {scale}: ny={ny}");
        }
    }

    #[test]
    fn endpoints_are_exact() {
        assert_eq!(physical_to_normalized(0, 0, 1920, 1080).unwrap(), (0, 0));
        assert_eq!(
            physical_to_normalized(1919, 1079, 1920, 1080).unwrap(),
            (65_535, 65_535)
        );
        // Single-pixel displays must not divide by zero.
        assert_eq!(physical_to_normalized(0, 0, 1, 1).unwrap(), (0, 0));
    }

    #[test]
    fn out_of_bounds_rejects_instead_of_clamping() {
        for (x, y) in [(-1, 0), (0, -1), (1920, 0), (0, 1080), (5000, 5000)] {
            assert!(
                matches!(
                    physical_to_normalized(x, y, 1920, 1080),
                    Err(CuaError::CoordinateOutOfRange)
                ),
                "({x}, {y}) must reject"
            );
        }
        for (w, h) in [(0, 1080), (1920, 0), (-1, -1)] {
            assert!(matches!(
                physical_to_normalized(0, 0, w, h),
                Err(CuaError::CoordinateOutOfRange)
            ));
        }
    }

    #[test]
    fn logical_scaling_rounds_predictably() {
        assert_eq!(logical_to_physical(100, 50, 1.25), (125, 63));
        assert_eq!(logical_to_physical(100, 50, 1.5), (150, 75));
        assert_eq!(logical_to_physical(100, 50, 2.0), (200, 100));
    }
}

//! Small text formatters shared by HUDs.

/// `m:ss` below an hour, `h:mm:ss` above. Negative input clamps to zero.
pub fn elapsed(seconds: f64) -> String {
    let total = if seconds.is_finite() { seconds.max(0.0).floor() as u64 } else { 0 };
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The selection size pill: `1280 × 720` (pixel dimensions).
pub fn dimensions(width: u32, height: u32) -> String {
    format!("{width} × {height}")
}

/// Human file size: `512 B`, `1.2 MB`.
pub fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut unit = "B";
    for u in UNITS {
        if v < 1000.0 {
            break;
        }
        v /= 1000.0;
        unit = u;
    }
    format!("{v:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_formats() {
        assert_eq!(elapsed(0.0), "0:00");
        assert_eq!(elapsed(7.9), "0:07");
        assert_eq!(elapsed(62.0), "1:02");
        assert_eq!(elapsed(3723.0), "1:02:03");
        assert_eq!(elapsed(-5.0), "0:00");
        assert_eq!(elapsed(f64::NAN), "0:00");
    }

    #[test]
    fn dims_and_sizes() {
        assert_eq!(dimensions(1280, 720), "1280 × 720");
        assert_eq!(file_size(999), "999 B");
        assert_eq!(file_size(1_234_567), "1.2 MB");
        assert_eq!(file_size(5_000_000_000), "5.0 GB");
    }
}

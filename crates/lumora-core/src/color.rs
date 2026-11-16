//! Colour helpers for the magnifier loupe and annotation styles.

/// An 8-bit sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb8 { r, g, b }
    }

    /// `#RRGGBB`, uppercase.
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Parse `#RRGGBB`, `RRGGBB` or `#RGB`.
    pub fn from_hex(s: &str) -> Option<Rgb8> {
        let s = s.trim().trim_start_matches('#');
        // Byte-indexed slicing below: reject anything that isn't plain hex
        // (non-ASCII would panic on a char boundary; `+` would sneak past
        // `from_str_radix`).
        if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let n = |h: &str| u8::from_str_radix(h, 16).ok();
        match s.len() {
            6 => Some(Rgb8::new(n(&s[0..2])?, n(&s[2..4])?, n(&s[4..6])?)),
            3 => {
                let d = |i: usize| n(&s[i..i + 1]).map(|v| v * 17);
                Some(Rgb8::new(d(0)?, d(1)?, d(2)?))
            }
            _ => None,
        }
    }

    /// Read a pixel from a raw buffer. `bgra` selects byte order (Core
    /// Graphics' native little-endian `kCGImageAlphaPremultipliedFirst` is
    /// BGRA in memory).
    pub fn from_pixel(bytes: &[u8], bgra: bool) -> Option<Rgb8> {
        let p = bytes.get(0..4)?;
        Some(if bgra { Rgb8::new(p[2], p[1], p[0]) } else { Rgb8::new(p[0], p[1], p[2]) })
    }

    /// WCAG relative luminance (0–1).
    pub fn luminance(self) -> f64 {
        fn lin(c: u8) -> f64 {
            let c = f64::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// Whether text drawn over this colour should be dark for contrast.
    pub fn prefers_dark_text(self) -> bool {
        // Contrast vs black beats contrast vs white above L ≈ 0.179.
        self.luminance() > 0.179
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rejects_non_ascii_without_panicking() {
        for bad in ["aé123", "é1", "#ééé12", "+fffff", "+ff", "é"] {
            assert_eq!(Rgb8::from_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hex_round_trip() {
        let c = Rgb8::new(0x0A, 0x84, 0xFF);
        assert_eq!(c.hex(), "#0A84FF");
        assert_eq!(Rgb8::from_hex("#0a84ff"), Some(c));
        assert_eq!(Rgb8::from_hex("fff"), Some(Rgb8::new(255, 255, 255)));
        assert_eq!(Rgb8::from_hex("#12345"), None);
        assert_eq!(Rgb8::from_hex("zzzzzz"), None);
    }

    #[test]
    fn pixels_and_contrast() {
        assert_eq!(Rgb8::from_pixel(&[1, 2, 3, 255], true), Some(Rgb8::new(3, 2, 1)));
        assert_eq!(Rgb8::from_pixel(&[1, 2, 3, 255], false), Some(Rgb8::new(1, 2, 3)));
        assert_eq!(Rgb8::from_pixel(&[1, 2], false), None);
        assert!(Rgb8::new(255, 255, 255).prefers_dark_text());
        assert!(!Rgb8::new(0, 0, 0).prefers_dark_text());
        // Mid grey sits just above the crossover and gets dark text.
        assert!(Rgb8::new(0x77, 0x77, 0x77).prefers_dark_text());
        assert!(!Rgb8::new(0x70, 0x70, 0x70).prefers_dark_text());
    }
}

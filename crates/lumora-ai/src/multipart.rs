//! A tiny `multipart/form-data` body builder (RFC 7578). Pure: no I/O.

use std::time::{SystemTime, UNIX_EPOCH};

/// Builds a `multipart/form-data` body part by part.
#[derive(Debug, Clone)]
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

/// A boundary that is unlikely to appear in the payload: a fixed prefix plus
/// the clock and a per-process counter, hashed into hex. Not cryptographic.
pub fn random_boundary() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let addr = &COUNTER as *const AtomicU64 as u64;
    // splitmix64 over the mixed inputs.
    let mut x = nanos ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ addr.rotate_left(17);
    let mut next = || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    format!("----LumoraBoundary{:016x}{:016x}", next(), next())
}

/// Quote a header parameter value: escape `"` and `\`, drop CR/LF.
fn quote(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\r' | '\n' => {}
            c => out.push(c),
        }
    }
    out
}

impl Multipart {
    /// Start a body with the given boundary (see [`random_boundary`]).
    pub fn new(boundary: impl Into<String>) -> Self {
        Self { boundary: boundary.into(), body: Vec::new() }
    }

    /// The boundary in use.
    pub fn boundary(&self) -> &str {
        &self.boundary
    }

    fn open_part(&mut self, disposition: &str, content_type: Option<&str>) {
        self.body.extend_from_slice(b"--");
        self.body.extend_from_slice(self.boundary.as_bytes());
        self.body.extend_from_slice(b"\r\nContent-Disposition: form-data; ");
        self.body.extend_from_slice(disposition.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        if let Some(ct) = content_type {
            self.body.extend_from_slice(b"Content-Type: ");
            self.body.extend_from_slice(ct.as_bytes());
            self.body.extend_from_slice(b"\r\n");
        }
        self.body.extend_from_slice(b"\r\n");
    }

    /// Add a text field.
    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.open_part(&format!("name=\"{}\"", quote(name)), None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// Add a file field.
    pub fn file(mut self, name: &str, filename: &str, content_type: &str, bytes: &[u8]) -> Self {
        self.open_part(&format!("name=\"{}\"; filename=\"{}\"", quote(name), quote(filename)), Some(content_type));
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// Close the body. Returns the `Content-Type` header value and the bytes.
    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body.extend_from_slice(b"--");
        self.body.extend_from_slice(self.boundary.as_bytes());
        self.body.extend_from_slice(b"--\r\n");
        (format!("multipart/form-data; boundary={}", self.boundary), self.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_layout_with_crlf() {
        let (ct, body) = Multipart::new("XyZ")
            .text("model", "whisper-1")
            .file("file", "a.wav", "audio/wav", b"\x00\x01RIFF")
            .text("timestamp_granularities[]", "segment")
            .finish();
        assert_eq!(ct, "multipart/form-data; boundary=XyZ");
        let mut want = Vec::new();
        want.extend_from_slice(b"--XyZ\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n");
        want.extend_from_slice(
            b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n\x00\x01RIFF\r\n",
        );
        want.extend_from_slice(
            b"--XyZ\r\nContent-Disposition: form-data; name=\"timestamp_granularities[]\"\r\n\r\nsegment\r\n",
        );
        want.extend_from_slice(b"--XyZ--\r\n");
        assert_eq!(body, want);
    }

    #[test]
    fn empty_body_is_just_the_terminator() {
        let (_, body) = Multipart::new("b").finish();
        assert_eq!(body, b"--b--\r\n");
    }

    #[test]
    fn filenames_are_quoted_safely() {
        let (_, body) = Multipart::new("b").file("file", "we\"ird\r\n\\.m4a", "audio/mp4", b"x").finish();
        let s = String::from_utf8(body).unwrap();
        assert!(s.contains("filename=\"we\\\"ird\\\\.m4a\""), "{s}");
        assert_eq!(s.matches("\r\n\r\n").count(), 1);
    }

    #[test]
    fn boundaries_differ_and_are_token_safe() {
        let a = random_boundary();
        let b = random_boundary();
        assert_ne!(a, b);
        assert!(a.len() <= 70);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}

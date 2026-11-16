//! Cloud transcription through an OpenAI-compatible
//! `POST {base}/audio/transcriptions` (OpenAI Whisper, Groq, and others).
//!
//! Uses the same base URL, key handling and HTTP stack as [`crate::client`].
//! Blocking: call it off the main thread.

use lumora_core::ai::{AiError, AiSettings};
use lumora_core::transcript::Segment;
use serde_json::Value;

use crate::client::{agent, io_error, map_error, with_auth};
use crate::multipart::{random_boundary, Multipart};

/// Upload limit of OpenAI's transcription endpoint (25 MB).
pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// Errors from [`transcribe`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TranscribeError {
    /// The file is over [`MAX_UPLOAD_BYTES`]; nothing was sent.
    #[error("file is too large to upload ({bytes} bytes; the limit is 25 MB)")]
    FileTooLarge { bytes: usize },
    /// Configuration, network, HTTP or response error.
    #[error(transparent)]
    Ai(#[from] AiError),
}

/// What to ask the transcription endpoint for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribeRequest {
    /// Transcription model, e.g. `whisper-1` or `whisper-large-v3`.
    pub model: String,
    /// ISO-639-1 language hint; `None` lets the server detect it.
    pub language: Option<String>,
    /// Optional prompt (vocabulary, spelling hints).
    pub prompt: Option<String>,
}

impl Default for TranscribeRequest {
    fn default() -> Self {
        Self { model: "whisper-1".into(), language: None, prompt: None }
    }
}

/// `POST` target for transcription under the configured base URL.
pub fn transcriptions_url(cfg: &AiSettings) -> String {
    format!("{}/audio/transcriptions", cfg.base_url.trim().trim_end_matches('/'))
}

fn content_type_for(filename: &str) -> &'static str {
    let ext = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "mp3" | "mpga" | "mpeg" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "wav" => "audio/wav",
        "webm" => "audio/webm",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

fn non_blank(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Build the multipart body for `req`.
fn build_body(file_bytes: &[u8], filename: &str, req: &TranscribeRequest) -> (String, Vec<u8>) {
    let model = if req.model.trim().is_empty() { "whisper-1" } else { req.model.trim() };
    let mut m = Multipart::new(random_boundary())
        .text("model", model)
        .text("response_format", "verbose_json")
        .text("timestamp_granularities[]", "segment");
    if let Some(lang) = non_blank(&req.language) {
        m = m.text("language", lang);
    }
    if let Some(prompt) = non_blank(&req.prompt) {
        m = m.text("prompt", prompt);
    }
    m.file("file", filename, content_type_for(filename), file_bytes).finish()
}

/// Transcribe `file_bytes` (an audio or video file named `filename`) and
/// return timed segments. Refuses files over [`MAX_UPLOAD_BYTES`] with
/// [`TranscribeError::FileTooLarge`] before touching the network.
pub fn transcribe(
    cfg: &AiSettings,
    key: Option<&str>,
    file_bytes: &[u8],
    filename: &str,
    req: &TranscribeRequest,
) -> Result<Vec<Segment>, TranscribeError> {
    if file_bytes.len() > MAX_UPLOAD_BYTES {
        return Err(TranscribeError::FileTooLarge { bytes: file_bytes.len() });
    }
    cfg.check_ready(key)?;
    let (content_type, body) = build_body(file_bytes, filename, req);
    let resp = with_auth(agent(cfg).post(&transcriptions_url(cfg)), key)
        .set("Content-Type", &content_type)
        .set("Accept", "application/json")
        .send_bytes(&body)
        .map_err(map_error)?;
    let text = resp.into_string().map_err(io_error)?;
    Ok(parse_verbose_json(&text)?)
}

/// Parse a `verbose_json` transcription response.
///
/// Uses `segments[] {start, end, text}` (extra fields ignored); without
/// segments, falls back to one segment of `text` spanning `duration`
/// (0 when absent). An `{"error": {"message": …}}` body becomes
/// [`AiError::BadResponse`] carrying the message.
pub fn parse_verbose_json(body: &str) -> Result<Vec<Segment>, AiError> {
    let v: Value = serde_json::from_str(body).map_err(|e| AiError::BadResponse(format!("invalid JSON: {e}")))?;
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| err.as_str())
            .unwrap_or("transcription failed")
            .trim()
            .to_string();
        return Err(AiError::BadResponse(msg));
    }
    let num = |s: &Value, k: &str| s.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
    if let Some(segs) = v.get("segments").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        let mut out = Vec::with_capacity(segs.len());
        for s in segs {
            let (Some(start), Some(end)) = (num(s, "start"), num(s, "end")) else {
                return Err(AiError::BadResponse("segment without start/end".into()));
            };
            let text = s.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            let start = start.max(0.0);
            out.push(Segment::new(start, end.max(start), text));
        }
        return Ok(out);
    }
    match v.get("text").and_then(Value::as_str) {
        Some(t) if t.trim().is_empty() => Ok(Vec::new()),
        Some(t) => Ok(vec![Segment::new(0.0, num(&v, "duration").unwrap_or(0.0).max(0.0), t.trim())]),
        None => Err(AiError::BadResponse("no segments or text in response".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::server;
    use lumora_core::ai::AiProvider;
    use std::io::Write;

    fn cfg(base: &str) -> AiSettings {
        AiSettings { base_url: base.into(), timeout_secs: 5, ..AiSettings::for_provider(AiProvider::OpenAI) }
    }

    #[test]
    fn parses_openai_style() {
        let body = r#"{"task":"transcribe","language":"english","duration":4.2,"text":"Hello there. General.",
            "segments":[{"id":0,"seek":0,"start":0.0,"end":2.0,"text":" Hello there.","tokens":[1,2],"temperature":0.0,
            "avg_logprob":-0.2,"compression_ratio":1.1,"no_speech_prob":0.01},
            {"id":1,"seek":0,"start":2.0,"end":4.2,"text":" General.","tokens":[3]}]}"#;
        assert_eq!(
            parse_verbose_json(body).unwrap(),
            vec![Segment::new(0.0, 2.0, "Hello there."), Segment::new(2.0, 4.2, "General.")]
        );
    }

    #[test]
    fn parses_groq_style_with_extra_fields() {
        let body = r#"{"task":"transcribe","language":"Chinese","duration":3.5,"text":"你好 世界",
            "segments":[{"id":0,"seek":0,"start":0.12,"end":1.5,"text":"你好","transient":false},
            {"id":1,"start":1.5,"end":1.4,"text":"世界"},{"id":2,"start":1.6,"end":1.7,"text":"  "}],
            "x_groq":{"id":"req_01abc"}}"#;
        assert_eq!(
            parse_verbose_json(body).unwrap(),
            vec![Segment::new(0.12, 1.5, "你好"), Segment::new(1.5, 1.5, "世界")]
        );
    }

    #[test]
    fn text_only_fallback() {
        assert_eq!(
            parse_verbose_json(r#"{"text":" Just text. ","duration":7.25}"#).unwrap(),
            vec![Segment::new(0.0, 7.25, "Just text.")]
        );
        assert_eq!(
            parse_verbose_json(r#"{"text":"No duration"}"#).unwrap(),
            vec![Segment::new(0.0, 0.0, "No duration")]
        );
        assert_eq!(parse_verbose_json(r#"{"text":"Empty list","segments":[],"duration":1}"#).unwrap().len(), 1);
        assert!(parse_verbose_json(r#"{"text":""}"#).unwrap().is_empty());
    }

    #[test]
    fn error_json_carries_message() {
        assert_eq!(
            parse_verbose_json(r#"{"error":{"message":"Invalid file format.","type":"invalid_request_error"}}"#),
            Err(AiError::BadResponse("Invalid file format.".into()))
        );
        assert!(matches!(parse_verbose_json("not json"), Err(AiError::BadResponse(_))));
        assert!(matches!(parse_verbose_json("{}"), Err(AiError::BadResponse(_))));
    }

    #[test]
    fn refuses_oversized_files_before_network() {
        let big = vec![0u8; MAX_UPLOAD_BYTES + 1];
        let c = cfg("http://127.0.0.1:9/v1");
        assert_eq!(
            transcribe(&c, Some("k"), &big, "a.wav", &TranscribeRequest::default()),
            Err(TranscribeError::FileTooLarge { bytes: MAX_UPLOAD_BYTES + 1 })
        );
        assert!(TranscribeError::FileTooLarge { bytes: 1 }.to_string().contains("25 MB"));
    }

    #[test]
    fn missing_key_is_reported() {
        let c = cfg("http://127.0.0.1:9/v1");
        assert_eq!(
            transcribe(&c, None, b"x", "a.wav", &TranscribeRequest::default()),
            Err(TranscribeError::Ai(AiError::MissingKey))
        );
    }

    #[test]
    fn posts_multipart_and_parses_segments() {
        let (base, rx) = server(|s| {
            let body = r#"{"text":"Hi","segments":[{"start":0.0,"end":1.0,"text":" Hi"}]}"#;
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let req = TranscribeRequest { language: Some("en".into()), ..TranscribeRequest::default() };
        let segs = transcribe(&cfg(&base), Some("sk-test"), b"RIFFdata", "clip.wav", &req).unwrap();
        assert_eq!(segs, vec![Segment::new(0.0, 1.0, "Hi")]);
        let raw = rx.recv().unwrap();
        let lower = raw.to_ascii_lowercase();
        assert!(raw.starts_with("POST /v1/audio/transcriptions"), "{raw}");
        assert!(lower.contains("authorization: bearer sk-test"));
        assert!(lower.contains("content-type: multipart/form-data; boundary="));
        for part in [
            "name=\"model\"\r\n\r\nwhisper-1\r\n",
            "name=\"response_format\"\r\n\r\nverbose_json\r\n",
            "name=\"timestamp_granularities[]\"\r\n\r\nsegment\r\n",
            "name=\"language\"\r\n\r\nen\r\n",
            "name=\"file\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFFdata\r\n",
        ] {
            assert!(raw.contains(part), "missing {part:?} in {raw}");
        }
        assert!(!raw.contains("name=\"prompt\""));
    }

    #[test]
    fn http_errors_are_mapped() {
        let (base, _rx) = server(|s| {
            let body = r#"{"error":{"message":"Incorrect API key provided"}}"#;
            write!(s, "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        assert_eq!(
            transcribe(&cfg(&base), Some("bad"), b"x", "a.mp3", &TranscribeRequest::default()),
            Err(TranscribeError::Ai(AiError::BadKey(401)))
        );
    }

    #[test]
    fn content_types_and_url() {
        assert_eq!(content_type_for("A.MP3"), "audio/mpeg");
        assert_eq!(content_type_for("x.m4a"), "audio/mp4");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
        assert_eq!(
            transcriptions_url(&cfg("https://api.groq.com/openai/v1/ ")),
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
    }
}

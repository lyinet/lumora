//! Blocking HTTP for OpenAI-compatible chat completions.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lumora_core::ai::{parse_completion, AiError, AiSettings, ChatRequest, SseEvent, SseParser};

/// Connect timeout; the configurable timeout governs each read.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Error bodies larger than this are not read in full.
const MAX_ERROR_BODY: u64 = 64 * 1024;

fn agent(cfg: &AiSettings) -> ureq::Agent {
    let t = Duration::from_secs(u64::from(cfg.timeout_secs.max(1)));
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(t)
        .timeout_write(t)
        .user_agent(concat!("Lumora/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn with_auth(req: ureq::Request, key: Option<&str>) -> ureq::Request {
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => req.set("Authorization", &format!("Bearer {k}")),
        None => req,
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)
}

fn io_error(e: std::io::Error) -> AiError {
    if is_timeout(&e) {
        AiError::Timeout
    } else {
        AiError::Network(e.to_string())
    }
}

fn map_error(e: ureq::Error) -> AiError {
    match e {
        ureq::Error::Status(status, resp) => {
            let mut body = String::new();
            let _ = resp.into_reader().take(MAX_ERROR_BODY).read_to_string(&mut body);
            AiError::from_status(status, &body)
        }
        ureq::Error::Transport(t) => {
            let timed_out =
                std::error::Error::source(&t).and_then(|s| s.downcast_ref::<std::io::Error>()).is_some_and(is_timeout)
                    || t.to_string().contains("timed out");
            if timed_out {
                AiError::Timeout
            } else {
                AiError::Network(t.message().map(str::to_string).unwrap_or_else(|| t.kind().to_string()))
            }
        }
    }
}

/// Send `request` and return the full generated text.
///
/// With `request.stream`, `on_delta` is called with each piece of text as
/// it arrives (on the calling thread). A non-streaming request, or a server
/// that ignores `stream`, calls it once with the whole text. Setting
/// `cancel` stops reading at the next chunk and returns
/// [`AiError::Cancelled`]; a read already blocked waits at most
/// `cfg.timeout_secs`.
pub fn complete(
    cfg: &AiSettings,
    key: Option<&str>,
    request: &ChatRequest,
    cancel: &AtomicBool,
    mut on_delta: impl FnMut(&str),
) -> Result<String, AiError> {
    cfg.check_ready(key)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }
    let accept = if request.stream { "text/event-stream" } else { "application/json" };
    let req =
        with_auth(agent(cfg).post(&request.url), key).set("Content-Type", "application/json").set("Accept", accept);
    let resp = req.send_string(&request.body_string()).map_err(map_error)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    let is_sse = resp.content_type().eq_ignore_ascii_case("text/event-stream");
    if !request.stream || !is_sse {
        let body = resp.into_string().map_err(io_error)?;
        let text = parse_completion(&body)?;
        on_delta(&text);
        return Ok(text);
    }

    let mut reader = resp.into_reader();
    let mut parser = SseParser::new();
    let mut out = String::new();
    let mut buf = [0u8; 4096];
    let mut handle = |events: Vec<SseEvent>, out: &mut String| -> Result<bool, AiError> {
        for ev in events {
            match ev {
                SseEvent::Delta(d) => {
                    out.push_str(&d);
                    on_delta(&d);
                }
                SseEvent::Done => return Ok(true),
                SseEvent::Error(e) => return Err(e),
            }
        }
        Ok(false)
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AiError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(e)),
        };
        if n == 0 {
            handle(parser.finish(), &mut out)?;
            break;
        }
        if handle(parser.push(&buf[..n]), &mut out)? {
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }
    if out.is_empty() {
        return Err(AiError::BadResponse("empty stream".into()));
    }
    Ok(out)
}

/// Result of a successful [`test_connection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// Whether the configured model appears in `GET /models`; `None` when
    /// the endpoint has no model list (checked with a tiny completion).
    pub model_listed: Option<bool>,
}

/// Check URL, key and model: `GET {base}/models`, falling back to a
/// one-token completion where the list isn't available.
pub fn test_connection(cfg: &AiSettings, key: Option<&str>) -> Result<ConnectionInfo, AiError> {
    cfg.check_ready(key)?;
    let agent = agent(cfg);
    match with_auth(agent.get(&cfg.models_url()), key).call() {
        Ok(resp) => {
            let body = resp.into_string().map_err(io_error)?;
            let listed = lumora_core::ai::model_ids(&body).map(|ids| ids.iter().any(|m| m == &cfg.model));
            Ok(ConnectionInfo { model_listed: listed })
        }
        // No model list here: try a minimal completion instead.
        Err(ureq::Error::Status(404 | 405 | 501, _)) => {
            let body = lumora_core::ai::ping_body(&cfg.model).to_string();
            let r = with_auth(agent.post(&cfg.chat_completions_url()), key)
                .set("Content-Type", "application/json")
                .send_string(&body)
                .map_err(map_error)?;
            let text = r.into_string().map_err(io_error)?;
            parse_completion(&text)?;
            Ok(ConnectionInfo { model_listed: None })
        }
        Err(e) => Err(map_error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumora_core::ai::{build_request, AiContext, AiProvider, WritingTask};
    use lumora_core::i18n::Language;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A one-shot HTTP server. Returns the base URL and a channel yielding
    /// the raw request it received. `respond` writes the response.
    fn server(respond: impl FnOnce(&mut std::net::TcpStream) + Send + 'static) -> (String, mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut head = String::new();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                head.push_str(&line);
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            head.push_str(&String::from_utf8_lossy(&body));
            tx.send(head).unwrap();
            respond(&mut s);
        });
        (format!("http://{addr}/v1"), rx)
    }

    fn cfg(base: &str) -> AiSettings {
        AiSettings { base_url: base.into(), timeout_secs: 5, ..AiSettings::for_provider(AiProvider::OpenAI) }
    }

    fn req(c: &AiSettings, stream: bool) -> ChatRequest {
        let ctx = AiContext { ocr_text: "Hello".into(), ..AiContext::default() };
        build_request(c, &WritingTask::Caption, &ctx, Language::En, stream).unwrap()
    }

    #[test]
    fn streams_deltas_across_writes() {
        let (base, rx) = server(|s| {
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").unwrap();
            for part in [
                "data: {\"choices\":[{\"delta\":{\"content\":\"A ca",
                "\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"ption\"}}]}\n\n",
                "data: [DONE]\n\n",
            ] {
                s.write_all(part.as_bytes()).unwrap();
                s.flush().unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let c = cfg(&base);
        let mut deltas = Vec::new();
        let text =
            complete(&c, Some("sk-test"), &req(&c, true), &AtomicBool::new(false), |d| deltas.push(d.to_string()))
                .unwrap();
        assert_eq!(text, "A caption");
        assert_eq!(deltas, ["A ca", "ption"]);
        let raw = rx.recv().unwrap();
        assert!(raw.starts_with("POST /v1/chat/completions"), "{raw}");
        assert!(raw.to_ascii_lowercase().contains("authorization: bearer sk-test"));
        assert!(raw.contains("\"stream\":true"));
    }

    #[test]
    fn non_streaming_response() {
        let (base, _rx) = server(|s| {
            let body = r#"{"choices":[{"message":{"content":"Done."}}]}"#;
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let c = cfg(&base);
        let mut n = 0;
        assert_eq!(complete(&c, Some("k"), &req(&c, false), &AtomicBool::new(false), |_| n += 1).unwrap(), "Done.");
        assert_eq!(n, 1);
    }

    #[test]
    fn http_errors_are_mapped() {
        let (base, _rx) = server(|s| {
            let body = r#"{"error":{"message":"Incorrect API key provided"}}"#;
            write!(
                s,
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let c = cfg(&base);
        assert_eq!(
            complete(&c, Some("bad"), &req(&c, true), &AtomicBool::new(false), |_| {}),
            Err(AiError::BadKey(401))
        );
    }

    #[test]
    fn missing_key_cancel_and_unreachable() {
        let c = cfg("http://127.0.0.1:9/v1");
        assert_eq!(complete(&c, None, &req(&c, true), &AtomicBool::new(false), |_| {}), Err(AiError::MissingKey));
        assert_eq!(complete(&c, Some("k"), &req(&c, true), &AtomicBool::new(true), |_| {}), Err(AiError::Cancelled));
        // Bind and drop to get a port nobody listens on.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = cfg(&format!("http://127.0.0.1:{port}/v1"));
        assert!(matches!(
            complete(&c, Some("k"), &req(&c, false), &AtomicBool::new(false), |_| {}),
            Err(AiError::Network(_))
        ));
    }

    #[test]
    fn cancel_mid_stream() {
        let (base, _rx) = server(|s| {
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n").unwrap();
            for _ in 0..50 {
                if s.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let c = cfg(&base);
        let cancel = AtomicBool::new(false);
        let r = complete(&c, Some("k"), &req(&c, true), &cancel, |_| cancel.store(true, Ordering::Relaxed));
        assert_eq!(r, Err(AiError::Cancelled));
    }

    #[test]
    fn test_connection_checks_model_list() {
        let (base, rx) = server(|s| {
            let body = r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"gpt-4o-mini"}]}"#;
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let info = test_connection(&cfg(&base), Some("k")).unwrap();
        assert_eq!(info, ConnectionInfo { model_listed: Some(true) });
        assert!(rx.recv().unwrap().starts_with("GET /v1/models"));
    }
}

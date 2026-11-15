//! Parsing chat-completion responses: whole JSON bodies and the
//! server-sent-event stream (`data: {...}` … `data: [DONE]`).

use serde_json::Value;

use super::error::error_message;
use super::AiError;

/// Text of a non-streaming chat-completions response
/// (`choices[0].message.content`).
pub fn parse_completion(body: &str) -> Result<String, AiError> {
    let v: Value = serde_json::from_str(body).map_err(|e| AiError::BadResponse(e.to_string()))?;
    if v.get("error").is_some_and(|e| !e.is_null()) {
        return Err(AiError::Http { status: 200, message: error_message(body) });
    }
    let choice = v.pointer("/choices/0").ok_or_else(|| AiError::BadResponse("no choices".into()))?;
    choice
        .pointer("/message/content")
        .or_else(|| choice.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AiError::BadResponse("no message content".into()))
}

/// Model ids from a `GET /models` response (`data[].id`), or `None` if
/// the body isn't a model list.
pub fn model_ids(body: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(body).ok()?;
    let data = v.get("data")?.as_array()?;
    Some(data.iter().filter_map(|m| m.get("id")?.as_str().map(str::to_string)).collect())
}

/// One parsed stream event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    /// A piece of generated text.
    Delta(String),
    /// `data: [DONE]`: the stream is complete.
    Done,
    /// The server reported an error mid-stream.
    Error(AiError),
}

/// Incremental SSE parser. Feed it bytes as they arrive (chunk boundaries
/// may fall anywhere, even inside a UTF-8 sequence); it returns the events
/// completed so far.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    data: Vec<String>,
    done: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `[DONE]` has been seen; later input is ignored.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Feed bytes; returns completed events.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        self.buf.extend_from_slice(bytes);
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            self.line(line.trim_end_matches('\r'), &mut out);
            if self.done {
                self.buf.clear();
                break;
            }
        }
        out
    }

    /// End of input: flush a final event that lacked its blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.done && !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let line = String::from_utf8_lossy(&rest).into_owned();
            self.line(line.trim_end_matches('\r'), &mut out);
        }
        if !self.done {
            self.dispatch(&mut out);
        }
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
        } else if let Some(v) = line.strip_prefix("data:") {
            let v = v.strip_prefix(' ').unwrap_or(v);
            // Some servers omit the blank line between events; a data line
            // that is a complete event on its own is dispatched right away.
            if self.data.is_empty() && (v == "[DONE]" || serde_json::from_str::<Value>(v).is_ok()) {
                self.data.push(v.to_string());
                self.dispatch(out);
            } else {
                self.data.push(v.to_string());
            }
        }
        // Comments (":keep-alive"), `event:`, `id:` and `retry:` are ignored.
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.data.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        if data.trim() == "[DONE]" {
            self.done = true;
            out.push(SseEvent::Done);
            return;
        }
        match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                if v.get("error").is_some_and(|e| !e.is_null()) {
                    out.push(SseEvent::Error(AiError::Http { status: 200, message: error_message(&data) }));
                    return;
                }
                let piece = v
                    .pointer("/choices/0/delta/content")
                    .or_else(|| v.pointer("/choices/0/message/content"))
                    .and_then(Value::as_str);
                if let Some(p) = piece.filter(|p| !p.is_empty()) {
                    out.push(SseEvent::Delta(p.to_string()));
                }
            }
            Err(e) => out.push(SseEvent::Error(AiError::BadResponse(e.to_string()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(s: &str) -> String {
        format!(
            "data: {{\"id\":\"c1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}}}}]}}\n\n",
            serde_json::to_string(s).unwrap()
        )
    }

    fn text(events: &[SseEvent]) -> String {
        events.iter().filter_map(|e| if let SseEvent::Delta(d) = e { Some(d.as_str()) } else { None }).collect()
    }

    #[test]
    fn parses_whole_completion() {
        let body = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"Hello there"},"finish_reason":"stop"}],"usage":{"total_tokens":5}}"#;
        assert_eq!(parse_completion(body).unwrap(), "Hello there");
        assert!(matches!(parse_completion("<html>"), Err(AiError::BadResponse(_))));
        assert!(matches!(parse_completion(r#"{"choices":[]}"#), Err(AiError::BadResponse(_))));
        assert_eq!(
            parse_completion(r#"{"error":{"message":"model overloaded"}}"#),
            Err(AiError::Http { status: 200, message: "model overloaded".into() })
        );
    }

    #[test]
    fn model_list() {
        assert_eq!(model_ids(r#"{"data":[{"id":"a"},{"id":"b"}]}"#), Some(vec!["a".into(), "b".into()]));
        assert_eq!(model_ids("{}"), None);
        assert_eq!(model_ids("nope"), None);
    }

    #[test]
    fn stream_in_one_piece() {
        let mut p = SseParser::new();
        let input = format!(
            ": keep-alive\n\ndata: {{\"choices\":[{{\"delta\":{{\"role\":\"assistant\"}}}}]}}\n\n{}{}data: [DONE]\n\n",
            chunk("Hello"),
            chunk(", 世界")
        );
        let ev = p.push(input.as_bytes());
        assert_eq!(text(&ev), "Hello, 世界");
        assert_eq!(ev.last(), Some(&SseEvent::Done));
        assert!(p.is_done());
        assert!(p.push(chunk("late").as_bytes()).is_empty(), "nothing after [DONE]");
    }

    #[test]
    fn stream_split_at_every_byte() {
        let input = format!("{}{}\r\n{}data: [DONE]\n\n", chunk("步骤"), chunk("一"), chunk("!"));
        let mut p = SseParser::new();
        let mut ev = Vec::new();
        for b in input.as_bytes() {
            ev.extend(p.push(std::slice::from_ref(b)));
        }
        assert_eq!(text(&ev), "步骤一!");
        assert_eq!(ev.iter().filter(|e| **e == SseEvent::Done).count(), 1);
    }

    #[test]
    fn stream_without_blank_lines_and_eof_flush() {
        let mut p = SseParser::new();
        let mut ev = p.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\ndata: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}");
        assert_eq!(text(&ev), "a");
        ev.extend(p.finish());
        assert_eq!(text(&ev), "ab");
        assert!(!p.is_done(), "EOF without [DONE] is not done");
    }

    #[test]
    fn stream_errors() {
        let mut p = SseParser::new();
        let ev = p.push(b"data: {\"error\":{\"message\":\"quota exceeded\"}}\n\ndata: {not json\n\n");
        assert_eq!(ev[0], SseEvent::Error(AiError::Http { status: 200, message: "quota exceeded".into() }));
        assert!(matches!(ev[1], SseEvent::Error(AiError::BadResponse(_))));
    }
}

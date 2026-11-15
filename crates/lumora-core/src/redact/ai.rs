//! Optional AI second pass for smart redaction.
//!
//! Only the recognised text lines are sent (numbered), never the image.
//! The model answers with the indices of lines that hold personal or
//! secret information; [`parse_response`] is lenient about code fences,
//! surrounding prose and a few shapes of JSON.

use serde_json::{json, Value};

use super::Category;
use crate::ai::{AiError, AiSettings, ChatRequest};

/// At most this many lines (and chars per line) go to the model.
pub const MAX_LINES: usize = 200;
pub const MAX_LINE_CHARS: usize = 200;

const SYSTEM: &str = "You are a privacy filter in Lumora, a screenshot app. You receive numbered lines of text recognised in a screenshot. Find every line that contains personal or secret information: names together with contact details, email addresses, phone numbers, postal addresses, ID or passport numbers, bank or card numbers, passwords, API keys, access tokens, private URLs, license plates or similar. Ignore ordinary UI text, dates, prices and version numbers. Reply with only a JSON array, no prose and no code fences, like [{\"line\": 3, \"category\": \"phone\"}]. Categories: email, phone, id_card, bank_card, secret, ip, url_token, address, name, other. Reply [] if nothing is sensitive.";

/// The numbered text sent to the model.
pub fn numbered_lines(lines: &[&str]) -> String {
    lines
        .iter()
        .take(MAX_LINES)
        .enumerate()
        .map(|(i, l)| {
            let t: String = l.chars().take(MAX_LINE_CHARS).collect();
            format!("{i}: {}", t.replace('\n', " "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A non-streaming chat request asking which `lines` are sensitive.
pub fn build_request(cfg: &AiSettings, lines: &[&str]) -> Result<ChatRequest, AiError> {
    if lines.iter().all(|l| l.trim().is_empty()) {
        return Err(AiError::EmptyInput);
    }
    let body = json!({
        "model": cfg.model,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": format!("Lines:\n<<<\n{}\n>>>", numbered_lines(lines)) },
        ],
        "temperature": 0.0,
        "stream": false,
    });
    Ok(ChatRequest { url: cfg.chat_completions_url(), body, stream: false })
}

fn category_from(s: &str) -> Category {
    match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "email" | "e_mail" => Category::Email,
        "phone" | "phone_number" | "mobile" => Category::Phone,
        "id_card" | "id" | "id_number" | "passport" => Category::IdCard,
        "bank_card" | "card" | "credit_card" | "bank_account" => Category::BankCard,
        "secret" | "password" | "api_key" | "token" => Category::Secret,
        "ip" | "ip_address" => Category::IpAddress,
        "url_token" | "url" => Category::UrlToken,
        _ => Category::Other,
    }
}

fn index_of(v: &Value) -> Option<usize> {
    match v {
        Value::Number(n) => n.as_u64().map(|n| n as usize),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The `(line index, category)` pairs in a model reply, in range and
/// de-duplicated (first category wins). Garbage yields an empty list.
pub fn parse_response(text: &str, line_count: usize) -> Vec<(usize, Category)> {
    let t = text.trim();
    let start = t.find(['[', '{']);
    let end = t.rfind([']', '}']);
    let (Some(s), Some(e)) = (start, end) else { return Vec::new() };
    if e < s {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(&t[s..=e]) else { return Vec::new() };
    let items = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => match o.get("lines").or_else(|| o.get("items")).or_else(|| o.get("results")) {
            Some(Value::Array(a)) => a.clone(),
            _ => vec![v.clone()],
        },
        _ => Vec::new(),
    };
    let mut out: Vec<(usize, Category)> = Vec::new();
    for item in &items {
        let (idx, cat) = match item {
            Value::Object(o) => (
                o.get("line").or_else(|| o.get("index")).and_then(index_of),
                o.get("category").and_then(Value::as_str).map(category_from).unwrap_or(Category::Other),
            ),
            other => (index_of(other), Category::Other),
        };
        if let Some(i) = idx.filter(|&i| i < line_count.min(MAX_LINES)) {
            if !out.iter().any(|(j, _)| *j == i) {
                out.push((i, cat));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_sends_numbered_text_only() {
        let cfg = AiSettings::default();
        let req = build_request(&cfg, &["Hello", "Call 138 0013 8000"]).unwrap();
        assert!(!req.stream);
        let body = req.body_string();
        assert!(body.contains("0: Hello\\n1: Call 138 0013 8000"));
        assert!(!body.contains("image_url"));
        assert!(matches!(build_request(&cfg, &["  ", ""]), Err(AiError::EmptyInput)));
        assert_eq!(numbered_lines(&["a\nb"]), "0: a b");
    }

    #[test]
    fn parses_lenient_replies() {
        assert_eq!(
            parse_response("```json\n[{\"line\": 1, \"category\": \"phone\"}, {\"line\": \"3\", \"category\": \"Home address\"}]\n```", 5),
            vec![(1, Category::Phone), (3, Category::Other)]
        );
        assert_eq!(parse_response("Sensitive: [0, 2, 2, 9]", 4), vec![(0, Category::Other), (2, Category::Other)]);
        assert_eq!(
            parse_response("{\"lines\": [{\"index\": 1, \"category\": \"api key\"}]}", 2),
            vec![(1, Category::Secret)]
        );
        assert!(parse_response("[]", 3).is_empty());
        assert!(parse_response("nothing here", 3).is_empty());
        assert!(parse_response("] oops [", 3).is_empty());
    }
}

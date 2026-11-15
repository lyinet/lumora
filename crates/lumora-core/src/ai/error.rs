//! Errors from AI requests, mapped to messages a user can act on.

use thiserror::Error;

use crate::i18n::{tr, trf, Msg};

/// Everything that can go wrong with an AI request.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AiError {
    #[error("no API key")]
    MissingKey,
    #[error("base URL or model is not set")]
    BadConfig,
    #[error("nothing to send")]
    EmptyInput,
    #[error("API key rejected (HTTP {0})")]
    BadKey(u16),
    #[error("rate limited or out of quota")]
    RateLimited,
    #[error("model or endpoint not found")]
    NotFound,
    #[error("server error (HTTP {0})")]
    Server(u16),
    /// Any other non-success status, with the provider's message.
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    BadResponse(String),
    #[error("cancelled")]
    Cancelled,
    #[error("keychain error: {0}")]
    Keychain(String),
}

/// Pull `error.message` (OpenAI and compatibles) or a top-level `message`
/// out of an error body; fall back to a trimmed snippet of the raw body.
pub(crate) fn error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let m = v
            .pointer("/error/message")
            .or_else(|| v.get("message"))
            .or_else(|| v.get("error").filter(|e| e.is_string()))
            .and_then(|m| m.as_str());
        if let Some(m) = m {
            return m.trim().to_string();
        }
    }
    let body = body.trim();
    let snippet: String = body.chars().take(200).collect();
    if snippet.len() < body.len() {
        format!("{snippet}…")
    } else {
        snippet
    }
}

impl AiError {
    /// Map a non-2xx HTTP status (and its body) to an error.
    pub fn from_status(status: u16, body: &str) -> AiError {
        match status {
            401 | 403 => AiError::BadKey(status),
            402 | 429 => AiError::RateLimited,
            404 => AiError::NotFound,
            408 | 504 => AiError::Timeout,
            500..=599 => AiError::Server(status),
            _ => AiError::Http { status, message: error_message(body) },
        }
    }

    /// Localised, user-facing description.
    pub fn user_message(&self) -> String {
        match self {
            AiError::MissingKey => tr(Msg::AiErrMissingKey).to_string(),
            AiError::BadConfig => tr(Msg::AiErrBadConfig).to_string(),
            AiError::EmptyInput => tr(Msg::AiErrEmptyInput).to_string(),
            AiError::BadKey(_) => tr(Msg::AiErrBadKey).to_string(),
            AiError::RateLimited => tr(Msg::AiErrRateLimited).to_string(),
            AiError::NotFound => tr(Msg::AiErrNotFound).to_string(),
            AiError::Server(s) => trf(Msg::AiErrServer, &[s]),
            AiError::Http { status, message } => trf(Msg::AiErrHttp, &[status, message]),
            AiError::Timeout => tr(Msg::AiErrTimeout).to_string(),
            AiError::Network(e) => trf(Msg::AiErrNetwork, &[e]),
            AiError::BadResponse(e) => trf(Msg::AiErrBadResponse, &[e]),
            AiError::Cancelled => tr(Msg::AiErrCancelled).to_string(),
            AiError::Keychain(e) => trf(Msg::AiErrKeychain, &[e]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(AiError::from_status(401, ""), AiError::BadKey(401));
        assert_eq!(AiError::from_status(403, ""), AiError::BadKey(403));
        assert_eq!(AiError::from_status(429, "{}"), AiError::RateLimited);
        assert_eq!(AiError::from_status(404, ""), AiError::NotFound);
        assert_eq!(AiError::from_status(504, ""), AiError::Timeout);
        assert_eq!(AiError::from_status(503, "down"), AiError::Server(503));
        assert_eq!(
            AiError::from_status(400, r#"{"error":{"message":"Invalid model 'x'","type":"invalid_request_error"}}"#),
            AiError::Http { status: 400, message: "Invalid model 'x'".into() }
        );
        assert_eq!(
            AiError::from_status(422, r#"{"error":"bad"}"#),
            AiError::Http { status: 422, message: "bad".into() }
        );
        let long = "x".repeat(500);
        match AiError::from_status(418, &long) {
            AiError::Http { message, .. } => assert!(message.chars().count() == 201 && message.ends_with('…')),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn user_messages_are_localised() {
        use crate::i18n::Language;
        assert!(Msg::AiErrBadKey.text(Language::En).contains("API key"));
        assert!(AiError::Server(502).user_message().contains("502"));
        assert!(AiError::Http { status: 400, message: "nope".into() }.user_message().contains("nope"));
    }
}

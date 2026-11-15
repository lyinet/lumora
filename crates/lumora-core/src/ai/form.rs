//! Pure helpers behind the Settings window's AI tab: field parsing and
//! clamping, readouts, the vision hint, and a key wrapper that never prints.

use super::{AiProvider, TEMPERATURE_RANGE, TIMEOUT_RANGE};
use crate::i18n::{tr, Msg};

/// Parse the timeout field: whole seconds, surrounding whitespace and a
/// trailing `s` allowed, clamped to [`TIMEOUT_RANGE`]. `None` when the text
/// is not a number (the field then reverts to the current value).
pub fn parse_timeout(text: &str) -> Option<u32> {
    let t = text.trim().trim_end_matches(['s', 'S', '秒']).trim();
    if t.is_empty() {
        return None;
    }
    let secs: f64 = t.parse().ok().filter(|v: &f64| v.is_finite())?;
    let secs = secs.round().clamp(TIMEOUT_RANGE.0 as f64, TIMEOUT_RANGE.1 as f64);
    Some(secs as u32)
}

/// Snap a slider value to the temperature grid (0.1 steps within
/// [`TEMPERATURE_RANGE`]). Non-finite input becomes the default 0.3.
pub fn snap_temperature(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.3;
    }
    let v = value.clamp(TEMPERATURE_RANGE.0, TEMPERATURE_RANGE.1);
    (v * 10.0).round() / 10.0
}

/// Slider readout, one decimal: `0.3`, `1.0`, `2.0`.
pub fn format_temperature(value: f64) -> String {
    format!("{:.1}", snap_temperature(value))
}

/// Whether `model` likely accepts images. The preset's own default model
/// uses [`AiProvider::default_model_has_vision`]; other names are matched
/// against common vision-model naming. A hint only — the user decides.
pub fn model_likely_has_vision(provider: AiProvider, model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() || m == provider.default_model() {
        return provider.default_model_has_vision();
    }
    // "vl" covers qwen-vl, Qwen2.5-VL, glm-4v-style "-vl" names.
    const MARKERS: [&str; 10] =
        ["gpt-4o", "gpt-4.1", "gpt-4-turbo", "gpt-5", "vision", "vl", "llava", "claude-3", "gemini", "pixtral"];
    MARKERS.iter().any(|k| m.contains(k)) || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

/// Status line for a successful connection test.
pub fn connection_message(model_listed: Option<bool>) -> &'static str {
    match model_listed {
        Some(false) => tr(Msg::AiTestModelMissing),
        Some(true) | None => tr(Msg::AiTestOk),
    }
}

/// The note under the API key field: `None` when a key is required.
pub fn key_note(provider: AiProvider) -> Option<&'static str> {
    match provider {
        AiProvider::Ollama => Some(tr(Msg::AiNoKeyNeeded)),
        p if !p.requires_key() => Some(tr(Msg::AiKeyOptional)),
        _ => None,
    }
}

/// An API key in transit (UI → app). `Debug` is redacted so the key never
/// reaches a log through `{:?}`; read it with [`Secret::expose`].
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap `key`, trimmed.
    pub fn new(key: &str) -> Self {
        Secret(key.trim().to_string())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `Some(key)` unless empty — the shape `check_ready`/`test_connection` take.
    pub fn as_option(&self) -> Option<&str> {
        Some(self.0.as_str()).filter(|k| !k.is_empty())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "Secret(empty)" } else { "Secret(***)" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_parsing_clamps_and_rejects_garbage() {
        assert_eq!(parse_timeout("60"), Some(60));
        assert_eq!(parse_timeout(" 90 s "), Some(90));
        assert_eq!(parse_timeout("30秒"), Some(30));
        assert_eq!(parse_timeout("1"), Some(5));
        assert_eq!(parse_timeout("0"), Some(5));
        assert_eq!(parse_timeout("-3"), Some(5));
        assert_eq!(parse_timeout("100000"), Some(600));
        assert_eq!(parse_timeout("12.6"), Some(13));
        assert_eq!(parse_timeout(""), None);
        assert_eq!(parse_timeout("abc"), None);
        assert_eq!(parse_timeout("NaN"), None);
        assert_eq!(parse_timeout("inf"), None);
    }

    #[test]
    fn temperature_snaps_to_tenths_within_range() {
        assert_eq!(snap_temperature(0.34), 0.3);
        assert_eq!(snap_temperature(0.35), 0.4);
        assert_eq!(snap_temperature(-1.0), 0.0);
        assert_eq!(snap_temperature(7.0), 2.0);
        assert_eq!(snap_temperature(f64::NAN), 0.3);
        assert_eq!(format_temperature(1.0), "1.0");
        assert_eq!(format_temperature(0.2999), "0.3");
        assert_eq!(format_temperature(2.4), "2.0");
    }

    #[test]
    fn vision_hint() {
        assert!(model_likely_has_vision(AiProvider::OpenAI, ""));
        assert!(model_likely_has_vision(AiProvider::OpenAI, "gpt-4o-mini"));
        assert!(!model_likely_has_vision(AiProvider::OpenAI, "gpt-3.5-turbo"));
        assert!(!model_likely_has_vision(AiProvider::DeepSeek, "deepseek-chat"));
        assert!(model_likely_has_vision(AiProvider::Qwen, "qwen-vl-plus"));
        assert!(model_likely_has_vision(AiProvider::Ollama, "llava:13b"));
        assert!(model_likely_has_vision(AiProvider::Moonshot, "moonshot-v1-8k-vision-preview"));
        assert!(!model_likely_has_vision(AiProvider::Ollama, "llama3.2"));
        assert!(model_likely_has_vision(AiProvider::Custom, "Qwen2.5-VL-7B"));
    }

    #[test]
    fn messages_and_notes() {
        assert_eq!(connection_message(Some(true)), connection_message(None));
        assert_ne!(connection_message(Some(false)), connection_message(Some(true)));
        assert!(key_note(AiProvider::OpenAI).is_none());
        assert!(key_note(AiProvider::Ollama).is_some());
        assert!(key_note(AiProvider::Custom).is_some());
        assert_ne!(key_note(AiProvider::Ollama), key_note(AiProvider::Custom));
    }

    #[test]
    fn secret_never_debug_prints() {
        let s = Secret::new("  sk-very-secret  ");
        assert_eq!(s.expose(), "sk-very-secret");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert!(!format!("{:?}", Some(s.clone())).contains("sk-"));
        assert_eq!(s.as_option(), Some("sk-very-secret"));
        assert_eq!(Secret::new("   ").as_option(), None);
        assert_eq!(format!("{:?}", Secret::default()), "Secret(empty)");
    }
}

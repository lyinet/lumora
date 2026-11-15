//! AI writing: settings, prompt building and response parsing for
//! OpenAI-compatible chat-completion APIs.
//!
//! Everything here is pure: no networking and no Keychain. The blocking HTTP
//! client lives in the `lumora-ai` crate, which also implements
//! [`KeyStore`] on top of the macOS Keychain. The API key is never part of
//! [`AiSettings`] and so never reaches `settings.toml`.

mod base64;
mod error;
mod prompt;
mod stream;

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::i18n::{tr, Language, Msg};

pub use base64::{encode as base64_encode, png_data_url};
pub use error::AiError;
pub use prompt::{build_request, ping_body, AiContext, ChatRequest, StepHint, WritingTask, MAX_INPUT_CHARS};
pub use stream::{model_ids, parse_completion, SseEvent, SseParser};

/// Keychain generic-password service under which API keys are stored; the
/// account is [`AiProvider::id`].
pub const KEYCHAIN_SERVICE: &str = "app.lumora.ai";

/// Supported range for [`AiSettings::temperature`].
pub const TEMPERATURE_RANGE: (f64, f64) = (0.0, 2.0);
/// Supported range for [`AiSettings::timeout_secs`].
pub const TIMEOUT_RANGE: (u32, u32) = (5, 600);

/// A provider preset. Each fills in a base URL and default model; `Custom`
/// is any other OpenAI-compatible endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    #[default]
    OpenAI,
    DeepSeek,
    /// Alibaba Cloud DashScope, OpenAI-compatible mode.
    Qwen,
    /// Moonshot AI (Kimi).
    Moonshot,
    /// Local Ollama server; needs no API key.
    Ollama,
    Custom,
}

impl AiProvider {
    pub const ALL: [AiProvider; 6] = [
        AiProvider::OpenAI,
        AiProvider::DeepSeek,
        AiProvider::Qwen,
        AiProvider::Moonshot,
        AiProvider::Ollama,
        AiProvider::Custom,
    ];

    /// Stable id: the serde name and the Keychain account.
    pub fn id(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "openai",
            AiProvider::DeepSeek => "deepseek",
            AiProvider::Qwen => "qwen",
            AiProvider::Moonshot => "moonshot",
            AiProvider::Ollama => "ollama",
            AiProvider::Custom => "custom",
        }
    }

    /// Base URL (up to and including `/v1` or equivalent). Empty for `Custom`.
    pub fn default_base_url(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "https://api.openai.com/v1",
            AiProvider::DeepSeek => "https://api.deepseek.com/v1",
            AiProvider::Qwen => "https://dashscope.aliyuncs.com/compatible-mode/v1",
            AiProvider::Moonshot => "https://api.moonshot.cn/v1",
            AiProvider::Ollama => "http://localhost:11434/v1",
            AiProvider::Custom => "",
        }
    }

    /// A sensible, inexpensive default model. Empty for `Custom`.
    pub fn default_model(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "gpt-4o-mini",
            AiProvider::DeepSeek => "deepseek-chat",
            AiProvider::Qwen => "qwen-plus",
            AiProvider::Moonshot => "moonshot-v1-8k",
            AiProvider::Ollama => "llama3.2",
            AiProvider::Custom => "",
        }
    }

    /// Whether requests need an API key. Ollama runs locally without one;
    /// a custom endpoint may or may not, so a missing key is allowed there.
    pub fn requires_key(self) -> bool {
        !matches!(self, AiProvider::Ollama | AiProvider::Custom)
    }

    /// Whether the preset's default model accepts images. A hint for the
    /// Settings UI only; the user decides via `send_image`.
    pub fn default_model_has_vision(self) -> bool {
        matches!(self, AiProvider::OpenAI)
    }

    /// Display name. Brand names that read the same in every language are
    /// not localised.
    pub fn label(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "OpenAI",
            AiProvider::DeepSeek => "DeepSeek",
            AiProvider::Qwen => tr(Msg::AiProviderQwen),
            AiProvider::Moonshot => tr(Msg::AiProviderMoonshot),
            AiProvider::Ollama => tr(Msg::AiProviderOllama),
            AiProvider::Custom => tr(Msg::AiProviderCustom),
        }
    }
}

/// Which language AI output is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum OutputLanguage {
    /// Match the screenshot's text; fall back to the UI language.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-Hans")]
    SimplifiedChinese,
}

impl OutputLanguage {
    pub const ALL: [OutputLanguage; 3] =
        [OutputLanguage::Auto, OutputLanguage::English, OutputLanguage::SimplifiedChinese];

    /// The forced language, if any.
    pub fn fixed(self) -> Option<Language> {
        match self {
            OutputLanguage::Auto => None,
            OutputLanguage::English => Some(Language::En),
            OutputLanguage::SimplifiedChinese => Some(Language::ZhHans),
        }
    }

    /// Popup label; language names in their own language.
    pub fn label(self) -> &'static str {
        match self {
            OutputLanguage::Auto => tr(Msg::AiOutputAuto),
            OutputLanguage::English => "English",
            OutputLanguage::SimplifiedChinese => "简体中文",
        }
    }
}

/// `[ai]` section of `settings.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Master switch; off by default so nothing leaves the Mac unasked.
    pub enabled: bool,
    pub provider: AiProvider,
    /// OpenAI-compatible base URL, without a trailing slash. Missing or
    /// blank means the preset's (filled in by [`AiSettings::validate`]).
    #[serde(default)]
    pub base_url: String,
    /// Model name; missing or blank means the preset's default.
    #[serde(default)]
    pub model: String,
    /// Sampling temperature, clamped to 0…2.
    pub temperature: f64,
    pub output_language: OutputLanguage,
    /// Also send the screenshot itself (vision models only). Off by
    /// default: only recognised text and annotation text are sent.
    pub send_image: bool,
    /// Connect/read timeout in seconds, clamped to 5…600.
    pub timeout_secs: u32,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings::for_provider(AiProvider::default())
    }
}

impl AiSettings {
    /// Defaults with `provider`'s preset URL and model.
    pub fn for_provider(provider: AiProvider) -> Self {
        AiSettings {
            enabled: false,
            provider,
            base_url: provider.default_base_url().to_string(),
            model: provider.default_model().to_string(),
            temperature: 0.3,
            output_language: OutputLanguage::Auto,
            send_image: false,
            timeout_secs: 60,
        }
    }

    /// Switch preset. The base URL and model follow the new preset unless
    /// the user had overridden them (i.e. they differ from the old preset's
    /// defaults and are not empty).
    pub fn set_provider(&mut self, provider: AiProvider) {
        let old = self.provider;
        if self.base_url.trim().is_empty() || self.base_url == old.default_base_url() {
            self.base_url = provider.default_base_url().to_string();
        }
        if self.model.trim().is_empty() || self.model == old.default_model() {
            self.model = provider.default_model().to_string();
        }
        self.provider = provider;
    }

    /// Clamp values and fill blanks from the preset.
    pub fn validate(&mut self) {
        if !self.temperature.is_finite() {
            self.temperature = 0.3;
        }
        self.temperature = self.temperature.clamp(TEMPERATURE_RANGE.0, TEMPERATURE_RANGE.1);
        self.timeout_secs = self.timeout_secs.clamp(TIMEOUT_RANGE.0, TIMEOUT_RANGE.1);
        let url = self.base_url.trim().trim_end_matches('/');
        self.base_url = if url.is_empty() { self.provider.default_base_url().to_string() } else { url.to_string() };
        let model = self.model.trim();
        self.model = if model.is_empty() { self.provider.default_model().to_string() } else { model.to_string() };
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url.trim().trim_end_matches('/'))
    }

    /// `POST` target for chat completions.
    pub fn chat_completions_url(&self) -> String {
        self.endpoint("chat/completions")
    }

    /// `GET` target used by "Test connection".
    pub fn models_url(&self) -> String {
        self.endpoint("models")
    }

    /// Whether the settings are complete enough to send a request with
    /// `key`. Returns the error to show otherwise.
    pub fn check_ready(&self, key: Option<&str>) -> Result<(), AiError> {
        if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
            return Err(AiError::BadConfig);
        }
        if self.model.trim().is_empty() {
            return Err(AiError::BadConfig);
        }
        let has_key = key.is_some_and(|k| !k.trim().is_empty());
        if self.provider.requires_key() && !has_key {
            return Err(AiError::MissingKey);
        }
        Ok(())
    }
}

/// Where API keys live. The app uses the macOS Keychain (`lumora-ai`);
/// tests use [`MemoryKeyStore`].
pub trait KeyStore {
    /// The key for `provider`, or `None` if none is stored.
    fn get(&self, provider: AiProvider) -> Result<Option<String>, AiError>;
    /// Store (or replace) the key. An empty key deletes it.
    fn set(&self, provider: AiProvider, key: &str) -> Result<(), AiError>;
    /// Remove the key; succeeds if there was none.
    fn delete(&self, provider: AiProvider) -> Result<(), AiError>;
}

/// In-memory [`KeyStore`] for tests and previews.
#[derive(Debug, Default)]
pub struct MemoryKeyStore(Mutex<HashMap<AiProvider, String>>);

impl KeyStore for MemoryKeyStore {
    fn get(&self, provider: AiProvider) -> Result<Option<String>, AiError> {
        Ok(self.0.lock().map_err(|_| AiError::Keychain("poisoned".into()))?.get(&provider).cloned())
    }

    fn set(&self, provider: AiProvider, key: &str) -> Result<(), AiError> {
        let key = key.trim();
        if key.is_empty() {
            return self.delete(provider);
        }
        self.0.lock().map_err(|_| AiError::Keychain("poisoned".into()))?.insert(provider, key.to_string());
        Ok(())
    }

    fn delete(&self, provider: AiProvider) -> Result<(), AiError> {
        self.0.lock().map_err(|_| AiError::Keychain("poisoned".into()))?.remove(&provider);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_urls_and_models() {
        for p in AiProvider::ALL {
            let s = AiSettings::for_provider(p);
            if p == AiProvider::Custom {
                assert!(s.base_url.is_empty() && s.model.is_empty());
                continue;
            }
            assert!(s.base_url.starts_with("http"), "{p:?}");
            assert!(!s.base_url.ends_with('/'), "{p:?}");
            assert!(!s.model.is_empty(), "{p:?}");
            assert!(!p.label().is_empty());
        }
        let q = AiSettings::for_provider(AiProvider::Qwen);
        assert_eq!(q.chat_completions_url(), "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions");
        let o = AiSettings::for_provider(AiProvider::Ollama);
        assert_eq!(o.models_url(), "http://localhost:11434/v1/models");
        assert_eq!(o.model, "llama3.2");
        assert_eq!(AiSettings::for_provider(AiProvider::DeepSeek).model, "deepseek-chat");
        assert_eq!(AiSettings::for_provider(AiProvider::Moonshot).model, "moonshot-v1-8k");
    }

    #[test]
    fn switching_provider_keeps_overrides() {
        let mut s = AiSettings::default();
        s.set_provider(AiProvider::DeepSeek);
        assert_eq!(s.base_url, "https://api.deepseek.com/v1");
        assert_eq!(s.model, "deepseek-chat");
        s.model = "deepseek-reasoner".into();
        s.set_provider(AiProvider::Moonshot);
        assert_eq!(s.base_url, "https://api.moonshot.cn/v1", "untouched URL follows preset");
        assert_eq!(s.model, "deepseek-reasoner", "overridden model is kept");
    }

    #[test]
    fn validate_clamps_and_fills() {
        let mut s = AiSettings {
            temperature: 9.0,
            timeout_secs: 0,
            base_url: " https://x.example/v1/ ".into(),
            model: "  ".into(),
            ..AiSettings::default()
        };
        s.validate();
        assert_eq!(s.temperature, 2.0);
        assert_eq!(s.timeout_secs, 5);
        assert_eq!(s.base_url, "https://x.example/v1");
        assert_eq!(s.model, "gpt-4o-mini");
        s.temperature = f64::NAN;
        s.validate();
        assert_eq!(s.temperature, 0.3);
    }

    #[test]
    fn readiness_requires_key_except_local() {
        let s = AiSettings::default();
        assert_eq!(s.check_ready(None), Err(AiError::MissingKey));
        assert_eq!(s.check_ready(Some("  ")), Err(AiError::MissingKey));
        assert_eq!(s.check_ready(Some("sk-1")), Ok(()));
        assert_eq!(AiSettings::for_provider(AiProvider::Ollama).check_ready(None), Ok(()));
        assert_eq!(AiSettings::for_provider(AiProvider::Custom).check_ready(None), Err(AiError::BadConfig));
    }

    #[test]
    fn memory_key_store() {
        let ks = MemoryKeyStore::default();
        assert_eq!(ks.get(AiProvider::OpenAI).unwrap(), None);
        ks.set(AiProvider::OpenAI, " sk-abc ").unwrap();
        assert_eq!(ks.get(AiProvider::OpenAI).unwrap().as_deref(), Some("sk-abc"));
        assert_eq!(ks.get(AiProvider::DeepSeek).unwrap(), None);
        ks.set(AiProvider::OpenAI, "").unwrap();
        assert_eq!(ks.get(AiProvider::OpenAI).unwrap(), None);
        ks.delete(AiProvider::Qwen).unwrap();
        assert_eq!(KEYCHAIN_SERVICE, "app.lumora.ai");
        assert_eq!(AiProvider::Moonshot.id(), "moonshot");
    }
}

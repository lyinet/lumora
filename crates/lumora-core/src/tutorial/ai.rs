//! AI write-up for tutorial steps.
//!
//! Only text leaves the Mac: per step the click kind, app, window title and
//! the OCR text next to the click. No images. The model answers with a JSON
//! array of `{title, detail}`; [`parse_steps`] is tolerant of code fences,
//! prose around the array, a `{"steps": […]}` wrapper and alternative key
//! names, and [`apply`] keeps local titles for anything missing.

use serde_json::{json, Value};

use super::{Step, Tutorial};
use crate::ai::{AiError, AiSettings, ChatRequest};
use crate::click_log::MouseButton;
use crate::i18n::Language;

/// Longest field passed per step (characters).
const MAX_FIELD: usize = 120;

fn clip(s: &str) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars().take(MAX_FIELD).collect()
}

fn language_name(l: Language) -> &'static str {
    match l {
        Language::En => "English",
        Language::ZhHans => "Simplified Chinese (简体中文)",
    }
}

/// The language steps are written in: the forced output language, else
/// the UI language.
pub fn output_language(cfg: &AiSettings, ui: Language) -> Language {
    cfg.output_language.fixed().unwrap_or(ui)
}

fn action(step: &Step) -> &'static str {
    match (step.button, step.double_click) {
        (MouseButton::Right, _) => "right-click",
        (_, true) => "double-click",
        (MouseButton::Other, _) => "middle-click",
        _ => "click",
    }
}

/// The user message listing the steps.
pub fn steps_listing(t: &Tutorial) -> String {
    let mut out = String::new();
    for s in &t.steps {
        out.push_str(&format!("Step {}: {} at {:.1}s", s.index, action(s), s.t));
        if let Some(a) = s.app.as_deref() {
            out.push_str(&format!("; app: {}", clip(a)));
        }
        if let Some(w) = s.window_title.as_deref() {
            out.push_str(&format!("; window: \"{}\"", clip(w)));
        }
        match s.ocr_hint.as_deref() {
            Some(h) => out.push_str(&format!("; text at the pointer: \"{}\"", clip(h))),
            None => out.push_str("; text at the pointer: (none)"),
        }
        out.push('\n');
    }
    out
}

const SYSTEM: &str = "You are the writing assistant in Lumora, a screen-recording app. You turn a list of recorded clicks into a clear step-by-step tutorial. Reply with JSON only: no preamble, no explanations, no code fences.";

/// Build the chat request (non-streaming) for `t`'s steps.
pub fn build_request(cfg: &AiSettings, t: &Tutorial, ui: Language) -> Result<ChatRequest, AiError> {
    if t.steps.is_empty() {
        return Err(AiError::EmptyInput);
    }
    let lang = language_name(output_language(cfg, ui));
    let user = format!(
        "A user recorded their screen while doing a task. Below is each click in order with the app, the window title and the text recognised right next to the pointer (OCR, may contain errors or be unrelated). You cannot see the screen.\n\n\
For every step write a short imperative title (at most 8 words, e.g. Click \"Save\") naming the control when the text makes it clear, and a one-sentence detail that adds useful context (where it is or what it does). Do not invent controls that the text does not support; if unsure, describe the location (app and window). Write in {lang}; keep UI labels exactly as recognised.\n\n\
Answer with a JSON array of exactly {n} objects in step order, each {{\"title\": \"…\", \"detail\": \"…\"}}.\n\n<<<\n{listing}>>>",
        n = t.steps.len(),
        listing = steps_listing(t),
    );
    let body = json!({
        "model": cfg.model,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": user },
        ],
        "temperature": cfg.temperature,
        "stream": false,
    });
    Ok(ChatRequest { url: cfg.chat_completions_url(), body, stream: false })
}

/// One parsed step text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepText {
    /// 1-based step number when the model gave one.
    pub index: Option<usize>,
    pub title: String,
    pub detail: String,
}

fn str_field(v: &Value, keys: &[&str]) -> String {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn from_value(v: Value) -> Option<Vec<StepText>> {
    let arr = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => match ["steps", "tutorial", "items", "result"].iter().find_map(|k| o.remove(*k)) {
            Some(Value::Array(a)) => a,
            _ => return None,
        },
        _ => return None,
    };
    let out: Vec<StepText> = arr
        .iter()
        .map(|item| match item {
            Value::String(s) => StepText { index: None, title: s.trim().to_string(), detail: String::new() },
            _ => StepText {
                index: ["index", "step", "number", "n"]
                    .iter()
                    .find_map(|k| item.get(*k).and_then(Value::as_u64))
                    .map(|n| n as usize),
                title: str_field(item, &["title", "name", "action", "heading"]),
                detail: str_field(item, &["detail", "details", "description", "desc", "text"]),
            },
        })
        .collect();
    Some(out)
}

/// Parse the model's answer. Errors when no JSON array of steps is found.
pub fn parse_steps(raw: &str) -> Result<Vec<StepText>, AiError> {
    let s = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        if let Some(out) = from_value(v) {
            return Ok(out);
        }
    }
    // Fenced or wrapped in prose: try the outermost array, then object.
    for (open, close) in [('[', ']'), ('{', '}')] {
        if let (Some(a), Some(b)) = (s.find(open), s.rfind(close)) {
            if a < b {
                if let Ok(v) = serde_json::from_str::<Value>(&s[a..=b]) {
                    if let Some(out) = from_value(v) {
                        return Ok(out);
                    }
                }
            }
        }
    }
    Err(AiError::BadResponse("no JSON step list in the answer".into()))
}

/// Write parsed texts onto `t`'s steps (by the model's step numbers when
/// they all fit, else in order). Empty titles keep the local one. Returns
/// how many steps got an AI title.
pub fn apply(t: &mut Tutorial, texts: &[StepText]) -> usize {
    let n = t.steps.len();
    let numbered = !texts.is_empty() && texts.iter().all(|x| x.index.is_some_and(|i| (1..=n).contains(&i))) && {
        let mut seen: Vec<usize> = texts.iter().filter_map(|x| x.index).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len() == texts.len()
    };
    let mut count = 0;
    for (pos, text) in texts.iter().enumerate() {
        let slot = if numbered { text.index.unwrap_or(0).wrapping_sub(1) } else { pos };
        let Some(step) = t.steps.get_mut(slot) else { continue };
        let title = text.title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            continue;
        }
        step.title = title;
        step.detail = text.detail.trim().to_string();
        count += 1;
    }
    if count > 0 {
        t.ai_written = true;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::OutputLanguage;

    fn tutorial(n: usize) -> Tutorial {
        let steps = (1..=n)
            .map(|i| Step {
                index: i,
                t: i as f64,
                frame_time: i as f64 + 0.12,
                click: [1.0, 2.0],
                button: if i == 2 { MouseButton::Right } else { MouseButton::Left },
                double_click: false,
                app: Some("Safari".into()),
                bundle_id: None,
                window_title: (i == 1).then(|| "Apple — Start\n Page".to_string()),
                ocr_hint: (i == 1).then(|| "Bookmarks".to_string()),
                title: format!("local {i}"),
                detail: String::new(),
                image: None,
                crop: None,
            })
            .collect();
        Tutorial {
            schema_version: 1,
            movie: "a.mp4".into(),
            title: "a".into(),
            video_width: 10,
            video_height: 10,
            scale: 1.0,
            ai_written: false,
            steps,
        }
    }

    #[test]
    fn request_sends_text_only_in_the_right_language() {
        let mut cfg = AiSettings::default();
        cfg.validate();
        let t = tutorial(2);
        let req = build_request(&cfg, &t, Language::ZhHans).unwrap();
        assert!(!req.stream);
        assert!(req.url.ends_with("/chat/completions"));
        let body = req.body_string();
        assert!(!body.contains("image_url"));
        let user = req.body["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("Simplified Chinese"));
        assert!(user.contains("exactly 2 objects"));
        assert!(user.contains(
            "Step 1: click at 1.0s; app: Safari; window: \"Apple — Start Page\"; text at the pointer: \"Bookmarks\""
        ));
        assert!(user.contains("Step 2: right-click"));
        cfg.output_language = OutputLanguage::English;
        let req = build_request(&cfg, &t, Language::ZhHans).unwrap();
        assert!(req.body["messages"][1]["content"].as_str().unwrap().contains("Write in English"));
        assert_eq!(build_request(&cfg, &tutorial(0), Language::En), Err(AiError::EmptyInput));
    }

    #[test]
    fn setting_defaults_on_with_ai() {
        let s: AiSettings = toml::from_str("enabled = true").unwrap();
        assert!(s.tutorial_steps && s.writes_tutorials());
        let s: AiSettings = toml::from_str("enabled = false").unwrap();
        assert!(!s.writes_tutorials());
        let s: AiSettings = toml::from_str("enabled = true\ntutorial_steps = false").unwrap();
        assert!(!s.writes_tutorials());
    }

    #[test]
    fn tolerant_parsing() {
        let plain = r#"[{"title":"Open Bookmarks","detail":"In the toolbar."},{"title":"Pick one","detail":""}]"#;
        assert_eq!(parse_steps(plain).unwrap().len(), 2);
        let fenced = format!("Sure! Here you go:\n```json\n{plain}\n```");
        assert_eq!(parse_steps(&fenced).unwrap()[0].title, "Open Bookmarks");
        let wrapped = r#"{"steps":[{"step":2,"name":"B","description":"d"},{"step":1,"action":"A"}]}"#;
        let p = parse_steps(wrapped).unwrap();
        assert_eq!(p[0], StepText { index: Some(2), title: "B".into(), detail: "d".into() });
        assert_eq!(parse_steps(r#"["Click A","Click B"]"#).unwrap()[1].title, "Click B");
        assert!(parse_steps("I cannot help with that.").is_err());
        assert!(parse_steps("{\"oops\": 1}").is_err());
    }

    #[test]
    fn apply_by_number_or_order_and_keep_local() {
        let mut t = tutorial(3);
        let texts = parse_steps(r#"{"steps":[{"step":2,"title":"B"},{"step":1,"title":"A","detail":" x "}]}"#).unwrap();
        assert_eq!(apply(&mut t, &texts), 2);
        assert_eq!(t.steps[0].title, "A");
        assert_eq!(t.steps[0].detail, "x");
        assert_eq!(t.steps[1].title, "B");
        assert_eq!(t.steps[2].title, "local 3");
        assert!(t.ai_written);

        // Numbers out of range → in order; empty title keeps local; extras ignored.
        let mut t = tutorial(2);
        let texts = vec![
            StepText { index: Some(7), title: "One".into(), detail: String::new() },
            StepText { index: Some(8), title: " ".into(), detail: "d".into() },
            StepText { index: Some(9), title: "Three".into(), detail: String::new() },
        ];
        assert_eq!(apply(&mut t, &texts), 1);
        assert_eq!(t.steps[0].title, "One");
        assert_eq!(t.steps[1].title, "local 2");
        let mut t = tutorial(1);
        assert_eq!(apply(&mut t, &[]), 0);
        assert!(!t.ai_written);
    }
}

//! Writing tasks and the OpenAI-compatible request bodies they become.

use serde_json::{json, Value};

use super::{png_data_url, AiError, AiSettings};
use crate::annotate::{Document, Shape};
use crate::i18n::{tr, Language, Msg};

/// Longest OCR/input text sent, in characters; screenshots rarely come
/// close, and it keeps a pathological capture from blowing the context.
pub const MAX_INPUT_CHARS: usize = 12_000;

/// Something the user asks AI to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WritingTask {
    /// A short description of the screenshot for sharing.
    Caption,
    /// Numbered how-to text, following the document's step badges.
    StepGuide,
    /// An accessibility description (alt text).
    AltText,
    /// Fix grammar and tighten the given text, keeping its meaning.
    Polish(String),
    /// Translate the given text.
    Translate { text: String, to: Language },
    /// Summarise the given text (usually the OCR text); empty means use
    /// [`AiContext::ocr_text`].
    Summarize(String),
    /// Rebuild a table as Markdown from OCR runs listed with their x
    /// positions ([`crate::extract::layout_listing`]); empty means use
    /// [`AiContext::ocr_text`].
    ExtractTable(String),
    /// Clean up OCR'd code (indentation already estimated locally).
    ExtractCode(String),
    /// Turn the text into a to-do checklist.
    Checklist(String),
}

impl WritingTask {
    /// Menu title.
    pub fn label(&self) -> &'static str {
        tr(match self {
            WritingTask::Caption => Msg::AiTaskCaption,
            WritingTask::StepGuide => Msg::AiTaskStepGuide,
            WritingTask::AltText => Msg::AiTaskAltText,
            WritingTask::Polish(_) => Msg::AiTaskPolish,
            WritingTask::Translate { .. } => Msg::AiTaskTranslate,
            WritingTask::Summarize(_) => Msg::AiTaskSummarize,
            WritingTask::ExtractTable(_) => Msg::AiTaskExtractTable,
            WritingTask::ExtractCode(_) => Msg::AiTaskExtractCode,
            WritingTask::Checklist(_) => Msg::AiTaskChecklist,
        })
    }

    /// Whether the screenshot image helps this task (sent only when the
    /// user enabled `send_image`).
    pub fn uses_image(&self) -> bool {
        matches!(self, WritingTask::Caption | WritingTask::StepGuide | WritingTask::AltText)
    }
}

/// One numbered step badge and the text annotation nearest to it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepHint {
    pub number: u32,
    pub label: Option<String>,
}

/// What the model gets to see about the screenshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiContext {
    /// Text recognised by Vision, assembled in reading order.
    pub ocr_text: String,
    /// Text annotations the user added, in creation order.
    pub annotation_texts: Vec<String>,
    /// Step badges in number order.
    pub steps: Vec<StepHint>,
    /// The rendered screenshot, PNG-encoded. Sent only with `send_image`.
    pub image_png: Option<Vec<u8>>,
}

/// Text annotations within this distance (points) of a step badge label it.
const STEP_LABEL_RADIUS: f64 = 160.0;

impl AiContext {
    /// Collect annotation texts and step badges from `doc`.
    pub fn from_document(doc: &Document, ocr_text: impl Into<String>) -> AiContext {
        let texts: Vec<(crate::Point, String)> = doc
            .items()
            .iter()
            .filter_map(|a| match &a.shape {
                Shape::Text { origin, text, .. } if !text.trim().is_empty() => Some((*origin, text.trim().to_string())),
                _ => None,
            })
            .collect();
        let mut steps: Vec<StepHint> = doc
            .items()
            .iter()
            .filter_map(|a| match a.shape {
                Shape::Step { center } => {
                    let number = doc.step_number(a.id)?;
                    let label = texts
                        .iter()
                        .map(|(o, t)| ((o.x - center.x).hypot(o.y - center.y), t))
                        .filter(|(d, _)| *d <= STEP_LABEL_RADIUS)
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .map(|(_, t)| t.clone());
                    Some(StepHint { number, label })
                }
                _ => None,
            })
            .collect();
        steps.sort_by_key(|s| s.number);
        AiContext {
            ocr_text: ocr_text.into(),
            annotation_texts: texts.into_iter().map(|(_, t)| t).collect(),
            steps,
            image_png: None,
        }
    }

    fn has_text(&self) -> bool {
        !self.ocr_text.trim().is_empty() || !self.annotation_texts.is_empty() || !self.steps.is_empty()
    }
}

/// A ready-to-send chat-completions request.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub url: String,
    pub body: Value,
    pub stream: bool,
}

impl ChatRequest {
    pub fn body_string(&self) -> String {
        self.body.to_string()
    }
}

fn language_name(l: Language) -> &'static str {
    match l {
        Language::En => "English",
        Language::ZhHans => "Simplified Chinese (简体中文)",
    }
}

fn truncate(s: &str) -> &str {
    match s.char_indices().nth(MAX_INPUT_CHARS) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// The language instruction for `task` under `cfg`.
pub(crate) fn language_rule(cfg: &AiSettings, task: &WritingTask, ui: Language) -> String {
    if let WritingTask::Translate { to, .. } = task {
        return format!("Write the translation in {}.", language_name(*to));
    }
    if matches!(task, WritingTask::ExtractTable(_) | WritingTask::ExtractCode(_)) {
        return "Keep every piece of text exactly in its original language; do not translate.".to_string();
    }
    if let Some(l) = cfg.output_language.fixed() {
        return format!("Always write in {}.", language_name(l));
    }
    match task {
        WritingTask::Polish(_) | WritingTask::Checklist(_) => "Keep the language of the input text.".to_string(),
        _ => format!(
            "Write in the main language of the screenshot's text. If there is no text or the language is unclear, write in {}.",
            language_name(ui)
        ),
    }
}

fn instruction(task: &WritingTask) -> &'static str {
    match task {
        WritingTask::Caption => {
            "Write one or two sentences describing this screenshot for sharing with others: what it shows and why it matters. No hashtags."
        }
        WritingTask::StepGuide => {
            "Write a short how-to guide as a numbered list based on this screenshot. If the screenshot has numbered step badges, write exactly one list item per badge, in badge order, using the badge numbers; otherwise infer the steps. One short imperative sentence per step."
        }
        WritingTask::AltText => {
            "Write alt text for this screenshot for people using a screen reader: one concise paragraph (under 60 words) naming the app or kind of content and the important visible text. Do not start with \"Image of\" or \"Screenshot of\"."
        }
        WritingTask::Polish(_) => {
            "Polish the following text: fix spelling, grammar and punctuation and make it clear and concise, keeping its meaning, tone and formatting."
        }
        WritingTask::Translate { .. } => {
            "Translate the following text faithfully and naturally, keeping its formatting, names, numbers and code unchanged."
        }
        WritingTask::Summarize(_) => {
            "Summarise the following text in a few bullet points (at most five), keeping key facts, numbers and names."
        }
        WritingTask::ExtractTable(_) => {
            "Rebuild the table in this screenshot as a GitHub-flavoured Markdown table. The text runs below were recognised by OCR and grouped into rows, each run prefixed with its left x position in pixels; runs with similar x positions belong to the same column. Use the first row as the header, keep every value exactly as written (fix only obvious OCR errors), leave empty cells empty, and reply with the table only."
        }
        WritingTask::ExtractCode(_) => {
            "The following is source code recognised by OCR from a screenshot, with indentation estimated from positions. Reconstruct the code: fix OCR mistakes (confused characters such as l/1/I, O/0, quotes and brackets), restore correct indentation and line breaks, and do not add, remove or explain anything. Reply with the code only."
        }
        WritingTask::Checklist(_) => {
            "Turn the following text into a concise to-do checklist in Markdown, one actionable item per line, each starting with \"- [ ] \" (use \"- [x] \" for items already marked done). Merge lines that belong to one item and drop text that is not a task."
        }
    }
}

const SYSTEM_PROMPT: &str = "You are the writing assistant in Lumora, a screenshot app. You write text about the user's screenshot. Reply with only the requested text: no preamble, no explanations, no surrounding quotes and no code fences.";

fn context_block(ctx: &AiContext) -> String {
    let mut out = String::new();
    let ocr = ctx.ocr_text.trim();
    if !ocr.is_empty() {
        out.push_str("Text recognised in the screenshot (OCR):\n<<<\n");
        out.push_str(truncate(ocr));
        out.push_str("\n>>>\n");
    }
    if !ctx.annotation_texts.is_empty() {
        out.push_str("Text annotations the user added:\n");
        for t in &ctx.annotation_texts {
            out.push_str(&format!("- {}\n", t.replace('\n', " ")));
        }
    }
    if !ctx.steps.is_empty() {
        out.push_str(&format!("Numbered step badges on the screenshot: {}\n", ctx.steps.len()));
        for s in &ctx.steps {
            match &s.label {
                Some(l) => out.push_str(&format!("- Step {}: labelled \"{}\"\n", s.number, l.replace('\n', " "))),
                None => out.push_str(&format!("- Step {}: no label\n", s.number)),
            }
        }
    }
    out
}

/// Build the chat-completions request for `task`.
///
/// `ui` is the app's UI language, used by the `Auto` output-language rule
/// when the screenshot has no text. The image is attached only when
/// `cfg.send_image` is on, the task benefits from it and `ctx` has one.
pub fn build_request(
    cfg: &AiSettings,
    task: &WritingTask,
    ctx: &AiContext,
    ui: Language,
    stream: bool,
) -> Result<ChatRequest, AiError> {
    let image = ctx.image_png.as_deref().filter(|_| cfg.send_image && task.uses_image());
    let user_text = match task {
        WritingTask::Polish(text) | WritingTask::Translate { text, .. } => {
            if text.trim().is_empty() {
                return Err(AiError::EmptyInput);
            }
            format!("{}\n\n<<<\n{}\n>>>", instruction(task), truncate(text.trim()))
        }
        WritingTask::Summarize(text)
        | WritingTask::ExtractTable(text)
        | WritingTask::ExtractCode(text)
        | WritingTask::Checklist(text) => {
            let text = if text.trim().is_empty() { ctx.ocr_text.trim() } else { text.trim() };
            if text.is_empty() {
                return Err(AiError::EmptyInput);
            }
            format!("{}\n\n<<<\n{}\n>>>", instruction(task), truncate(text))
        }
        WritingTask::Caption | WritingTask::StepGuide | WritingTask::AltText => {
            if !ctx.has_text() && image.is_none() {
                return Err(AiError::EmptyInput);
            }
            let mut s = instruction(task).to_string();
            if image.is_none() {
                s.push_str(" You cannot see the image; rely on the details below.");
            }
            let block = context_block(ctx);
            if !block.is_empty() {
                s.push_str("\n\n");
                s.push_str(block.trim_end());
            }
            s
        }
    };
    let system = format!("{SYSTEM_PROMPT} {}", language_rule(cfg, task, ui));
    let user_content = match image {
        Some(png) => json!([
            { "type": "text", "text": user_text },
            { "type": "image_url", "image_url": { "url": png_data_url(png) } },
        ]),
        None => Value::String(user_text),
    };
    let body = json!({
        "model": cfg.model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user_content },
        ],
        "temperature": cfg.temperature,
        "stream": stream,
    });
    Ok(ChatRequest { url: cfg.chat_completions_url(), body, stream })
}

/// The smallest useful completion request, for connection tests.
pub fn ping_body(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [{ "role": "user", "content": "ping" }],
        "max_tokens": 1,
        "stream": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::OutputLanguage;
    use crate::annotate::Style;
    use crate::{Point, Size};

    fn ctx() -> AiContext {
        AiContext { ocr_text: "Settings\nGeneral  Privacy".into(), ..AiContext::default() }
    }

    fn user_text(r: &ChatRequest) -> String {
        let c = &r.body["messages"][1]["content"];
        c.as_str().map(str::to_string).unwrap_or_else(|| c[0]["text"].as_str().unwrap().to_string())
    }

    #[test]
    fn request_shape() {
        let cfg = AiSettings::default();
        let r = build_request(&cfg, &WritingTask::Caption, &ctx(), Language::En, true).unwrap();
        assert_eq!(r.url, "https://api.openai.com/v1/chat/completions");
        assert!(r.stream);
        assert_eq!(r.body["model"], "gpt-4o-mini");
        assert_eq!(r.body["stream"], true);
        assert_eq!(r.body["temperature"], 0.3);
        let msgs = r.body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert!(user_text(&r).contains("General  Privacy"));
        let parsed: Value = serde_json::from_str(&r.body_string()).unwrap();
        assert_eq!(parsed, r.body);
    }

    #[test]
    fn each_task_has_its_own_instruction() {
        let cfg = AiSettings::default();
        let tasks = [
            WritingTask::Caption,
            WritingTask::StepGuide,
            WritingTask::AltText,
            WritingTask::Polish("teh text".into()),
            WritingTask::Translate { text: "你好".into(), to: Language::En },
            WritingTask::Summarize(String::new()),
            WritingTask::ExtractTable(String::new()),
            WritingTask::ExtractCode("fn x() {}".into()),
            WritingTask::Checklist("buy milk".into()),
        ];
        let mut seen = std::collections::HashSet::new();
        for t in &tasks {
            let r = build_request(&cfg, t, &ctx(), Language::En, false).unwrap();
            let u = user_text(&r);
            assert!(seen.insert(u.lines().next().unwrap().to_string()), "{t:?}");
            assert!(!t.label().is_empty());
        }
        let p = build_request(&cfg, &tasks[3], &ctx(), Language::En, false).unwrap();
        assert!(user_text(&p).contains("teh text") && !user_text(&p).contains("Privacy"), "polish sends only its text");
        let s = build_request(&cfg, &tasks[5], &ctx(), Language::En, false).unwrap();
        assert!(user_text(&s).contains("General  Privacy"), "summarize falls back to OCR");
    }

    #[test]
    fn empty_input_is_rejected() {
        let cfg = AiSettings::default();
        let empty = AiContext::default();
        for t in [WritingTask::Caption, WritingTask::Polish("  ".into()), WritingTask::Summarize(String::new())] {
            assert_eq!(build_request(&cfg, &t, &empty, Language::En, false), Err(AiError::EmptyInput), "{t:?}");
        }
        // An image alone is enough when it may be sent.
        let img = AiContext { image_png: Some(vec![1, 2, 3]), ..AiContext::default() };
        let send = AiSettings { send_image: true, ..cfg.clone() };
        assert!(build_request(&send, &WritingTask::AltText, &img, Language::En, false).is_ok());
        assert_eq!(build_request(&cfg, &WritingTask::AltText, &img, Language::En, false), Err(AiError::EmptyInput));
    }

    #[test]
    fn image_part_only_when_allowed() {
        let mut c = ctx();
        c.image_png = Some(vec![0x89, b'P', b'N', b'G']);
        let off = AiSettings::default();
        let r = build_request(&off, &WritingTask::Caption, &c, Language::En, false).unwrap();
        assert!(r.body["messages"][1]["content"].is_string(), "send_image off → text only");
        assert!(!r.body.to_string().contains("base64"));
        let on = AiSettings { send_image: true, ..off };
        let r = build_request(&on, &WritingTask::Caption, &c, Language::En, false).unwrap();
        let parts = r.body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,iVBORw==");
        assert!(!user_text(&r).contains("cannot see"));
        // Text-only tasks never send the image.
        let r = build_request(&on, &WritingTask::Summarize(String::new()), &c, Language::En, false).unwrap();
        assert!(r.body["messages"][1]["content"].is_string());
    }

    #[test]
    fn output_language_rule() {
        let mut cfg = AiSettings::default();
        let sys = |cfg: &AiSettings, t: &WritingTask, ui| {
            build_request(cfg, t, &ctx(), ui, false).unwrap().body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let auto = sys(&cfg, &WritingTask::Caption, Language::ZhHans);
        assert!(auto.contains("main language of the screenshot") && auto.contains("Simplified Chinese"));
        assert!(sys(&cfg, &WritingTask::Polish("x".into()), Language::ZhHans).contains("Keep the language"));
        cfg.output_language = OutputLanguage::English;
        assert!(sys(&cfg, &WritingTask::Caption, Language::ZhHans).contains("Always write in English."));
        cfg.output_language = OutputLanguage::SimplifiedChinese;
        assert!(sys(&cfg, &WritingTask::Summarize(String::new()), Language::En)
            .contains("Always write in Simplified Chinese"));
        // Translation target beats the output-language setting.
        let t = WritingTask::Translate { text: "hi".into(), to: Language::En };
        let s = sys(&cfg, &t, Language::ZhHans);
        assert!(s.contains("translation in English") && !s.contains("Always write"));
    }

    #[test]
    fn step_guide_is_aware_of_badges() {
        let mut doc = Document::new(Size::new(800.0, 600.0));
        let style = Style::default();
        doc.add(Shape::Step { center: Point::new(100.0, 100.0) }, style);
        doc.add(Shape::Step { center: Point::new(500.0, 400.0) }, style);
        doc.add(
            Shape::Text {
                origin: Point::new(130.0, 90.0),
                text: "Open Settings".into(),
                size: Size::new(0.0, 0.0),
                background: false,
            },
            style,
        );
        let c = AiContext::from_document(&doc, "General");
        assert_eq!(c.annotation_texts, vec!["Open Settings".to_string()]);
        assert_eq!(
            c.steps,
            vec![StepHint { number: 1, label: Some("Open Settings".into()) }, StepHint { number: 2, label: None }]
        );
        let r = build_request(&AiSettings::default(), &WritingTask::StepGuide, &c, Language::En, false).unwrap();
        let u = user_text(&r);
        assert!(u.contains("Numbered step badges on the screenshot: 2"));
        assert!(u.contains("Step 1: labelled \"Open Settings\""));
        assert!(u.contains("Step 2: no label"));
        assert!(u.contains("one list item per badge"));
    }

    #[test]
    fn extraction_tasks_keep_original_language() {
        let cfg = AiSettings { output_language: OutputLanguage::English, ..AiSettings::default() };
        let sys = |t: &WritingTask| {
            build_request(&cfg, t, &ctx(), Language::ZhHans, false).unwrap().body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(sys(&WritingTask::ExtractTable("row 1: [x=0] a".into())).contains("do not translate"));
        assert!(sys(&WritingTask::ExtractCode("x".into())).contains("do not translate"));
        assert!(sys(&WritingTask::Checklist("x".into())).contains("Always write in English"));
        let r =
            build_request(&cfg, &WritingTask::ExtractTable("row 1: [x=0] Name".into()), &ctx(), Language::En, false)
                .unwrap();
        let u = user_text(&r);
        assert!(u.contains("Markdown table") && u.contains("row 1: [x=0] Name") && !u.contains("Privacy"));
        assert!(!WritingTask::ExtractTable(String::new()).uses_image());
        assert_eq!(
            build_request(&cfg, &WritingTask::Checklist(" ".into()), &AiContext::default(), Language::En, false),
            Err(AiError::EmptyInput)
        );
    }

    #[test]
    fn long_input_is_truncated() {
        let c = AiContext { ocr_text: "字".repeat(MAX_INPUT_CHARS + 50), ..AiContext::default() };
        let r = build_request(&AiSettings::default(), &WritingTask::Summarize(String::new()), &c, Language::En, false)
            .unwrap();
        assert_eq!(user_text(&r).matches('字').count(), MAX_INPUT_CHARS);
    }
}

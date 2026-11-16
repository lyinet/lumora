//! Chapters and a short summary for a transcript.
//!
//! Two sources:
//!
//! * [`local_chapters`] works offline from segment timing and click times:
//!   a new chapter starts at a pause in speech or a burst of clicks, else
//!   about every minute, never sooner than [`MIN_CHAPTER_SECS`].
//! * The AI path: [`build_request`] sends the timestamped text (only text,
//!   sampled evenly to a character budget) plus click times as hints;
//!   [`parse_summary`] tolerates code fences, prose and key variants, and
//!   [`summary_from_ai`] sanitizes the answer and falls back to local
//!   chapters when it is unusable.
//!
//! [`chapter_list`] renders YouTube-style `00:00 Title` lines and
//! [`summary_markdown`] a small Markdown document.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{is_cjk, Segment};
use crate::ai::{AiError, AiSettings, ChatRequest};
use crate::i18n::Language;

/// A silence longer than this (seconds) between segments starts a chapter.
pub const GAP_SECS: f64 = 2.5;
/// Without another cue, a chapter starts about this often (seconds).
pub const TARGET_CHAPTER_SECS: f64 = 60.0;
/// No chapter is shorter than this (seconds), except the last.
pub const MIN_CHAPTER_SECS: f64 = 20.0;
/// Clicks closer together than this (seconds) form one cluster.
pub const CLICK_CLUSTER_GAP: f64 = 3.0;
/// A cluster needs at least this many clicks to start a chapter.
pub const MIN_CLUSTER_CLICKS: usize = 2;
/// Character budget of the transcript listing sent to the model.
pub const PROMPT_CHAR_BUDGET: usize = 12_000;
/// Words in a local Latin title.
const TITLE_WORDS: usize = 6;
/// Characters in a local CJK title.
const TITLE_CJK_CHARS: usize = 12;
/// Longest line of transcript sent per segment (characters).
const MAX_LINE: usize = 300;
/// Most click hints sent.
const MAX_CLICK_HINTS: usize = 200;
/// Chapters starting closer than this (seconds) are duplicates.
const DEDUPE_SECS: f64 = 1.0;

/// One chapter: start time in seconds and a title.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    /// Start time in seconds.
    pub start: f64,
    /// Short title.
    pub title: String,
}

impl Chapter {
    /// Convenience constructor.
    pub fn new(start: f64, title: impl Into<String>) -> Self {
        Self { start, title: title.into() }
    }
}

/// A short summary plus chapters.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Summary {
    /// A few sentences describing the recording (may be empty).
    #[serde(default)]
    pub summary: String,
    /// Chapters in time order, the first at 0.
    #[serde(default)]
    pub chapters: Vec<Chapter>,
}

// ---------------------------------------------------------------- local --

fn is_title_punct(c: char) -> bool {
    c.is_ascii_punctuation() || "，。！？、；：（）「」『』《》【】〈〉…—–·“”‘’".contains(c)
}

/// Local title: first ~6 words (Latin) or ~12 characters (CJK) of `text`,
/// trimmed of surrounding punctuation and whitespace.
pub fn local_title(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cjk = text.chars().filter(|c| is_cjk(*c) && !is_title_punct(*c)).count();
    let letters = text.chars().filter(|c| c.is_alphanumeric()).count();
    let raw: String = if cjk * 2 >= letters.max(1) {
        // Cut at the first sentence end within the budget, else at the budget.
        let mut s = String::new();
        for c in text.chars() {
            if s.chars().count() >= TITLE_CJK_CHARS || (!s.is_empty() && "。！？；".contains(c)) {
                break;
            }
            s.push(c);
        }
        s
    } else {
        text.split(' ').take(TITLE_WORDS).collect::<Vec<_>>().join(" ")
    };
    raw.trim_matches(|c: char| c.is_whitespace() || is_title_punct(c)).to_string()
}

/// Start times of click clusters (clicks within [`CLICK_CLUSTER_GAP`] of
/// each other, at least [`MIN_CLUSTER_CLICKS`] of them).
pub fn click_clusters(click_times: &[f64]) -> Vec<f64> {
    let mut t: Vec<f64> = click_times.iter().copied().filter(|x| x.is_finite() && *x >= 0.0).collect();
    t.sort_by(f64::total_cmp);
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let mut j = i + 1;
        while j < t.len() && t[j] - t[j - 1] <= CLICK_CLUSTER_GAP {
            j += 1;
        }
        if j - i >= MIN_CLUSTER_CLICKS {
            out.push(t[i]);
        }
        i = j;
    }
    out
}

/// Chapters from timing alone. The first starts at 0; a new one starts at
/// a segment after a pause longer than [`GAP_SECS`] or at a click cluster,
/// else every ~[`TARGET_CHAPTER_SECS`]; none shorter than
/// [`MIN_CHAPTER_SECS`]. Titles come from [`local_title`]. Empty input
/// gives no chapters.
pub fn local_chapters(segments: &[Segment], click_times: &[f64]) -> Vec<Chapter> {
    let mut segs: Vec<&Segment> = segments.iter().filter(|s| !s.text.trim().is_empty()).collect();
    segs.sort_by(|a, b| a.start.total_cmp(&b.start));
    let Some(first) = segs.first() else { return Vec::new() };
    let clusters = click_clusters(click_times);
    let mut out = vec![Chapter::new(0.0, local_title(&first.text))];
    let mut chapter_start = 0.0;
    for pair in segs.windows(2) {
        let (prev, seg) = (pair[0], pair[1]);
        if seg.start - chapter_start < MIN_CHAPTER_SECS {
            continue;
        }
        let gap = seg.start - prev.end > GAP_SECS;
        let clicked = clusters.iter().any(|&c| c > prev.start && c <= seg.start);
        let long = seg.start - chapter_start >= TARGET_CHAPTER_SECS;
        if gap || clicked || long {
            let title = local_title(&seg.text);
            if title.is_empty() {
                continue;
            }
            chapter_start = seg.start;
            out.push(Chapter::new(seg.start, title));
        }
    }
    if out[0].title.is_empty() {
        out[0].title = out.get(1).map_or_else(|| "Introduction".into(), |c| c.title.clone());
    }
    out
}

/// Summary with local chapters and no summary text.
pub fn local_summary(segments: &[Segment], click_times: &[f64]) -> Summary {
    Summary { summary: String::new(), chapters: local_chapters(segments, click_times) }
}

// ----------------------------------------------------------- formatting --

/// `MM:SS`, or `H:MM:SS` when `long` (or when `secs` ≥ 1 h).
pub fn format_chapter_time(secs: f64, long: bool) -> String {
    let total = if secs.is_finite() { secs.max(0.0).floor() as u64 } else { 0 };
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if long || h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// YouTube-style chapter list, one `00:00 Title` per line. All times use
/// `H:MM:SS` when any chapter starts at or after one hour.
pub fn chapter_list(chapters: &[Chapter]) -> String {
    let long = chapters.iter().any(|c| c.start >= 3600.0);
    chapters.iter().map(|c| format!("{} {}\n", format_chapter_time(c.start, long), c.title)).collect()
}

/// Markdown: `# title`, the summary, then a `## Chapters` list. Empty parts
/// are left out.
pub fn summary_markdown(summary: &Summary, title: &str) -> String {
    let mut parts = Vec::new();
    if !title.trim().is_empty() {
        parts.push(format!("# {}\n", title.trim()));
    }
    if !summary.summary.trim().is_empty() {
        parts.push(format!("{}\n", summary.summary.trim()));
    }
    if !summary.chapters.is_empty() {
        let long = summary.chapters.iter().any(|c| c.start >= 3600.0);
        let list: String = summary
            .chapters
            .iter()
            .map(|c| format!("- `{}` {}\n", format_chapter_time(c.start, long), c.title))
            .collect();
        parts.push(format!("## Chapters\n\n{list}"));
    }
    parts.join("\n")
}

// ------------------------------------------------------------------- AI --

fn language_name(l: Language) -> &'static str {
    match l {
        Language::En => "English",
        Language::ZhHans => "Simplified Chinese (简体中文)",
    }
}

fn duration_of(segments: &[Segment]) -> f64 {
    segments.iter().map(|s| s.end.max(s.start)).filter(|x| x.is_finite()).fold(0.0, f64::max)
}

/// Timestamped listing `[MM:SS] text`, one segment per line. When longer
/// than `budget` characters, lines are sampled evenly across the recording.
pub fn transcript_listing(segments: &[Segment], budget: usize) -> String {
    let long = duration_of(segments) >= 3600.0;
    let lines: Vec<String> = segments
        .iter()
        .filter_map(|s| {
            let text = s.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return None;
            }
            let text: String = text.chars().take(MAX_LINE).collect();
            Some(format!("[{}] {}\n", format_chapter_time(s.start, long), text))
        })
        .collect();
    let len = |l: &String| l.chars().count();
    let total: usize = lines.iter().map(len).sum();
    if total <= budget || lines.is_empty() {
        return lines.concat();
    }
    let n = lines.len();
    let mut keep = (budget * n / total.max(1)).clamp(1, n);
    loop {
        let picked: Vec<&String> = (0..keep).map(|i| &lines[i * n / keep]).collect();
        if keep == 1 || picked.iter().map(|l| len(l)).sum::<usize>() <= budget {
            return picked.into_iter().map(String::as_str).collect();
        }
        keep -= 1;
    }
}

const SYSTEM: &str = "You are the writing assistant in Lumora, a screen-recording app. You turn a timestamped transcript into a short summary and chapters. Reply with JSON only: no preamble, no explanations, no code fences.";

/// Build the chat request (non-streaming) for chapters and a summary,
/// written in `lang`.
///
/// # Errors
/// [`AiError::EmptyInput`] when there is no transcript text.
pub fn build_request(
    cfg: &AiSettings,
    segments: &[Segment],
    click_times: &[f64],
    lang: Language,
) -> Result<ChatRequest, AiError> {
    let listing = transcript_listing(segments, PROMPT_CHAR_BUDGET);
    if listing.is_empty() {
        return Err(AiError::EmptyInput);
    }
    let duration = duration_of(segments);
    let long = duration >= 3600.0;
    let mut clicks: Vec<f64> = click_times.iter().copied().filter(|t| t.is_finite() && *t >= 0.0).collect();
    clicks.sort_by(f64::total_cmp);
    let hints = if clicks.is_empty() {
        "(none)".to_string()
    } else {
        let n = clicks.len();
        let k = n.min(MAX_CLICK_HINTS);
        (0..k).map(|i| format_chapter_time(clicks[i * n / k], long)).collect::<Vec<_>>().join(", ")
    };
    let user = format!(
        "Below is the transcript of a screen recording ({dur} long), one line per segment as [MM:SS] text. It may be sampled and may contain recognition errors. Mouse clicks happened at these times (hints for where a new step or topic may begin): {hints}.\n\n\
Write a summary of 2–4 sentences and split the recording into chapters at real topic changes (roughly one per minute or two, at least 20 seconds each). Chapter titles are short (at most 6 words), descriptive, without numbering. The first chapter starts at 00:00. Write in {lang}.\n\n\
Answer with a JSON object: {{\"summary\": \"…\", \"chapters\": [{{\"start\": \"MM:SS\", \"title\": \"…\"}}]}}.\n\n<<<\n{listing}>>>",
        dur = format_chapter_time(duration, false),
        lang = language_name(lang),
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

/// Parse a time: a number of seconds, or `"HH:MM:SS"`, `"MM:SS"`,
/// `"12.5"`, `"12.5s"` (comma decimals accepted).
pub fn parse_time(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite() && *x >= 0.0),
        Value::String(s) => parse_time_str(s),
        _ => None,
    }
}

fn parse_time_str(s: &str) -> Option<f64> {
    let s = s.trim().trim_matches(['[', ']', '(', ')']).trim();
    let s = s.strip_suffix('s').unwrap_or(s).trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for p in &parts {
        let x: f64 = p.trim().parse().ok()?;
        if !x.is_finite() || x < 0.0 {
            return None;
        }
        total = total * 60.0 + x;
    }
    Some(total)
}

/// Split `"00:00 Intro"` / `"[01:30] - Setup"` into time and title.
fn split_line(s: &str) -> Option<Chapter> {
    let s = s.trim();
    let (head, rest) = s.split_once(char::is_whitespace)?;
    let start = parse_time_str(head)?;
    let title = rest.trim().trim_start_matches(['-', '–', '—', ':']).trim();
    Some(Chapter::new(start, title))
}

fn str_field(v: &Value, keys: &[&str]) -> String {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn chapter_from(item: &Value) -> Option<Chapter> {
    match item {
        Value::String(s) => split_line(s),
        Value::Object(_) => {
            let start = ["start", "time", "timestamp", "ts", "at", "start_time", "startTime", "seconds", "begin"]
                .iter()
                .find_map(|k| item.get(*k).and_then(parse_time))?;
            let title = str_field(item, &["title", "name", "heading", "label", "chapter", "topic", "text"]);
            Some(Chapter::new(start, title))
        }
        _ => None,
    }
}

fn chapters_from(arr: &[Value]) -> Vec<Chapter> {
    arr.iter().filter_map(chapter_from).collect()
}

fn from_value(v: Value) -> Option<Summary> {
    match v {
        Value::Array(a) => {
            let chapters = chapters_from(&a);
            (!chapters.is_empty()).then_some(Summary { summary: String::new(), chapters })
        }
        Value::Object(o) => {
            let v = Value::Object(o);
            let summary = str_field(&v, &["summary", "abstract", "overview", "description", "tldr"]);
            let chapters = ["chapters", "sections", "outline", "timestamps", "items", "toc"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_array))
                .map(|a| chapters_from(a))
                .unwrap_or_default();
            (!summary.is_empty() || !chapters.is_empty()).then_some(Summary { summary, chapters })
        }
        _ => None,
    }
}

/// Parse the model's answer (not yet sanitized; see [`sanitize_chapters`]).
///
/// # Errors
/// [`AiError::BadResponse`] when no summary or chapters are found.
pub fn parse_summary(raw: &str) -> Result<Summary, AiError> {
    let s = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        if let Some(out) = from_value(v) {
            return Ok(out);
        }
    }
    for (open, close) in [('{', '}'), ('[', ']')] {
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
    Err(AiError::BadResponse("no JSON summary or chapters in the answer".into()))
}

/// Clean chapters: collapse whitespace in titles and drop empty ones, drop
/// starts beyond `duration` (when `duration` > 0), sort, drop chapters
/// within a second of the previous one, and move the first to 0.
pub fn sanitize_chapters(chapters: Vec<Chapter>, duration: f64) -> Vec<Chapter> {
    let mut v: Vec<Chapter> = chapters
        .into_iter()
        .filter_map(|c| {
            let title = c.title.split_whitespace().collect::<Vec<_>>().join(" ");
            let ok = c.start.is_finite() && c.start >= 0.0 && !title.is_empty();
            let in_range = duration <= 0.0 || c.start < duration;
            (ok && in_range).then_some(Chapter::new(c.start, title))
        })
        .collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<Chapter> = Vec::with_capacity(v.len());
    for c in v {
        if out.last().is_some_and(|p| c.start - p.start < DEDUPE_SECS) {
            continue;
        }
        out.push(c);
    }
    if let Some(first) = out.first_mut() {
        first.start = 0.0;
    }
    out
}

/// Turn the model's raw answer into a [`Summary`]: parsed and sanitized
/// against the transcript's duration. When parsing fails or no chapter
/// survives, the chapters come from [`local_chapters`] (the AI summary text
/// is kept when there is one).
pub fn summary_from_ai(raw: &str, segments: &[Segment], click_times: &[f64]) -> Summary {
    let parsed = parse_summary(raw).unwrap_or_default();
    let chapters = sanitize_chapters(parsed.chapters, duration_of(segments));
    let chapters = if chapters.is_empty() { local_chapters(segments, click_times) } else { chapters };
    Summary { summary: parsed.summary.trim().to_string(), chapters }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, text: &str) -> Segment {
        Segment::new(start, end, text)
    }

    /// Continuous speech, one 5 s segment after another, no gaps.
    fn steady(n: usize) -> Vec<Segment> {
        (0..n).map(|i| seg(i as f64 * 5.0, i as f64 * 5.0 + 5.0, &format!("Sentence number {i} here."))).collect()
    }

    #[test]
    fn titles_latin_and_cjk() {
        assert_eq!(
            local_title("  Hello, everyone! Today we will look at Lumora."),
            "Hello, everyone! Today we will look"
        );
        assert_eq!(local_title("“Open the menu.”"), "Open the menu");
        assert_eq!(local_title("大家好，今天我们来看看如何使用录屏功能。"), "大家好，今天我们来看看如");
        assert_eq!(local_title("首先打开设置。然后点击"), "首先打开设置");
        assert_eq!(local_title("..."), "");
    }

    #[test]
    fn local_empty_input() {
        assert!(local_chapters(&[], &[1.0, 2.0]).is_empty());
        assert!(local_chapters(&[seg(0.0, 1.0, "  ")], &[]).is_empty());
    }

    #[test]
    fn local_every_minute_without_cues() {
        let segs = steady(40); // 200 s
        let ch = local_chapters(&segs, &[]);
        let starts: Vec<f64> = ch.iter().map(|c| c.start).collect();
        assert_eq!(starts, vec![0.0, 60.0, 120.0, 180.0]);
        assert_eq!(ch[1].title, "Sentence number 12 here");
    }

    #[test]
    fn local_first_chapter_at_zero_even_if_speech_starts_late() {
        let segs = vec![seg(4.0, 8.0, "Welcome back"), seg(8.0, 10.0, "more")];
        assert_eq!(local_chapters(&segs, &[]), vec![Chapter::new(0.0, "Welcome back")]);
    }

    #[test]
    fn local_gap_starts_chapter_but_respects_min_length() {
        let mut segs = vec![seg(0.0, 5.0, "Intro part"), seg(10.0, 15.0, "too soon after a gap")];
        segs.push(seg(15.0, 22.0, "still intro"));
        segs.push(seg(26.0, 30.0, "Second topic starts"));
        let ch = local_chapters(&segs, &[]);
        assert_eq!(ch, vec![Chapter::new(0.0, "Intro part"), Chapter::new(26.0, "Second topic starts")]);
    }

    #[test]
    fn local_click_cluster_starts_chapter() {
        let segs = steady(12); // 60 s, no gaps
                               // A lone click does nothing; a burst at 31–32 s starts a chapter at 35 s.
        let ch = local_chapters(&segs, &[12.0, 32.0, 31.0]);
        let starts: Vec<f64> = ch.iter().map(|c| c.start).collect();
        assert_eq!(starts, vec![0.0, 35.0]);
        assert_eq!(click_clusters(&[1.0, 2.0, 2.5, 10.0, 20.0, 22.0, f64::NAN]), vec![1.0, 20.0]);
    }

    #[test]
    fn time_formatting_and_chapter_list() {
        assert_eq!(format_chapter_time(0.0, false), "00:00");
        assert_eq!(format_chapter_time(75.9, false), "01:15");
        assert_eq!(format_chapter_time(3725.0, false), "1:02:05");
        assert_eq!(format_chapter_time(5.0, true), "0:00:05");
        let ch = vec![Chapter::new(0.0, "Intro"), Chapter::new(95.0, "Setup")];
        assert_eq!(chapter_list(&ch), "00:00 Intro\n01:35 Setup\n");
        let ch = vec![Chapter::new(0.0, "Intro"), Chapter::new(3600.0, "Late")];
        assert_eq!(chapter_list(&ch), "0:00:00 Intro\n1:00:00 Late\n");
        assert_eq!(chapter_list(&[]), "");
    }

    #[test]
    fn markdown_output() {
        let s = Summary {
            summary: " A demo of recording. ".into(),
            chapters: vec![Chapter::new(0.0, "Intro"), Chapter::new(61.0, "Export")],
        };
        assert_eq!(
            summary_markdown(&s, "Demo"),
            "# Demo\n\nA demo of recording.\n\n## Chapters\n\n- `00:00` Intro\n- `01:01` Export\n"
        );
        let s = Summary { summary: String::new(), chapters: vec![Chapter::new(0.0, "Only")] };
        assert_eq!(summary_markdown(&s, ""), "## Chapters\n\n- `00:00` Only\n");
        assert_eq!(summary_markdown(&Summary::default(), "T"), "# T\n");
    }

    #[test]
    fn listing_formats_and_samples_evenly() {
        let segs = vec![seg(0.0, 1.0, " Hello   world "), seg(65.0, 66.0, ""), seg(75.0, 76.0, "Next")];
        assert_eq!(transcript_listing(&segs, 1000), "[00:00] Hello world\n[01:15] Next\n");
        let many: Vec<Segment> = (0..1000).map(|i| seg(i as f64, i as f64 + 1.0, &"x".repeat(40))).collect();
        let out = transcript_listing(&many, PROMPT_CHAR_BUDGET);
        assert!(out.chars().count() <= PROMPT_CHAR_BUDGET);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() > 100);
        assert!(lines[0].starts_with("[00:00]"));
        // Sampling spans the whole recording.
        assert!(lines.last().unwrap().starts_with("[16:"));
    }

    #[test]
    fn request_contents() {
        let mut cfg = AiSettings::default();
        cfg.validate();
        let segs = vec![seg(0.0, 2.0, "Hello there"), seg(70.0, 72.0, "Second part")];
        let req = build_request(&cfg, &segs, &[71.0, 3.5], Language::ZhHans).unwrap();
        assert!(!req.stream);
        assert!(req.url.ends_with("/chat/completions"));
        let user = req.body["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("Simplified Chinese"));
        assert!(user.contains("[00:00] Hello there\n[01:10] Second part\n"));
        assert!(user.contains("00:03, 01:11"));
        assert!(user.contains("01:12 long"));
        assert!(user.contains("\"chapters\""));
        let req = build_request(&cfg, &segs, &[], Language::En).unwrap();
        let user = req.body["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("Write in English") && user.contains("(none)"));
        assert_eq!(build_request(&cfg, &[seg(0.0, 1.0, " ")], &[], Language::En), Err(AiError::EmptyInput));
    }

    #[test]
    fn parse_times() {
        assert_eq!(parse_time(&json!(12.5)), Some(12.5));
        assert_eq!(parse_time(&json!("01:30")), Some(90.0));
        assert_eq!(parse_time(&json!("1:02:03")), Some(3723.0));
        assert_eq!(parse_time(&json!("45s")), Some(45.0));
        assert_eq!(parse_time(&json!("[00:07]")), Some(7.0));
        assert_eq!(parse_time(&json!("1,5")), Some(1.5));
        assert_eq!(parse_time(&json!("soon")), None);
        assert_eq!(parse_time(&json!(-1)), None);
        assert_eq!(parse_time(&json!(null)), None);
    }

    #[test]
    fn tolerant_parsing() {
        let plain =
            r#"{"summary":"A demo.","chapters":[{"start":"00:00","title":"Intro"},{"start":75,"title":"Setup"}]}"#;
        let p = parse_summary(plain).unwrap();
        assert_eq!(p.summary, "A demo.");
        assert_eq!(p.chapters, vec![Chapter::new(0.0, "Intro"), Chapter::new(75.0, "Setup")]);
        let fenced = format!("Here you go:\n```json\n{plain}\n```\nHope it helps!");
        assert_eq!(parse_summary(&fenced).unwrap(), p);
        let variants = r#"{"overview":"x","sections":[{"timestamp":"1:00:00","name":"A"},{"time":"02:00","heading":"B"},{"title":"no time"}]}"#;
        let p = parse_summary(variants).unwrap();
        assert_eq!(p.summary, "x");
        assert_eq!(p.chapters, vec![Chapter::new(3600.0, "A"), Chapter::new(120.0, "B")]);
        let lines = r#"["00:00 Intro", "[01:30] - Setup", "nonsense"]"#;
        assert_eq!(
            parse_summary(lines).unwrap().chapters,
            vec![Chapter::new(0.0, "Intro"), Chapter::new(90.0, "Setup")]
        );
        assert!(parse_summary("I cannot help with that.").is_err());
        assert!(parse_summary("{\"oops\": 1}").is_err());
    }

    #[test]
    fn sanitizing() {
        let raw = vec![
            Chapter::new(130.0, "  Wrap   up "),
            Chapter::new(5.0, "Intro"),
            Chapter::new(60.0, "Middle"),
            Chapter::new(60.4, "Middle again"),
            Chapter::new(90.0, "   "),
            Chapter::new(500.0, "Past the end"),
            Chapter::new(f64::NAN, "Bad"),
        ];
        let s = sanitize_chapters(raw, 200.0);
        assert_eq!(s, vec![Chapter::new(0.0, "Intro"), Chapter::new(60.0, "Middle"), Chapter::new(130.0, "Wrap up")]);
        // Unknown duration keeps late chapters.
        assert_eq!(sanitize_chapters(vec![Chapter::new(500.0, "X")], 0.0), vec![Chapter::new(0.0, "X")]);
        assert!(sanitize_chapters(vec![], 10.0).is_empty());
    }

    #[test]
    fn ai_with_local_fallback() {
        let segs = steady(40);
        let good = r#"{"summary":"S.","chapters":[{"start":"00:10","title":"A"},{"start":"02:00","title":"B"},{"start":"09:00","title":"C"}]}"#;
        let s = summary_from_ai(good, &segs, &[]);
        assert_eq!(s.summary, "S.");
        assert_eq!(s.chapters, vec![Chapter::new(0.0, "A"), Chapter::new(120.0, "B")]);
        // Summary only: chapters fall back to local.
        let s = summary_from_ai(r#"{"summary":"Only text"}"#, &segs, &[]);
        assert_eq!(s.summary, "Only text");
        assert_eq!(s.chapters, local_chapters(&segs, &[]));
        // Garbage: fully local.
        assert_eq!(summary_from_ai("nope", &segs, &[]), local_summary(&segs, &[]));
    }

    #[test]
    fn serde_round_trip() {
        let s = Summary { summary: "x".into(), chapters: vec![Chapter::new(1.5, "A")] };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Summary>(&json).unwrap(), s);
        assert_eq!(serde_json::from_str::<Summary>("{}").unwrap(), Summary::default());
    }
}

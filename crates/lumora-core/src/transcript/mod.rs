//! Transcripts and subtitles.
//!
//! A [`Transcript`] is the engine-independent result of speech recognition on
//! a recording: a list of timed [`Segment`]s. It is stored as a JSON sidecar
//! next to the movie (see [`sidecar_path`]) and exported as SRT or WebVTT
//! (see [`to_srt`], [`to_vtt`]).
//!
//! Line wrapping is CJK-aware: Latin text breaks on spaces with a budget of
//! [`MAX_LATIN`] characters per line, CJK text may break between any two
//! characters with a budget of [`MAX_CJK`] characters per line. Mixed lines
//! charge each character its share of the line (a CJK character costs
//! `1 / MAX_CJK` of a line, anything else `1 / MAX_LATIN`).

pub mod burn;
pub mod chapters;
mod srt;

pub use chapters::{Chapter, Summary};
pub use srt::{format_srt_time, format_vtt_time, parse_srt, parse_timestamp, to_srt, to_vtt};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Current transcript sidecar schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// Default maximum characters per line for Latin (space-separated) text.
pub const MAX_LATIN: usize = 42;
/// Default maximum characters per line for CJK text.
pub const MAX_CJK: usize = 20;
/// Maximum number of lines a single subtitle cue may occupy.
pub const MAX_LINES: usize = 2;
/// Segments shorter than this (seconds) are merged into a neighbour.
pub const MIN_SEGMENT_SECS: f64 = 0.6;

/// One timed piece of transcript text. Times are seconds from the start of
/// the movie.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// Start time in seconds.
    pub start: f64,
    /// End time in seconds.
    pub end: f64,
    /// Spoken text.
    pub text: String,
}

impl Segment {
    /// Convenience constructor.
    pub fn new(start: f64, end: f64, text: impl Into<String>) -> Self {
        Self { start, end, text: text.into() }
    }

    /// Duration in seconds (never negative).
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// Errors reading a transcript sidecar.
#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    /// The JSON could not be parsed.
    #[error("invalid transcript JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The sidecar was written by a newer, incompatible Lumora.
    #[error("unsupported transcript schema version {0}")]
    UnsupportedVersion(u32),
}

/// A full transcript of one recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// Schema version; always [`SCHEMA_VERSION`] when written by this build.
    pub schema_version: u32,
    /// BCP-47 language tag (e.g. `en`, `zh-Hans`).
    pub language: String,
    /// Identifier of the recognition engine that produced it.
    pub engine: String,
    /// Timed segments in playback order.
    pub segments: Vec<Segment>,
    /// Summary and chapters, when generated. Optional within schema 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
}

impl Transcript {
    /// New transcript at the current schema version.
    pub fn new(language: impl Into<String>, engine: impl Into<String>, segments: Vec<Segment>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            language: language.into(),
            engine: engine.into(),
            segments,
            summary: None,
        }
    }

    /// Serialize to pretty-printed JSON.
    ///
    /// # Errors
    /// Only if serialization fails (e.g. a non-finite time).
    pub fn to_json(&self) -> Result<String, TranscriptError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse from JSON, rejecting unknown schema versions.
    ///
    /// # Errors
    /// Invalid JSON or an unsupported `schema_version`.
    pub fn from_json(json: &str) -> Result<Self, TranscriptError> {
        let t: Self = serde_json::from_str(json)?;
        if t.schema_version != SCHEMA_VERSION {
            return Err(TranscriptError::UnsupportedVersion(t.schema_version));
        }
        Ok(t)
    }
}

/// Path of the transcript sidecar for `movie`: `<stem>.transcript.json` in
/// the same directory.
pub fn sidecar_path(movie: &Path) -> PathBuf {
    let stem = movie.file_stem().map_or_else(|| "transcript".into(), |s| s.to_string_lossy());
    movie.with_file_name(format!("{stem}.transcript.json"))
}

/// Whether `c` is a CJK character (Han, kana, Hangul, bopomofo, CJK
/// punctuation or full-width forms) that may be line-broken anywhere.
pub fn is_cjk(c: char) -> bool {
    matches!(u32::from(c),
        0x1100..=0x11FF       // Hangul Jamo
        | 0x2E80..=0x2FDF     // CJK radicals, Kangxi
        | 0x3000..=0x303F     // CJK symbols & punctuation
        | 0x3040..=0x30FF     // Hiragana, Katakana
        | 0x3100..=0x312F     // Bopomofo
        | 0x3130..=0x318F     // Hangul compatibility Jamo
        | 0x31F0..=0x31FF     // Katakana phonetic extensions
        | 0x3400..=0x4DBF     // CJK ext A
        | 0x4E00..=0x9FFF     // CJK unified
        | 0xAC00..=0xD7AF     // Hangul syllables
        | 0xF900..=0xFAFF     // CJK compatibility ideographs
        | 0xFE30..=0xFE4F     // CJK compatibility forms
        | 0xFF00..=0xFFEF     // Half/full-width forms
        | 0x20000..=0x2FA1F) // CJK ext B..
}

/// Punctuation that should not start a line; it may overhang the previous one.
fn is_closing_punct(c: char) -> bool {
    "，。！？、；：）」』》】〉,.!?;:)".contains(c)
}

enum Token {
    Word(String),
    Cjk(char),
}

/// Line-width calculator: a line holds `max_latin * max_cjk` units; a CJK
/// char costs `max_latin`, any other char `max_cjk`.
struct Width {
    latin_cost: usize,
    cjk_cost: usize,
    limit: usize,
}

impl Width {
    fn new(max_latin: usize, max_cjk: usize) -> Self {
        let max_latin = max_latin.max(1);
        let max_cjk = max_cjk.max(1);
        Self { latin_cost: max_cjk, cjk_cost: max_latin, limit: max_latin * max_cjk }
    }

    fn of(&self, s: &str) -> usize {
        s.chars().map(|c| if is_cjk(c) { self.cjk_cost } else { self.latin_cost }).sum()
    }
}

/// Wrap `text` into lines of at most `max_latin` Latin characters (breaking
/// on spaces) or `max_cjk` CJK characters (breaking anywhere). Whitespace is
/// collapsed. A single word longer than a line is hard-broken. Closing
/// punctuation never starts a line.
pub fn wrap_lines(text: &str, max_latin: usize, max_cjk: usize) -> Vec<String> {
    let w = Width::new(max_latin, max_cjk);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_w = 0usize;

    for (spaced, tok) in tokenize_precise(text) {
        let piece = match &tok {
            Token::Word(s) => s.clone(),
            Token::Cjk(c) => c.to_string(),
        };
        let sep = if spaced && !line.is_empty() { " " } else { "" };
        let piece_w = w.of(&piece);
        let sep_w = w.of(sep);
        let overhang = matches!(tok, Token::Cjk(c) if is_closing_punct(c))
            || (piece.chars().count() == 1 && piece.chars().all(is_closing_punct) && !spaced);
        if overhang && !line.is_empty() {
            // Closing punctuation hangs off the current line, even if full.
            line.push_str(sep);
            line.push_str(&piece);
            line_w += sep_w + piece_w;
            continue;
        }
        if line.is_empty() || line_w + sep_w + piece_w <= w.limit {
            line.push_str(sep);
            line.push_str(&piece);
            line_w += sep_w + piece_w;
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(&piece);
            line_w = piece_w;
        }
        // Hard-break an over-long word.
        while line_w > w.limit && line.chars().count() > 1 {
            let mut head = String::new();
            let mut head_w = 0;
            let mut rest = String::new();
            for c in line.chars() {
                let cw = w.of(c.encode_utf8(&mut [0; 4]));
                if rest.is_empty() && (head.is_empty() || head_w + cw <= w.limit) {
                    head.push(c);
                    head_w += cw;
                } else {
                    rest.push(c);
                }
            }
            if rest.is_empty() {
                break;
            }
            lines.push(head);
            line_w = w.of(&rest);
            line = rest;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Tokenize into words / CJK chars, each flagged with whether whitespace
/// preceded it in the source.
fn tokenize_precise(text: &str) -> Vec<(bool, Token)> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut word_spaced = false;
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                out.push((word_spaced, Token::Word(std::mem::take(&mut word))));
            }
            pending_space = true;
        } else if is_cjk(c) {
            if !word.is_empty() {
                out.push((word_spaced, Token::Word(std::mem::take(&mut word))));
            }
            out.push((pending_space, Token::Cjk(c)));
            pending_space = false;
        } else {
            if word.is_empty() {
                word_spaced = pending_space;
                pending_space = false;
            }
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push((word_spaced, Token::Word(word)));
    }
    out
}

/// Join two text fragments: with a space between Latin fragments, directly
/// when either side of the seam is CJK.
fn join_text(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }
    let seam_cjk = a.chars().next_back().is_some_and(is_cjk) || b.chars().next().is_some_and(is_cjk);
    if seam_cjk {
        format!("{a}{b}")
    } else {
        format!("{a} {b}")
    }
}

/// Clean up raw recognizer output for display:
///
/// 1. trim text and drop empty segments (and fix `end < start`);
/// 2. merge segments shorter than [`MIN_SEGMENT_SECS`] into a neighbour
///    (the previous one, or the next one for the first segment);
/// 3. split segments whose text wraps to more than [`MAX_LINES`] lines into
///    pieces of at most two lines, dividing the time proportionally to each
///    piece's character count.
pub fn normalize(segments: &[Segment]) -> Vec<Segment> {
    // 1. trim / drop empty
    let mut segs: Vec<Segment> = segments
        .iter()
        .filter_map(|s| {
            let text = s.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return None;
            }
            let start = s.start.max(0.0);
            Some(Segment::new(start, s.end.max(start), text))
        })
        .collect();

    // 2. merge short segments
    while segs.len() > 1 {
        let Some(i) = segs.iter().position(|s| s.duration() < MIN_SEGMENT_SECS) else {
            break;
        };
        let (a, b) = if i == 0 { (0, 1) } else { (i - 1, i) };
        let second = segs.remove(b);
        let first = &mut segs[a];
        first.start = first.start.min(second.start);
        first.end = first.end.max(second.end);
        first.text = join_text(&first.text, &second.text);
    }

    // 3. split long segments
    let mut out = Vec::with_capacity(segs.len());
    for s in segs {
        let lines = wrap_lines(&s.text, MAX_LATIN, MAX_CJK);
        if lines.len() <= MAX_LINES {
            out.push(s);
            continue;
        }
        let pieces: Vec<String> =
            lines.chunks(MAX_LINES).map(|c| c.iter().fold(String::new(), |acc, l| join_text(&acc, l))).collect();
        out.extend(split_proportional(s.start, s.end, pieces));
    }
    out
}

/// Split `[start, end]` among `pieces` in proportion to each piece's
/// non-whitespace character count. The last piece ends exactly at `end`.
pub(crate) fn split_proportional(start: f64, end: f64, pieces: Vec<String>) -> Vec<Segment> {
    if pieces.is_empty() {
        return Vec::new();
    }
    let counts: Vec<usize> = pieces.iter().map(|p| p.chars().filter(|c| !c.is_whitespace()).count().max(1)).collect();
    let total: usize = counts.iter().sum();
    let dur = (end - start).max(0.0);
    let mut t = start;
    let mut acc = 0usize;
    let last = pieces.len() - 1;
    let mut out = Vec::with_capacity(pieces.len());
    for (k, (text, n)) in pieces.into_iter().zip(counts).enumerate() {
        acc += n;
        #[allow(clippy::cast_precision_loss)]
        let e = if k == last { end } else { start + dur * acc as f64 / total as f64 };
        out.push(Segment::new(t, e, text));
        t = e;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_detection() {
        assert!(is_cjk('中'));
        assert!(is_cjk('か'));
        assert!(is_cjk('한'));
        assert!(is_cjk('，'));
        assert!(!is_cjk('a'));
        assert!(!is_cjk(' '));
        assert!(!is_cjk('é'));
    }

    #[test]
    fn wrap_latin_breaks_on_spaces() {
        let text = "The quick brown fox jumps over the lazy dog and keeps on running far away";
        let lines = wrap_lines(text, 42, 20);
        assert!(lines.len() >= 2);
        for l in &lines {
            assert!(l.chars().count() <= 42, "{l:?}");
            assert!(!l.starts_with(' ') && !l.ends_with(' '));
        }
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn wrap_short_text_is_one_line() {
        assert_eq!(wrap_lines("  hello   world ", 42, 20), vec!["hello world"]);
        assert!(wrap_lines("   ", 42, 20).is_empty());
    }

    #[test]
    fn wrap_long_word_is_hard_broken() {
        let word = "a".repeat(50);
        let lines = wrap_lines(&word, 42, 20);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 42);
        assert_eq!(lines[1].len(), 8);
    }

    #[test]
    fn wrap_cjk_breaks_anywhere_at_20() {
        let text: String = "这是一个用于测试中文字幕自动换行功能的句子它应该被分成两行".into();
        let lines = wrap_lines(&text, 42, 20);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].chars().count(), 20);
        assert_eq!(lines.concat(), text);
    }

    #[test]
    fn wrap_cjk_closing_punct_does_not_start_line() {
        let text = format!("{}。后面", "字".repeat(20));
        let lines = wrap_lines(&text, 42, 20);
        assert!(!lines[1].starts_with('。'), "{lines:?}");
        assert!(lines[0].ends_with('。'));
    }

    #[test]
    fn wrap_mixed_text_respects_weighted_width() {
        let text = "我们今天用 Lumora 录制一段屏幕演示视频然后导出 SRT 字幕文件给团队";
        let lines = wrap_lines(text, 42, 20);
        assert!(lines.len() >= 2);
        let w = Width::new(42, 20);
        for l in &lines {
            assert!(w.of(l) <= w.limit, "{l:?} too wide");
        }
        // No characters lost (spaces aside).
        let strip = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(strip(&lines.concat()), strip(text));
    }

    #[test]
    fn normalize_trims_and_drops_empty() {
        let segs =
            vec![Segment::new(0.0, 1.0, "  hello  "), Segment::new(1.0, 2.0, "   "), Segment::new(2.0, 3.0, "world")];
        let n = normalize(&segs);
        assert_eq!(n, vec![Segment::new(0.0, 1.0, "hello"), Segment::new(2.0, 3.0, "world")]);
    }

    #[test]
    fn normalize_merges_short_segments() {
        let segs = vec![
            Segment::new(0.0, 0.3, "Hi"),
            Segment::new(0.3, 2.0, "there friend"),
            Segment::new(2.0, 2.4, "ok"),
            Segment::new(3.0, 4.0, "好的"),
            Segment::new(4.0, 4.2, "吗"),
        ];
        let n = normalize(&segs);
        assert_eq!(n, vec![Segment::new(0.0, 2.4, "Hi there friend ok"), Segment::new(3.0, 4.2, "好的吗"),]);
    }

    #[test]
    fn normalize_keeps_single_short_segment() {
        let n = normalize(&[Segment::new(1.0, 1.2, "x")]);
        assert_eq!(n, vec![Segment::new(1.0, 1.2, "x")]);
    }

    #[test]
    fn normalize_splits_long_segments_proportionally() {
        let text = "one two three four five six seven eight nine ten eleven twelve \
                    thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty \
                    twenty-one twenty-two twenty-three twenty-four";
        let n = normalize(&[Segment::new(10.0, 20.0, text)]);
        assert!(n.len() >= 2);
        assert!((n[0].start - 10.0).abs() < 1e-9);
        assert!((n.last().unwrap().end - 20.0).abs() < 1e-9);
        for pair in n.windows(2) {
            assert!((pair[0].end - pair[1].start).abs() < 1e-9);
        }
        for s in &n {
            assert!(wrap_lines(&s.text, MAX_LATIN, MAX_CJK).len() <= MAX_LINES);
        }
        // Proportional: duration / chars roughly constant.
        let rate = |s: &Segment| s.duration() / s.text.chars().filter(|c| !c.is_whitespace()).count() as f64;
        let r0 = rate(&n[0]);
        for s in &n {
            assert!((rate(s) - r0).abs() < 1e-9);
        }
        let words: Vec<&str> = n.iter().flat_map(|s| s.text.split(' ')).collect();
        assert_eq!(words.join(" "), text.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    #[test]
    fn normalize_splits_long_cjk_segment() {
        let text = "字".repeat(100);
        let n = normalize(&[Segment::new(0.0, 10.0, text.clone())]);
        assert_eq!(n.len(), 3); // 40 + 40 + 20 chars
        assert!((n[0].end - 4.0).abs() < 1e-9);
        assert!((n[1].end - 8.0).abs() < 1e-9);
        assert_eq!(n.iter().map(|s| s.text.as_str()).collect::<String>(), text);
    }

    #[test]
    fn sidecar_path_next_to_movie() {
        assert_eq!(sidecar_path(Path::new("/tmp/rec/Demo 1.mov")), PathBuf::from("/tmp/rec/Demo 1.transcript.json"));
        assert_eq!(sidecar_path(Path::new("clip.v2.mp4")), PathBuf::from("clip.v2.transcript.json"));
    }

    #[test]
    fn json_round_trip() {
        let t = Transcript::new(
            "zh-Hans",
            "whisper-small",
            vec![Segment::new(0.0, 1.5, "你好"), Segment::new(1.5, 3.25, "hello")],
        );
        let json = t.to_json().unwrap();
        assert!(json.contains("\"schema_version\": 1"));
        assert_eq!(Transcript::from_json(&json).unwrap(), t);
    }

    #[test]
    fn json_rejects_future_schema() {
        let json = r#"{"schema_version":2,"language":"en","engine":"x","segments":[]}"#;
        assert!(matches!(Transcript::from_json(json), Err(TranscriptError::UnsupportedVersion(2))));
        assert!(Transcript::from_json("not json").is_err());
    }

    #[test]
    fn json_summary_is_optional() {
        let mut t = Transcript::new("en", "x", vec![Segment::new(0.0, 1.0, "hi")]);
        assert!(!t.to_json().unwrap().contains("summary"));
        t.summary = Some(Summary { summary: "S".into(), chapters: vec![Chapter::new(0.0, "Intro")] });
        let json = t.to_json().unwrap();
        assert!(json.contains("\"schema_version\": 1") && json.contains("\"chapters\""));
        assert_eq!(Transcript::from_json(&json).unwrap(), t);
    }
}

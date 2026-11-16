//! Grouping recognized words into subtitle segments.
//!
//! Speech recognizers report one timed item per word (or per CJK
//! character run). [`group_words`] joins them into sentence-sized
//! [`Segment`]s: a segment ends after sentence punctuation, after clause
//! punctuation once it is already long, before a pause longer than
//! [`PAUSE_SECS`], or before it would grow past [`MAX_SEGMENT_SECS`].
//! Platform independent so it can be unit-tested anywhere.

use lumora_core::transcript::{is_cjk, Segment};

/// A silence longer than this (seconds) between two words starts a new segment.
pub const PAUSE_SECS: f64 = 0.8;
/// A segment never grows longer than this (seconds) unless one word is.
pub const MAX_SEGMENT_SECS: f64 = 12.0;
/// Clause punctuation (commas…) ends a segment once it is at least this long.
pub const CLAUSE_BREAK_SECS: f64 = 5.0;

/// One recognized word: start time and duration in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub start: f64,
    pub duration: f64,
    pub text: String,
}

impl Word {
    pub fn new(start: f64, duration: f64, text: impl Into<String>) -> Self {
        Self { start, duration, text: text.into() }
    }

    fn end(&self) -> f64 {
        self.start + self.duration.max(0.0)
    }
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', '”', '’', ')', '）', '」', '』'])
        .chars()
        .next_back()
        .is_some_and(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '…'))
}

fn ends_clause(text: &str) -> bool {
    text.chars().next_back().is_some_and(|c| matches!(c, ',' | ';' | ':' | '，' | '、' | '；' | '：'))
}

/// Join two pieces of text: no space at a CJK seam or before punctuation.
fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    let first = b.chars().next();
    let no_space = a.chars().next_back().is_some_and(is_cjk)
        || first.is_some_and(is_cjk)
        || first.is_some_and(|c| matches!(c, ',' | '.' | '!' | '?' | ';' | ':' | '，' | '。' | '！' | '？' | '、'));
    if no_space {
        format!("{a}{b}")
    } else {
        format!("{a} {b}")
    }
}

/// Group `words` (in time order) into segments, adding `offset` seconds to
/// every time (for audio transcribed in chunks).
pub fn group_words(words: &[Word], offset: f64) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut cur: Option<Segment> = None;
    for w in words {
        let text = w.text.trim();
        if text.is_empty() {
            continue;
        }
        let start = w.start.max(0.0) + offset;
        let end = w.end().max(w.start.max(0.0)) + offset;
        if let Some(c) = cur.as_mut() {
            let pause = start - c.end > PAUSE_SECS;
            let too_long = end - c.start > MAX_SEGMENT_SECS;
            if pause || too_long {
                out.extend(cur.take());
            }
        }
        let c = cur.get_or_insert_with(|| Segment::new(start, end, String::new()));
        c.text = join(&c.text, text);
        c.end = c.end.max(end);
        if ends_sentence(text) || (ends_clause(text) && c.duration() >= CLAUSE_BREAK_SECS) {
            out.extend(cur.take());
        }
    }
    out.extend(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: f64, dur: f64, t: &str) -> Word {
        Word::new(start, dur, t)
    }

    #[test]
    fn empty_input() {
        assert!(group_words(&[], 0.0).is_empty());
        assert!(group_words(&[w(0.0, 1.0, "  ")], 0.0).is_empty());
    }

    #[test]
    fn breaks_on_sentence_punctuation() {
        let words = [w(0.0, 0.3, "Hello"), w(0.35, 0.3, "world."), w(0.7, 0.3, "Next"), w(1.05, 0.4, "one?")];
        let s = group_words(&words, 0.0);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "Hello world.");
        assert!((s[0].end - 0.65).abs() < 1e-9);
        assert_eq!(s[1].text, "Next one?");
        assert!((s[1].start - 0.7).abs() < 1e-9);
    }

    #[test]
    fn breaks_on_pause() {
        let words = [w(0.0, 0.5, "one"), w(0.6, 0.5, "two"), w(2.0, 0.5, "three")];
        let s = group_words(&words, 0.0);
        assert_eq!(s.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["one two", "three"]);
    }

    #[test]
    fn caps_segment_length() {
        let words: Vec<Word> = (0..40).map(|i| w(f64::from(i) * 0.5, 0.45, "la")).collect();
        let s = group_words(&words, 0.0);
        assert!(s.len() >= 2);
        assert!(s.iter().all(|s| s.duration() <= MAX_SEGMENT_SECS + 1e-9));
        assert_eq!(s.iter().map(|s| s.text.split(' ').count()).sum::<usize>(), 40);
    }

    #[test]
    fn clause_break_only_when_long() {
        let short = [w(0.0, 0.5, "Yes,"), w(0.6, 0.5, "sure")];
        assert_eq!(group_words(&short, 0.0).len(), 1);
        let long: Vec<Word> = (0..12)
            .map(|i| w(f64::from(i) * 0.5, 0.45, if i == 11 { "end," } else { "word" }))
            .chain([w(6.0, 0.4, "more")])
            .collect();
        let s = group_words(&long, 0.0);
        assert_eq!(s.len(), 2);
        assert!(s[0].text.ends_with("end,"));
    }

    #[test]
    fn cjk_joins_without_spaces_and_offsets() {
        let words = [w(0.0, 0.3, "你好"), w(0.3, 0.3, "世界"), w(0.6, 0.2, "。"), w(1.0, 0.3, "OK")];
        let s = group_words(&words, 50.0);
        assert_eq!(s[0].text, "你好世界。");
        assert!((s[0].start - 50.0).abs() < 1e-9);
        assert_eq!(s[1], Segment::new(51.0, 51.3, "OK"));
    }
}

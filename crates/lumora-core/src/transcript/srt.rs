//! SRT / WebVTT export and SRT import.

use super::{join_text, wrap_lines, Segment, MAX_CJK, MAX_LATIN, MAX_LINES};

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn format_time(secs: f64, ms_sep: char) -> String {
    let ms = if secs.is_finite() && secs > 0.0 { (secs * 1000.0).round() as u64 } else { 0 };
    let h = ms / 3_600_000;
    let m = ms / 60_000 % 60;
    let s = ms / 1000 % 60;
    let frac = ms % 1000;
    format!("{h:02}:{m:02}:{s:02}{ms_sep}{frac:03}")
}

/// Format seconds as an SRT timestamp, `HH:MM:SS,mmm` (rounded to the
/// nearest millisecond; negative or non-finite times become zero).
pub fn format_srt_time(secs: f64) -> String {
    format_time(secs, ',')
}

/// Format seconds as a WebVTT timestamp, `HH:MM:SS.mmm`.
pub fn format_vtt_time(secs: f64) -> String {
    format_time(secs, '.')
}

/// Wrapped cue lines, at most [`MAX_LINES`]; overflow is folded into the
/// last line so no text is lost (call [`super::normalize`] first to avoid it).
fn cue_lines(text: &str) -> Vec<String> {
    let mut lines = wrap_lines(text, MAX_LATIN, MAX_CJK);
    if lines.len() > MAX_LINES {
        let tail = lines.split_off(MAX_LINES - 1).iter().fold(String::new(), |acc, l| join_text(&acc, l));
        lines.push(tail);
    }
    lines
}

/// Render segments as SubRip (`.srt`). Empty segments are skipped; cues are
/// numbered from 1 and wrapped to at most two lines.
pub fn to_srt(segments: &[Segment]) -> String {
    let mut out = String::new();
    let mut index = 0usize;
    for s in segments {
        let lines = cue_lines(&s.text);
        if lines.is_empty() {
            continue;
        }
        index += 1;
        out.push_str(&format!(
            "{index}\n{} --> {}\n{}\n\n",
            format_srt_time(s.start),
            format_srt_time(s.end),
            lines.join("\n")
        ));
    }
    out
}

fn escape_vtt(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Render segments as WebVTT (`.vtt`), with the `WEBVTT` header.
pub fn to_vtt(segments: &[Segment]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for s in segments {
        let lines = cue_lines(&s.text);
        if lines.is_empty() {
            continue;
        }
        let body: Vec<String> = lines.iter().map(|l| escape_vtt(l)).collect();
        out.push_str(&format!("{} --> {}\n{}\n\n", format_vtt_time(s.start), format_vtt_time(s.end), body.join("\n")));
    }
    out
}

/// Parse an SRT/VTT timestamp: `HH:MM:SS,mmm`, `HH:MM:SS.mmm` or `MM:SS.mmm`.
///
/// # Errors
/// A descriptive message if the timestamp is malformed.
pub fn parse_timestamp(ts: &str) -> Result<f64, String> {
    let ts = ts.trim();
    let bad = || format!("invalid timestamp {ts:?}");
    let parts: Vec<&str> = ts.split(':').collect();
    let (h, m, rest) = match parts.as_slice() {
        [h, m, rest] => (*h, *m, *rest),
        [m, rest] => ("0", *m, *rest),
        _ => return Err(bad()),
    };
    let (s, frac) = rest.split_once([',', '.']).unwrap_or((rest, "0"));
    let num = |x: &str| -> Result<u64, String> {
        if x.is_empty() || !x.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        x.parse::<u64>().map_err(|_| bad())
    };
    let (h, m, s) = (num(h)?, num(m)?, num(s)?);
    if m >= 60 || s >= 60 || frac.len() > 3 {
        return Err(bad());
    }
    let frac_ms = num(frac)? * 10u64.pow(3 - u32::try_from(frac.len()).map_err(|_| bad())?);
    #[allow(clippy::cast_precision_loss)]
    let total = ((h * 3600 + m * 60 + s) * 1000 + frac_ms) as f64 / 1000.0;
    Ok(total)
}

/// Parse SubRip text into segments. Tolerates CRLF / CR line endings, a BOM,
/// missing or extra blank lines, missing cue numbers, `.` as the millisecond
/// separator and cue settings after the end time. Cue text lines are joined
/// into one string (with a space between Latin lines). A text-only block
/// (e.g. after a stray blank line inside a cue) is appended to the previous
/// cue.
///
/// # Errors
/// A message naming the line of a malformed timing line, or text before the
/// first cue.
pub fn parse_srt(input: &str) -> Result<Vec<Segment>, String> {
    let text = input.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut segments: Vec<Segment> = Vec::new();
    let mut block: Vec<(usize, &str)> = Vec::new();
    let mut blocks: Vec<Vec<(usize, &str)>> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
        } else {
            block.push((i + 1, line));
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }

    for block in blocks {
        let Some(pos) = block.iter().position(|(_, l)| l.contains("-->")) else {
            let extra = block.iter().fold(String::new(), |acc, (_, l)| join_text(&acc, l.trim()));
            match segments.last_mut() {
                Some(prev) => prev.text = join_text(&prev.text, &extra),
                None if block.len() == 1 && block[0].1.trim() == "WEBVTT" => {}
                None => return Err(format!("line {}: text before first cue", block[0].0)),
            }
            continue;
        };
        let (line_no, timing) = block[pos];
        let (a, b) = timing.split_once("-->").ok_or_else(|| format!("line {line_no}: missing '-->'"))?;
        let b = b.split_whitespace().next().unwrap_or("");
        let start = parse_timestamp(a).map_err(|e| format!("line {line_no}: {e}"))?;
        let end = parse_timestamp(b).map_err(|e| format!("line {line_no}: {e}"))?;
        let body = block[pos + 1..].iter().fold(String::new(), |acc, (_, l)| join_text(&acc, l.trim()));
        segments.push(Segment::new(start, end, body));
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::super::normalize;
    use super::*;

    #[test]
    fn srt_timestamps() {
        assert_eq!(format_srt_time(0.0), "00:00:00,000");
        assert_eq!(format_srt_time(1.5), "00:00:01,500");
        assert_eq!(format_srt_time(61.25), "00:01:01,250");
        assert_eq!(format_srt_time(3725.007), "01:02:05,007");
        assert_eq!(format_srt_time(36_000.0), "10:00:00,000");
        assert_eq!(format_srt_time(-3.0), "00:00:00,000");
        assert_eq!(format_srt_time(f64::NAN), "00:00:00,000");
    }

    #[test]
    fn timestamp_rounding_carries() {
        assert_eq!(format_srt_time(1.0004), "00:00:01,000");
        assert_eq!(format_srt_time(1.0006), "00:00:01,001");
        assert_eq!(format_srt_time(59.9996), "00:01:00,000");
        assert_eq!(format_srt_time(3599.9999), "01:00:00,000");
        assert_eq!(format_vtt_time(3599.9999), "01:00:00.000");
    }

    #[test]
    fn vtt_timestamps_use_dot() {
        assert_eq!(format_vtt_time(4000.123), "01:06:40.123");
    }

    #[test]
    fn parse_timestamps() {
        assert!((parse_timestamp("01:02:05,007").unwrap() - 3725.007).abs() < 1e-9);
        assert!((parse_timestamp("00:00:01.5").unwrap() - 1.5).abs() < 1e-9);
        assert!((parse_timestamp("02:03.250").unwrap() - 123.25).abs() < 1e-9);
        assert!(parse_timestamp("1:2").is_ok());
        assert!(parse_timestamp("aa:00:00,000").is_err());
        assert!(parse_timestamp("00:61:00,000").is_err());
        assert!(parse_timestamp("00:00:00,0000").is_err());
    }

    #[test]
    fn srt_output_format() {
        let segs = vec![
            Segment::new(0.0, 1.5, "Hello world"),
            Segment::new(1.5, 2.0, ""),
            Segment::new(3661.0, 3662.25, "第二句"),
        ];
        assert_eq!(
            to_srt(&segs),
            "1\n00:00:00,000 --> 00:00:01,500\nHello world\n\n\
             2\n01:01:01,000 --> 01:01:02,250\n第二句\n\n"
        );
    }

    #[test]
    fn srt_cues_have_at_most_two_lines() {
        let long = "word ".repeat(40);
        let out = to_srt(&[Segment::new(0.0, 5.0, long.trim())]);
        let body: Vec<&str> = out.lines().skip(2).take_while(|l| !l.is_empty()).collect();
        assert_eq!(body.len(), 2);
    }

    #[test]
    fn vtt_header_and_cues() {
        let out = to_vtt(&[Segment::new(1.0, 2.0, "a <b> & c")]);
        assert!(out.starts_with("WEBVTT\n\n"));
        assert!(out.contains("00:00:01.000 --> 00:00:02.000\na &lt;b&gt; &amp; c\n"));
        assert_eq!(to_vtt(&[]), "WEBVTT\n\n");
    }

    #[test]
    fn srt_round_trip() {
        let segs = normalize(&[
            Segment::new(0.0, 2.5, "The quick brown fox jumps over the lazy dog, again and again and again."),
            Segment::new(2.5, 5.0, "这是一个用于测试中文字幕自动换行功能的句子"),
            Segment::new(3700.123, 3702.0, "Mixed 中文 and English"),
        ]);
        let srt = to_srt(&segs);
        let back = parse_srt(&srt).unwrap();
        assert_eq!(back.len(), segs.len());
        for (a, b) in segs.iter().zip(&back) {
            assert!((a.start - b.start).abs() < 5e-4);
            assert!((a.end - b.end).abs() < 5e-4);
            assert_eq!(a.text, b.text);
        }
    }

    #[test]
    fn parse_tolerates_crlf_and_blank_lines() {
        let input = "\u{feff}\r\n\r\n1\r\n00:00:01,000 --> 00:00:02,000\r\nHello\r\nthere\r\n\r\n\r\n\r\n\
                     2\r\n00:00:03.000 --> 00:00:04,500 X1:0\r\n你好\r\n世界\r\n\r\n\
                     00:00:05,000 --> 00:00:06,000\r\nno index\r\n\r\ntrailing text\r\n";
        let segs = parse_srt(input).unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::new(1.0, 2.0, "Hello there"),
                Segment::new(3.0, 4.5, "你好世界"),
                Segment::new(5.0, 6.0, "no index trailing text"),
            ]
        );
    }

    #[test]
    fn parse_reports_errors() {
        assert!(parse_srt("1\n00:00:01,000 --> nope\nx\n").is_err());
        assert!(parse_srt("just text\n").is_err());
        assert_eq!(parse_srt("").unwrap(), vec![]);
    }
}

//! Tutorial export: Markdown (images linked) and a single self-contained
//! HTML page (images inlined as base64, light and dark styling).

use std::path::Path;

use super::{Step, Tutorial};
use crate::ai::png_data_url;
use crate::i18n::{format_args_into, Language, Msg};

fn heading(step: &Step, lang: Language) -> String {
    let title = step.title.trim();
    if title.is_empty() {
        format_args_into(Msg::TutorialStepN.text(lang), &[&step.index])
    } else {
        format_args_into(Msg::TutorialHeading.text(lang), &[&step.index, &one_line(title)])
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A Markdown link target for `path`: wrapped in `<…>` when it has
/// spaces or brackets (CommonMark), with `<`/`>` percent-encoded.
pub fn link_target(path: &str) -> String {
    let p = path.replace('\\', "/").replace('<', "%3C").replace('>', "%3E");
    if p.contains([' ', '(', ')']) {
        format!("<{p}>")
    } else {
        p
    }
}

/// Where a Markdown file in `md_dir` should point for a step image stored
/// at `rel` relative to `base_dir` (the `.steps.json` folder): the relative
/// path when both are in the same folder, else the absolute path.
pub fn image_path_for(md_dir: Option<&Path>, base_dir: &Path, rel: &str) -> String {
    if md_dir == Some(base_dir) {
        rel.to_string()
    } else {
        base_dir.join(rel).to_string_lossy().into_owned()
    }
}

/// The tutorial as Markdown. `image` returns each step's link target path
/// (see [`image_path_for`]) or `None` to skip the image.
pub fn to_markdown(t: &Tutorial, lang: Language, image: impl Fn(&Step) -> Option<String>) -> String {
    let mut out = String::new();
    let title = one_line(&t.title);
    if !title.is_empty() {
        out.push_str(&format!("# {title}\n\n"));
    }
    for s in &t.steps {
        out.push_str(&format!("## {}\n\n", heading(s, lang)));
        if let Some(path) = image(s) {
            let alt = format_args_into(Msg::TutorialStepN.text(lang), &[&s.index]);
            out.push_str(&format!("![{alt}]({})\n\n", link_target(&path)));
        }
        let detail = s.detail.trim();
        if !detail.is_empty() {
            out.push_str(detail);
            out.push_str("\n\n");
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

/// Escape text for HTML element content and attribute values.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const CSS: &str = r#":root{color-scheme:light dark;--bg:#f6f6f8;--card:#fff;--text:#1d1d1f;--muted:#6e6e73;--accent:#5e5ce6;--line:rgba(0,0,0,.08)}
@media (prefers-color-scheme:dark){:root{--bg:#161618;--card:#232326;--text:#f5f5f7;--muted:#a1a1a6;--accent:#7d7aff;--line:rgba(255,255,255,.1)}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--text);font:16px/1.55 -apple-system,BlinkMacSystemFont,"Helvetica Neue","PingFang SC","Hiragino Sans GB",sans-serif;-webkit-font-smoothing:antialiased}
main{max-width:860px;margin:0 auto;padding:48px 20px 64px}
h1{font-size:30px;letter-spacing:-.02em;margin:0 0 32px}
ol{list-style:none;margin:0;padding:0;counter-reset:step}
li{background:var(--card);border:1px solid var(--line);border-radius:16px;padding:20px 22px 22px;margin:0 0 20px;box-shadow:0 1px 3px rgba(0,0,0,.04)}
.head{display:flex;gap:12px;align-items:baseline}
.num{flex:none;min-width:28px;height:28px;border-radius:14px;background:var(--accent);color:#fff;font-weight:600;font-size:14px;display:inline-flex;align-items:center;justify-content:center;padding:0 8px}
h2{font-size:19px;margin:0;line-height:1.35}
p{margin:10px 0 0 40px;color:var(--muted);white-space:pre-wrap}
img{display:block;width:100%;height:auto;margin:16px 0 0;border-radius:10px;border:1px solid var(--line)}
footer{color:var(--muted);font-size:13px;text-align:center;margin-top:32px}
@media print{body{background:#fff}li{break-inside:avoid;box-shadow:none}}"#;

/// The tutorial as one HTML file. `image` returns each step's PNG bytes
/// (inlined as a data URL) or `None`.
pub fn to_html(t: &Tutorial, lang: Language, image: impl Fn(&Step) -> Option<Vec<u8>>) -> String {
    let title = one_line(&t.title);
    let title = if title.is_empty() { Msg::TutorialWindowTitle.text(lang).to_string() } else { title };
    let mut out = String::new();
    out.push_str(&format!(
        "<!DOCTYPE html>\n<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta name=\"generator\" content=\"Lumora\">\n<title>{}</title>\n<style>\n{CSS}\n</style>\n</head>\n<body>\n<main>\n<h1>{}</h1>\n<ol>\n",
        lang.tag(),
        escape_html(&title),
        escape_html(&title)
    ));
    for s in &t.steps {
        let step_title = one_line(s.title.trim());
        let step_title = if step_title.is_empty() {
            format_args_into(Msg::TutorialStepN.text(lang), &[&s.index])
        } else {
            step_title
        };
        out.push_str(&format!(
            "<li id=\"step-{0}\">\n<div class=\"head\"><span class=\"num\">{0}</span><h2>{1}</h2></div>\n",
            s.index,
            escape_html(&step_title)
        ));
        let detail = s.detail.trim();
        if !detail.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", escape_html(detail)));
        }
        if let Some(png) = image(s) {
            let alt = format_args_into(Msg::TutorialStepN.text(lang), &[&s.index]);
            out.push_str(&format!("<img alt=\"{}\" src=\"{}\">\n", escape_html(&alt), png_data_url(&png)));
        }
        out.push_str("</li>\n");
    }
    out.push_str(&format!(
        "</ol>\n<footer>{}</footer>\n</main>\n</body>\n</html>\n",
        escape_html(Msg::TutorialFooter.text(lang))
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_log::MouseButton;
    use std::path::PathBuf;

    fn step(index: usize, title: &str, detail: &str) -> Step {
        Step {
            index,
            t: index as f64,
            frame_time: index as f64 + 0.12,
            click: [10.0, 10.0],
            button: MouseButton::Left,
            double_click: false,
            app: None,
            bundle_id: None,
            window_title: None,
            ocr_hint: None,
            title: title.into(),
            detail: detail.into(),
            image: Some(format!("Rec 1.tutorial/step-{index:02}.png")),
            crop: None,
        }
    }

    fn tutorial() -> Tutorial {
        Tutorial {
            schema_version: 1,
            movie: "Rec 1.mp4".into(),
            title: "Rec 1".into(),
            video_width: 100,
            video_height: 100,
            scale: 2.0,
            ai_written: false,
            steps: vec![step(1, "Click “Save”", "Saves the <file>."), step(2, "  ", "")],
        }
    }

    #[test]
    fn markdown_layout() {
        let t = tutorial();
        let md = to_markdown(&t, Language::En, |s| s.image.clone());
        let expected = "# Rec 1\n\n## Step 1: Click “Save”\n\n![Step 1](<Rec 1.tutorial/step-01.png>)\n\nSaves the <file>.\n\n## Step 2\n\n![Step 2](<Rec 1.tutorial/step-02.png>)\n";
        assert_eq!(md, expected);
        let zh = to_markdown(&t, Language::ZhHans, |_| None);
        assert!(zh.contains("## 第 1 步：Click “Save”\n"));
        assert!(zh.ends_with("## 第 2 步\n"));
    }

    #[test]
    fn link_targets_and_paths() {
        assert_eq!(link_target("a/b.png"), "a/b.png");
        assert_eq!(link_target("a b/c(1).png"), "<a b/c(1).png>");
        assert_eq!(link_target("x<y>.png"), "x%3Cy%3E.png");
        let base = PathBuf::from("/m");
        assert_eq!(image_path_for(Some(Path::new("/m")), &base, "R.tutorial/s.png"), "R.tutorial/s.png");
        assert_eq!(image_path_for(Some(Path::new("/other")), &base, "R.tutorial/s.png"), "/m/R.tutorial/s.png");
        assert_eq!(image_path_for(None, &base, "R.tutorial/s.png"), "/m/R.tutorial/s.png");
    }

    #[test]
    fn html_is_self_contained_and_escaped() {
        let t = tutorial();
        let html = to_html(&t, Language::En, |s| (s.index == 1).then(|| vec![0x89, b'P', b'N', b'G']));
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("prefers-color-scheme:dark"));
        assert!(html.contains("Saves the &lt;file&gt;."));
        assert!(html.contains("src=\"data:image/png;base64,iVBORw==\""));
        assert_eq!(html.matches("<img ").count(), 1);
        assert!(html.contains("<h2>Step 2</h2>"));
        assert!(!html.contains("src=\"http"), "no external resources");
        let zh = to_html(&t, Language::ZhHans, |_| None);
        assert!(zh.contains("<html lang=\"zh-Hans\">"));
        assert_eq!(escape_html("a&\"'"), "a&amp;&quot;&#39;");
    }
}

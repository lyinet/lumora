//! Smart redaction (v0.5). The editor emits
//! [`EditorAction::SmartRedact`](lumora_ui::events::EditorAction::SmartRedact);
//! here the app runs Vision OCR and face detection on a background queue,
//! maps detector hits onto the image with [`lumora_core::redact`], and —
//! only when the user turned on "AI check" and AI writing is enabled —
//! sends the recognised text lines (never the image) to the model for a
//! second opinion. The rectangles go back to the editor, which adds them
//! as one undoable group. Nothing recognised is logged.

use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_ai::{AiError, AiSettings, KeyStore, Keychain};
use lumora_capture::CaptureError;
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, trf, Msg};
use lumora_core::redact::{self, Finding, RedactSettings};
use lumora_ui::events::SmartRedactRequest;
use lumora_ui::toast::ToastKind;
use objc2_core_graphics::CGImage;

use crate::app::{on_main, App};
use crate::capture_flow::error_text;

/// What a detection run found.
pub struct Detection {
    /// Redaction rectangles in points of the image.
    pub rects: Vec<Rect>,
    /// The AI check was requested but failed (local results only).
    pub ai_failed: bool,
}

/// The AI settings and key for the AI check, or `Err(())` when it was
/// asked for but cannot run (AI off, no key…). `Ok(None)` = not asked.
pub fn ai_for_check(settings: &lumora_core::settings::Settings) -> Result<Option<(AiSettings, Option<String>)>, ()> {
    if !settings.redact.ai_check {
        return Ok(None);
    }
    let mut cfg = settings.ai.clone();
    cfg.validate();
    if !cfg.enabled {
        return Err(());
    }
    let key =
        Keychain.get(cfg.provider).map_err(|e| log::warn!("reading the API key for the AI check failed: {e:?}"))?;
    cfg.check_ready(key.as_deref()).map_err(|_| ())?;
    Ok(Some((cfg, key)))
}

/// Blocking detection over `image` (call off the main thread).
pub fn detect_sync(
    image: &CGImage,
    scale: f64,
    settings: &RedactSettings,
    ai: Option<(AiSettings, Option<String>)>,
) -> Result<Detection, CaptureError> {
    let (w, h) = lumora_capture::image::pixel_size(image);
    if w == 0 || h == 0 {
        return Err(CaptureError::Empty);
    }
    let text_wanted = redact::Category::TEXT.iter().any(|&c| settings.enabled(c)) || ai.is_some();
    let boxes = if text_wanted { lumora_capture::ocr::recognize_boxes_sync(image) } else { Ok(Vec::new()) };
    let faces = if settings.faces { lumora_capture::faces::detect_faces_sync(image) } else { Ok(Vec::new()) };
    let (boxes, faces) = match (boxes, faces) {
        (Err(e), Err(_)) => return Err(e),
        (b, f) => (
            b.unwrap_or_else(|e| {
                log::warn!("OCR for smart redaction failed: {e}");
                Vec::new()
            }),
            f.unwrap_or_else(|e| {
                log::warn!("face detection failed: {e}");
                Vec::new()
            }),
        ),
    };
    let lines = redact::ocr_lines(&boxes);
    let mut findings: Vec<Finding> = redact::find_in_lines(&lines, settings);
    findings.extend(redact::face_findings(&faces));
    let mut ai_failed = false;
    if let Some((cfg, key)) = ai {
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        let run = || -> Result<Vec<Finding>, AiError> {
            let req = redact::ai::build_request(&cfg, &texts)?;
            let reply = lumora_ai::complete(&cfg, key.as_deref(), &req, &AtomicBool::new(false), |_| {})?;
            let flagged = redact::ai::parse_response(&reply, lines.len());
            Ok(redact::findings_for_lines(&lines, &flagged, &findings))
        };
        match run() {
            Ok(extra) => findings.extend(extra),
            Err(AiError::EmptyInput) => {}
            Err(e) => {
                log::info!("AI check for smart redaction failed: {e:?}");
                ai_failed = true;
            }
        }
    }
    let canvas = Rect::new(0.0, 0.0, w as f64 / scale, h as f64 / scale);
    log::info!("smart redaction: {} findings", findings.len());
    Ok(Detection { rects: redact::redaction_rects(&findings, scale, canvas), ai_failed })
}

impl App {
    /// "Smart Redact" in editor `editor`.
    pub fn on_smart_redact(self: &Rc<Self>, editor: u64, req: SmartRedactRequest) {
        let settings = self.settings.borrow().clone();
        let (ai, ai_unavailable) = match ai_for_check(&settings) {
            Ok(ai) => (ai, false),
            Err(()) => (None, true),
        };
        let SmartRedactRequest { request, base, scale } = req;
        let redact_settings = settings.redact;
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let result = detect_sync(&base, scale, &redact_settings, ai).map(|mut d| {
                    d.ai_failed |= ai_unavailable;
                    d
                });
                let mode = redact_settings.mode;
                on_main(move |app| app.smart_redact_done(editor, request, result, mode));
            },
        );
    }

    fn smart_redact_done(
        self: &Rc<Self>,
        editor: u64,
        request: u64,
        result: Result<Detection, CaptureError>,
        mode: lumora_core::annotate::RedactMode,
    ) {
        let editors = self.editors.borrow();
        let Some(e) = editors.get(&editor) else { return };
        match result {
            Ok(d) => {
                let Some(n) = e.smart_redact_finished(request, &d.rects, mode) else { return };
                drop(editors);
                let ai_note = d.ai_failed.then(|| tr(Msg::RedactAiFailed));
                if n == 0 {
                    let detail = ai_note.unwrap_or(tr(Msg::ToastRedactNothingDetail));
                    self.notify_text(ToastKind::Warning, tr(Msg::ToastRedactNothing), Some(detail));
                } else {
                    let title =
                        if n == 1 { tr(Msg::ToastRedactedOne).to_string() } else { trf(Msg::ToastRedacted, &[&n]) };
                    let detail = match ai_note {
                        Some(a) => format!("{a} {}", tr(Msg::ToastRedactUndo)),
                        None => tr(Msg::ToastRedactUndo).to_string(),
                    };
                    self.notify_text(ToastKind::Success, &title, Some(&detail));
                }
            }
            Err(err) => {
                if e.smart_redact_finished(request, &[], mode).is_none() {
                    return;
                }
                drop(editors);
                log::error!("smart redaction failed: {err}");
                self.notify_text(ToastKind::Error, tr(Msg::ToastRedactFailed), Some(&error_text(&err)));
            }
        }
    }
}

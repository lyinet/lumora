//! Screenshot flows: area / window (via the overlay) and fullscreen.
//!
//! Area capture freezes every display first (one `SCScreenshotManager` grab
//! per display, in parallel), shows the overlay over those frozen frames,
//! and crops the final image from the frozen frame — so what you select is
//! exactly what you get, even for video or animations. Window capture uses
//! a desktop-independent window filter instead, which captures the window
//! cleanly (with its shadow) even if it is partially covered.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_capture::content::ShareableContent;
use lumora_capture::image::StitchPart;
use lumora_capture::screenshot::{self, Shot, ShotOptions, ShotTarget};
use lumora_capture::CaptureError;
use lumora_core::annotate::{Document, Shape, Style};
use lumora_core::filename::{render, unique_path, CaptureKind};
use lumora_core::geometry::{Rect, Size};
use lumora_core::selection::Target;
use lumora_core::snap::WindowList;
use lumora_core::stitch::{self, Source};
use lumora_ui::events::{FrozenDisplay, OverlayMode};
use lumora_ui::overlay::{OverlayConfig, OverlayController};
use lumora_ui::quick_access::CardContent;
use lumora_ui::toast::ToastKind;
use lumora_ui::{shutter, util};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;

use lumora_core::i18n::{tr, trf, Msg};

use crate::app::{on_main, App, CaptureRecord, Editable, Mode, Notice};
use crate::system;

/// Encode `image` to PNG once, write it to `path` and return the bytes (for
/// the `.lumora` document that embeds the same PNG).
fn write_png_shared(
    image: &CGImage,
    path: &std::path::Path,
    scale: f64,
) -> Result<std::sync::Arc<Vec<u8>>, CaptureError> {
    let err = |reason: String| CaptureError::Write { path: path.display().to_string(), reason };
    let png = lumora_capture::image::png_bytes(image, scale).map_err(|e| err(e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| err(e.to_string()))?;
    }
    std::fs::write(path, &png).map_err(|e| err(e.to_string()))?;
    Ok(std::sync::Arc::new(png))
}

fn background() -> dispatch2::DispatchRetained<DispatchQueue> {
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated))
}

impl App {
    fn shot_options(&self) -> ShotOptions {
        let s = self.settings.borrow();
        ShotOptions { show_cursor: false, retina: s.capture.retina, window_shadow: s.capture.window_shadow }
    }

    /// Start an overlay session (capture or record).
    pub fn begin_selection(self: &Rc<Self>, mode: OverlayMode, window_only: bool) {
        if !self.mode.borrow().is_idle() || !self.require_permission() {
            return;
        }
        self.set_mode(Mode::Preparing);
        ShareableContent::fetch(move |res| on_main(move |app| app.on_content_for_selection(res, mode, window_only)));
    }

    fn on_content_for_selection(
        self: &Rc<Self>,
        res: Result<ShareableContent, CaptureError>,
        mode: OverlayMode,
        window_only: bool,
    ) {
        let content = match res {
            Ok(c) => Arc::new(c),
            Err(e) => return self.fail(e),
        };
        if mode == OverlayMode::Record {
            return self.open_overlay(mode, window_only, content, Vec::new());
        }
        // Freeze every display in parallel; open the overlay when all land.
        let displays = content.displays();
        if displays.is_empty() {
            return self.fail(CaptureError::TargetGone("display"));
        }
        type Slot = (Vec<Option<(u32, Shot)>>, usize, Option<CaptureError>);
        let pending: Arc<Mutex<Slot>> =
            Arc::new(Mutex::new(((0..displays.len()).map(|_| None).collect(), displays.len(), None)));
        let opts = ShotOptions { window_shadow: false, ..self.shot_options() };
        for (i, d) in displays.iter().enumerate() {
            let pending = pending.clone();
            let content_for_done = content.clone();
            let id = d.id;
            screenshot::capture(&content, ShotTarget::Display(id), opts, move |res| {
                let Ok(mut slot) = pending.lock() else { return };
                match res {
                    Ok(shot) => slot.0[i] = Some((id, shot)),
                    Err(e) => slot.2 = Some(e),
                }
                slot.1 -= 1;
                if slot.1 == 0 {
                    let shots: Vec<(u32, Shot)> = slot.0.drain(..).flatten().collect();
                    let err = slot.2.take();
                    on_main(move |app| match (shots.is_empty(), err) {
                        (true, Some(e)) => app.fail(e),
                        _ => app.open_overlay(mode, window_only, content_for_done, shots),
                    });
                }
            });
        }
    }

    fn open_overlay(
        self: &Rc<Self>,
        mode: OverlayMode,
        window_only: bool,
        content: Arc<ShareableContent>,
        shots: Vec<(u32, Shot)>,
    ) {
        if !matches!(*self.mode.borrow(), Mode::Preparing) {
            return; // cancelled meanwhile
        }
        let frozen: Rc<Vec<FrozenDisplay>> = Rc::new(
            shots
                .into_iter()
                .map(|(id, s)| FrozenDisplay { id, frame: s.frame, scale: s.scale, image: s.image })
                .collect(),
        );
        let s = self.settings.borrow();
        let config = OverlayConfig {
            mode,
            show_magnifier: s.capture.show_magnifier,
            show_crosshair: s.capture.show_crosshair,
            window_picking: window_only || s.capture.window_picking,
            window_only,
        };
        drop(s);
        let windows = WindowList::new(content.windows());
        let overlay = OverlayController::show(self.mtm, config, frozen.clone(), windows, self.sink.clone());
        self.set_mode(Mode::Selecting { overlay, content, frozen });
    }

    pub fn on_overlay_confirmed(self: &Rc<Self>, mode: OverlayMode, target: Target) {
        if !matches!(*self.mode.borrow(), Mode::Selecting { .. }) {
            return;
        }
        let Mode::Selecting { overlay, content, frozen } = self.mode.replace(Mode::Preparing) else { return };
        drop(overlay);
        if mode == OverlayMode::Record {
            return self.on_record_target(&content, target);
        }
        if mode == OverlayMode::Text {
            self.set_mode(Mode::Idle);
            return match frozen_image(&frozen, &target) {
                Ok((image, _, _)) => self.recognize(image),
                Err(e) => self.fail_with(Msg::ToastTextFailed, e),
            };
        }
        match target {
            Target::Area(_) | Target::Display(_) => {
                self.set_mode(Mode::Idle);
                match frozen_image(&frozen, &target) {
                    Ok((img, scale, frame)) => self.finish_capture(img, scale, frame),
                    Err(e) => self.fail(e),
                }
            }
            Target::Window(w) => {
                screenshot::capture(&content, ShotTarget::Window(w.id), self.shot_options(), move |res| {
                    on_main(move |app| {
                        app.set_mode(Mode::Idle);
                        match res {
                            Ok(shot) => app.finish_capture(shot.image, shot.scale, shot.frame),
                            Err(e) => app.fail(e),
                        }
                    })
                });
            }
        }
    }

    /// OCR an image and put the text on the clipboard, with a toast and
    /// the Capture Text panel (Summarize, Translate, As Table, …).
    pub fn recognize(self: &Rc<Self>, image: CFRetained<CGImage>) {
        lumora_capture::ocr::recognize_text_boxes(image, |res| {
            let res = res.map(|boxes| (lumora_core::ocr::assemble(&boxes), boxes));
            on_main(move |app| match res {
                Ok((text, boxes)) if !text.trim().is_empty() => {
                    lumora_capture::clipboard::copy_text(&text);
                    let preview = lumora_core::ocr::preview(&text, 60);
                    app.notify(Notice::Success(Msg::ToastTextCopied), Some(preview));
                    app.show_text_result(&text, boxes);
                }
                Ok(_) => app.notify(Notice::Warning(Msg::ToastNoText), Some(tr(Msg::ToastNoTextDetail).to_string())),
                Err(e) => app.fail_with(Msg::ToastTextFailed, e),
            })
        });
    }

    pub fn capture_fullscreen(self: &Rc<Self>) {
        if !self.mode.borrow().is_idle() || !self.require_permission() {
            return;
        }
        self.set_mode(Mode::Preparing);
        let display_id = util::screen_under_mouse(self.mtm).map(|s| util::screen_id(&s)).unwrap_or(0);
        let opts = self.shot_options();
        ShareableContent::fetch(move |res| match res {
            Ok(content) => {
                let id =
                    if display_id == 0 { content.displays().first().map(|d| d.id).unwrap_or(0) } else { display_id };
                screenshot::capture(&content, ShotTarget::Display(id), opts, move |res| {
                    on_main(move |app| {
                        app.set_mode(Mode::Idle);
                        match res {
                            Ok(shot) => app.finish_capture(shot.image, shot.scale, shot.frame),
                            Err(e) => app.fail(e),
                        }
                    })
                });
            }
            Err(e) => on_main(move |app| app.fail(e)),
        });
    }

    /// Common tail of every screenshot. With "redact before copy / save"
    /// on, local smart-redaction detectors run first (off the main thread;
    /// never the AI check) and their redactions are applied as editable
    /// annotations.
    pub fn finish_capture(self: &Rc<Self>, image: CFRetained<CGImage>, scale: f64, frame: Rect) {
        let redact = self.settings.borrow().redact.clone();
        if !redact.auto_redact {
            return self.finish_capture_with(image, scale, frame, Vec::new());
        }
        background().exec_async(move || {
            let rects = match crate::redact_flow::detect_sync(&image, scale, &redact, None) {
                Ok(d) => d.rects,
                Err(e) => {
                    log::warn!("auto-redaction failed: {e}");
                    Vec::new()
                }
            };
            on_main(move |app| app.finish_capture_with(image, scale, frame, rects));
        });
    }

    /// Name, shutter, Quick Access card, background PNG encode, then
    /// clipboard. `redactions` (points) are flattened into what is shown,
    /// saved and copied, and kept as annotations for re-editing.
    fn finish_capture_with(
        self: &Rc<Self>,
        original: CFRetained<CGImage>,
        scale: f64,
        frame: Rect,
        redactions: Vec<Rect>,
    ) {
        let id = self.next_id();
        let s = self.settings.borrow().clone();
        let mut document = None;
        let mut image = original.clone();
        if !redactions.is_empty() {
            let (pw, ph) = lumora_capture::image::pixel_size(&original);
            let mut doc = Document::new(Size::new(pw as f64 / scale, ph as f64 / scale));
            let mode = s.redact.mode;
            let shapes = redactions.iter().map(|&rect| Shape::Redact { rect, mode }).collect();
            let n = doc.add_many(shapes, Style::default()).len();
            if let Some(flat) = lumora_ui::editor::flatten_document(&original, scale, &doc) {
                image = flat;
                document = Some(doc);
                self.notify_text(ToastKind::Success, &trf(Msg::ToastAutoRedacted, &[&n]), None);
            }
        }
        let stem = render(&s.general.filename_template, &system::now_parts(), CaptureKind::Screenshot);
        let (dir, temp) =
            if s.general.save_to_disk { (s.save_folder(&self.home), false) } else { (system::temp_dir(), true) };
        // Skip names whose `.lumora` sibling exists too (an orphaned document
        // must not be overwritten).
        let file = unique_path(&dir, &stem, CaptureKind::Screenshot.extension(), |p| {
            lumora_core::lumora_doc::pair_taken(p, |q| q.exists())
        });

        if s.general.play_sounds {
            system::play_shutter();
        }
        let point_size = Size::new(frame.width(), frame.height());
        let appkit_frame = util::flip(frame, self.mtm);
        let edit_now = s.general.open_editor_after_capture;
        if s.general.show_quick_access && !edit_now {
            let to = self.quick.next_frame(point_size);
            shutter::play(self.mtm, &image, appkit_frame, Some(to), || {});
            self.quick.push(CardContent {
                id,
                image: Some(image.clone()),
                point_size,
                file: file.clone(),
                video_duration: None,
                tutorial: false,
            });
        } else {
            shutter::play(self.mtm, &image, appkit_frame, None, || {});
        }
        // No auto-redaction: the saved image is the original capture.
        let unredacted = document.is_none();
        let rec = CaptureRecord {
            image: Some(image.clone()),
            scale,
            frame,
            file: file.clone(),
            temp,
            video: false,
            editable: Some(Editable { document, ..Editable::new(original, scale) }),
        };
        // The editable document keeps the original capture (not the
        // beautified export) plus the beautify style, when enabled.
        let job = self.doc_job(&rec);
        self.captures.borrow_mut().insert(id, rec);
        let copy = s.general.copy_to_clipboard;
        // The record, card and editor keep the plain capture; the saved file
        // and the clipboard get the beautified one (when enabled).
        let style = s.beautify;
        // Plain capture and a `.lumora` to write: both files hold the same
        // PNG, so encode it once (below) and hand the bytes to the doc job.
        let shared_job = match job {
            Some(job) if !style.enabled && unredacted => Some(job),
            Some(job) => {
                self.spawn_doc_write(job);
                None
            }
            None => None,
        };
        if edit_now {
            self.open_editor(id);
        }
        // Nothing to render: fill the clipboard now rather than after the
        // encode + write. The file URL resolves once the PNG lands.
        if copy && !style.enabled {
            lumora_capture::clipboard::copy_image(&image, scale, Some(&file));
        }

        background().exec_async(move || {
            let image = lumora_capture::beautify_render::beautify_for_export(image, &style, scale);
            if copy && style.enabled {
                // Rendered: hop to main for the clipboard before encoding.
                let (img, f) = (image.clone(), file.clone());
                on_main(move |_| lumora_capture::clipboard::copy_image(&img, scale, Some(&f)));
            }
            let result = match shared_job {
                Some(job) => write_png_shared(&image, &file, scale).map(|png| {
                    on_main(move |app| app.spawn_doc_write(job.with_png(png)));
                }),
                None => lumora_capture::image::write_png(&image, &file, scale),
            };
            on_main(move |_app| match result {
                Ok(()) => log::info!("saved {}", file.display()),
                Err(e) => {
                    log::error!("{e}");
                    if copy {
                        // Drop the dangling file URL.
                        lumora_capture::clipboard::copy_image(&image, scale, None);
                    }
                    _app.notify(Notice::Error(Msg::ToastSaveFailed), Some(error_text(&e)));
                }
            });
        });
    }

    /// Reset to idle and surface a capture error.
    pub fn fail(self: &Rc<Self>, e: CaptureError) {
        self.fail_with(Msg::ToastCaptureFailed, e);
    }

    /// Reset to idle and surface an error under `title` (a toast; the
    /// onboarding panel for a missing permission).
    pub fn fail_with(self: &Rc<Self>, title: Msg, e: CaptureError) {
        log::error!("{}: {e}", tr(title));
        self.set_mode(Mode::Idle);
        if e == CaptureError::PermissionDenied {
            self.dispatch(crate::command::Command::ShowOnboarding);
        } else {
            self.notify(Notice::Error(title), Some(error_text(&e)));
        }
    }
}

/// A localised one-line description of a capture error.
pub fn error_text(e: &CaptureError) -> String {
    match e {
        CaptureError::PermissionDenied => tr(Msg::ErrPermission).to_string(),
        CaptureError::MicrophoneDenied => tr(Msg::ErrMicPermission).to_string(),
        CaptureError::TargetGone(what) => {
            let what = if *what == "window" { tr(Msg::TargetWindow) } else { tr(Msg::TargetDisplay) };
            trf(Msg::ErrTargetGone, &[&what])
        }
        CaptureError::Framework(s) => s.clone(),
        CaptureError::Write { path, .. } => {
            let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned());
            trf(Msg::ErrWrite, &[&name.unwrap_or_else(|| path.clone())])
        }
        CaptureError::Empty => tr(Msg::ErrNothing).to_string(),
        CaptureError::SpeechDenied => tr(Msg::ErrSpeechPermission).to_string(),
        CaptureError::SpeechUnavailable(why) if why.starts_with(lumora_capture::transcribe::STRICT_UNAVAILABLE) => {
            tr(Msg::ErrSpeechUnavailableStrict).to_string()
        }
        CaptureError::SpeechUnavailable(why) => trf(Msg::ErrSpeechUnavailable, &[why]),
        CaptureError::Speech(why) => trf(Msg::ErrSpeech, &[why]),
        CaptureError::NoAudio => tr(Msg::ErrNoAudio).to_string(),
        CaptureError::Cancelled => tr(Msg::ErrCancelled).to_string(),
    }
}

/// The pixels for an overlay target, cut from the frozen display frames:
/// `(image, scale, frame in global points)`.
fn frozen_image(frozen: &[FrozenDisplay], target: &Target) -> Result<(CFRetained<CGImage>, f64, Rect), CaptureError> {
    let r = match target {
        Target::Display(frame) => {
            let fd = frozen.iter().find(|f| f.frame == *frame).ok_or(CaptureError::TargetGone("display"))?;
            return Ok((fd.image.clone(), fd.scale, fd.frame));
        }
        Target::Area(r) => *r,
        Target::Window(w) => w.frame,
    };
    // Selections may span displays: plan per-display pieces at the highest
    // scale involved, and only composite when more than one is needed.
    let sources: Vec<Source> = frozen.iter().map(|f| Source { frame: f.frame, scale: f.scale }).collect();
    let plan = stitch::plan(r, &sources).ok_or(CaptureError::Empty)?;
    if plan.is_single() {
        let fd = &frozen[plan.pieces[0].source];
        let img = lumora_capture::image::crop(&fd.image, fd.frame, plan.area)?;
        return Ok((img, fd.scale, plan.area.integral(fd.scale)));
    }
    let parts: Vec<StitchPart<'_>> = plan
        .pieces
        .iter()
        .map(|p| {
            let fd = &frozen[p.source];
            StitchPart { image: &fd.image, frame: fd.frame, area: p.area, dest: p.dest }
        })
        .collect();
    let img = lumora_capture::image::stitch(plan.width, plan.height, &parts)?;
    Ok((img, plan.scale, plan.area))
}

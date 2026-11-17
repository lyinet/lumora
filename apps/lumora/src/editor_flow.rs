//! The annotation editor flow: open an editor for a capture, write its
//! exports back over the capture's file, and bring the Quick Access card
//! back (with the annotated image) when the editor closes.

use std::rc::Rc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_core::geometry::{Rect, Size};
use lumora_core::i18n::Msg;
use lumora_ui::editor::EditorWindow;
use lumora_ui::events::{EditorAction, EditorImage};
use lumora_ui::quick_access::CardContent;

use crate::app::{on_main, App, Notice};

impl App {
    /// Open (or focus) the editor for capture `id`.
    pub fn open_editor(self: &Rc<Self>, id: u64) {
        if let Some(e) = self.editors.borrow().get(&id) {
            e.focus();
            return;
        }
        self.hydrate_from_disk(id);
        // Re-editing starts from the original capture plus the saved
        // annotations (non-destructive); records without one (none today)
        // edit their current image.
        let info = self.captures.borrow().get(&id).and_then(|r| {
            if r.video {
                return None;
            }
            let name = r.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match &r.editable {
                Some(ed) => Some((ed.original.clone(), ed.scale, ed.document.clone(), ed.beautify.clone(), name)),
                None => Some((r.image.clone()?, r.scale, None, None, name)),
            }
        });
        let Some((image, scale, document, beautify, name)) = info else { return };
        let beautify = beautify.unwrap_or_else(|| self.settings.borrow().beautify.clone());
        let editor = EditorWindow::show(self.mtm, id, image, scale, &name, beautify, document, self.sink.clone());
        editor.set_ai_enabled(self.settings.borrow().ai.enabled);
        self.editors.borrow_mut().insert(id, editor);
    }

    pub fn on_editor(self: &Rc<Self>, id: u64, action: EditorAction) {
        match action {
            EditorAction::Copied(_) => self.notify(Notice::Success(Msg::ToastCopied), None),
            EditorAction::Save(img) => self.write_annotated(id, img, true),
            EditorAction::AiWrite(req) => self.on_ai_write(id, *req),
            EditorAction::AiCancel(request) => self.on_ai_cancel(id, request),
            EditorAction::OpenAiSettings => self.open_ai_settings(),
            EditorAction::SmartRedact(req) => self.on_smart_redact(id, *req),
            EditorAction::Closed(img) => {
                self.cancel_ai(id);
                if let Some(img) = img {
                    self.write_annotated(id, img, false);
                }
                // Queued event: the window has finished closing by now.
                let editor = self.editors.borrow_mut().remove(&id);
                drop(editor);
                self.show_card_again(id);
            }
        }
    }

    /// Replace the capture's image and rewrite its file (and its `.lumora`
    /// document) in the background.
    fn write_annotated(self: &Rc<Self>, id: u64, img: EditorImage, notify: bool) {
        let (file, job) = {
            let mut caps = self.captures.borrow_mut();
            let Some(rec) = caps.get_mut(&id) else { return };
            let (pw, ph) = lumora_capture::image::pixel_size(&img.image);
            rec.frame =
                Rect { origin: rec.frame.origin, size: Size::new(pw as f64 / img.scale, ph as f64 / img.scale) };
            rec.image = Some(img.image.clone());
            rec.scale = img.scale;
            if let Some(ed) = rec.editable.as_mut() {
                ed.document = Some(img.document);
                ed.beautify = Some(img.beautify.clone());
            }
            (rec.file.clone(), self.doc_job(rec))
        };
        if let Some(job) = job {
            self.spawn_doc_write(job);
        }
        let image = img.image;
        let scale = img.scale;
        // The record keeps the plain annotated image (card, pin); re-editing
        // starts from the original + document; only the file gets the
        // beautified version, in the editor's style.
        let style = img.beautify;
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let image = lumora_capture::beautify_render::beautify_for_export(image, &style, scale);
                let result = lumora_capture::image::write_png(&image, &file, scale);
                on_main(move |app| match result {
                    Ok(()) => {
                        log::info!("saved annotated {}", file.display());
                        if notify {
                            app.notify(Notice::Success(Msg::ToastSaved), None);
                        }
                    }
                    Err(e) => {
                        log::error!("{e}");
                        app.notify(Notice::Error(Msg::ToastSaveFailed), Some(crate::capture_flow::error_text(&e)));
                    }
                });
            },
        );
    }

    /// After editing, show the (annotated) capture's card again. The record
    /// moves to a fresh id so the old card's exit animation can't collide
    /// with the new card.
    fn show_card_again(self: &Rc<Self>, id: u64) {
        if !self.settings.borrow().general.show_quick_access {
            self.captures.borrow_mut().remove(&id);
            return;
        }
        let Some(rec) = self.captures.borrow_mut().remove(&id) else { return };
        let new_id = self.next_id();
        let content = CardContent {
            id: new_id,
            image: rec.image.clone(),
            point_size: Size::new(rec.frame.width(), rec.frame.height()),
            file: rec.file.clone(),
            video_duration: None,
            tutorial: false,
        };
        self.captures.borrow_mut().insert(new_id, rec);
        self.quick.push(content);
    }
}

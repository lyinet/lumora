//! AI writing from the annotation editor. The editor emits
//! [`EditorAction::AiWrite`](lumora_ui::events::EditorAction::AiWrite); here
//! the app reads the API key from the Keychain, checks the settings, then on
//! a background queue runs Vision OCR on the capture (when the task needs
//! it), builds the prompt and streams the completion. Deltas hop to the main
//! queue tagged with the editor's request id, so a stopped or retried
//! request can never write into the sheet. Nothing but the task kind is
//! logged: no text, no key.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_ai::{build_request, AiContext, AiError, KeyStore, Keychain};
use lumora_ui::events::AiWriteRequest;

use crate::app::{on_main, App};

/// A running request: the editor's request id and its cancel flag.
pub struct AiJob {
    pub request: u64,
    pub cancel: Arc<AtomicBool>,
}

impl App {
    /// Start an AI writing request for editor `editor`.
    pub fn on_ai_write(self: &Rc<Self>, editor: u64, req: AiWriteRequest) {
        let request = req.request;
        let mut cfg = self.settings.borrow().ai.clone();
        cfg.validate();
        if !cfg.enabled {
            return self.ai_done(editor, request, Err(AiError::BadConfig));
        }
        let key = match Keychain.get(cfg.provider) {
            Ok(k) => k,
            Err(e) => {
                log::warn!("reading the {} API key failed: {e:?}", cfg.provider.id());
                return self.ai_done(editor, request, Err(e));
            }
        };
        if let Err(e) = cfg.check_ready(key.as_deref()) {
            return self.ai_done(editor, request, Err(e));
        }
        // The rendered (annotated) image goes along only when the user
        // opted in and the task can use it. Flattening is main-thread work.
        let image = if cfg.send_image && req.kind.uses_image() {
            lumora_ui::editor::flatten_document(&req.base, req.scale, &req.document)
        } else {
            None
        };
        self.cancel_ai(editor);
        let cancel = Arc::new(AtomicBool::new(false));
        self.ai_jobs.borrow_mut().insert(editor, AiJob { request, cancel: cancel.clone() });
        let ui = lumora_core::i18n::language();
        let AiWriteRequest { kind, source_text, document, base, scale, .. } = req;
        log::info!("AI writing: {kind:?} (editor {editor}, request {request})");
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let run = || -> Result<String, AiError> {
                    let ocr = if kind.needs_ocr(source_text.is_some()) {
                        lumora_capture::ocr::recognize_sync(&base).unwrap_or_else(|e| {
                            log::warn!("OCR for AI writing failed: {e}");
                            String::new()
                        })
                    } else {
                        String::new()
                    };
                    if cancel.load(Ordering::Relaxed) {
                        return Err(AiError::Cancelled);
                    }
                    let mut ctx = AiContext::from_document(&document, ocr.clone());
                    if let Some(img) = image {
                        match lumora_capture::image::png_bytes(&img, scale) {
                            Ok(png) => ctx.image_png = Some(png),
                            Err(e) => log::warn!("encoding the screenshot for AI failed: {e}"),
                        }
                    }
                    let task =
                        kind.task(source_text.unwrap_or(if kind.uses_selection() { ocr } else { String::new() }));
                    let body = build_request(&cfg, &task, &ctx, ui, true)?;
                    lumora_ai::complete(&cfg, key.as_deref(), &body, &cancel, |delta| {
                        let delta = delta.to_string();
                        on_main(move |app| app.ai_delta(editor, request, &delta));
                    })
                };
                let result = run();
                if let Err(e) = &result {
                    log::info!("AI writing ended: {e:?}");
                }
                on_main(move |app| app.ai_done(editor, request, result));
            },
        );
    }

    /// Stop editor `editor`'s request `request` (if it is still running).
    pub fn on_ai_cancel(self: &Rc<Self>, editor: u64, request: u64) {
        let mut jobs = self.ai_jobs.borrow_mut();
        if jobs.get(&editor).is_some_and(|j| j.request == request) {
            if let Some(j) = jobs.remove(&editor) {
                j.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Stop whatever editor `editor` is running (window closed, new request).
    pub fn cancel_ai(&self, editor: u64) {
        if let Some(j) = self.ai_jobs.borrow_mut().remove(&editor) {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn ai_delta(self: &Rc<Self>, editor: u64, request: u64, delta: &str) {
        let current = self.ai_jobs.borrow().get(&editor).is_some_and(|j| j.request == request);
        if !current {
            return;
        }
        if let Some(e) = self.editors.borrow().get(&editor) {
            e.ai_delta(request, delta);
        }
    }

    fn ai_done(self: &Rc<Self>, editor: u64, request: u64, result: Result<String, AiError>) {
        {
            let mut jobs = self.ai_jobs.borrow_mut();
            if jobs.get(&editor).is_some_and(|j| j.request == request) {
                jobs.remove(&editor);
            }
        }
        if let Some(e) = self.editors.borrow().get(&editor) {
            e.ai_finished(request, result);
        }
    }

    /// "Open AI Settings" from an editor's result sheet.
    pub fn open_ai_settings(self: &Rc<Self>) {
        self.show_settings();
        if let Some(w) = self.settings_window.borrow().as_ref() {
            w.select_ai_tab();
        }
    }
}

//! The Capture Text result panel: shown after ⌥⇧2 copies recognised text.
//! The panel computes on-device results itself; here the app runs its AI
//! requests (Summarize, Translate, and AI refinement of table / code /
//! checklist) on a background queue and streams answers back under the
//! panel's request id. Only the task kind is logged, never the text.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_ai::{build_request, AiContext, AiError, KeyStore, Keychain};
use lumora_core::ocr::TextBox;
use lumora_ui::events::{TextAiRequest, TextResultAction};
use lumora_ui::text_result::TextResultPanel;

use crate::ai_write_flow::AiJob;
use crate::app::{on_main, App};

impl App {
    /// Show (or refresh) the Capture Text panel with `text` and its runs.
    pub fn show_text_result(self: &Rc<Self>, text: &str, boxes: Vec<TextBox>) {
        let (ai_enabled, panel_on) = {
            let st = self.settings.borrow();
            (st.ai.enabled, st.capture.text_actions_panel)
        };
        if !panel_on {
            return;
        }
        if self.text_panel.borrow().is_none() {
            *self.text_panel.borrow_mut() = Some(TextResultPanel::new(self.mtm, self.sink.clone()));
        }
        if let Some(p) = self.text_panel.borrow().as_ref() {
            p.show(text, boxes, ai_enabled);
        }
    }

    pub fn on_text_result(self: &Rc<Self>, action: TextResultAction) {
        match action {
            TextResultAction::Run(req) => self.on_text_ai(req),
            TextResultAction::Cancel(request) => {
                let mut job = self.text_job.borrow_mut();
                if job.as_ref().is_some_and(|j| j.request == request) {
                    if let Some(j) = job.take() {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
            TextResultAction::OpenAiSettings => self.open_ai_settings(),
            TextResultAction::Closed => {
                if let Some(j) = self.text_job.borrow_mut().take() {
                    j.cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    fn on_text_ai(self: &Rc<Self>, req: TextAiRequest) {
        let TextAiRequest { request, kind, input } = req;
        let mut cfg = self.settings.borrow().ai.clone();
        cfg.validate();
        if !cfg.enabled {
            return self.text_ai_done(request, Err(AiError::BadConfig));
        }
        if let Some(j) = self.text_job.borrow_mut().take() {
            j.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.text_job.borrow_mut() = Some(AiJob { request, cancel: cancel.clone() });
        let ui = lumora_core::i18n::language();
        log::info!("text AI: {kind:?} (request {request})");
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let run = || -> Result<String, AiError> {
                    // Off the main thread: an ad-hoc-signed build may show a
                    // Keychain prompt, which would block the run loop.
                    let key = Keychain.get(cfg.provider).inspect_err(|e| {
                        log::warn!("reading the {} API key failed: {e:?}", cfg.provider.id());
                    })?;
                    cfg.check_ready(key.as_deref())?;
                    let ctx = AiContext { ocr_text: input.clone(), ..AiContext::default() };
                    let body = build_request(&cfg, &kind.task(input), &ctx, ui, true)?;
                    lumora_ai::complete(&cfg, key.as_deref(), &body, &cancel, |delta| {
                        let delta = delta.to_string();
                        on_main(move |app| app.text_ai_delta(request, &delta));
                    })
                };
                let result = run();
                if let Err(e) = &result {
                    log::info!("text AI ended: {e:?}");
                }
                on_main(move |app| app.text_ai_done(request, result));
            },
        );
    }

    fn text_ai_delta(self: &Rc<Self>, request: u64, delta: &str) {
        if !self.text_job.borrow().as_ref().is_some_and(|j| j.request == request) {
            return;
        }
        if let Some(p) = self.text_panel.borrow().as_ref() {
            p.ai_delta(request, delta);
        }
    }

    fn text_ai_done(self: &Rc<Self>, request: u64, result: Result<String, AiError>) {
        {
            let mut job = self.text_job.borrow_mut();
            if job.as_ref().is_some_and(|j| j.request == request) {
                *job = None;
            }
        }
        if let Some(p) = self.text_panel.borrow().as_ref() {
            p.ai_finished(request, result);
        }
    }
}

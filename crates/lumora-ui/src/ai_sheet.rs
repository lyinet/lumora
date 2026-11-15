//! The AI writing result sheet: a dark glass card, docked to the bottom of
//! the editor canvas or hosted by the Capture Text panel
//! ([`crate::text_result`]). It shows the task title, a spinner while the app reads
//! the screenshot's text and the model streams its answer, the (read-only,
//! selectable) text as it arrives, errors in red, and the actions Stop,
//! Retry, Copy, Insert as Text / Replace Text, Open AI Settings and Close.
//!
//! It fades in with a small spring rise (like the beautify panel's fade)
//! and hides at once so it never takes clicks while fading out. All logic
//! (what to insert where) is in the editor; the sheet only reports button
//! presses through its [`SheetHandler`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Msg};
use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSColor,
    NSControlSize, NSFont, NSLineBreakMode, NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSTextField,
    NSTextView, NSVisualEffectView,
};
use objc2_foundation::{NSRange, NSString};

use crate::tokens::{motion, radius, typography};
use crate::util::{cg_rect, glass_view, spring_scalar, weight, ActionTarget};

/// Sheet size and spacing (points).
const WIDTH: f64 = 540.0;
const HEIGHT: f64 = 248.0;
const PAD: f64 = 14.0;
const BUTTON_H: f64 = 28.0;
const BOTTOM: f64 = 18.0;

/// A button press in the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SheetEvent {
    Stop,
    Retry,
    Copy,
    /// "Copy as CSV" (table results).
    CopyCsv,
    Insert,
    OpenSettings,
    Close,
}

pub(crate) type SheetHandler = Rc<dyn Fn(SheetEvent)>;

/// What the sheet is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Hidden,
    /// Waiting for the first words (OCR, connecting).
    Working,
    Streaming,
    Done,
    Failed,
}

pub(crate) struct AiSheet {
    pub view: Retained<NSVisualEffectView>,
    title: Retained<NSTextField>,
    status: Retained<NSTextField>,
    spinner: Retained<NSProgressIndicator>,
    text: Retained<NSTextView>,
    stop: Retained<NSButton>,
    retry: Retained<NSButton>,
    copy: Retained<NSButton>,
    insert: Retained<NSButton>,
    csv: Retained<NSButton>,
    settings: Retained<NSButton>,
    _close: Retained<NSButton>,
    _targets: Vec<Retained<ActionTarget>>,
    buffer: RefCell<String>,
    phase: Cell<Phase>,
    /// The on-device result shown while AI refines it; restored when the
    /// request fails or is stopped before any text arrived.
    fallback: RefCell<Option<String>>,
    mono: Cell<bool>,
}

/// How a sheet is built.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SheetOptions {
    /// Show "Insert as Text" (editor only).
    pub insert: bool,
    /// Space reserved at the top for the host's own controls (points).
    pub top_inset: f64,
}

impl std::fmt::Debug for AiSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiSheet").field("phase", &self.phase.get()).finish_non_exhaustive()
    }
}

fn label(mtm: objc2::MainThreadMarker, size: f64, w: objc2_app_kit::NSFontWeight) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::new(), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(size, w)));
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l
}

fn button(
    mtm: objc2::MainThreadMarker,
    title: &str,
    on: &SheetHandler,
    event: SheetEvent,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSButton> {
    let h = on.clone();
    let target = ActionTarget::new(mtm, move |_| h(event));
    // SAFETY: the target is kept alive by the sheet.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(&target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.setBezelStyle(NSBezelStyle::Push);
    b.setControlSize(NSControlSize::Small);
    b.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY - 1.0, weight::REGULAR)));
    b.sizeToFit();
    targets.push(target);
    b
}

impl AiSheet {
    /// A hidden sheet for a container of `width` (the editor's content
    /// view, y-up). It stays bottom-centred as the window resizes.
    pub fn new(mtm: objc2::MainThreadMarker, width: f64, on: SheetHandler) -> Self {
        let w = WIDTH.min(width - 2.0 * PAD).max(320.0);
        let frame = Rect::new(((width - w) / 2.0).max(0.0), BOTTOM, w, HEIGHT);
        let sheet = AiSheet::build(mtm, frame, SheetOptions { insert: true, top_inset: 0.0 }, on);
        sheet.view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        sheet
    }

    /// The default sheet height (points), without a top inset.
    pub const HEIGHT: f64 = HEIGHT;

    /// A hidden sheet filling `frame` of its host view.
    pub fn build(mtm: objc2::MainThreadMarker, frame: Rect, opts: SheetOptions, on: SheetHandler) -> Self {
        let w = frame.width();
        let height = frame.height();
        let view = glass_view(mtm, cg_rect(frame), radius::HUD);
        view.setHidden(true);
        view.setAlphaValue(0.0);

        let mut targets = Vec::new();
        // Header: spinner, title, status (right).
        let top = height - opts.top_inset - PAD - 18.0;
        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            cg_rect(Rect::new(PAD, top, 16.0, 16.0)),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        view.addSubview(&spinner);
        let title = label(mtm, typography::BODY, weight::SEMIBOLD);
        title.setTextColor(Some(&NSColor::labelColor()));
        title.setFrame(cg_rect(Rect::new(PAD + 22.0, top - 1.0, w * 0.45, 18.0)));
        view.addSubview(&title);
        let status = label(mtm, typography::CAPTION, weight::REGULAR);
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setAlignment(objc2_app_kit::NSTextAlignment::Right);
        status.setFrame(cg_rect(Rect::new(w * 0.45 + PAD + 28.0, top, w * 0.55 - 2.0 * PAD - 28.0, 16.0)));
        status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        view.addSubview(&status);

        // Body: scrollable, read-only, selectable text.
        let body_y = PAD + BUTTON_H + 10.0;
        let body = Rect::new(PAD, body_y, w - 2.0 * PAD, top - 8.0 - body_y);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), cg_rect(body));
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            cg_rect(Rect::new(0.0, 0.0, body.width(), body.height())),
        );
        text.setEditable(false);
        text.setSelectable(true);
        text.setDrawsBackground(false);
        text.setRichText(false);
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        text.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)));
        text.setTextColor(Some(&NSColor::labelColor()));
        if let Some(c) = unsafe { text.textContainer() } {
            c.setWidthTracksTextView(true);
        }
        scroll.setDocumentView(Some(&text));
        view.addSubview(&scroll);

        // Buttons: leading Retry / Stop / Settings, trailing Close / Copy / Insert.
        let stop = button(mtm, tr(Msg::AiStop), &on, SheetEvent::Stop, &mut targets);
        let retry = button(mtm, tr(Msg::AiRetry), &on, SheetEvent::Retry, &mut targets);
        let settings = button(mtm, tr(Msg::AiOpenSettings), &on, SheetEvent::OpenSettings, &mut targets);
        let close = button(mtm, tr(Msg::AiClose), &on, SheetEvent::Close, &mut targets);
        let copy = button(mtm, tr(Msg::Copy), &on, SheetEvent::Copy, &mut targets);
        let insert = button(mtm, tr(Msg::AiInsert), &on, SheetEvent::Insert, &mut targets);
        let csv = button(mtm, tr(Msg::TextCopyCsv), &on, SheetEvent::CopyCsv, &mut targets);
        insert.setHidden(!opts.insert);
        csv.setHidden(true);
        let mut x = PAD;
        for b in [&retry, &stop] {
            let f = b.frame();
            b.setFrame(cg_rect(Rect::new(x, PAD, f.size.width.max(64.0), BUTTON_H)));
            // Retry and Stop share the leading slot; one is hidden.
            view.addSubview(b);
        }
        x += retry.frame().size.width.max(stop.frame().size.width) + 8.0;
        let f = settings.frame();
        settings.setFrame(cg_rect(Rect::new(x, PAD, f.size.width, BUTTON_H)));
        view.addSubview(&settings);
        // Trailing, left to right: [CSV] [Insert] [Copy] [Close].
        let mut right = w - PAD;
        let trailing: Vec<&Retained<NSButton>> =
            if opts.insert { vec![&close, &copy, &insert, &csv] } else { vec![&close, &copy, &csv] };
        for b in trailing {
            let bw = b.frame().size.width.max(64.0);
            right -= bw;
            b.setFrame(cg_rect(Rect::new(right, PAD, bw, BUTTON_H)));
            b.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            view.addSubview(b);
            right -= 8.0;
        }

        AiSheet {
            view,
            title,
            status,
            spinner,
            text,
            stop,
            retry,
            copy,
            insert,
            csv,
            settings,
            _close: close,
            _targets: targets,
            buffer: RefCell::new(String::new()),
            phase: Cell::new(Phase::Hidden),
            fallback: RefCell::new(None),
            mono: Cell::new(false),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase.get()
    }

    pub fn is_open(&self) -> bool {
        self.phase.get() != Phase::Hidden
    }

    /// The text shown (the cleaned result once done).
    pub fn text(&self) -> String {
        self.buffer.borrow().clone()
    }

    /// Start a request: title, empty body, spinner. `replace` labels the
    /// insert button "Replace Text".
    pub fn begin(&self, title: &str, status: Msg, replace: bool) {
        self.title.setStringValue(&NSString::from_str(title));
        self.insert.setTitle(&NSString::from_str(tr(if replace { Msg::AiReplace } else { Msg::AiInsert })));
        self.buffer.borrow_mut().clear();
        *self.fallback.borrow_mut() = None;
        self.show_text();
        self.set_status(tr(status), false);
        self.phase.set(Phase::Working);
        self.sync_controls();
        self.reveal();
    }

    /// Show `local` (an on-device result) while AI refines it: the first
    /// streamed words replace it; a failure or an early stop restores it.
    pub fn begin_refine(&self, title: &str, local: &str) {
        self.title.setStringValue(&NSString::from_str(title));
        *self.buffer.borrow_mut() = local.to_string();
        *self.fallback.borrow_mut() = Some(local.to_string());
        self.show_text();
        self.set_status(tr(Msg::TextRefining), false);
        self.phase.set(Phase::Working);
        self.sync_controls();
        self.reveal();
    }

    /// Show a finished result straight away (OCR text, on-device result).
    pub fn show_result(&self, title: &str, text: &str, status: &str) {
        self.title.setStringValue(&NSString::from_str(title));
        *self.fallback.borrow_mut() = None;
        self.finish(text);
        self.set_status(status, false);
        self.reveal();
    }

    /// Show "Copy as CSV" (table results).
    pub fn set_csv_visible(&self, visible: bool) {
        self.csv.setHidden(!visible);
    }

    /// Monospaced body text (code and tables).
    pub fn set_monospace(&self, mono: bool) {
        self.mono.set(mono);
        self.show_text();
    }

    /// Append streamed text.
    pub fn append(&self, delta: &str) {
        if !matches!(self.phase.get(), Phase::Working | Phase::Streaming) {
            return;
        }
        if self.phase.get() == Phase::Working {
            if self.fallback.borrow().is_some() {
                self.buffer.borrow_mut().clear();
            }
            self.phase.set(Phase::Streaming);
            self.set_status(tr(Msg::AiWriting), false);
            self.sync_controls();
        }
        self.buffer.borrow_mut().push_str(delta);
        self.show_text();
    }

    /// Show the final (cleaned) text.
    pub fn finish(&self, text: &str) {
        let text =
            if text.trim().is_empty() { self.fallback.borrow().clone().unwrap_or_default() } else { text.into() };
        *self.fallback.borrow_mut() = None;
        *self.buffer.borrow_mut() = text;
        self.show_text();
        self.set_status("", false);
        self.phase.set(Phase::Done);
        self.sync_controls();
    }

    /// Show an error in red. Keeps any partial text. `settings` shows the
    /// "Open AI Settings" button.
    pub fn fail(&self, message: &str, settings: bool) {
        if let Some(local) = self.fallback.borrow_mut().take() {
            // Keep the on-device result usable; say why AI didn't help.
            *self.buffer.borrow_mut() = local;
            self.show_text();
            self.phase.set(Phase::Done);
            self.sync_controls();
            self.set_status(&lumora_core::i18n::trf(Msg::TextAiKeptLocal, &[&message]), true);
            self.settings.setHidden(!settings);
            return;
        }
        self.set_status(message, true);
        self.phase.set(Phase::Failed);
        self.sync_controls();
        self.settings.setHidden(!settings);
        // The status line truncates; long errors also go in the body when
        // there is no partial text to keep.
        if self.buffer.borrow().is_empty() {
            self.text.setString(&NSString::from_str(message));
            self.text.setTextColor(Some(&NSColor::systemRedColor()));
        }
    }

    /// The user stopped the request: keep what arrived (it can still be
    /// copied or inserted) and say so.
    pub fn stopped(&self, text: &str) {
        let streamed = self.phase.get() == Phase::Streaming;
        let fallback = self.fallback.borrow_mut().take();
        let text = match fallback {
            Some(local) if !streamed || text.trim().is_empty() => local,
            _ => text.to_string(),
        };
        *self.buffer.borrow_mut() = text;
        self.show_text();
        self.phase.set(Phase::Done);
        self.sync_controls();
        self.set_status(tr(Msg::AiStopped), false);
    }

    /// A short grey note in the header ("Copied to the clipboard").
    pub fn note(&self, s: &str) {
        self.set_status(s, false);
    }

    pub fn hide(&self) {
        self.phase.set(Phase::Hidden);
        unsafe { self.spinner.stopAnimation(None) };
        self.view.setHidden(true);
        self.view.setAlphaValue(0.0);
    }

    fn set_status(&self, s: &str, error: bool) {
        self.status.setStringValue(&NSString::from_str(s));
        self.status.setToolTip(Some(&NSString::from_str(s)));
        let c = if error { NSColor::systemRedColor() } else { NSColor::secondaryLabelColor() };
        self.status.setTextColor(Some(&c));
    }

    fn show_text(&self) {
        let buf = self.buffer.borrow();
        self.text.setString(&NSString::from_str(&buf));
        let font = if self.mono.get() {
            NSFont::monospacedSystemFontOfSize_weight(typography::BODY - 1.0, weight::REGULAR)
        } else {
            NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)
        };
        self.text.setFont(Some(&font));
        self.text.setTextColor(Some(&NSColor::labelColor()));
        let len = buf.encode_utf16().count();
        self.text.scrollRangeToVisible(NSRange::new(len, 0));
    }

    fn sync_controls(&self) {
        let phase = self.phase.get();
        let busy = matches!(phase, Phase::Working | Phase::Streaming);
        // SAFETY: plain AppKit calls on the main thread.
        unsafe {
            if busy {
                self.spinner.startAnimation(None);
            } else {
                self.spinner.stopAnimation(None);
            }
        }
        self.stop.setHidden(!busy);
        self.retry.setHidden(busy);
        self.settings.setHidden(true);
        let has_text = phase == Phase::Done && !self.buffer.borrow().trim().is_empty();
        self.copy.setEnabled(has_text);
        self.insert.setEnabled(has_text);
        self.csv.setEnabled(has_text);
    }

    /// Fade in with a small spring rise.
    fn reveal(&self) {
        if !self.view.isHidden() && self.view.alphaValue() >= 1.0 {
            return;
        }
        self.view.setAlphaValue(0.0);
        self.view.setHidden(false);
        NSAnimationContext::beginGrouping();
        NSAnimationContext::currentContext().setDuration(motion::HOVER_FADE * 1.5);
        self.view.animator().setAlphaValue(1.0);
        NSAnimationContext::endGrouping();
        if let Some(l) = self.view.layer() {
            spring_scalar(&l, "transform.translation.y", -14.0, 0.0, motion::TOAST);
        }
    }

    /// Show at once (standalone hosts are never docked or faded).
    pub fn show_now(&self) {
        self.view.setHidden(false);
        self.view.setAlphaValue(1.0);
    }

    /// Keep the sheet inside a container that got narrower than it.
    pub fn fit(&self, container_width: f64) {
        let f = self.view.frame();
        let w = WIDTH.min(container_width - 2.0 * PAD).max(320.0);
        if (f.size.width - w).abs() > 0.5 {
            self.view.setFrame(cg_rect(Rect::new(((container_width - w) / 2.0).max(0.0), BOTTOM, w, HEIGHT)));
        }
    }
}

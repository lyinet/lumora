//! The Capture Text result panel: after ⌥⇧2 copies the recognised text, a
//! small floating glass panel shows it with actions — Copy Text, Summarize,
//! Translate (English / 简体中文), As Table, As Code and Checklist.
//!
//! Table, code and checklist results appear at once from on-device
//! heuristics ([`lumora_core::ai::local_result`]); with AI writing on, AI
//! then refines them in place. Summarize and Translate need AI and are
//! disabled (with a hint) when it is off. Results use the same streaming
//! sheet as the editor ([`crate::ai_sheet`]) with Copy, Copy as CSV
//! (tables), Stop and Retry. The app runs requests and streams answers back
//! through [`TextResultPanel::ai_delta`] / [`TextResultPanel::ai_finished`]
//! under the panel's request id; stale ids are ignored.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use lumora_core::ai::{
    ai_input, clean_output, error_opens_settings, language_name, local_result, table_csv, AiError, AiTaskKind,
};
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Language, Msg};
use lumora_core::ocr::TextBox;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSControlSize, NSFloatingWindowLevel, NSFont, NSMenu, NSMenuItem, NSPanel, NSView,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::NSString;

use crate::ai_sheet::{AiSheet, SheetEvent, SheetHandler, SheetOptions};
use crate::events::{EventSink, TextAiRequest, TextResultAction, UiEvent};
use crate::tokens::{size, typography};
use crate::util::{cg_rect, make_panel, rect, screen_under_mouse, weight, ActionTarget, PanelSpec};

const WIDTH: f64 = 580.0;
/// Height of the actions row above the sheet's header.
const ACTIONS: f64 = 40.0;
const PAD: f64 = 14.0;

struct State {
    mtm: MainThreadMarker,
    sink: EventSink,
    panel: Retained<NSPanel>,
    sheet: AiSheet,
    ai_buttons: Vec<(AiTaskKind, Retained<NSButton>)>,
    translate: Retained<NSButton>,
    _targets: Vec<Retained<ActionTarget>>,
    menu_targets: RefCell<Vec<Retained<ActionTarget>>>,
    text: RefCell<String>,
    boxes: RefCell<Vec<TextBox>>,
    ai_enabled: Cell<bool>,
    kind: Cell<Option<AiTaskKind>>,
    request: Cell<u64>,
    next: Cell<u64>,
    open: Cell<bool>,
}

/// The floating Capture Text panel (one per app; reused for each capture).
pub struct TextResultPanel {
    state: Rc<State>,
}

impl std::fmt::Debug for TextResultPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextResultPanel").field("open", &self.state.open.get()).finish_non_exhaustive()
    }
}

fn small_button(mtm: MainThreadMarker, title: &str, target: &ActionTarget) -> Retained<NSButton> {
    // SAFETY: the target is kept alive by the panel.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.setBezelStyle(NSBezelStyle::Push);
    b.setControlSize(NSControlSize::Small);
    b.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY - 1.0, weight::REGULAR)));
    b.sizeToFit();
    b
}

impl TextResultPanel {
    pub fn new(mtm: MainThreadMarker, sink: EventSink) -> Self {
        let h = AiSheet::HEIGHT + ACTIONS;
        let panel = make_panel(
            mtm,
            cg_rect(Rect::new(0.0, 0.0, WIDTH, h)),
            PanelSpec { level: NSFloatingWindowLevel, keyable: true, shadow: true, ignores_mouse: false },
        );
        panel.setMovableByWindowBackground(true);
        let root = NSView::initWithFrame(NSView::alloc(mtm), cg_rect(Rect::new(0.0, 0.0, WIDTH, h)));

        let state = Rc::new_cyclic(|weak: &Weak<State>| {
            let w = weak.clone();
            let handler: SheetHandler = Rc::new(move |e| {
                if let Some(s) = w.upgrade() {
                    State::sheet_event(&s, e);
                }
            });
            let sheet = AiSheet::build(
                mtm,
                Rect::new(0.0, 0.0, WIDTH, h),
                SheetOptions { insert: false, top_inset: ACTIONS },
                handler,
            );
            root.addSubview(&sheet.view);

            // Actions row, left to right along the top.
            let mut targets = Vec::new();
            let y = h - PAD - 24.0;
            let mut x = PAD;
            let mut place = |b: &NSButton, gap: f64| {
                let f = b.frame();
                b.setFrame(cg_rect(Rect::new(x, y, f.size.width.max(56.0), 24.0)));
                sheet.view.addSubview(b);
                x += f.size.width.max(56.0) + gap;
            };
            let w = weak.clone();
            let t = ActionTarget::new(mtm, move |_| {
                if let Some(s) = w.upgrade() {
                    State::copy_text(&s);
                }
            });
            let copy = small_button(mtm, tr(Msg::TextCopy), &t);
            place(&copy, 16.0);
            targets.push(t);
            let mut ai_buttons = Vec::new();
            let mut translate = None;
            for kind in AiTaskKind::TEXT_ACTIONS {
                let w = weak.clone();
                let t = ActionTarget::new(mtm, move |_| {
                    if let Some(s) = w.upgrade() {
                        match kind {
                            AiTaskKind::Translate(_) => State::show_translate_menu(&s),
                            k => State::run(&s, k),
                        }
                    }
                });
                let title = match kind {
                    AiTaskKind::Translate(_) => format!("{} ▾", kind.button_title()),
                    k => k.button_title().to_string(),
                };
                let b = small_button(mtm, &title, &t);
                place(&b, 6.0);
                targets.push(t);
                if matches!(kind, AiTaskKind::Translate(_)) {
                    translate = Some(b.clone());
                }
                ai_buttons.push((kind, b));
            }
            State {
                mtm,
                sink,
                panel: panel.clone(),
                sheet,
                ai_buttons,
                translate: translate.expect("Translate is a text action"),
                _targets: targets,
                menu_targets: RefCell::new(Vec::new()),
                text: RefCell::new(String::new()),
                boxes: RefCell::new(Vec::new()),
                ai_enabled: Cell::new(false),
                kind: Cell::new(None),
                request: Cell::new(0),
                next: Cell::new(1),
                open: Cell::new(false),
            }
        });
        panel.setContentView(Some(&root));
        TextResultPanel { state }
    }

    /// Show the recognised `text` (and its runs) near the top of the screen
    /// under the mouse, replacing whatever the panel showed.
    pub fn show(&self, text: &str, boxes: Vec<TextBox>, ai_enabled: bool) {
        let s = &self.state;
        s.cancel();
        *s.text.borrow_mut() = text.to_string();
        *s.boxes.borrow_mut() = boxes;
        s.kind.set(None);
        self.set_ai_enabled(ai_enabled);
        s.sheet.set_csv_visible(false);
        s.sheet.set_monospace(false);
        s.sheet.show_result(tr(Msg::TextResultTitle), text, tr(Msg::AiCopied));
        s.sheet.show_now();
        if !s.open.replace(true) {
            if let Some(screen) = screen_under_mouse(s.mtm) {
                let v = rect(screen.visibleFrame());
                let f = s.panel.frame();
                let x = (v.mid().x - f.size.width / 2.0).round();
                let y = (v.max_y() - size::TOAST_TOP - size::TOAST_HEIGHT - 28.0 - f.size.height).round();
                s.panel.setFrameOrigin(CGPoint::new(x, y));
            }
        }
        s.panel.orderFrontRegardless();
        s.panel.makeKeyWindow();
    }

    /// AI writing turned on or off in Settings.
    pub fn set_ai_enabled(&self, enabled: bool) {
        let s = &self.state;
        s.ai_enabled.set(enabled);
        for (kind, b) in &s.ai_buttons {
            let usable = enabled || kind.has_local();
            b.setEnabled(usable);
            let tip = if usable { None } else { Some(NSString::from_str(tr(Msg::TextAiOffHint))) };
            b.setToolTip(tip.as_deref());
        }
    }

    pub fn is_open(&self) -> bool {
        self.state.open.get()
    }

    /// Streamed text for `request`; stale ids are ignored.
    pub fn ai_delta(&self, request: u64, delta: &str) {
        let s = &self.state;
        if s.open.get() && request != 0 && request == s.request.get() {
            s.sheet.append(delta);
        }
    }

    /// `request` ended; stale ids are ignored.
    pub fn ai_finished(&self, request: u64, result: Result<String, AiError>) {
        let s = &self.state;
        if !s.open.get() || request == 0 || request != s.request.get() {
            return;
        }
        s.request.set(0);
        match result {
            Ok(text) => s.sheet.finish(&clean_output(&text)),
            Err(AiError::Cancelled) => s.sheet.stopped(&clean_output(&s.sheet.text())),
            Err(e) => s.sheet.fail(&e.user_message(), error_opens_settings(&e)),
        }
    }

    /// Close the panel (stopping any request).
    pub fn close(&self) {
        State::close(&self.state);
    }
}

impl State {
    fn emit(&self, action: TextResultAction) {
        (self.sink)(UiEvent::TextResult(action));
    }

    /// Stop the running request, if any.
    fn cancel(&self) {
        let id = self.request.replace(0);
        if id != 0 {
            self.emit(TextResultAction::Cancel(id));
        }
    }

    fn copy_text(this: &Rc<State>) {
        let text = this.text.borrow().clone();
        if !text.trim().is_empty() {
            lumora_capture::clipboard::copy_text(&text);
            this.sheet.note(tr(Msg::AiCopied));
        }
    }

    /// Run `kind`: the on-device result first, then AI (when on).
    fn run(this: &Rc<State>, kind: AiTaskKind) {
        if !this.open.get() {
            return;
        }
        this.cancel();
        this.kind.set(Some(kind));
        let (local, input) = {
            let (boxes, text) = (this.boxes.borrow(), this.text.borrow());
            (local_result(kind, &boxes, &text), ai_input(kind, &boxes, &text))
        };
        let title = kind.title();
        this.sheet.set_csv_visible(kind.is_table());
        this.sheet.set_monospace(kind.needs_boxes());
        if !this.ai_enabled.get() {
            match local {
                Some(l) => this.sheet.show_result(&title, &l, tr(Msg::TextLocalResult)),
                None => {
                    this.sheet.begin(&title, Msg::AiWriting, false);
                    this.sheet.fail(tr(Msg::TextAiOffHint), true);
                }
            }
            return;
        }
        let request = this.next.get();
        this.next.set(request + 1);
        this.request.set(request);
        match &local {
            Some(l) => this.sheet.begin_refine(&title, l),
            None => this.sheet.begin(&title, Msg::AiWriting, false),
        }
        this.emit(TextResultAction::Run(TextAiRequest { request, kind, input }));
    }

    fn show_translate_menu(this: &Rc<State>) {
        let mtm = this.mtm;
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(tr(Msg::AiTaskTranslate)));
        let mut targets = Vec::new();
        for lang in Language::ALL {
            let weak = Rc::downgrade(this);
            let target = ActionTarget::new(mtm, move |_| {
                if let Some(s) = weak.upgrade() {
                    State::run(&s, AiTaskKind::Translate(lang));
                }
            });
            // SAFETY: the target is kept alive in `menu_targets`.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(language_name(lang)),
                    Some(ActionTarget::action()),
                    &NSString::new(),
                )
            };
            unsafe { item.setTarget(Some(&target)) };
            menu.addItem(&item);
            targets.push(target);
        }
        *this.menu_targets.borrow_mut() = targets;
        let b = &this.translate;
        let at = if b.isFlipped() { CGPoint::new(0.0, b.bounds().size.height + 4.0) } else { CGPoint::new(0.0, -4.0) };
        menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(b));
    }

    fn sheet_event(this: &Rc<State>, event: SheetEvent) {
        match event {
            SheetEvent::Stop => {
                this.cancel();
                this.sheet.stopped(&clean_output(&this.sheet.text()));
            }
            SheetEvent::Retry => match this.kind.get() {
                Some(k) => State::run(this, k),
                None => {
                    let text = this.text.borrow().clone();
                    this.sheet.show_result(tr(Msg::TextResultTitle), &text, "");
                }
            },
            SheetEvent::Copy => {
                let text = this.sheet.text();
                if !text.trim().is_empty() {
                    lumora_capture::clipboard::copy_text(&text);
                    this.sheet.note(tr(Msg::AiCopied));
                }
            }
            SheetEvent::CopyCsv => {
                let text = this.sheet.text();
                if !text.trim().is_empty() {
                    lumora_capture::clipboard::copy_text(&table_csv(&text));
                    this.sheet.note(tr(Msg::AiCopied));
                }
            }
            SheetEvent::OpenSettings => this.emit(TextResultAction::OpenAiSettings),
            SheetEvent::Insert => {}
            SheetEvent::Close => State::close(this),
        }
    }

    fn close(this: &Rc<State>) {
        if !this.open.replace(false) {
            return;
        }
        this.cancel();
        this.kind.set(None);
        this.panel.orderOut(None);
        this.emit(TextResultAction::Closed);
    }
}

//! The tutorial window: a scrolling list of steps (thumbnail, editable
//! title and detail, delete) with Export Markdown, Export HTML and Copy
//! Markdown in a bar along the top.
//!
//! The window owns the [`Tutorial`] while it is open. Every action reads
//! the text fields back into it first and hands the app a copy through
//! [`UiEvent::Tutorial`]; the app writes files and the `.steps.json`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, trf, Msg};
use lumora_core::tutorial::Tutorial;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezelStyle, NSBorderType, NSBox, NSBoxType, NSButton, NSColor, NSFont,
    NSImage, NSImageScaling, NSImageView, NSLineBreakMode, NSResponder, NSScrollView, NSTextField, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSString};

use crate::events::{EventSink, TutorialAction, UiEvent};
use crate::tokens::typography;
use crate::util::{cg_rect, screen_under_mouse, weight, ActionTarget};

const WIDTH: f64 = 760.0;
const HEIGHT: f64 = 640.0;
const BAR: f64 = 52.0;
const PAD: f64 = 20.0;
const MIN_W: f64 = 560.0;
const MIN_H: f64 = 360.0;
/// Narrowest the top-bar summary may get before the window stops shrinking.
const SUMMARY_MIN_W: f64 = 200.0;
const ROW: f64 = 176.0;
const THUMB_W: f64 = 240.0;
const THUMB_H: f64 = 150.0;

type MakeAction = fn(Box<Tutorial>) -> TutorialAction;

define_class!(
    /// Top-down document view for the step list.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraTutorialListView"]
    struct ListView;

    unsafe impl NSObjectProtocol for ListView {}

    impl ListView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl ListView {
    fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

struct DelegateIvars {
    state: RefCell<Weak<State>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraTutorialWindowDelegate"]
    #[ivars = DelegateIvars]
    struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                s.emit(TutorialAction::Closed);
            }
        }
    }
);

impl WindowDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { state: RefCell::new(Weak::new()) });
        unsafe { msg_send![super(this), init] }
    }
}

struct RowFields {
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
}

struct State {
    mtm: MainThreadMarker,
    id: u64,
    sink: EventSink,
    window: Retained<NSWindow>,
    scroll: Retained<NSScrollView>,
    list: Retained<ListView>,
    summary: Retained<NSTextField>,
    tutorial: RefCell<Tutorial>,
    /// Folder the step image paths are relative to.
    base_dir: PathBuf,
    rows: RefCell<Vec<RowFields>>,
    row_targets: RefCell<Vec<Retained<ActionTarget>>>,
    _bar_targets: RefCell<Vec<Retained<ActionTarget>>>,
    _delegate: RefCell<Option<Retained<WindowDelegate>>>,
}

/// An open tutorial.
pub struct TutorialWindow {
    state: Rc<State>,
}

impl std::fmt::Debug for TutorialWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TutorialWindow").field("id", &self.state.id).finish_non_exhaustive()
    }
}

fn button(mtm: MainThreadMarker, title: &str, target: &ActionTarget) -> Retained<NSButton> {
    // SAFETY: the target is kept alive by the window state.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.setBezelStyle(NSBezelStyle::Push);
    b.sizeToFit();
    b
}

impl State {
    /// Read the text fields back into the model.
    fn sync(&self) {
        let rows = self.rows.borrow();
        let mut t = self.tutorial.borrow_mut();
        for (step, row) in t.steps.iter_mut().zip(rows.iter()) {
            step.title = crate::esc_window::field_text(&row.title).trim().to_string();
            step.detail = crate::esc_window::field_text(&row.detail).trim().to_string();
        }
    }

    fn emit(&self, action: impl FnOnce(Box<Tutorial>) -> TutorialAction) {
        self.sync();
        let copy = Box::new(self.tutorial.borrow().clone());
        (self.sink)(UiEvent::Tutorial { id: self.id, action: action(copy) });
    }

    fn update_summary(&self) {
        let t = self.tutorial.borrow();
        let n = t.steps.len();
        let s = if t.ai_written { trf(Msg::TutorialSummaryAi, &[&n]) } else { trf(Msg::TutorialSummary, &[&n]) };
        self.summary.setStringValue(&NSString::from_str(&s));
    }

    fn delete_step(self: &Rc<Self>, index: usize) {
        self.sync();
        self.tutorial.borrow_mut().remove_step(index);
        self.rebuild();
    }

    /// Lay the rows out again from the model.
    fn rebuild(self: &Rc<Self>) {
        let mtm = self.mtm;
        for v in self.list.subviews().iter() {
            v.removeFromSuperview();
        }
        let mut rows = Vec::new();
        let mut targets = Vec::new();
        let width = self.scroll.contentSize().width.max(400.0);
        let steps = self.tutorial.borrow().steps.clone();
        let height = (steps.len() as f64 * ROW + PAD).max(self.scroll.contentSize().height);
        self.list.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, height)));
        let text_x = PAD + THUMB_W + 18.0;
        let text_w = (width - text_x - PAD - 36.0).max(160.0);
        for (i, step) in steps.iter().enumerate() {
            let y = PAD + i as f64 * ROW;
            let wide = NSAutoresizingMaskOptions::ViewWidthSizable;

            // Thumbnail.
            let thumb =
                NSImageView::initWithFrame(NSImageView::alloc(mtm), cg_rect(Rect::new(PAD, y, THUMB_W, THUMB_H)));
            thumb.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            thumb.setWantsLayer(true);
            if let Some(l) = thumb.layer() {
                l.setCornerRadius(8.0);
                l.setMasksToBounds(true);
                l.setBorderWidth(1.0);
                l.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
            }
            if let Some(rel) = step.image.as_deref() {
                let path = self.base_dir.join(rel);
                let img =
                    NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(&path.to_string_lossy()));
                if let Some(img) = img {
                    thumb.setImage(Some(&img));
                }
            }
            self.list.addSubview(&thumb);

            // "Step N" caption.
            let n = NSTextField::labelWithString(&NSString::from_str(&trf(Msg::TutorialStepN, &[&step.index])), mtm);
            n.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::SEMIBOLD)));
            n.setTextColor(Some(&NSColor::controlAccentColor()));
            n.setFrame(cg_rect(Rect::new(text_x, y, text_w, 16.0)));
            n.setAutoresizingMask(wide);
            self.list.addSubview(&n);

            let title = NSTextField::textFieldWithString(&NSString::from_str(&step.title), mtm);
            title.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY + 2.0, weight::SEMIBOLD)));
            title.setPlaceholderString(Some(&NSString::from_str(tr(Msg::TutorialTitlePlaceholder))));
            title.setFrame(cg_rect(Rect::new(text_x, y + 20.0, text_w, 26.0)));
            title.setAutoresizingMask(wide);
            self.list.addSubview(&title);

            let detail = NSTextField::textFieldWithString(&NSString::from_str(&step.detail), mtm);
            detail.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)));
            detail.setPlaceholderString(Some(&NSString::from_str(tr(Msg::TutorialDetailPlaceholder))));
            detail.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
            detail.setUsesSingleLineMode(false);
            if let Some(cell) = detail.cell() {
                cell.setWraps(true);
                cell.setScrollable(false);
            }
            detail.setFrame(cg_rect(Rect::new(text_x, y + 54.0, text_w, THUMB_H - 54.0)));
            detail.setAutoresizingMask(wide);
            self.list.addSubview(&detail);

            // Delete (trash glyph) at the right edge.
            let weak = Rc::downgrade(self);
            let index = step.index;
            let target = ActionTarget::new(mtm, move |_| {
                if let Some(s) = weak.upgrade() {
                    // Defer: the button is removed while rebuilding.
                    let s2 = s.clone();
                    crate::util::schedule_timer(0.0, false, move || {
                        s2.delete_step(index);
                        false
                    });
                }
            });
            let del = match crate::util::symbol("trash", tr(Msg::TutorialDeleteStep)) {
                // SAFETY: target kept alive in `row_targets`.
                Some(img) => unsafe {
                    NSButton::buttonWithImage_target_action(&img, Some(&target), Some(ActionTarget::action()), mtm)
                },
                None => button(mtm, tr(Msg::TutorialDeleteStep), &target),
            };
            del.setBordered(false);
            del.setToolTip(Some(&NSString::from_str(tr(Msg::TutorialDeleteStep))));
            del.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            del.setFrame(cg_rect(Rect::new(width - PAD - 24.0, y + 18.0, 24.0, 24.0)));
            del.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            self.list.addSubview(&del);
            targets.push(target);

            if i + 1 < steps.len() {
                let line = NSBox::initWithFrame(
                    NSBox::alloc(mtm),
                    cg_rect(Rect::new(PAD, y + ROW - 13.0, width - 2.0 * PAD, 1.0)),
                );
                line.setBoxType(NSBoxType::Separator);
                line.setAutoresizingMask(wide);
                self.list.addSubview(&line);
            }
            rows.push(RowFields { title, detail });
        }
        *self.rows.borrow_mut() = rows;
        *self.row_targets.borrow_mut() = targets;
        self.update_summary();
    }
}

impl TutorialWindow {
    /// Open a window for `tutorial`, whose image paths are relative to
    /// `base_dir`. `id` comes back with every [`UiEvent::Tutorial`].
    pub fn show(mtm: MainThreadMarker, id: u64, tutorial: Tutorial, base_dir: PathBuf, sink: EventSink) -> Self {
        let visible = screen_under_mouse(mtm).map(|s| s.visibleFrame()).map(crate::util::rect);
        let (w, h) = match visible {
            Some(v) => (WIDTH.min(v.width() * 0.9), HEIGHT.min(v.height() * 0.9)),
            None => (WIDTH, HEIGHT),
        };
        // Never open smaller than the minimum size (raised below once the
        // action buttons are measured).
        let (w, h) = (w.max(MIN_W), h.max(MIN_H));
        let origin =
            visible.map_or(CGPoint::new(200.0, 200.0), |v| CGPoint::new(v.mid().x - w / 2.0, v.mid().y - h / 2.0));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        // Esc commits a field edit, then closes (⌘W via the main menu).
        let window = crate::esc_window::create(mtm, CGRect::new(origin, CGSize::new(w, h)), style);
        // SAFETY: we own the window through `Retained`.
        unsafe { window.setReleasedWhenClosed(false) };
        let title = if tutorial.title.trim().is_empty() {
            tr(Msg::TutorialWindowTitle).to_string()
        } else {
            format!("{} — {}", tr(Msg::TutorialWindowTitle), tutorial.title.trim())
        };
        window.setTitle(&NSString::from_str(&title));
        window.setContentMinSize(CGSize::new(MIN_W, MIN_H));

        let content = NSView::initWithFrame(NSView::alloc(mtm), CGRect::new(CGPoint::ZERO, CGSize::new(w, h)));

        // Top bar: summary on the left, actions on the right.
        let summary = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        summary.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::MEDIUM)));
        summary.setTextColor(Some(&NSColor::secondaryLabelColor()));
        summary.setFrame(cg_rect(Rect::new(PAD, h - BAR + 16.0, 260.0, 20.0)));
        summary.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&summary);

        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), cg_rect(Rect::new(0.0, 0.0, w, h - BAR)));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let list = ListView::new(mtm, CGRect::new(CGPoint::ZERO, CGSize::new(w, h - BAR)));
        list.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        scroll.setDocumentView(Some(&list));
        content.addSubview(&scroll);

        let sep = NSBox::initWithFrame(NSBox::alloc(mtm), cg_rect(Rect::new(0.0, h - BAR, w, 1.0)));
        sep.setBoxType(NSBoxType::Separator);
        sep.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&sep);

        let delegate = WindowDelegate::new(mtm);
        let state = Rc::new(State {
            mtm,
            id,
            sink,
            window: window.clone(),
            scroll: scroll.clone(),
            list,
            summary,
            tutorial: RefCell::new(tutorial),
            base_dir,
            rows: RefCell::new(Vec::new()),
            row_targets: RefCell::new(Vec::new()),
            _bar_targets: RefCell::new(Vec::new()),
            _delegate: RefCell::new(None),
        });

        // Buttons, right to left.
        let actions: [(Msg, MakeAction); 3] = [
            (Msg::TutorialCopyMarkdown, TutorialAction::CopyMarkdown),
            (Msg::TutorialExportHtml, TutorialAction::ExportHtml),
            (Msg::TutorialExportMarkdown, TutorialAction::ExportMarkdown),
        ];
        let mut x = w - PAD;
        let mut bar_targets = Vec::new();
        for (msg, make) in actions {
            let weak = Rc::downgrade(&state);
            let target = ActionTarget::new(mtm, move |_| {
                if let Some(s) = weak.upgrade() {
                    s.emit(make);
                }
            });
            let b = button(mtm, tr(msg), &target);
            let bw = b.frame().size.width.max(96.0);
            x -= bw;
            b.setFrame(cg_rect(Rect::new(x, h - BAR + 10.0, bw, 32.0)));
            b.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            content.addSubview(&b);
            bar_targets.push(target);
            x -= 8.0;
        }
        *state._bar_targets.borrow_mut() = bar_targets;
        // The summary takes what the buttons leave (and shrinks/grows with
        // the window); the window can't get narrow enough for them to meet.
        let bar_w = (w - PAD) - (x + 8.0);
        let summary_x = PAD;
        let summary_w = (x + 8.0 - PAD - summary_x).max(SUMMARY_MIN_W);
        state.summary.setFrame(cg_rect(Rect::new(summary_x, h - BAR + 16.0, summary_w, 20.0)));
        let min_w = MIN_W.max(bar_w + SUMMARY_MIN_W + 3.0 * PAD);
        window.setContentMinSize(CGSize::new(min_w, MIN_H));

        *delegate.ivars().state.borrow_mut() = Rc::downgrade(&state);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        *state._delegate.borrow_mut() = Some(delegate);
        window.setContentView(Some(&content));
        if w < min_w {
            window.setContentSize(CGSize::new(min_w, h));
        }
        state.rebuild();
        // Scroll to the top (flipped document: origin is the top).
        scroll.contentView().scrollToPoint(CGPoint::ZERO);
        scroll.reflectScrolledClipView(&scroll.contentView());

        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        TutorialWindow { state }
    }

    pub fn focus(&self) {
        NSApplication::sharedApplication(self.state.mtm).activate();
        self.state.window.makeKeyAndOrderFront(None);
    }

    /// The tutorial with the current field contents.
    pub fn tutorial(&self) -> Tutorial {
        self.state.sync();
        self.state.tutorial.borrow().clone()
    }
}

impl Drop for TutorialWindow {
    fn drop(&mut self) {
        self.state.window.setDelegate(None);
        self.state.window.orderOut(None);
    }
}

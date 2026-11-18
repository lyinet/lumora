//! The transcript window: summary and chapters at the top, then the timed
//! segments (time + editable text), with Export SRT, Export VTT, Copy
//! Chapters and Copy Summary in a bar along the top, and the burn-in
//! subtitle style (size, position, background box) with "Burn In
//! Subtitles…" along the bottom; while a burn-in runs the bottom bar shows
//! its progress and Cancel. Esc and ⌘W close it.
//!
//! The window owns the [`Transcript`] while it is open. Every action reads
//! the text fields back into it first and hands the app a copy through
//! [`UiEvent::Transcript`]; the app writes files and the `.transcript.json`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, trf, Msg};
use lumora_core::transcript::burn::{SubtitlePosition, SubtitleSize, SubtitleStyle};
use lumora_core::transcript::chapters::format_chapter_time;
use lumora_core::transcript::Transcript;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezelStyle, NSBorderType, NSBox, NSBoxType, NSButton, NSColor,
    NSControl, NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSLineBreakMode, NSPopUpButton,
    NSProgressIndicator, NSProgressIndicatorStyle, NSResponder, NSScrollView, NSTextField, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSString, NSTimer};

use crate::events::{EventSink, TranscriptAction, UiEvent};
use crate::tokens::typography;
use crate::util::{cg_rect, mono_digits, schedule_timer, screen_under_mouse, weight, ActionTarget};

const WIDTH: f64 = 760.0;
const HEIGHT: f64 = 640.0;
const BAR: f64 = 52.0;
/// Bottom bar: burn-in style and button, or burn-in progress.
const FOOT: f64 = 52.0;
const POPUP_W: f64 = 104.0;
const PAD: f64 = 20.0;
const MIN_W: f64 = 560.0;
const MIN_H: f64 = 360.0;
/// Narrowest the top-bar info line may get before the window stops shrinking.
const INFO_MIN_W: f64 = 120.0;
const HEADING_H: f64 = 28.0;
const SUMMARY_H: f64 = 84.0;
const CHAPTER_ROW: f64 = 28.0;
const SEGMENT_ROW: f64 = 30.0;
const TIME_W: f64 = 96.0;
/// Segment rows built before the window shows; the rest are added in
/// batches on later run-loop turns so a long recording (thousands of
/// segments, two views each) opens at once and stays responsive.
const FIRST_ROWS: usize = 120;
const BATCH_ROWS: usize = 250;

type MakeAction = fn(Box<Transcript>) -> TranscriptAction;

define_class!(
    /// Top-down document view for the transcript.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraTranscriptListView"]
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
    #[name = "LumoraTranscriptWindowDelegate"]
    #[ivars = DelegateIvars]
    struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                s.emit(TranscriptAction::Closed);
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

struct Fields {
    summary: Option<Retained<NSTextField>>,
    chapters: Vec<Retained<NSTextField>>,
    segments: Vec<Retained<NSTextField>>,
}

struct State {
    mtm: MainThreadMarker,
    id: u64,
    sink: EventSink,
    window: Retained<NSWindow>,
    transcript: RefCell<Transcript>,
    fields: RefCell<Fields>,
    rows: RefCell<Option<Rows>>,
    foot: RefCell<Option<Foot>>,
    _bar_targets: RefCell<Vec<Retained<ActionTarget>>>,
    _delegate: RefCell<Option<Retained<WindowDelegate>>>,
}

/// An open transcript.
pub struct TranscriptWindow {
    state: Rc<State>,
}

impl std::fmt::Debug for TranscriptWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptWindow").field("id", &self.state.id).finish_non_exhaustive()
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

fn heading(mtm: MainThreadMarker, text: &str, frame: Rect) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::SEMIBOLD)));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l.setFrame(cg_rect(frame));
    l.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    l
}

fn time_label(mtm: MainThreadMarker, text: &str, frame: Rect) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&mono_digits(typography::CAPTION, weight::MEDIUM)));
    l.setTextColor(Some(&NSColor::controlAccentColor()));
    l.setFrame(cg_rect(frame));
    l
}

fn text_field(mtm: MainThreadMarker, text: &str, frame: Rect) -> Retained<NSTextField> {
    let f = NSTextField::textFieldWithString(&NSString::from_str(text), mtm);
    f.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)));
    f.setFrame(cg_rect(frame));
    f.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    f
}

fn separator(mtm: MainThreadMarker, frame: Rect) -> Retained<NSBox> {
    let line = NSBox::initWithFrame(NSBox::alloc(mtm), cg_rect(frame));
    line.setBoxType(NSBoxType::Separator);
    line.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    line
}

fn read(field: &NSTextField) -> String {
    crate::esc_window::field_text(field).trim().to_string()
}

/// The bottom bar's controls.
struct Foot {
    size: Retained<NSPopUpButton>,
    position: Retained<NSPopUpButton>,
    background: Retained<NSButton>,
    burn: Retained<NSButton>,
    /// Shown while idle.
    style_views: Vec<Retained<NSView>>,
    /// Shown while burning.
    status: Retained<NSTextField>,
    bar: Retained<NSProgressIndicator>,
    cancel: Retained<NSButton>,
    can_burn: bool,
}

impl Foot {
    fn style(&self) -> SubtitleStyle {
        let size =
            SubtitleSize::ALL[(self.size.indexOfSelectedItem().max(0) as usize).min(SubtitleSize::ALL.len() - 1)];
        let position =
            if self.position.indexOfSelectedItem() == 1 { SubtitlePosition::Top } else { SubtitlePosition::Bottom };
        SubtitleStyle { size, position, background: self.background.state() == NSControlStateValueOn }
    }
}

fn size_name(size: SubtitleSize) -> &'static str {
    tr(match size {
        SubtitleSize::Small => Msg::SubtitleSizeSmall,
        SubtitleSize::Medium => Msg::SubtitleSizeMedium,
        SubtitleSize::Large => Msg::SubtitleSizeLarge,
    })
}

fn foot_label(mtm: MainThreadMarker, text: &str, x: f64) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)));
    l.sizeToFit();
    let w = l.frame().size.width;
    l.setFrame(cg_rect(Rect::new(x, (FOOT - 20.0) / 2.0, w, 20.0)));
    l
}

fn foot_popup(mtm: MainThreadMarker, items: &[&str], selected: usize, x: f64) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        cg_rect(Rect::new(x, (FOOT - 26.0) / 2.0, POPUP_W, 26.0)),
        false,
    );
    for i in items {
        p.addItemWithTitle(&NSString::from_str(i));
    }
    p.selectItemAtIndex(selected as isize);
    p
}

fn wire(control: &NSControl, target: &ActionTarget) {
    // SAFETY: the target is kept alive by the window state.
    unsafe {
        control.setTarget(Some(target));
        control.setAction(Some(ActionTarget::action()));
    }
}

/// Where the segment rows go and how many are built.
struct Rows {
    list: Retained<ListView>,
    /// Top of the first segment row.
    top: f64,
    /// Show hours in the time column.
    long: bool,
    built: usize,
    timer: Option<Retained<NSTimer>>,
}

impl State {
    /// Read the text fields back into the model.
    fn sync(&self) {
        let fields = self.fields.borrow();
        let mut t = self.transcript.borrow_mut();
        for (seg, f) in t.segments.iter_mut().zip(fields.segments.iter()) {
            seg.text = read(f);
        }
        if let Some(summary) = t.summary.as_mut() {
            if let Some(f) = &fields.summary {
                summary.summary = read(f);
            }
            for (ch, f) in summary.chapters.iter_mut().zip(fields.chapters.iter()) {
                let v = read(f);
                if !v.is_empty() {
                    ch.title = v;
                }
            }
        }
    }

    fn emit(&self, action: impl FnOnce(Box<Transcript>) -> TranscriptAction) {
        self.sync();
        let copy = Box::new(self.transcript.borrow().clone());
        (self.sink)(UiEvent::Transcript { id: self.id, action: action(copy) });
    }

    /// Lay out the document: summary, chapters, then segments.
    fn build(&self, list: &ListView, width: f64) -> f64 {
        let mtm = self.mtm;
        let t = self.transcript.borrow();
        let inner = (width - 2.0 * PAD).max(200.0);
        let mut y = PAD;
        let mut fields = Fields { summary: None, chapters: Vec::new(), segments: Vec::new() };
        let long = t.segments.iter().any(|s| s.end >= 3600.0);

        if let Some(summary) = &t.summary {
            list.addSubview(&heading(mtm, tr(Msg::TranscriptSummaryHeading), Rect::new(PAD, y, inner, 16.0)));
            y += 20.0;
            let f = text_field(mtm, &summary.summary, Rect::new(PAD, y, inner, SUMMARY_H));
            f.setPlaceholderString(Some(&NSString::from_str(tr(Msg::TranscriptSummaryPlaceholder))));
            f.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
            f.setUsesSingleLineMode(false);
            if let Some(cell) = f.cell() {
                cell.setWraps(true);
                cell.setScrollable(false);
            }
            list.addSubview(&f);
            fields.summary = Some(f);
            y += SUMMARY_H + 14.0;

            if !summary.chapters.is_empty() {
                list.addSubview(&heading(mtm, tr(Msg::TranscriptChaptersHeading), Rect::new(PAD, y, inner, 16.0)));
                y += 20.0;
                let ch_long = summary.chapters.iter().any(|c| c.start >= 3600.0);
                for ch in &summary.chapters {
                    let time = format_chapter_time(ch.start, ch_long);
                    list.addSubview(&time_label(mtm, &time, Rect::new(PAD, y + 4.0, TIME_W, 18.0)));
                    let f = text_field(mtm, &ch.title, Rect::new(PAD + TIME_W, y, inner - TIME_W, 22.0));
                    list.addSubview(&f);
                    fields.chapters.push(f);
                    y += CHAPTER_ROW;
                }
                y += 6.0;
            }
            list.addSubview(&separator(mtm, Rect::new(PAD, y, inner, 1.0)));
            y += 12.0;
        }

        list.addSubview(&heading(mtm, tr(Msg::TranscriptSegmentsHeading), Rect::new(PAD, y, inner, 16.0)));
        y += HEADING_H - 8.0;
        let total = t.segments.len();
        drop(t);
        *self.fields.borrow_mut() = fields;
        *self.rows.borrow_mut() = Some(Rows { list: list.retain(), top: y, long, built: 0, timer: None });
        // Every row has the same height, so the document is sized for all
        // of them up front; rows past the first batch fill in shortly.
        self.build_rows(FIRST_ROWS);
        y + total as f64 * SEGMENT_ROW + PAD
    }

    /// Build up to `n` more segment rows. Returns whether rows remain.
    fn build_rows(&self, n: usize) -> bool {
        let mut rows = self.rows.borrow_mut();
        let Some(rows) = rows.as_mut() else { return false };
        let t = self.transcript.borrow();
        let mut fields = self.fields.borrow_mut();
        let inner = (rows.list.frame().size.width - 2.0 * PAD).max(200.0);
        let end = (rows.built + n).min(t.segments.len());
        for seg in &t.segments[rows.built..end] {
            let y = rows.top + rows.built as f64 * SEGMENT_ROW;
            let time = format_chapter_time(seg.start, rows.long);
            rows.list.addSubview(&time_label(self.mtm, &time, Rect::new(PAD, y + 4.0, TIME_W, 18.0)));
            let f = text_field(self.mtm, &seg.text, Rect::new(PAD + TIME_W, y, inner - TIME_W, 22.0));
            rows.list.addSubview(&f);
            fields.segments.push(f);
            rows.built += 1;
        }
        rows.built < t.segments.len()
    }

    /// Add the remaining rows a batch per run-loop turn.
    fn build_rest(this: &Rc<Self>) {
        let weak = Rc::downgrade(this);
        let timer = schedule_timer(0.0, true, move || match weak.upgrade() {
            Some(s) => s.build_rows(BATCH_ROWS),
            None => false,
        });
        if let Some(rows) = this.rows.borrow_mut().as_mut() {
            rows.timer = Some(timer);
        }
    }

    /// The bottom bar. Returns the width it needs.
    fn build_foot(
        this: &Rc<Self>,
        content: &NSView,
        w: f64,
        style: SubtitleStyle,
        targets: &mut Vec<Retained<ActionTarget>>,
    ) -> f64 {
        let mtm = this.mtm;
        let sep = NSBox::initWithFrame(NSBox::alloc(mtm), cg_rect(Rect::new(0.0, FOOT, w, 1.0)));
        sep.setBoxType(NSBoxType::Separator);
        sep.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        content.addSubview(&sep);

        let mut make = |f: fn(&State)| {
            let weak = Rc::downgrade(this);
            let t = ActionTarget::new(mtm, move |_| {
                if let Some(s) = weak.upgrade() {
                    f(&s);
                }
            });
            targets.push(t.clone());
            t
        };
        let style_changed = make(State::style_changed);
        let burn_t = make(State::burn);
        let cancel_t = make(|s| (s.sink)(UiEvent::Transcript { id: s.id, action: TranscriptAction::CancelBurn }));

        let mut x = PAD;
        let size_label = foot_label(mtm, tr(Msg::SubtitleSizeLabel), x);
        x += size_label.frame().size.width + 6.0;
        let sizes: Vec<&str> = SubtitleSize::ALL.iter().map(|&s| size_name(s)).collect();
        let selected = SubtitleSize::ALL.iter().position(|&s| s == style.size).unwrap_or(1);
        let size = foot_popup(mtm, &sizes, selected, x);
        x += POPUP_W + 14.0;
        let pos_label = foot_label(mtm, tr(Msg::SubtitlePositionLabel), x);
        x += pos_label.frame().size.width + 6.0;
        let position = foot_popup(
            mtm,
            &[tr(Msg::SubtitlePositionBottom), tr(Msg::SubtitlePositionTop)],
            usize::from(style.position == SubtitlePosition::Top),
            x,
        );
        x += POPUP_W + 14.0;
        // SAFETY: the target is kept alive by the window state.
        let background = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str(tr(Msg::SubtitleBackgroundBox)),
                Some(&style_changed),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        background.setState(if style.background { NSControlStateValueOn } else { NSControlStateValueOff });
        background.sizeToFit();
        let bw = background.frame().size.width;
        background.setFrame(cg_rect(Rect::new(x, (FOOT - 20.0) / 2.0, bw, 20.0)));
        x += bw;
        wire(&size, &style_changed);
        wire(&position, &style_changed);

        let burn = button(mtm, tr(Msg::TranscriptBurnIn), &burn_t);
        let burn_w = burn.frame().size.width.max(120.0);
        burn.setFrame(cg_rect(Rect::new(w - PAD - burn_w, (FOOT - 32.0) / 2.0, burn_w, 32.0)));
        burn.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        let can_burn = this.transcript.borrow().segments.iter().any(|s| !s.text.trim().is_empty());
        burn.setEnabled(can_burn);

        let cancel = button(mtm, tr(Msg::HistoryCancel), &cancel_t);
        let cancel_w = cancel.frame().size.width.max(80.0);
        let cancel_x = w - PAD - burn_w - 8.0 - cancel_w;
        cancel.setFrame(cg_rect(Rect::new(cancel_x, (FOOT - 32.0) / 2.0, cancel_w, 32.0)));
        cancel.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        status.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::REGULAR)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let prog_w = (cancel_x - 12.0 - PAD).max(80.0);
        status.setFrame(cg_rect(Rect::new(PAD, 28.0, prog_w, 16.0)));
        status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let bar = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            cg_rect(Rect::new(PAD, 10.0, prog_w, 16.0)),
        );
        bar.setStyle(NSProgressIndicatorStyle::Bar);
        bar.setIndeterminate(false);
        bar.setMinValue(0.0);
        bar.setMaxValue(1.0);
        bar.setControlSize(NSControlSize::Small);
        bar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

        let views: [&NSView; 5] = [&size_label, &size, &pos_label, &position, &background];
        let style_views: Vec<Retained<NSView>> = views.iter().map(|v| v.retain()).collect();
        for v in &style_views {
            content.addSubview(v);
        }
        for v in [&*status as &NSView, &bar, &cancel] {
            v.setHidden(true);
            content.addSubview(v);
        }
        content.addSubview(&burn);
        let need = x + 16.0 + burn_w + PAD;
        *this.foot.borrow_mut() =
            Some(Foot { size, position, background, burn, style_views, status, bar, cancel, can_burn });
        need
    }

    fn style(&self) -> SubtitleStyle {
        self.foot.borrow().as_ref().map(Foot::style).unwrap_or_default()
    }

    fn style_changed(&self) {
        let style = self.style();
        (self.sink)(UiEvent::Transcript { id: self.id, action: TranscriptAction::SubtitleStyle(style) });
    }

    fn burn(&self) {
        let style = self.style();
        self.emit(|t| TranscriptAction::BurnIn(t, style));
    }

    fn info(&self) -> String {
        trf(Msg::TranscriptInfo, &[&self.transcript.borrow().segments.len()])
    }
}

impl TranscriptWindow {
    /// Open a window for `transcript`; `title` names the recording. `id`
    /// comes back with every [`UiEvent::Transcript`].
    pub fn show(
        mtm: MainThreadMarker,
        id: u64,
        transcript: Transcript,
        title: &str,
        subtitle_style: SubtitleStyle,
        sink: EventSink,
    ) -> Self {
        let visible = screen_under_mouse(mtm).map(|s| s.visibleFrame()).map(crate::util::rect);
        let (w, h) = match visible {
            Some(v) => (WIDTH.min(v.width() * 0.9), HEIGHT.min(v.height() * 0.9)),
            None => (WIDTH, HEIGHT),
        };
        let (w, h) = (w.max(MIN_W), h.max(MIN_H));
        let origin =
            visible.map_or(CGPoint::new(200.0, 200.0), |v| CGPoint::new(v.mid().x - w / 2.0, v.mid().y - h / 2.0));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let window = crate::esc_window::create(mtm, CGRect::new(origin, CGSize::new(w, h)), style);
        // SAFETY: we own the window through `Retained`.
        unsafe { window.setReleasedWhenClosed(false) };
        let win_title = if title.trim().is_empty() {
            tr(Msg::TranscriptWindowTitle).to_string()
        } else {
            format!("{} — {}", tr(Msg::TranscriptWindowTitle), title.trim())
        };
        window.setTitle(&NSString::from_str(&win_title));

        let content = NSView::initWithFrame(NSView::alloc(mtm), CGRect::new(CGPoint::ZERO, CGSize::new(w, h)));

        let info = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        info.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::MEDIUM)));
        info.setTextColor(Some(&NSColor::secondaryLabelColor()));
        info.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        info.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&info);

        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), cg_rect(Rect::new(0.0, FOOT, w, h - BAR - FOOT)));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let list = ListView::new(mtm, CGRect::new(CGPoint::ZERO, CGSize::new(w, h - BAR - FOOT)));
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
            transcript: RefCell::new(transcript),
            fields: RefCell::new(Fields { summary: None, chapters: Vec::new(), segments: Vec::new() }),
            rows: RefCell::new(None),
            foot: RefCell::new(None),
            _bar_targets: RefCell::new(Vec::new()),
            _delegate: RefCell::new(None),
        });

        // Buttons, right to left.
        let has_summary = state.transcript.borrow().summary.is_some();
        let mut actions: Vec<(Msg, MakeAction)> = Vec::new();
        if has_summary {
            actions.push((Msg::TranscriptCopySummary, TranscriptAction::CopySummary));
            actions.push((Msg::TranscriptCopyChapters, TranscriptAction::CopyChapters));
        }
        actions.push((Msg::TranscriptExportVtt, TranscriptAction::ExportVtt));
        actions.push((Msg::TranscriptExportSrt, TranscriptAction::ExportSrt));
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
            let bw = b.frame().size.width.max(88.0);
            x -= bw;
            b.setFrame(cg_rect(Rect::new(x, h - BAR + 10.0, bw, 32.0)));
            b.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            content.addSubview(&b);
            bar_targets.push(target);
            x -= 8.0;
        }
        let foot_w = State::build_foot(&state, &content, w, subtitle_style, &mut bar_targets);
        *state._bar_targets.borrow_mut() = bar_targets;
        let bar_w = (w - PAD) - (x + 8.0);
        let info_w = (x + 8.0 - 2.0 * PAD).max(INFO_MIN_W);
        info.setFrame(cg_rect(Rect::new(PAD, h - BAR + 16.0, info_w, 20.0)));
        info.setStringValue(&NSString::from_str(&state.info()));
        let min_w = MIN_W.max(bar_w + INFO_MIN_W + 3.0 * PAD).max(foot_w);
        window.setContentMinSize(CGSize::new(min_w, MIN_H));

        *delegate.ivars().state.borrow_mut() = Rc::downgrade(&state);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        *state._delegate.borrow_mut() = Some(delegate);
        window.setContentView(Some(&content));
        let w = if w < min_w {
            window.setContentSize(CGSize::new(min_w, h));
            min_w
        } else {
            w
        };

        let doc_h = state.build(&list, w).max(h - BAR - FOOT);
        list.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(w, doc_h)));
        // Scroll to the top (flipped document: origin is the top).
        scroll.contentView().scrollToPoint(CGPoint::ZERO);
        scroll.reflectScrolledClipView(&scroll.contentView());

        if state.rows.borrow().as_ref().is_some_and(|r| r.built < state.transcript.borrow().segments.len()) {
            State::build_rest(&state);
        }

        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        TranscriptWindow { state }
    }

    pub fn focus(&self) {
        NSApplication::sharedApplication(self.state.mtm).activate();
        self.state.window.makeKeyAndOrderFront(None);
    }

    /// `Some(status)` swaps the burn-in style for a progress bar and
    /// Cancel and disables the button; `None` restores it.
    pub fn set_busy(&self, status: Option<&str>) {
        let foot = self.state.foot.borrow();
        let Some(f) = foot.as_ref() else { return };
        let busy = status.is_some();
        for v in &f.style_views {
            v.setHidden(busy);
        }
        for v in [&*f.status as &NSView, &f.bar, &f.cancel] {
            v.setHidden(!busy);
        }
        f.status.setStringValue(&NSString::from_str(status.unwrap_or("")));
        f.bar.setDoubleValue(0.0);
        f.burn.setEnabled(!busy && f.can_burn);
    }

    /// Burn-in progress, 0…1.
    pub fn set_progress(&self, fraction: f32) {
        if let Some(f) = self.state.foot.borrow().as_ref() {
            f.bar.setDoubleValue(f64::from(fraction).clamp(0.0, 1.0));
        }
    }

    /// The transcript with the current field contents.
    pub fn transcript(&self) -> Transcript {
        self.state.sync();
        self.state.transcript.borrow().clone()
    }
}

impl Drop for TranscriptWindow {
    fn drop(&mut self) {
        if let Some(timer) = self.state.rows.borrow_mut().as_mut().and_then(|r| r.timer.take()) {
            timer.invalidate();
        }
        self.state.window.setDelegate(None);
        self.state.window.orderOut(None);
    }
}

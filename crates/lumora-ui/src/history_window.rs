//! The capture history window (⌥⇧H): a grid of thumbnails of past
//! screenshots, recordings and `.lumora` documents, with a type filter and
//! a search field (file name and recognised text) along the top.
//!
//! Each cell has Reopen in editor (recordings: Play), Copy, Show in Finder
//! and Move to Trash; clicking the thumbnail reopens too. The app owns the
//! index and the files: the window shows what [`HistoryWindow::set_items`]
//! gives it, asks for thumbnails it hasn't got
//! ([`HistoryAction::NeedThumbnails`]) and reports every button as a
//! [`UiEvent::History`]. Esc (when no field is being edited) and ⌘W close it.
//!
//! Cells are built a page at a time ([`PAGE`] per "Show More") so a folder
//! with thousands of captures still opens at once.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use lumora_core::format;
use lumora_core::geometry::Rect;
use lumora_core::history::{Filter, Grid, HistoryItem, ItemKind};
use lumora_core::i18n::{tr, trf, Msg};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezelStyle, NSBorderType, NSButton, NSColor, NSFont, NSImage,
    NSImageScaling, NSImageView, NSLineBreakMode, NSResponder, NSScrollView, NSSearchField, NSSegmentSwitchTracking,
    NSSegmentedControl, NSTextAlignment, NSTextField, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{
    NSArray, NSDate, NSDateFormatter, NSDateFormatterStyle, NSNotification, NSObject, NSObjectProtocol, NSString,
};

use crate::events::{EventSink, HistoryAction, ThumbRequest, UiEvent};
use crate::tokens::typography;
use crate::util::{cg_rect, screen_under_mouse, weight, ActionTarget};

const WIDTH: f64 = 900.0;
const HEIGHT: f64 = 640.0;
const BAR: f64 = 52.0;
const BANNER: f64 = 40.0;
const PAD: f64 = 20.0;
const MIN_W: f64 = 600.0;
const MIN_H: f64 = 400.0;
const SEARCH_W: f64 = 240.0;
/// Cells built per page ("Show More" adds another).
pub const PAGE: usize = 120;
const BUTTON: f64 = 24.0;

define_class!(
    /// A top-down (flipped) view: the grid document and each cell.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraHistoryFlippedView"]
    struct FlippedView;

    unsafe impl NSObjectProtocol for FlippedView {}

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

struct DelegateIvars {
    state: RefCell<Weak<State>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraHistoryWindowDelegate"]
    #[ivars = DelegateIvars]
    struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                (s.sink)(UiEvent::History(HistoryAction::Closed));
            }
        }

        /// Re-flow the grid once a live resize ends (and for zoom / tiling,
        /// which resize without a live phase).
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                if !s.window.inLiveResize() {
                    s.relayout_if_needed();
                }
            }
        }

        #[unsafe(method(windowDidEndLiveResize:))]
        fn window_did_end_live_resize(&self, _notification: &NSNotification) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                s.relayout_if_needed();
            }
        }
    }
);

impl WindowDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { state: RefCell::new(Weak::new()) });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

struct State {
    mtm: MainThreadMarker,
    sink: EventSink,
    window: Retained<NSWindow>,
    scroll: Retained<NSScrollView>,
    grid: Retained<FlippedView>,
    count: Retained<NSTextField>,
    empty_title: Retained<NSTextField>,
    empty_detail: Retained<NSTextField>,
    banner: Retained<NSView>,
    banner_label: Retained<NSTextField>,
    items: RefCell<Vec<HistoryItem>>,
    filter: Cell<Filter>,
    query: RefCell<String>,
    /// Cells shown (a multiple of [`PAGE`]).
    limit: Cell<usize>,
    loading: Cell<bool>,
    /// Items older than the retention setting, and its days.
    expired: Cell<(usize, u64)>,
    /// Grid width the cells were last laid out for.
    laid_out_width: Cell<f64>,
    thumbs: RefCell<HashMap<String, Retained<NSImage>>>,
    requested: RefCell<HashSet<String>>,
    /// Thumbnail buttons by thumbnail key, for late images.
    thumb_buttons: RefCell<HashMap<String, Vec<Retained<NSButton>>>>,
    cell_targets: RefCell<Vec<Retained<ActionTarget>>>,
    _bar_targets: RefCell<Vec<Retained<ActionTarget>>>,
    _delegate: RefCell<Option<Retained<WindowDelegate>>>,
    dates: Retained<NSDateFormatter>,
}

/// The open history window.
pub struct HistoryWindow {
    state: Rc<State>,
}

impl std::fmt::Debug for HistoryWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryWindow").finish_non_exhaustive()
    }
}

fn symbol_button(mtm: MainThreadMarker, symbol: &str, tip: &str, target: &ActionTarget) -> Retained<NSButton> {
    let b = match crate::util::symbol(symbol, tip) {
        // SAFETY: the target is kept alive by the window state.
        Some(img) => unsafe {
            NSButton::buttonWithImage_target_action(&img, Some(target), Some(ActionTarget::action()), mtm)
        },
        // SAFETY: as above.
        None => unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(tip),
                Some(target),
                Some(ActionTarget::action()),
                mtm,
            )
        },
    };
    b.setBordered(false);
    b.setToolTip(Some(&NSString::from_str(tip)));
    b.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    b
}

fn kind_symbol(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Screenshot => "photo",
        ItemKind::Recording => "video.fill",
        ItemKind::Document => "pencil.and.outline",
    }
}

impl State {
    fn emit(&self, action: HistoryAction) {
        (self.sink)(UiEvent::History(action));
    }

    /// The items that pass the filter and the search, newest first.
    fn visible(&self) -> Vec<HistoryItem> {
        let (filter, query) = (self.filter.get(), self.query.borrow());
        self.items.borrow().iter().filter(|i| i.matches(filter, &query)).cloned().collect()
    }

    fn meta_line(&self, item: &HistoryItem) -> String {
        let date = NSDate::dateWithTimeIntervalSince1970(item.created_at as f64);
        let mut parts = vec![self.dates.stringFromDate(&date).to_string(), format::file_size(item.size_bytes)];
        if let Some((w, h)) = item.dimensions() {
            parts.push(format::dimensions(w, h));
        }
        parts.join(" · ")
    }

    fn update_banner(&self) {
        let (n, days) = self.expired.get();
        let show = n > 0;
        self.banner.setHidden(!show);
        if show {
            self.banner_label.setStringValue(&NSString::from_str(&trf(Msg::HistoryRetentionBanner, &[&n, &days])));
        }
        // The grid takes the banner's room when it is hidden.
        let content = self.window.contentView().map(|v| v.frame().size).unwrap_or(CGSize::new(WIDTH, HEIGHT));
        let top = BAR + if show { BANNER } else { 0.0 };
        self.scroll.setFrame(cg_rect(Rect::new(0.0, 0.0, content.width, (content.height - top).max(0.0))));
    }

    fn relayout_if_needed(self: &Rc<Self>) {
        let width = self.scroll.contentSize().width;
        if (width - self.laid_out_width.get()).abs() >= 1.0 {
            self.rebuild();
        }
    }

    /// Lay the cells out again from the items, filter and search.
    fn rebuild(self: &Rc<Self>) {
        let mtm = self.mtm;
        for v in self.grid.subviews().iter() {
            v.removeFromSuperview();
        }
        self.thumb_buttons.borrow_mut().clear();
        let mut targets = Vec::new();
        let width = self.scroll.contentSize().width.max(MIN_W - 40.0);
        self.laid_out_width.set(width);
        let grid = Grid::new(width);
        let visible = self.visible();
        let total = visible.len();
        let shown = total.min(self.limit.get());
        let more = shown < total;
        let height = grid.content_height(shown, if more { 48.0 } else { 0.0 }).max(self.scroll.contentSize().height);
        self.grid.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(width, height)));

        // Count and empty state.
        self.count.setStringValue(&NSString::from_str(&trf(Msg::HistoryCount, &[&total])));
        let empty = total == 0;
        self.empty_title.setHidden(!empty);
        self.empty_detail.setHidden(!empty || self.loading.get());
        if empty {
            let (title, detail) = if self.loading.get() {
                (tr(Msg::HistoryLoading), "")
            } else if self.items.borrow().is_empty() {
                (tr(Msg::HistoryEmptyTitle), tr(Msg::HistoryEmptyDetail))
            } else {
                (tr(Msg::HistoryNoMatches), "")
            };
            self.empty_title.setStringValue(&NSString::from_str(title));
            self.empty_detail.setStringValue(&NSString::from_str(detail));
        }

        let mut wanted = Vec::new();
        let thumb_h = grid.thumb_height();
        for (i, item) in visible.iter().take(shown).enumerate() {
            let (x, y) = grid.origin(i);
            let cw = grid.cell_width;
            let cell = FlippedView::new(mtm, cg_rect(Rect::new(x, y, cw, grid.cell_height)));

            // Thumbnail: a borderless image button (click = reopen / play).
            let path = item.path.clone();
            let weak = Rc::downgrade(self);
            let open = ActionTarget::new(mtm, move |_| {
                if let Some(s) = weak.upgrade() {
                    s.emit(HistoryAction::Reopen(path.clone()));
                }
            });
            let open_tip = if item.is_editable() { tr(Msg::HistoryReopen) } else { tr(Msg::HistoryPlay) };
            let key = item.thumb_key();
            let image = self
                .thumbs
                .borrow()
                .get(&key)
                .cloned()
                .or_else(|| crate::util::symbol(kind_symbol(item.kind), open_tip));
            let thumb = match image {
                // SAFETY: target kept alive in `cell_targets`.
                Some(img) => unsafe {
                    NSButton::buttonWithImage_target_action(&img, Some(&open), Some(ActionTarget::action()), mtm)
                },
                // SAFETY: as above.
                None => unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &NSString::from_str(""),
                        Some(&open),
                        Some(ActionTarget::action()),
                        mtm,
                    )
                },
            };
            thumb.setBordered(false);
            thumb.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            thumb.setToolTip(Some(&NSString::from_str(open_tip)));
            thumb.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
            thumb.setFrame(cg_rect(Rect::new(0.0, 0.0, cw, thumb_h)));
            thumb.setWantsLayer(true);
            if let Some(l) = thumb.layer() {
                l.setCornerRadius(8.0);
                l.setMasksToBounds(true);
                l.setBorderWidth(1.0);
                l.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
                l.setBackgroundColor(Some(&NSColor::quaternaryLabelColor().CGColor()));
            }
            cell.addSubview(&thumb);
            targets.push(open);
            if !self.thumbs.borrow().contains_key(&key) {
                if self.requested.borrow_mut().insert(key.clone()) {
                    wanted.push(ThumbRequest { key: key.clone(), path: item.path.clone(), kind: item.kind });
                }
                self.thumb_buttons.borrow_mut().entry(key).or_default().push(thumb.clone());
            }

            // Kind badge (top-left of the thumbnail) for recordings and
            // documents.
            if item.kind != ItemKind::Screenshot || item.document.is_some() {
                let symbol = if item.kind == ItemKind::Recording { "video.fill" } else { "pencil.and.outline" };
                if let Some(img) = crate::util::symbol(symbol, "") {
                    let badge = NSImageView::imageViewWithImage(&img, mtm);
                    badge.setFrame(cg_rect(Rect::new(6.0, 6.0, 22.0, 18.0)));
                    badge.setContentTintColor(Some(&NSColor::whiteColor()));
                    badge.setWantsLayer(true);
                    if let Some(l) = badge.layer() {
                        l.setCornerRadius(4.0);
                        l.setBackgroundColor(Some(&NSColor::colorWithWhite_alpha(0.0, 0.55).CGColor()));
                    }
                    cell.addSubview(&badge);
                }
            }

            let name = NSTextField::labelWithString(&NSString::from_str(&item.file_name()), mtm);
            name.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::MEDIUM)));
            name.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
            name.setToolTip(Some(&NSString::from_str(&item.path.display().to_string())));
            name.setFrame(cg_rect(Rect::new(0.0, thumb_h + 6.0, cw, 18.0)));
            cell.addSubview(&name);

            let meta = NSTextField::labelWithString(&NSString::from_str(&self.meta_line(item)), mtm);
            meta.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::REGULAR)));
            meta.setTextColor(Some(&NSColor::secondaryLabelColor()));
            meta.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            meta.setFrame(cg_rect(Rect::new(0.0, thumb_h + 25.0, cw, 16.0)));
            cell.addSubview(&meta);

            // Buttons: reopen / play, copy, show in Finder, trash.
            let reopen_symbol = if item.is_editable() { "square.and.pencil" } else { "play.circle" };
            type Make = fn(PathBuf) -> HistoryAction;
            let buttons: [(&str, &str, Make); 4] = [
                (reopen_symbol, open_tip, HistoryAction::Reopen),
                ("doc.on.doc", tr(Msg::Copy), HistoryAction::Copy),
                ("folder", tr(Msg::HistoryShowInFinder), HistoryAction::Reveal),
                ("trash", tr(Msg::HistoryMoveToTrash), HistoryAction::Delete),
            ];
            let mut bx = 0.0;
            for (symbol, tip, make) in buttons {
                let path = item.path.clone();
                let weak = Rc::downgrade(self);
                let t = ActionTarget::new(mtm, move |_| {
                    if let Some(s) = weak.upgrade() {
                        s.emit(make(path.clone()));
                    }
                });
                let b = symbol_button(mtm, symbol, tip, &t);
                b.setFrame(cg_rect(Rect::new(bx, thumb_h + 44.0, BUTTON + 4.0, BUTTON)));
                cell.addSubview(&b);
                targets.push(t);
                bx += BUTTON + 10.0;
            }
            self.grid.addSubview(&cell);
        }

        if more {
            let weak = Rc::downgrade(self);
            let t = ActionTarget::new(mtm, move |_| {
                if let Some(s) = weak.upgrade() {
                    // Defer: the button is removed while rebuilding.
                    let s2 = s.clone();
                    crate::util::schedule_timer(0.0, false, move || {
                        s2.limit.set(s2.limit.get() + PAGE);
                        s2.rebuild();
                        false
                    });
                }
            });
            // SAFETY: target kept alive in `cell_targets`.
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(tr(Msg::HistoryShowMore)),
                    Some(&t),
                    Some(ActionTarget::action()),
                    mtm,
                )
            };
            b.setBezelStyle(NSBezelStyle::Push);
            b.sizeToFit();
            let bw = b.frame().size.width.max(120.0);
            let top = grid.content_height(shown, 0.0) - grid.margin + 8.0;
            b.setFrame(cg_rect(Rect::new((width - bw) / 2.0, top, bw, 32.0)));
            self.grid.addSubview(&b);
            targets.push(t);
        }
        *self.cell_targets.borrow_mut() = targets;
        if !wanted.is_empty() {
            self.emit(HistoryAction::NeedThumbnails(wanted));
        }
    }
}

impl HistoryWindow {
    /// Open the window (empty, "Loading…" until [`Self::set_items`]).
    pub fn show(mtm: MainThreadMarker, sink: EventSink) -> Self {
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
        // Esc closes (after ending a search edit); ⌘W via the main menu.
        let window = crate::esc_window::create(mtm, CGRect::new(origin, CGSize::new(w, h)), style);
        // SAFETY: we own the window through `Retained`.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(tr(Msg::CaptureHistory)));
        window.setContentMinSize(CGSize::new(MIN_W, MIN_H));
        window.setFrameAutosaveName(&NSString::from_str("LumoraHistoryWindow"));

        let content = NSView::initWithFrame(NSView::alloc(mtm), CGRect::new(CGPoint::ZERO, CGSize::new(w, h)));
        let top_pin = NSAutoresizingMaskOptions::ViewMinYMargin;

        // Top bar: filter · count … search.
        let filter_target_slot: Rc<RefCell<Weak<State>>> = Rc::new(RefCell::new(Weak::new()));
        let slot = filter_target_slot.clone();
        let filter_target = ActionTarget::new(mtm, move |sender: &AnyObject| {
            let Some(s) = slot.borrow().upgrade() else { return };
            if let Some(seg) = sender.downcast_ref::<NSSegmentedControl>() {
                let i = seg.selectedSegment().max(0) as usize;
                s.filter.set(Filter::ALL[i.min(Filter::ALL.len() - 1)]);
                s.limit.set(PAGE);
                s.rebuild();
            }
        });
        let labels: Vec<Retained<NSString>> = Filter::ALL.iter().map(|f| NSString::from_str(tr(f.msg()))).collect();
        // SAFETY: the target is kept alive by the window state.
        let seg = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &NSArray::from_retained_slice(&labels),
                NSSegmentSwitchTracking::SelectOne,
                Some(&filter_target),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        seg.setSelectedSegment(0);
        seg.sizeToFit();
        let seg_w = seg.frame().size.width;
        seg.setFrame(cg_rect(Rect::new(PAD, h - BAR + 13.0, seg_w, 26.0)));
        seg.setAutoresizingMask(top_pin);
        content.addSubview(&seg);

        let count = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        count.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::MEDIUM)));
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let count_x = PAD + seg_w + 12.0;
        count.setFrame(cg_rect(Rect::new(
            count_x,
            h - BAR + 17.0,
            (w - count_x - SEARCH_W - 2.0 * PAD).max(40.0),
            18.0,
        )));
        count.setAutoresizingMask(top_pin | NSAutoresizingMaskOptions::ViewWidthSizable);
        content.addSubview(&count);

        let search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            cg_rect(Rect::new(w - PAD - SEARCH_W, h - BAR + 13.0, SEARCH_W, 26.0)),
        );
        search.setPlaceholderString(Some(&NSString::from_str(tr(Msg::HistorySearchPlaceholder))));
        search.setAutoresizingMask(top_pin | NSAutoresizingMaskOptions::ViewMinXMargin);
        let slot = filter_target_slot.clone();
        let search_target = ActionTarget::new(mtm, move |sender: &AnyObject| {
            let Some(s) = slot.borrow().upgrade() else { return };
            if let Some(f) = sender.downcast_ref::<NSSearchField>() {
                *s.query.borrow_mut() = f.stringValue().to_string();
                s.limit.set(PAGE);
                s.rebuild();
            }
        });
        // SAFETY: the target is kept alive by the window state.
        unsafe {
            search.setTarget(Some(&search_target));
            search.setAction(Some(ActionTarget::action()));
        }
        content.addSubview(&search);

        // Retention banner under the bar (hidden unless something is old).
        let banner = NSView::initWithFrame(NSView::alloc(mtm), cg_rect(Rect::new(0.0, h - BAR - BANNER, w, BANNER)));
        banner.setAutoresizingMask(top_pin | NSAutoresizingMaskOptions::ViewWidthSizable);
        banner.setWantsLayer(true);
        if let Some(l) = banner.layer() {
            l.setBackgroundColor(Some(&NSColor::systemYellowColor().colorWithAlphaComponent(0.14).CGColor()));
        }
        let slot = filter_target_slot.clone();
        let trash_target = ActionTarget::new(mtm, move |_| {
            if let Some(s) = slot.borrow().upgrade() {
                s.emit(HistoryAction::TrashExpired);
            }
        });
        // SAFETY: the target is kept alive by the window state.
        let trash = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(tr(Msg::HistoryRetentionButton)),
                Some(&trash_target),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        trash.setBezelStyle(NSBezelStyle::Push);
        trash.sizeToFit();
        let tw = trash.frame().size.width;
        trash.setFrame(cg_rect(Rect::new(w - PAD - tw, 5.0, tw, 30.0)));
        trash.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        banner.addSubview(&trash);
        let banner_label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        banner_label.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)));
        banner_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        banner_label.setFrame(cg_rect(Rect::new(PAD, 11.0, (w - 3.0 * PAD - tw).max(80.0), 18.0)));
        banner_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        banner.addSubview(&banner_label);
        banner.setHidden(true);
        content.addSubview(&banner);

        // The grid.
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), cg_rect(Rect::new(0.0, 0.0, w, h - BAR)));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let grid = FlippedView::new(mtm, CGRect::new(CGPoint::ZERO, CGSize::new(w, h - BAR)));
        grid.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        scroll.setDocumentView(Some(&grid));
        content.addSubview(&scroll);

        // Empty state, centred over the grid.
        let centred = |text: &str, size: f64, wt: f64, y: f64| {
            let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
            l.setFont(Some(&NSFont::systemFontOfSize_weight(size, wt)));
            l.setAlignment(NSTextAlignment::Center);
            l.setTextColor(Some(&NSColor::secondaryLabelColor()));
            l.setFrame(cg_rect(Rect::new(PAD, y, w - 2.0 * PAD, size + 8.0)));
            l.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewMinYMargin
                    | NSAutoresizingMaskOptions::ViewMaxYMargin,
            );
            content.addSubview(&l);
            l
        };
        let mid = (h - BAR) / 2.0;
        let empty_title = centred(tr(Msg::HistoryLoading), typography::BODY + 5.0, weight::SEMIBOLD, mid + 4.0);
        let empty_detail = centred("", typography::BODY, weight::REGULAR, mid - 22.0);
        empty_detail.setHidden(true);

        let dates = NSDateFormatter::new();
        dates.setDateStyle(NSDateFormatterStyle::MediumStyle);
        dates.setTimeStyle(NSDateFormatterStyle::ShortStyle);

        let delegate = WindowDelegate::new(mtm);
        let state = Rc::new(State {
            mtm,
            sink,
            window: window.clone(),
            scroll: scroll.clone(),
            grid,
            count,
            empty_title,
            empty_detail,
            banner,
            banner_label,
            items: RefCell::new(Vec::new()),
            filter: Cell::new(Filter::All),
            query: RefCell::new(String::new()),
            limit: Cell::new(PAGE),
            loading: Cell::new(true),
            expired: Cell::new((0, 0)),
            laid_out_width: Cell::new(0.0),
            thumbs: RefCell::new(HashMap::new()),
            requested: RefCell::new(HashSet::new()),
            thumb_buttons: RefCell::new(HashMap::new()),
            cell_targets: RefCell::new(Vec::new()),
            _bar_targets: RefCell::new(vec![filter_target, search_target, trash_target]),
            _delegate: RefCell::new(None),
            dates,
        });
        *filter_target_slot.borrow_mut() = Rc::downgrade(&state);
        *delegate.ivars().state.borrow_mut() = Rc::downgrade(&state);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        *state._delegate.borrow_mut() = Some(delegate);
        window.setContentView(Some(&content));
        state.update_banner();
        state.rebuild();

        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        // Nothing is being edited, so the first Esc closes the window; click
        // the search field to type.
        window.makeFirstResponder(None);
        HistoryWindow { state }
    }

    pub fn focus(&self) {
        NSApplication::sharedApplication(self.state.mtm).activate();
        self.state.window.makeKeyAndOrderFront(None);
    }

    /// Show `items` (newest first). `expired` = how many are older than the
    /// retention setting's `days` (0 hides the banner).
    pub fn set_items(&self, items: Vec<HistoryItem>, expired: usize, days: u64) {
        let s = &self.state;
        s.loading.set(false);
        *s.items.borrow_mut() = items;
        s.expired.set((expired, days));
        s.update_banner();
        // Scroll position survives a refresh (rebuild keeps the doc origin).
        s.rebuild();
    }

    /// A thumbnail arrived: show it on every cell using `key`.
    pub fn set_thumbnail(&self, key: &str, image: &CFRetained<CGImage>) {
        let s = &self.state;
        let (pw, ph) = lumora_capture::image::pixel_size(image);
        let ns = NSImage::initWithCGImage_size(NSImage::alloc(), image, CGSize::new(pw as f64, ph as f64));
        if let Some(buttons) = s.thumb_buttons.borrow_mut().remove(key) {
            for b in buttons {
                b.setImage(Some(&ns));
            }
        }
        s.thumbs.borrow_mut().insert(key.to_string(), ns);
    }

    /// A thumbnail couldn't be made: keep the placeholder, don't ask again.
    pub fn thumbnail_failed(&self, key: &str) {
        self.state.thumb_buttons.borrow_mut().remove(key);
    }
}

impl Drop for HistoryWindow {
    fn drop(&mut self) {
        self.state.window.setDelegate(None);
        self.state.window.orderOut(None);
    }
}

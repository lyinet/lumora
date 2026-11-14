//! Quick Access cards: the floating thumbnails that slide in bottom-left
//! after every capture.
//!
//! * Glass card (HUD material, 14 pt continuous corners, system shadow).
//! * Slides in from off-screen on [`motion::CARD`] (k = 300, c = 26).
//! * Hover reveals Copy / Save / Pin / Delete; clicking the thumbnail opens
//!   the file; dragging it out drops the file into any app.
//! * Auto-dismisses after the configured timeout unless hovered.
//! * Up to three stack upward; pushing a fourth retires the oldest and the
//!   rest reflow on [`motion::REFLOW`] with velocity-preserving retargeting.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use lumora_core::geometry::{Rect, Size};
use lumora_core::layout::{card_size, offscreen_left, stack_frames, MAX_CARDS};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSButton, NSColor, NSDragOperation, NSDraggingContext,
    NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent, NSImage, NSPanel, NSPasteboardWriting, NSResponder,
    NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWorkspace,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSObject, NSString, NSTimer};
use objc2_quartz_core::{kCAGravityResizeAspectFill, CALayer};

use lumora_core::i18n::{tr, Msg};

use crate::anim::WindowSpring;
use crate::events::{CardAction, EventSink, UiEvent};
use crate::tokens::{color, motion, radius, size, typography};
use crate::util::{
    cf_as_object, cg_color, cg_rect, glass_view, make_panel, mono_digits, rect, schedule_timer, screen_under_mouse,
    set_text, text_layer, weight, ActionTarget, PanelSpec,
};

/// What a card shows.
pub struct CardContent {
    pub id: u64,
    /// Thumbnail source (the capture itself, or a recording's poster frame).
    pub image: Option<CFRetained<CGImage>>,
    /// Size of the capture in points (drives the card's aspect).
    pub point_size: Size,
    /// File to drag/open (saved capture or a temp file).
    pub file: PathBuf,
    /// Recording duration; shows a play badge and disables Pin.
    pub video_duration: Option<f64>,
}

impl std::fmt::Debug for CardContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardContent").field("id", &self.id).field("file", &self.file).finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Card view (hover tracking + drag source)
// ---------------------------------------------------------------------------

/// Called with `true` on mouse-enter and `false` on mouse-exit.
type HoverHandler = RefCell<Option<Box<dyn Fn(bool)>>>;

pub struct CardViewIvars {
    id: u64,
    sink: EventSink,
    file: PathBuf,
    drag_image: Option<Retained<NSImage>>,
    on_hover: HoverHandler,
    /// Recordings open on double-click instead of annotating.
    video: bool,
    /// Pending single-click "open", cancelled by a second click.
    click_timer: RefCell<Option<Retained<NSTimer>>>,
    down_at: Cell<Option<CGPoint>>,
    dragging: Cell<bool>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraCardView"]
    #[ivars = CardViewIvars]
    pub struct CardView;

    unsafe impl NSObjectProtocol for CardView {}

    unsafe impl NSDraggingSource for CardView {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(&self, _session: &NSDraggingSession, _context: NSDraggingContext) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn drag_ended(&self, _session: &NSDraggingSession, _point: CGPoint, operation: NSDragOperation) {
            self.ivars().dragging.set(false);
            if operation != NSDragOperation::None {
                (self.ivars().sink)(UiEvent::Card { id: self.ivars().id, action: CardAction::DraggedOut });
            }
        }
    }

    impl CardView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            let opts = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect;
            // SAFETY: owner is self, which outlives the area.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    opts,
                    Some(self as &AnyObject),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().tracking.borrow_mut() = Some(area);
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            if let Some(f) = self.ivars().on_hover.borrow().as_ref() {
                f(true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            if let Some(f) = self.ivars().on_hover.borrow().as_ref() {
                f(false);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().down_at.set(Some(event.locationInWindow()));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some(start) = self.ivars().down_at.get() else { return };
            let p = event.locationInWindow();
            if self.ivars().dragging.get() || (p.x - start.x).hypot(p.y - start.y) < 4.0 {
                return;
            }
            self.ivars().dragging.set(true);
            self.begin_drag(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if self.ivars().down_at.take().is_none() || self.ivars().dragging.get() {
                return;
            }
            let (id, sink) = (self.ivars().id, self.ivars().sink.clone());
            if let Some(t) = self.ivars().click_timer.borrow_mut().take() {
                t.invalidate();
            }
            if event.clickCount() >= 2 {
                // Double-click: annotate (recordings just open).
                let action = if self.ivars().video { CardAction::Open } else { CardAction::Annotate };
                sink(UiEvent::Card { id, action });
            } else {
                // Single click opens, once we know it isn't a double-click.
                let timer = schedule_timer(NSEvent::doubleClickInterval(), false, move || {
                    sink(UiEvent::Card { id, action: CardAction::Open });
                    false
                });
                *self.ivars().click_timer.borrow_mut() = Some(timer);
            }
        }
    }
);

impl CardView {
    fn new(
        mtm: MainThreadMarker,
        frame: CGRect,
        id: u64,
        sink: EventSink,
        file: PathBuf,
        drag_image: Option<Retained<NSImage>>,
        video: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CardViewIvars {
            id,
            sink,
            file,
            drag_image,
            on_hover: RefCell::new(None),
            video,
            click_timer: RefCell::new(None),
            down_at: Cell::new(None),
            dragging: Cell::new(false),
            tracking: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Start an `NSDraggingSession` carrying the file URL, with the
    /// thumbnail as the drag image.
    fn begin_drag(&self, event: &NSEvent) {
        let Some(url) = lumora_capture::clipboard::file_url(&self.ivars().file) else { return };
        let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> = ProtocolObject::from_retained(url);
        let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
        let b = self.bounds();
        let frame = CGRect::new(
            CGPoint::new(b.size.width * 0.15, b.size.height * 0.15),
            CGSize::new(b.size.width * 0.7, b.size.height * 0.7),
        );
        // SAFETY: contents is an NSImage (or nil), as the API expects.
        unsafe {
            item.setDraggingFrame_contents(frame, self.ivars().drag_image.as_deref().map(|i| i as &AnyObject));
        }
        let source = ProtocolObject::from_ref(self);
        let _session =
            self.beginDraggingSessionWithItems_event_source(&NSArray::from_retained_slice(&[item]), event, source);
    }
}

// ---------------------------------------------------------------------------
// Card and stack
// ---------------------------------------------------------------------------

struct Card {
    id: u64,
    panel: Retained<NSPanel>,
    _view: Retained<CardView>,
    buttons: Retained<NSView>,
    spring: WindowSpring,
    size: Size,
    hovered: Cell<bool>,
    dismissing: Cell<bool>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    _targets: Vec<Retained<ActionTarget>>,
}

struct QaInner {
    mtm: MainThreadMarker,
    sink: EventSink,
    /// Newest first.
    cards: RefCell<Vec<Rc<Card>>>,
    timeout: Cell<f64>,
    visible: Cell<Rect>,
}

/// The Quick Access stack.
pub struct QuickAccess {
    inner: Rc<QaInner>,
}

impl std::fmt::Debug for QuickAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuickAccess").field("cards", &self.inner.cards.borrow().len()).finish()
    }
}

impl QuickAccess {
    pub fn new(mtm: MainThreadMarker, sink: EventSink, timeout: f64) -> Self {
        QuickAccess {
            inner: Rc::new(QaInner {
                mtm,
                sink,
                cards: RefCell::new(Vec::new()),
                timeout: Cell::new(timeout),
                visible: Cell::new(Rect::ZERO),
            }),
        }
    }

    pub fn set_timeout(&self, seconds: f64) {
        self.inner.timeout.set(seconds);
    }

    /// Size a card would have for a capture of `point_size`.
    pub fn card_size_for(point_size: Size) -> Size {
        card_size(point_size, size::CARD_WIDTH, size::CARD_MIN_HEIGHT, size::CARD_MAX_HEIGHT)
    }

    /// Where the next card will land (AppKit global points), so the shutter
    /// animation can fly the capture there.
    pub fn next_frame(&self, point_size: Size) -> Rect {
        let visible = self.current_visible();
        let mut sizes = vec![Self::card_size_for(point_size)];
        sizes.extend(self.inner.cards.borrow().iter().filter(|c| !c.dismissing.get()).map(|c| c.size));
        stack_frames(visible, &sizes, size::CARD_MARGIN, size::CARD_SPACING)[0]
    }

    fn current_visible(&self) -> Rect {
        let has_cards = !self.inner.cards.borrow().is_empty();
        if !has_cards || self.inner.visible.get().is_empty() {
            if let Some(screen) = screen_under_mouse(self.inner.mtm) {
                self.inner.visible.set(rect(screen.visibleFrame()));
            }
        }
        self.inner.visible.get()
    }

    /// Show a new card (newest at the bottom).
    pub fn push(&self, content: CardContent) {
        let inner = &self.inner;
        let mtm = inner.mtm;
        let visible = self.current_visible();
        let card_size = Self::card_size_for(content.point_size);

        // Retire the oldest if we're at capacity.
        let live: Vec<Rc<Card>> = inner.cards.borrow().iter().filter(|c| !c.dismissing.get()).cloned().collect();
        if live.len() >= MAX_CARDS {
            if let Some(oldest) = live.last() {
                QaInner::dismiss(inner, oldest.id, false);
            }
        }

        let frame = stack_frames(visible, &[card_size], size::CARD_MARGIN, size::CARD_SPACING)[0];
        let start = offscreen_left(frame, visible);
        let panel = make_panel(
            mtm,
            cg_rect(start),
            PanelSpec { level: NSStatusWindowLevel, keyable: false, shadow: true, ignores_mouse: false },
        );
        panel.setBecomesKeyOnlyIfNeeded(true);

        let local = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(card_size.width, card_size.height));
        let drag_image = content.image.as_ref().map(|img| {
            NSImage::initWithCGImage_size(NSImage::alloc(), img, CGSize::new(card_size.width, card_size.height))
        });
        let view = CardView::new(
            mtm,
            local,
            content.id,
            inner.sink.clone(),
            content.file.clone(),
            drag_image,
            content.video_duration.is_some(),
        );
        view.setWantsLayer(true);

        // Glass background.
        let glass = glass_view(mtm, local, radius::CARD);
        view.addSubview(&glass);

        // Thumbnail.
        let thumb_frame = Rect::new(
            size::CARD_INSET,
            size::CARD_INSET,
            card_size.width - 2.0 * size::CARD_INSET,
            card_size.height - 2.0 * size::CARD_INSET,
        );
        let thumb = NSView::initWithFrame(NSView::alloc(mtm), cg_rect(thumb_frame));
        let thumb_layer = CALayer::new();
        thumb_layer.setCornerRadius(radius::CARD_THUMB);
        thumb_layer.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        thumb_layer.setMasksToBounds(true);
        thumb_layer.setContentsGravity(unsafe { kCAGravityResizeAspectFill });
        thumb_layer.setBackgroundColor(Some(&cg_color(color::PILL_BG)));
        if let Some(img) = &content.image {
            // SAFETY: CGImage is a valid layer `contents`.
            unsafe { thumb_layer.setContents(Some(cf_as_object(&**img))) };
        }
        thumb.setLayer(Some(&thumb_layer));
        thumb.setWantsLayer(true);
        view.addSubview(&thumb);

        let scale = panel.backingScaleFactor();
        if let Some(d) = content.video_duration {
            let badge = CALayer::new();
            let text = format!("▶  {}", lumora_core::format::elapsed(d));
            let font = mono_digits(typography::CAPTION, weight::SEMIBOLD);
            let w = crate::util::text_width(&text, &font) + 16.0;
            badge.setFrame(cg_rect(Rect::new(8.0, 8.0, w, 20.0)));
            badge.setBackgroundColor(Some(&cg_color(color::PILL_BG)));
            badge.setCornerRadius(10.0);
            let label = text_layer(&font, typography::CAPTION, color::PILL_TEXT, scale);
            label.setFrame(cg_rect(Rect::new(0.0, 3.0, w, 15.0)));
            set_text(&label, &text);
            badge.addSublayer(&label);
            thumb_layer.addSublayer(&badge);
        }

        // Hover buttons.
        let (buttons, targets) =
            build_buttons(mtm, thumb_frame, content.id, content.video_duration.is_some(), &inner.sink);
        buttons.setAlphaValue(0.0);
        view.addSubview(&buttons);

        panel.setContentView(Some(&view));
        panel.setAlphaValue(1.0);
        panel.orderFrontRegardless();
        panel.invalidateShadow();

        let window: Retained<objc2_app_kit::NSWindow> = Retained::into_super(panel.clone());
        let card = Rc::new(Card {
            id: content.id,
            panel,
            _view: view.clone(),
            buttons,
            spring: WindowSpring::new(window),
            size: card_size,
            hovered: Cell::new(false),
            dismissing: Cell::new(false),
            timer: RefCell::new(None),
            _targets: targets,
        });

        // Hover: reveal buttons, pause auto-dismiss.
        let weak_inner = Rc::downgrade(inner);
        let weak_card = Rc::downgrade(&card);
        *view.ivars().on_hover.borrow_mut() = Some(Box::new(move |hovering| {
            let (Some(inner), Some(card)) = (weak_inner.upgrade(), weak_card.upgrade()) else { return };
            card.hovered.set(hovering);
            fade_view(&card.buttons, if hovering { 1.0 } else { 0.0 });
            if hovering {
                if let Some(t) = card.timer.borrow_mut().take() {
                    t.invalidate();
                }
            } else {
                QaInner::arm_timer(&inner, &card);
            }
        }));

        inner.cards.borrow_mut().insert(0, card.clone());
        QaInner::arm_timer(inner, &card);
        QaInner::reflow(inner, Some(content.id));
    }

    /// Animate a card away (button actions, drag-out, timeout).
    pub fn dismiss(&self, id: u64) {
        QaInner::dismiss(&self.inner, id, true);
    }

    pub fn dismiss_all(&self) {
        let ids: Vec<u64> = self.inner.cards.borrow().iter().map(|c| c.id).collect();
        for id in ids {
            QaInner::dismiss(&self.inner, id, true);
        }
    }
}

impl QaInner {
    /// Move every live card to its stack slot. `new_id` (if any) slides in on
    /// the card spring; the rest reflow on the smoother spring.
    fn reflow(this: &Rc<QaInner>, new_id: Option<u64>) {
        let live: Vec<Rc<Card>> = this.cards.borrow().iter().filter(|c| !c.dismissing.get()).cloned().collect();
        let sizes: Vec<Size> = live.iter().map(|c| c.size).collect();
        let frames = stack_frames(this.visible.get(), &sizes, size::CARD_MARGIN, size::CARD_SPACING);
        for (card, frame) in live.iter().zip(frames) {
            let spring = if Some(card.id) == new_id { motion::CARD } else { motion::REFLOW };
            card.spring.animate((frame.min_x(), frame.min_y()), 1.0, spring, None);
        }
    }

    fn arm_timer(this: &Rc<QaInner>, card: &Rc<Card>) {
        if let Some(t) = card.timer.borrow_mut().take() {
            t.invalidate();
        }
        let weak = Rc::downgrade(this);
        let id = card.id;
        let timer = schedule_timer(this.timeout.get(), false, move || {
            if let Some(inner) = weak.upgrade() {
                QaInner::dismiss(&inner, id, true);
            }
            false
        });
        *card.timer.borrow_mut() = Some(timer);
    }

    fn dismiss(this: &Rc<QaInner>, id: u64, reflow: bool) {
        let Some(card) = this.cards.borrow().iter().find(|c| c.id == id).cloned() else { return };
        if card.dismissing.replace(true) {
            return;
        }
        if let Some(t) = card.timer.borrow_mut().take() {
            t.invalidate();
        }
        let frame = rect(card.panel.frame());
        let off = offscreen_left(frame, this.visible.get());
        let weak: Weak<QaInner> = Rc::downgrade(this);
        card.spring.animate(
            (off.min_x(), off.min_y()),
            0.0,
            motion::REFLOW,
            Some(Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    let removed = {
                        let mut cards = inner.cards.borrow_mut();
                        cards.iter().position(|c| c.id == id).map(|i| cards.remove(i))
                    };
                    if let Some(card) = removed {
                        card.panel.orderOut(None);
                    }
                    (inner.sink)(UiEvent::CardDismissed(id));
                }
            })),
        );
        if reflow {
            QaInner::reflow(this, None);
        }
    }
}

/// Fade a view's alpha using the implicit animator.
fn fade_view(view: &NSView, to: f64) {
    NSAnimationContext::beginGrouping();
    NSAnimationContext::currentContext().setDuration(motion::HOVER_FADE);
    view.animator().setAlphaValue(to);
    NSAnimationContext::endGrouping();
}

/// The hover overlay with four round glyph buttons.
fn build_buttons(
    mtm: MainThreadMarker,
    frame: Rect,
    id: u64,
    is_video: bool,
    sink: &EventSink,
) -> (Retained<NSView>, Vec<Retained<ActionTarget>>) {
    let overlay = NSView::initWithFrame(NSView::alloc(mtm), cg_rect(frame));
    overlay.setWantsLayer(true);
    if let Some(l) = overlay.layer() {
        l.setBackgroundColor(Some(&cg_color(color::BUTTON_BG.with_alpha(0.35))));
        l.setCornerRadius(radius::CARD_THUMB);
    }
    let mut actions = vec![
        ("doc.on.doc", tr(Msg::Copy), CardAction::Copy),
        ("square.and.arrow.down", tr(Msg::Save), CardAction::Save),
        ("pencil.tip.crop.circle", tr(Msg::Annotate), CardAction::Annotate),
        ("pin", tr(Msg::PinToScreen), CardAction::Pin),
        ("trash", tr(Msg::Delete), CardAction::Delete),
    ];
    if is_video {
        actions.retain(|a| a.2 != CardAction::Pin && a.2 != CardAction::Annotate);
    }
    let b = size::CARD_BUTTON;
    let gap = 6.0;
    let total = actions.len() as f64 * b + (actions.len() as f64 - 1.0) * gap;
    let mut x = (frame.width() - total) / 2.0;
    let y = (frame.height() - b) / 2.0;
    let mut targets = Vec::new();
    for (symbol, tip, action) in actions {
        let sink = sink.clone();
        let target = ActionTarget::new(mtm, move |_| sink(UiEvent::Card { id, action }));
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(tip)),
        );
        let button = match image {
            // SAFETY: target outlives the button (kept in `targets`).
            Some(img) => unsafe {
                NSButton::buttonWithImage_target_action(&img, Some(&target), Some(ActionTarget::action()), mtm)
            },
            None => unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(tip),
                    Some(&target),
                    Some(ActionTarget::action()),
                    mtm,
                )
            },
        };
        button.setBordered(false);
        button.setContentTintColor(Some(&NSColor::whiteColor()));
        button.setToolTip(Some(&NSString::from_str(tip)));
        button.setFrame(cg_rect(Rect::new(x, y, b, b)));
        button.setWantsLayer(true);
        if let Some(l) = button.layer() {
            l.setBackgroundColor(Some(&cg_color(color::BUTTON_BG)));
            l.setCornerRadius(b / 2.0);
        }
        overlay.addSubview(&button);
        targets.push(target);
        x += b + gap;
    }
    (overlay, targets)
}

/// Open a file with its default app (used for card clicks).
pub fn open_file(path: &std::path::Path) {
    if let Some(url) = lumora_capture::clipboard::file_url(path) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

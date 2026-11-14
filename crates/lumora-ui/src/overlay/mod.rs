//! The area-selection overlay.
//!
//! One borderless, non-activating, key-capable panel per display at
//! screen-saver level, each hosting an [`view::OverlayView`]. A single
//! [`lumora_core::selection::SelectionMachine`] (in global Quartz points)
//! owns the interaction; the views only translate events and draw.
//!
//! Motion: while the mouse button is down the selection tracks the pointer
//! 1:1 (no animation — anything else feels laggy). Discrete changes —
//! arrow-key nudges, the hovered-window highlight jumping between windows,
//! committing a selection — are driven by [`RectSpring`]s so the cutout,
//! outline, handles and size pill glide together.

mod view;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use lumora_capture::image::PixelBuffer;
use lumora_core::format;
use lumora_core::geometry::{Point, Rect, Size};
use lumora_core::layout::{loupe_frame, loupe_sample, pill_frame};
use lumora_core::selection::{Config, Input, Outcome, Phase, SelectionMachine, Target};
use lumora_core::snap::WindowList;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSCursor, NSEvent, NSPanel, NSScreen, NSScreenSaverWindowLevel};
use objc2_core_graphics::CGImage;
use objc2_foundation::NSTimer;

use self::view::{Frame, LoupeFrame, OverlayView};
use crate::anim::RectSpring;
use crate::events::{EventSink, FrozenDisplay, OverlayMode, UiEvent};
use crate::tokens::{motion, size, typography};
use crate::util::{
    cg_rect, make_panel, mono_digits, now, primary_height, quartz_frame, schedule_timer, screen_id, text_width, weight,
    PanelSpec,
};

/// Overlay behaviour switches (from settings).
#[derive(Debug, Clone, Copy)]
pub struct OverlayConfig {
    pub mode: OverlayMode,
    pub show_magnifier: bool,
    pub show_crosshair: bool,
    pub window_picking: bool,
}

struct ScreenOverlay {
    panel: Retained<NSPanel>,
    view: Retained<OverlayView>,
    frame: Rect,
    scale: f64,
}

pub(crate) struct Inner {
    config: OverlayConfig,
    sink: EventSink,
    machine: RefCell<SelectionMachine>,
    screens: RefCell<Vec<ScreenOverlay>>,
    frozen: Rc<Vec<FrozenDisplay>>,
    /// Lazily-decoded pixels per frozen display, for the loupe's hex value.
    pixels: Vec<OnceCell<Option<PixelBuffer>>>,
    /// Selection as currently displayed and its spring (if animating).
    shown_selection: Cell<Option<Rect>>,
    selection_spring: RefCell<Option<RectSpring>>,
    /// Window highlight as displayed and its spring.
    shown_highlight: Cell<Option<Rect>>,
    highlight_spring: RefCell<Option<RectSpring>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    finished: Cell<bool>,
    was_selected: Cell<bool>,
    pop_pending: Cell<bool>,
    interacted: Cell<bool>,
    copied_until: Cell<f64>,
}

/// Owns an on-screen overlay. Dropping it removes the overlay.
pub struct OverlayController {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for OverlayController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayController").field("config", &self.inner.config).finish_non_exhaustive()
    }
}

impl OverlayController {
    /// Show the overlay on every display.
    ///
    /// `frozen` supplies the backdrop per display (capture mode); `windows`
    /// is the front-to-back list for hover picking.
    pub fn show(
        mtm: MainThreadMarker,
        config: OverlayConfig,
        frozen: Rc<Vec<FrozenDisplay>>,
        windows: WindowList,
        sink: EventSink,
    ) -> Self {
        let ph = primary_height(mtm);
        let screens = NSScreen::screens(mtm);
        let frames: Vec<Rect> = screens.iter().map(|s| quartz_frame(&s, ph)).collect();
        let mut cfg = Config::new(frames);
        cfg.window_picking = config.window_picking;
        cfg.handle_radius = size::HANDLE_HIT;
        let machine = SelectionMachine::new(cfg, windows);
        let pixels = frozen.iter().map(|_| OnceCell::new()).collect();

        let inner = Rc::new_cyclic(|weak: &Weak<Inner>| {
            let overlays = screens
                .iter()
                .map(|screen| {
                    let frame = quartz_frame(&screen, ph);
                    let scale = screen.backingScaleFactor();
                    let id = screen_id(&screen);
                    let backdrop: Option<&CGImage> = match config.mode {
                        OverlayMode::Capture => frozen.iter().find(|f| f.id == id).map(|f| &*f.image),
                        OverlayMode::Record => None,
                    };
                    let appkit_frame = screen.frame();
                    let panel = make_panel(
                        mtm,
                        appkit_frame,
                        PanelSpec {
                            level: NSScreenSaverWindowLevel,
                            keyable: true,
                            shadow: false,
                            ignores_mouse: false,
                        },
                    );
                    panel.setAcceptsMouseMovedEvents(true);
                    let local = objc2_core_foundation::CGRect::new(
                        objc2_core_foundation::CGPoint::new(0.0, 0.0),
                        appkit_frame.size,
                    );
                    let view = OverlayView::new(mtm, local, frame, scale, backdrop, config.mode, weak.clone());
                    panel.setContentView(Some(&view));
                    ScreenOverlay { panel, view, frame, scale }
                })
                .collect();
            Inner {
                config,
                sink,
                machine: RefCell::new(machine),
                screens: RefCell::new(overlays),
                frozen,
                pixels,
                shown_selection: Cell::new(None),
                selection_spring: RefCell::new(None),
                shown_highlight: Cell::new(None),
                highlight_spring: RefCell::new(None),
                timer: RefCell::new(None),
                finished: Cell::new(false),
                was_selected: Cell::new(false),
                pop_pending: Cell::new(false),
                interacted: Cell::new(false),
                copied_until: Cell::new(0.0),
            }
        });

        // Order in, focus the screen under the pointer, render frame zero.
        let mouse = NSEvent::mouseLocation();
        let mouse_q = Point::new(mouse.x, ph - mouse.y);
        for s in inner.screens.borrow().iter() {
            s.panel.orderFrontRegardless();
            s.view.appear();
            if s.frame.contains(mouse_q) {
                s.panel.makeKeyWindow();
                s.panel.makeFirstResponder(Some(&s.view));
            }
        }
        NSCursor::crosshairCursor().set();
        Inner::handle_input(&inner, Input::MouseMoved(mouse_q));
        OverlayController { inner }
    }

    pub fn mode(&self) -> OverlayMode {
        self.inner.config.mode
    }

    /// Remove the overlay without emitting an event. (After a confirm or
    /// cancel the overlay has already removed itself — the cancel fade-out
    /// finishes on its own timer.)
    pub fn close(&self) {
        if !self.inner.finished.replace(true) {
            self.inner.teardown();
        }
    }
}

impl Drop for OverlayController {
    fn drop(&mut self) {
        self.close();
    }
}

impl Inner {
    pub(crate) fn handle_input(this: &Rc<Inner>, input: Input) {
        if this.finished.get() {
            return;
        }
        if matches!(input, Input::MouseDown { .. }) {
            this.interacted.set(true);
        }
        let outcome = this.machine.borrow_mut().handle(input);
        match outcome {
            Outcome::None => {}
            Outcome::Redraw => {
                Self::update_springs(this);
                this.render();
            }
            Outcome::Cancel => return this.finish(None),
            Outcome::Confirm(t) => return this.finish(Some(t)),
            Outcome::CopyColor => {
                if let Some(hex) = this.color_under_mouse().map(|c| c.hex()) {
                    lumora_capture::clipboard::copy_text(&hex);
                    this.copied_until.set(now() + 1.2);
                    this.render();
                    let weak = Rc::downgrade(this);
                    schedule_timer(1.25, false, move || {
                        if let Some(i) = weak.upgrade() {
                            i.render();
                        }
                        false
                    });
                }
            }
        }
        OverlayView::set_cursor(this.machine.borrow().cursor_hint());
    }

    /// Decide what animates and (re)start the frame timer if needed.
    fn update_springs(this: &Rc<Inner>) {
        let t = now();
        let m = this.machine.borrow();

        // Selection: 1:1 while interacting, springs for discrete changes.
        let target = m.selection();
        let shown = this.current_selection(t);
        let mut sel_spring = this.selection_spring.borrow_mut();
        if m.is_interacting() || target.is_none() || shown.is_none() {
            *sel_spring = None;
            this.shown_selection.set(target);
        } else if let (Some(target), Some(shown)) = (target, shown) {
            let current_target = sel_spring.as_ref().map(|s| s.target()).or(this.shown_selection.get());
            if current_target != Some(target) {
                match sel_spring.as_mut() {
                    Some(s) => s.retarget(target, t),
                    None => *sel_spring = Some(RectSpring::new(motion::SELECTION, shown, target, t)),
                }
                this.shown_selection.set(Some(target));
            }
        }
        drop(sel_spring);

        // Handles pop when a selection is first committed.
        let selected = matches!(m.phase(), Phase::Selected);
        let pop = selected && !this.was_selected.get();
        this.was_selected.set(selected);
        if pop {
            this.pop_pending.set(true);
        }

        // Window highlight: springs between windows; pops in from 96 %.
        let hl_target = if target.is_none() { m.hovered_window().map(|w| w.frame) } else { None };
        // Read the displayed highlight *before* taking the mutable borrow.
        let hl_shown = this.current_highlight(t);
        let mut hl = this.highlight_spring.borrow_mut();
        match hl_target {
            None => {
                *hl = None;
                this.shown_highlight.set(None);
            }
            Some(to) => {
                let current_target = hl.as_ref().map(|s| s.target()).or(this.shown_highlight.get());
                if current_target != Some(to) {
                    let from = hl_shown.unwrap_or_else(|| {
                        let c = to.mid();
                        Rect::new(
                            c.x - to.width() * 0.48,
                            c.y - to.height() * 0.48,
                            to.width() * 0.96,
                            to.height() * 0.96,
                        )
                    });
                    match hl.as_mut() {
                        Some(s) => s.retarget(to, t),
                        None => *hl = Some(RectSpring::new(motion::SELECTION, from, to, t)),
                    }
                    this.shown_highlight.set(Some(to));
                }
            }
        }
        let animating = this.selection_spring.borrow().is_some() || hl.is_some();
        drop(hl);
        drop(m);
        if animating {
            Self::ensure_timer(this);
        }
    }

    fn ensure_timer(this: &Rc<Inner>) {
        if this.timer.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(this);
        let timer = schedule_timer(motion::FRAME_TICK, true, move || {
            let Some(inner) = weak.upgrade() else { return false };
            let t = now();
            let mut done = true;
            for spring in [&inner.selection_spring, &inner.highlight_spring] {
                let mut s = spring.borrow_mut();
                if s.as_ref().is_some_and(|s| s.is_settled(t)) {
                    *s = None;
                }
                done &= s.is_none();
            }
            inner.render();
            if done {
                *inner.timer.borrow_mut() = None;
            }
            !done
        });
        *this.timer.borrow_mut() = Some(timer);
    }

    fn current_selection(&self, t: f64) -> Option<Rect> {
        match self.selection_spring.borrow().as_ref() {
            Some(s) => Some(s.value(t)),
            None => self.shown_selection.get(),
        }
    }

    fn current_highlight(&self, t: f64) -> Option<Rect> {
        match self.highlight_spring.borrow().as_ref() {
            Some(s) => Some(s.value(t)),
            None => self.shown_highlight.get(),
        }
    }

    fn frozen_index_at(&self, p: Point) -> Option<usize> {
        self.frozen.iter().position(|f| f.frame.contains(p))
    }

    fn color_under_mouse(&self) -> Option<lumora_core::color::Rgb8> {
        let mouse = self.machine.borrow().mouse();
        let i = self.frozen_index_at(mouse)?;
        let fd = &self.frozen[i];
        let px = ((mouse.x - fd.frame.min_x()) * fd.scale).floor().max(0.0) as usize;
        let py = ((mouse.y - fd.frame.min_y()) * fd.scale).floor().max(0.0) as usize;
        self.pixels[i].get_or_init(|| PixelBuffer::from_image(&fd.image)).as_ref()?.pixel(px, py)
    }

    fn screen_frame_at(&self, p: Point) -> Option<(Rect, f64)> {
        self.screens.borrow().iter().find(|s| s.frame.contains(p)).map(|s| (s.frame, s.scale))
    }

    fn render(&self) {
        let t = now();
        let m = self.machine.borrow();
        let mouse = m.mouse();
        let phase = m.phase();
        let selection = self.current_selection(t);
        let highlight = if selection.is_none() { self.current_highlight(t) } else { None };
        let pill_font = mono_digits(typography::PILL, weight::MEDIUM);
        let make_pill = |text: String, anchor: Rect, screen: Rect| {
            let w = text_width(&text, &pill_font) + 2.0 * size::PILL_PADDING;
            let frame = pill_frame(anchor, Size::new(w, size::PILL_HEIGHT), screen, size::PILL_GAP);
            (text, frame.rounded(2.0))
        };

        // Pill: selection size, hovered window size, or cursor coordinates.
        let pill = if let Some(sel) = selection {
            self.screen_frame_at(sel.mid()).map(|(screen, scale)| {
                let (w, h) = sel.pixel_size(scale);
                make_pill(format::dimensions(w, h), sel, screen)
            })
        } else if let (Some(hl), Some(w)) = (highlight, m.hovered_window()) {
            self.screen_frame_at(hl.mid()).map(|(screen, scale)| {
                let (pw, ph) = w.frame.pixel_size(scale);
                let name = if w.app_name.is_empty() { String::new() } else { format!("{}  ·  ", w.app_name) };
                make_pill(format!("{name}{}", format::dimensions(pw, ph)), hl, screen)
            })
        } else if !self.config.show_magnifier || self.config.mode == OverlayMode::Record {
            self.screen_frame_at(mouse).map(|(screen, scale)| {
                let x = ((mouse.x - screen.min_x()) * scale) as i64;
                let y = ((mouse.y - screen.min_y()) * scale) as i64;
                let anchor = Rect::new(mouse.x + 8.0, mouse.y + 8.0, 1.0, 1.0);
                make_pill(format!("{x}, {y}"), anchor, screen)
            })
        } else {
            None
        };

        // Loupe while pointing precisely (not while moving a selection).
        let wants_loupe = self.config.mode == OverlayMode::Capture
            && self.config.show_magnifier
            && !matches!(phase, Phase::Moving { .. } | Phase::Selected);
        let loupe = if wants_loupe { self.loupe_frame(mouse, t) } else { None };

        let frame = Frame {
            selection,
            highlight,
            show_handles: matches!(phase, Phase::Selected | Phase::Moving { .. } | Phase::Resizing { .. }),
            pop_handles: self.pop_pending.replace(false),
            mouse,
            crosshair: self.config.show_crosshair && selection.is_none() && !m.is_interacting(),
            pill,
            loupe,
            hint: !self.interacted.get(),
        };
        drop(m);
        for s in self.screens.borrow().iter() {
            s.view.render(&frame);
        }
    }

    fn loupe_frame(&self, mouse: Point, t: f64) -> Option<LoupeFrame> {
        let i = self.frozen_index_at(mouse)?;
        let fd = &self.frozen[i];
        let (iw, ih) = lumora_capture::image::pixel_size(&fd.image);
        let px = ((mouse.x - fd.frame.min_x()) * fd.scale).floor() as i64;
        let py = ((mouse.y - fd.frame.min_y()) * fd.scale).floor() as i64;
        let s = loupe_sample(px, py, size::LOUPE_GRID, iw as u32, ih as u32)?;
        let image = CGImage::with_image_in_rect(
            Some(&fd.image),
            cg_rect(Rect::new(s.x as f64, s.y as f64, s.w as f64, s.h as f64)),
        )?;
        let hex = self.color_under_mouse().map(|c| c.hex()).unwrap_or_else(|| "—".into());
        let label = if t < self.copied_until.get() { format!("Copied {hex}") } else { format!("{hex}   {px}, {py}") };
        let side = size::LOUPE_GRID as f64 * size::LOUPE_CELL;
        let container = Size::new(side, side + 6.0 + size::LOUPE_LABEL_HEIGHT);
        let screen = self.screen_frame_at(mouse).map(|(f, _)| f).unwrap_or(fd.frame);
        Some(LoupeFrame { frame: loupe_frame(mouse, container, screen, size::LOUPE_OFFSET).rounded(2.0), image, label })
    }

    fn finish(&self, target: Option<Target>) {
        if self.finished.replace(true) {
            return;
        }
        let mode = self.config.mode;
        match target {
            Some(target) => {
                // Remove instantly so the shutter animation owns the screen.
                self.teardown();
                (self.sink)(UiEvent::OverlayConfirmed { mode, target });
            }
            None => {
                for s in self.screens.borrow().iter() {
                    s.view.disappear();
                }
                let panels: Vec<Retained<NSPanel>> = self.screens.borrow().iter().map(|s| s.panel.clone()).collect();
                schedule_timer(motion::OVERLAY_OUT, false, move || {
                    panels.iter().for_each(|p| p.orderOut(None));
                    false
                });
                NSCursor::arrowCursor().set();
                (self.sink)(UiEvent::OverlayCancelled { mode });
            }
        }
    }

    fn teardown(&self) {
        if let Some(t) = self.timer.borrow_mut().take() {
            t.invalidate();
        }
        for s in self.screens.borrow().iter() {
            s.panel.orderOut(None);
        }
        NSCursor::arrowCursor().set();
    }
}

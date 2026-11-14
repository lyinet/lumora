//! The 3-2-1 countdown before recording: a glass tile with a draining ring
//! and a digit that pops in on a bouncy spring each second. Esc cancels.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lumora_core::geometry::Rect;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSFont, NSPanel, NSScreenSaverWindowLevel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNumber, NSString, NSTimer};
use objc2_quartz_core::{kCALineCapRound, CABasicAnimation, CAMediaTiming, CAShapeLayer, CATextLayer};

use crate::events::{EventSink, UiEvent};
use crate::tokens::{color, motion, radius, size, typography};
use crate::util::{
    cg_color, cg_rect, ease_scalar, glass_view, make_panel, schedule_timer, set_text, spring_scalar, text_layer,
    weight, KeyCatcherView, PanelSpec, KEY_ESCAPE,
};

struct Inner {
    panel: Retained<NSPanel>,
    digit: Retained<CATextLayer>,
    ring: Retained<CAShapeLayer>,
    remaining: Cell<u32>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    done: Cell<bool>,
    sink: EventSink,
}

/// A running countdown. Dropping it removes the tile.
pub struct Countdown {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for Countdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Countdown").field("remaining", &self.inner.remaining.get()).finish()
    }
}

impl Countdown {
    /// Count down `seconds` centred over `area` (AppKit global points).
    pub fn start(mtm: MainThreadMarker, seconds: u32, area: Rect, sink: EventSink) -> Self {
        let s = size::COUNTDOWN;
        let c = area.mid();
        let frame = CGRect::new(CGPoint::new((c.x - s / 2.0).round(), (c.y - s / 2.0).round()), CGSize::new(s, s));
        let panel = make_panel(
            mtm,
            frame,
            PanelSpec { level: NSScreenSaverWindowLevel, keyable: true, shadow: true, ignores_mouse: false },
        );
        let local = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(s, s));
        let glass = glass_view(mtm, local, radius::COUNTDOWN);

        let weak_sink = sink.clone();
        let (catcher, root) = KeyCatcherView::new(mtm, local, move |code| {
            if code == KEY_ESCAPE {
                weak_sink(UiEvent::CountdownCancelled);
            }
        });
        glass.addSubview(&catcher);
        panel.setContentView(Some(&glass));

        let scale = panel.backingScaleFactor();
        let r = size::COUNTDOWN_RING;
        let ring_rect = Rect::new((s - r) / 2.0, (s - r) / 2.0, r, r);
        let track = CAShapeLayer::new();
        track.setPath(Some(&crate::util::round_rect_path(ring_rect, r / 2.0)));
        track.setFillColor(None);
        track.setStrokeColor(Some(&cg_color(color::HAIRLINE)));
        track.setLineWidth(size::COUNTDOWN_RING_WIDTH);
        root.addSublayer(&track);

        // Ring drawn from 12 o'clock clockwise (y-down space: -π/2 is up).
        let ring = CAShapeLayer::new();
        ring.setFrame(cg_rect(ring_rect));
        let arc = objc2_core_graphics::CGMutablePath::new();
        // SAFETY: valid mutable path, identity transform.
        unsafe {
            objc2_core_graphics::CGMutablePath::add_arc(
                Some(&arc),
                std::ptr::null(),
                r / 2.0,
                r / 2.0,
                r / 2.0,
                -std::f64::consts::FRAC_PI_2,
                3.0 * std::f64::consts::FRAC_PI_2,
                false,
            );
        }
        ring.setPath(Some(&arc));
        ring.setFillColor(None);
        ring.setStrokeColor(Some(&cg_color(color::RECORD)));
        ring.setLineWidth(size::COUNTDOWN_RING_WIDTH);
        ring.setLineCap(unsafe { kCALineCapRound });
        root.addSublayer(&ring);

        let font = NSFont::systemFontOfSize_weight(typography::COUNTDOWN_DIGIT, weight::BOLD);
        let digit = text_layer(&font, typography::COUNTDOWN_DIGIT, color::PILL_TEXT, scale);
        let dh = typography::COUNTDOWN_DIGIT * 1.25;
        digit.setFrame(cg_rect(Rect::new(0.0, (s - dh) / 2.0, s, dh)));
        root.addSublayer(&digit);

        panel.orderFrontRegardless();
        panel.makeKeyWindow();
        panel.makeFirstResponder(Some(&catcher));
        if let Some(l) = glass.layer() {
            spring_scalar(&l, "transform.scale", 0.85, 1.0, motion::POP);
        }

        let inner = Rc::new(Inner {
            panel,
            digit,
            ring,
            remaining: Cell::new(seconds.max(1)),
            timer: RefCell::new(None),
            done: Cell::new(false),
            sink,
        });
        Inner::show_digit(&inner);
        let weak = Rc::downgrade(&inner);
        let timer = schedule_timer(1.0, true, move || {
            let Some(inner) = weak.upgrade() else { return false };
            Inner::tick(&inner)
        });
        *inner.timer.borrow_mut() = Some(timer);
        Countdown { inner }
    }
}

impl Inner {
    fn show_digit(this: &Rc<Inner>) {
        let n = this.remaining.get();
        set_text(&this.digit, &n.to_string());
        spring_scalar(&this.digit, "transform.scale", 0.55, 1.0, motion::POP);
        // Drain the ring over the coming second.
        let anim = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("strokeEnd")));
        // SAFETY: NSNumber values for a scalar key path.
        unsafe {
            anim.setFromValue(Some(&NSNumber::numberWithDouble(1.0)));
            anim.setToValue(Some(&NSNumber::numberWithDouble(0.0)));
        }
        anim.setDuration(1.0);
        this.ring.addAnimation_forKey(&anim, Some(&NSString::from_str("drain")));
    }

    /// Returns whether the timer should keep running.
    fn tick(this: &Rc<Inner>) -> bool {
        if this.done.get() {
            return false;
        }
        let n = this.remaining.get().saturating_sub(1);
        this.remaining.set(n);
        if n == 0 {
            this.done.set(true);
            if let Some(root) = this.panel.contentView().and_then(|v| v.layer()) {
                ease_scalar(&root, "opacity", 1.0, 0.0, 0.12);
            }
            (this.sink)(UiEvent::CountdownFinished);
            return false;
        }
        Inner::show_digit(this);
        true
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(t) = self.timer.borrow_mut().take() {
            t.invalidate();
        }
        self.panel.orderOut(None);
    }
}

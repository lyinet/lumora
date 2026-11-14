//! Frame-driven spring animation for window frames and alpha.
//!
//! Core Animation animates layers, not windows. To move a whole `NSWindow`
//! (Quick Access cards sliding in and reflowing) we sample the analytic
//! spring from `lumora_core::spring` on a 120 Hz timer. Retargeting while in
//! flight carries velocity over, so rapid captures make the stack flow
//! instead of jump.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use lumora_core::spring::{Spring, SpringAnimator};
use objc2::rc::Retained;
use objc2_app_kit::NSWindow;
use objc2_core_foundation::CGPoint;
use objc2_foundation::NSTimer;

use crate::tokens::motion;
use crate::util::{now, schedule_timer};

struct Inner {
    window: Retained<NSWindow>,
    x: Option<SpringAnimator>,
    y: Option<SpringAnimator>,
    alpha: Option<SpringAnimator>,
    timer: Option<Retained<NSTimer>>,
    on_done: Option<Box<dyn FnOnce()>>,
}

/// Animates one window's origin and alpha with springs.
#[derive(Clone)]
pub struct WindowSpring {
    inner: Rc<RefCell<Inner>>,
}

impl std::fmt::Debug for WindowSpring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WindowSpring")
    }
}

impl WindowSpring {
    pub fn new(window: Retained<NSWindow>) -> Self {
        WindowSpring {
            inner: Rc::new(RefCell::new(Inner { window, x: None, y: None, alpha: None, timer: None, on_done: None })),
        }
    }

    /// Spring the window's origin (AppKit global points) and alpha to the
    /// targets. In-flight animations are retargeted, keeping velocity.
    /// `on_done` replaces any pending completion.
    pub fn animate(&self, origin: (f64, f64), alpha: f64, spring: Spring, on_done: Option<Box<dyn FnOnce()>>) {
        let t = now();
        {
            let mut inner = self.inner.borrow_mut();
            let frame = inner.window.frame();
            let current_alpha = inner.window.alphaValue();
            let step = |slot: &mut Option<SpringAnimator>, current: f64, target: f64| match slot {
                Some(a) => a.retarget_with(spring, target, t),
                None => *slot = Some(SpringAnimator::new(spring, current, target, t)),
            };
            step(&mut inner.x, frame.origin.x, origin.0);
            step(&mut inner.y, frame.origin.y, origin.1);
            step(&mut inner.alpha, current_alpha, alpha);
            inner.on_done = on_done;
        }
        self.ensure_timer();
    }

    /// Jump immediately (no animation), cancelling any running spring.
    pub fn set(&self, origin: (f64, f64), alpha: f64) {
        let mut inner = self.inner.borrow_mut();
        inner.x = None;
        inner.y = None;
        inner.alpha = None;
        if let Some(t) = inner.timer.take() {
            t.invalidate();
        }
        inner.window.setFrameOrigin(CGPoint::new(origin.0, origin.1));
        inner.window.setAlphaValue(alpha);
    }

    pub fn is_animating(&self) -> bool {
        self.inner.borrow().timer.is_some()
    }

    fn ensure_timer(&self) {
        if self.inner.borrow().timer.is_some() {
            return;
        }
        let weak: Weak<RefCell<Inner>> = Rc::downgrade(&self.inner);
        // TODO(display-link): drive this from `NSScreen.displayLink` (macOS 14)
        // for vsync-aligned frames on ProMotion displays.
        let timer = schedule_timer(motion::FRAME_TICK, true, move || {
            let Some(inner) = weak.upgrade() else { return false };
            let done = {
                let Ok(mut inner) = inner.try_borrow_mut() else { return true };
                let t = now();
                let (Some(x), Some(y), Some(a)) = (inner.x, inner.y, inner.alpha) else {
                    inner.timer = None;
                    return false;
                };
                let settled = x.is_settled(t) && y.is_settled(t) && (a.value(t) - a.target()).abs() < 0.005;
                let (px, py, pa) = if settled {
                    (x.target(), y.target(), a.target())
                } else {
                    (x.value(t), y.value(t), a.value(t).clamp(0.0, 1.0))
                };
                inner.window.setFrameOrigin(CGPoint::new(px.round(), py.round()));
                inner.window.setAlphaValue(pa);
                if settled {
                    inner.x = None;
                    inner.y = None;
                    inner.alpha = None;
                    inner.timer = None;
                    inner.on_done.take()
                } else {
                    return true;
                }
            };
            if let Some(done) = done {
                done();
            }
            false
        });
        self.inner.borrow_mut().timer = Some(timer);
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(t) = self.timer.take() {
            t.invalidate();
        }
    }
}

/// Four springs animating a rectangle (origin + size) — used for the
/// overlay's selection and window highlight so the dim cutout, outline,
/// handles and pill all move in lock-step.
#[derive(Debug, Clone, Copy)]
pub struct RectSpring {
    parts: [SpringAnimator; 4],
}

impl RectSpring {
    pub fn new(spring: Spring, from: lumora_core::Rect, to: lumora_core::Rect, now: f64) -> Self {
        let f = [from.min_x(), from.min_y(), from.width(), from.height()];
        let t = [to.min_x(), to.min_y(), to.width(), to.height()];
        RectSpring { parts: std::array::from_fn(|i| SpringAnimator::new(spring, f[i], t[i], now)) }
    }

    pub fn value(&self, now: f64) -> lumora_core::Rect {
        let v: [f64; 4] = std::array::from_fn(|i| self.parts[i].value(now));
        lumora_core::Rect::new(v[0], v[1], v[2].max(0.0), v[3].max(0.0))
    }

    pub fn target(&self) -> lumora_core::Rect {
        let v: [f64; 4] = std::array::from_fn(|i| self.parts[i].target());
        lumora_core::Rect::new(v[0], v[1], v[2], v[3])
    }

    pub fn retarget(&mut self, to: lumora_core::Rect, now: f64) {
        let t = [to.min_x(), to.min_y(), to.width(), to.height()];
        for (p, v) in self.parts.iter_mut().zip(t) {
            p.retarget(v, now);
        }
    }

    pub fn is_settled(&self, now: f64) -> bool {
        self.parts.iter().all(|p| p.is_settled(now))
    }
}

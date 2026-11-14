//! Toast HUD: a small glass capsule near the top of the active screen that
//! reports outcomes (text copied, saved, a capture that failed) without
//! taking focus or blocking anything.
//!
//! It drops in from 12 pt above on [`motion::TOAST`] while fading in, waits
//! ([`motion::TOAST_SHORT`] for success, [`motion::TOAST_LONG`] for
//! warnings and errors), then floats up and fades out. A new toast replaces
//! the current one immediately. The panel ignores the mouse.

use std::cell::RefCell;
use std::rc::Rc;

use lumora_core::geometry::Rect;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSColor, NSFont, NSImageSymbolConfiguration, NSImageView, NSLineBreakMode, NSPanel, NSPopUpMenuWindowLevel,
    NSTextField, NSView,
};
use objc2_foundation::{NSString, NSTimer};

use crate::anim::WindowSpring;
use crate::tokens::{color, motion, radius, size, typography};
use crate::util::{
    cg_rect, glass_view, make_panel, ns_color, rect, schedule_timer, screen_under_mouse, symbol, text_width, weight,
    PanelSpec,
};

/// What kind of news a toast carries (picks glyph and tint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Warning,
    Error,
}

impl ToastKind {
    fn symbol(self) -> &'static str {
        match self {
            ToastKind::Success => "checkmark.circle.fill",
            ToastKind::Warning => "exclamationmark.triangle.fill",
            ToastKind::Error => "xmark.octagon.fill",
        }
    }

    fn tint(self) -> crate::tokens::Rgba {
        match self {
            ToastKind::Success => color::SUCCESS,
            ToastKind::Warning => color::WARNING,
            ToastKind::Error => color::ERROR,
        }
    }

    fn duration(self) -> f64 {
        match self {
            ToastKind::Success => motion::TOAST_SHORT,
            _ => motion::TOAST_LONG,
        }
    }
}

struct Toast {
    panel: Retained<NSPanel>,
    spring: WindowSpring,
    timer: Option<Retained<NSTimer>>,
}

impl Drop for Toast {
    fn drop(&mut self) {
        if let Some(t) = self.timer.take() {
            t.invalidate();
        }
        self.spring.set((self.panel.frame().origin.x, self.panel.frame().origin.y), 0.0);
        self.panel.orderOut(None);
    }
}

/// Owns the on-screen toast (at most one).
pub struct ToastCenter {
    mtm: MainThreadMarker,
    current: Rc<RefCell<Option<Toast>>>,
}

impl std::fmt::Debug for ToastCenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToastCenter").field("showing", &self.current.borrow().is_some()).finish()
    }
}

fn label(mtm: MainThreadMarker, text: &str, font: &NSFont, color: &NSColor) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(font));
    l.setTextColor(Some(color));
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l
}

impl ToastCenter {
    pub fn new(mtm: MainThreadMarker) -> Self {
        ToastCenter { mtm, current: Rc::new(RefCell::new(None)) }
    }

    /// Show a toast; replaces any toast already on screen.
    pub fn show(&self, kind: ToastKind, title: &str, detail: Option<&str>) {
        let mtm = self.mtm;
        drop(self.current.borrow_mut().take());

        let title_font = NSFont::systemFontOfSize_weight(typography::BODY, weight::SEMIBOLD);
        let detail_font = NSFont::systemFontOfSize_weight(typography::CAPTION, weight::REGULAR);
        let detail = detail.map(str::trim).filter(|d| !d.is_empty());
        let text_w = text_width(title, &title_font).max(detail.map_or(0.0, |d| text_width(d, &detail_font)));
        let (pad, icon) = (16.0, 20.0);
        let w = (pad + icon + 10.0 + text_w + pad + 4.0).clamp(size::TOAST_MIN_WIDTH, size::TOAST_MAX_WIDTH);
        let h = if detail.is_some() { size::TOAST_HEIGHT + 14.0 } else { size::TOAST_HEIGHT };

        let Some(screen) = screen_under_mouse(mtm) else { return };
        let visible = rect(screen.visibleFrame());
        let x = (visible.mid().x - w / 2.0).round();
        let y = (visible.max_y() - size::TOAST_TOP - h).round();
        let start = Rect::new(x, y + 12.0, w, h);

        let panel = make_panel(
            mtm,
            cg_rect(start),
            PanelSpec { level: NSPopUpMenuWindowLevel, keyable: false, shadow: true, ignores_mouse: true },
        );
        let local = Rect::new(0.0, 0.0, w, h);
        let root = NSView::initWithFrame(NSView::alloc(mtm), cg_rect(local));
        let glass = glass_view(mtm, cg_rect(local), radius::TOAST.min(h / 2.0));
        root.addSubview(&glass);

        if let Some(img) = symbol(kind.symbol(), title) {
            let cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(17.0, weight::SEMIBOLD);
            let img = img.imageWithSymbolConfiguration(&cfg).unwrap_or(img);
            let iv = NSImageView::imageViewWithImage(&img, mtm);
            iv.setContentTintColor(Some(&ns_color(kind.tint())));
            iv.setFrame(cg_rect(Rect::new(pad, (h - icon) / 2.0, icon, icon)));
            root.addSubview(&iv);
        }
        let text_x = pad + icon + 10.0;
        let text_w = w - text_x - pad;
        let white = NSColor::colorWithWhite_alpha(1.0, 0.96);
        let t = label(mtm, title, &title_font, &white);
        match detail {
            Some(d) => {
                t.setFrame(cg_rect(Rect::new(text_x, h / 2.0 + 1.0, text_w, 17.0)));
                let secondary = NSColor::colorWithWhite_alpha(1.0, 0.62);
                let dl = label(mtm, d, &detail_font, &secondary);
                dl.setFrame(cg_rect(Rect::new(text_x, h / 2.0 - 16.0, text_w, 15.0)));
                root.addSubview(&dl);
            }
            None => t.setFrame(cg_rect(Rect::new(text_x, (h - 17.0) / 2.0, text_w, 17.0))),
        }
        root.addSubview(&t);
        panel.setContentView(Some(&root));
        panel.setAlphaValue(0.0);
        panel.orderFrontRegardless();

        let window: Retained<objc2_app_kit::NSWindow> = Retained::into_super(panel.clone());
        let spring = WindowSpring::new(window);
        spring.animate((x, y), 1.0, motion::TOAST, None);

        let current = Rc::downgrade(&self.current);
        let timer = schedule_timer(kind.duration(), false, move || {
            if let Some(cur) = current.upgrade() {
                ToastCenter::dismiss(&cur);
            }
            false
        });
        *self.current.borrow_mut() = Some(Toast { panel, spring, timer: Some(timer) });
    }

    /// Float up and fade out, then release.
    fn dismiss(current: &Rc<RefCell<Option<Toast>>>) {
        let target = {
            let cur = current.borrow();
            let Some(t) = cur.as_ref() else { return };
            let f = t.panel.frame();
            (t.spring.clone(), (f.origin.x, f.origin.y + 8.0), t.panel.clone())
        };
        let (spring, origin, panel) = target;
        let weak = Rc::downgrade(current);
        spring.animate(
            origin,
            0.0,
            motion::REFLOW,
            Some(Box::new(move || {
                if let Some(cur) = weak.upgrade() {
                    // Only clear if it's still the same toast.
                    let same = cur.borrow().as_ref().is_some_and(|t| t.panel == panel);
                    if same {
                        drop(cur.borrow_mut().take());
                    }
                }
            })),
        );
        if let Some(t) = current.borrow_mut().as_mut() {
            t.timer = None;
        }
    }
}

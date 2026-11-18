//! Keystroke HUD while recording: a dark pill inside the recorded area that
//! shows the last shortcut or special key (`⇧⌘4`, `⌘Z ×3`, `↩`) and fades
//! out after [`HUD_DURATION`].
//!
//! Like the click-ripple overlay — and unlike the rest of the recording
//! chrome — the panel is meant to be **in** the video: the record flow
//! passes [`KeystrokeHud::window_numbers`] to the recorder, which leaves it
//! out of the excluded windows. The panel is non-activating and ignores the
//! mouse. What to show (and what never to show) is decided in
//! `lumora_core::keystroke`; this only draws text it is given.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use lumora_core::geometry::Rect;
use lumora_core::keystroke::{HudPosition, HUD_DURATION};
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSFont, NSPanel, NSScreen, NSScreenSaverWindowLevel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSTimer;
use objc2_quartz_core::{CALayer, CATextLayer};

use crate::tokens::color;
use crate::util::{
    cg_color, cg_rect, ease_scalar, layer_host_view, make_panel, schedule_timer, set_text, text_layer, text_width,
    weight, without_actions, PanelSpec,
};

const FONT_SIZE: f64 = 24.0;
const PILL_HEIGHT: f64 = 46.0;
const PILL_PADDING: f64 = 18.0;
const PILL_RADIUS: f64 = 12.0;
/// Distance from the edges of the recorded area.
const MARGIN: f64 = 28.0;
const FADE: f64 = 0.25;

/// The keystroke pill, one panel covering the recorded area.
pub struct KeystrokeHud {
    panel: Retained<NSPanel>,
    pill: Retained<CALayer>,
    text: Retained<CATextLayer>,
    font: Retained<NSFont>,
    size: (f64, f64),
    position: HudPosition,
    hide: RefCell<Option<Retained<NSTimer>>>,
    me: Weak<KeystrokeHud>,
}

impl std::fmt::Debug for KeystrokeHud {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeystrokeHud").field("position", &self.position).finish()
    }
}

impl KeystrokeHud {
    /// Put an (empty) HUD panel over `area` (AppKit global points, the
    /// recorded area). Must be on screen before the recorder fetches
    /// shareable content so its window can be allowed into the video.
    pub fn show(mtm: MainThreadMarker, area: Rect, position: HudPosition) -> Rc<KeystrokeHud> {
        let scale = NSScreen::screens(mtm)
            .iter()
            .find(|s| crate::util::rect(s.frame()).intersection(&area).is_some())
            .map_or(2.0, |s| s.backingScaleFactor());
        let panel = make_panel(
            mtm,
            cg_rect(area),
            PanelSpec { level: NSScreenSaverWindowLevel + 1, keyable: false, shadow: false, ignores_mouse: true },
        );
        let (view, root) = layer_host_view(mtm, cg_rect(Rect::new(0.0, 0.0, area.width(), area.height())));
        let font = NSFont::systemFontOfSize_weight(FONT_SIZE, weight::SEMIBOLD);
        let pill = CALayer::new();
        let text = text_layer(&font, FONT_SIZE, color::PILL_TEXT, scale);
        without_actions(|| {
            pill.setBackgroundColor(Some(&cg_color(color::PILL_BG)));
            pill.setCornerRadius(PILL_RADIUS);
            pill.setBorderColor(Some(&cg_color(color::HAIRLINE)));
            pill.setBorderWidth(1.0);
            pill.setContentsScale(scale);
            pill.setOpacity(0.0);
            pill.addSublayer(&text);
            root.addSublayer(&pill);
        });
        panel.setContentView(Some(&view));
        panel.orderFrontRegardless();
        Rc::new_cyclic(|me| KeystrokeHud {
            panel,
            pill,
            text,
            font,
            size: (area.width(), area.height()),
            position,
            hide: RefCell::new(None),
            me: me.clone(),
        })
    }

    /// Window number (`CGWindowID`) of the HUD panel.
    pub fn window_numbers(&self) -> Vec<u32> {
        vec![self.panel.windowNumber().max(0) as u32]
    }

    /// Show `label` now and fade it out after [`HUD_DURATION`] unless
    /// another label arrives first.
    pub fn display(&self, label: &str) {
        let (w, h) = self.size;
        let pill_w = (text_width(label, &self.font) + PILL_PADDING * 2.0).max(PILL_HEIGHT);
        let pill_w = pill_w.min((w - 8.0).max(1.0));
        let x = match self.position {
            HudPosition::BottomLeft => MARGIN,
            HudPosition::BottomRight => w - MARGIN - pill_w,
            HudPosition::BottomCenter | HudPosition::TopCenter => (w - pill_w) / 2.0,
        }
        .min((w - pill_w - 4.0).max(0.0))
        .max(0.0);
        // Root layer is geometry-flipped: y grows downwards.
        let y = match self.position {
            HudPosition::TopCenter => MARGIN,
            _ => h - MARGIN - PILL_HEIGHT,
        }
        .max(0.0);
        let text_h = FONT_SIZE * 1.3;
        without_actions(|| {
            self.pill.removeAllAnimations();
            self.pill.setFrame(CGRect::new(CGPoint::new(x, y), CGSize::new(pill_w, PILL_HEIGHT)));
            self.text
                .setFrame(CGRect::new(CGPoint::new(0.0, (PILL_HEIGHT - text_h) / 2.0), CGSize::new(pill_w, text_h)));
            set_text(&self.text, label);
            self.pill.setOpacity(1.0);
        });
        if let Some(t) = self.hide.borrow_mut().take() {
            t.invalidate();
        }
        let me = self.me.clone();
        let timer = schedule_timer(HUD_DURATION, false, move || {
            if let Some(hud) = me.upgrade() {
                ease_scalar(&hud.pill, "opacity", 1.0, 0.0, FADE);
            }
            false
        });
        *self.hide.borrow_mut() = Some(timer);
    }
}

impl Drop for KeystrokeHud {
    fn drop(&mut self) {
        if let Some(t) = self.hide.borrow_mut().take() {
            t.invalidate();
        }
        self.panel.orderOut(None);
    }
}

//! Recording chrome: the marching-ants border around the recorded area and
//! the floating stop control. Both are Lumora windows and are excluded from
//! the recording through the `SCContentFilter`.

use lumora_core::geometry::Rect;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSButton, NSColor, NSImage, NSPanel, NSScreenSaverWindowLevel, NSStatusWindowLevel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSNumber, NSString};
use objc2_quartz_core::{CABasicAnimation, CAMediaTiming, CAShapeLayer, CATextLayer};

use crate::events::{EventSink, UiEvent};
use crate::tokens::{color, motion, size, typography};
use crate::util::{
    cg_color, cg_rect, ease_scalar, glass_view, layer_host_view, make_panel, mono_digits, rect_path, set_text,
    spring_scalar, text_layer, weight, ActionTarget, PanelSpec,
};

/// Animated dashed outline around the recorded area.
pub struct RecordingBorder {
    panel: Retained<NSPanel>,
}

impl std::fmt::Debug for RecordingBorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecordingBorder")
    }
}

impl RecordingBorder {
    /// `area` in AppKit global points. The outline sits just *outside* the
    /// area so it never overlaps recorded pixels.
    pub fn show(mtm: MainThreadMarker, area: Rect) -> Self {
        let pad = size::BORDER_INSET + 2.0;
        let outer = area.inset(-pad, -pad);
        let panel = make_panel(
            mtm,
            cg_rect(outer),
            PanelSpec { level: NSScreenSaverWindowLevel, keyable: false, shadow: false, ignores_mouse: true },
        );
        let (view, root) = layer_host_view(mtm, cg_rect(Rect::new(0.0, 0.0, outer.width(), outer.height())));
        let dash = CAShapeLayer::new();
        dash.setPath(Some(&rect_path(Rect::new(1.0, 1.0, outer.width() - 2.0, outer.height() - 2.0))));
        dash.setFillColor(None);
        dash.setStrokeColor(Some(&cg_color(color::RECORD)));
        dash.setLineWidth(2.0);
        dash.setLineDashPattern(Some(&NSArray::from_retained_slice(&[
            NSNumber::numberWithDouble(6.0),
            NSNumber::numberWithDouble(4.0),
        ])));
        // March: animate the dash phase by one full period, forever.
        let march = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("lineDashPhase")));
        // SAFETY: NSNumber values for a scalar key path.
        unsafe {
            march.setFromValue(Some(&NSNumber::numberWithDouble(0.0)));
            march.setToValue(Some(&NSNumber::numberWithDouble(10.0)));
        }
        march.setDuration(motion::MARCH_PERIOD);
        march.setRepeatCount(f32::INFINITY);
        dash.addAnimation_forKey(&march, Some(&NSString::from_str("march")));
        root.addSublayer(&dash);
        panel.setContentView(Some(&view));
        panel.orderFrontRegardless();
        ease_scalar(&root, "opacity", 0.0, 1.0, 0.2);
        RecordingBorder { panel }
    }
}

impl Drop for RecordingBorder {
    fn drop(&mut self) {
        self.panel.orderOut(None);
    }
}

/// Floating glass pill with a stop button and the elapsed time.
pub struct StopControl {
    panel: Retained<NSPanel>,
    label: Retained<CATextLayer>,
    _target: Retained<ActionTarget>,
}

impl std::fmt::Debug for StopControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StopControl")
    }
}

impl StopControl {
    /// Place the control centred near the bottom of `visible` (the screen's
    /// visible frame, AppKit points), avoiding `area` when possible.
    pub fn show(mtm: MainThreadMarker, visible: Rect, area: Rect, sink: EventSink) -> Self {
        let (w, h) = (size::STOP_HUD_WIDTH, size::STOP_HUD_HEIGHT);
        let x = (visible.mid().x - w / 2.0).round();
        // Below the area if there's room, else at the bottom of the screen.
        let below = area.min_y() - h - 16.0;
        let y = if below >= visible.min_y() + 8.0 { below } else { visible.min_y() + 24.0 };
        let frame = CGRect::new(CGPoint::new(x, y), CGSize::new(w, h));
        let panel = make_panel(
            mtm,
            frame,
            PanelSpec { level: NSStatusWindowLevel, keyable: false, shadow: true, ignores_mouse: false },
        );
        panel.setMovableByWindowBackground(true);
        let local = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w, h));
        let glass = glass_view(mtm, local, h / 2.0);

        let target = ActionTarget::new(mtm, move |_| sink(UiEvent::StopRecording));
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("stop.fill"),
            Some(&NSString::from_str("Stop Recording")),
        );
        let button = match symbol {
            // SAFETY: target outlives the button (owned by StopControl).
            Some(img) => unsafe {
                NSButton::buttonWithImage_target_action(&img, Some(&target), Some(ActionTarget::action()), mtm)
            },
            None => unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("Stop"),
                    Some(&target),
                    Some(ActionTarget::action()),
                    mtm,
                )
            },
        };
        button.setBordered(false);
        button.setContentTintColor(Some(&NSColor::whiteColor()));
        button.setFrame(cg_rect(Rect::new(6.0, 6.0, h - 12.0, h - 12.0)));
        button.setWantsLayer(true);
        if let Some(l) = button.layer() {
            l.setBackgroundColor(Some(&cg_color(color::RECORD)));
            l.setCornerRadius((h - 12.0) / 2.0);
        }
        button.setToolTip(Some(&NSString::from_str("Stop Recording (⌥⇧R)")));
        glass.addSubview(&button);

        glass.setWantsLayer(true);
        let scale = panel.backingScaleFactor();
        let font = mono_digits(typography::HUD_TIMER, weight::SEMIBOLD);
        let label = text_layer(&font, typography::HUD_TIMER, color::PILL_TEXT, scale);
        // NSVisualEffectView's layer is not flipped: y is bottom-up here.
        label.setFrame(cg_rect(Rect::new(h, (h - 17.0) / 2.0, w - h - 10.0, 17.0)));
        set_text(&label, "0:00");
        if let Some(l) = glass.layer() {
            l.addSublayer(&label);
        }
        panel.setContentView(Some(&glass));
        panel.orderFrontRegardless();
        if let Some(l) = glass.layer() {
            spring_scalar(&l, "transform.scale", 0.9, 1.0, motion::POP);
        }
        StopControl { panel, label, _target: target }
    }

    pub fn set_elapsed(&self, seconds: f64) {
        crate::util::without_actions(|| set_text(&self.label, &lumora_core::format::elapsed(seconds)));
    }
}

impl Drop for StopControl {
    fn drop(&mut self) {
        self.panel.orderOut(None);
    }
}

//! Scrolling capture control: a small glass pill with "Scroll slowly…", the
//! number of frames stitched so far, Done and Cancel. Keyable but
//! non-activating, so ↩ / Esc reach it while the app being scrolled stays
//! frontmost. It sits outside the captured area when there is any room, and
//! Lumora is excluded from every capture as an application, so it never
//! shows up in the stitched image.

use lumora_core::geometry::Rect;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSButton, NSColor, NSFont, NSPanel, NSStatusWindowLevel, NSTextField};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

use lumora_core::i18n::{tr, trf, Msg};

use crate::events::{EventSink, UiEvent};
use crate::tokens::{motion, typography};
use crate::util::{cg_rect, glass_view, make_panel, spring_scalar, ActionTarget, KeyCatcherView, PanelSpec};

const KEY_ESCAPE: u16 = 53;
const KEY_RETURN: u16 = 36;
const KEY_ENTER: u16 = 76;
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 44.0;
const BUTTON_W: f64 = 72.0;

/// The floating control. Dropping it closes the panel.
pub struct ScrollControl {
    panel: Retained<NSPanel>,
    count: Retained<NSTextField>,
    _targets: [Retained<ActionTarget>; 2],
    _catcher: Retained<KeyCatcherView>,
    inside: bool,
}

impl std::fmt::Debug for ScrollControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScrollControl")
    }
}

fn label(mtm: MainThreadMarker, text: &str, frame: Rect, bold: bool) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font = if bold {
        NSFont::boldSystemFontOfSize(typography::HUD_TIMER)
    } else {
        NSFont::monospacedDigitSystemFontOfSize_weight(typography::HUD_TIMER, 0.0)
    };
    l.setFont(Some(&font));
    l.setTextColor(Some(&NSColor::whiteColor()));
    l.setFrame(cg_rect(frame));
    l
}

fn button(mtm: MainThreadMarker, title: &str, target: &ActionTarget, frame: Rect) -> Retained<NSButton> {
    // SAFETY: the target outlives the button (owned by ScrollControl).
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.setFrame(cg_rect(frame));
    b
}

impl ScrollControl {
    /// Place the control outside `area` (AppKit points): below it, above
    /// it, beside it, or in a corner of `visible` (the screen's visible
    /// frame) — see [`lumora_core::layout::scroll_control_frame`]. It never
    /// takes mouse-moved events; when only a spot inside the area is left
    /// (the area covers the screen) it ignores the mouse entirely, so the
    /// scroll wheel always reaches the content underneath (↩ / Esc and the
    /// shortcut still work).
    pub fn show(mtm: MainThreadMarker, visible: Rect, area: Rect, sink: EventSink) -> Self {
        let (placed, inside) = lumora_core::layout::scroll_control_frame(
            visible,
            area,
            lumora_core::geometry::Size::new(WIDTH, HEIGHT),
            16.0,
            8.0,
        );
        let frame = cg_rect(placed);
        let panel = make_panel(
            mtm,
            frame,
            PanelSpec { level: NSStatusWindowLevel, keyable: true, shadow: true, ignores_mouse: inside },
        );
        panel.setAcceptsMouseMovedEvents(false);
        panel.setMovableByWindowBackground(true);
        let local = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(WIDTH, HEIGHT));
        let glass = glass_view(mtm, local, HEIGHT / 2.0);

        let key_sink = sink.clone();
        let (catcher, _root) = KeyCatcherView::new(mtm, local, move |code| match code {
            KEY_ESCAPE => key_sink(UiEvent::ScrollCaptureCancel),
            KEY_RETURN | KEY_ENTER => key_sink(UiEvent::ScrollCaptureDone),
            _ => {}
        });
        glass.addSubview(&catcher);

        // NSVisualEffectView is not flipped: y is bottom-up.
        let text_h = 17.0;
        let ty = (HEIGHT - text_h) / 2.0;
        let prompt = label(mtm, tr(Msg::ScrollPrompt), Rect::new(16.0, ty, 104.0, text_h), true);
        glass.addSubview(&prompt);
        let count = label(mtm, &trf(Msg::ScrollFrames, &[&1]), Rect::new(120.0, ty, 96.0, text_h), false);
        count.setTextColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.7)));
        glass.addSubview(&count);

        let done_sink = sink.clone();
        let done_t = ActionTarget::new(mtm, move |_| done_sink(UiEvent::ScrollCaptureDone));
        let cancel_t = ActionTarget::new(mtm, move |_| sink(UiEvent::ScrollCaptureCancel));
        let by = (HEIGHT - 28.0) / 2.0;
        let cancel =
            button(mtm, tr(Msg::ScrollCancel), &cancel_t, Rect::new(WIDTH - 2.0 * BUTTON_W - 14.0, by, BUTTON_W, 28.0));
        glass.addSubview(&cancel);
        let done = button(mtm, tr(Msg::ScrollDone), &done_t, Rect::new(WIDTH - BUTTON_W - 10.0, by, BUTTON_W, 28.0));
        // ↩ / Esc go through the key catcher, which ignores auto-repeat (a
        // ↩ still held from confirming the selection must not finish). A
        // button key equivalent would fire on repeats, so Done is only
        // tinted like the default button.
        done.setBezelColor(Some(&NSColor::controlAccentColor()));
        glass.addSubview(&done);

        panel.setContentView(Some(&glass));
        panel.orderFrontRegardless();
        panel.makeKeyWindow();
        panel.makeFirstResponder(Some(&catcher));
        glass.setWantsLayer(true);
        if let Some(l) = glass.layer() {
            spring_scalar(&l, "transform.scale", 0.9, 1.0, motion::POP);
        }
        ScrollControl { panel, count, _targets: [done_t, cancel_t], _catcher: catcher, inside }
    }

    /// The panel had to go inside the captured area (and ignores the mouse).
    pub fn is_inside_area(&self) -> bool {
        self.inside
    }

    /// The panel's window number (for logs).
    pub fn window_number(&self) -> isize {
        self.panel.windowNumber()
    }

    /// Show how many frames are stitched so far.
    pub fn set_frames(&self, n: usize) {
        self.count.setStringValue(&NSString::from_str(&trf(Msg::ScrollFrames, &[&n])));
    }
}

impl Drop for ScrollControl {
    fn drop(&mut self) {
        self.panel.orderOut(None);
    }
}

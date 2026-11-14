//! First-run onboarding: explains why Lumora needs Screen Recording access,
//! asks for it, and offers a relaunch once granted (macOS only applies the
//! permission to new processes).

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBezelStyle, NSButton, NSFont, NSImageView, NSTextAlignment, NSTextField,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

use crate::events::{EventSink, OnboardingAction, UiEvent};
use crate::tokens::typography;
use crate::util::{cg_rect, weight, ActionTarget};
use lumora_core::geometry::Rect;

const W: f64 = 440.0;
const H: f64 = 400.0;

/// The onboarding window.
pub struct Onboarding {
    window: Retained<NSWindow>,
    body: Retained<NSTextField>,
    primary: Retained<NSButton>,
    secondary: Retained<NSButton>,
    _targets: Vec<Retained<ActionTarget>>,
    granted: std::cell::Cell<bool>,
}

impl std::fmt::Debug for Onboarding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Onboarding").field("granted", &self.granted.get()).finish()
    }
}

const BODY_ASK: &str =
    "Lumora captures your screen only when you ask it to — a shortcut, the menu bar, or a recording you start. \
macOS requires you to allow Screen Recording once.\n\nClick Grant Access, then switch Lumora on in System Settings.";
const BODY_DONE: &str =
    "Screen Recording is allowed. Relaunch Lumora so macOS applies it, then press ⌥⇧4 to capture an area.";

impl Onboarding {
    pub fn show(mtm: MainThreadMarker, sink: EventSink, granted: bool) -> Self {
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::FullSizeContentView;
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(W, H));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setMovableByWindowBackground(true);
        window.setTitle(&NSString::from_str("Welcome to Lumora"));

        let bg = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        bg.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        bg.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        bg.setState(NSVisualEffectState::FollowsWindowActiveState);

        let mark = NSImageView::imageViewWithImage(&crate::icon::app_mark(96.0), mtm);
        mark.setFrame(cg_rect(Rect::new((W - 96.0) / 2.0, H - 60.0 - 96.0, 96.0, 96.0)));
        bg.addSubview(&mark);

        let title = NSTextField::labelWithString(&NSString::from_str("Welcome to Lumora"), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(typography::TITLE, weight::BOLD)));
        title.setAlignment(NSTextAlignment::Center);
        title.setFrame(cg_rect(Rect::new(20.0, H - 196.0, W - 40.0, 30.0)));
        bg.addSubview(&title);

        let body =
            NSTextField::wrappingLabelWithString(&NSString::from_str(if granted { BODY_DONE } else { BODY_ASK }), mtm);
        body.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)));
        body.setAlignment(NSTextAlignment::Center);
        body.setFrame(cg_rect(Rect::new(44.0, 92.0, W - 88.0, 100.0)));
        bg.addSubview(&body);

        let mut targets = Vec::new();
        let mut button = |title: &str, x: f64, w: f64, action: OnboardingAction, primary: bool| {
            let sink = sink.clone();
            let t = ActionTarget::new(mtm, move |_| sink(UiEvent::Onboarding(action)));
            // SAFETY: target kept alive in `targets`.
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&t),
                    Some(ActionTarget::action()),
                    mtm,
                )
            };
            b.setBezelStyle(NSBezelStyle::Push);
            b.setFrame(cg_rect(Rect::new(x, 36.0, w, 32.0)));
            if primary {
                b.setKeyEquivalent(&NSString::from_str("\r"));
            }
            bg.addSubview(&b);
            targets.push(t);
            b
        };
        let (primary_action, primary_title) = if granted {
            (OnboardingAction::Relaunch, "Relaunch")
        } else {
            (OnboardingAction::RequestAccess, "Grant Access")
        };
        let secondary = button("Open System Settings", 40.0, 180.0, OnboardingAction::OpenSystemSettings, false);
        let primary = button(primary_title, W - 40.0 - 180.0, 180.0, primary_action, true);
        let later_sink = sink.clone();
        let later = ActionTarget::new(mtm, move |_| later_sink(UiEvent::Onboarding(OnboardingAction::Dismiss)));
        let later_btn = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Not Now"),
                Some(&later),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        later_btn.setBordered(false);
        later_btn.setFrame(cg_rect(Rect::new((W - 100.0) / 2.0, 8.0, 100.0, 24.0)));
        bg.addSubview(&later_btn);
        targets.push(later);

        window.setContentView(Some(&bg));
        window.center();
        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        Onboarding { window, body, primary, secondary, _targets: targets, granted: std::cell::Cell::new(granted) }
    }

    /// Switch to the "granted — relaunch" state.
    pub fn set_granted(&self, mtm: MainThreadMarker, sink: EventSink) -> Option<Retained<ActionTarget>> {
        if self.granted.replace(true) {
            return None;
        }
        self.body.setStringValue(&NSString::from_str(BODY_DONE));
        self.primary.setTitle(&NSString::from_str("Relaunch"));
        let t = ActionTarget::new(mtm, move |_| sink(UiEvent::Onboarding(OnboardingAction::Relaunch)));
        // SAFETY: the caller keeps the returned target alive.
        unsafe {
            self.primary.setTarget(Some(&t));
            self.primary.setAction(Some(ActionTarget::action()));
        }
        self.secondary.setHidden(true);
        Some(t)
    }

    pub fn close(&self) {
        self.window.orderOut(None);
    }
}

impl Drop for Onboarding {
    fn drop(&mut self) {
        self.window.orderOut(None);
    }
}

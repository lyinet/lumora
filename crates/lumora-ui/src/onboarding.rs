//! First-run onboarding: explains why Lumora needs Screen Recording access,
//! asks for it, and offers a relaunch once granted (macOS only applies the
//! permission to new processes).

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSFont, NSImageView, NSTextAlignment, NSTextField, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSString};

use lumora_core::i18n::{tr, trf, Msg};

use crate::events::{EventSink, OnboardingAction, UiEvent};
use crate::tokens::typography;
use crate::util::{cg_rect, weight, ActionTarget};
use lumora_core::geometry::Rect;

const W: f64 = 440.0;
const H: f64 = 400.0;

struct DelegateIvars {
    sink: RefCell<Option<EventSink>>,
}

define_class!(
    /// Closing the window (red button, Esc, ⌘W) counts as "Not Now", so the
    /// app forgets the panel and shows a fresh one next time.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraOnboardingWindowDelegate"]
    #[ivars = DelegateIvars]
    struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            let sink = self.ivars().sink.borrow().clone();
            if let Some(sink) = sink {
                sink(UiEvent::Onboarding(OnboardingAction::Dismiss));
            }
        }
    }
);

impl WindowDelegate {
    fn new(mtm: MainThreadMarker, sink: EventSink) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { sink: RefCell::new(Some(sink)) });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// The onboarding window.
pub struct Onboarding {
    window: Retained<NSWindow>,
    delegate: Retained<WindowDelegate>,
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

fn body_ask() -> &'static str {
    tr(Msg::OnboardingAsk)
}

fn body_done() -> String {
    trf(Msg::OnboardingDone, &[&lumora_core::Action::CaptureArea.default_shortcut()])
}

impl Onboarding {
    pub fn show(mtm: MainThreadMarker, sink: EventSink, granted: bool) -> Self {
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::FullSizeContentView;
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(W, H));
        // Esc closes it like ⌘W.
        let window = crate::esc_window::create(mtm, frame, style);
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setMovableByWindowBackground(true);
        window.setTitle(&NSString::from_str(tr(Msg::WelcomeTitle)));

        let bg = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        bg.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        bg.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        bg.setState(NSVisualEffectState::FollowsWindowActiveState);

        let mark = NSImageView::imageViewWithImage(&crate::icon::app_mark(96.0), mtm);
        mark.setFrame(cg_rect(Rect::new((W - 96.0) / 2.0, H - 60.0 - 96.0, 96.0, 96.0)));
        bg.addSubview(&mark);

        let title = NSTextField::labelWithString(&NSString::from_str(tr(Msg::WelcomeTitle)), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(typography::TITLE, weight::BOLD)));
        title.setAlignment(NSTextAlignment::Center);
        title.setFrame(cg_rect(Rect::new(20.0, H - 196.0, W - 40.0, 30.0)));
        bg.addSubview(&title);

        let body = NSTextField::wrappingLabelWithString(
            &NSString::from_str(&if granted { body_done() } else { body_ask().to_string() }),
            mtm,
        );
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
            (OnboardingAction::Relaunch, tr(Msg::Relaunch))
        } else {
            (OnboardingAction::RequestAccess, tr(Msg::GrantAccess))
        };
        let secondary = button(tr(Msg::OpenSystemSettings), 40.0, 180.0, OnboardingAction::OpenSystemSettings, false);
        let primary = button(primary_title, W - 40.0 - 180.0, 180.0, primary_action, true);
        let later_sink = sink.clone();
        let later = ActionTarget::new(mtm, move |_| later_sink(UiEvent::Onboarding(OnboardingAction::Dismiss)));
        let later_btn = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(tr(Msg::NotNow)),
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
        let delegate = WindowDelegate::new(mtm, sink.clone());
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.center();
        // Float above other apps' windows: Lumora has no Dock icon, so a
        // panel left behind the frontmost app is as good as invisible.
        window.setLevel(objc2_app_kit::NSFloatingWindowLevel);
        crate::util::force_activate(mtm);
        window.makeKeyAndOrderFront(None);
        window.orderFrontRegardless();
        Onboarding {
            window,
            delegate,
            body,
            primary,
            secondary,
            _targets: targets,
            granted: std::cell::Cell::new(granted),
        }
    }

    /// Switch to the "granted — relaunch" state.
    pub fn set_granted(&self, mtm: MainThreadMarker, sink: EventSink) -> Option<Retained<ActionTarget>> {
        if self.granted.replace(true) {
            return None;
        }
        self.body.setStringValue(&NSString::from_str(&body_done()));
        self.primary.setTitle(&NSString::from_str(tr(Msg::Relaunch)));
        let t = ActionTarget::new(mtm, move |_| sink(UiEvent::Onboarding(OnboardingAction::Relaunch)));
        // SAFETY: the caller keeps the returned target alive.
        unsafe {
            self.primary.setTarget(Some(&t));
            self.primary.setAction(Some(ActionTarget::action()));
        }
        self.secondary.setHidden(true);
        Some(t)
    }

    /// Show the window again (it may have been closed with its close button
    /// or Esc, or be hidden behind other windows).
    pub fn bring_to_front(&self, mtm: MainThreadMarker) {
        if !self.window.isVisible() {
            self.window.center();
        }
        crate::util::force_activate(mtm);
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
    }

    /// Whether the window is on screen.
    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    pub fn close(&self) {
        self.window.orderOut(None);
    }
}

impl Drop for Onboarding {
    fn drop(&mut self) {
        self.delegate.ivars().sink.borrow_mut().take();
        self.window.setDelegate(None);
        self.window.orderOut(None);
    }
}

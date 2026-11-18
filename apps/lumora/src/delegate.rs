//! `NSApplicationDelegate` and menu action receiver.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSURL};

use crate::app::{on_main, post_command, App};
use crate::command::Command;

pub struct DelegateIvars {
    app: RefCell<Option<Rc<App>>>,
    /// Documents Finder asked us to open before the app finished starting.
    pending: RefCell<Vec<PathBuf>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraAppDelegate"]
    #[ivars = DelegateIvars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            // Translocated / DMG copy: offer to move first (quits on success).
            if crate::relocate::check(self.mtm()) {
                return;
            }
            let at_login = launched_as_login_item();
            log::info!("launched{}", if at_login { " as a login item" } else { "" });
            let app = App::start(self.mtm(), self, at_login);
            *self.ivars().app.borrow_mut() = Some(app);
            let pending = std::mem::take(&mut *self.ivars().pending.borrow_mut());
            if !pending.is_empty() {
                on_main(move |app| app.open_documents(pending));
            }
        }

        /// Finder double-click / Open With on `.lumora` documents.
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            let paths: Vec<PathBuf> =
                urls.iter().filter_map(|u| u.path()).map(|p| PathBuf::from(p.to_string())).collect();
            if self.ivars().app.borrow().is_some() {
                on_main(move |app| app.open_documents(paths));
            } else {
                self.ivars().pending.borrow_mut().extend(paths);
            }
        }

        /// Opening Lumora again while it runs (Finder, Spotlight, `open`).
        /// A menu-bar app has no window to bring back, so show one.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _application: &NSApplication, _has_visible_windows: bool) -> bool {
            if self.ivars().app.borrow().is_some() {
                on_main(|app| app.handle_reopen());
            }
            true
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            if let Some(app) = self.ivars().app.borrow_mut().take() {
                app.shutdown();
            }
        }
    }

    // Menu actions. Each forwards to the central dispatcher.
    impl AppDelegate {
        #[unsafe(method(captureArea:))]
        fn capture_area(&self, _sender: Option<&AnyObject>) {
            post_command(Command::CaptureArea);
        }

        #[unsafe(method(captureFullscreen:))]
        fn capture_fullscreen(&self, _sender: Option<&AnyObject>) {
            post_command(Command::CaptureFullscreen);
        }

        #[unsafe(method(captureWindow:))]
        fn capture_window(&self, _sender: Option<&AnyObject>) {
            post_command(Command::CaptureWindow);
        }

        #[unsafe(method(captureText:))]
        fn capture_text(&self, _sender: Option<&AnyObject>) {
            post_command(Command::CaptureText);
        }

        #[unsafe(method(toggleRecording:))]
        fn toggle_recording(&self, _sender: Option<&AnyObject>) {
            post_command(Command::ToggleRecording);
        }

        #[unsafe(method(openFolder:))]
        fn open_folder(&self, _sender: Option<&AnyObject>) {
            post_command(Command::OpenFolder);
        }

        #[unsafe(method(scrollingCapture:))]
        fn scrolling_capture(&self, _sender: Option<&AnyObject>) {
            post_command(Command::ScrollingCapture);
        }

        #[unsafe(method(showHistory:))]
        fn show_history(&self, _sender: Option<&AnyObject>) {
            post_command(Command::ShowHistory);
        }

        #[unsafe(method(openLumoraDocument:))]
        fn open_lumora_document(&self, _sender: Option<&AnyObject>) {
            post_command(Command::OpenDocument);
        }

        #[unsafe(method(makeTutorial:))]
        fn make_tutorial(&self, _sender: Option<&AnyObject>) {
            post_command(Command::MakeTutorial);
        }

        #[unsafe(method(transcribeRecording:))]
        fn transcribe_recording(&self, _sender: Option<&AnyObject>) {
            post_command(Command::Transcribe);
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&AnyObject>) {
            post_command(Command::ShowSettings);
        }

        #[unsafe(method(showOnboarding:))]
        fn show_onboarding(&self, _sender: Option<&AnyObject>) {
            post_command(Command::ShowOnboarding);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            post_command(Command::Quit);
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this =
            Self::alloc(mtm).set_ivars(DelegateIvars { app: RefCell::new(None), pending: RefCell::new(Vec::new()) });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Whether the launch Apple Event says we were started as a login item
/// (`keyAEPropData` = `keyAELaunchedAsLogInItem`). Best effort: `false`
/// when there is no event or it says nothing.
fn launched_as_login_item() -> bool {
    use objc2::runtime::AnyClass;
    const KEY_AE_PROP_DATA: u32 = u32::from_be_bytes(*b"prdt");
    const KEY_AE_LAUNCHED_AS_LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");
    let Some(cls) = AnyClass::get(c"NSAppleEventManager") else { return false };
    // SAFETY: documented NSAppleEventManager / NSAppleEventDescriptor API;
    // every object may be nil and is checked.
    unsafe {
        let mgr: Option<Retained<AnyObject>> = msg_send![cls, sharedAppleEventManager];
        let Some(mgr) = mgr else { return false };
        let event: Option<Retained<AnyObject>> = msg_send![&*mgr, currentAppleEvent];
        let Some(event) = event else { return false };
        let prop: Option<Retained<AnyObject>> = msg_send![&*event, paramDescriptorForKeyword: KEY_AE_PROP_DATA];
        let Some(prop) = prop else { return false };
        let code: u32 = msg_send![&*prop, enumCodeValue];
        code == KEY_AE_LAUNCHED_AS_LOGIN_ITEM
    }
}

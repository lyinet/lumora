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
            let app = App::start(self.mtm(), self);
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

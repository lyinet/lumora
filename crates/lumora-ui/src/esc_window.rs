//! Document windows that close on Esc without losing an edit in progress.
//!
//! Esc while a text field is being edited ends the edit (keeping the typed
//! text) instead of reaching the field editor — which could revert it or pop
//! up completions — or closing the window under the user; a second Esc (or
//! ⌘W) closes. Esc during input-method composition (pinyin…) still goes to
//! the IME. Used by the transcript, tutorial and onboarding windows.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSEvent, NSEventType, NSResponder, NSTextField, NSTextInputClient, NSTextView, NSWindow,
    NSWindowStyleMask,
};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSObject, NSObjectProtocol};

/// Virtual key code of Esc.
const KEY_ESCAPE: u16 = 53;

define_class!(
    /// A document window that closes on Esc (⌘. too), like a sheet, and
    /// commits a text edit on the first Esc.
    #[unsafe(super(NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraEscWindow"]
    struct EscWindow;

    unsafe impl NSObjectProtocol for EscWindow {}

    impl EscWindow {
        /// Esc while a field is being edited ends the edit (see the module
        /// docs).
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if event.r#type() == NSEventType::KeyDown && event.keyCode() == KEY_ESCAPE && self.commit_editing() {
                return;
            }
            // SAFETY: forwarding the event to NSWindow's implementation.
            unsafe { msg_send![super(self), sendEvent: event] }
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            if !self.commit_editing() {
                self.performClose(None);
            }
        }
    }
);

impl EscWindow {
    /// When a text field is being edited (and no IME composition is in
    /// progress), end the edit, committing its text, and return `true`.
    fn commit_editing(&self) -> bool {
        let Some(responder) = self.firstResponder() else { return false };
        let Some(text) = responder.downcast_ref::<NSTextView>() else { return false };
        if !text.isFieldEditor() || text.hasMarkedText() {
            return false;
        }
        // Resigning first responder ends editing and keeps the typed text.
        self.makeFirstResponder(None);
        true
    }

    fn create(mtm: MainThreadMarker, frame: CGRect, style: NSWindowStyleMask) -> Retained<NSWindow> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSWindow's designated initialiser on a fresh subclass instance.
        let w: Retained<Self> = unsafe {
            msg_send![super(this), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        Retained::into_super(w)
    }
}

/// A new Esc-aware window (see the module docs).
pub fn create(mtm: MainThreadMarker, frame: CGRect, style: NSWindowStyleMask) -> Retained<NSWindow> {
    EscWindow::create(mtm, frame, style)
}

/// A field's text, including an edit in progress (read from its field
/// editor, so nothing typed is lost when a window acts mid-edit).
pub fn field_text(field: &NSTextField) -> String {
    match field.currentEditor() {
        Some(editor) => editor.string().to_string(),
        None => field.stringValue().to_string(),
    }
}

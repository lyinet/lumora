//! System-wide hotkeys via Carbon's `RegisterEventHotKey`.
//!
//! Still the only public API for global shortcuts that works without
//! Accessibility permission (an `NSEvent` global monitor would need it and
//! can't swallow the key). Events are delivered on the main run loop.

use std::ffi::c_void;
use std::ptr::null_mut;

use lumora_core::hotkey::Hotkey;
use lumora_core::Action;

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerUPP = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

const fn four_cc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
}

const K_EVENT_CLASS_KEYBOARD: u32 = four_cc(b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = four_cc(b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = four_cc(b"hkid");
const SIGNATURE: u32 = four_cc(b"LUMR");

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerUPP,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OSStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
}

/// Called by Carbon on the main thread.
extern "C" fn on_hotkey(_next: EventHandlerCallRef, event: EventRef, _user: *mut c_void) -> OSStatus {
    let mut id = EventHotKeyID::default();
    // SAFETY: reading the documented direct-object parameter into a
    // correctly sized EventHotKeyID.
    let status = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            null_mut(),
            (&mut id as *mut EventHotKeyID).cast(),
        )
    };
    if status == 0 && id.signature == SIGNATURE {
        if let Some(action) = Action::from_id(id.id) {
            crate::app::post_command(action.into());
        }
    }
    0
}

/// Owns the registered hotkeys.
pub struct HotkeyManager {
    registered: Vec<(Action, EventHotKeyRef)>,
    installed: bool,
    suspended: bool,
    bindings: Vec<(Action, Hotkey)>,
}

impl HotkeyManager {
    pub fn new() -> Self {
        HotkeyManager { registered: Vec::new(), installed: false, suspended: false, bindings: Vec::new() }
    }

    fn install(&mut self) {
        if self.installed {
            return;
        }
        let spec = EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOT_KEY_PRESSED };
        // SAFETY: valid target, handler and spec; handler lives forever.
        let status =
            unsafe { InstallEventHandler(GetApplicationEventTarget(), on_hotkey, 1, &spec, null_mut(), null_mut()) };
        if status == 0 {
            self.installed = true;
        } else {
            log::error!("InstallEventHandler failed: {status}");
        }
    }

    /// Replace all bindings. Returns the actions whose shortcut could not be
    /// registered (already taken by another app).
    pub fn set(&mut self, bindings: Vec<(Action, Hotkey)>) -> Vec<Action> {
        self.bindings = bindings;
        if self.suspended {
            return Vec::new();
        }
        self.apply()
    }

    fn apply(&mut self) -> Vec<Action> {
        self.install();
        self.unregister_all();
        let mut failed = Vec::new();
        for (action, hk) in &self.bindings {
            let mut r: EventHotKeyRef = null_mut();
            let id = EventHotKeyID { signature: SIGNATURE, id: action.id() };
            // SAFETY: valid key code / modifiers; out-pointer is valid.
            let status = unsafe {
                RegisterEventHotKey(
                    u32::from(hk.key.code),
                    hk.modifiers.carbon_mask(),
                    id,
                    GetApplicationEventTarget(),
                    0,
                    &mut r,
                )
            };
            if status == 0 {
                self.registered.push((*action, r));
            } else {
                log::warn!("could not register {hk} for {:?} (status {status})", action);
                failed.push(*action);
            }
        }
        failed
    }

    fn unregister_all(&mut self) {
        for (_, r) in self.registered.drain(..) {
            // SAFETY: `r` came from RegisterEventHotKey.
            unsafe { UnregisterEventHotKey(r) };
        }
    }

    /// Temporarily release all shortcuts (while the user records a new one).
    pub fn suspend(&mut self, on: bool) {
        if self.suspended == on {
            return;
        }
        self.suspended = on;
        if on {
            self.unregister_all();
        } else {
            self.apply();
        }
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.unregister_all();
    }
}

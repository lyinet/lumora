//! Listen-only keyboard event tap for the recording keystroke HUD, and the
//! Input Monitoring permission (TCC) it needs.
//!
//! * [`has_input_monitoring`] / [`request_input_monitoring`] wrap
//!   `CGPreflightListenEventAccess` / `CGRequestListenEventAccess`.
//! * [`KeyTap`] is a `kCGSessionEventTap` with `kCGEventTapOptionListenOnly`
//!   on keyDown + flagsChanged, attached to the main run loop. It never
//!   modifies or swallows events, skips everything while secure event input
//!   is on (password fields), and keeps nothing: each event is handed to the
//!   callback and forgotten. Dropping the tap removes it.

use std::ffi::c_void;

/// Raw `CGEventType` values we listen to.
const KEY_DOWN: u32 = 10;
const FLAGS_CHANGED: u32 = 12;
/// `kCGEventTapDisabledByTimeout` / `…ByUserInput`.
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
/// `kCGSessionEventTap`, `kCGHeadInsertEventTap`, `kCGEventTapOptionListenOnly`.
const SESSION_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const LISTEN_ONLY: u32 = 1;
/// `kCGKeyboardEventAutorepeat`, `kCGKeyboardEventKeycode`.
const FIELD_AUTOREPEAT: u32 = 8;
const FIELD_KEYCODE: u32 = 9;

type TapCallback = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventGetFlags(event: *mut c_void) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFMachPortCreateRunLoopSource(allocator: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFMachPortInvalidate(port: *mut c_void);
    fn CFRelease(cf: *const c_void);
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

/// Whether Lumora may listen to keyboard events (Input Monitoring). Never
/// prompts.
pub fn has_input_monitoring() -> bool {
    // SAFETY: plain query, no arguments.
    unsafe { CGPreflightListenEventAccess() }
}

/// Ask for Input Monitoring: the first call shows the system prompt (and
/// adds Lumora to the list in System Settings); later calls just return
/// the current state. A grant takes effect for taps created afterwards.
pub fn request_input_monitoring() -> bool {
    // SAFETY: plain request, no arguments.
    unsafe { CGRequestListenEventAccess() }
}

/// URL that opens System Settings at the Input Monitoring pane.
pub const INPUT_MONITORING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";

/// What a [`KeyTap`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// A key press: virtual key code, raw `CGEventFlags`, auto-repeat.
    KeyDown { keycode: u16, flags: u64, autorepeat: bool },
    /// Modifier keys changed: raw `CGEventFlags`.
    FlagsChanged { flags: u64 },
}

struct Context {
    handler: Box<dyn Fn(KeyEvent)>,
    port: std::cell::Cell<*mut c_void>,
}

unsafe extern "C" fn tap_callback(
    _proxy: *mut c_void,
    kind: u32,
    event: *mut c_void,
    refcon: *mut c_void,
) -> *mut c_void {
    // SAFETY: `refcon` is the `Context` boxed in `KeyTap::start`, alive
    // until the tap is invalidated in `Drop` (both on the main thread).
    let ctx = unsafe { &*(refcon as *const Context) };
    if kind == TAP_DISABLED_BY_TIMEOUT || kind == TAP_DISABLED_BY_USER_INPUT {
        let port = ctx.port.get();
        if !port.is_null() {
            // SAFETY: the port is valid until `Drop` invalidates it.
            unsafe { CGEventTapEnable(port, true) };
        }
        return event;
    }
    // SAFETY: plain query.
    if event.is_null() || unsafe { IsSecureEventInputEnabled() } != 0 {
        return event;
    }
    // SAFETY: `event` is the valid CGEventRef the tap was called with.
    let flags = unsafe { CGEventGetFlags(event) };
    let report = match kind {
        KEY_DOWN => KeyEvent::KeyDown {
            // SAFETY: as above.
            keycode: unsafe { CGEventGetIntegerValueField(event, FIELD_KEYCODE) } as u16,
            autorepeat: unsafe { CGEventGetIntegerValueField(event, FIELD_AUTOREPEAT) } != 0,
            flags,
        },
        FLAGS_CHANGED => KeyEvent::FlagsChanged { flags },
        _ => return event,
    };
    // A panic must not unwind into CoreGraphics.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (ctx.handler)(report)));
    event
}

/// A listen-only session event tap on the main run loop; removed on drop.
pub struct KeyTap {
    port: *mut c_void,
    source: *mut c_void,
    ctx: *mut Context,
}

impl std::fmt::Debug for KeyTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyTap")
    }
}

impl KeyTap {
    /// Start listening; `handler` runs on the main thread for every key-down
    /// and modifier change outside secure input. Returns `None` when the tap
    /// cannot be created (usually: no Input Monitoring permission).
    /// Call on the main thread.
    pub fn start(handler: impl Fn(KeyEvent) + 'static) -> Option<KeyTap> {
        let ctx = Box::into_raw(Box::new(Context {
            handler: Box::new(handler),
            port: std::cell::Cell::new(std::ptr::null_mut()),
        }));
        let mask = (1u64 << KEY_DOWN) | (1u64 << FLAGS_CHANGED);
        // SAFETY: valid callback and refcon (the boxed context outlives the tap).
        let port = unsafe { CGEventTapCreate(SESSION_TAP, HEAD_INSERT, LISTEN_ONLY, mask, tap_callback, ctx.cast()) };
        if port.is_null() {
            // SAFETY: `ctx` came from `Box::into_raw` and was never shared.
            drop(unsafe { Box::from_raw(ctx) });
            return None;
        }
        // SAFETY: `ctx` is valid; `port` is a live CFMachPort we own.
        unsafe {
            (*ctx).port.set(port);
            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
            if source.is_null() {
                CFMachPortInvalidate(port);
                CFRelease(port);
                drop(Box::from_raw(ctx));
                return None;
            }
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            CGEventTapEnable(port, true);
            Some(KeyTap { port, source, ctx })
        }
    }
}

impl Drop for KeyTap {
    fn drop(&mut self) {
        // SAFETY: all three were created in `start` and are released once.
        unsafe {
            CGEventTapEnable(self.port, false);
            CFRunLoopRemoveSource(CFRunLoopGetMain(), self.source, kCFRunLoopCommonModes);
            CFMachPortInvalidate(self.port);
            CFRelease(self.source);
            CFRelease(self.port);
            drop(Box::from_raw(self.ctx));
        }
    }
}

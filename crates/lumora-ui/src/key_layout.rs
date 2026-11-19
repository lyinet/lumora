//! Keyboard-layout lookups for the keystroke HUD (thin FFI; the rules are
//! in [`lumora_core::keystroke`]).
//!
//! [`LayoutTranslator`] implements [`KeyTranslator`] with Text Input
//! Sources + `UCKeyTranslate`:
//!
//! * `LayoutKind::Current` → `TISCopyCurrentKeyboardLayoutInputSource` (for
//!   an input method such as Pinyin, the layout it types through);
//! * `LayoutKind::AsciiCapable` → `TISCopyCurrentASCIICapableKeyboardLayoutInputSource`
//!   (what macOS uses for ⌘ shortcuts on Russian, Greek… layouts).
//!
//! Each translation is a key-down with no dead-key state
//! (`kUCKeyTranslateNoDeadKeysBit`). Results are cached per input source id
//! ([`LayoutCache`]); the sources are re-read when the translator is created
//! (each recording) and after `kTISNotifySelectedKeyboardInputSourceChanged`
//! (a distributed notification, observed on the main run loop, which only
//! sets a flag). Everything runs on the main thread (TIS requires it).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use lumora_core::keystroke::{KeyTranslator, LayoutCache, LayoutKind, TranslateMods, Translation};

/// `kUCKeyActionDown`.
const KEY_ACTION_DOWN: u16 = 0;
/// `kUCKeyTranslateNoDeadKeysMask` (`1 << kUCKeyTranslateNoDeadKeysBit`).
const NO_DEAD_KEYS: u32 = 1;
/// `(cmdKey >> 8) & 0xFF` — UCKeyTranslate wants Carbon modifiers shifted.
const UC_CMD: u32 = 1;
/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;
/// `CFNotificationSuspensionBehaviorDeliverImmediately`.
const DELIVER_IMMEDIATELY: isize = 4;

type NotificationCallback = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *const c_void, *const c_void);

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    static kTISPropertyInputSourceID: *const c_void;
    static kTISNotifySelectedKeyboardInputSourceChanged: *const c_void;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut c_void;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *const c_void;
    fn LMGetKbdType() -> u8;
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        actual_length: *mut usize,
        unicode: *mut u16,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFStringGetLength(s: *const c_void) -> isize;
    fn CFStringGetCString(s: *const c_void, buf: *mut u8, size: isize, encoding: u32) -> bool;
    fn CFRelease(cf: *const c_void);
    fn CFNotificationCenterGetDistributedCenter() -> *mut c_void;
    fn CFNotificationCenterAddObserver(
        center: *mut c_void,
        observer: *const c_void,
        callback: NotificationCallback,
        name: *const c_void,
        object: *const c_void,
        behavior: isize,
    );
    fn CFNotificationCenterRemoveObserver(
        center: *mut c_void,
        observer: *const c_void,
        name: *const c_void,
        object: *const c_void,
    );
}

/// A retained `TISInputSourceRef` with its layout data and id.
struct Source {
    source: *mut c_void,
    /// `UCKeyboardLayout*` inside the source's `CFData` (owned by `source`);
    /// null when the source has no Unicode layout data.
    layout: *const c_void,
    id: Option<String>,
}

impl Source {
    /// Takes ownership of a `TISCopy…` result (may be null).
    fn adopt(source: *mut c_void) -> Option<Source> {
        if source.is_null() {
            return None;
        }
        // SAFETY: `source` is a live TISInputSourceRef we own; the returned
        // properties follow the Get rule (owned by `source`).
        unsafe {
            let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
            let layout = if data.is_null() { std::ptr::null() } else { CFDataGetBytePtr(data).cast() };
            let id = cf_string(TISGetInputSourceProperty(source, kTISPropertyInputSourceID));
            Some(Source { source, layout, id })
        }
    }

    fn translate(&self, keycode: u16, mods: TranslateMods) -> Option<String> {
        if self.layout.is_null() {
            return None;
        }
        let modifiers = match mods {
            TranslateMods::None => 0,
            TranslateMods::Command => UC_CMD,
        };
        let mut dead = 0u32;
        let mut buf = [0u16; 8];
        let mut len = 0usize;
        // SAFETY: `layout` points into the CFData kept alive by `source`;
        // the buffers are local and sized as passed.
        let status = unsafe {
            UCKeyTranslate(
                self.layout,
                keycode,
                KEY_ACTION_DOWN,
                modifiers,
                u32::from(LMGetKbdType()),
                NO_DEAD_KEYS,
                &mut dead,
                buf.len(),
                &mut len,
                buf.as_mut_ptr(),
            )
        };
        (status == 0).then(|| String::from_utf16_lossy(&buf[..len.min(buf.len())]))
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: we own the +1 reference from `TISCopy…`.
        unsafe { CFRelease(self.source) };
    }
}

/// A `CFStringRef` as a Rust string (`None` for null / non-UTF-8).
///
/// # Safety
/// `s` must be null or a valid CFString.
unsafe fn cf_string(s: *const c_void) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: valid CFString per the contract; buffer sized for UTF-8.
    unsafe {
        let size = CFStringGetLength(s) * 4 + 1;
        let mut buf = vec![0u8; usize::try_from(size).ok()?];
        if !CFStringGetCString(s, buf.as_mut_ptr(), size, UTF8) {
            return None;
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        String::from_utf8(buf).ok()
    }
}

unsafe extern "C" fn source_changed(
    _center: *mut c_void,
    observer: *mut c_void,
    _name: *const c_void,
    _object: *const c_void,
    _info: *const c_void,
) {
    if !observer.is_null() {
        // SAFETY: `observer` is the boxed flag registered in `new`, removed
        // in `Drop` before the box is freed.
        unsafe { (*(observer as *const AtomicBool)).store(true, Ordering::Relaxed) };
    }
}

/// Characters on keys from the user's keyboard layouts, for one recording.
pub struct LayoutTranslator {
    current: Option<Source>,
    ascii: Option<Source>,
    cache: LayoutCache,
    /// Set by the input-source-changed notification.
    changed: Box<AtomicBool>,
}

impl std::fmt::Debug for LayoutTranslator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutTranslator")
            .field("current", &self.cache.source_id(LayoutKind::Current))
            .field("ascii", &self.cache.source_id(LayoutKind::AsciiCapable))
            .finish()
    }
}

impl LayoutTranslator {
    /// Reads the current layouts and starts watching for changes. Call on
    /// the main thread.
    pub fn new() -> LayoutTranslator {
        let changed = Box::new(AtomicBool::new(false));
        // SAFETY: the observer pointer stays valid until `Drop` removes it.
        unsafe {
            CFNotificationCenterAddObserver(
                CFNotificationCenterGetDistributedCenter(),
                (&*changed as *const AtomicBool).cast(),
                source_changed,
                kTISNotifySelectedKeyboardInputSourceChanged,
                std::ptr::null(),
                DELIVER_IMMEDIATELY,
            );
        }
        let mut t = LayoutTranslator { current: None, ascii: None, cache: LayoutCache::new(), changed };
        t.refresh();
        t
    }

    /// Re-read the selected and ASCII-capable layouts.
    fn refresh(&mut self) {
        // SAFETY: `TISCopy…` return +1 references (or null), adopted here.
        self.current = Source::adopt(unsafe { TISCopyCurrentKeyboardLayoutInputSource() });
        self.ascii = Source::adopt(unsafe { TISCopyCurrentASCIICapableKeyboardLayoutInputSource() });
        let id = |s: &Option<Source>| s.as_ref().and_then(|s| s.id.clone());
        self.cache.set_sources(id(&self.current), id(&self.ascii));
    }

    /// Input source ids in use: (selected, ASCII-capable), for the log.
    pub fn source_ids(&self) -> (Option<&str>, Option<&str>) {
        (self.cache.source_id(LayoutKind::Current), self.cache.source_id(LayoutKind::AsciiCapable))
    }
}

impl Default for LayoutTranslator {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyTranslator for LayoutTranslator {
    fn translate(&mut self, keycode: u16, how: Translation) -> Option<String> {
        if self.changed.swap(false, Ordering::Relaxed) {
            self.refresh();
        }
        let source = match how.layout {
            LayoutKind::Current => self.current.as_ref(),
            LayoutKind::AsciiCapable => self.ascii.as_ref().or(self.current.as_ref()),
        }?;
        self.cache.get_or_translate(keycode, how, || source.translate(keycode, how.mods))
    }
}

impl Drop for LayoutTranslator {
    fn drop(&mut self) {
        // SAFETY: removes the observer registered in `new` before the flag
        // it points to is freed.
        unsafe {
            CFNotificationCenterRemoveObserver(
                CFNotificationCenterGetDistributedCenter(),
                (&*self.changed as *const AtomicBool).cast(),
                kTISNotifySelectedKeyboardInputSourceChanged,
                std::ptr::null(),
            );
        }
    }
}

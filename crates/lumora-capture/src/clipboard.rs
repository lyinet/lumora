//! Pasteboard helpers. Call from the main thread.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::AnyThread;
use objc2_app_kit::{
    NSImage, NSPasteboard, NSPasteboardItem, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardWriting,
};
use objc2_core_foundation::CGSize;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

/// Put an image on the general pasteboard. When the capture was also saved,
/// the file URL is written alongside so pasting into Finder/Slack/Mail gets
/// the real PNG file, while image editors get the bitmap.
///
/// `scale` sets the `NSImage` point size so Retina captures paste at the
/// right size.
pub fn copy_image(image: &CGImage, scale: f64, file: Option<&Path>) {
    let (w, h) = crate::image::pixel_size(image);
    let size = CGSize::new(w as f64 / scale.max(1.0), h as f64 / scale.max(1.0));
    let ns_image = NSImage::initWithCGImage_size(NSImage::alloc(), image, size);
    let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        vec![ProtocolObject::from_retained(ns_image)];
    if let Some(url) = file.and_then(file_url) {
        items.push(ProtocolObject::from_retained(url));
    }
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.writeObjects(&NSArray::from_retained_slice(&items));
}

/// Put already-encoded PNG bytes on the general pasteboard as
/// `NSPasteboardTypePNG` (plus the file URL when the capture is saved).
///
/// Prefer this over [`copy_image`] whenever the PNG exists anyway: an
/// `NSImage` is serialised to uncompressed TIFF on the main thread (tens of
/// MB for a 5K capture), while PNG data is a few MB and costs nothing more
/// here. Returns the pasteboard's change count after the write, so a later
/// fix-up can tell whether the user copied something else meanwhile.
pub fn copy_png(png: &[u8], file: Option<&Path>) -> isize {
    let item = NSPasteboardItem::new();
    let data = NSData::with_bytes(png);
    // SAFETY: NSPasteboardTypePNG is a valid static.
    unsafe { item.setData_forType(&data, NSPasteboardTypePNG) };
    let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = vec![ProtocolObject::from_retained(item)];
    if let Some(url) = file.and_then(file_url) {
        items.push(ProtocolObject::from_retained(url));
    }
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.writeObjects(&NSArray::from_retained_slice(&items));
    pb.changeCount()
}

/// The general pasteboard's change count (bumps on every write by anyone).
pub fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

/// Put a file (e.g. a finished recording) on the pasteboard.
pub fn copy_file(path: &Path) {
    let Some(url) = file_url(path) else { return };
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = vec![ProtocolObject::from_retained(url)];
    pb.writeObjects(&NSArray::from_retained_slice(&items));
}

/// Put plain text (e.g. a hex colour) on the pasteboard.
pub fn copy_text(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: NSPasteboardTypeString is a valid static.
    unsafe { pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
}

/// `file://` URL for a path.
pub fn file_url(path: &Path) -> Option<Retained<NSURL>> {
    let s = path.to_str()?;
    Some(NSURL::fileURLWithPath(&NSString::from_str(s)))
}

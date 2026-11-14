//! Lumora — premium screenshots and screen recordings for macOS.
//!
//! This binary wires the pieces together: an `NSApplication` with the
//! Accessory activation policy (menu bar only, no Dock icon), a status item,
//! global hotkeys, and a central [`app::App`] that owns all state and
//! dispatches [`command::Command`]s.

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod capture_flow;
mod command;
#[cfg(target_os = "macos")]
mod delegate;
#[cfg(target_os = "macos")]
mod editor_flow;
#[cfg(target_os = "macos")]
mod hotkeys;
mod logger;
#[cfg(target_os = "macos")]
mod record_flow;
#[cfg(target_os = "macos")]
mod status;
#[cfg(target_os = "macos")]
mod system;

#[cfg(target_os = "macos")]
fn main() {
    use objc2::runtime::ProtocolObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    logger::init();
    let mtm = MainThreadMarker::new().expect("Lumora must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Menu-bar app: no Dock icon, no main menu. (Info.plist also sets
    // LSUIElement so this holds even before this line runs.)
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate = delegate::AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    logger::init();
    eprintln!("Lumora is a macOS app. Build with --target aarch64-apple-darwin or x86_64-apple-darwin.");
    std::process::exit(1);
}

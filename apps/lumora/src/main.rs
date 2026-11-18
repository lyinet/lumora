//! Lumora — premium screenshots and screen recordings for macOS.
//!
//! This binary wires the pieces together: an `NSApplication` with the
//! Accessory activation policy (menu bar only, no Dock icon), a status item,
//! global hotkeys, and a central [`app::App`] that owns all state and
//! dispatches [`command::Command`]s.

#[cfg(target_os = "macos")]
mod ai_flow;
#[cfg(target_os = "macos")]
mod ai_write_flow;
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod capture_flow;
#[cfg(target_os = "macos")]
mod clip_flow;
mod command;
#[cfg(target_os = "macos")]
mod delegate;
#[cfg(target_os = "macos")]
mod doc_flow;
#[cfg(target_os = "macos")]
mod editor_flow;
#[cfg(target_os = "macos")]
mod history_flow;
#[cfg(target_os = "macos")]
mod hotkeys;
mod logger;
#[cfg(target_os = "macos")]
mod record_flow;
#[cfg(target_os = "macos")]
mod redact_flow;
#[cfg(target_os = "macos")]
mod status;
#[cfg(target_os = "macos")]
mod system;
#[cfg(target_os = "macos")]
mod text_flow;
#[cfg(target_os = "macos")]
mod transcribe_flow;
#[cfg(target_os = "macos")]
mod tutorial_flow;

#[cfg(target_os = "macos")]
fn main() {
    use objc2::runtime::ProtocolObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    logger::init();
    let mtm = MainThreadMarker::new().expect("Lumora must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Menu-bar app: no Dock icon. (Info.plist also sets LSUIElement so this
    // holds even before this line runs.)
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    install_main_menu(mtm, &app);
    let delegate = delegate::AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}

/// A main menu that is never shown (LSUIElement apps have no menu bar) but
/// that AppKit still consults for key equivalents. It gives every text field
/// outside the editor (Settings, Tutorial) the standard Edit shortcuts and
/// lets ⌘W / ⌘M close or minimise the key window. Every item has a nil target,
/// so it goes down the responder chain and is disabled where nothing answers.
///
/// Views see `performKeyEquivalent:` before the main menu, so the editor's own
/// ⌘-routing still wins inside the editor. The status item has its own menu
/// and is unaffected. Deliberately no ⌘Q: quitting from any focused window
/// would throw away an unsaved editor (Quit stays in the status menu).
#[cfg(target_os = "macos")]
fn install_main_menu(mtm: objc2::MainThreadMarker, app: &objc2_app_kit::NSApplication) {
    use objc2::rc::Retained;
    use objc2::runtime::Sel;
    use objc2::sel;
    use objc2::MainThreadOnly;
    use objc2_app_kit::{NSEventModifierFlags, NSMenu, NSMenuItem};
    use objc2_foundation::NSString;

    fn item(
        mtm: objc2::MainThreadMarker,
        title: &str,
        action: Option<Sel>,
        key: &str,
        mods: NSEventModifierFlags,
    ) -> Retained<NSMenuItem> {
        // SAFETY: standard responder-chain selectors (or none); nil target.
        let it = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(key),
            )
        };
        if !key.is_empty() {
            it.setKeyEquivalentModifierMask(mods);
        }
        it
    }
    fn submenu(mtm: objc2::MainThreadMarker, bar: &NSMenu, title: &str, items: &[Retained<NSMenuItem>]) {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        for i in items {
            menu.addItem(i);
        }
        let holder = item(mtm, title, None, "", NSEventModifierFlags::empty());
        holder.setSubmenu(Some(&menu));
        bar.addItem(&holder);
    }

    let cmd = NSEventModifierFlags::Command;
    let bar = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Lumora"));
    // The first submenu is always the application menu.
    submenu(mtm, &bar, "Lumora", &[item(mtm, "Quit Lumora", Some(sel!(terminate:)), "", cmd)]);
    submenu(
        mtm,
        &bar,
        "Edit",
        &[
            item(mtm, "Undo", Some(sel!(undo:)), "z", cmd),
            item(mtm, "Redo", Some(sel!(redo:)), "z", cmd | NSEventModifierFlags::Shift),
            NSMenuItem::separatorItem(mtm),
            item(mtm, "Cut", Some(sel!(cut:)), "x", cmd),
            item(mtm, "Copy", Some(sel!(copy:)), "c", cmd),
            item(mtm, "Paste", Some(sel!(paste:)), "v", cmd),
            item(mtm, "Select All", Some(sel!(selectAll:)), "a", cmd),
        ],
    );
    submenu(
        mtm,
        &bar,
        "Window",
        &[
            item(mtm, "Close", Some(sel!(performClose:)), "w", cmd),
            item(mtm, "Minimize", Some(sel!(performMiniaturize:)), "m", cmd),
        ],
    );
    app.setMainMenu(Some(&bar));
}

#[cfg(not(target_os = "macos"))]
fn main() {
    logger::init();
    eprintln!("Lumora is a macOS app. Build with --target aarch64-apple-darwin or x86_64-apple-darwin.");
    std::process::exit(1);
}

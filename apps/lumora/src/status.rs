//! The menu-bar status item and its menu.

use lumora_core::hotkey::Hotkey;
use lumora_core::i18n::{tr, Msg};
use lumora_core::settings::Settings;
use lumora_core::Action;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSCellImagePosition, NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;

use lumora_ui::icon;
use lumora_ui::util::{mono_digits, weight};

pub struct StatusBar {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
    action_items: Vec<(Action, Retained<NSMenuItem>)>,
    record_item: Retained<NSMenuItem>,
    /// Items re-titled when the language changes.
    fixed_items: Vec<(Msg, Retained<NSMenuItem>)>,
    recording: std::cell::Cell<bool>,
    idle_icon: Retained<NSImage>,
    rec_icon: Retained<NSImage>,
}

fn menu_item(mtm: MainThreadMarker, title: &str, action: Sel, target: &AnyObject) -> Retained<NSMenuItem> {
    // SAFETY: valid selector implemented by the target (the app delegate).
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(""),
        )
    };
    // SAFETY: the delegate lives for the whole app lifetime.
    unsafe { item.setTarget(Some(target)) };
    item
}

/// Show a shortcut next to a menu item (display only — the global hotkey
/// does the real work).
fn show_shortcut(item: &NSMenuItem, hk: Option<Hotkey>) {
    let Some(hk) = hk else {
        item.setKeyEquivalent(&NSString::from_str(""));
        return;
    };
    let label = hk.key.label;
    let key = if label.chars().count() == 1 { label.to_lowercase() } else { String::new() };
    item.setKeyEquivalent(&NSString::from_str(&key));
    let mut mask = NSEventModifierFlags::empty();
    let m = hk.modifiers;
    use lumora_core::hotkey::Modifiers;
    if m.contains(Modifiers::CONTROL) {
        mask |= NSEventModifierFlags::Control;
    }
    if m.contains(Modifiers::OPTION) {
        mask |= NSEventModifierFlags::Option;
    }
    if m.contains(Modifiers::SHIFT) {
        mask |= NSEventModifierFlags::Shift;
    }
    if m.contains(Modifiers::COMMAND) {
        mask |= NSEventModifierFlags::Command;
    }
    item.setKeyEquivalentModifierMask(mask);
}

impl StatusBar {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject, settings: &Settings) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let idle_icon = icon::status_icon();
        let rec_icon = icon::recording_dot();
        if let Some(button) = item.button(mtm) {
            button.setImage(Some(&idle_icon));
            button.setToolTip(Some(&NSString::from_str(tr(Msg::AppName))));
        }

        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Lumora"));
        menu.setAutoenablesItems(false);
        let entries: [(Action, Sel); 5] = [
            (Action::CaptureArea, sel!(captureArea:)),
            (Action::CaptureFullscreen, sel!(captureFullscreen:)),
            (Action::CaptureWindow, sel!(captureWindow:)),
            (Action::CaptureText, sel!(captureText:)),
            (Action::RecordScreen, sel!(toggleRecording:)),
        ];
        let mut action_items = Vec::new();
        let mut record_item = None;
        for (action, s) in entries {
            let mi = menu_item(mtm, action.title(), s, target);
            menu.addItem(&mi);
            if action == Action::RecordScreen {
                record_item = Some(mi.clone());
            }
            action_items.push((action, mi));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let open = menu_item(mtm, Action::OpenFolder.title(), sel!(openFolder:), target);
        menu.addItem(&open);
        action_items.push((Action::OpenFolder, open));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let settings_item = menu_item(mtm, tr(Msg::SettingsMenu), sel!(showSettings:), target);
        settings_item.setKeyEquivalent(&NSString::from_str(","));
        menu.addItem(&settings_item);
        let permission_item = menu_item(mtm, tr(Msg::PermissionMenu), sel!(showOnboarding:), target);
        menu.addItem(&permission_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let quit = menu_item(mtm, tr(Msg::QuitLumora), sel!(quit:), target);
        quit.setKeyEquivalent(&NSString::from_str("q"));
        menu.addItem(&quit);
        item.setMenu(Some(&menu));

        let bar = StatusBar {
            mtm,
            item,
            _menu: menu,
            action_items,
            record_item: record_item.expect("record item exists"),
            fixed_items: vec![
                (Msg::SettingsMenu, settings_item),
                (Msg::PermissionMenu, permission_item),
                (Msg::QuitLumora, quit),
            ],
            recording: std::cell::Cell::new(false),
            idle_icon,
            rec_icon,
        };
        bar.update_shortcuts(settings);
        bar
    }

    /// Re-title every item after a language change.
    pub fn relabel(&self) {
        for (action, item) in &self.action_items {
            item.setTitle(&NSString::from_str(action.title()));
        }
        for (msg, item) in &self.fixed_items {
            item.setTitle(&NSString::from_str(tr(*msg)));
        }
        let rec = if self.recording.get() { tr(Msg::StopRecording) } else { tr(Msg::RecordScreen) };
        self.record_item.setTitle(&NSString::from_str(rec));
    }

    pub fn update_shortcuts(&self, settings: &Settings) {
        for (action, item) in &self.action_items {
            show_shortcut(item, settings.shortcuts.get(*action));
        }
    }

    /// `Some(elapsed)` switches to the red-dot + timer look; `None` restores
    /// the template icon.
    pub fn set_recording(&self, elapsed: Option<f64>) {
        let Some(button) = self.item.button(self.mtm) else { return };
        match elapsed {
            Some(secs) => {
                button.setImage(Some(&self.rec_icon));
                button.setImagePosition(NSCellImagePosition::ImageLeft);
                button.setFont(Some(&mono_digits(12.0, weight::MEDIUM)));
                button.setTitle(&NSString::from_str(&format!(" {}", lumora_core::format::elapsed(secs))));
                self.record_item.setTitle(&NSString::from_str(tr(Msg::StopRecording)));
                self.recording.set(true);
            }
            None => {
                button.setImage(Some(&self.idle_icon));
                button.setTitle(&NSString::from_str(""));
                self.record_item.setTitle(&NSString::from_str(tr(Msg::RecordScreen)));
                self.recording.set(false);
            }
        }
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

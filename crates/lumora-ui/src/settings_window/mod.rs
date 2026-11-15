//! The Settings window: a native `NSWindow` with General, Shortcuts,
//! Capture, Recording and AI tabs. Every change is applied immediately (no Save
//! button, per macOS convention) and emitted as
//! [`UiEvent::SettingsChanged`]; the app persists and re-applies it.

mod ai;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use lumora_core::action::Action;
use lumora_core::geometry::Rect;
use lumora_core::hotkey::{find_conflicts, Conflict, Hotkey, Key, Modifiers};
use lumora_core::i18n::{tr, trf, LanguagePref, Msg};
use lumora_core::settings::{Codec, Settings};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn, NSEvent,
    NSEventModifierFlags, NSFont, NSOpenPanel, NSPopUpButton, NSResponder, NSTabView, NSTabViewDelegate, NSTabViewItem,
    NSTextAlignment, NSTextField, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSObject, NSString};
use objc2_quartz_core::{CALayer, CATextLayer};

use crate::events::{EventSink, UiEvent};
use crate::tokens::{color, radius, typography};
use crate::util::{cg_color, cg_rect, mono_digits, set_text, text_layer, weight, ActionTarget};

const W: f64 = 540.0;
const H: f64 = 580.0;
const TAB_H: f64 = 500.0;
const LABEL_X: f64 = 24.0;
const LABEL_W: f64 = 170.0;
const CONTROL_X: f64 = 206.0;
const ROW_H: f64 = 30.0;

// ---------------------------------------------------------------------------
// Shortcut recorder control
// ---------------------------------------------------------------------------

pub struct ShortcutFieldIvars {
    current: Cell<Option<Hotkey>>,
    recording: Cell<bool>,
    /// Another app holds this shortcut (registration failed).
    invalid: Cell<bool>,
    label: RefCell<Option<Retained<CATextLayer>>>,
    on_change: Box<dyn Fn(Option<Hotkey>)>,
    on_recording: Box<dyn Fn(bool)>,
}

define_class!(
    /// Click, then press a key combination. Esc cancels, ⌫ clears.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraShortcutField"]
    #[ivars = ShortcutFieldIvars]
    pub struct ShortcutField;

    unsafe impl NSObjectProtocol for ShortcutField {}

    impl ShortcutField {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            if let Some(w) = self.window() {
                w.makeFirstResponder(Some(self));
            }
            self.set_recording(!self.ivars().recording.get());
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.set_recording(false);
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if self.ivars().recording.get() {
                self.record(event);
            }
        }

        /// ⌘-combinations arrive here before keyDown.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            if self.ivars().recording.get() {
                self.record(event);
                true
            } else {
                false
            }
        }
    }
);

impl ShortcutField {
    fn new(
        mtm: MainThreadMarker,
        frame: CGRect,
        current: Option<Hotkey>,
        on_change: impl Fn(Option<Hotkey>) + 'static,
        on_recording: impl Fn(bool) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ShortcutFieldIvars {
            current: Cell::new(current),
            recording: Cell::new(false),
            invalid: Cell::new(false),
            label: RefCell::new(None),
            on_change: Box::new(on_change),
            on_recording: Box::new(on_recording),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let root = CALayer::new();
        root.setCornerRadius(radius::BUTTON);
        root.setBorderWidth(1.0);
        root.setBackgroundColor(Some(&cg_color(color::PILL_BG.with_alpha(0.08))));
        let font = mono_digits(typography::BODY, weight::MEDIUM);
        let label = text_layer(&font, typography::BODY, crate::tokens::Rgba::new(0.5, 0.5, 0.55, 1.0), 2.0);
        label.setFrame(cg_rect(Rect::new(0.0, (frame.size.height - 17.0) / 2.0, frame.size.width, 17.0)));
        root.addSublayer(&label);
        this.setLayer(Some(&root));
        this.setWantsLayer(true);
        *this.ivars().label.borrow_mut() = Some(label);
        this.refresh();
        this
    }

    fn set_recording(&self, on: bool) {
        if self.ivars().recording.replace(on) != on {
            (self.ivars().on_recording)(on);
        }
        self.refresh();
    }

    fn refresh(&self) {
        let recording = self.ivars().recording.get();
        let text = if recording {
            tr(Msg::TypeShortcut).to_string()
        } else {
            self.ivars().current.get().map(|h| h.to_string()).unwrap_or_else(|| tr(Msg::ClickToRecord).into())
        };
        if let Some(l) = self.ivars().label.borrow().as_ref() {
            set_text(l, &text);
        }
        if let Some(root) = self.layer() {
            let border = if recording {
                color::ACCENT
            } else if self.ivars().invalid.get() {
                color::ERROR
            } else {
                color::HAIRLINE.with_alpha(0.25)
            };
            root.setBorderColor(Some(&cg_color(border)));
        }
    }

    fn record(&self, event: &NSEvent) {
        let code = event.keyCode();
        match code {
            53 => return self.set_recording(false), // Esc: cancel
            51 | 117 => {
                // Delete / Forward delete: clear the binding.
                self.ivars().current.set(None);
                (self.ivars().on_change)(None);
                return self.set_recording(false);
            }
            _ => {}
        }
        let flags = event.modifierFlags();
        let mut mods = Modifiers::NONE;
        for (flag, m) in [
            (NSEventModifierFlags::Control, Modifiers::CONTROL),
            (NSEventModifierFlags::Option, Modifiers::OPTION),
            (NSEventModifierFlags::Shift, Modifiers::SHIFT),
            (NSEventModifierFlags::Command, Modifiers::COMMAND),
        ] {
            if flags.contains(flag) {
                mods = mods | m;
            }
        }
        let Some(key) = Key::from_code(code) else { return };
        let needs_modifier = !(mods.contains(Modifiers::CONTROL)
            || mods.contains(Modifiers::OPTION)
            || mods.contains(Modifiers::COMMAND));
        if needs_modifier {
            if let Some(l) = self.ivars().label.borrow().as_ref() {
                set_text(l, tr(Msg::AddModifier));
            }
            return;
        }
        let hk = Hotkey { modifiers: mods, key };
        self.ivars().current.set(Some(hk));
        (self.ivars().on_change)(Some(hk));
        self.set_recording(false);
    }

    fn set_value(&self, h: Option<Hotkey>) {
        self.ivars().current.set(h);
        self.refresh();
    }

    fn set_invalid(&self, invalid: bool) {
        self.ivars().invalid.set(invalid);
        self.refresh();
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

type Refresh = Rc<dyn Fn(&Settings)>;

struct State {
    settings: RefCell<Settings>,
    sink: EventSink,
    home: PathBuf,
    /// Shortcuts the system refused to register (held by another app).
    taken: RefCell<Vec<Action>>,
    /// Re-evaluates the Shortcuts tab's warning line.
    refresh_warning: RefCell<Option<Refresh>>,
    /// The AI tab's controls, for the app's Keychain/test replies.
    ai: RefCell<Option<Rc<ai::AiControls>>>,
}

impl State {
    fn update(&self, f: impl FnOnce(&mut Settings)) {
        let snapshot = {
            let mut s = self.settings.borrow_mut();
            f(&mut s);
            s.validate();
            s.clone()
        };
        (self.sink)(UiEvent::SettingsChanged(Box::new(snapshot)));
    }
}

/// Simple top-down row layout inside a tab.
struct Form<'a> {
    mtm: MainThreadMarker,
    view: &'a NSView,
    y: f64,
}

impl Form<'_> {
    fn label(&self, text: &str) {
        let l = NSTextField::labelWithString(&NSString::from_str(text), self.mtm);
        l.setAlignment(NSTextAlignment::Right);
        l.setFrame(cg_rect(Rect::new(LABEL_X, self.y + 4.0, LABEL_W, 20.0)));
        self.view.addSubview(&l);
    }

    fn add(&mut self, label: &str, control: &NSView, height: f64) {
        self.y -= height.max(ROW_H);
        if !label.is_empty() {
            self.label(label);
        }
        let mut f = control.frame();
        f.origin = CGPoint::new(CONTROL_X, self.y + (ROW_H - f.size.height) / 2.0);
        control.setFrame(f);
        self.view.addSubview(control);
    }

    /// A right-aligned label on its own row (heading for an inline note).
    fn label_only(&mut self, text: &str) {
        self.y -= ROW_H - 10.0;
        self.label(text);
    }

    /// A secondary note on the current row, in the control column.
    fn note_inline(&mut self, text: &str) {
        let l = NSTextField::labelWithString(&NSString::from_str(text), self.mtm);
        l.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::REGULAR)));
        l.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        l.setFrame(cg_rect(Rect::new(CONTROL_X, self.y + 6.0, W - CONTROL_X - 24.0, 16.0)));
        self.view.addSubview(&l);
    }

    fn note(&mut self, text: &str) {
        self.y -= 20.0;
        let l = NSTextField::labelWithString(&NSString::from_str(text), self.mtm);
        l.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::REGULAR)));
        l.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        l.setFrame(cg_rect(Rect::new(CONTROL_X, self.y + 6.0, W - CONTROL_X - 24.0, 16.0)));
        self.view.addSubview(&l);
    }
}

/// The Settings window controller. Keep it alive while the window is open.
pub struct SettingsWindow {
    window: Retained<NSWindow>,
    tabs: Retained<NSTabView>,
    /// Weakly referenced by the window and tab view; owned here.
    _delegate: Retained<SettingsDelegate>,
    state: Rc<State>,
    _targets: Vec<Retained<ActionTarget>>,
    _fields: Vec<Retained<ShortcutField>>,
}

impl std::fmt::Debug for SettingsWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SettingsWindow")
    }
}

impl SettingsWindow {
    pub fn show(mtm: MainThreadMarker, settings: Settings, home: PathBuf, sink: EventSink) -> Self {
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(W, H));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(tr(Msg::SettingsTitle)));

        let state = Rc::new(State {
            settings: RefCell::new(settings),
            sink,
            home,
            taken: RefCell::new(Vec::new()),
            refresh_warning: RefCell::new(None),
            ai: RefCell::new(None),
        });
        let mut targets = Vec::new();
        let mut fields = Vec::new();

        let tabs = NSTabView::initWithFrame(NSTabView::alloc(mtm), cg_rect(Rect::new(12.0, 12.0, W - 24.0, H - 24.0)));
        let tab_frame = cg_rect(Rect::new(0.0, 0.0, W - 48.0, TAB_H));
        for (title, build) in [
            (tr(Msg::TabGeneral), build_general as TabBuilder),
            (tr(Msg::TabShortcuts), build_shortcuts as TabBuilder),
            (tr(Msg::TabCapture), build_capture as TabBuilder),
            (tr(Msg::TabRecording), build_recording as TabBuilder),
            (tr(Msg::TabAi), build_ai as TabBuilder),
        ] {
            let view = NSView::initWithFrame(NSView::alloc(mtm), tab_frame);
            let mut form = Form { mtm, view: &view, y: TAB_H - 16.0 };
            build(&mut form, &state, &mut targets, &mut fields);
            let item = unsafe { NSTabViewItem::initWithIdentifier(NSTabViewItem::alloc(), None) };
            item.setLabel(&NSString::from_str(title));
            item.setView(Some(&view));
            tabs.addTabViewItem(&item);
        }
        let delegate = SettingsDelegate::new(mtm, Rc::downgrade(&state));
        tabs.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.setContentView(Some(&tabs));
        window.center();
        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        SettingsWindow { window, tabs, _delegate: delegate, state, _targets: targets, _fields: fields }
    }

    /// Keep the window's settings snapshot in step with a beautify style
    /// changed in an editor, so the next edit here doesn't revert it.
    pub fn set_beautify(&self, style: lumora_core::beautify::BeautifyStyle) {
        self.state.settings.borrow_mut().beautify = style;
    }

    /// Mark shortcuts another app already holds: red field outline plus an
    /// explanation under the list.
    pub fn set_unavailable(&self, taken: &[Action]) {
        *self.state.taken.borrow_mut() = taken.to_vec();
        for (field, action) in self._fields.iter().zip(Action::ALL) {
            field.set_invalid(taken.contains(&action));
        }
        let refresh = self.state.refresh_warning.borrow().clone();
        if let Some(r) = refresh {
            r(&self.state.settings.borrow());
        }
    }

    /// The app's answer to [`UiEvent::AiKeyRequested`]. Never log `key`.
    pub fn set_ai_key(
        &self,
        provider: lumora_core::ai::AiProvider,
        key: Result<Option<String>, lumora_core::ai::AiError>,
    ) {
        if let Some(c) = self.state.ai.borrow().as_ref() {
            c.set_key(provider, key);
        }
    }

    /// Saving the key ([`UiEvent::AiKeySave`]) failed.
    pub fn set_ai_key_error(&self, provider: lumora_core::ai::AiProvider, error: &lumora_core::ai::AiError) {
        if let Some(c) = self.state.ai.borrow().as_ref() {
            c.set_key_error(provider, error);
        }
    }

    /// Result of [`UiEvent::AiTestConnection`] number `id`: the
    /// `ConnectionInfo::model_listed` value, or the error.
    pub fn set_ai_test_result(&self, id: u64, result: Result<Option<bool>, lumora_core::ai::AiError>) {
        if let Some(c) = self.state.ai.borrow().as_ref() {
            c.set_test_result(id, result);
        }
    }

    /// Bring the existing window forward.
    /// Bring the AI tab to the front.
    pub fn select_ai_tab(&self) {
        let n = self.tabs.numberOfTabViewItems();
        if n > 0 {
            self.tabs.selectTabViewItemAtIndex(n - 1);
        }
    }

    pub fn focus(&self, mtm: MainThreadMarker) {
        NSApplication::sharedApplication(mtm).activate();
        self.window.makeKeyAndOrderFront(None);
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    /// Window origin (AppKit global points), to reopen in the same place.
    pub fn origin(&self) -> (f64, f64) {
        let f = self.window.frame();
        (f.origin.x, f.origin.y)
    }

    pub fn set_origin(&self, origin: (f64, f64)) {
        self.window.setFrameOrigin(CGPoint::new(origin.0, origin.1));
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        self.window.close();
        self.window.setDelegate(None);
        self.tabs.setDelegate(None);
    }
}

// ---------------------------------------------------------------------------
// Window / tab delegate
// ---------------------------------------------------------------------------

pub struct SettingsDelegateIvars {
    state: std::rc::Weak<State>,
}

define_class!(
    /// Loads the API key when the AI tab is first shown and commits a
    /// pending key edit when the window closes.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraSettingsDelegate"]
    #[ivars = SettingsDelegateIvars]
    pub struct SettingsDelegate;

    unsafe impl NSObjectProtocol for SettingsDelegate {}

    unsafe impl NSTabViewDelegate for SettingsDelegate {
        #[unsafe(method(tabView:didSelectTabViewItem:))]
        fn did_select(&self, tab_view: &NSTabView, item: Option<&NSTabViewItem>) {
            let Some(item) = item else { return };
            if tab_view.indexOfTabViewItem(item) != ai::AI_TAB_INDEX {
                return;
            }
            if let Some(st) = self.ivars().state.upgrade() {
                if let Some(c) = st.ai.borrow().as_ref() {
                    c.request_key(&st);
                }
            }
        }
    }

    unsafe impl NSWindowDelegate for SettingsDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(st) = self.ivars().state.upgrade() {
                if let Some(c) = st.ai.borrow().as_ref() {
                    c.commit_key(&st);
                }
            }
        }
    }
);

impl SettingsDelegate {
    fn new(mtm: MainThreadMarker, state: std::rc::Weak<State>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SettingsDelegateIvars { state });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

type TabBuilder = fn(&mut Form<'_>, &Rc<State>, &mut Vec<Retained<ActionTarget>>, &mut Vec<Retained<ShortcutField>>);

fn checkbox(
    form: &mut Form<'_>,
    targets: &mut Vec<Retained<ActionTarget>>,
    state: &Rc<State>,
    label: &str,
    title: &str,
    get: fn(&Settings) -> bool,
    set: fn(&mut Settings, bool),
) {
    let st = state.clone();
    let t = ActionTarget::new(form.mtm, move |sender: &AnyObject| {
        let on = sender.downcast_ref::<NSButton>().is_some_and(|b| b.state() == NSControlStateValueOn);
        st.update(|s| set(s, on));
    });
    // SAFETY: target kept alive by the window controller.
    let b = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(title),
            Some(&t),
            Some(ActionTarget::action()),
            form.mtm,
        )
    };
    b.setState(if get(&state.settings.borrow()) { NSControlStateValueOn } else { NSControlStateValueOff });
    b.sizeToFit();
    form.add(label, &b, ROW_H);
    targets.push(t);
}

fn popup(
    form: &mut Form<'_>,
    targets: &mut Vec<Retained<ActionTarget>>,
    state: &Rc<State>,
    label: &str,
    items: &[&str],
    get: fn(&Settings) -> usize,
    set: fn(&mut Settings, usize),
) {
    let p = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(form.mtm),
        cg_rect(Rect::new(0.0, 0.0, 160.0, 26.0)),
        false,
    );
    for i in items {
        p.addItemWithTitle(&NSString::from_str(i));
    }
    p.selectItemAtIndex(get(&state.settings.borrow()) as isize);
    let st = state.clone();
    let t = ActionTarget::new(form.mtm, move |sender: &AnyObject| {
        if let Some(p) = sender.downcast_ref::<NSPopUpButton>() {
            let i = p.indexOfSelectedItem().max(0) as usize;
            st.update(|s| set(s, i));
        }
    });
    // SAFETY: target kept alive by the window controller.
    unsafe {
        p.setTarget(Some(&t));
        p.setAction(Some(ActionTarget::action()));
    }
    form.add(label, &p, ROW_H);
    targets.push(t);
}

fn build_general(
    form: &mut Form<'_>,
    state: &Rc<State>,
    targets: &mut Vec<Retained<ActionTarget>>,
    _f: &mut Vec<Retained<ShortcutField>>,
) {
    let mtm = form.mtm;
    // Save folder: path label + Choose… button.
    let folder = state.settings.borrow().save_folder(&state.home);
    let path_label = NSTextField::labelWithString(&NSString::from_str(&folder.display().to_string()), mtm);
    path_label.setFrame(cg_rect(Rect::new(0.0, 0.0, 200.0, 20.0)));
    path_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
    form.add(tr(Msg::SaveCapturesTo), &path_label, ROW_H);
    let st = state.clone();
    let label_ref = path_label.clone();
    let choose = ActionTarget::new(mtm, move |_| {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setCanCreateDirectories(true);
        panel.setPrompt(Some(&NSString::from_str(tr(Msg::ChoosePrompt))));
        if panel.runModal() == objc2_app_kit::NSModalResponseOK {
            if let Some(path) = panel.URL().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())) {
                label_ref.setStringValue(&NSString::from_str(&path.display().to_string()));
                st.update(|s| s.general.save_folder = Some(path));
            }
        }
    });
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(tr(Msg::Choose)),
            Some(&choose),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.sizeToFit();
    let mut f = b.frame();
    f.origin = CGPoint::new(CONTROL_X + 210.0, form.y + 1.0);
    b.setFrame(f);
    form.view.addSubview(&b);
    targets.push(choose);

    // Filename template.
    let field =
        NSTextField::textFieldWithString(&NSString::from_str(&state.settings.borrow().general.filename_template), mtm);
    field.setFrame(cg_rect(Rect::new(0.0, 0.0, 280.0, 22.0)));
    let st = state.clone();
    let t = ActionTarget::new(mtm, move |sender: &AnyObject| {
        if let Some(tf) = sender.downcast_ref::<NSTextField>() {
            let v = tf.stringValue().to_string();
            st.update(|s| s.general.filename_template = v);
        }
    });
    unsafe {
        field.setTarget(Some(&t));
        field.setAction(Some(ActionTarget::action()));
    }
    form.add(tr(Msg::FileName), &field, ROW_H);
    targets.push(t);
    form.note(tr(Msg::FileNameTokens));

    checkbox(
        form,
        targets,
        state,
        tr(Msg::AfterCapture),
        tr(Msg::CopyToClipboard),
        |s| s.general.copy_to_clipboard,
        |s, v| s.general.copy_to_clipboard = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::SaveToDisk),
        |s| s.general.save_to_disk,
        |s, v| s.general.save_to_disk = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::SaveEditableDocument),
        |s| s.general.save_editable_document,
        |s, v| s.general.save_editable_document = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::ShowQuickAccess),
        |s| s.general.show_quick_access,
        |s, v| s.general.show_quick_access = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::OpenEditorAfterCapture),
        |s| s.general.open_editor_after_capture,
        |s, v| s.general.open_editor_after_capture = v,
    );
    popup(
        form,
        targets,
        state,
        tr(Msg::HideCardAfter),
        &[tr(Msg::Seconds3), tr(Msg::Seconds6), tr(Msg::Seconds10), tr(Msg::Seconds30)],
        |s| match s.general.quick_access_timeout as u32 {
            0..=4 => 0,
            5..=7 => 1,
            8..=15 => 2,
            _ => 3,
        },
        |s, i| s.general.quick_access_timeout = [3.0, 6.0, 10.0, 30.0][i.min(3)],
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::PlaySounds),
        |s| s.general.play_sounds,
        |s, v| s.general.play_sounds = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::LaunchAtLogin),
        |s| s.general.launch_at_login,
        |s, v| s.general.launch_at_login = v,
    );
    let labels: Vec<&str> = LanguagePref::ALL.iter().map(|l| l.label()).collect();
    popup(
        form,
        targets,
        state,
        tr(Msg::LanguageLabel),
        &labels,
        |s| LanguagePref::ALL.iter().position(|l| *l == s.general.language).unwrap_or(0),
        |s, i| s.general.language = LanguagePref::ALL[i.min(LanguagePref::ALL.len() - 1)],
    );
    form.note(tr(Msg::LanguageNote));
}

fn build_shortcuts(
    form: &mut Form<'_>,
    state: &Rc<State>,
    targets: &mut Vec<Retained<ActionTarget>>,
    fields: &mut Vec<Retained<ShortcutField>>,
) {
    let mtm = form.mtm;
    let warning = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    warning.setTextColor(Some(&objc2_app_kit::NSColor::systemRedColor()));
    warning.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::MEDIUM)));
    let refresh_warning = {
        let warning = warning.clone();
        let st = Rc::downgrade(state);
        move |s: &Settings| {
            let bound: Vec<Option<Hotkey>> = Action::ALL.iter().map(|a| s.shortcuts.get(*a)).collect();
            let conflict = find_conflicts(&bound).first().map(|c| match *c {
                Conflict::Duplicate(i, j) => {
                    trf(Msg::ConflictDuplicate, &[&Action::ALL[i].title(), &Action::ALL[j].title()])
                }
                Conflict::System(i) => trf(Msg::ConflictSystem, &[&Action::ALL[i].title()]),
            });
            // Then shortcuts the system refused at registration time.
            let taken = || {
                let st = st.upgrade()?;
                let taken = st.taken.borrow();
                let a = taken.iter().find(|a| s.shortcuts.get(**a).is_some())?;
                let hk = s.shortcuts.get(*a)?;
                Some(trf(Msg::ConflictTaken, &[&hk, &a.title()]))
            };
            let msg = conflict.or_else(taken).unwrap_or_default();
            warning.setStringValue(&NSString::from_str(&msg));
        }
    };
    let refresh_warning: Refresh = Rc::new(refresh_warning);
    *state.refresh_warning.borrow_mut() = Some(refresh_warning.clone());
    for action in Action::ALL {
        let st = state.clone();
        let rw = refresh_warning.clone();
        let st2 = state.clone();
        let field = ShortcutField::new(
            mtm,
            cg_rect(Rect::new(0.0, 0.0, 160.0, 24.0)),
            state.settings.borrow().shortcuts.get(action),
            move |h| {
                st.update(|s| s.shortcuts.set(action, h));
                rw(&st.settings.borrow());
            },
            move |recording| (st2.sink)(UiEvent::ShortcutRecording(recording)),
        );
        form.add(&format!("{}:", action.title()), &field, ROW_H);
        fields.push(field);
    }
    form.y -= 36.0;
    warning.setFrame(cg_rect(Rect::new(CONTROL_X, form.y + 2.0, W - CONTROL_X - 30.0, 32.0)));
    form.view.addSubview(&warning);
    refresh_warning(&state.settings.borrow());

    let st = state.clone();
    let fields_for_reset: Vec<Retained<ShortcutField>> = fields.clone();
    let rw = refresh_warning.clone();
    let reset = ActionTarget::new(mtm, move |_| {
        st.update(|s| s.shortcuts = Default::default());
        let s = st.settings.borrow();
        for (f, a) in fields_for_reset.iter().zip(Action::ALL) {
            f.set_value(s.shortcuts.get(a));
        }
        rw(&s);
    });
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(tr(Msg::RestoreDefaults)),
            Some(&reset),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.sizeToFit();
    form.add("", &b, ROW_H);
    targets.push(reset);
    form.note(tr(Msg::ShortcutsNote));
}

fn build_capture(
    form: &mut Form<'_>,
    state: &Rc<State>,
    targets: &mut Vec<Retained<ActionTarget>>,
    _f: &mut Vec<Retained<ShortcutField>>,
) {
    checkbox(
        form,
        targets,
        state,
        tr(Msg::AreaSelection),
        tr(Msg::ShowMagnifier),
        |s| s.capture.show_magnifier,
        |s, v| s.capture.show_magnifier = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::ShowCrosshair),
        |s| s.capture.show_crosshair,
        |s, v| s.capture.show_crosshair = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::HighlightWindows),
        |s| s.capture.window_picking,
        |s, v| s.capture.window_picking = v,
    );
    checkbox(
        form,
        targets,
        state,
        tr(Msg::WindowCaptureLabel),
        tr(Msg::IncludeShadow),
        |s| s.capture.window_shadow,
        |s, v| s.capture.window_shadow = v,
    );
    checkbox(
        form,
        targets,
        state,
        tr(Msg::ResolutionLabel),
        tr(Msg::SaveRetina),
        |s| s.capture.retina,
        |s, v| s.capture.retina = v,
    );
    form.note(tr(Msg::CaptureNote));
    form.y -= 8.0;
    form.label_only(tr(Msg::TextRecognitionLabel));
    form.note_inline(tr(Msg::TextRecognitionNote));
}

fn build_ai(
    form: &mut Form<'_>,
    state: &Rc<State>,
    targets: &mut Vec<Retained<ActionTarget>>,
    _f: &mut Vec<Retained<ShortcutField>>,
) {
    ai::build_ai(form, state, targets);
}

fn build_recording(
    form: &mut Form<'_>,
    state: &Rc<State>,
    targets: &mut Vec<Retained<ActionTarget>>,
    _f: &mut Vec<Retained<ShortcutField>>,
) {
    popup(
        form,
        targets,
        state,
        tr(Msg::FrameRate),
        &[tr(Msg::Fps30), tr(Msg::Fps60)],
        |s| usize::from(s.recording.fps >= 60),
        |s, i| s.recording.fps = if i == 1 { 60 } else { 30 },
    );
    popup(
        form,
        targets,
        state,
        tr(Msg::VideoFormat),
        &[tr(Msg::H264), tr(Msg::Hevc)],
        |s| usize::from(s.recording.codec == Codec::Hevc),
        |s, i| s.recording.codec = if i == 1 { Codec::Hevc } else { Codec::H264 },
    );
    popup(
        form,
        targets,
        state,
        tr(Msg::Countdown),
        &[tr(Msg::Off), tr(Msg::Seconds3), tr(Msg::Seconds5), tr(Msg::Seconds10)],
        |s| match s.recording.countdown {
            0 => 0,
            1..=3 => 1,
            4..=5 => 2,
            _ => 3,
        },
        |s, i| s.recording.countdown = [0, 3, 5, 10][i.min(3)],
    );
    checkbox(
        form,
        targets,
        state,
        tr(Msg::Include),
        tr(Msg::MousePointer),
        |s| s.recording.show_cursor,
        |s, v| s.recording.show_cursor = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::SystemAudio),
        |s| s.recording.system_audio,
        |s, v| s.recording.system_audio = v,
    );
    checkbox(
        form,
        targets,
        state,
        "",
        tr(Msg::Microphone),
        |s| s.recording.microphone,
        |s, v| s.recording.microphone = v,
    );
    form.note(tr(Msg::MicrophoneNote));
}

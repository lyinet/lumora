//! Central application state and the command/event dispatcher.
//!
//! Threading model:
//! * Everything in [`App`] lives on the main thread (`Rc`/`RefCell`, AppKit).
//! * ScreenCaptureKit and the recorder call back on their own queues; those
//!   callbacks only carry `Send` data and hop to the main queue with
//!   [`on_main`], which looks the app up through a main-thread-local.
//! * UI components emit [`UiEvent`]s into a queue that is drained on the
//!   *next* main-queue turn, so a component is never torn down from inside
//!   its own callback.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use dispatch2::DispatchQueue;
use lumora_capture::content::ShareableContent;
use lumora_capture::permission;
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Msg};
use lumora_core::recording::RecordingSession;
use lumora_core::settings::Settings;
use lumora_ui::countdown::Countdown;
use lumora_ui::editor::EditorWindow;
use lumora_ui::events::{CardAction, EventSink, FrozenDisplay, OnboardingAction, UiEvent};
use lumora_ui::onboarding::Onboarding;
use lumora_ui::overlay::OverlayController;
use lumora_ui::pin::PinWindow;
use lumora_ui::quick_access::{open_file, QuickAccess};
use lumora_ui::recording_hud::{RecordingBorder, StopControl};
use lumora_ui::settings_window::SettingsWindow;
use lumora_ui::toast::{ToastCenter, ToastKind};
use lumora_ui::util::ActionTarget;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;
use objc2_foundation::NSTimer;

use crate::command::Command;
use crate::delegate::AppDelegate;
use crate::hotkeys::HotkeyManager;
use crate::status::StatusBar;
use crate::system;

thread_local! {
    /// The running app, reachable from Carbon callbacks and main-queue hops.
    static APP: RefCell<Weak<App>> = const { RefCell::new(Weak::new()) };
}

/// Run `f` with the app on the main thread, if it is still alive.
pub fn with_app(f: impl FnOnce(&Rc<App>)) {
    if let Some(app) = APP.with(|a| a.borrow().upgrade()) {
        f(&app);
    }
}

/// Hop to the main queue and run `f` with the app.
pub fn on_main(f: impl FnOnce(&Rc<App>) + Send + 'static) {
    DispatchQueue::main().exec_async(move || with_app(f));
}

/// Dispatch a command asynchronously (from hotkeys, menu items, …).
pub fn post_command(cmd: Command) {
    on_main(move |app| app.dispatch(cmd));
}

/// A recording target resolved from the overlay.
#[derive(Debug, Clone, Copy)]
pub struct RecordTarget {
    pub display_id: u32,
    /// Quartz global points; `None` = whole display.
    pub area: Option<Rect>,
    /// What is recorded, in AppKit global points (for chrome placement).
    pub appkit_frame: Rect,
}

/// What the app is doing right now. Exactly one capture/record flow at a time.
pub enum Mode {
    Idle,
    /// Fetching shareable content / freezing displays.
    Preparing,
    Selecting {
        overlay: OverlayController,
        content: Arc<ShareableContent>,
        frozen: Rc<Vec<FrozenDisplay>>,
    },
    Countdown {
        _countdown: Countdown,
        target: RecordTarget,
        border: Option<RecordingBorder>,
    },
    Recording {
        recorder: Option<lumora_capture::recorder::Recorder>,
        target: RecordTarget,
        _border: Option<RecordingBorder>,
        stop: StopControl,
        timer: Retained<NSTimer>,
    },
    /// The writer is finalising the file.
    Finishing,
}

impl Mode {
    pub fn is_idle(&self) -> bool {
        matches!(self, Mode::Idle)
    }
}

/// A user-visible outcome for [`App::notify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Success(Msg),
    Warning(Msg),
    Error(Msg),
}

/// A capture shown in Quick Access.
pub struct CaptureRecord {
    pub image: Option<CFRetained<CGImage>>,
    pub scale: f64,
    /// Quartz global points.
    pub frame: Rect,
    pub file: PathBuf,
    /// Whether `file` lives in the temp dir (Save moves it).
    pub temp: bool,
    pub video: bool,
    /// What re-editing starts from (screenshots only).
    pub editable: Option<Editable>,
}

/// The non-destructive state behind a screenshot: the original capture
/// plus the annotations last saved on it. Re-editing starts from here, and
/// the `.lumora` document next to the file is written from it.
pub struct Editable {
    /// The original capture (un-annotated, un-beautified).
    pub original: CFRetained<CGImage>,
    /// Pixels per point of `original`.
    pub scale: f64,
    /// `original` PNG-encoded, once known (kept byte-exact for documents
    /// opened from disk).
    pub png: Option<Arc<Vec<u8>>>,
    /// Annotations last saved; `None` = none drawn yet.
    pub document: Option<lumora_core::annotate::Document>,
    /// Beautify style last saved with it; `None` = follow Settings.
    pub beautify: Option<lumora_core::beautify::BeautifyStyle>,
    /// Unix seconds when the capture was taken.
    pub created_at: u64,
    /// Opened from a `.lumora` file: keep that file updated regardless of
    /// the "save editable document" setting.
    pub from_file: bool,
}

impl Editable {
    pub fn new(original: CFRetained<CGImage>, scale: f64) -> Self {
        Editable {
            original,
            scale,
            png: None,
            document: None,
            beautify: None,
            created_at: crate::system::unix_now(),
            from_file: false,
        }
    }
}

pub struct App {
    pub mtm: MainThreadMarker,
    pub settings: RefCell<Settings>,
    settings_path: Option<PathBuf>,
    pub home: PathBuf,
    pub status: StatusBar,
    hotkeys: RefCell<HotkeyManager>,
    pub sink: EventSink,
    events: RefCell<VecDeque<UiEvent>>,
    drain_scheduled: Cell<bool>,
    pub mode: RefCell<Mode>,
    pub quick: QuickAccess,
    pub captures: RefCell<HashMap<u64, CaptureRecord>>,
    pins: RefCell<HashMap<u64, PinWindow>>,
    pub editors: RefCell<HashMap<u64, EditorWindow>>,
    toasts: ToastCenter,
    /// Actions whose shortcut another app already holds.
    hotkey_failures: RefCell<Vec<lumora_core::Action>>,
    settings_window: RefCell<Option<SettingsWindow>>,
    onboarding: RefCell<Option<Onboarding>>,
    onboarding_target: RefCell<Option<Retained<ActionTarget>>>,
    permission_poll: RefCell<Option<Retained<NSTimer>>>,
    next_id: Cell<u64>,
    pub session: RefCell<RecordingSession>,
}

impl App {
    pub fn start(mtm: MainThreadMarker, delegate: &AppDelegate) -> Rc<App> {
        let home = system::home_dir();
        let settings_path = Settings::default_path();
        let settings = settings_path.as_deref().map(Settings::load_or_default).unwrap_or_default();
        let lang = system::apply_language(settings.general.language);
        log::info!(
            "starting Lumora {} (settings: {:?}, language: {})",
            env!("CARGO_PKG_VERSION"),
            settings_path,
            lang.tag()
        );

        let app = Rc::new_cyclic(|weak: &Weak<App>| {
            let w = weak.clone();
            let sink: EventSink = Rc::new(move |event| {
                if let Some(app) = w.upgrade() {
                    app.enqueue(event);
                }
            });
            App {
                mtm,
                status: StatusBar::new(mtm, delegate, &settings),
                quick: QuickAccess::new(mtm, sink.clone(), settings.general.quick_access_timeout),
                settings: RefCell::new(settings),
                settings_path,
                home,
                hotkeys: RefCell::new(HotkeyManager::new()),
                sink,
                events: RefCell::new(VecDeque::new()),
                drain_scheduled: Cell::new(false),
                mode: RefCell::new(Mode::Idle),
                captures: RefCell::new(HashMap::new()),
                pins: RefCell::new(HashMap::new()),
                editors: RefCell::new(HashMap::new()),
                toasts: ToastCenter::new(mtm),
                hotkey_failures: RefCell::new(Vec::new()),
                settings_window: RefCell::new(None),
                onboarding: RefCell::new(None),
                onboarding_target: RefCell::new(None),
                permission_poll: RefCell::new(None),
                next_id: Cell::new(1),
                session: RefCell::new(RecordingSession::default()),
            }
        });
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(&app));
        app.apply_hotkeys();

        if !permission::has_screen_recording() {
            app.dispatch(Command::ShowOnboarding);
        }
        app
    }

    /// Tell the user how something went, with a toast HUD.
    pub fn notify(&self, notice: Notice, detail: Option<String>) {
        let (kind, msg) = match notice {
            Notice::Success(m) => (ToastKind::Success, m),
            Notice::Warning(m) => (ToastKind::Warning, m),
            Notice::Error(m) => (ToastKind::Error, m),
        };
        log::info!("toast: {}{}", tr(msg), detail.as_deref().map(|d| format!(" — {d}")).unwrap_or_default());
        self.toasts.show(kind, tr(msg), detail.as_deref());
    }

    pub fn shutdown(&self) {
        // Finalise an in-flight recording synchronously is not possible
        // here; dropping the recorder cancels it cleanly.
        self.mode.replace(Mode::Idle);
        self.quick.dismiss_all();
        self.editors.borrow_mut().clear();
    }

    pub fn next_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// Replace the mode, dropping the old one *after* the borrow ends (its
    /// destructors may close windows that re-enter the app).
    pub fn set_mode(&self, mode: Mode) {
        let old = self.mode.replace(mode);
        drop(old);
    }

    // -- commands -----------------------------------------------------------

    pub fn dispatch(self: &Rc<Self>, cmd: Command) {
        log::debug!("command {cmd:?}");
        match cmd {
            Command::CaptureArea => self.begin_selection(lumora_ui::events::OverlayMode::Capture, false),
            Command::CaptureWindow => self.begin_selection(lumora_ui::events::OverlayMode::Capture, true),
            Command::CaptureFullscreen => self.capture_fullscreen(),
            Command::CaptureText => self.begin_selection(lumora_ui::events::OverlayMode::Text, false),
            Command::ToggleRecording => self.toggle_recording(),
            Command::OpenFolder => {
                let folder = self.settings.borrow().save_folder(&self.home);
                system::open_folder(&folder);
            }
            Command::OpenDocument => self.open_document_panel(),
            Command::ShowSettings => self.show_settings(),
            Command::ShowOnboarding => self.show_onboarding(),
            Command::Quit => NSApplication::sharedApplication(self.mtm).terminate(None),
        }
    }

    /// Ensure Screen Recording permission; shows onboarding when missing.
    pub fn require_permission(self: &Rc<Self>) -> bool {
        if permission::has_screen_recording() {
            return true;
        }
        self.show_onboarding();
        false
    }

    // -- UI events ----------------------------------------------------------

    fn enqueue(&self, event: UiEvent) {
        self.events.borrow_mut().push_back(event);
        if !self.drain_scheduled.replace(true) {
            on_main(|app| app.drain_events());
        }
    }

    fn drain_events(self: &Rc<Self>) {
        self.drain_scheduled.set(false);
        loop {
            let next = self.events.borrow_mut().pop_front();
            match next {
                Some(e) => self.handle_event(e),
                None => break,
            }
        }
    }

    fn handle_event(self: &Rc<Self>, event: UiEvent) {
        log::debug!("ui event {event:?}");
        match event {
            UiEvent::OverlayConfirmed { mode, target } => self.on_overlay_confirmed(mode, target),
            UiEvent::OverlayCancelled { .. } => self.set_mode(Mode::Idle),
            UiEvent::Card { id, action } => self.on_card_action(id, action),
            UiEvent::CardDismissed(id) => {
                // An open editor still needs the capture.
                if !self.editors.borrow().contains_key(&id) {
                    self.captures.borrow_mut().remove(&id);
                }
            }
            UiEvent::Editor { id, action } => self.on_editor(id, action),
            UiEvent::PinClosed(id) => {
                self.pins.borrow_mut().remove(&id);
            }
            UiEvent::CountdownFinished => self.on_countdown_finished(),
            UiEvent::CountdownCancelled => self.cancel_recording_setup(),
            UiEvent::StopRecording => self.stop_recording(),
            UiEvent::SettingsChanged(s) => self.apply_settings(*s),
            UiEvent::BeautifyChanged { from, style } => self.on_beautify_changed(from, *style),
            UiEvent::ShortcutRecording(on) => self.hotkeys.borrow_mut().suspend(on),
            UiEvent::Onboarding(action) => self.on_onboarding(action),
        }
    }

    fn on_card_action(self: &Rc<Self>, id: u64, action: CardAction) {
        match action {
            CardAction::Copy => {
                if let Some(rec) = self.captures.borrow().get(&id) {
                    match (&rec.image, rec.video) {
                        (Some(img), false) => {
                            // The record holds the plain capture; the file
                            // (and so the clipboard) gets the beautified one.
                            let style = self.settings.borrow().beautify.clone();
                            let out =
                                lumora_capture::beautify_render::beautify_for_export(img.clone(), &style, rec.scale);
                            lumora_capture::clipboard::copy_image(&out, rec.scale, Some(&rec.file));
                        }
                        _ => lumora_capture::clipboard::copy_file(&rec.file),
                    }
                }
                self.quick.dismiss(id);
            }
            CardAction::Save => {
                let folder = self.settings.borrow().save_folder(&self.home);
                let mut caps = self.captures.borrow_mut();
                if let Some(rec) = caps.get_mut(&id) {
                    if rec.temp {
                        let name = rec.file.file_name().map(PathBuf::from).unwrap_or_default();
                        let stem = name.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        let ext = name.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        let dest = lumora_core::filename::unique_path(&folder, &stem, &ext, |p| p.exists());
                        let _ = std::fs::create_dir_all(&folder);
                        if std::fs::rename(&rec.file, &dest)
                            .or_else(|_| std::fs::copy(&rec.file, &dest).map(|_| ()))
                            .is_ok()
                        {
                            rec.file = dest;
                            rec.temp = false;
                        }
                    }
                    system::reveal_in_finder(&rec.file);
                }
                drop(caps);
                self.quick.dismiss(id);
            }
            CardAction::Pin => {
                let pin = self.captures.borrow().get(&id).and_then(|rec| {
                    let image = rec.image.clone()?;
                    let frame = lumora_ui::util::flip(rec.frame, self.mtm);
                    Some(PinWindow::show(self.mtm, id, image, frame, self.sink.clone()))
                });
                if let Some(pin) = pin {
                    self.pins.borrow_mut().insert(id, pin);
                }
                self.quick.dismiss(id);
            }
            CardAction::Delete => {
                if let Some(rec) = self.captures.borrow_mut().remove(&id) {
                    system::trash(&rec.file);
                    let doc = lumora_core::lumora_doc::doc_path_for(&rec.file);
                    if rec.editable.is_some() && doc.exists() {
                        system::trash(&doc);
                    }
                }
                self.quick.dismiss(id);
            }
            CardAction::Open => {
                if let Some(rec) = self.captures.borrow().get(&id) {
                    open_file(&rec.file);
                }
            }
            CardAction::DraggedOut => self.quick.dismiss(id),
            CardAction::Annotate => {
                self.open_editor(id);
                self.quick.dismiss(id);
            }
        }
    }

    // -- settings -------------------------------------------------------------

    fn show_settings(self: &Rc<Self>) {
        if let Some(w) = self.settings_window.borrow().as_ref() {
            if w.is_visible() {
                w.focus(self.mtm);
                return;
            }
        }
        let w = SettingsWindow::show(self.mtm, self.settings.borrow().clone(), self.home.clone(), self.sink.clone());
        w.set_unavailable(&self.hotkey_failures.borrow());
        *self.settings_window.borrow_mut() = Some(w);
    }

    fn apply_settings(self: &Rc<Self>, new: Settings) {
        let login_changed = new.general.launch_at_login != self.settings.borrow().general.launch_at_login;
        let language_changed = new.general.language != self.settings.borrow().general.language;
        *self.settings.borrow_mut() = new;
        let s = self.settings.borrow();
        if let Some(path) = &self.settings_path {
            if let Err(e) = s.save(path) {
                log::error!("saving settings failed: {e}");
            }
        }
        self.quick.set_timeout(s.general.quick_access_timeout);
        self.status.update_shortcuts(&s);
        if login_changed {
            if let Err(e) = system::set_launch_at_login(s.general.launch_at_login) {
                log::warn!("launch at login: {e}");
            }
        }
        let beautify = s.beautify.clone();
        drop(s);
        for editor in self.editors.borrow().values() {
            editor.set_beautify(beautify.clone());
        }
        self.apply_hotkeys();
        if language_changed {
            self.apply_language();
        }
    }

    /// An editor's beautify panel changed the style: remember it (Settings,
    /// saved to disk) and push it to the other editors and the Settings
    /// window's snapshot.
    fn on_beautify_changed(self: &Rc<Self>, from: u64, style: lumora_core::beautify::BeautifyStyle) {
        let style = style.validated();
        {
            let mut s = self.settings.borrow_mut();
            if s.beautify == style {
                return;
            }
            s.beautify = style.clone();
        }
        if let Some(path) = &self.settings_path {
            if let Err(e) = self.settings.borrow().save(path) {
                log::error!("saving settings failed: {e}");
            }
        }
        for (id, editor) in self.editors.borrow().iter() {
            if *id != from {
                editor.set_beautify(style.clone());
            }
        }
        if let Some(w) = self.settings_window.borrow().as_ref() {
            w.set_beautify(style);
        }
    }

    /// Switch the UI language live: menus re-title, the Settings window is
    /// rebuilt in place. Open editors keep their language until reopened.
    fn apply_language(self: &Rc<Self>) {
        let lang = system::apply_language(self.settings.borrow().general.language);
        log::info!("language: {}", lang.tag());
        self.status.relabel();
        let reopen = self.settings_window.borrow_mut().take();
        if let Some(old) = reopen.filter(|w| w.is_visible()) {
            let origin = old.origin();
            drop(old);
            self.show_settings();
            if let Some(w) = self.settings_window.borrow().as_ref() {
                w.set_origin(origin);
            }
        }
    }

    fn apply_hotkeys(&self) {
        let bindings = self.settings.borrow().shortcuts.bound();
        let failed = self.hotkeys.borrow_mut().set(bindings.clone());
        for a in &failed {
            log::warn!("shortcut for “{}” is in use by another app", a.title());
        }
        // Toast only for newly failing shortcuts, so re-applying settings
        // doesn't nag; the Shortcuts tab always shows the full picture.
        let previous = self.hotkey_failures.replace(failed.clone());
        if let Some(a) = failed.iter().find(|a| !previous.contains(a)) {
            if let Some((_, hk)) = bindings.iter().find(|(b, _)| b == a) {
                let detail = lumora_core::i18n::trf(Msg::ToastShortcutTakenDetail, &[hk]);
                self.notify(Notice::Warning(Msg::ToastShortcutTaken), Some(detail));
            }
        }
        if let Some(w) = self.settings_window.borrow().as_ref() {
            w.set_unavailable(&failed);
        }
    }

    // -- onboarding -----------------------------------------------------------

    fn show_onboarding(self: &Rc<Self>) {
        if self.onboarding.borrow().is_some() {
            return;
        }
        let granted = permission::has_screen_recording();
        *self.onboarding.borrow_mut() = Some(Onboarding::show(self.mtm, self.sink.clone(), granted));
        // Poll so the panel flips to "Relaunch" as soon as access is granted.
        let weak = Rc::downgrade(self);
        let timer = lumora_ui::util::schedule_timer(1.0, true, move || {
            let Some(app) = weak.upgrade() else { return false };
            if app.onboarding.borrow().is_none() {
                return false;
            }
            if permission::has_screen_recording() {
                let t = app.onboarding.borrow().as_ref().and_then(|o| o.set_granted(app.mtm, app.sink.clone()));
                if t.is_some() {
                    *app.onboarding_target.borrow_mut() = t;
                }
                return false;
            }
            true
        });
        *self.permission_poll.borrow_mut() = Some(timer);
    }

    fn on_onboarding(self: &Rc<Self>, action: OnboardingAction) {
        match action {
            OnboardingAction::RequestAccess => {
                if !permission::request_screen_recording() {
                    system::open_url(permission::SCREEN_RECORDING_SETTINGS_URL);
                }
            }
            OnboardingAction::OpenSystemSettings => system::open_url(permission::SCREEN_RECORDING_SETTINGS_URL),
            OnboardingAction::Relaunch => system::relaunch(self.mtm),
            OnboardingAction::Dismiss => {
                if let Some(t) = self.permission_poll.borrow_mut().take() {
                    t.invalidate();
                }
                let o = self.onboarding.borrow_mut().take();
                drop(o);
            }
        }
    }
}

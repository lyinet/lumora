//! Events flowing from UI components to the app.

use std::rc::Rc;

use lumora_core::geometry::Rect;
use lumora_core::selection::Target;
use lumora_core::settings::Settings;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;

/// A display image grabbed before the overlay appears, so the selection is
/// made on (and cropped from) a frozen frame.
pub struct FrozenDisplay {
    /// `CGDirectDisplayID`.
    pub id: u32,
    /// Global Quartz frame in points.
    pub frame: Rect,
    /// Pixels per point of `image`.
    pub scale: f64,
    pub image: CFRetained<CGImage>,
}

impl std::fmt::Debug for FrozenDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrozenDisplay").field("id", &self.id).field("frame", &self.frame).finish_non_exhaustive()
    }
}

/// What the overlay is selecting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMode {
    /// Screenshot: frozen backdrop, loupe, window picking.
    Capture,
    /// Choose a region to record: live backdrop, red accent, no loupe.
    Record,
    /// Choose a region to read text from (OCR): frozen backdrop, no loupe.
    Text,
}

/// Buttons and gestures on a Quick Access card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    Copy,
    Save,
    Pin,
    Delete,
    /// Clicked the thumbnail: open in the default viewer.
    Open,
    /// "Annotate" button or double-click: open the editor.
    Annotate,
    /// Dropped into another app; the card dismisses itself.
    DraggedOut,
}

/// A flattened editor export, plus the editable state it came from (for
/// the `.lumora` document saved next to it).
pub struct EditorImage {
    pub image: CFRetained<CGImage>,
    /// Pixels per point.
    pub scale: f64,
    /// The annotations (and crop) that `image` flattens.
    pub document: lumora_core::annotate::Document,
    /// The editor's beautify style at export time.
    pub beautify: lumora_core::beautify::BeautifyStyle,
}

impl std::fmt::Debug for EditorImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = (CGImage::width(Some(&self.image)), CGImage::height(Some(&self.image)));
        write!(f, "EditorImage({w}×{h} @{}x)", self.scale)
    }
}

/// What the annotation editor tells the app.
#[derive(Debug)]
pub enum EditorAction {
    /// ⌘C: the flattened image is already on the clipboard.
    Copied(EditorImage),
    /// ⌘S: write this over the capture's file.
    Save(EditorImage),
    /// The window closed. Carries the flattened image if there were
    /// unsaved edits (so the card thumbnail can follow).
    Closed(Option<EditorImage>),
}

/// Onboarding panel buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingAction {
    RequestAccess,
    OpenSystemSettings,
    Relaunch,
    Dismiss,
}

/// Everything a UI component can tell the app.
#[derive(Debug)]
pub enum UiEvent {
    OverlayConfirmed {
        mode: OverlayMode,
        target: Target,
    },
    OverlayCancelled {
        mode: OverlayMode,
    },
    Card {
        id: u64,
        action: CardAction,
    },
    /// The card timed out or was swiped away; the app may drop its data.
    CardDismissed(u64),
    PinClosed(u64),
    Editor {
        id: u64,
        action: EditorAction,
    },
    CountdownFinished,
    CountdownCancelled,
    StopRecording,
    SettingsChanged(Box<Settings>),
    /// An editor's beautify panel changed the style ("remember last"): the
    /// app stores it in `Settings::beautify`, saves, and pushes it to the
    /// other open editors. `from` is the editor's capture id.
    BeautifyChanged {
        from: u64,
        style: Box<lumora_core::beautify::BeautifyStyle>,
    },
    /// The Shortcuts tab started/stopped recording a key combo; the app
    /// suspends global hotkeys meanwhile so the combo reaches the field.
    ShortcutRecording(bool),
    Onboarding(OnboardingAction),
}

/// Where components send events. The app's implementation queues them and
/// processes them on the next main-queue turn (never re-entrantly).
pub type EventSink = Rc<dyn Fn(UiEvent)>;

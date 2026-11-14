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
    /// Dropped into another app; the card dismisses itself.
    DraggedOut,
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
    CountdownFinished,
    CountdownCancelled,
    StopRecording,
    SettingsChanged(Box<Settings>),
    /// The Shortcuts tab started/stopped recording a key combo; the app
    /// suspends global hotkeys meanwhile so the combo reaches the field.
    ShortcutRecording(bool),
    Onboarding(OnboardingAction),
}

/// Where components send events. The app's implementation queues them and
/// processes them on the next main-queue turn (never re-entrantly).
pub type EventSink = Rc<dyn Fn(UiEvent)>;

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
    /// "Make Tutorial" (recordings with a click log).
    Tutorial,
    /// "Subtitles" (recordings): transcribe, or reopen the transcript.
    Subtitles,
    /// "Trim / GIF…" (recordings): open the trim window.
    Trim,
}

/// What the trim window tells the app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrimAction {
    /// A slider moved: show the frame at this time (seconds).
    Preview(f64),
    /// "Save Trimmed".
    Save(lumora_core::clip::TrimRange),
    /// "Export GIF…" with the window's options.
    ExportGif { range: lumora_core::clip::TrimRange, width: lumora_core::clip::GifWidth, fps: u32 },
    /// Cancel the running export.
    Cancel,
    /// The window closed (the app cancels any export and drops it).
    Closed,
}

/// What the transcript window tells the app. Each carries the transcript
/// with the window's current edits.
#[derive(Debug)]
pub enum TranscriptAction {
    ExportSrt(Box<lumora_core::transcript::Transcript>),
    ExportVtt(Box<lumora_core::transcript::Transcript>),
    CopyChapters(Box<lumora_core::transcript::Transcript>),
    CopySummary(Box<lumora_core::transcript::Transcript>),
    /// The window closed (the app saves `.transcript.json` and drops it).
    Closed(Box<lumora_core::transcript::Transcript>),
}

/// What the tutorial window tells the app. Each carries the tutorial with
/// the window's current edits.
#[derive(Debug)]
pub enum TutorialAction {
    ExportMarkdown(Box<lumora_core::tutorial::Tutorial>),
    ExportHtml(Box<lumora_core::tutorial::Tutorial>),
    CopyMarkdown(Box<lumora_core::tutorial::Tutorial>),
    /// The window closed (the app saves `.steps.json` and drops it).
    Closed(Box<lumora_core::tutorial::Tutorial>),
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
    /// Run an AI writing task; stream the answer back with
    /// `EditorWindow::ai_delta` / `ai_finished` under `request.request`.
    AiWrite(Box<AiWriteRequest>),
    /// Stop AI request `request` (Stop, Close, or a newer request).
    AiCancel(u64),
    /// "Open AI Settings" in the result sheet.
    OpenAiSettings,
    /// Smart Redact: find sensitive info in the capture and answer with
    /// `EditorWindow::smart_redact_finished` under `request.request`.
    SmartRedact(Box<SmartRedactRequest>),
}

/// A smart-redaction request from the editor. The app runs OCR, face
/// detection and (when enabled) the AI check off the main thread.
pub struct SmartRedactRequest {
    /// Per-editor request id; replies with another id are stale.
    pub request: u64,
    /// The unannotated capture (detection runs on this).
    pub base: CFRetained<CGImage>,
    /// Pixels per point of `base`.
    pub scale: f64,
}

impl std::fmt::Debug for SmartRedactRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmartRedactRequest").field("request", &self.request).finish_non_exhaustive()
    }
}

/// An AI writing request from the editor. The app reads the key, runs OCR
/// when needed and talks to the provider; the editor only shows the result.
pub struct AiWriteRequest {
    /// Per-editor request id; replies with another id are stale.
    pub request: u64,
    pub kind: lumora_core::ai::AiTaskKind,
    /// The selected text annotation's text (Polish / Translate), if any.
    pub source_text: Option<String>,
    /// The annotations, for step badges and annotation text.
    pub document: lumora_core::annotate::Document,
    /// The unannotated capture (OCR runs on this).
    pub base: CFRetained<CGImage>,
    /// Pixels per point of `base`.
    pub scale: f64,
}

impl std::fmt::Debug for AiWriteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiWriteRequest")
            .field("request", &self.request)
            .field("kind", &self.kind)
            .field("has_source", &self.source_text.is_some())
            .finish_non_exhaustive()
    }
}

/// What the Capture Text result panel tells the app.
#[derive(Debug)]
pub enum TextResultAction {
    /// Run an AI task on the recognised text; stream the answer back with
    /// `TextResultPanel::ai_delta` / `ai_finished` under `request`.
    Run(TextAiRequest),
    /// Stop request `id` (Stop, Close, or a newer request).
    Cancel(u64),
    /// "Open AI Settings" in the result sheet.
    OpenAiSettings,
    /// The panel closed.
    Closed,
}

/// An AI request from the Capture Text panel. `input` is already prepared
/// ([`lumora_core::ai::ai_input`]): the text, the positioned listing for
/// tables, or the locally indented code.
pub struct TextAiRequest {
    pub request: u64,
    pub kind: lumora_core::ai::AiTaskKind,
    pub input: String,
}

impl std::fmt::Debug for TextAiRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never log the recognised text.
        f.debug_struct("TextAiRequest")
            .field("request", &self.request)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// A thumbnail the history window needs: the app renders it off the main
/// thread (cached on disk under `key`) and answers with
/// `HistoryWindow::set_thumbnail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbRequest {
    pub key: String,
    pub path: std::path::PathBuf,
    pub kind: lumora_core::history::ItemKind,
}

/// What the capture history window tells the app. Paths name an item by
/// its primary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryAction {
    /// Reopen in the editor (its `.lumora` when there is one); recordings
    /// open in the default player.
    Reopen(std::path::PathBuf),
    Copy(std::path::PathBuf),
    Reveal(std::path::PathBuf),
    /// Move to the Trash (the app confirms first).
    Delete(std::path::PathBuf),
    /// Open the trim / GIF window (recordings).
    Trim(std::path::PathBuf),
    /// Move everything older than the retention setting to the Trash (the
    /// app confirms first).
    TrashExpired,
    NeedThumbnails(Vec<ThumbRequest>),
    Closed,
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
    /// The Capture Text result panel.
    TextResult(TextResultAction),
    /// A tutorial window (`id` from `TutorialWindow::show`).
    Tutorial {
        id: u64,
        action: TutorialAction,
    },
    /// A transcript window (`id` from `TranscriptWindow::show`).
    Transcript {
        id: u64,
        action: TranscriptAction,
    },
    /// A trim window (`id` from `TrimWindow::show`).
    Trim {
        id: u64,
        action: TrimAction,
    },
    /// The capture history window.
    History(HistoryAction),
    /// The AI tab needs the stored API key for `provider`; the app reads
    /// the Keychain and answers with `SettingsWindow::set_ai_key`.
    AiKeyRequested(lumora_core::ai::AiProvider),
    /// Store `key` for `provider` in the Keychain; an empty key deletes it.
    /// Errors go back via `SettingsWindow::set_ai_key_error`.
    AiKeySave {
        provider: lumora_core::ai::AiProvider,
        key: lumora_core::ai::Secret,
    },
    /// "Test connection": run `test_connection` off the main thread and
    /// answer with `SettingsWindow::set_ai_test_result(id, …)`. `key` is
    /// the field's current value (it may not be saved yet).
    AiTestConnection {
        id: u64,
        settings: Box<lumora_core::ai::AiSettings>,
        key: lumora_core::ai::Secret,
    },
}

/// Where components send events. The app's implementation queues them and
/// processes them on the next main-queue turn (never re-entrantly).
pub type EventSink = Rc<dyn Fn(UiEvent)>;

//! The transcript window's live subtitle preview: a frame of the movie with
//! the burned-in subtitles drawn over it exactly as the export draws them,
//! and a scrubber over the movie.
//!
//! "Exactly" is by construction, not by imitation:
//! - the cues come from [`cues_at`], which runs the export's own
//!   `layout_cues` (same font size, wrapping, box, margins, position) at the
//!   video's render size ([`lumora_capture::burn::preview_canvas`] reads the
//!   size the export's video composition renders at);
//! - each cue is drawn by [`lumora_capture::burn::cue_overlay`], the layer
//!   code the export uses;
//! - the frame and the overlay live in a "stage" layer of the video's pixel
//!   size, and the whole stage is scaled to fit the pane ([`fit`]), just as
//!   a player scales the exported movie. Only the text's rasterisation
//!   scale differs ([`contents_scale`]), so it stays sharp when scaled.
//!
//! Frames are generated off the main thread by the app (it owns the movie
//! path and the worker queue): the pane asks for one through `on_request`
//! and gets it back in [`SubtitlePreview::frame_arrived`]. At most one
//! request is in flight; a newer wish replaces any queued one, and frames are
//! kept in a small LRU cache by time bucket.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lumora_capture::burn::{cue_overlay, Canvas};
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Msg};
use lumora_core::transcript::burn::{BurnLayout, SubtitleStyle};
use lumora_core::transcript::chapters::format_chapter_time;
use lumora_core::transcript::preview::{
    clamp_time, contents_scale, cues_at, fit, frame_key, key_time, FrameCache, FRAME_CACHE_CAP,
};
use lumora_core::transcript::Segment;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSFont, NSLineBreakMode, NSSlider, NSTextAlignment, NSTextField, NSView,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGImage};
use objc2_foundation::NSString;
use objc2_quartz_core::{CALayer, CATransaction, CATransform3D};

use crate::tokens::typography;
use crate::util::{cf_as_object, cg_rect, mono_digits, weight, ActionTarget};

/// Height of the scrubber row under the stage.
const SCRUB_H: f64 = 34.0;
const TIME_W: f64 = 128.0;
const PAD: f64 = 20.0;
/// Pixels on the longer side of a requested frame: enough for a Retina
/// pane, small enough that the cache stays modest (16 × ~3.7 MB).
pub const FRAME_MAX_SIDE: f64 = 1280.0;

/// What the preview shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PreviewSource {
    /// Still reading the movie.
    Loading,
    /// A movie with video.
    Video(Canvas),
    /// Audio only: no pane, just a note.
    NoVideo,
    /// The movie couldn't be read.
    Failed,
}

/// The preview pane (stage + scrubber + note).
pub struct SubtitlePreview {
    /// The whole pane; the window frames it.
    pub view: Retained<NSView>,
    stage_view: Retained<NSView>,
    root: Retained<CALayer>,
    stage: Retained<CALayer>,
    frame_layer: Retained<CALayer>,
    overlay: Retained<CALayer>,
    slider: Retained<NSSlider>,
    time_label: Retained<NSTextField>,
    note: Retained<NSTextField>,
    _targets: Vec<Retained<ActionTarget>>,
    source: Cell<PreviewSource>,
    time: Cell<f64>,
    /// Bucket of the frame we want on screen.
    want: Cell<Option<i64>>,
    /// Bucket of the frame on screen.
    shown: Cell<Option<i64>>,
    in_flight: Cell<Option<i64>>,
    queued: Cell<Option<i64>>,
    cache: RefCell<FrameCache<CFRetained<CGImage>>>,
    on_request: Box<dyn Fn(f64)>,
}

impl std::fmt::Debug for SubtitlePreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubtitlePreview").field("source", &self.source.get()).finish_non_exhaustive()
    }
}

impl SubtitlePreview {
    /// A pane of `frame` (in its superview). `on_scrub` runs when the
    /// scrubber moves (the window debounces and re-renders); `on_request`
    /// asks the app for the frame at a time.
    pub fn new(
        mtm: MainThreadMarker,
        frame: Rect,
        on_scrub: impl Fn() + 'static,
        on_request: impl Fn(f64) + 'static,
    ) -> Rc<Self> {
        let (w, h) = (frame.width(), frame.height());
        let view = NSView::initWithFrame(NSView::alloc(mtm), cg_rect(frame));

        // The stage view hosts a plain (bottom-left origin) layer tree, the
        // same orientation as the export's Core Animation tool.
        let stage_h = (h - SCRUB_H - 8.0).max(40.0);
        let stage_view =
            NSView::initWithFrame(NSView::alloc(mtm), cg_rect(Rect::new(PAD, SCRUB_H, w - 2.0 * PAD, stage_h)));
        stage_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let root = CALayer::new();
        root.setGeometryFlipped(false);
        let backdrop = CGColor::new_generic_gray(0.08, 1.0);
        root.setBackgroundColor(Some(&backdrop));
        root.setCornerRadius(6.0);
        root.setMasksToBounds(true);
        stage_view.setLayer(Some(&root));
        stage_view.setWantsLayer(true);
        let stage = CALayer::new();
        stage.setGeometryFlipped(false);
        let frame_layer = CALayer::new();
        let black = CGColor::new_generic_gray(0.0, 1.0);
        frame_layer.setBackgroundColor(Some(&black));
        let overlay = CALayer::new();
        stage.addSublayer(&frame_layer);
        stage.addSublayer(&overlay);
        root.addSublayer(&stage);
        view.addSubview(&stage_view);

        let slider =
            NSSlider::initWithFrame(NSSlider::alloc(mtm), cg_rect(Rect::new(PAD, 6.0, w - 2.0 * PAD - TIME_W, 24.0)));
        slider.setMinValue(0.0);
        slider.setMaxValue(1.0);
        slider.setContinuous(true);
        slider.setEnabled(false);
        slider.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let scrub = ActionTarget::new(mtm, move |_| on_scrub());
        // SAFETY: the target is kept alive by the pane.
        unsafe {
            slider.setTarget(Some(&scrub));
            slider.setAction(Some(ActionTarget::action()));
        }
        view.addSubview(&slider);

        let time_label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        time_label.setFont(Some(&mono_digits(typography::CAPTION, weight::MEDIUM)));
        time_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        time_label.setAlignment(NSTextAlignment::Right);
        time_label.setFrame(cg_rect(Rect::new(w - PAD - TIME_W + 8.0, 9.0, TIME_W - 8.0, 18.0)));
        time_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        view.addSubview(&time_label);

        let note = NSTextField::labelWithString(&NSString::from_str(tr(Msg::SubtitlePreviewLoading)), mtm);
        note.setFont(Some(&NSFont::systemFontOfSize_weight(typography::BODY, weight::REGULAR)));
        note.setTextColor(Some(&NSColor::secondaryLabelColor()));
        note.setAlignment(NSTextAlignment::Center);
        note.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        note.setFrame(cg_rect(Rect::new(PAD, (h - 20.0) / 2.0, w - 2.0 * PAD, 20.0)));
        note.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        view.addSubview(&note);

        let this = Rc::new(SubtitlePreview {
            view,
            stage_view,
            root,
            stage,
            frame_layer,
            overlay,
            slider,
            time_label,
            note,
            _targets: vec![scrub],
            source: Cell::new(PreviewSource::Loading),
            time: Cell::new(0.0),
            want: Cell::new(None),
            shown: Cell::new(None),
            in_flight: Cell::new(None),
            queued: Cell::new(None),
            cache: RefCell::new(FrameCache::new(FRAME_CACHE_CAP)),
            on_request: Box::new(on_request),
        });
        this.stage_view.setHidden(true);
        this.relayout();
        this
    }

    pub fn source(&self) -> PreviewSource {
        self.source.get()
    }

    /// Whether the pane has a movie to show (else the window collapses it to
    /// a one-line note).
    pub fn has_video(&self) -> bool {
        matches!(self.source.get(), PreviewSource::Video(_) | PreviewSource::Loading)
    }

    /// Set what the preview shows. A video starts at `start`.
    pub fn set_source(&self, source: PreviewSource, start: f64) {
        self.source.set(source);
        self.cache.borrow_mut().clear();
        self.want.set(None);
        self.shown.set(None);
        self.queued.set(None);
        // SAFETY: clearing contents is always valid.
        unsafe { self.frame_layer.setContents(None) };
        let (note, video) = match source {
            PreviewSource::Loading => (Some(Msg::SubtitlePreviewLoading), false),
            PreviewSource::Video(_) => (None, true),
            PreviewSource::NoVideo => (Some(Msg::SubtitlePreviewNoVideo), false),
            PreviewSource::Failed => (Some(Msg::SubtitlePreviewFailed), false),
        };
        self.note.setHidden(note.is_none());
        if let Some(msg) = note {
            self.note.setStringValue(&NSString::from_str(tr(msg)));
        }
        self.stage_view.setHidden(!video);
        self.slider.setEnabled(video);
        self.slider.setHidden(!video);
        self.time_label.setHidden(!video);
        if let PreviewSource::Video(c) = source {
            self.slider.setMaxValue(c.duration.max(0.01));
            self.set_time(start);
        }
        self.relayout();
    }

    /// The scrub position (seconds).
    pub fn time(&self) -> f64 {
        match self.source.get() {
            PreviewSource::Video(c) => clamp_time(self.slider.doubleValue(), c.duration),
            _ => self.time.get(),
        }
    }

    /// Move the scrubber to `t`.
    pub fn set_time(&self, t: f64) {
        let PreviewSource::Video(c) = self.source.get() else { return };
        let t = clamp_time(t, c.duration);
        self.time.set(t);
        self.slider.setDoubleValue(t);
        self.update_time_label();
    }

    /// Re-read the scrubber (it moved) and update the time readout.
    pub fn scrubbed(&self) {
        self.time.set(self.time());
        self.update_time_label();
    }

    fn update_time_label(&self) {
        let PreviewSource::Video(c) = self.source.get() else { return };
        let long = c.duration >= 3600.0;
        let text = format!("{} / {}", format_chapter_time(self.time(), long), format_chapter_time(c.duration, long));
        self.time_label.setStringValue(&NSString::from_str(&text));
    }

    /// Fit the stage to the pane (after a resize or a backing change; the
    /// window re-renders afterwards so the text is rasterised for the new
    /// scale).
    pub fn relayout(&self) {
        let PreviewSource::Video(c) = self.source.get() else { return };
        let b = self.stage_view.bounds();
        let f = fit(c.width, c.height, b.size.width, b.size.height);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.root.setFrame(b);
        let video = CGRect::new(CGPoint::ZERO, CGSize::new(c.width, c.height));
        self.stage.setBounds(video);
        self.stage.setPosition(CGPoint::new(f.frame.mid().x, f.frame.mid().y));
        self.stage.setTransform(CATransform3D::new_scale(f.scale, f.scale, 1.0));
        self.frame_layer.setFrame(video);
        self.overlay.setFrame(video);
        CATransaction::commit();
    }

    fn text_scale(&self, fit_scale: f64) -> f64 {
        let backing = self.stage_view.window().map_or(2.0, |w| w.backingScaleFactor());
        contents_scale(fit_scale, backing)
    }

    /// Draw `segments` with `style` at the scrub time, and fetch the frame
    /// there if it isn't cached.
    pub fn render(&self, segments: &[Segment], style: SubtitleStyle) {
        let PreviewSource::Video(c) = self.source.get() else { return };
        let t = self.time();
        let layout = BurnLayout::new(c.width, c.height, style);
        let cues = cues_at(segments, &layout, c.duration, t);
        let b = self.stage_view.bounds();
        let scale = self.text_scale(fit(c.width, c.height, b.size.width, b.size.height).scale);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        // SAFETY: removing every sublayer is always valid.
        unsafe { self.overlay.setSublayers(None) };
        for cue in &cues {
            self.overlay.addSublayer(&cue_overlay(cue, &layout, scale));
        }
        CATransaction::commit();
        self.want_frame(frame_key(t));
    }

    fn want_frame(&self, key: i64) {
        self.want.set(Some(key));
        if self.shown.get() == Some(key) {
            return;
        }
        let cached = self.cache.borrow_mut().get(key);
        if let Some(image) = cached {
            self.show(key, &image);
            return;
        }
        if self.in_flight.get().is_some() {
            // Only the newest wish matters.
            self.queued.set(Some(key));
        } else {
            self.request(key);
        }
    }

    fn request(&self, key: i64) {
        self.in_flight.set(Some(key));
        (self.on_request)(key_time(key));
    }

    fn show(&self, key: i64, image: &CGImage) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        // SAFETY: a CGImage is valid layer contents.
        unsafe { self.frame_layer.setContents(Some(cf_as_object(image))) };
        CATransaction::commit();
        self.shown.set(Some(key));
        if matches!(self.source.get(), PreviewSource::Video(_)) {
            self.note.setHidden(true);
        }
    }

    /// The frame the app generated for a request at `time` (`None` if it
    /// failed).
    pub fn frame_arrived(&self, time: f64, image: Option<CFRetained<CGImage>>) {
        let key = frame_key(time);
        if self.in_flight.get() == Some(key) {
            self.in_flight.set(None);
        }
        match image {
            Some(img) => {
                self.cache.borrow_mut().insert(key, img.clone());
                if self.want.get() == Some(key) {
                    self.show(key, &img);
                }
            }
            None if self.want.get() == Some(key) && self.shown.get().is_none() => {
                self.note.setStringValue(&NSString::from_str(tr(Msg::SubtitlePreviewFailed)));
                self.note.setHidden(false);
            }
            None => {}
        }
        if self.in_flight.get().is_none() {
            if let Some(next) = self.queued.take() {
                if Some(next) == self.want.get() && self.shown.get() != Some(next) {
                    let cached = self.cache.borrow_mut().get(next);
                    match cached {
                        Some(image) => self.show(next, &image),
                        None => self.request(next),
                    }
                }
            }
        }
    }
}

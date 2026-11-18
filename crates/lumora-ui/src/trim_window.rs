//! The trim window for a recording: a preview frame, Start / End sliders
//! with time labels, "Save Trimmed", GIF options (width, frame rate, size
//! estimate) with "Export GIF…", and a progress bar with Cancel while an
//! export runs.
//!
//! The window only keeps the chosen range and options; the app does the
//! exports (off the main thread) and feeds back preview frames and progress.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use lumora_core::clip::{gif_size, time_label, GifPlan, GifWidth, TrimRange, GIF_FPS};
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, trf, Msg};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSControlSize, NSFont, NSImage, NSImageScaling,
    NSImageView, NSLineBreakMode, NSPopUpButton, NSProgressIndicator, NSProgressIndicatorStyle, NSSlider, NSTextField,
    NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSString};

use crate::events::{EventSink, TrimAction, UiEvent};
use crate::tokens::typography;
use crate::util::{cg_rect, screen_under_mouse, weight, ActionTarget};

const W: f64 = 580.0;
const H: f64 = 520.0;
const PAD: f64 = 20.0;
const LABEL_W: f64 = 92.0;
const TIME_W: f64 = 72.0;

struct DelegateIvars {
    state: RefCell<Weak<State>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraTrimWindowDelegate"]
    #[ivars = DelegateIvars]
    struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                s.emit(TrimAction::Closed);
            }
        }
    }
);

impl WindowDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { state: RefCell::new(Weak::new()) });
        unsafe { msg_send![super(this), init] }
    }
}

struct State {
    id: u64,
    sink: EventSink,
    window: Retained<NSWindow>,
    duration: f64,
    video: (u32, u32),
    range: Cell<TrimRange>,
    preview: Retained<NSImageView>,
    start: Retained<NSSlider>,
    end: Retained<NSSlider>,
    start_label: Retained<NSTextField>,
    end_label: Retained<NSTextField>,
    length: Retained<NSTextField>,
    width_popup: Retained<NSPopUpButton>,
    fps_popup: Retained<NSPopUpButton>,
    estimate: Retained<NSTextField>,
    save: Retained<NSButton>,
    gif: Retained<NSButton>,
    status: Retained<NSTextField>,
    bar: Retained<NSProgressIndicator>,
    cancel: Retained<NSButton>,
    _targets: RefCell<Vec<Retained<ActionTarget>>>,
    _delegate: RefCell<Option<Retained<WindowDelegate>>>,
}

impl State {
    fn emit(&self, action: TrimAction) {
        (self.sink)(UiEvent::Trim { id: self.id, action });
    }

    fn gif_width(&self) -> GifWidth {
        GifWidth::ALL[(self.width_popup.indexOfSelectedItem().max(0) as usize).min(GifWidth::ALL.len() - 1)]
    }

    fn gif_fps(&self) -> u32 {
        GIF_FPS[(self.fps_popup.indexOfSelectedItem().max(0) as usize).min(GIF_FPS.len() - 1)]
    }

    /// A slider moved: clamp, sync both sliders and labels, ask for a frame.
    fn slider_moved(&self, which_start: bool) {
        let (s, e) = (self.start.doubleValue(), self.end.doubleValue());
        let mut r = TrimRange { start: s, end: e };
        // Dragging one handle past the other pushes the other along.
        if which_start && s > e {
            r.end = s;
        } else if !which_start && e < s {
            r.start = e;
        }
        let r = r.clamped(self.duration);
        self.range.set(r);
        self.refresh();
        self.emit(TrimAction::Preview(if which_start { r.start } else { r.end }));
    }

    fn refresh(&self) {
        let r = self.range.get();
        self.start.setDoubleValue(r.start);
        self.end.setDoubleValue(r.end);
        self.start_label.setStringValue(&NSString::from_str(&time_label(r.start)));
        self.end_label.setStringValue(&NSString::from_str(&time_label(r.end)));
        let length = trf(Msg::TrimLength, &[&time_label(r.len()), &time_label(self.duration)]);
        self.length.setStringValue(&NSString::from_str(&length));
        let size = gif_size(self.video.0, self.video.1, self.gif_width());
        let plan = GifPlan::new(r, self.gif_fps(), size);
        let mut text = trf(
            Msg::GifEstimate,
            &[
                &lumora_core::format::file_size(plan.estimated_bytes()),
                &plan.frames(),
                &lumora_core::format::dimensions(size.0, size.1),
            ],
        );
        if plan.capped {
            text = format!("{text} · {}", trf(Msg::GifCapped, &[&plan.frames()]));
        }
        self.estimate.setStringValue(&NSString::from_str(&text));
    }
}

fn label(mtm: MainThreadMarker, text: &str, frame: Rect, secondary: bool) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(
        if secondary { typography::CAPTION } else { typography::BODY },
        weight::REGULAR,
    )));
    if secondary {
        l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l.setFrame(cg_rect(frame));
    l
}

fn button(mtm: MainThreadMarker, title: &str, target: &ActionTarget) -> Retained<NSButton> {
    // SAFETY: the target is kept alive by the window state.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.setBezelStyle(NSBezelStyle::Push);
    b.sizeToFit();
    b
}

fn popup(mtm: MainThreadMarker, items: &[String], selected: usize, frame: Rect) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), cg_rect(frame), false);
    for i in items {
        p.addItemWithTitle(&NSString::from_str(i));
    }
    p.selectItemAtIndex(selected as isize);
    p
}

fn separator(mtm: MainThreadMarker, y: f64) -> Retained<NSBox> {
    let sep = NSBox::initWithFrame(NSBox::alloc(mtm), cg_rect(Rect::new(PAD, y, W - 2.0 * PAD, 1.0)));
    sep.setBoxType(NSBoxType::Separator);
    sep
}

/// An open trim window. Dropping it closes the window.
pub struct TrimWindow {
    state: Rc<State>,
}

impl std::fmt::Debug for TrimWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrimWindow").field("id", &self.state.id).finish_non_exhaustive()
    }
}

impl TrimWindow {
    /// Open the window for a movie `duration` seconds long whose video is
    /// `video` pixels; `name` goes in the title, `poster` in the preview.
    pub fn show(
        mtm: MainThreadMarker,
        id: u64,
        name: &str,
        duration: f64,
        video: (u32, u32),
        poster: Option<CFRetained<CGImage>>,
        sink: EventSink,
    ) -> Self {
        let visible = screen_under_mouse(mtm).map(|s| s.visibleFrame()).map(crate::util::rect);
        let origin =
            visible.map_or(CGPoint::new(200.0, 200.0), |v| CGPoint::new(v.mid().x - W / 2.0, v.mid().y - H / 2.0));
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
        let window = crate::esc_window::create(mtm, CGRect::new(origin, CGSize::new(W, H)), style);
        // SAFETY: we own the window through `Retained`.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&format!("{} — {name}", tr(Msg::TrimWindowTitle))));
        let content = NSView::initWithFrame(NSView::alloc(mtm), CGRect::new(CGPoint::ZERO, CGSize::new(W, H)));

        // Bottom: export progress (hidden while idle).
        let status = label(mtm, "", Rect::new(PAD, 46.0, W - 2.0 * PAD, 16.0), true);
        let bar = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            cg_rect(Rect::new(PAD, 20.0, W - 2.0 * PAD - 110.0, 20.0)),
        );
        bar.setStyle(NSProgressIndicatorStyle::Bar);
        bar.setIndeterminate(false);
        bar.setMinValue(0.0);
        bar.setMaxValue(1.0);
        bar.setControlSize(NSControlSize::Small);

        // GIF options.
        let gif_y = 116.0;
        let widths: Vec<String> = GifWidth::ALL
            .iter()
            .map(|w| w.pixels().map_or_else(|| tr(Msg::GifWidthOriginal).to_string(), |p| format!("{p} px")))
            .collect();
        let fps: Vec<String> = GIF_FPS.iter().map(|f| trf(Msg::GifFps, &[f])).collect();
        let width_popup = popup(mtm, &widths, 1, Rect::new(PAD + LABEL_W, gif_y, 120.0, 26.0));
        let fps_popup = popup(mtm, &fps, 1, Rect::new(PAD + LABEL_W + 128.0 + 80.0, gif_y, 110.0, 26.0));
        let estimate = label(mtm, "", Rect::new(PAD + LABEL_W, gif_y - 26.0, W - 2.0 * PAD - LABEL_W, 16.0), true);

        // Trim rows.
        let slider_x = PAD + LABEL_W;
        let slider_w = W - PAD - TIME_W - 8.0 - slider_x;
        let make_slider = |y: f64, value: f64| {
            let s = NSSlider::initWithFrame(NSSlider::alloc(mtm), cg_rect(Rect::new(slider_x, y, slider_w, 24.0)));
            s.setMinValue(0.0);
            s.setMaxValue(duration.max(0.01));
            s.setDoubleValue(value);
            s.setContinuous(true);
            s
        };
        let start_y = 250.0;
        let end_y = 218.0;
        let start = make_slider(start_y, 0.0);
        let end = make_slider(end_y, duration);
        let start_label = label(mtm, "", Rect::new(W - PAD - TIME_W, start_y + 3.0, TIME_W, 18.0), false);
        let end_label = label(mtm, "", Rect::new(W - PAD - TIME_W, end_y + 3.0, TIME_W, 18.0), false);
        for l in [&start_label, &end_label] {
            l.setFont(Some(&crate::util::mono_digits(typography::BODY, weight::REGULAR)));
            l.setAlignment(objc2_app_kit::NSTextAlignment::Right);
        }
        let length = label(mtm, "", Rect::new(PAD + LABEL_W, 182.0, 260.0, 18.0), true);

        // Preview.
        let preview_y = 290.0;
        let preview = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            cg_rect(Rect::new(PAD, preview_y, W - 2.0 * PAD, H - preview_y - 16.0)),
        );
        preview.setImageScaling(NSImageScaling::ScaleProportionallyDown);

        let delegate = WindowDelegate::new(mtm);
        // Placeholder buttons are replaced below once targets exist.
        let noop = ActionTarget::new(mtm, |_| {});
        let state = Rc::new(State {
            id,
            sink,
            window: window.clone(),
            duration,
            video,
            range: Cell::new(TrimRange::full(duration)),
            preview,
            start,
            end,
            start_label,
            end_label,
            length,
            width_popup,
            fps_popup,
            estimate,
            save: button(mtm, tr(Msg::TrimSave), &noop),
            gif: button(mtm, tr(Msg::GifExport), &noop),
            status,
            bar,
            cancel: button(mtm, tr(Msg::HistoryCancel), &noop),
            _targets: RefCell::new(Vec::new()),
            _delegate: RefCell::new(None),
        });

        let mut targets = vec![noop];
        let target = |f: Box<dyn Fn(&State)>, targets: &mut Vec<Retained<ActionTarget>>| {
            let weak = Rc::downgrade(&state);
            let t = ActionTarget::new(mtm, move |_: &AnyObject| {
                if let Some(s) = weak.upgrade() {
                    f(&s);
                }
            });
            targets.push(t.clone());
            t
        };
        let wire = |control: &objc2_app_kit::NSControl, t: &ActionTarget| {
            // SAFETY: the target is kept alive in `_targets`.
            unsafe {
                control.setTarget(Some(t));
                control.setAction(Some(ActionTarget::action()));
            }
        };
        let t = target(Box::new(|s| s.slider_moved(true)), &mut targets);
        wire(&state.start, &t);
        let t = target(Box::new(|s| s.slider_moved(false)), &mut targets);
        wire(&state.end, &t);
        let t = target(Box::new(|s| s.refresh()), &mut targets);
        wire(&state.width_popup, &t);
        wire(&state.fps_popup, &t);
        let t = target(Box::new(|s| s.emit(TrimAction::Save(s.range.get()))), &mut targets);
        wire(&state.save, &t);
        let t = target(
            Box::new(|s| {
                s.emit(TrimAction::ExportGif { range: s.range.get(), width: s.gif_width(), fps: s.gif_fps() })
            }),
            &mut targets,
        );
        wire(&state.gif, &t);
        let t = target(Box::new(|s| s.emit(TrimAction::Cancel)), &mut targets);
        wire(&state.cancel, &t);
        *state._targets.borrow_mut() = targets;

        // Place the buttons (right-aligned) now that they are sized.
        let place_right = |b: &NSButton, y: f64| {
            let bw = b.frame().size.width.max(120.0);
            b.setFrame(cg_rect(Rect::new(W - PAD - bw, y, bw, 32.0)));
        };
        place_right(&state.save, 176.0);
        place_right(&state.gif, gif_y - 3.0);
        place_right(&state.cancel, 14.0);
        let cancel_w = state.cancel.frame().size.width;
        state.bar.setFrame(cg_rect(Rect::new(PAD, 20.0, W - 2.0 * PAD - cancel_w - 12.0, 20.0)));

        content.addSubview(&state.preview);
        content.addSubview(&label(mtm, tr(Msg::TrimStart), Rect::new(PAD, start_y + 3.0, LABEL_W - 8.0, 18.0), false));
        content.addSubview(&state.start);
        content.addSubview(&state.start_label);
        content.addSubview(&label(mtm, tr(Msg::TrimEnd), Rect::new(PAD, end_y + 3.0, LABEL_W - 8.0, 18.0), false));
        content.addSubview(&state.end);
        content.addSubview(&state.end_label);
        content.addSubview(&state.length);
        content.addSubview(&state.save);
        content.addSubview(&separator(mtm, 160.0));
        content.addSubview(&label(
            mtm,
            tr(Msg::GifWidthLabel),
            Rect::new(PAD, gif_y + 4.0, LABEL_W - 8.0, 18.0),
            false,
        ));
        content.addSubview(&state.width_popup);
        content.addSubview(&label(
            mtm,
            tr(Msg::GifFpsLabel),
            Rect::new(PAD + LABEL_W + 128.0, gif_y + 4.0, 76.0, 18.0),
            false,
        ));
        content.addSubview(&state.fps_popup);
        content.addSubview(&state.estimate);
        content.addSubview(&state.gif);
        content.addSubview(&separator(mtm, 76.0));
        content.addSubview(&state.status);
        content.addSubview(&state.bar);
        content.addSubview(&state.cancel);

        *delegate.ivars().state.borrow_mut() = Rc::downgrade(&state);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        *state._delegate.borrow_mut() = Some(delegate);
        window.setContentView(Some(&content));

        let this = TrimWindow { state };
        this.set_busy(None);
        if let Some(img) = poster {
            this.set_preview(&img);
        }
        this.state.refresh();
        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        this
    }

    pub fn focus(&self) {
        let mtm = self.state.window.mtm();
        NSApplication::sharedApplication(mtm).activate();
        self.state.window.makeKeyAndOrderFront(None);
    }

    /// Show `image` (a frame of the movie) in the preview.
    pub fn set_preview(&self, image: &CGImage) {
        let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
        let ns = NSImage::initWithCGImage_size(NSImage::alloc(), image, CGSize::new(w as f64, h as f64));
        self.state.preview.setImage(Some(&ns));
    }

    /// `Some(status)` shows the progress row and locks the controls while
    /// an export runs; `None` hides it again.
    pub fn set_busy(&self, status: Option<&str>) {
        let s = &self.state;
        let busy = status.is_some();
        for v in [&*s.status as &NSView, &s.bar, &s.cancel] {
            v.setHidden(!busy);
        }
        s.status.setStringValue(&NSString::from_str(status.unwrap_or("")));
        s.bar.setDoubleValue(0.0);
        let controls: [&objc2_app_kit::NSControl; 6] =
            [&s.start, &s.end, &s.width_popup, &s.fps_popup, &s.save, &s.gif];
        for c in controls {
            c.setEnabled(!busy);
        }
    }

    /// Export progress, 0…1.
    pub fn set_progress(&self, fraction: f32) {
        self.state.bar.setDoubleValue((fraction as f64).clamp(0.0, 1.0));
    }

    pub fn close(&self) {
        self.state.window.performClose(None);
    }
}

impl Drop for TrimWindow {
    fn drop(&mut self) {
        self.state.window.setDelegate(None);
        self.state.window.orderOut(None);
    }
}

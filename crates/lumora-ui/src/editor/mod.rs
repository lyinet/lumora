//! The annotation editor window.
//!
//! A native, resizable `NSWindow` with a full-size content view: a glass
//! toolbar strip under the transparent title bar and a dark canvas showing
//! the capture fit to the window (never upscaled). All interaction logic
//! lives in [`lumora_core::annotate::editor::Editor`]; this module turns
//! `NSEvent`s into editor inputs and keeps a layer tree in sync with the
//! document:
//!
//! ```text
//! canvas root (flipped)
//! ├── beautify background (canvas rect; solid / gradient / image; hidden when off)
//! ├── stage shadow (frame rect, shadowPath)
//! ├── frame (rounded, masks; = the image rect when beautify is off)
//! │   ├── chrome bar + traffic lights (beautify window frame)
//! │   └── stage  (image rect, masks to the crop)
//! │       └── content (document points, scaled by zoom, offset by the crop)
//! │           ├── base image
//! │           └── annotations — one container per annotation, z-ordered
//! └── chrome (view points): selection outline + handles, crop overlay
//! ```
//!
//! The beautify panel ([`beautify_panel`]) edits the editor's style; the
//! preview is re-laid on every change ([`lumora_core::beautify::preview_geometry`])
//! and the style goes back to the app as [`UiEvent::BeautifyChanged`].
//!
//! Exports (⌘C, ⌘S, drag-out, close-with-edits) flatten through
//! [`render::flatten`] at the capture's native scale and go to the app as
//! [`EditorAction`]s; the editor never writes the capture file itself.

mod beautify_panel;
mod render;
mod toolbar;
mod views;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use lumora_core::annotate::document::{Annotation, AnnotationId};
use lumora_core::annotate::editor::{Editor, Effect, Input, Key, Tool};
use lumora_core::annotate::render::{ops_bounds, render as render_ops};
use lumora_core::annotate::shape::Shape;
use lumora_core::beautify::{preview_geometry, Adjust, Background, BeautifyStyle, Preset, Rgba};
use lumora_core::geometry::{Point, Rect, Size};
use lumora_core::i18n::{tr, trf, Msg};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSColorPanel, NSEvent, NSEventModifierFlags,
    NSFocusRingType, NSImage, NSTextField, NSView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSString};
use objc2_quartz_core::{CAGradientLayer, CALayer, CAShapeLayer, CATransaction, CATransform3D};

use self::beautify_panel::{cg_rgba, choose_background_image, set_gradient, BeautifyPanel, PanelEvent, PanelHandler};

use self::render::{annotation_font, build_layer, flatten, LayerContext, RedactionCache};
pub use self::toolbar::ToolbarAction;
use self::toolbar::{Toolbar, ToolbarHandler};
use self::views::{CanvasView, DragTile, EditorWindowDelegate, Pointer};
use crate::events::{EditorAction, EditorImage, EventSink, UiEvent};
use crate::tokens::{color, motion, radius, size};
use crate::util::{
    cf_as_object, cg_color, cg_path, cg_rect, ease_scalar, rect, rgb8_of, spring_scalar, text_size, without_actions,
    ActionTarget,
};

/// A live annotation layer and what it was built from.
struct Cached {
    item: Annotation,
    step: Option<u32>,
    layer: Retained<CALayer>,
}

/// The inline text field while typing.
struct TextEdit {
    field: Retained<NSTextField>,
    id: Option<AnnotationId>,
    origin: Point,
    _target: Retained<ActionTarget>,
}

pub(crate) struct State {
    mtm: MainThreadMarker,
    id: u64,
    sink: EventSink,
    editor: RefCell<Editor>,
    base: CFRetained<CGImage>,
    scale: f64,
    file_name: String,
    /// The editor's beautify style: drives the live preview and ⌘C /
    /// drag-out exports (when enabled); mirrored to Settings on change.
    beautify: RefCell<BeautifyStyle>,
    panel: BeautifyPanel,
    bg: Retained<CALayer>,
    bg_gradient: Retained<CAGradientLayer>,
    frame: Retained<CALayer>,
    bar: Retained<CALayer>,
    lights: Vec<Retained<CALayer>>,
    /// Decoded wallpaper for `Background::Image` (`None` = failed to load).
    bg_image: RefCell<Option<(PathBuf, Option<CFRetained<CGImage>>)>>,
    window: Retained<NSWindow>,
    canvas: Retained<CanvasView>,
    toolbar: Toolbar,
    stage_shadow: Retained<CALayer>,
    stage: Retained<CALayer>,
    content: Retained<CALayer>,
    annotations: Retained<CALayer>,
    chrome: Retained<CALayer>,
    layers: RefCell<Vec<Cached>>,
    redactions: RefCell<RedactionCache>,
    zoom: Cell<f64>,
    stage_rect: Cell<Rect>,
    shown: Cell<Rect>,
    text: RefCell<Option<TextEdit>>,
    saved_revision: Cell<u64>,
    closed: Cell<bool>,
    _delegate: Retained<EditorWindowDelegate>,
    color_target: Retained<ActionTarget>,
}

/// An open editor. Keep it alive while the window is open; the app drops it
/// on [`EditorAction::Closed`].
pub struct EditorWindow {
    state: Rc<State>,
}

impl std::fmt::Debug for EditorWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorWindow").field("id", &self.state.id).finish_non_exhaustive()
    }
}

/// Flatten `doc` onto `base` (at `scale` pixels per point) outside an open
/// editor, e.g. for the thumbnail of a reopened `.lumora` document. Main
/// thread only.
pub fn flatten_document(
    base: &CGImage,
    scale: f64,
    doc: &lumora_core::annotate::Document,
) -> Option<CFRetained<CGImage>> {
    flatten(base, scale, doc, &mut RedactionCache::default())
}

impl EditorWindow {
    /// Adopt a beautify style changed elsewhere (Settings, another
    /// editor): updates the panel, the live preview and later exports.
    /// Emits nothing.
    pub fn set_beautify(&self, style: BeautifyStyle) {
        let s = &self.state;
        if s.closed.get() || *s.beautify.borrow() == style {
            return;
        }
        *s.beautify.borrow_mut() = style.clone();
        s.panel.sync(&style);
        s.toolbar.set_beautify_state(s.panel.is_open(), style.enabled);
        s.layout_with(true);
    }

    /// Open an editor for capture `id`. `image` is the capture at `scale`
    /// pixels per point; `file_name` titles the window. `beautify` is
    /// applied to copied and dragged exports when its `enabled` is set (the
    /// app applies it to ⌘S writes itself). `document` restores earlier
    /// annotations (a reopened `.lumora` file); its size must match the
    /// image's point size, otherwise it is ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        mtm: MainThreadMarker,
        id: u64,
        image: CFRetained<CGImage>,
        scale: f64,
        file_name: &str,
        beautify: BeautifyStyle,
        document: Option<lumora_core::annotate::Document>,
        sink: EventSink,
    ) -> Self {
        let scale = scale.max(1.0);
        let px = (CGImage::width(Some(&image)) as f64, CGImage::height(Some(&image)) as f64);
        let doc_size = Size::new(px.0 / scale, px.1 / scale);
        let document = document.filter(|d| {
            let s = d.size();
            (s.width - doc_size.width).abs() < 0.5 && (s.height - doc_size.height).abs() < 0.5
        });

        // Window: image + margins + toolbar, within 90 % of the screen.
        let visible = crate::util::screen_under_mouse(mtm)
            .map(|s| rect(s.visibleFrame()))
            .unwrap_or(Rect::new(0.0, 0.0, 1440.0, 900.0));
        let m = size::EDITOR_CANVAS_MARGIN;
        let beautify = beautify.validated();
        let shown_size =
            if beautify.enabled { lumora_core::beautify::layout(doc_size, &beautify).canvas } else { doc_size };
        let want = Size::new(shown_size.width + 2.0 * m, shown_size.height + 2.0 * m + size::EDITOR_TOOLBAR_HEIGHT);
        let w = want.width.clamp(size::EDITOR_MIN_WIDTH, visible.width() * 0.9);
        let h = want.height.clamp(size::EDITOR_MIN_HEIGHT, visible.height() * 0.9);
        let frame = Rect::new(visible.mid().x - w / 2.0, visible.mid().y - h / 2.0, w, h);

        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                cg_rect(frame),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: we own the window through `Retained`.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitle(&NSString::from_str(&trf(Msg::EditorTitle, &[&file_name])));
        window.setContentMinSize(CGSize::new(size::EDITOR_MIN_WIDTH, size::EDITOR_MIN_HEIGHT));
        window.setBackgroundColor(Some(&crate::util::ns_color(color::EDITOR_BACKDROP)));

        let content_rect = Rect::new(0.0, 0.0, w, h);
        let container = NSView::initWithFrame(NSView::alloc(mtm), cg_rect(content_rect));
        let canvas_frame = Rect::new(0.0, 0.0, w, h - size::EDITOR_TOOLBAR_HEIGHT);
        let canvas = CanvasView::new(mtm, cg_rect(canvas_frame));
        canvas.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let root = CALayer::new();
        root.setGeometryFlipped(true);
        root.setBackgroundColor(Some(&cg_color(color::EDITOR_BACKDROP)));
        canvas.setLayer(Some(&root));
        canvas.setWantsLayer(true);
        container.addSubview(&canvas);

        let bg = CALayer::new();
        bg.setMasksToBounds(true);
        bg.setHidden(true);
        // SAFETY: a valid gravity constant.
        bg.setContentsGravity(unsafe { objc2_quartz_core::kCAGravityResizeAspectFill });
        let bg_gradient = CAGradientLayer::new();
        bg.addSublayer(&bg_gradient);
        root.addSublayer(&bg);
        let stage_shadow = CALayer::new();
        stage_shadow.setShadowOpacity(0.45);
        stage_shadow.setShadowRadius(18.0);
        stage_shadow.setShadowOffset(CGSize::new(0.0, 8.0));
        root.addSublayer(&stage_shadow);
        let frame_layer = CALayer::new();
        frame_layer.setMasksToBounds(true);
        frame_layer.setCornerRadius(radius::EDITOR_IMAGE);
        frame_layer.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        root.addSublayer(&frame_layer);
        let bar = CALayer::new();
        bar.setHidden(true);
        frame_layer.addSublayer(&bar);
        let lights: Vec<Retained<CALayer>> = lumora_core::beautify::WindowChrome::LIGHT_COLORS
            .iter()
            .map(|c| {
                let l = CALayer::new();
                l.setBackgroundColor(Some(&cg_rgba(*c)));
                bar.addSublayer(&l);
                l
            })
            .collect();
        let stage = CALayer::new();
        stage.setMasksToBounds(true);
        frame_layer.addSublayer(&stage);
        let content = CALayer::new();
        content.setAnchorPoint(CGPoint::new(0.0, 0.0));
        content.setBounds(cg_rect(Rect { origin: Point::ZERO, size: doc_size }));
        stage.addSublayer(&content);
        let base_layer = CALayer::new();
        base_layer.setFrame(cg_rect(Rect { origin: Point::ZERO, size: doc_size }));
        // SAFETY: a CGImage is valid layer contents.
        unsafe { base_layer.setContents(Some(cf_as_object(&*image))) };
        base_layer.setMagnificationFilter(unsafe { objc2_quartz_core::kCAFilterNearest });
        content.addSublayer(&base_layer);
        let annotations = CALayer::new();
        annotations.setFrame(cg_rect(Rect { origin: Point::ZERO, size: doc_size }));
        content.addSublayer(&annotations);
        let chrome = CALayer::new();
        chrome.setFrame(cg_rect(canvas_frame));
        root.addSublayer(&chrome);

        let delegate = EditorWindowDelegate::new(mtm);
        let drag = DragTile::new(mtm);

        let state = Rc::new_cyclic(|weak: &Weak<State>| {
            let w = weak.clone();
            let handler: ToolbarHandler = Rc::new(move |a| {
                if let Some(s) = w.upgrade() {
                    State::toolbar_action(&s, a);
                }
            });
            let w = weak.clone();
            let panel_handler: PanelHandler = Rc::new(move |e| {
                if let Some(s) = w.upgrade() {
                    State::beautify_event(&s, e);
                }
            });
            let panel = BeautifyPanel::new(mtm, canvas_frame.width(), canvas_frame.height(), panel_handler);
            panel.sync(&beautify);
            container.addSubview(&panel.view);
            let toolbar = Toolbar::new(mtm, content_rect.width(), content_rect.height(), handler, drag.clone());
            toolbar.set_beautify_state(false, beautify.enabled);
            container.addSubview(&toolbar.view);
            let w = weak.clone();
            let color_target = ActionTarget::new(mtm, move |_| {
                let (Some(s), Some(mtm)) = (w.upgrade(), MainThreadMarker::new()) else { return };
                // The beautify colour well drives the shared panel itself.
                if s.panel.well_active() {
                    return;
                }
                if let Some(c) = rgb8_of(&NSColorPanel::sharedColorPanel(mtm).color()) {
                    let effect = s.editor.borrow_mut().set_color(c);
                    State::apply(&s, effect);
                    s.sync(None);
                }
            });
            State {
                mtm,
                id,
                sink,
                editor: RefCell::new(match document {
                    Some(d) => Editor::with_document(d),
                    None => Editor::new(doc_size),
                }),
                base: image,
                scale,
                file_name: file_name.to_string(),
                beautify: RefCell::new(beautify.clone()),
                panel,
                bg,
                bg_gradient,
                frame: frame_layer,
                bar,
                lights,
                bg_image: RefCell::new(None),
                window: window.clone(),
                canvas: canvas.clone(),
                toolbar,
                stage_shadow,
                stage,
                content,
                annotations,
                chrome,
                layers: RefCell::new(Vec::new()),
                redactions: RefCell::new(RedactionCache::default()),
                zoom: Cell::new(1.0),
                stage_rect: Cell::new(Rect::ZERO),
                shown: Cell::new(Rect::ZERO),
                text: RefCell::new(None),
                saved_revision: Cell::new(0),
                closed: Cell::new(false),
                _delegate: delegate.clone(),
                color_target,
            }
        });
        *canvas.ivars().state.borrow_mut() = Rc::downgrade(&state);
        *drag.ivars().state.borrow_mut() = Rc::downgrade(&state);
        *delegate.ivars().state.borrow_mut() = Rc::downgrade(&state);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.setContentView(Some(&container));
        state.saved_revision.set(state.editor.borrow().document().revision());

        state.layout();
        state.sync(None);
        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(&canvas));
        EditorWindow { state }
    }

    pub fn id(&self) -> u64 {
        self.state.id
    }

    /// Bring the window forward.
    pub fn focus(&self) {
        NSApplication::sharedApplication(self.state.mtm).activate();
        self.state.window.makeKeyAndOrderFront(None);
    }

    /// Close the window (emits [`EditorAction::Closed`]).
    pub fn close(&self) {
        self.state.window.close();
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        if !self.state.closed.get() {
            self.state.closed.set(true);
            self.state.window.close();
        }
        self.state.window.setDelegate(None);
    }
}

impl State {
    // -- geometry ---------------------------------------------------------------

    /// The document rect on screen: the whole canvas while cropping, else
    /// the crop.
    fn wanted_shown(&self) -> Rect {
        let ed = self.editor.borrow();
        if ed.tool() == Tool::Crop {
            ed.document().canvas()
        } else {
            ed.document().visible_rect()
        }
    }

    /// Fit the shown rect (plus the beautify canvas, when on) into the
    /// canvas area left of the panel (never upscaled) and position every
    /// preview layer. Called on resize and when the shown rect changes.
    fn layout(&self) {
        self.layout_with(false);
    }

    /// [`State::layout`], optionally with a short implicit animation (for
    /// discrete changes such as toggling the panel or the window frame).
    fn layout_with(&self, animate: bool) {
        let b = rect(self.canvas.bounds());
        let shown = self.wanted_shown();
        if shown.is_empty() {
            return;
        }
        let mut avail = b.inset(size::EDITOR_CANVAS_MARGIN, size::EDITOR_CANVAS_MARGIN);
        if self.panel.is_open() {
            avail = Rect::new(
                avail.min_x(),
                avail.min_y(),
                (avail.width() - beautify_panel::WIDTH).max(1.0),
                avail.height(),
            );
        }
        let style = self.beautify.borrow().clone();
        let g = preview_geometry(shown.size, &style, avail, radius::EDITOR_IMAGE);
        let zoom = g.zoom;
        // Snap the image to device pixels; move everything else with it.
        let backing = self.window.backingScaleFactor();
        let snapped = g.content.rounded(backing);
        let (dx, dy) = (snapped.min_x() - g.content.min_x(), snapped.min_y() - g.content.min_y());
        let stage = Rect::new(snapped.min_x(), snapped.min_y(), g.content.width(), g.content.height());
        let frame = g.frame.offset(dx, dy);
        let canvas = g.canvas.offset(dx, dy);
        let local = |r: Rect| r.offset(-frame.min_x(), -frame.min_y());
        let apply = || {
            self.bg.setHidden(!style.enabled);
            self.bg.setFrame(cg_rect(canvas));
            if style.enabled {
                self.apply_background(&style.background, canvas.size);
            }
            self.frame.setFrame(cg_rect(frame));
            self.frame.setCornerRadius(g.corner_radius);
            let fill = style.enabled.then(|| cg_rgba(style.chrome.bar_color().unwrap_or(Rgba::WHITE)));
            self.frame.setBackgroundColor(fill.as_deref());
            match (style.enabled, g.chrome_bar, style.chrome.bar_color()) {
                (true, Some(bar), Some(c)) => {
                    self.bar.setHidden(false);
                    self.bar.setFrame(cg_rect(local(bar)));
                    self.bar.setBackgroundColor(Some(&cg_rgba(c)));
                    if let Some((centres, d)) = g.traffic_lights() {
                        for (l, p) in self.lights.iter().zip(centres) {
                            let p = (p.x - bar.min_x(), p.y - bar.min_y());
                            l.setFrame(cg_rect(Rect::new(p.0 - d / 2.0, p.1 - d / 2.0, d, d)));
                            l.setCornerRadius(d / 2.0);
                        }
                    }
                }
                _ => self.bar.setHidden(true),
            }
            self.stage.setFrame(cg_rect(local(stage)));
            self.stage_shadow.setFrame(cg_rect(frame));
            self.stage_shadow.setShadowPath(Some(&crate::util::round_rect_path(
                Rect::new(0.0, 0.0, frame.width(), frame.height()),
                g.corner_radius,
            )));
            let (opacity, blur, offset) = if !style.enabled {
                (0.45, 18.0, 8.0)
            } else if style.shadow.enabled {
                (style.shadow.opacity, style.shadow.radius * zoom, style.shadow.offset_y * zoom)
            } else {
                (0.0, 0.0, 0.0)
            };
            self.stage_shadow.setShadowOpacity(opacity as f32);
            self.stage_shadow.setShadowRadius(blur);
            self.stage_shadow.setShadowOffset(CGSize::new(0.0, offset));
            self.content.setTransform(CATransform3D::new_scale(zoom, zoom, 1.0));
            self.content.setPosition(CGPoint::new(-shown.min_x() * zoom, -shown.min_y() * zoom));
            self.chrome.setFrame(cg_rect(b));
        };
        if animate {
            CATransaction::begin();
            CATransaction::setAnimationDuration(0.22);
            apply();
            CATransaction::commit();
        } else {
            without_actions(apply);
        }
        self.zoom.set(zoom);
        self.stage_rect.set(stage);
        self.shown.set(shown);
        self.editor.borrow_mut().set_zoom(zoom);
        self.draw_chrome();
        self.position_text_field();
    }

    /// Fill the background layer for `bg` over a canvas of `size`.
    fn apply_background(&self, bg: &Background, size: Size) {
        self.bg_gradient.setFrame(cg_rect(Rect { origin: Point::ZERO, size }));
        let image = match bg {
            Background::Image(path) => self.background_image(path),
            _ => None,
        };
        let gradient = |from, to, angle| {
            set_gradient(&self.bg_gradient, from, to, angle, size);
            self.bg_gradient.setHidden(false);
            self.bg.setBackgroundColor(None);
        };
        // SAFETY: clearing or setting a CGImage as contents is valid.
        unsafe { self.bg.setContents(image.as_deref().map(cf_as_object)) };
        match bg {
            Background::None => {
                self.bg_gradient.setHidden(true);
                self.bg.setBackgroundColor(None);
            }
            Background::Solid(c) => {
                self.bg_gradient.setHidden(true);
                self.bg.setBackgroundColor(Some(&cg_rgba(*c)));
            }
            Background::LinearGradient { from, to, angle_deg } => gradient(*from, *to, *angle_deg),
            Background::Image(_) if image.is_some() => {
                self.bg_gradient.setHidden(true);
                self.bg.setBackgroundColor(None);
            }
            Background::Image(_) => {
                // Same fallback as the renderer.
                let (from, to, angle) = Preset::DEFAULT.stops();
                gradient(from, to, angle);
            }
        }
    }

    /// The decoded wallpaper at `path`, cached per path.
    fn background_image(&self, path: &std::path::Path) -> Option<CFRetained<CGImage>> {
        let mut cache = self.bg_image.borrow_mut();
        match cache.as_ref() {
            Some((p, img)) if p == path => img.clone(),
            _ => {
                let img = lumora_capture::beautify_render::load_image(path);
                *cache = Some((path.to_path_buf(), img.clone()));
                img
            }
        }
    }

    fn to_doc(&self, p: (f64, f64)) -> Point {
        let (s, z, shown) = (self.stage_rect.get(), self.zoom.get(), self.shown.get());
        Point::new((p.0 - s.min_x()) / z + shown.min_x(), (p.1 - s.min_y()) / z + shown.min_y())
    }

    fn to_view(&self, p: Point) -> Point {
        let (s, z, shown) = (self.stage_rect.get(), self.zoom.get(), self.shown.get());
        Point::new(s.min_x() + (p.x - shown.min_x()) * z, s.min_y() + (p.y - shown.min_y()) * z)
    }

    fn rect_to_view(&self, r: Rect) -> Rect {
        let a = self.to_view(r.origin);
        let z = self.zoom.get();
        Rect::new(a.x, a.y, r.width() * z, r.height() * z)
    }

    // -- input --------------------------------------------------------------------

    fn pointer(this: &Rc<State>, phase: Pointer, p: (f64, f64), clicks: u32, shift: bool) {
        if phase == Pointer::Down && this.text.borrow().is_some() {
            State::end_text_edit(this);
            return;
        }
        let at = this.to_doc(p);
        let input = match phase {
            Pointer::Down => Input::Down { at, clicks, shift },
            Pointer::Drag => Input::Drag { at, shift },
            Pointer::Up => Input::Up { at, shift },
        };
        let effect = this.editor.borrow_mut().handle(input);
        State::apply(this, effect);
    }

    fn key_down(this: &Rc<State>, event: &NSEvent) {
        let flags = event.modifierFlags();
        let shift = flags.contains(NSEventModifierFlags::Shift);
        let key = match event.keyCode() {
            51 | 117 => Some(Key::Delete),
            53 => Some(Key::Escape),
            36 | 76 => Some(Key::Enter),
            123 => Some(Key::Nudge(-1, 0)),
            124 => Some(Key::Nudge(1, 0)),
            125 => Some(Key::Nudge(0, 1)),
            126 => Some(Key::Nudge(0, -1)),
            _ => None,
        };
        if let Some(key) = key {
            let effect = this.editor.borrow_mut().handle(Input::Key { key, shift });
            return State::apply(this, effect);
        }
        let plain = !flags
            .intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Control | NSEventModifierFlags::Option);
        let Some(c) = event.charactersIgnoringModifiers().and_then(|s| s.to_string().chars().next()) else { return };
        if !plain {
            return;
        }
        if let Some(i) = c.to_digit(10).filter(|d| (1..=3).contains(d)) {
            State::toolbar_action(this, ToolbarAction::Width(lumora_core::annotate::StrokeWidth::ALL[i as usize - 1]));
        } else if let Some(t) = Tool::from_key(c) {
            State::toolbar_action(this, ToolbarAction::Tool(t));
        }
    }

    fn key_equivalent(this: &Rc<State>, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        if !flags.contains(NSEventModifierFlags::Command) {
            return false;
        }
        let shift = flags.contains(NSEventModifierFlags::Shift);
        let Some(c) = event.charactersIgnoringModifiers().map(|s| s.to_string().to_lowercase()) else { return false };
        if this.text.borrow().is_some() {
            // No Edit menu in an accessory app: route text editing commands
            // to the field editor ourselves.
            let action = match c.as_str() {
                "c" => Some(sel!(copy:)),
                "v" => Some(sel!(paste:)),
                "x" => Some(sel!(cut:)),
                "a" => Some(sel!(selectAll:)),
                "z" if shift => Some(sel!(redo:)),
                "z" => Some(sel!(undo:)),
                _ => None,
            };
            if let Some(a) = action {
                // SAFETY: standard responder-chain actions, nil target.
                return unsafe { NSApplication::sharedApplication(this.mtm).sendAction_to_from(a, None, None) };
            }
            return false;
        }
        match c.as_str() {
            "z" => State::toolbar_action(this, if shift { ToolbarAction::Redo } else { ToolbarAction::Undo }),
            "c" => State::toolbar_action(this, ToolbarAction::Copy),
            "s" => State::toolbar_action(this, ToolbarAction::Save),
            "w" => this.window.performClose(None),
            "]" => {
                let e = this.editor.borrow_mut().bring_to_front();
                State::apply(this, e);
            }
            "[" => {
                let e = this.editor.borrow_mut().send_to_back();
                State::apply(this, e);
            }
            _ => return false,
        }
        true
    }

    fn toolbar_action(this: &Rc<State>, action: ToolbarAction) {
        if this.closed.get() {
            return;
        }
        if !matches!(action, ToolbarAction::Copy | ToolbarAction::Save) {
            State::end_text_edit(this);
        }
        let effect = {
            let mut ed = this.editor.borrow_mut();
            match action {
                ToolbarAction::Tool(t) => ed.set_tool(t),
                ToolbarAction::Color(c) => ed.set_color(c),
                ToolbarAction::Width(w) => ed.set_width(w),
                ToolbarAction::Toggle(on) => {
                    let mut o = ed.options();
                    match ed.tool() {
                        Tool::Arrow => o.curved_arrow = on,
                        Tool::Rect => o.rounded_rect = on,
                        Tool::Text => o.text_background = on,
                        _ => {}
                    }
                    ed.set_options(o)
                }
                ToolbarAction::Redact(m) => {
                    let mut o = ed.options();
                    o.redact = m;
                    ed.set_options(o)
                }
                ToolbarAction::ApplyCrop => ed.apply_crop(),
                ToolbarAction::ResetCrop => ed.reset_crop(),
                ToolbarAction::Undo => ed.undo(),
                ToolbarAction::Redo => ed.redo(),
                ToolbarAction::CustomColor => {
                    drop(ed);
                    this.open_color_panel();
                    Effect::None
                }
                ToolbarAction::Copy => {
                    drop(ed);
                    this.copy();
                    Effect::None
                }
                ToolbarAction::Save => {
                    drop(ed);
                    this.save();
                    Effect::None
                }
                ToolbarAction::Beautify => {
                    drop(ed);
                    State::toggle_panel(this);
                    Effect::None
                }
            }
        };
        State::apply(this, effect);
        // Tool/option changes with nothing selected still need the toolbar.
        this.sync(None);
    }

    // -- beautify -------------------------------------------------------------

    fn toggle_panel(this: &Rc<State>) {
        let open = !this.panel.is_open();
        this.panel.set_open(open);
        this.toolbar.set_beautify_state(open, this.beautify.borrow().enabled);
        this.layout_with(true);
    }

    /// A panel edit: fold it into the style, re-lay the preview and tell
    /// the app (which saves it as "remember last").
    fn beautify_event(this: &Rc<State>, event: PanelEvent) {
        if this.closed.get() {
            return;
        }
        let adjust = match event {
            PanelEvent::Adjust(a) => a,
            PanelEvent::ChooseImage => match choose_background_image(this.mtm) {
                Some(path) => Adjust::Background(Background::Image(path)),
                None => {
                    this.panel.sync(&this.beautify.borrow());
                    return;
                }
            },
        };
        // Continuous controls track the pointer; discrete ones animate.
        let animate =
            matches!(adjust, Adjust::Enabled(_) | Adjust::Chrome(_) | Adjust::Aspect(_) | Adjust::AutoBalance(_));
        let changed = this.beautify.borrow_mut().apply(adjust);
        let style = this.beautify.borrow().clone();
        this.panel.sync(&style);
        if !changed {
            return;
        }
        this.toolbar.set_beautify_state(this.panel.is_open(), style.enabled);
        this.layout_with(animate);
        (this.sink)(UiEvent::BeautifyChanged { from: this.id, style: Box::new(style) });
    }

    fn apply(this: &Rc<State>, effect: Effect) {
        match effect {
            Effect::None => {}
            Effect::Redraw => this.sync(None),
            Effect::Inserted(id) => this.sync(Some(id)),
            Effect::EditText { id, origin } => {
                this.sync(None);
                State::begin_text_edit(this, id, origin);
            }
            Effect::Close => this.window.performClose(None),
        }
    }

    // -- sync -----------------------------------------------------------------------

    /// Bring layers, chrome and toolbar in line with the editor. `inserted`
    /// plays the insertion spring on that annotation.
    fn sync(&self, inserted: Option<AnnotationId>) {
        if self.wanted_shown() != self.shown.get() {
            self.layout();
        }
        let editing = self.text.borrow().as_ref().and_then(|t| t.id);
        {
            let ed = self.editor.borrow();
            let doc = ed.document();
            let canvas = doc.canvas();
            let mut cache = self.layers.borrow_mut();
            let mut redactions = self.redactions.borrow_mut();
            let text_scale = self.window.backingScaleFactor();
            let mut fresh: Vec<Cached> = Vec::with_capacity(doc.items().len());
            without_actions(|| {
                for item in doc.items() {
                    let step = doc.step_number(item.id);
                    if let Some(pos) = cache.iter().position(|c| c.item.id == item.id) {
                        let c = cache.swap_remove(pos);
                        if c.item == *item && c.step == step {
                            fresh.push(c);
                            continue;
                        }
                    }
                    let ops = render_ops(item, canvas, step);
                    let mut ctx = LayerContext {
                        base: &self.base,
                        scale: self.scale,
                        canvas,
                        text_scale,
                        redactions: &mut redactions,
                    };
                    let layer = build_layer(&ops, &mut ctx);
                    fresh.push(Cached { item: item.clone(), step, layer });
                }
                for c in &fresh {
                    c.layer.setHidden(Some(c.item.id) == editing);
                }
                let layers: Vec<Retained<CALayer>> = fresh.iter().map(|c| c.layer.clone()).collect();
                // SAFETY: an array of CALayers is a valid `sublayers` value.
                unsafe { self.annotations.setSublayers(Some(&NSArray::from_retained_slice(&layers))) };
            });
            cache.clear();
            *cache = fresh;
            if let Some(id) = inserted {
                if let Some(c) = cache.iter().find(|c| c.item.id == id) {
                    let ops = render_ops(&c.item, canvas, c.step);
                    if let Some(b) = ops_bounds(&ops) {
                        pop_in(&c.layer, b, canvas);
                    }
                }
            }
            let hist = doc.history();
            self.toolbar.sync(
                ed.tool(),
                ed.style(),
                ed.options(),
                hist.can_undo(),
                hist.can_redo(),
                ed.selected().is_some(),
            );
        }
        self.draw_chrome();
    }

    /// Selection outline + handles, or the crop overlay, in view points.
    fn draw_chrome(&self) {
        let ed = self.editor.borrow();
        let mut parts: Vec<Retained<CALayer>> = Vec::new();
        if let Some(crop) = ed.pending_crop() {
            let stage = self.stage_rect.get();
            let c = self.rect_to_view(crop);
            let dim = CAShapeLayer::new();
            let mut path = lumora_core::annotate::path::Path::rect(stage);
            path.extend(&lumora_core::annotate::path::Path::rect(c));
            dim.setPath(Some(&cg_path(&path)));
            dim.setFillColor(Some(&cg_color(color::CROP_DIM)));
            unsafe { dim.setFillRule(objc2_quartz_core::kCAFillRuleEvenOdd) };
            parts.push(Retained::into_super(dim));
            let mut guides = lumora_core::annotate::path::Path::rect(c);
            for i in 1..3 {
                let f = f64::from(i) / 3.0;
                guides.move_to(Point::new(c.min_x() + c.width() * f, c.min_y()));
                guides.line_to(Point::new(c.min_x() + c.width() * f, c.max_y()));
                guides.move_to(Point::new(c.min_x(), c.min_y() + c.height() * f));
                guides.line_to(Point::new(c.max_x(), c.min_y() + c.height() * f));
            }
            let outline = outline_layer(&guides, color::SELECTION_STROKE.with_alpha(0.8), 1.0, false);
            parts.push(Retained::into_super(outline));
            for h in lumora_core::geometry::Handle::ALL {
                parts.push(handle_layer(c.handle_point(h)));
            }
        } else if let Some(a) = ed.selected().and_then(|id| ed.document().get(id)) {
            let b = self.rect_to_view(a.shape.bounds(&a.style)).inset(-4.0, -4.0);
            let linear = a.shape.is_linear();
            if !linear {
                parts.push(Retained::into_super(outline_layer(
                    &lumora_core::annotate::path::Path::rect(b),
                    color::ACCENT,
                    1.0,
                    true,
                )));
            }
            for (_, p) in a.shape.handles(&a.style) {
                parts.push(handle_layer(self.to_view(p)));
            }
        }
        without_actions(|| {
            // SAFETY: an array of CALayers is a valid `sublayers` value.
            unsafe { self.chrome.setSublayers(Some(&NSArray::from_retained_slice(&parts))) };
        });
    }

    // -- text -------------------------------------------------------------------------

    fn begin_text_edit(this: &Rc<State>, id: Option<AnnotationId>, origin: Point) {
        State::end_text_edit(this);
        let (text, style) = {
            let ed = this.editor.borrow();
            match id.and_then(|i| ed.document().get(i)) {
                Some(Annotation { shape: Shape::Text { text, .. }, style, .. }) => (text.clone(), *style),
                _ => (String::new(), ed.style()),
            }
        };
        let font_size = style.width.font_size();
        let field = NSTextField::textFieldWithString(&NSString::from_str(&text), this.mtm);
        field.setFont(Some(&annotation_font(font_size * this.zoom.get())));
        field.setTextColor(Some(&crate::util::ns_rgb8(style.color)));
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        field.setPlaceholderString(Some(&NSString::from_str(
            tr(Msg::ToolText).trim_end_matches(" (T)").trim_end_matches("（T）"),
        )));
        if let Some(cell) = field.cell() {
            cell.setSendsActionOnEndEditing(true);
            cell.setScrollable(true);
            cell.setWraps(false);
        }
        let weak = Rc::downgrade(this);
        let target = ActionTarget::new(this.mtm, move |_| {
            if let Some(s) = weak.upgrade() {
                State::end_text_edit(&s);
            }
        });
        // SAFETY: the target is kept alive in `TextEdit`.
        unsafe {
            field.setTarget(Some(&target));
            field.setAction(Some(ActionTarget::action()));
        }
        this.canvas.addSubview(&field);
        *this.text.borrow_mut() = Some(TextEdit { field: field.clone(), id, origin, _target: target });
        this.position_text_field();
        this.window.makeFirstResponder(Some(&field));
        this.sync(None);
    }

    fn position_text_field(&self) {
        let text = self.text.borrow();
        let Some(t) = text.as_ref() else { return };
        let ed = self.editor.borrow();
        let fs = match t.id.and_then(|i| ed.document().get(i)) {
            Some(a) => a.style.width.font_size(),
            None => ed.style().width.font_size(),
        };
        let z = self.zoom.get();
        t.field.setFont(Some(&annotation_font(fs * z)));
        let p = self.to_view(t.origin);
        let b = rect(self.canvas.bounds());
        let (_, line) = text_size("Hg", &annotation_font(fs * z));
        let w = (b.max_x() - p.x - 8.0).max(120.0);
        // Field editors draw with a ~2 pt inset; nudge so text stays put.
        t.field.setFrame(cg_rect(Rect::new(p.x - 2.0, p.y, w, line + 4.0)));
    }

    /// Commit the inline text (if any) to the document.
    fn end_text_edit(this: &Rc<State>) {
        let Some(t) = this.text.borrow_mut().take() else { return };
        let text = t.field.stringValue().to_string();
        t.field.removeFromSuperview();
        let fs = {
            let ed = this.editor.borrow();
            match t.id.and_then(|i| ed.document().get(i)) {
                Some(a) => a.style.width.font_size(),
                None => ed.style().width.font_size(),
            }
        };
        let (w, h) = text_size(&text, &annotation_font(fs));
        let effect = this.editor.borrow_mut().commit_text(t.id, t.origin, &text, Size::new(w, h));
        this.window.makeFirstResponder(Some(&this.canvas));
        State::apply(this, effect);
        this.sync(None);
    }

    // -- export ---------------------------------------------------------------------------

    fn flatten(&self) -> Option<CFRetained<CGImage>> {
        let ed = self.editor.borrow();
        flatten(&self.base, self.scale, ed.document(), &mut self.redactions.borrow_mut())
    }

    /// The flattened image plus the state it was made from.
    fn editor_image(&self) -> Option<EditorImage> {
        let image = self.flatten()?;
        Some(EditorImage {
            image,
            scale: self.scale,
            document: self.editor.borrow().document().clone(),
            beautify: self.beautify.borrow().clone(),
        })
    }

    /// The flattened image with the beautify style applied (if enabled).
    fn export_image(&self) -> Option<CFRetained<CGImage>> {
        let image = self.flatten()?;
        let style = self.beautify.borrow().clone();
        Some(lumora_capture::beautify_render::beautify_for_export(image, &style, self.scale))
    }

    fn copy(&self) {
        let Some(img) = self.editor_image() else { return };
        let out = lumora_capture::beautify_render::beautify_for_export(img.image.clone(), &img.beautify, self.scale);
        lumora_capture::clipboard::copy_image(&out, self.scale, None);
        (self.sink)(UiEvent::Editor { id: self.id, action: EditorAction::Copied(img) });
    }

    fn save(&self) {
        let Some(img) = self.editor_image() else { return };
        self.saved_revision.set(self.editor.borrow().document().revision());
        (self.sink)(UiEvent::Editor { id: self.id, action: EditorAction::Save(img) });
    }

    /// Flatten to a temp PNG for drag-and-drop.
    fn export_for_drag(&self) -> Option<(PathBuf, Retained<NSImage>)> {
        let image = self.export_image()?;
        let stem = std::path::Path::new(&self.file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Lumora".into());
        let path = std::env::temp_dir().join("Lumora").join(format!("{stem} (annotated).png"));
        lumora_capture::image::write_png(&image, &path, self.scale).ok()?;
        let (pw, ph) = (CGImage::width(Some(&image)) as f64, CGImage::height(Some(&image)) as f64);
        let ns = NSImage::initWithCGImage_size(NSImage::alloc(), &image, CGSize::new(pw / self.scale, ph / self.scale));
        Some((path, ns))
    }

    fn open_color_panel(&self) {
        self.panel.deactivate_well();
        let panel = NSColorPanel::sharedColorPanel(self.mtm);
        panel.setShowsAlpha(false);
        panel.setColor(&crate::util::ns_rgb8(self.editor.borrow().style().color));
        // SAFETY: the target lives as long as this editor; cleared on close.
        unsafe {
            panel.setTarget(Some(&self.color_target as &AnyObject));
            panel.setAction(Some(ActionTarget::action()));
        }
        panel.setContinuous(true);
        panel.orderFront(None);
    }

    fn window_closed(self: &Rc<Self>) {
        if self.closed.replace(true) {
            return;
        }
        State::end_text_edit(self);
        self.panel.deactivate_well();
        if NSColorPanel::sharedColorPanelExists(self.mtm) {
            let panel = NSColorPanel::sharedColorPanel(self.mtm);
            // SAFETY: clearing the target is always valid.
            unsafe { panel.setTarget(None) };
        }
        let dirty = self.editor.borrow().document().revision() != self.saved_revision.get();
        let image = if dirty { self.editor_image() } else { None };
        (self.sink)(UiEvent::Editor { id: self.id, action: EditorAction::Closed(image) });
    }
}

/// A new annotation settles in: scale 0.85 → 1 on [`motion::INSERT`] about
/// its own centre, fading in.
fn pop_in(layer: &CALayer, bounds: Rect, canvas: Rect) {
    if canvas.width() <= 0.0 || canvas.height() <= 0.0 {
        return;
    }
    let mid = bounds.mid();
    without_actions(|| {
        layer.setAnchorPoint(CGPoint::new(mid.x / canvas.width(), mid.y / canvas.height()));
        layer.setPosition(CGPoint::new(mid.x, mid.y));
    });
    spring_scalar(layer, "transform.scale", 0.85, 1.0, motion::INSERT);
    ease_scalar(layer, "opacity", 0.0, 1.0, 0.12);
}

fn outline_layer(
    path: &lumora_core::annotate::path::Path,
    c: crate::tokens::Rgba,
    width: f64,
    dashed: bool,
) -> Retained<CAShapeLayer> {
    let l = CAShapeLayer::new();
    l.setPath(Some(&cg_path(path)));
    l.setFillColor(None);
    l.setStrokeColor(Some(&cg_color(c)));
    l.setLineWidth(width);
    if dashed {
        let dash: Vec<Retained<objc2_foundation::NSNumber>> =
            [4.0, 3.0].iter().map(|v| objc2_foundation::NSNumber::numberWithDouble(*v)).collect();
        l.setLineDashPattern(Some(&NSArray::from_retained_slice(&dash)));
    }
    l
}

fn handle_layer(p: Point) -> Retained<CALayer> {
    let d = size::EDITOR_HANDLE;
    let l = CALayer::new();
    l.setFrame(cg_rect(Rect::new(p.x - d / 2.0, p.y - d / 2.0, d, d)));
    l.setCornerRadius(d / 2.0);
    l.setBackgroundColor(Some(&cg_color(color::HANDLE_FILL)));
    l.setBorderColor(Some(&cg_color(color::HANDLE_STROKE)));
    l.setBorderWidth(1.5);
    l.setShadowOpacity(0.25);
    l.setShadowRadius(1.5);
    l.setShadowOffset(CGSize::new(0.0, 1.0));
    l
}

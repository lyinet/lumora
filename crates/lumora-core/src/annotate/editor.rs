//! The editor's interaction model as a pure state machine, in the same
//! spirit as [`crate::selection`]: the canvas view converts `NSEvent`s into
//! [`Input`]s in document points and reacts to the returned [`Effect`].
//!
//! ```text
//!           down on selected handle ──▶ Resizing ──┐
//!  Idle ──┤ down on an annotation ────▶ Moving ────┼─ up ─▶ Idle (one undo step)
//!           down elsewhere (draw tool) ▶ Creating ─┘
//!           down (crop tool) ──────────▶ Cropping ──── Enter applies the crop
//! ```
//!
//! Selection works with every tool: grabbing the selected annotation's
//! body or handles always edits it, so you never have to switch to the
//! pointer to nudge the arrow you just drew.

use crate::geometry::{Handle, Point, Rect, Size};

use super::document::{Annotation, AnnotationId, Command, Document};
use super::path::{push_sample, simplify, snap_angle};
use super::shape::{resize_rect, EditHandle, RedactMode, Shape};
use super::style::{StrokeWidth, Style};
use crate::color::Rgb8;
use crate::selection::constrain_aspect;

/// Editor tools, in toolbar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tool {
    Select,
    #[default]
    Arrow,
    Rect,
    Ellipse,
    Line,
    Pen,
    Highlighter,
    Text,
    Step,
    Redact,
    Spotlight,
    Crop,
}

impl Tool {
    pub const ALL: [Tool; 12] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Line,
        Tool::Pen,
        Tool::Highlighter,
        Tool::Text,
        Tool::Step,
        Tool::Redact,
        Tool::Spotlight,
        Tool::Crop,
    ];

    /// SF Symbol shown in the toolbar.
    pub fn symbol(self) -> &'static str {
        match self {
            Tool::Select => "cursorarrow",
            Tool::Arrow => "arrow.up.right",
            Tool::Rect => "rectangle",
            Tool::Ellipse => "circle",
            Tool::Line => "line.diagonal",
            Tool::Pen => "scribble",
            Tool::Highlighter => "highlighter",
            Tool::Text => "textformat",
            Tool::Step => "1.circle",
            Tool::Redact => "square.grid.3x3.middle.filled",
            Tool::Spotlight => "light.max",
            Tool::Crop => "crop",
        }
    }

    /// Single-key shortcut (no modifiers).
    pub fn key(self) -> char {
        match self {
            Tool::Select => 'v',
            Tool::Arrow => 'a',
            Tool::Rect => 'r',
            Tool::Ellipse => 'o',
            Tool::Line => 'l',
            Tool::Pen => 'p',
            Tool::Highlighter => 'h',
            Tool::Text => 't',
            Tool::Step => 'n',
            Tool::Redact => 'b',
            Tool::Spotlight => 's',
            Tool::Crop => 'c',
        }
    }

    pub fn from_key(c: char) -> Option<Tool> {
        let c = c.to_ascii_lowercase();
        Tool::ALL.iter().copied().find(|t| t.key() == c)
    }

    /// Whether colour applies (redaction, spotlight and crop ignore it).
    pub fn uses_color(self) -> bool {
        !matches!(self, Tool::Redact | Tool::Spotlight | Tool::Crop | Tool::Select)
    }
}

/// Per-tool toggles from the toolbar's options area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolOptions {
    pub curved_arrow: bool,
    pub rounded_rect: bool,
    pub text_background: bool,
    pub redact: RedactMode,
}

/// Keys the editor handles itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Delete,
    Escape,
    Enter,
    /// Arrow keys: nudge by (dx, dy) points.
    Nudge(i8, i8),
}

/// Pointer and key input, in document points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    Down { at: Point, clicks: u32, shift: bool },
    Drag { at: Point, shift: bool },
    Up { at: Point, shift: bool },
    Key { key: Key, shift: bool },
}

/// What the view should do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    /// Something changed; re-sync layers and chrome.
    Redraw,
    /// An annotation was just committed (play the insertion animation).
    Inserted(AnnotationId),
    /// Show the inline text field: editing `id`, or a new text at `origin`.
    EditText {
        id: Option<AnnotationId>,
        origin: Point,
    },
    /// Escape with nothing to cancel: close the editor.
    Close,
}

#[derive(Debug, Clone, PartialEq)]
enum Gesture {
    Idle,
    /// Drawing a new annotation (inserted untracked).
    Creating {
        id: AnnotationId,
        anchor: Point,
    },
    Moving {
        before: Annotation,
        grab: Point,
        moved: bool,
    },
    Resizing {
        before: Annotation,
        handle: EditHandle,
    },
    /// Drawing / adjusting the pending crop.
    CropDrawing {
        anchor: Point,
    },
    CropMoving {
        grab: Point,
        start: Rect,
    },
    CropResizing {
        handle: Handle,
        start: Rect,
    },
}

/// The annotation editor's state.
#[derive(Debug, Clone)]
pub struct Editor {
    doc: Document,
    tool: Tool,
    style: Style,
    options: ToolOptions,
    selected: Option<AnnotationId>,
    gesture: Gesture,
    pending_crop: Option<Rect>,
    /// Hit slop and handle radius in document points (screen size / zoom).
    slop: f64,
    handle_radius: f64,
}

/// Minimum distance between freehand samples, in document points.
const PEN_MIN_GAP: f64 = 1.2;
/// Simplification tolerance applied when a pen stroke ends.
const PEN_SIMPLIFY: f64 = 0.35;

impl Editor {
    /// A new editor for an image of `size` points.
    pub fn new(size: Size) -> Self {
        Editor {
            doc: Document::new(size),
            tool: Tool::default(),
            style: Style::default(),
            options: ToolOptions::default(),
            selected: None,
            gesture: Gesture::Idle,
            pending_crop: None,
            slop: 4.0,
            handle_radius: 7.0,
        }
    }

    /// An editor over an existing document (a reopened `.lumora` file).
    /// The document's history is kept as given — [`Document::from_parts`]
    /// starts it empty — and step numbering continues from its badges.
    pub fn with_document(doc: Document) -> Self {
        let mut e = Editor::new(doc.size());
        e.doc = doc;
        e
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }
    pub fn tool(&self) -> Tool {
        self.tool
    }
    pub fn style(&self) -> Style {
        self.style
    }
    pub fn options(&self) -> ToolOptions {
        self.options
    }
    pub fn selected(&self) -> Option<AnnotationId> {
        self.selected
    }
    /// The crop being adjusted with the crop tool (not yet applied).
    pub fn pending_crop(&self) -> Option<Rect> {
        self.pending_crop
    }
    pub fn is_interacting(&self) -> bool {
        self.gesture != Gesture::Idle
    }

    /// Tell the editor the current zoom so hit targets stay a constant
    /// size on screen.
    pub fn set_zoom(&mut self, zoom: f64) {
        let z = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        self.slop = 4.0 / z;
        self.handle_radius = 7.0 / z;
    }

    /// Switch tools. Leaving the crop tool discards an unapplied crop;
    /// entering it starts from the current crop.
    pub fn set_tool(&mut self, tool: Tool) -> Effect {
        if tool == self.tool {
            return Effect::None;
        }
        self.finish_gesture();
        self.tool = tool;
        self.pending_crop = if tool == Tool::Crop { Some(self.doc.visible_rect()) } else { None };
        if tool == Tool::Crop {
            self.selected = None;
        }
        Effect::Redraw
    }

    /// Change the colour; also recolours the selection (one undo step).
    pub fn set_color(&mut self, color: Rgb8) -> Effect {
        self.style.color = color;
        self.restyle(|a| a.style.color = color)
    }

    /// Change the stroke width; also applies to the selection.
    pub fn set_width(&mut self, width: StrokeWidth) -> Effect {
        self.style.width = width;
        self.restyle(|a| a.style.width = width)
    }

    /// Change the opacity (percent, clamped to 10–100); also applies to the
    /// selection as one undo step.
    pub fn set_opacity(&mut self, percent: u8) -> Effect {
        let o = super::style::clamp_opacity(percent);
        self.style.opacity = o;
        self.restyle(|a| a.style.opacity = o)
    }

    /// Change tool options; applies to a selected annotation of that kind.
    pub fn set_options(&mut self, options: ToolOptions) -> Effect {
        self.options = options;
        self.restyle(|a| {
            a.shape = match &a.shape {
                s @ Shape::Arrow { .. } => s.with_curve(options.curved_arrow),
                Shape::Rect { rect, .. } => Shape::Rect { rect: *rect, rounded: options.rounded_rect },
                Shape::Text { origin, text, size, .. } => Shape::Text {
                    origin: *origin,
                    text: text.clone(),
                    size: *size,
                    background: options.text_background,
                },
                Shape::Redact { rect, .. } => Shape::Redact { rect: *rect, mode: options.redact },
                other => other.clone(),
            }
        })
    }

    fn restyle(&mut self, f: impl FnOnce(&mut Annotation)) -> Effect {
        match self.selected {
            Some(id) if self.doc.update(id, f) => Effect::Redraw,
            _ => Effect::None,
        }
    }

    /// Select an annotation (or none) programmatically.
    pub fn select(&mut self, id: Option<AnnotationId>) -> Effect {
        let id = id.filter(|i| self.doc.get(*i).is_some());
        if self.selected == id {
            return Effect::None;
        }
        self.selected = id;
        if let Some(a) = id.and_then(|i| self.doc.get(i)) {
            // Mirror the selection's look in the toolbar.
            self.style = a.style;
        }
        Effect::Redraw
    }

    pub fn undo(&mut self) -> Effect {
        self.finish_gesture();
        if self.doc.undo() {
            self.drop_stale_selection();
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    pub fn redo(&mut self) -> Effect {
        self.finish_gesture();
        if self.doc.redo() {
            self.drop_stale_selection();
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    fn drop_stale_selection(&mut self) {
        if self.selected.is_some_and(|id| self.doc.get(id).is_none()) {
            self.selected = None;
        }
        if self.tool == Tool::Crop {
            self.pending_crop = Some(self.doc.visible_rect());
        }
    }

    /// Smart redaction: cover each of `rects` (document points) with a
    /// redaction in `mode`, all as one undo step. Returns how many were
    /// added (with [`Effect::Redraw`] when any were).
    pub fn add_redactions(&mut self, rects: &[Rect], mode: RedactMode) -> (usize, Effect) {
        self.finish_gesture();
        let canvas = self.doc.canvas();
        let shapes: Vec<Shape> = rects
            .iter()
            .filter_map(|r| r.normalized().intersection(&canvas))
            .filter(|r| r.width() >= 1.0 && r.height() >= 1.0)
            .map(|rect| Shape::Redact { rect, mode })
            .collect();
        let ids = self.doc.add_many(shapes, self.style);
        if ids.is_empty() {
            (0, Effect::None)
        } else {
            self.selected = None;
            (ids.len(), Effect::Redraw)
        }
    }

    pub fn delete_selected(&mut self) -> Effect {
        match self.selected.take() {
            Some(id) if self.doc.remove(id) => Effect::Redraw,
            _ => Effect::None,
        }
    }

    pub fn bring_to_front(&mut self) -> Effect {
        match self.selected {
            Some(id) if self.doc.bring_to_front(id) => Effect::Redraw,
            _ => Effect::None,
        }
    }

    pub fn send_to_back(&mut self) -> Effect {
        match self.selected {
            Some(id) if self.doc.send_to_back(id) => Effect::Redraw,
            _ => Effect::None,
        }
    }

    /// Finish inline text editing. `size` is the measured text size. Empty
    /// text deletes an existing annotation (or creates nothing).
    pub fn commit_text(&mut self, id: Option<AnnotationId>, origin: Point, text: &str, size: Size) -> Effect {
        let text = text.trim_end_matches('\n').to_string();
        let empty = text.trim().is_empty();
        match id.and_then(|i| self.doc.get(i).cloned()) {
            Some(before) => {
                if empty {
                    self.selected = None;
                    self.doc.remove(before.id);
                    return Effect::Redraw;
                }
                let background = matches!(before.shape, Shape::Text { background: true, .. });
                let shape = Shape::Text { origin: text_origin(&before.shape, origin), text, size, background };
                self.doc.update(before.id, |a| a.shape = shape);
                self.selected = Some(before.id);
                Effect::Redraw
            }
            None if empty => Effect::Redraw,
            None => {
                let shape = Shape::Text { origin, text, size, background: self.options.text_background };
                let id = self.doc.add(shape, self.style);
                self.selected = Some(id);
                Effect::Inserted(id)
            }
        }
    }

    /// Apply the pending crop (crop tool, Enter or double-click) and return
    /// to the pointer tool.
    pub fn apply_crop(&mut self) -> Effect {
        let Some(c) = self.pending_crop.take() else { return Effect::None };
        self.doc.set_crop(Some(c));
        self.tool = Tool::Select;
        Effect::Redraw
    }

    /// Remove the crop (one undo step).
    pub fn reset_crop(&mut self) -> Effect {
        let ok = self.doc.set_crop(None);
        if self.tool == Tool::Crop {
            self.pending_crop = Some(self.doc.canvas());
        }
        if ok || self.tool == Tool::Crop {
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    /// Feed one input.
    pub fn handle(&mut self, input: Input) -> Effect {
        match input {
            Input::Down { at, clicks, .. } => self.down(at, clicks),
            Input::Drag { at, shift } => self.drag(at, shift),
            Input::Up { at, shift } => self.up(at, shift),
            Input::Key { key, shift } => self.key(key, shift),
        }
    }

    // -- pointer -----------------------------------------------------------------

    fn down(&mut self, at: Point, clicks: u32) -> Effect {
        self.finish_gesture();
        if self.tool == Tool::Crop {
            return self.crop_down(at, clicks);
        }
        // 1. The selected annotation's handles and body.
        if let Some(sel) = self.selected.and_then(|id| self.doc.get(id).cloned()) {
            if let Some(handle) = sel.shape.handle_at(at, &sel.style, self.handle_radius) {
                self.gesture = Gesture::Resizing { before: sel, handle };
                return Effect::Redraw;
            }
            if sel.shape.hit(at, &sel.style, self.slop) {
                if clicks >= 2 || self.tool == Tool::Text {
                    if let Shape::Text { origin, .. } = sel.shape {
                        return Effect::EditText { id: Some(sel.id), origin };
                    }
                }
                self.gesture = Gesture::Moving { before: sel, grab: at, moved: false };
                return Effect::Redraw;
            }
        }
        let hit = self.doc.hit_test(at, self.slop);
        // 2. Text tool on existing text edits it.
        if self.tool == Tool::Text {
            if let Some(a) = hit.and_then(|id| self.doc.get(id)).filter(|a| matches!(a.shape, Shape::Text { .. })) {
                let (id, origin) = (a.id, text_origin(&a.shape, at));
                self.selected = Some(id);
                return Effect::EditText { id: Some(id), origin };
            }
            self.selected = None;
            return Effect::EditText { id: None, origin: at };
        }
        // 3. The pointer tool selects and moves.
        if self.tool == Tool::Select {
            let Some(id) = hit else {
                let changed = self.selected.take().is_some();
                return if changed { Effect::Redraw } else { Effect::None };
            };
            self.select(Some(id));
            let before = self.doc.get(id).cloned().expect("hit annotation exists");
            if clicks >= 2 {
                if let Shape::Text { origin, .. } = before.shape {
                    return Effect::EditText { id: Some(id), origin };
                }
            }
            self.gesture = Gesture::Moving { before, grab: at, moved: false };
            return Effect::Redraw;
        }
        // 4. Step badges are placed on press (and can be dragged into place).
        if self.tool == Tool::Step {
            let id = self.doc.add(Shape::Step { center: at }, self.style);
            self.selected = Some(id);
            let before = self.doc.get(id).cloned().expect("just added");
            self.gesture = Gesture::Moving { before, grab: at, moved: false };
            return Effect::Inserted(id);
        }
        // 5. Every other tool draws.
        let shape = self.initial_shape(at);
        let id = self.doc.new_id();
        self.doc.insert_untracked(Annotation { id, shape, style: self.style });
        self.selected = None;
        self.gesture = Gesture::Creating { id, anchor: at };
        Effect::Redraw
    }

    fn initial_shape(&self, at: Point) -> Shape {
        let r = Rect { origin: at, size: Size::ZERO };
        match self.tool {
            Tool::Arrow => Shape::Arrow { start: at, end: at, control: None },
            Tool::Line => Shape::Line { start: at, end: at },
            Tool::Rect => Shape::Rect { rect: r, rounded: self.options.rounded_rect },
            Tool::Ellipse => Shape::Ellipse { rect: r },
            Tool::Pen => Shape::Pen { points: vec![at] },
            Tool::Highlighter => Shape::Highlighter { points: vec![at] },
            Tool::Redact => Shape::Redact { rect: r, mode: self.options.redact },
            Tool::Spotlight => Shape::Spotlight { rect: r, ellipse: false },
            // Not reachable: handled before drawing starts.
            Tool::Select | Tool::Text | Tool::Step | Tool::Crop => Shape::Step { center: at },
        }
    }

    fn drag(&mut self, at: Point, shift: bool) -> Effect {
        match self.gesture.clone() {
            Gesture::Idle => Effect::None,
            Gesture::Creating { id, anchor } => {
                let Some(mut a) = self.doc.get(id).cloned() else { return Effect::None };
                a.shape = self.extend_shape(&a.shape, anchor, at, shift);
                self.doc.set_untracked(a);
                Effect::Redraw
            }
            Gesture::Moving { before, grab, .. } => {
                let mut a = before.clone();
                a.shape = before.shape.translated(at.x - grab.x, at.y - grab.y);
                self.doc.set_untracked(a);
                self.gesture = Gesture::Moving { before, grab, moved: true };
                Effect::Redraw
            }
            Gesture::Resizing { before, handle } => {
                let mut a = before.clone();
                a.shape = Shape::with_handle(&before.shape, handle, at, &before.style, shift);
                self.doc.set_untracked(a);
                Effect::Redraw
            }
            Gesture::CropDrawing { anchor } => {
                let end = if shift { constrain_aspect(anchor, at, 1.0) } else { at };
                self.pending_crop = Rect::from_corners(anchor, end).intersection(&self.doc.canvas());
                Effect::Redraw
            }
            Gesture::CropMoving { grab, start } => {
                let moved = start.offset(at.x - grab.x, at.y - grab.y).translated_within(&self.doc.canvas());
                self.pending_crop = Some(moved);
                Effect::Redraw
            }
            Gesture::CropResizing { handle, start } => {
                let r = resize_rect(start, handle, at, shift);
                self.pending_crop = r.intersection(&self.doc.canvas()).or(Some(r));
                Effect::Redraw
            }
        }
    }

    fn extend_shape(&self, shape: &Shape, anchor: Point, at: Point, shift: bool) -> Shape {
        let boxed = |square: bool| {
            let end = if square { constrain_aspect(anchor, at, 1.0) } else { at };
            Rect::from_corners(anchor, end)
        };
        match shape {
            Shape::Arrow { .. } => {
                let end = if shift { snap_angle(anchor, at) } else { at };
                Shape::Arrow { start: anchor, end, control: None }.with_curve(self.options.curved_arrow)
            }
            Shape::Line { .. } => Shape::Line { start: anchor, end: if shift { snap_angle(anchor, at) } else { at } },
            Shape::Rect { rounded, .. } => Shape::Rect { rect: boxed(shift), rounded: *rounded },
            Shape::Ellipse { .. } => Shape::Ellipse { rect: boxed(shift) },
            Shape::Redact { mode, .. } => Shape::Redact { rect: boxed(shift), mode: *mode },
            Shape::Spotlight { ellipse, .. } => Shape::Spotlight { rect: boxed(shift), ellipse: *ellipse },
            Shape::Pen { points } => {
                let mut pts = points.clone();
                push_sample(&mut pts, at, PEN_MIN_GAP);
                Shape::Pen { points: pts }
            }
            Shape::Highlighter { points } => {
                if shift {
                    // Straight marker stroke, snapped to 45°.
                    Shape::Highlighter { points: vec![anchor, snap_angle(anchor, at)] }
                } else {
                    let mut pts = points.clone();
                    push_sample(&mut pts, at, PEN_MIN_GAP);
                    Shape::Highlighter { points: pts }
                }
            }
            other => other.clone(),
        }
    }

    fn up(&mut self, at: Point, shift: bool) -> Effect {
        let effect = self.drag(at, shift);
        // Freehand strokes always end exactly where the pointer was released,
        // even if that sample was inside the jitter gap.
        if let Gesture::Creating { id, .. } = self.gesture {
            if let Some(mut a) = self.doc.get(id).cloned() {
                if let Shape::Pen { points } | Shape::Highlighter { points } = &mut a.shape {
                    if !shift && points.len() > 1 && points.last() != Some(&at) {
                        points.push(at);
                        self.doc.set_untracked(a);
                    }
                }
            }
        }
        let gesture = std::mem::replace(&mut self.gesture, Gesture::Idle);
        match gesture {
            Gesture::Creating { id, .. } => self.commit_creation(id),
            Gesture::Moving { before, moved, .. } => {
                if moved {
                    if let Some(after) = self.doc.get(before.id).cloned() {
                        self.doc.record(Command::Replace { before, after });
                    }
                }
                Effect::Redraw
            }
            Gesture::Resizing { before, .. } => {
                if let Some(after) = self.doc.get(before.id).cloned() {
                    self.doc.record(Command::Replace { before, after });
                }
                Effect::Redraw
            }
            Gesture::CropDrawing { .. } | Gesture::CropMoving { .. } | Gesture::CropResizing { .. } => {
                if self.pending_crop.is_some_and(|r| r.width() < 4.0 || r.height() < 4.0) {
                    self.pending_crop = Some(self.doc.visible_rect());
                }
                Effect::Redraw
            }
            Gesture::Idle => effect,
        }
    }

    fn commit_creation(&mut self, id: AnnotationId) -> Effect {
        let Some(mut item) = self.doc.get(id).cloned() else { return Effect::Redraw };
        if !item.shape.is_meaningful(&item.style) {
            self.doc.remove_untracked(id);
            return Effect::Redraw;
        }
        // Tidy freehand strokes once, at the end.
        if let Shape::Pen { points } | Shape::Highlighter { points } = &mut item.shape {
            *points = simplify(points, PEN_SIMPLIFY);
        }
        self.doc.set_untracked(item.clone());
        let index = self.doc.index_of(id).unwrap_or(0);
        self.doc.record(Command::Insert { index, item });
        self.selected = Some(id);
        Effect::Inserted(id)
    }

    /// Abandon or complete whatever gesture is in flight (tool switch, undo
    /// mid-drag, a stray mouse-down without an up).
    fn finish_gesture(&mut self) {
        match std::mem::replace(&mut self.gesture, Gesture::Idle) {
            Gesture::Creating { id, .. } => {
                let _ = self.commit_creation(id);
            }
            Gesture::Moving { before, .. } | Gesture::Resizing { before, .. } => {
                if let Some(after) = self.doc.get(before.id).cloned() {
                    self.doc.record(Command::Replace { before, after });
                }
            }
            _ => {}
        }
    }

    fn crop_down(&mut self, at: Point, clicks: u32) -> Effect {
        let current = self.pending_crop.unwrap_or_else(|| self.doc.visible_rect());
        if clicks >= 2 && current.contains(at) {
            return self.apply_crop();
        }
        if let Some(handle) = current.handle_at(at, self.handle_radius) {
            self.gesture = Gesture::CropResizing { handle, start: current };
        } else if current.contains(at) && current != self.doc.canvas() {
            self.gesture = Gesture::CropMoving { grab: at, start: current };
        } else {
            self.gesture = Gesture::CropDrawing { anchor: self.doc.canvas().clamp_point(at) };
        }
        Effect::Redraw
    }

    // -- keys --------------------------------------------------------------------

    fn key(&mut self, key: Key, shift: bool) -> Effect {
        match key {
            Key::Escape => {
                if self.is_interacting() {
                    self.cancel_gesture();
                    return Effect::Redraw;
                }
                if self.tool == Tool::Crop {
                    self.pending_crop = None;
                    self.tool = Tool::Select;
                    return Effect::Redraw;
                }
                if self.selected.take().is_some() {
                    return Effect::Redraw;
                }
                Effect::Close
            }
            Key::Enter => {
                if self.tool == Tool::Crop {
                    return self.apply_crop();
                }
                match self.selected.and_then(|id| self.doc.get(id)) {
                    Some(a) => match a.shape {
                        Shape::Text { origin, .. } => Effect::EditText { id: Some(a.id), origin },
                        _ => Effect::None,
                    },
                    None => Effect::None,
                }
            }
            Key::Delete => self.delete_selected(),
            Key::Nudge(dx, dy) => {
                let step = if shift { 10.0 } else { 1.0 };
                let (dx, dy) = (f64::from(dx) * step, f64::from(dy) * step);
                if self.tool == Tool::Crop {
                    if let Some(c) = self.pending_crop {
                        self.pending_crop = Some(c.offset(dx, dy).translated_within(&self.doc.canvas()));
                        return Effect::Redraw;
                    }
                    return Effect::None;
                }
                match self.selected {
                    Some(id) if self.doc.update(id, |a| a.shape = a.shape.translated(dx, dy)) => Effect::Redraw,
                    _ => Effect::None,
                }
            }
        }
    }

    /// Escape mid-gesture: put things back as they were.
    fn cancel_gesture(&mut self) {
        match std::mem::replace(&mut self.gesture, Gesture::Idle) {
            Gesture::Creating { id, .. } => {
                self.doc.remove_untracked(id);
            }
            Gesture::Moving { before, .. } | Gesture::Resizing { before, .. } => self.doc.set_untracked(before),
            Gesture::CropDrawing { .. } | Gesture::CropMoving { .. } | Gesture::CropResizing { .. } => {
                self.pending_crop = Some(self.doc.visible_rect());
            }
            Gesture::Idle => {}
        }
    }
}

/// Keep a text's existing origin when re-editing it.
fn text_origin(shape: &Shape, fallback: Point) -> Point {
    match shape {
        Shape::Text { origin, .. } => *origin,
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::style::PALETTE;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn editor() -> Editor {
        Editor::new(Size::new(800.0, 600.0))
    }

    fn drag(e: &mut Editor, from: Point, to: Point) -> Effect {
        e.handle(Input::Down { at: from, clicks: 1, shift: false });
        e.handle(Input::Drag { at: from.lerp(to, 0.5), shift: false });
        e.handle(Input::Drag { at: to, shift: false });
        e.handle(Input::Up { at: to, shift: false })
    }

    fn key(e: &mut Editor, k: Key) -> Effect {
        e.handle(Input::Key { key: k, shift: false })
    }

    #[test]
    fn drawing_an_arrow_is_one_undo_step_and_selects_it() {
        let mut e = editor();
        assert_eq!(e.tool(), Tool::Arrow);
        let eff = drag(&mut e, p(10.0, 10.0), p(200.0, 100.0));
        let id = match eff {
            Effect::Inserted(id) => id,
            other => panic!("{other:?}"),
        };
        assert_eq!(e.selected(), Some(id));
        assert_eq!(e.document().items().len(), 1);
        assert_eq!(e.document().history().undo_len(), 1);
        assert_eq!(
            e.document().items()[0].shape,
            Shape::Arrow { start: p(10.0, 10.0), end: p(200.0, 100.0), control: None }
        );
        e.undo();
        assert!(e.document().items().is_empty());
        assert_eq!(e.selected(), None);
        e.redo();
        assert_eq!(e.document().items().len(), 1);
    }

    #[test]
    fn tiny_drags_create_nothing() {
        let mut e = editor();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(10.0, 10.0), p(11.0, 11.0));
        assert!(e.document().items().is_empty());
        assert!(!e.document().history().can_undo());
    }

    #[test]
    fn shift_constrains_shapes_and_angles() {
        let mut e = editor();
        e.set_tool(Tool::Rect);
        e.handle(Input::Down { at: p(0.0, 0.0), clicks: 1, shift: true });
        e.handle(Input::Up { at: p(100.0, 40.0), shift: true });
        assert_eq!(
            e.document().items()[0].shape,
            Shape::Rect { rect: Rect::new(0.0, 0.0, 100.0, 100.0), rounded: false }
        );
        e.set_tool(Tool::Line);
        e.handle(Input::Down { at: p(300.0, 300.0), clicks: 1, shift: true });
        e.handle(Input::Up { at: p(400.0, 310.0), shift: true });
        match &e.document().items()[1].shape {
            Shape::Line { end, .. } => assert!((end.y - 300.0).abs() < 1e-9),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn curved_option_bends_new_arrows_and_toggles_selected() {
        let mut e = editor();
        e.set_options(ToolOptions { curved_arrow: true, ..Default::default() });
        drag(&mut e, p(0.0, 100.0), p(200.0, 100.0));
        assert!(matches!(e.document().items()[0].shape, Shape::Arrow { control: Some(_), .. }));
        e.set_options(ToolOptions { curved_arrow: false, ..Default::default() });
        assert!(matches!(e.document().items()[0].shape, Shape::Arrow { control: None, .. }));
        e.undo();
        assert!(matches!(e.document().items()[0].shape, Shape::Arrow { control: Some(_), .. }));
    }

    #[test]
    fn grabbing_the_selection_moves_it_with_any_tool() {
        let mut e = editor();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(100.0, 100.0), p(200.0, 200.0));
        // Still on the rect tool: dragging its edge moves instead of drawing.
        drag(&mut e, p(100.0, 130.0), p(110.0, 150.0));
        assert_eq!(e.document().items().len(), 1);
        assert_eq!(
            e.document().items()[0].shape,
            Shape::Rect { rect: Rect::new(110.0, 120.0, 100.0, 100.0), rounded: false }
        );
        assert_eq!(e.document().history().undo_len(), 2);
        e.undo();
        assert_eq!(
            e.document().items()[0].shape,
            Shape::Rect { rect: Rect::new(100.0, 100.0, 100.0, 100.0), rounded: false }
        );
    }

    #[test]
    fn handles_resize_the_selection() {
        let mut e = editor();
        e.set_tool(Tool::Ellipse);
        drag(&mut e, p(100.0, 100.0), p(200.0, 200.0));
        drag(&mut e, p(200.0, 200.0), p(300.0, 250.0));
        assert_eq!(e.document().items()[0].shape, Shape::Ellipse { rect: Rect::new(100.0, 100.0, 200.0, 150.0) });
        assert_eq!(e.document().history().undo_len(), 2);
    }

    #[test]
    fn a_click_without_movement_records_nothing() {
        let mut e = editor();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(100.0, 100.0), p(200.0, 200.0));
        e.handle(Input::Down { at: p(130.0, 100.0), clicks: 1, shift: false });
        e.handle(Input::Up { at: p(130.0, 100.0), shift: false });
        assert_eq!(e.document().history().undo_len(), 1);
    }

    #[test]
    fn select_tool_picks_topmost_and_clicking_empty_deselects() {
        let mut e = editor();
        e.set_tool(Tool::Redact);
        drag(&mut e, p(0.0, 0.0), p(100.0, 100.0));
        drag(&mut e, p(50.0, 50.0), p(150.0, 150.0));
        // The second press landed inside the selected first redaction, so it moved it.
        assert_eq!(e.document().items().len(), 1);
        e.set_tool(Tool::Select);
        e.handle(Input::Down { at: p(700.0, 500.0), clicks: 1, shift: false });
        e.handle(Input::Up { at: p(700.0, 500.0), shift: false });
        assert_eq!(e.selected(), None);
        e.handle(Input::Down { at: p(120.0, 120.0), clicks: 1, shift: false });
        assert_eq!(e.selected(), Some(e.document().items()[0].id));
    }

    #[test]
    fn steps_auto_increment_and_are_draggable() {
        let mut e = editor();
        e.set_tool(Tool::Step);
        for x in [10.0, 60.0, 110.0] {
            e.handle(Input::Down { at: p(x, 300.0), clicks: 1, shift: false });
            e.handle(Input::Up { at: p(x, 300.0), shift: false });
            // Deselect so the next press doesn't grab the badge just placed.
            e.select(None);
        }
        let d = e.document();
        let nums: Vec<_> = d.items().iter().map(|a| d.step_number(a.id).unwrap()).collect();
        assert_eq!(nums, vec![1, 2, 3]);
        assert_eq!(d.history().undo_len(), 3);
        // Press-and-drag places then moves (two steps: insert + move).
        let mut e2 = editor();
        e2.set_tool(Tool::Step);
        let eff = e2.handle(Input::Down { at: p(10.0, 10.0), clicks: 1, shift: false });
        assert!(matches!(eff, Effect::Inserted(_)));
        e2.handle(Input::Drag { at: p(40.0, 50.0), shift: false });
        e2.handle(Input::Up { at: p(40.0, 50.0), shift: false });
        assert_eq!(e2.document().items()[0].shape, Shape::Step { center: p(40.0, 50.0) });
        e2.undo();
        assert_eq!(e2.document().items()[0].shape, Shape::Step { center: p(10.0, 10.0) });
    }

    #[test]
    fn text_flow_create_edit_delete() {
        let mut e = editor();
        e.set_tool(Tool::Text);
        let eff = e.handle(Input::Down { at: p(50.0, 50.0), clicks: 1, shift: false });
        assert_eq!(eff, Effect::EditText { id: None, origin: p(50.0, 50.0) });
        e.handle(Input::Up { at: p(50.0, 50.0), shift: false });
        let id = match e.commit_text(None, p(50.0, 50.0), "Hello\n", Size::new(60.0, 24.0)) {
            Effect::Inserted(id) => id,
            other => panic!("{other:?}"),
        };
        assert!(matches!(&e.document().get(id).unwrap().shape, Shape::Text { text, .. } if text == "Hello"));
        // Clicking it again with the text tool edits it in place.
        let eff = e.handle(Input::Down { at: p(60.0, 60.0), clicks: 1, shift: false });
        assert_eq!(eff, Effect::EditText { id: Some(id), origin: p(50.0, 50.0) });
        e.handle(Input::Up { at: p(60.0, 60.0), shift: false });
        e.commit_text(Some(id), p(50.0, 50.0), "Hello, world", Size::new(120.0, 24.0));
        assert!(matches!(&e.document().get(id).unwrap().shape, Shape::Text { text, .. } if text == "Hello, world"));
        // Emptying it deletes it; undo brings it back.
        e.commit_text(Some(id), p(50.0, 50.0), "   ", Size::ZERO);
        assert!(e.document().get(id).is_none());
        e.undo();
        assert!(e.document().get(id).is_some());
        // Empty new text creates nothing.
        let n = e.document().history().undo_len();
        e.commit_text(None, p(0.0, 0.0), "", Size::ZERO);
        assert_eq!(e.document().history().undo_len(), n);
    }

    #[test]
    fn text_background_option_applies() {
        let mut e = editor();
        e.set_options(ToolOptions { text_background: true, ..Default::default() });
        e.set_tool(Tool::Text);
        let id = match e.commit_text(None, p(0.0, 0.0), "pill", Size::new(30.0, 20.0)) {
            Effect::Inserted(id) => id,
            _ => unreachable!(),
        };
        assert!(matches!(e.document().get(id).unwrap().shape, Shape::Text { background: true, .. }));
    }

    #[test]
    fn pen_strokes_are_sampled_and_simplified() {
        let mut e = editor();
        e.set_tool(Tool::Pen);
        e.handle(Input::Down { at: p(0.0, 0.0), clicks: 1, shift: false });
        for i in 1..=100 {
            e.handle(Input::Drag { at: p(i as f64 * 0.5, 0.0), shift: false });
        }
        e.handle(Input::Up { at: p(50.0, 0.0), shift: false });
        match &e.document().items()[0].shape {
            Shape::Pen { points } => {
                assert_eq!(points, &vec![p(0.0, 0.0), p(50.0, 0.0)], "a straight stroke collapses")
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn style_changes_apply_to_selection_and_future_shapes() {
        let mut e = editor();
        drag(&mut e, p(0.0, 0.0), p(100.0, 0.0));
        e.set_color(PALETTE[3]);
        e.set_width(StrokeWidth::Thick);
        let a = &e.document().items()[0];
        assert_eq!(a.style, Style::new(PALETTE[3], StrokeWidth::Thick));
        assert_eq!(e.document().history().undo_len(), 3);
        e.select(None);
        e.set_color(PALETTE[2]);
        assert_eq!(e.document().history().undo_len(), 3, "nothing selected: no undo step");
        drag(&mut e, p(0.0, 200.0), p(100.0, 200.0));
        assert_eq!(e.document().items()[1].style.color, PALETTE[2]);
    }

    #[test]
    fn opacity_restyles_the_selection_and_undoes() {
        let mut e = editor();
        drag(&mut e, p(0.0, 0.0), p(100.0, 0.0));
        let undo0 = e.document().history().undo_len();
        e.set_opacity(40);
        assert_eq!(e.document().items()[0].style.opacity, 40);
        assert_eq!(e.document().history().undo_len(), undo0 + 1, "one undo step");
        e.set_opacity(3);
        assert_eq!(e.document().items()[0].style.opacity, 10, "clamped");
        e.undo();
        assert_eq!(e.document().items()[0].style.opacity, 40);
        e.undo();
        assert_eq!(e.document().items()[0].style.opacity, 100);
        e.redo();
        assert_eq!(e.document().items()[0].style.opacity, 40);
        // Selecting mirrors the opacity into the toolbar state; new shapes use it.
        e.select(None);
        e.set_opacity(70);
        drag(&mut e, p(0.0, 200.0), p(100.0, 200.0));
        assert_eq!(e.document().items()[1].style.opacity, 70);
        e.select(Some(e.document().items()[0].id));
        assert_eq!(e.style().opacity, 40);
    }

    #[test]
    fn keys_delete_nudge_escape() {
        let mut e = editor();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(100.0, 100.0), p(200.0, 200.0));
        e.handle(Input::Key { key: Key::Nudge(1, 0), shift: true });
        assert_eq!(
            e.document().items()[0].shape,
            Shape::Rect { rect: Rect::new(110.0, 100.0, 100.0, 100.0), rounded: false }
        );
        assert_eq!(key(&mut e, Key::Delete), Effect::Redraw);
        assert!(e.document().items().is_empty());
        assert_eq!(key(&mut e, Key::Escape), Effect::Close, "nothing to cancel closes the editor");
    }

    #[test]
    fn escape_mid_drag_cancels_without_history() {
        let mut e = editor();
        e.handle(Input::Down { at: p(0.0, 0.0), clicks: 1, shift: false });
        e.handle(Input::Drag { at: p(50.0, 50.0), shift: false });
        assert_eq!(e.document().items().len(), 1);
        assert_eq!(key(&mut e, Key::Escape), Effect::Redraw);
        assert!(e.document().items().is_empty());
        assert!(!e.document().history().can_undo());
    }

    #[test]
    fn switching_tools_mid_drag_commits_the_shape() {
        let mut e = editor();
        e.handle(Input::Down { at: p(0.0, 0.0), clicks: 1, shift: false });
        e.handle(Input::Drag { at: p(80.0, 0.0), shift: false });
        e.set_tool(Tool::Rect);
        assert_eq!(e.document().items().len(), 1);
        assert_eq!(e.document().history().undo_len(), 1);
    }

    #[test]
    fn crop_flow() {
        let mut e = editor();
        e.set_tool(Tool::Crop);
        assert_eq!(e.pending_crop(), Some(Rect::new(0.0, 0.0, 800.0, 600.0)));
        drag(&mut e, p(100.0, 100.0), p(500.0, 400.0));
        assert_eq!(e.pending_crop(), Some(Rect::new(100.0, 100.0, 400.0, 300.0)));
        // Move it by dragging inside.
        drag(&mut e, p(200.0, 200.0), p(250.0, 200.0));
        assert_eq!(e.pending_crop(), Some(Rect::new(150.0, 100.0, 400.0, 300.0)));
        // Resize from a corner.
        drag(&mut e, p(550.0, 400.0), p(600.0, 450.0));
        assert_eq!(e.pending_crop(), Some(Rect::new(150.0, 100.0, 450.0, 350.0)));
        assert_eq!(key(&mut e, Key::Enter), Effect::Redraw);
        assert_eq!(e.document().crop(), Some(Rect::new(150.0, 100.0, 450.0, 350.0)));
        assert_eq!(e.tool(), Tool::Select);
        e.undo();
        assert_eq!(e.document().crop(), None);
        // Escape in the crop tool discards.
        e.set_tool(Tool::Crop);
        drag(&mut e, p(10.0, 10.0), p(50.0, 50.0));
        key(&mut e, Key::Escape);
        assert_eq!(e.pending_crop(), None);
        assert_eq!(e.document().crop(), None);
    }

    #[test]
    fn crop_drawn_outside_the_canvas_is_clamped() {
        let mut e = editor();
        e.set_tool(Tool::Crop);
        drag(&mut e, p(-50.0, -50.0), p(100.0, 100.0));
        assert_eq!(e.pending_crop(), Some(Rect::new(0.0, 0.0, 100.0, 100.0)));
        e.reset_crop();
        assert_eq!(e.pending_crop(), Some(e.document().canvas()));
    }

    #[test]
    fn zoom_scales_hit_targets() {
        let mut e = editor();
        e.set_tool(Tool::Line);
        drag(&mut e, p(0.0, 100.0), p(200.0, 100.0));
        e.select(None);
        e.set_tool(Tool::Select);
        e.set_zoom(0.25); // zoomed out: 4 screen pt = 16 doc pt
        e.handle(Input::Down { at: p(100.0, 115.0), clicks: 1, shift: false });
        assert!(e.selected().is_some());
        e.handle(Input::Up { at: p(100.0, 115.0), shift: false });
        e.select(None);
        e.set_zoom(2.0);
        e.handle(Input::Down { at: p(100.0, 105.0), clicks: 1, shift: false });
        assert!(e.selected().is_none());
    }

    #[test]
    fn z_order_commands_work_on_selection() {
        let mut e = editor();
        e.set_tool(Tool::Redact);
        drag(&mut e, p(0.0, 0.0), p(50.0, 50.0));
        e.select(None);
        drag(&mut e, p(100.0, 100.0), p(150.0, 150.0));
        let top = e.selected().unwrap();
        assert_eq!(e.send_to_back(), Effect::Redraw);
        assert_eq!(e.document().items()[0].id, top);
        assert_eq!(e.bring_to_front(), Effect::Redraw);
        assert_eq!(e.document().items()[1].id, top);
    }

    #[test]
    fn tool_keys_round_trip() {
        for t in Tool::ALL {
            assert_eq!(Tool::from_key(t.key()), Some(t));
            assert_eq!(Tool::from_key(t.key().to_ascii_uppercase()), Some(t));
            assert!(!t.symbol().is_empty());
        }
        assert_eq!(Tool::from_key('z'), None);
        assert!(!Tool::Redact.uses_color() && Tool::Arrow.uses_color());
    }

    #[test]
    fn smart_redactions_are_one_undo_step() {
        let mut e = Editor::new(Size::new(200.0, 100.0));
        let rects =
            [Rect::new(10.0, 10.0, 30.0, 10.0), Rect::new(150.0, 80.0, 100.0, 50.0), Rect::new(0.0, 0.0, 0.5, 9.0)];
        let (n, effect) = e.add_redactions(&rects, RedactMode::Blur);
        assert_eq!((n, effect), (2, Effect::Redraw));
        assert_eq!(e.document().items().len(), 2);
        // Clipped to the canvas.
        assert!(
            matches!(e.document().items()[1].shape, Shape::Redact { rect, mode: RedactMode::Blur } if rect == Rect::new(150.0, 80.0, 50.0, 20.0))
        );
        assert_eq!(e.undo(), Effect::Redraw);
        assert!(e.document().is_empty());
        assert_eq!(e.redo(), Effect::Redraw);
        assert_eq!(e.document().items().len(), 2);
        assert_eq!(e.add_redactions(&[], RedactMode::Pixelate), (0, Effect::None));
        assert_eq!(e.document().history().undo_len(), 1);
    }
}

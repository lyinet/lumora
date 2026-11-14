//! The capture overlay's interaction model as a pure state machine.
//!
//! The overlay view translates `NSEvent`s into [`Input`]s (in global
//! top-left points), feeds them to [`SelectionMachine::handle`] and reacts to
//! the returned [`Outcome`]. Keeping this logic out of AppKit makes every
//! gesture — drag, Space-to-move, Shift aspect lock, nudging, resizing,
//! window click — testable on any host.
//!
//! ```text
//!            mouse down                 drag > tolerance
//!  Hovering ───────────▶ Pressing ────────────────────────▶ Dragging
//!     ▲                     │ mouse up (click)                 │ mouse up
//!     │                     ▼                                  ▼
//!     │             Confirm(Window/Display)   ┌──────────── Selected ◀──┐
//!     │                                       │ down on handle / inside │ up
//!     └── too small ◀── Dragging              ▼                         │
//!                                     Resizing / Moving ────────────────┘
//! ```

use crate::geometry::{Handle, Point, Rect};
use crate::snap::{SnapEdges, WindowInfo, WindowList};

/// Tunables for the machine. Defaults match DESIGN.md.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Union of all display frames (global top-left points). The selection is
    /// always clamped inside it.
    pub bounds: Rect,
    /// Individual display frames, used as snap edges and to resolve a plain
    /// click on the desktop to a display.
    pub displays: Vec<Rect>,
    /// Selections smaller than this on either side are discarded on release.
    pub min_size: f64,
    /// Movement (points) below which a press-release is a click, not a drag.
    pub click_tolerance: f64,
    /// Half-size of the square hit area around resize handles.
    pub handle_radius: f64,
    /// Distance at which drag corners stick to window / display edges.
    /// Holding ⌥ while dragging disables snapping.
    pub snap_threshold: f64,
    /// Whether hovering highlights windows and a click captures them.
    pub window_picking: bool,
}

impl Config {
    pub fn new(displays: Vec<Rect>) -> Self {
        let bounds = displays.iter().fold(Rect::ZERO, |acc, d| acc.union(d));
        Config {
            bounds,
            displays,
            min_size: 4.0,
            click_tolerance: 3.0,
            handle_radius: 8.0,
            snap_threshold: 6.0,
            window_picking: true,
        }
    }
}

/// Direction for arrow-key nudging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    fn delta(self) -> (f64, f64) {
        match self {
            Direction::Left => (-1.0, 0.0),
            Direction::Right => (1.0, 0.0),
            Direction::Up => (0.0, -1.0), // y-down space
            Direction::Down => (0.0, 1.0),
        }
    }
}

/// Keys the overlay cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Space,
    Arrow(Direction),
    /// `C` — copy the colour under the cursor.
    CopyColor,
}

/// Modifier state that changes gesture behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub option: bool,
}

/// An input event in global top-left points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    MouseMoved(Point),
    MouseDown { at: Point, clicks: u32 },
    MouseDragged(Point),
    MouseUp(Point),
    KeyDown(Key),
    KeyUp(Key),
    ModifiersChanged(Mods),
}

/// What the user ultimately chose.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// An arbitrary rectangle (global top-left points).
    Area(Rect),
    /// A specific window.
    Window(WindowInfo),
    /// The whole display containing the click (its frame).
    Display(Rect),
}

/// What the view should do after an input.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Nothing visible changed.
    None,
    /// State changed; redraw.
    Redraw,
    /// Dismiss the overlay without capturing.
    Cancel,
    /// Capture this target and dismiss.
    Confirm(Target),
    /// Copy the hex colour under the cursor to the clipboard.
    CopyColor,
}

/// Which cursor the view should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorHint {
    Crosshair,
    Move,
    Resize(Handle),
}

/// Current interaction phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// No button down. `window` is the hovered window index, if any.
    Hovering { window: Option<usize> },
    /// Button down but not yet moved past the click tolerance.
    Pressing { at: Point, window: Option<usize> },
    /// Rubber-banding a new selection from `anchor`.
    Dragging { anchor: Point },
    /// A selection exists and no button is down.
    Selected,
    /// Dragging an existing selection; `grab` is the cursor offset from origin.
    Moving { grab: Point },
    /// Resizing via `handle`; `fixed` is the opposite corner/edge point.
    Resizing { handle: Handle, fixed: Point },
}

/// The overlay state machine.
#[derive(Debug, Clone)]
pub struct SelectionMachine {
    cfg: Config,
    windows: WindowList,
    edges: SnapEdges,
    phase: Phase,
    selection: Option<Rect>,
    mouse: Point,
    mods: Mods,
    /// Space held during a rubber-band drag: move instead of resize.
    space_moving: bool,
    /// Locked aspect ratio (w/h) while Shift is held.
    aspect: Option<f64>,
}

impl SelectionMachine {
    pub fn new(cfg: Config, windows: WindowList) -> Self {
        let edges = windows.edges(&cfg.displays);
        SelectionMachine {
            cfg,
            windows,
            edges,
            phase: Phase::Hovering { window: None },
            selection: None,
            mouse: Point::ZERO,
            mods: Mods::default(),
            space_moving: false,
            aspect: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn selection(&self) -> Option<Rect> {
        self.selection
    }
    pub fn mouse(&self) -> Point {
        self.mouse
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }
    pub fn windows(&self) -> &WindowList {
        &self.windows
    }

    /// The window to highlight, if hovering over one.
    pub fn hovered_window(&self) -> Option<&WindowInfo> {
        match self.phase {
            Phase::Hovering { window: Some(i) } | Phase::Pressing { window: Some(i), .. } => self.windows.get(i),
            _ => None,
        }
    }

    /// Whether a button is currently down.
    pub fn is_interacting(&self) -> bool {
        matches!(
            self.phase,
            Phase::Pressing { .. } | Phase::Dragging { .. } | Phase::Moving { .. } | Phase::Resizing { .. }
        )
    }

    /// Suggested cursor for the current position.
    pub fn cursor_hint(&self) -> CursorHint {
        match self.phase {
            Phase::Moving { .. } => CursorHint::Move,
            Phase::Resizing { handle, .. } => CursorHint::Resize(handle),
            Phase::Selected => {
                let sel = self.selection.unwrap_or_default();
                if let Some(h) = sel.handle_at(self.mouse, self.cfg.handle_radius) {
                    CursorHint::Resize(h)
                } else if sel.contains(self.mouse) {
                    CursorHint::Move
                } else {
                    CursorHint::Crosshair
                }
            }
            _ => CursorHint::Crosshair,
        }
    }

    /// Feed one input and get the reaction.
    pub fn handle(&mut self, input: Input) -> Outcome {
        match input {
            Input::MouseMoved(p) => self.mouse_moved(p),
            Input::MouseDown { at, clicks } => self.mouse_down(at, clicks),
            Input::MouseDragged(p) => self.mouse_dragged(p),
            Input::MouseUp(p) => self.mouse_up(p),
            Input::KeyDown(k) => self.key_down(k),
            Input::KeyUp(k) => self.key_up(k),
            Input::ModifiersChanged(m) => self.modifiers_changed(m),
        }
    }

    fn clamp(&self, p: Point) -> Point {
        self.cfg.bounds.clamp_point(p)
    }

    fn hover_index(&self, p: Point) -> Option<usize> {
        if self.cfg.window_picking {
            self.windows.index_at(p)
        } else {
            None
        }
    }

    fn mouse_moved(&mut self, p: Point) -> Outcome {
        self.mouse = self.clamp(p);
        if let Phase::Hovering { window } = self.phase {
            let new = self.hover_index(self.mouse);
            self.phase = Phase::Hovering { window: new };
            // Always redraw while hovering: crosshair and loupe follow.
            let _ = window;
        }
        Outcome::Redraw
    }

    fn mouse_down(&mut self, at: Point, clicks: u32) -> Outcome {
        let p = self.clamp(at);
        self.mouse = p;
        match (self.phase, self.selection) {
            (Phase::Selected, Some(sel)) => {
                if let Some(handle) = sel.handle_at(p, self.cfg.handle_radius) {
                    let fixed = sel.handle_point(handle.opposite());
                    self.phase = Phase::Resizing { handle, fixed };
                } else if sel.contains(p) {
                    if clicks >= 2 {
                        return self.confirm_selection();
                    }
                    self.phase = Phase::Moving { grab: p - sel.origin };
                } else {
                    // Clicking outside starts over.
                    self.selection = None;
                    self.phase = Phase::Pressing { at: p, window: self.hover_index(p) };
                }
            }
            _ => {
                let window = self.hover_index(p);
                self.phase = Phase::Pressing { at: p, window };
            }
        }
        Outcome::Redraw
    }

    fn mouse_dragged(&mut self, raw: Point) -> Outcome {
        let prev = self.mouse;
        let p = self.clamp(raw);
        self.mouse = p;
        match self.phase {
            Phase::Pressing { at, .. } => {
                if at.distance(p) <= self.cfg.click_tolerance {
                    return Outcome::None;
                }
                let anchor = self.snap(at);
                self.phase = Phase::Dragging { anchor };
                if self.mods.shift {
                    self.aspect = Some(1.0);
                }
                self.update_drag(anchor, p);
            }
            Phase::Dragging { anchor } => {
                if self.space_moving {
                    // Translate the whole band: move the anchor with the cursor.
                    let d = p - prev;
                    let sel = self.selection.unwrap_or_default().offset(d.x, d.y).translated_within(&self.cfg.bounds);
                    let actual = sel.origin - self.selection.unwrap_or_default().origin;
                    self.phase = Phase::Dragging { anchor: anchor + actual };
                    self.selection = Some(sel);
                } else {
                    self.update_drag(anchor, p);
                }
            }
            Phase::Moving { grab } => {
                if let Some(sel) = self.selection {
                    let origin = p - grab;
                    let moved = Rect { origin, size: sel.size }.translated_within(&self.cfg.bounds);
                    self.selection = Some(moved);
                }
            }
            Phase::Resizing { handle, fixed } => {
                if let Some(sel) = self.selection {
                    self.selection = Some(self.resized(sel, handle, fixed, p));
                }
            }
            _ => return Outcome::None,
        }
        Outcome::Redraw
    }

    fn mouse_up(&mut self, at: Point) -> Outcome {
        let p = self.clamp(at);
        self.mouse = p;
        self.space_moving = false;
        match self.phase {
            Phase::Pressing { at, window } => {
                if let Some(w) = window.and_then(|i| self.windows.get(i)).cloned() {
                    return Outcome::Confirm(Target::Window(w));
                }
                let display = self.cfg.displays.iter().find(|d| d.contains(at)).copied();
                match display {
                    Some(d) => Outcome::Confirm(Target::Display(d)),
                    None => {
                        self.phase = Phase::Hovering { window: None };
                        Outcome::Redraw
                    }
                }
            }
            Phase::Dragging { .. } => {
                let big_enough =
                    self.selection.is_some_and(|s| s.width() >= self.cfg.min_size && s.height() >= self.cfg.min_size);
                if big_enough {
                    self.phase = Phase::Selected;
                } else {
                    self.selection = None;
                    self.phase = Phase::Hovering { window: self.hover_index(p) };
                }
                Outcome::Redraw
            }
            Phase::Moving { .. } | Phase::Resizing { .. } => {
                self.phase = Phase::Selected;
                Outcome::Redraw
            }
            _ => Outcome::None,
        }
    }

    fn key_down(&mut self, key: Key) -> Outcome {
        match key {
            Key::Escape => Outcome::Cancel,
            Key::Enter => {
                if self.selection.is_some() && !matches!(self.phase, Phase::Dragging { .. }) {
                    self.confirm_selection()
                } else if let Some(w) = self.hovered_window().cloned() {
                    Outcome::Confirm(Target::Window(w))
                } else {
                    Outcome::None
                }
            }
            Key::Space => {
                if matches!(self.phase, Phase::Dragging { .. }) && !self.space_moving {
                    self.space_moving = true;
                    Outcome::Redraw
                } else {
                    Outcome::None
                }
            }
            Key::Arrow(dir) => self.nudge(dir),
            Key::CopyColor => Outcome::CopyColor,
        }
    }

    fn key_up(&mut self, key: Key) -> Outcome {
        if key == Key::Space && self.space_moving {
            self.space_moving = false;
            return Outcome::Redraw;
        }
        Outcome::None
    }

    fn modifiers_changed(&mut self, m: Mods) -> Outcome {
        let was_shift = self.mods.shift;
        self.mods = m;
        if m.shift && !was_shift {
            // Lock to the current ratio mid-drag, or square if nothing yet.
            self.aspect = Some(
                self.selection
                    .filter(|s| s.width() >= self.cfg.min_size && s.height() >= self.cfg.min_size)
                    .and_then(|s| s.size.aspect_ratio())
                    .unwrap_or(1.0),
            );
        } else if !m.shift {
            self.aspect = None;
        }
        // Re-apply the drag with the new constraint so the change is visible
        // without moving the mouse.
        if let Phase::Dragging { anchor } = self.phase {
            if !self.space_moving {
                self.update_drag(anchor, self.mouse);
            }
            return Outcome::Redraw;
        }
        Outcome::None
    }

    /// Arrow keys: move the selection 1pt (10pt with ⇧). With ⌥, grow/shrink
    /// the size instead (right/down grow, left/up shrink).
    fn nudge(&mut self, dir: Direction) -> Outcome {
        let Some(sel) = self.selection else { return Outcome::None };
        if !matches!(self.phase, Phase::Selected) {
            return Outcome::None;
        }
        let step = if self.mods.shift { 10.0 } else { 1.0 };
        let (dx, dy) = dir.delta();
        let new = if self.mods.option {
            let w = (sel.width() + dx * step).max(self.cfg.min_size);
            let h = (sel.height() + dy * step).max(self.cfg.min_size);
            let r = Rect { origin: sel.origin, size: crate::Size::new(w, h) };
            r.intersection(&self.cfg.bounds).unwrap_or(sel)
        } else {
            sel.offset(dx * step, dy * step).translated_within(&self.cfg.bounds)
        };
        self.selection = Some(new);
        Outcome::Redraw
    }

    fn confirm_selection(&mut self) -> Outcome {
        match self.selection {
            Some(s) if !s.is_empty() => Outcome::Confirm(Target::Area(s)),
            _ => Outcome::None,
        }
    }

    fn snap(&self, p: Point) -> Point {
        if self.mods.option {
            p
        } else {
            self.edges.snap(p, self.cfg.snap_threshold)
        }
    }

    fn update_drag(&mut self, anchor: Point, cursor: Point) {
        let cursor = self.snap(cursor);
        let end = match self.aspect {
            Some(r) => self.clamp(constrain_aspect(anchor, cursor, r)),
            None => cursor,
        };
        self.selection = Some(Rect::from_corners(anchor, end));
    }

    fn resized(&self, sel: Rect, handle: Handle, fixed: Point, p: Point) -> Rect {
        let p = self.snap(p);
        match handle.axes() {
            // Top/bottom edge: horizontal extent is unchanged; `fixed.y` is the
            // opposite edge captured at mouse-down, so crossing over flips.
            (false, _) => Rect::from_corners(Point::new(sel.min_x(), fixed.y), Point::new(sel.max_x(), p.y)),
            // Left/right edge: vertical extent is unchanged.
            (_, false) => Rect::from_corners(Point::new(fixed.x, sel.min_y()), Point::new(p.x, sel.max_y())),
            // Corner: free, or aspect-locked while Shift is held.
            _ => {
                let moving = match self.aspect {
                    Some(r) => self.clamp(constrain_aspect(fixed, p, r)),
                    None => p,
                };
                Rect::from_corners(fixed, moving)
            }
        }
    }
}

/// Move `cursor` so the rect from `anchor` has width/height ratio `ratio`,
/// growing to cover the cursor's dominant axis.
pub fn constrain_aspect(anchor: Point, cursor: Point, ratio: f64) -> Point {
    let ratio = if ratio.is_finite() && ratio > 0.0 { ratio } else { 1.0 };
    let dx = cursor.x - anchor.x;
    let dy = cursor.y - anchor.y;
    let w = dx.abs().max(dy.abs() * ratio);
    let h = w / ratio;
    let sx = if dx < 0.0 { -1.0 } else { 1.0 };
    let sy = if dy < 0.0 { -1.0 } else { 1.0 };
    Point::new(anchor.x + sx * w, anchor.y + sy * h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> SelectionMachine {
        let displays = vec![Rect::new(0.0, 0.0, 1000.0, 800.0), Rect::new(1000.0, 0.0, 500.0, 800.0)];
        let windows = WindowList::new([WindowInfo {
            id: 42,
            frame: Rect::new(200.0, 200.0, 300.0, 200.0),
            app_name: "Notes".into(),
            title: "Untitled".into(),
            layer: 0,
        }]);
        SelectionMachine::new(Config::new(displays), windows)
    }

    fn drag(m: &mut SelectionMachine, from: Point, to: Point) {
        m.handle(Input::MouseDown { at: from, clicks: 1 });
        m.handle(Input::MouseDragged(from.lerp(to, 0.5)));
        m.handle(Input::MouseDragged(to));
        m.handle(Input::MouseUp(to));
    }

    #[test]
    fn hover_highlights_frontmost_window() {
        let mut m = machine();
        m.handle(Input::MouseMoved(Point::new(250.0, 250.0)));
        assert_eq!(m.hovered_window().map(|w| w.id), Some(42));
        m.handle(Input::MouseMoved(Point::new(50.0, 50.0)));
        assert!(m.hovered_window().is_none());
    }

    #[test]
    fn click_on_window_confirms_window() {
        let mut m = machine();
        m.handle(Input::MouseMoved(Point::new(250.0, 250.0)));
        m.handle(Input::MouseDown { at: Point::new(250.0, 250.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(251.0, 251.0))); // within tolerance
        match m.handle(Input::MouseUp(Point::new(251.0, 251.0))) {
            Outcome::Confirm(Target::Window(w)) => assert_eq!(w.id, 42),
            o => panic!("unexpected {o:?}"),
        }
    }

    #[test]
    fn click_on_desktop_confirms_display() {
        let mut m = machine();
        m.handle(Input::MouseDown { at: Point::new(1200.0, 50.0), clicks: 1 });
        assert_eq!(
            m.handle(Input::MouseUp(Point::new(1200.0, 50.0))),
            Outcome::Confirm(Target::Display(Rect::new(1000.0, 0.0, 500.0, 800.0)))
        );
    }

    #[test]
    fn drag_creates_selection_and_enter_confirms() {
        let mut m = machine();
        drag(&mut m, Point::new(20.0, 30.0), Point::new(120.0, 90.0));
        assert_eq!(m.phase(), Phase::Selected);
        assert_eq!(m.selection(), Some(Rect::new(20.0, 30.0, 100.0, 60.0)));
        assert_eq!(
            m.handle(Input::KeyDown(Key::Enter)),
            Outcome::Confirm(Target::Area(Rect::new(20.0, 30.0, 100.0, 60.0)))
        );
    }

    #[test]
    fn reverse_drag_normalises() {
        let mut m = machine();
        drag(&mut m, Point::new(120.0, 90.0), Point::new(20.0, 30.0));
        assert_eq!(m.selection(), Some(Rect::new(20.0, 30.0, 100.0, 60.0)));
    }

    #[test]
    fn drag_snaps_to_window_edges_unless_option() {
        let mut m = machine();
        drag(&mut m, Point::new(50.0, 50.0), Point::new(203.0, 397.0));
        assert_eq!(m.selection(), Some(Rect::new(50.0, 50.0, 150.0, 350.0)));

        let mut m = machine();
        m.handle(Input::ModifiersChanged(Mods { shift: false, option: true }));
        drag(&mut m, Point::new(50.0, 50.0), Point::new(203.0, 397.0));
        assert_eq!(m.selection(), Some(Rect::new(50.0, 50.0, 153.0, 347.0)));
    }

    #[test]
    fn tiny_drag_is_discarded() {
        let mut m = machine();
        m.handle(Input::MouseDown { at: Point::new(10.0, 10.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(14.0, 12.0)));
        m.handle(Input::MouseUp(Point::new(14.0, 12.0)));
        assert_eq!(m.selection(), None);
        assert!(matches!(m.phase(), Phase::Hovering { .. }));
    }

    #[test]
    fn selection_is_clamped_to_bounds() {
        let mut m = machine();
        drag(&mut m, Point::new(1400.0, 700.0), Point::new(2000.0, 2000.0));
        assert_eq!(m.selection(), Some(Rect::new(1400.0, 700.0, 100.0, 100.0)));
    }

    #[test]
    fn shift_locks_square_from_start() {
        let mut m = machine();
        m.handle(Input::ModifiersChanged(Mods { shift: true, option: true }));
        drag(&mut m, Point::new(10.0, 10.0), Point::new(110.0, 40.0));
        assert_eq!(m.selection(), Some(Rect::new(10.0, 10.0, 100.0, 100.0)));
    }

    #[test]
    fn shift_mid_drag_locks_current_ratio() {
        let mut m = machine();
        m.handle(Input::ModifiersChanged(Mods { shift: false, option: true }));
        m.handle(Input::MouseDown { at: Point::new(0.0, 0.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(160.0, 90.0)));
        m.handle(Input::ModifiersChanged(Mods { shift: true, option: true }));
        m.handle(Input::MouseDragged(Point::new(320.0, 100.0)));
        let s = m.selection().unwrap();
        assert!((s.width() / s.height() - 16.0 / 9.0).abs() < 1e-9);
        assert_eq!(s.width(), 320.0);
    }

    #[test]
    fn space_moves_band_while_dragging() {
        let mut m = machine();
        m.handle(Input::ModifiersChanged(Mods { shift: false, option: true }));
        m.handle(Input::MouseDown { at: Point::new(100.0, 100.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(150.0, 150.0)));
        m.handle(Input::KeyDown(Key::Space));
        m.handle(Input::MouseDragged(Point::new(170.0, 160.0)));
        assert_eq!(m.selection(), Some(Rect::new(120.0, 110.0, 50.0, 50.0)));
        m.handle(Input::KeyUp(Key::Space));
        m.handle(Input::MouseDragged(Point::new(200.0, 200.0)));
        assert_eq!(m.selection(), Some(Rect::new(120.0, 110.0, 80.0, 90.0)));
    }

    #[test]
    fn move_existing_selection_and_double_click() {
        let mut m = machine();
        m.handle(Input::ModifiersChanged(Mods { shift: false, option: true }));
        drag(&mut m, Point::new(10.0, 10.0), Point::new(110.0, 110.0));
        m.handle(Input::MouseMoved(Point::new(50.0, 50.0)));
        assert_eq!(m.cursor_hint(), CursorHint::Move);
        m.handle(Input::MouseDown { at: Point::new(50.0, 50.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(80.0, 70.0)));
        m.handle(Input::MouseUp(Point::new(80.0, 70.0)));
        assert_eq!(m.selection(), Some(Rect::new(40.0, 30.0, 100.0, 100.0)));
        let o = m.handle(Input::MouseDown { at: Point::new(60.0, 60.0), clicks: 2 });
        assert_eq!(o, Outcome::Confirm(Target::Area(Rect::new(40.0, 30.0, 100.0, 100.0))));
    }

    #[test]
    fn resize_corner_and_edge() {
        let mut m = machine();
        m.handle(Input::ModifiersChanged(Mods { shift: false, option: true }));
        drag(&mut m, Point::new(10.0, 10.0), Point::new(110.0, 110.0));
        // Bottom-right corner.
        m.handle(Input::MouseMoved(Point::new(110.0, 110.0)));
        assert_eq!(m.cursor_hint(), CursorHint::Resize(Handle::BottomRight));
        m.handle(Input::MouseDown { at: Point::new(110.0, 110.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(150.0, 130.0)));
        m.handle(Input::MouseUp(Point::new(150.0, 130.0)));
        assert_eq!(m.selection(), Some(Rect::new(10.0, 10.0, 140.0, 120.0)));
        // Left edge: only x changes.
        m.handle(Input::MouseDown { at: Point::new(10.0, 70.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(30.0, 300.0)));
        m.handle(Input::MouseUp(Point::new(30.0, 300.0)));
        assert_eq!(m.selection(), Some(Rect::new(30.0, 10.0, 120.0, 120.0)));
        // Top edge dragged past the bottom flips cleanly.
        m.handle(Input::MouseDown { at: Point::new(90.0, 10.0), clicks: 1 });
        m.handle(Input::MouseDragged(Point::new(0.0, 200.0)));
        m.handle(Input::MouseUp(Point::new(0.0, 200.0)));
        assert_eq!(m.selection(), Some(Rect::new(30.0, 130.0, 120.0, 70.0)));
    }

    #[test]
    fn arrow_nudges_and_resizes() {
        let mut m = machine();
        m.handle(Input::ModifiersChanged(Mods { shift: false, option: true }));
        drag(&mut m, Point::new(10.0, 10.0), Point::new(110.0, 110.0));
        m.handle(Input::ModifiersChanged(Mods::default()));
        m.handle(Input::KeyDown(Key::Arrow(Direction::Right)));
        m.handle(Input::KeyDown(Key::Arrow(Direction::Up)));
        assert_eq!(m.selection(), Some(Rect::new(11.0, 9.0, 100.0, 100.0)));
        m.handle(Input::ModifiersChanged(Mods { shift: true, option: false }));
        m.handle(Input::KeyDown(Key::Arrow(Direction::Left)));
        assert_eq!(m.selection(), Some(Rect::new(1.0, 9.0, 100.0, 100.0)));
        m.handle(Input::KeyDown(Key::Arrow(Direction::Left)));
        assert_eq!(m.selection().unwrap().origin.x, 0.0, "clamped at the edge");
        m.handle(Input::ModifiersChanged(Mods { shift: false, option: true }));
        m.handle(Input::KeyDown(Key::Arrow(Direction::Down)));
        assert_eq!(m.selection().unwrap().size, crate::Size::new(100.0, 101.0));
    }

    #[test]
    fn escape_cancels_and_c_copies() {
        let mut m = machine();
        assert_eq!(m.handle(Input::KeyDown(Key::Escape)), Outcome::Cancel);
        assert_eq!(m.handle(Input::KeyDown(Key::CopyColor)), Outcome::CopyColor);
        assert_eq!(m.handle(Input::KeyDown(Key::Enter)), Outcome::None);
    }

    #[test]
    fn enter_on_hovered_window() {
        let mut m = machine();
        m.handle(Input::MouseMoved(Point::new(300.0, 300.0)));
        assert!(matches!(m.handle(Input::KeyDown(Key::Enter)), Outcome::Confirm(Target::Window(_))));
    }

    #[test]
    fn constrain_aspect_quadrants() {
        let a = Point::new(100.0, 100.0);
        assert_eq!(constrain_aspect(a, Point::new(50.0, 90.0), 2.0), Point::new(50.0, 75.0));
        assert_eq!(constrain_aspect(a, Point::new(110.0, 200.0), 1.0), Point::new(200.0, 200.0));
        assert_eq!(constrain_aspect(a, a, f64::NAN), a);
    }
}

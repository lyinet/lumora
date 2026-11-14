//! The annotation document: z-ordered annotations, the crop, and an
//! undo/redo history built on the command pattern.
//!
//! Every mutation that should be undoable goes through [`Command`]. Live
//! gestures (dragging out a rectangle, moving a selection) mutate the
//! document *untracked* while the pointer moves and then [`Document::record`]
//! a single command describing the whole gesture, so one ⌘Z undoes one
//! gesture rather than sixty mouse-moved events.

use crate::geometry::{Point, Rect, Size};

use super::render::{render, RenderOp};
use super::shape::Shape;
use super::style::Style;

pub type AnnotationId = u64;

/// One annotation: identity, geometry, style.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub id: AnnotationId,
    pub shape: Shape,
    pub style: Style,
}

/// An undoable change.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Insert `item` at z-index `index`.
    Insert { index: usize, item: Annotation },
    /// Remove the annotation at `index` (which is `item`).
    Delete { index: usize, item: Annotation },
    /// Swap an annotation's contents (move, resize, restyle, edit text).
    Replace { before: Annotation, after: Annotation },
    /// Change an annotation's z-index.
    Reorder { id: AnnotationId, from: usize, to: usize },
    /// Set the crop rectangle (`None` = uncropped).
    Crop { before: Option<Rect>, after: Option<Rect> },
    /// Several commands as one undo step (applied in order).
    Batch(Vec<Command>),
}

impl Command {
    /// The command that undoes this one.
    pub fn inverse(&self) -> Command {
        match self {
            Command::Insert { index, item } => Command::Delete { index: *index, item: item.clone() },
            Command::Delete { index, item } => Command::Insert { index: *index, item: item.clone() },
            Command::Replace { before, after } => Command::Replace { before: after.clone(), after: before.clone() },
            Command::Reorder { id, from, to } => Command::Reorder { id: *id, from: *to, to: *from },
            Command::Crop { before, after } => Command::Crop { before: *after, after: *before },
            Command::Batch(cmds) => Command::Batch(cmds.iter().rev().map(Command::inverse).collect()),
        }
    }

    /// Whether this command does nothing (e.g. a click that didn't move).
    pub fn is_noop(&self) -> bool {
        match self {
            Command::Replace { before, after } => before == after,
            Command::Reorder { from, to, .. } => from == to,
            Command::Crop { before, after } => before == after,
            Command::Batch(cmds) => cmds.iter().all(Command::is_noop),
            _ => false,
        }
    }
}

/// Undo/redo stacks.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
}

/// Oldest steps are dropped beyond this.
pub const HISTORY_LIMIT: usize = 200;

impl History {
    fn push(&mut self, cmd: Command) {
        self.undo.push(cmd);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

/// The document being edited.
#[derive(Debug, Clone)]
pub struct Document {
    /// Image size in points.
    size: Size,
    /// Bottom → top.
    items: Vec<Annotation>,
    crop: Option<Rect>,
    next_id: AnnotationId,
    history: History,
    /// Bumped on every change, so views can cheaply tell they're stale.
    revision: u64,
}

impl Document {
    pub fn new(size: Size) -> Self {
        Document { size, items: Vec::new(), crop: None, next_id: 1, history: History::default(), revision: 0 }
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// The full (uncropped) image rect.
    pub fn canvas(&self) -> Rect {
        Rect { origin: Point::ZERO, size: self.size }
    }

    pub fn crop(&self) -> Option<Rect> {
        self.crop
    }

    /// What is exported: the crop, or the whole canvas.
    pub fn visible_rect(&self) -> Rect {
        self.crop.unwrap_or_else(|| self.canvas())
    }

    pub fn items(&self) -> &[Annotation] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.crop.is_none()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.items.iter().find(|a| a.id == id)
    }

    pub fn index_of(&self, id: AnnotationId) -> Option<usize> {
        self.items.iter().position(|a| a.id == id)
    }

    /// A fresh id (never reused, so undo/redo keep identities stable).
    pub fn new_id(&mut self) -> AnnotationId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The topmost annotation under `p`.
    pub fn hit_test(&self, p: Point, slop: f64) -> Option<AnnotationId> {
        self.items.iter().rev().find(|a| a.shape.hit(p, &a.style, slop)).map(|a| a.id)
    }

    // -- steps ---------------------------------------------------------------

    /// The badge number of step `id`: its rank among steps by creation order
    /// (ids increase monotonically), starting at 1.
    pub fn step_number(&self, id: AnnotationId) -> Option<u32> {
        let a = self.get(id)?;
        if !matches!(a.shape, Shape::Step { .. }) {
            return None;
        }
        let rank = self.items.iter().filter(|b| matches!(b.shape, Shape::Step { .. }) && b.id < id).count();
        Some(rank as u32 + 1)
    }

    /// The number the next step badge will get.
    pub fn next_step_number(&self) -> u32 {
        self.items.iter().filter(|a| matches!(a.shape, Shape::Step { .. })).count() as u32 + 1
    }

    // -- rendering -------------------------------------------------------------

    /// Display list for one annotation.
    pub fn render_item(&self, id: AnnotationId) -> Vec<RenderOp> {
        match self.get(id) {
            Some(a) => render(a, self.canvas(), self.step_number(id)),
            None => Vec::new(),
        }
    }

    /// Display list for the whole document, bottom → top.
    pub fn render_all(&self) -> Vec<(AnnotationId, Vec<RenderOp>)> {
        self.items.iter().map(|a| (a.id, render(a, self.canvas(), self.step_number(a.id)))).collect()
    }

    // -- commands --------------------------------------------------------------

    /// Apply a command and push it on the undo stack. Returns `false` (and
    /// records nothing) if it could not be applied.
    pub fn execute(&mut self, cmd: Command) -> bool {
        if cmd.is_noop() {
            return false;
        }
        if !self.apply(&cmd) {
            return false;
        }
        self.history.push(cmd);
        true
    }

    /// Push a command whose effect is *already* in the document (the end
    /// of a live gesture). No-ops are dropped.
    pub fn record(&mut self, cmd: Command) {
        if !cmd.is_noop() {
            self.history.push(cmd);
            self.revision += 1;
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(cmd) = self.history.undo.pop() else { return false };
        let ok = self.apply(&cmd.inverse());
        debug_assert!(ok, "undo of {cmd:?} failed");
        self.history.redo.push(cmd);
        ok
    }

    pub fn redo(&mut self) -> bool {
        let Some(cmd) = self.history.redo.pop() else { return false };
        let ok = self.apply(&cmd);
        debug_assert!(ok, "redo of {cmd:?} failed");
        self.history.undo.push(cmd);
        ok
    }

    /// Apply without touching history.
    fn apply(&mut self, cmd: &Command) -> bool {
        let ok = match cmd {
            Command::Insert { index, item } => {
                if *index > self.items.len() || self.get(item.id).is_some() {
                    false
                } else {
                    self.items.insert(*index, item.clone());
                    self.next_id = self.next_id.max(item.id + 1);
                    true
                }
            }
            Command::Delete { index, item } => {
                if self.items.get(*index).map(|a| a.id) == Some(item.id) {
                    self.items.remove(*index);
                    true
                } else {
                    false
                }
            }
            Command::Replace { before, after } => match self.index_of(before.id) {
                Some(i) if before.id == after.id => {
                    self.items[i] = after.clone();
                    true
                }
                _ => false,
            },
            Command::Reorder { id, from, to } => {
                if self.index_of(*id) == Some(*from) && *to < self.items.len() {
                    let item = self.items.remove(*from);
                    self.items.insert(*to, item);
                    true
                } else {
                    false
                }
            }
            Command::Crop { after, .. } => {
                self.crop = after.and_then(|r| r.normalized().intersection(&self.canvas()));
                true
            }
            Command::Batch(cmds) => {
                // All-or-nothing: roll back what was applied if one fails.
                let mut done = Vec::new();
                for c in cmds {
                    if self.apply(c) {
                        done.push(c);
                    } else {
                        for d in done.into_iter().rev() {
                            self.apply(&d.inverse());
                        }
                        return false;
                    }
                }
                true
            }
        };
        if ok {
            self.revision += 1;
        }
        ok
    }

    // -- conveniences (each one undo step) ---------------------------------------

    /// Add an annotation on top. Returns its id.
    pub fn add(&mut self, shape: Shape, style: Style) -> AnnotationId {
        let id = self.new_id();
        let index = self.items.len();
        self.execute(Command::Insert { index, item: Annotation { id, shape, style } });
        id
    }

    pub fn remove(&mut self, id: AnnotationId) -> bool {
        match (self.index_of(id), self.get(id).cloned()) {
            (Some(index), Some(item)) => self.execute(Command::Delete { index, item }),
            _ => false,
        }
    }

    /// Replace an annotation's contents as one undo step.
    pub fn update(&mut self, id: AnnotationId, f: impl FnOnce(&mut Annotation)) -> bool {
        let Some(before) = self.get(id).cloned() else { return false };
        let mut after = before.clone();
        f(&mut after);
        after.id = id;
        self.execute(Command::Replace { before, after })
    }

    pub fn bring_to_front(&mut self, id: AnnotationId) -> bool {
        match self.index_of(id) {
            Some(from) => self.execute(Command::Reorder { id, from, to: self.items.len() - 1 }),
            None => false,
        }
    }

    pub fn send_to_back(&mut self, id: AnnotationId) -> bool {
        match self.index_of(id) {
            Some(from) => self.execute(Command::Reorder { id, from, to: 0 }),
            None => false,
        }
    }

    pub fn set_crop(&mut self, crop: Option<Rect>) -> bool {
        let after = crop.and_then(|r| r.normalized().intersection(&self.canvas())).filter(|r| *r != self.canvas());
        self.execute(Command::Crop { before: self.crop, after })
    }

    /// Delete everything (one undo step).
    pub fn clear(&mut self) -> bool {
        let cmds: Vec<Command> = self
            .items
            .iter()
            .enumerate()
            .rev()
            .map(|(index, item)| Command::Delete { index, item: item.clone() })
            .collect();
        if cmds.is_empty() {
            return false;
        }
        self.execute(Command::Batch(cmds))
    }

    // -- untracked edits for live gestures ----------------------------------------

    /// Insert without history (start of a creation drag).
    pub fn insert_untracked(&mut self, item: Annotation) {
        if self.get(item.id).is_none() {
            self.next_id = self.next_id.max(item.id + 1);
            self.items.push(item);
            self.revision += 1;
        }
    }

    /// Replace without history (during a drag).
    pub fn set_untracked(&mut self, item: Annotation) {
        if let Some(i) = self.index_of(item.id) {
            if self.items[i] != item {
                self.items[i] = item;
                self.revision += 1;
            }
        }
    }

    /// Remove without history (a creation drag that was too small).
    pub fn remove_untracked(&mut self, id: AnnotationId) -> Option<Annotation> {
        let i = self.index_of(id)?;
        self.revision += 1;
        Some(self.items.remove(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::style::PALETTE;

    fn rect_shape(x: f64) -> Shape {
        Shape::Rect { rect: Rect::new(x, 0.0, 10.0, 10.0), rounded: false }
    }

    fn doc() -> Document {
        Document::new(Size::new(200.0, 100.0))
    }

    #[test]
    fn add_undo_redo() {
        let mut d = doc();
        let a = d.add(rect_shape(0.0), Style::default());
        let b = d.add(rect_shape(20.0), Style::default());
        assert_eq!(d.items().len(), 2);
        assert!(d.undo());
        assert_eq!(d.items().iter().map(|a| a.id).collect::<Vec<_>>(), vec![a]);
        assert!(d.redo());
        assert_eq!(d.items().iter().map(|a| a.id).collect::<Vec<_>>(), vec![a, b]);
        assert!(!d.redo());
        assert!(d.undo() && d.undo());
        assert!(d.items().is_empty());
        assert!(!d.undo());
    }

    #[test]
    fn new_command_clears_redo() {
        let mut d = doc();
        d.add(rect_shape(0.0), Style::default());
        d.undo();
        assert!(d.history().can_redo());
        d.add(rect_shape(5.0), Style::default());
        assert!(!d.history().can_redo());
    }

    #[test]
    fn ids_are_never_reused_across_undo() {
        let mut d = doc();
        let a = d.add(rect_shape(0.0), Style::default());
        d.undo();
        let b = d.add(rect_shape(0.0), Style::default());
        assert_ne!(a, b);
    }

    #[test]
    fn delete_restores_at_same_z_index() {
        let mut d = doc();
        let ids: Vec<_> = (0..3).map(|i| d.add(rect_shape(i as f64 * 20.0), Style::default())).collect();
        assert!(d.remove(ids[1]));
        assert_eq!(d.items().len(), 2);
        d.undo();
        assert_eq!(d.items().iter().map(|a| a.id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn update_is_one_step_and_noop_updates_are_not_recorded() {
        let mut d = doc();
        let a = d.add(rect_shape(0.0), Style::default());
        let n = d.history().undo_len();
        assert!(!d.update(a, |_| {}));
        assert_eq!(d.history().undo_len(), n);
        assert!(d.update(a, |x| x.style.color = PALETTE[2]));
        assert_eq!(d.get(a).unwrap().style.color, PALETTE[2]);
        d.undo();
        assert_eq!(d.get(a).unwrap().style.color, PALETTE[0]);
    }

    #[test]
    fn z_order_and_hit_testing_prefers_topmost() {
        let mut d = doc();
        let s = Style::default();
        let a = d.add(Shape::Redact { rect: Rect::new(0.0, 0.0, 50.0, 50.0), mode: Default::default() }, s);
        let b = d.add(Shape::Redact { rect: Rect::new(25.0, 25.0, 50.0, 50.0), mode: Default::default() }, s);
        assert_eq!(d.hit_test(Point::new(30.0, 30.0), 0.0), Some(b));
        assert!(d.send_to_back(b));
        assert_eq!(d.hit_test(Point::new(30.0, 30.0), 0.0), Some(a));
        assert!(d.bring_to_front(b));
        assert_eq!(d.hit_test(Point::new(30.0, 30.0), 0.0), Some(b));
        d.undo();
        assert_eq!(d.items()[0].id, b);
        assert_eq!(d.hit_test(Point::new(150.0, 90.0), 0.0), None);
    }

    #[test]
    fn steps_renumber_on_delete_and_undo() {
        let mut d = doc();
        let s = Style::default();
        let step = |x| Shape::Step { center: Point::new(x, 10.0) };
        let one = d.add(step(10.0), s);
        d.add(rect_shape(50.0), s);
        let two = d.add(step(30.0), s);
        let three = d.add(step(60.0), s);
        assert_eq!(d.next_step_number(), 4);
        assert_eq!([d.step_number(one), d.step_number(two), d.step_number(three)], [Some(1), Some(2), Some(3)]);
        d.remove(two);
        assert_eq!(d.step_number(three), Some(2));
        assert_eq!(d.next_step_number(), 3);
        d.undo();
        assert_eq!(d.step_number(three), Some(3));
        // Z-order changes do not renumber.
        d.send_to_back(three);
        assert_eq!(d.step_number(three), Some(3));
        assert_eq!(d.step_number(d.items()[2].id), None, "a rect has no number");
    }

    #[test]
    fn crop_is_clamped_and_undoable() {
        let mut d = doc();
        assert!(d.set_crop(Some(Rect::new(-10.0, 10.0, 100.0, 200.0))));
        assert_eq!(d.crop(), Some(Rect::new(0.0, 10.0, 90.0, 90.0)));
        assert_eq!(d.visible_rect(), Rect::new(0.0, 10.0, 90.0, 90.0));
        // Cropping to the whole canvas means "uncropped".
        assert!(d.set_crop(Some(d.canvas())));
        assert_eq!(d.crop(), None);
        d.undo();
        assert_eq!(d.crop(), Some(Rect::new(0.0, 10.0, 90.0, 90.0)));
        d.undo();
        assert_eq!(d.crop(), None);
        assert!(!d.set_crop(None), "no-op");
    }

    #[test]
    fn live_gestures_record_one_step() {
        let mut d = doc();
        let id = d.new_id();
        let mut item = Annotation { id, shape: rect_shape(0.0), style: Style::default() };
        d.insert_untracked(item.clone());
        for x in 1..20 {
            item.shape = rect_shape(x as f64);
            d.set_untracked(item.clone());
        }
        d.record(Command::Insert { index: 0, item: item.clone() });
        assert_eq!(d.history().undo_len(), 1);
        d.undo();
        assert!(d.items().is_empty());
        d.redo();
        assert_eq!(d.get(id), Some(&item));
    }

    #[test]
    fn batch_is_atomic() {
        let mut d = doc();
        let a = d.add(rect_shape(0.0), Style::default());
        let item = d.get(a).unwrap().clone();
        let bad = Command::Batch(vec![
            Command::Delete { index: 0, item: item.clone() },
            Command::Delete { index: 0, item: item.clone() }, // already gone
        ]);
        assert!(!d.execute(bad));
        assert_eq!(d.items().len(), 1, "rolled back");
        assert!(d.clear());
        assert!(d.items().is_empty());
        d.undo();
        assert_eq!(d.items().len(), 1);
        assert!(!doc().clear());
    }

    #[test]
    fn inverse_is_an_involution() {
        let item = Annotation { id: 7, shape: rect_shape(0.0), style: Style::default() };
        let cmds = [
            Command::Insert { index: 0, item: item.clone() },
            Command::Reorder { id: 7, from: 0, to: 2 },
            Command::Crop { before: None, after: Some(Rect::new(0.0, 0.0, 1.0, 1.0)) },
            Command::Batch(vec![Command::Insert { index: 0, item }, Command::Reorder { id: 7, from: 1, to: 0 }]),
        ];
        for c in cmds {
            assert_eq!(c.inverse().inverse(), c);
        }
    }

    #[test]
    fn history_is_bounded() {
        let mut d = doc();
        for i in 0..(HISTORY_LIMIT + 10) {
            d.add(rect_shape(i as f64), Style::default());
        }
        assert_eq!(d.history().undo_len(), HISTORY_LIMIT);
    }

    #[test]
    fn render_all_is_bottom_to_top_and_revision_moves() {
        let mut d = doc();
        let r0 = d.revision();
        let a = d.add(rect_shape(0.0), Style::default());
        let b = d.add(Shape::Step { center: Point::new(5.0, 5.0) }, Style::default());
        assert!(d.revision() > r0);
        let all = d.render_all();
        assert_eq!(all.iter().map(|x| x.0).collect::<Vec<_>>(), vec![a, b]);
        assert!(!d.render_item(b).is_empty());
        assert!(d.render_item(999).is_empty());
    }
}

//! The set of committed selections plus per-op undo and redo stacks.

use crate::geometry::{Point, ResizeHandle, Shape};

/// What a mouse-press on the overlay grabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabKind {
    /// Inside a shape: drag moves it.
    Move,
    /// On a shape's border: drag resizes it.
    Resize(ResizeHandle),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub shape: Shape,
    pub label: String,
    /// Index into the session's monitor list.
    pub monitor: usize,
    /// Rotation in degrees (`0..360`) about the shape's bbox center.
    pub rot_deg: i32,
}

impl Selection {
    pub const fn new(shape: Shape, monitor: usize) -> Self {
        Self {
            shape,
            label: String::new(),
            monitor,
            rot_deg: 0,
        }
    }
}

/// State-restoring operations. Applying one yields its own inverse, which
/// is what lets a single engine drive both undo and redo.
#[derive(Debug, Clone)]
enum UndoOp {
    RemoveLast,
    Restack { from: usize, to: usize },
    RemoveAt { index: usize },
    Reinsert { index: usize, selection: Selection },
    RestoreShape { index: usize, shape: Shape },
    RestoreLabel { index: usize, label: String },
    RestoreRotation { index: usize, deg: i32 },
}

#[derive(Debug, Default)]
pub struct SelectionSet {
    items: Vec<Selection>,
    undo: Vec<UndoOp>,
    redo: Vec<UndoOp>,
}

impl SelectionSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// A set pre-populated with existing selections — restoring a saved
    /// session. Seeding is not an edit: no undo history is created, so
    /// undo in a resumed session stops at the resume point instead of
    /// dismantling the session it reopened.
    pub fn seed(items: Vec<Selection>) -> Self {
        Self {
            items,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn items(&self) -> &[Selection] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Selection> {
        self.items.get(index)
    }

    /// Topmost selection on `monitor` containing `p` (most recent wins),
    /// respecting each selection's rotation.
    pub fn hit_topmost(&self, monitor: usize, p: Point) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| s.monitor == monitor && s.shape.hit_test_rotated(s.rot_deg, p))
            .map(|(i, _)| i)
    }

    /// What a press at `p` grabs, topmost selection first: a border within
    /// `tolerance` resizes, an interior moves. A shape's border outranks
    /// its own interior; a topmost shape outranks anything beneath it.
    pub fn grab_topmost(
        &self,
        monitor: usize,
        p: Point,
        tolerance: i32,
    ) -> Option<(usize, GrabKind)> {
        self.items
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, s)| s.monitor == monitor)
            .find_map(|(i, s)| {
                if let Some(handle) = s.shape.resize_grab_rotated(s.rot_deg, p, tolerance) {
                    return Some((i, GrabKind::Resize(handle)));
                }
                if s.shape.hit_test_rotated(s.rot_deg, p) {
                    return Some((i, GrabKind::Move));
                }
                None
            })
    }

    /// Rotate a selection by `delta` degrees about its bbox center; circles
    /// are inherently rotation-free and are left untouched. Returns the new
    /// rotation, or `None` if nothing changed.
    pub fn rotate(&mut self, index: usize, delta: i32) -> Option<i32> {
        let s = self.items.get_mut(index)?;
        if matches!(s.shape, Shape::Circle { .. }) || delta == 0 {
            return None;
        }
        let previous = s.rot_deg;
        s.rot_deg = crate::geometry::normalize_deg(s.rot_deg + delta);
        self.undo.push(UndoOp::RestoreRotation {
            index,
            deg: previous,
        });
        self.redo.clear();
        Some(self.items[index].rot_deg)
    }

    pub fn add(&mut self, selection: Selection) {
        self.items.push(selection);
        self.undo.push(UndoOp::RemoveLast);
        self.redo.clear();
    }

    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        let selection = self.items.remove(index);
        self.undo.push(UndoOp::Reinsert { index, selection });
        self.redo.clear();
        true
    }

    /// Update a shape mid-drag without recording undo history; pair with
    /// `commit_move` when the drag ends.
    pub fn set_shape_live(&mut self, index: usize, shape: Shape) {
        if let Some(s) = self.items.get_mut(index) {
            s.shape = shape;
        }
    }

    /// Record a completed move: `original` is the shape as it was when the
    /// drag started. No-op if the shape ended up back where it began.
    /// Returns whether the shape actually changed — a no-op move records
    /// nothing and shouldn't dirty the session.
    pub fn commit_move(&mut self, index: usize, original: Shape) -> bool {
        let Some(s) = self.items.get(index) else {
            return false;
        };
        if s.shape == original {
            return false;
        }
        self.undo.push(UndoOp::RestoreShape {
            index,
            shape: original,
        });
        self.redo.clear();
        true
    }

    /// Returns whether the label actually changed — committing the editor
    /// without edits records nothing.
    pub fn set_label(&mut self, index: usize, label: String) -> bool {
        let Some(s) = self.items.get_mut(index) else {
            return false;
        };
        if s.label == label {
            return false;
        }
        let previous = std::mem::replace(&mut s.label, label);
        self.undo.push(UndoOp::RestoreLabel {
            index,
            label: previous,
        });
        self.redo.clear();
        true
    }

    /// Send the topmost shape under `p` to the bottom of the stack, so
    /// the next grab reaches what was beneath it. Returns false when
    /// fewer than two shapes are under the point. Stacking order is also
    /// save order, so this is an undoable edit like any other.
    pub fn cycle_at(&mut self, monitor: usize, p: Point) -> bool {
        let hits: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, s)| s.monitor == monitor && s.shape.hit_test_rotated(s.rot_deg, p))
            .map(|(i, _)| i)
            .collect();
        let (Some(&bottom), Some(&top)) = (hits.first(), hits.last()) else {
            return false;
        };
        if bottom == top {
            return false;
        }
        let selection = self.items.remove(top);
        self.items.insert(bottom, selection);
        self.undo.push(UndoOp::Restack {
            from: bottom,
            to: top,
        });
        self.redo.clear();
        true
    }

    /// Undo the most recent operation. Returns false when there is nothing
    /// to undo.
    pub fn undo(&mut self) -> bool {
        let Some(op) = self.undo.pop() else {
            return false;
        };
        if let Some(inverse) = self.apply(op) {
            self.redo.push(inverse);
        }
        true
    }

    /// Re-apply the most recently undone operation. Returns false when
    /// there is nothing to redo — any new edit empties the redo branch.
    pub fn redo(&mut self) -> bool {
        let Some(op) = self.redo.pop() else {
            return false;
        };
        if let Some(inverse) = self.apply(op) {
            self.undo.push(inverse);
        }
        true
    }

    /// Apply a state-restoring op and return its inverse — the op that
    /// puts things back exactly as they were before this call.
    fn apply(&mut self, op: UndoOp) -> Option<UndoOp> {
        match op {
            UndoOp::Restack { from, to } => {
                if from >= self.items.len() {
                    return None;
                }
                let selection = self.items.remove(from);
                let to = to.min(self.items.len());
                self.items.insert(to, selection);
                Some(UndoOp::Restack { from: to, to: from })
            }
            UndoOp::RemoveLast => {
                let selection = self.items.pop()?;
                Some(UndoOp::Reinsert {
                    index: self.items.len(),
                    selection,
                })
            }
            UndoOp::RemoveAt { index } => {
                if index >= self.items.len() {
                    return None;
                }
                let selection = self.items.remove(index);
                Some(UndoOp::Reinsert { index, selection })
            }
            UndoOp::Reinsert { index, selection } => {
                let index = index.min(self.items.len());
                self.items.insert(index, selection);
                Some(UndoOp::RemoveAt { index })
            }
            UndoOp::RestoreShape { index, shape } => {
                let s = self.items.get_mut(index)?;
                let previous = std::mem::replace(&mut s.shape, shape);
                Some(UndoOp::RestoreShape {
                    index,
                    shape: previous,
                })
            }
            UndoOp::RestoreLabel { index, label } => {
                let s = self.items.get_mut(index)?;
                let previous = std::mem::replace(&mut s.label, label);
                Some(UndoOp::RestoreLabel {
                    index,
                    label: previous,
                })
            }
            UndoOp::RestoreRotation { index, deg } => {
                let s = self.items.get_mut(index)?;
                let previous = std::mem::replace(&mut s.rot_deg, deg);
                Some(UndoOp::RestoreRotation {
                    index,
                    deg: previous,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn rect_at(x: i32) -> Shape {
        Shape::Rect(Rect::new(x, 0, 100, 100))
    }

    #[test]
    fn hit_topmost_prefers_most_recent() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        set.add(Selection::new(rect_at(50), 0));
        // (60, 10) is inside both; the later one wins.
        assert_eq!(set.hit_topmost(0, Point::new(60, 10)), Some(1));
        assert_eq!(set.hit_topmost(0, Point::new(10, 10)), Some(0));
        assert_eq!(set.hit_topmost(0, Point::new(500, 500)), None);
    }

    #[test]
    fn hit_topmost_filters_by_monitor() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 1));
        assert_eq!(set.hit_topmost(0, Point::new(10, 10)), None);
        assert_eq!(set.hit_topmost(1, Point::new(10, 10)), Some(0));
    }

    #[test]
    fn undo_add_removes_it() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        assert!(set.undo());
        assert!(set.is_empty());
    }

    #[test]
    fn undo_delete_reinserts_at_original_position() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        set.add(Selection::new(rect_at(200), 0));
        set.add(Selection::new(rect_at(400), 0));
        assert!(set.delete(1));
        assert_eq!(set.len(), 2);
        assert!(set.undo());
        assert_eq!(set.items()[1].shape, rect_at(200));
    }

    #[test]
    fn undo_move_restores_original_shape() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        let original = set.items()[0].shape.clone();
        set.set_shape_live(0, rect_at(300));
        set.commit_move(0, original.clone());
        assert!(set.undo());
        assert_eq!(set.items()[0].shape, original);
    }

    #[test]
    fn commit_move_without_change_records_nothing() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        let original = set.items()[0].shape.clone();
        set.commit_move(0, original.clone());
        // Only the add is on the stack: one undo empties the set.
        assert!(set.undo());
        assert!(set.is_empty());
        assert!(!set.undo());
    }

    #[test]
    fn noop_label_and_move_record_nothing() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        // Committing the same label (including empty -> empty) is a no-op.
        assert!(!set.set_label(0, String::new()));
        assert!(set.set_label(0, "named".into()));
        assert!(!set.set_label(0, "named".into()));
        // An unchanged move is a no-op.
        let original = set.items()[0].shape.clone();
        assert!(!set.commit_move(0, original));
        // Stack: label change, then the add — exactly two undos.
        assert!(set.undo());
        assert_eq!(set.items()[0].label, "");
        assert!(set.undo());
        assert!(!set.undo());
    }

    #[test]
    fn undo_label_restores_previous_text() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        set.set_label(0, "first".into());
        set.set_label(0, "second".into());
        assert!(set.undo());
        assert_eq!(set.items()[0].label, "first");
        assert!(set.undo());
        assert_eq!(set.items()[0].label, "");
    }

    #[test]
    fn cycling_overlap_reaches_the_shape_beneath_and_undoes() {
        let mut set = SelectionSet::new();
        let mut below = Selection::new(Shape::Rect(Rect::new(0, 0, 100, 100)), 0);
        below.label = "below".into();
        let mut above = Selection::new(Shape::Rect(Rect::new(10, 10, 50, 50)), 0);
        above.label = "above".into();
        set.add(below);
        set.add(above);
        let p = Point::new(20, 20);
        assert_eq!(set.items()[set.hit_topmost(0, p).unwrap()].label, "above");

        assert!(set.cycle_at(0, p));
        assert_eq!(
            set.items()[set.hit_topmost(0, p).unwrap()].label,
            "below",
            "the shape beneath is now grabbable"
        );
        // Cycling again comes back around.
        assert!(set.cycle_at(0, p));
        assert_eq!(set.items()[set.hit_topmost(0, p).unwrap()].label, "above");
        // And the whole dance unwinds.
        assert!(set.undo());
        assert!(set.undo());
        assert_eq!(set.items()[set.hit_topmost(0, p).unwrap()].label, "above");
        assert_eq!(set.items()[0].label, "below", "original order restored");
    }

    #[test]
    fn get_returns_the_selection_or_nothing() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(Shape::Rect(Rect::new(1, 1, 2, 2)), 0));
        assert!(set.get(0).is_some());
        assert!(set.get(1).is_none());
    }

    #[test]
    fn cycling_needs_at_least_two_shapes_under_the_point() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(Shape::Rect(Rect::new(0, 0, 10, 10)), 0));
        assert!(
            !set.cycle_at(0, Point::new(5, 5)),
            "one shape: nothing to cycle"
        );
        assert!(
            !set.cycle_at(0, Point::new(50, 50)),
            "no shape: nothing to cycle"
        );
    }

    #[test]
    fn seeding_restores_items_without_undo_history() {
        let mut sel = Selection::new(Shape::Rect(Rect::new(1, 2, 3, 4)), 0);
        sel.label = "kept".into();
        sel.rot_deg = 30;
        let mut set = SelectionSet::seed(vec![sel]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.items()[0].label, "kept");
        assert!(!set.undo(), "the resume point is the floor of history");
        // Edits after seeding behave normally.
        set.delete(0);
        assert!(set.undo());
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn redo_reapplies_undone_edits_across_every_op_kind() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(Shape::Rect(Rect::new(0, 0, 10, 10)), 0));
        set.set_label(0, "a".into());
        set.rotate(0, 45);
        set.set_shape_live(0, Shape::Rect(Rect::new(5, 5, 10, 10)));
        set.commit_move(0, Shape::Rect(Rect::new(0, 0, 10, 10)));
        set.delete(0);
        while set.undo() {}
        assert!(set.is_empty(), "everything unwinds");
        while set.redo() {}
        assert!(set.is_empty(), "the final delete replays too");
        // One more undo brings the deleted selection back fully formed.
        assert!(set.undo());
        let s = &set.items()[0];
        assert_eq!(s.label, "a");
        assert_eq!(s.rot_deg, 45);
        assert_eq!(s.shape, Shape::Rect(Rect::new(5, 5, 10, 10)));
    }

    #[test]
    fn a_new_edit_empties_the_redo_branch() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(Shape::Rect(Rect::new(0, 0, 10, 10)), 0));
        assert!(set.undo());
        set.add(Selection::new(Shape::Rect(Rect::new(9, 9, 5, 5)), 0));
        assert!(!set.redo(), "a new edit forked history; redo is gone");
    }

    #[test]
    fn redo_with_nothing_undone_is_false() {
        let mut set = SelectionSet::new();
        assert!(!set.redo());
        set.add(Selection::new(Shape::Rect(Rect::new(0, 0, 4, 4)), 0));
        assert!(!set.redo(), "un-undone edits leave nothing to redo");
    }

    #[test]
    fn undo_is_lifo_across_mixed_ops() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        set.set_label(0, "a".into());
        set.delete(0);
        assert!(set.undo()); // reinsert with label "a"
        assert_eq!(set.items()[0].label, "a");
        assert!(set.undo()); // label back to ""
        assert_eq!(set.items()[0].label, "");
        assert!(set.undo()); // remove the add
        assert!(set.is_empty());
        assert!(!set.undo());
    }

    #[test]
    fn delete_out_of_range_is_false() {
        let mut set = SelectionSet::new();
        assert!(!set.delete(0));
    }

    #[test]
    fn rotate_records_undo_and_skips_circles() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0));
        set.add(Selection::new(Shape::Circle { cx: 0, cy: 0, r: 9 }, 0));

        assert_eq!(set.rotate(0, 15), Some(15));
        assert_eq!(set.rotate(0, -30), Some(345));
        assert_eq!(set.rotate(1, 15), None, "circles don't rotate");
        assert_eq!(set.rotate(0, 0), None, "zero delta is a no-op");

        assert!(set.undo());
        assert_eq!(set.items()[0].rot_deg, 15);
        assert!(set.undo());
        assert_eq!(set.items()[0].rot_deg, 0);
    }

    #[test]
    fn rotated_selection_hit_follows_rotation() {
        let mut set = SelectionSet::new();
        // Wide flat rect at (0,0) 100x20... rect_at gives 100x100; use a
        // custom flat one so rotation visibly changes the hit region.
        set.add(Selection::new(Shape::Rect(Rect::new(100, 100, 200, 20)), 0));
        set.rotate(0, 90);
        assert_eq!(set.hit_topmost(0, Point::new(200, 30)), Some(0));
        assert_eq!(set.hit_topmost(0, Point::new(290, 110)), None);
    }

    #[test]
    fn grab_prefers_border_resize_over_interior_move() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0)); // (0,0,100,100)
        // On the left edge: resize.
        let Some((0, GrabKind::Resize(_))) = set.grab_topmost(0, Point::new(1, 50), 5) else {
            panic!("expected a resize grab on the edge");
        };
        // Deep inside: move.
        assert_eq!(
            set.grab_topmost(0, Point::new(50, 50), 5),
            Some((0, GrabKind::Move))
        );
        // Far away: nothing.
        assert_eq!(set.grab_topmost(0, Point::new(500, 500), 5), None);
    }

    #[test]
    fn grab_topmost_shape_shadows_lower_border() {
        let mut set = SelectionSet::new();
        set.add(Selection::new(rect_at(0), 0)); // right edge at x=100
        set.add(Selection::new(Shape::Rect(Rect::new(50, 0, 200, 100)), 0)); // covers it
        // (100, 50) is the lower rect's edge but the upper rect's interior:
        // the topmost shape wins with a move grab.
        assert_eq!(
            set.grab_topmost(0, Point::new(100, 50), 5),
            Some((1, GrabKind::Move))
        );
    }
}

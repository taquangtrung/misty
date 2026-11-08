//! A [`SplitTree`]: panes laid out by splitting an area in two, again and
//! again, side by side or one above the other. Each split divides its area
//! by a ratio, kept between 0.1 and 0.9 so neither side vanishes, with a
//! divider between the two sides.
//!
//! The tree only knows pane ids; what a pane shows is the app's. It lays
//! the panes out in a [`Rect`] as a [`SplitLayout`], and moves focus by
//! direction from that geometry, so "the pane to the right" is whatever is
//! to the right now.

use std::mem;

use super::geometry::{Point, Rect};

// ========================================================================
// Constants
// ========================================================================

/// The smallest share of its area a split gives either side.
const MIN_RATIO: f32 = 0.1;

/// The largest share of its area a split gives either side.
const MAX_RATIO: f32 = 0.9;

/// The share a new split gives each side.
const EVEN_RATIO: f32 = 0.5;

/// How far, in pixels, a pane's edge may reach past the focused pane's
/// edge and still count as lying beyond it, for rounding.
const EDGE_TOLERANCE: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// Panes laid out by repeated splits, with one pane focused. A tree always
/// has at least one pane.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitTree {
    /// The pane keyboard input goes to.
    focused: PaneId,
    /// The number the next pane or split id gets.
    next_id: u64,
    /// The whole tree.
    root: Node,
}

/// Where each pane and divider of a [`SplitTree`] is, in pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplitLayout {
    /// The dividers between the two sides of each split, outermost first.
    pub dividers: Vec<Divider>,
    /// The panes, in tree order: left to right and top to bottom within
    /// each split.
    pub panes: Vec<PaneRect>,
}

/// Where one pane is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneRect {
    /// Which pane.
    pub id: PaneId,
    /// Its area.
    pub rect: Rect,
}

/// The divider of one split: the line between its two sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divider {
    /// The whole area the split divides, both sides and the divider.
    pub area: Rect,
    /// How the split places its sides.
    pub axis: Axis,
    /// Which split.
    pub id: SplitId,
    /// The divider itself.
    pub rect: Rect,
}

/// Names one pane of a [`SplitTree`] for as long as it is open. Ids are
/// never reused within a tree.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PaneId(u64);

/// Names one split of a [`SplitTree`] for as long as both its sides are
/// open.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SplitId(u64);

/// A part of the tree.
#[derive(Clone, Debug, PartialEq)]
enum Node {
    /// A pane.
    Pane(PaneId),
    /// An area split in two.
    Split(Split),
}

/// An area split in two.
#[derive(Clone, Debug, PartialEq)]
struct Split {
    /// How the sides are placed.
    axis: Axis,
    /// The left or top side.
    first: Box<Node>,
    /// Which split this is.
    id: SplitId,
    /// The share of the area, less the divider, the first side takes.
    ratio: f32,
    /// The right or bottom side.
    second: Box<Node>,
}

/// How a split places its two sides.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Axis {
    /// Side by side, divided by an upright line.
    Horizontal,
    /// One above the other, divided by a level line.
    Vertical,
}

/// A direction to move focus in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Direction {
    /// Toward the bottom.
    Down,
    /// Toward the left.
    Left,
    /// Toward the right.
    Right,
    /// Toward the top.
    Up,
}

// ========================================================================
// SplitTree Implementations
// ========================================================================

impl SplitTree {
    /// A tree of one pane, focused.
    pub fn new() -> Self {
        Self {
            focused: PaneId(0),
            next_id: 1,
            root: Node::Pane(PaneId(0)),
        }
    }

    /// Splits `pane` in two along `axis`, evenly, and focuses the new pane,
    /// which takes the right or bottom side. `None` when no pane has that
    /// id.
    pub fn split(&mut self, pane: PaneId, axis: Axis) -> Option<PaneId> {
        let split_id = SplitId(self.next_id);
        let new_pane = PaneId(self.next_id + 1);
        let node = self.root.find_pane_mut(pane)?;
        *node = Node::Split(Split {
            axis,
            first: Box::new(Node::Pane(pane)),
            id: split_id,
            ratio: EVEN_RATIO,
            second: Box::new(Node::Pane(new_pane)),
        });
        self.next_id += 2;
        self.focused = new_pane;
        Some(new_pane)
    }

    /// Closes `pane`; the other side of its split takes the whole area.
    /// When `pane` was focused, the pane that was next to it on that other
    /// side is. `false` when no pane has that id or it is the only pane,
    /// which a tree always keeps.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        let Some(neighbor) = self.root.remove_pane(pane) else {
            return false;
        };
        if self.focused == pane {
            self.focused = neighbor;
        }
        true
    }

    /// Focuses `pane`; `false` when no pane has that id.
    pub fn focus(&mut self, pane: PaneId) -> bool {
        let is_open = self.contains(pane);
        if is_open {
            self.focused = pane;
        }
        is_open
    }

    /// Focuses the nearest pane in `direction` from the focused one, as laid
    /// out in `area` with dividers `divider` pixels thick, and returns it.
    /// `None`, and focus stays, when no pane lies that way: focus never
    /// wraps around.
    pub fn focus_toward(
        &mut self,
        direction: Direction,
        area: Rect,
        divider: f32,
    ) -> Option<PaneId> {
        let layout = self.layout(area, divider);
        let from = layout.panes.iter().find(|pane| pane.id == self.focused)?;
        let target = nearest_toward(from.rect, direction, &layout.panes)?;
        self.focused = target;
        Some(target)
    }

    /// Sets how split `split` shares its area, `ratio` going to the left or
    /// top side, held between 0.1 and 0.9. `false` when no split has that
    /// id.
    pub fn set_ratio(&mut self, split: SplitId, ratio: f32) -> bool {
        match self.root.find_split_mut(split) {
            Some(found) => {
                found.ratio = clamp_ratio(ratio);
                true
            }
            None => false,
        }
    }

    /// The focused pane.
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// Whether a pane has the id `pane`.
    pub fn contains(&self, pane: PaneId) -> bool {
        self.panes().contains(&pane)
    }

    /// The panes, in tree order.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.root.collect_panes(&mut panes);
        panes
    }

    /// Where the panes and dividers are when the tree fills `area`, with
    /// dividers `divider` pixels thick. Sizes are whole pixels.
    pub fn layout(&self, area: Rect, divider: f32) -> SplitLayout {
        let mut layout = SplitLayout::default();
        self.root.lay_out(area, divider, &mut layout);
        layout
    }
}

impl Default for SplitTree {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Divider Implementations
// ========================================================================

impl Divider {
    /// The ratio that puts this divider's middle at `point`, held between
    /// 0.1 and 0.9: what dragging the divider to `point` sets.
    pub fn ratio_at(&self, point: Point) -> f32 {
        let (start, length, position, thickness) = match self.axis {
            Axis::Horizontal => (self.area.x, self.area.width, point.x, self.rect.width),
            Axis::Vertical => (self.area.y, self.area.height, point.y, self.rect.height),
        };
        let room = length - thickness;
        if room <= 0.0 {
            return EVEN_RATIO;
        }
        clamp_ratio((position - start - thickness / 2.0) / room)
    }
}

// ========================================================================
// Node Implementations
// ========================================================================

impl Node {
    /// The node that is the pane `pane`.
    fn find_pane_mut(&mut self, pane: PaneId) -> Option<&mut Node> {
        match self {
            Self::Pane(id) if *id == pane => Some(self),
            Self::Pane(_) => None,
            Self::Split(split) => match split.first.find_pane_mut(pane) {
                Some(found) => Some(found),
                None => split.second.find_pane_mut(pane),
            },
        }
    }

    /// The split `id`.
    fn find_split_mut(&mut self, id: SplitId) -> Option<&mut Split> {
        let Self::Split(split) = self else {
            return None;
        };
        if split.id == id {
            return Some(split);
        }
        let Split { first, second, .. } = split;
        first
            .find_split_mut(id)
            .or_else(|| second.find_split_mut(id))
    }

    /// Removes the pane `pane` from below this node, putting its sibling in
    /// its split's place, and returns the pane of that sibling that was
    /// nearest to it. `None` when `pane` is not below this node or is this
    /// node itself.
    fn remove_pane(&mut self, pane: PaneId) -> Option<PaneId> {
        let Self::Split(split) = self else {
            return None;
        };
        let is_pane = |node: &Node| matches!(node, Self::Pane(id) if *id == pane);
        let (sibling, neighbor) = if is_pane(&split.first) {
            let sibling = mem::replace(split.second.as_mut(), Self::Pane(pane));
            let neighbor = sibling.first_pane();
            (sibling, neighbor)
        } else if is_pane(&split.second) {
            let sibling = mem::replace(split.first.as_mut(), Self::Pane(pane));
            let neighbor = sibling.last_pane();
            (sibling, neighbor)
        } else {
            return split
                .first
                .remove_pane(pane)
                .or_else(|| split.second.remove_pane(pane));
        };
        *self = sibling;
        Some(neighbor)
    }

    /// The leftmost or topmost pane below this node.
    fn first_pane(&self) -> PaneId {
        match self {
            Self::Pane(id) => *id,
            Self::Split(split) => split.first.first_pane(),
        }
    }

    /// The rightmost or bottommost pane below this node.
    fn last_pane(&self) -> PaneId {
        match self {
            Self::Pane(id) => *id,
            Self::Split(split) => split.second.last_pane(),
        }
    }

    /// Adds the panes below this node to `panes`, in order.
    fn collect_panes(&self, panes: &mut Vec<PaneId>) {
        match self {
            Self::Pane(id) => panes.push(*id),
            Self::Split(split) => {
                split.first.collect_panes(panes);
                split.second.collect_panes(panes);
            }
        }
    }

    /// Lays this node out in `area` with dividers `divider` thick, adding
    /// to `layout`.
    fn lay_out(&self, area: Rect, divider: f32, layout: &mut SplitLayout) {
        let split = match self {
            Self::Pane(id) => {
                layout.panes.push(PaneRect {
                    id: *id,
                    rect: area,
                });
                return;
            }
            Self::Split(split) => split,
        };
        let (first, line, second) = split.divide(area, divider);
        layout.dividers.push(Divider {
            area,
            axis: split.axis,
            id: split.id,
            rect: line,
        });
        split.first.lay_out(first, divider, layout);
        split.second.lay_out(second, divider, layout);
    }
}

// ========================================================================
// Split Implementations
// ========================================================================

impl Split {
    /// `area` cut into the first side, the divider `divider` thick, and the
    /// second side.
    fn divide(&self, area: Rect, divider: f32) -> (Rect, Rect, Rect) {
        let Rect {
            height,
            width,
            x,
            y,
        } = area;
        match self.axis {
            Axis::Horizontal => {
                let line = divider.min(width).max(0.0);
                let first = ((width - line) * self.ratio).round();
                let second = (width - line - first).max(0.0);
                (
                    Rect::new(x, y, first, height),
                    Rect::new(x + first, y, line, height),
                    Rect::new(x + first + line, y, second, height),
                )
            }
            Axis::Vertical => {
                let line = divider.min(height).max(0.0);
                let first = ((height - line) * self.ratio).round();
                let second = (height - line - first).max(0.0);
                (
                    Rect::new(x, y, width, first),
                    Rect::new(x, y + first, width, line),
                    Rect::new(x, y + first + line, width, second),
                )
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `ratio` held between [`MIN_RATIO`] and [`MAX_RATIO`]; even when it is
/// not a number.
fn clamp_ratio(ratio: f32) -> f32 {
    match ratio.is_nan() {
        true => EVEN_RATIO,
        false => ratio.clamp(MIN_RATIO, MAX_RATIO),
    }
}

/// The pane of `panes` nearest to `from` in `direction`: wholly beyond it
/// that way and overlapping it across, the closest first, then the one
/// sharing the longest edge with it, then the first in tree order.
fn nearest_toward(from: Rect, direction: Direction, panes: &[PaneRect]) -> Option<PaneId> {
    let mut best: Option<(f32, f32, PaneId)> = None;
    for pane in panes {
        let rect = pane.rect;
        let (gap, overlap) = match direction {
            Direction::Down => (
                rect.y - (from.y + from.height),
                span_overlap(from, rect, Axis::Horizontal),
            ),
            Direction::Left => (
                from.x - (rect.x + rect.width),
                span_overlap(from, rect, Axis::Vertical),
            ),
            Direction::Right => (
                rect.x - (from.x + from.width),
                span_overlap(from, rect, Axis::Vertical),
            ),
            Direction::Up => (
                from.y - (rect.y + rect.height),
                span_overlap(from, rect, Axis::Horizontal),
            ),
        };
        if gap < -EDGE_TOLERANCE || overlap <= 0.0 {
            continue;
        }
        let is_better = match best {
            None => true,
            Some((best_gap, best_overlap, _)) => {
                gap < best_gap - EDGE_TOLERANCE
                    || (gap <= best_gap + EDGE_TOLERANCE && overlap > best_overlap)
            }
        };
        if is_better {
            best = Some((gap, overlap, pane.id));
        }
    }
    best.map(|(_, _, id)| id)
}

/// How far `first` and `second` overlap along `axis`: across their widths
/// for [`Axis::Horizontal`], their heights for [`Axis::Vertical`].
fn span_overlap(first: Rect, second: Rect, axis: Axis) -> f32 {
    let (start, end) = match axis {
        Axis::Horizontal => (
            first.x.max(second.x),
            (first.x + first.width).min(second.x + second.width),
        ),
        Axis::Vertical => (
            first.y.max(second.y),
            (first.y + first.height).min(second.y + second.height),
        ),
    };
    end - start
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 600.0);

    const DIVIDER: f32 = 2.0;

    /// `a | (b over c)`, with `c` focused.
    fn three_panes() -> (SplitTree, [PaneId; 3]) {
        let mut tree = SplitTree::new();
        let a = tree.focused();
        let b = tree.split(a, Axis::Horizontal).unwrap();
        let c = tree.split(b, Axis::Vertical).unwrap();
        (tree, [a, b, c])
    }

    fn rect_of(layout: &SplitLayout, id: PaneId) -> Rect {
        layout.panes.iter().find(|pane| pane.id == id).unwrap().rect
    }

    #[test]
    fn test_a_split_focuses_the_new_pane_on_the_right_or_bottom() {
        let (tree, [a, b, c]) = three_panes();
        assert_eq!(tree.panes(), [a, b, c]);
        assert_eq!(tree.focused(), c);
        let layout = tree.layout(AREA, DIVIDER);
        assert_eq!(rect_of(&layout, a), Rect::new(0.0, 0.0, 499.0, 600.0));
        assert_eq!(rect_of(&layout, b), Rect::new(501.0, 0.0, 499.0, 299.0));
        assert_eq!(rect_of(&layout, c), Rect::new(501.0, 301.0, 499.0, 299.0));
        assert_eq!(layout.dividers[0].rect, Rect::new(499.0, 0.0, 2.0, 600.0));
        assert_eq!(layout.dividers[1].rect, Rect::new(501.0, 299.0, 499.0, 2.0));
    }

    #[test]
    fn test_removing_a_pane_gives_its_area_to_its_sibling() {
        let (mut tree, [a, b, c]) = three_panes();
        assert!(tree.remove(b));
        assert_eq!(tree.panes(), [a, c]);
        let layout = tree.layout(AREA, DIVIDER);
        assert_eq!(rect_of(&layout, c), Rect::new(501.0, 0.0, 499.0, 600.0));
        assert_eq!(layout.dividers.len(), 1);
    }

    #[test]
    fn test_removing_the_focused_pane_focuses_its_nearest_neighbor() {
        let (mut tree, [a, b, _]) = three_panes();
        tree.focus(a);
        tree.remove(a);
        assert_eq!(tree.focused(), b, "the top of the side that took over");
        let (mut tree, [_, b, c]) = three_panes();
        tree.remove(c);
        assert_eq!(tree.focused(), b, "the bottom of the side that took over");
    }

    #[test]
    fn test_the_last_pane_and_unknown_panes_are_not_removed() {
        let mut tree = SplitTree::new();
        let only = tree.focused();
        assert!(!tree.remove(only));
        let other = tree.split(only, Axis::Vertical).unwrap();
        tree.remove(other);
        assert!(!tree.remove(other), "already closed");
        assert_eq!(tree.panes(), [only]);
    }

    #[test]
    fn test_ratios_are_held_between_a_tenth_and_nine_tenths() {
        let (mut tree, _) = three_panes();
        let outer = tree.layout(AREA, DIVIDER).dividers[0].id;
        assert!(tree.set_ratio(outer, 0.99));
        assert_eq!(tree.layout(AREA, DIVIDER).dividers[0].rect.x, 898.0);
        assert!(tree.set_ratio(outer, f32::NAN));
        assert_eq!(tree.layout(AREA, DIVIDER).dividers[0].rect.x, 499.0);
        assert!(!tree.set_ratio(SplitId(99), 0.5));
    }

    #[test]
    fn test_dragging_a_divider_puts_its_middle_under_the_pointer() {
        let (tree, _) = three_panes();
        let layout = tree.layout(AREA, DIVIDER);
        let outer = layout.dividers[0];
        assert_eq!(outer.ratio_at(Point::new(250.0, 10.0)), 249.0 / 998.0);
        assert_eq!(outer.ratio_at(Point::new(5.0, 10.0)), MIN_RATIO);
        let inner = layout.dividers[1];
        assert_eq!(inner.ratio_at(Point::new(0.0, 600.0)), MAX_RATIO);
    }

    #[test]
    fn test_focus_moves_to_the_neighbor_that_way_and_never_wraps() {
        let (mut tree, [a, b, _]) = three_panes();
        let mut toward = |direction| tree.focus_toward(direction, AREA, DIVIDER);
        assert_eq!(toward(Direction::Up), Some(b));
        assert_eq!(toward(Direction::Up), None, "nothing above the top");
        assert_eq!(toward(Direction::Right), None, "no wrap to the left");
        assert_eq!(toward(Direction::Left), Some(a));
        assert_eq!(toward(Direction::Left), None);
        assert_eq!(tree.focused(), a);
    }

    #[test]
    fn test_focus_prefers_the_neighbor_sharing_the_longest_edge() {
        // (a over b) | (c over d), with the right column split lower.
        let mut tree = SplitTree::new();
        let a = tree.focused();
        let c = tree.split(a, Axis::Horizontal).unwrap();
        let b = tree.split(a, Axis::Vertical).unwrap();
        let d = tree.split(c, Axis::Vertical).unwrap();
        let right_inner = tree.layout(AREA, DIVIDER).dividers[2].id;
        tree.set_ratio(right_inner, 0.8);
        // d spans y 480 to 600, all beside b (301 to 600).
        tree.focus(d);
        assert_eq!(tree.focus_toward(Direction::Left, AREA, DIVIDER), Some(b));
        // c spans y 0 to 478: more beside a (0 to 299) than b (180 shared).
        tree.focus(c);
        assert_eq!(tree.focus_toward(Direction::Left, AREA, DIVIDER), Some(a));
        assert_eq!(tree.panes(), [a, b, c, d]);
    }
}

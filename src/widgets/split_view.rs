//! [`SplitView`]: draws a [`SplitTree`]'s dividers and focus mark, and lets
//! the pointer drag a divider to resize the panes beside it or click a pane
//! to focus it.
//!
//! The app owns the tree and a `SplitView`, offers the view each window
//! event with the lent [`Widget::handle`](crate::Widget::handle), and each frame calls
//! the lent [`Widget::draw`](crate::Widget::draw), which returns where each pane is for the app to
//! draw its content in, then [`SplitView::draw_focus_outline`] over that
//! content.
//!
//! A [`DropZone`] names where in a pane something dragged over it (a tab
//! from a [`TabStrip`](crate::TabStrip)) would dock: a side to split the
//! pane toward, or its center to join it.

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
use super::super::base::split::{
    Axis, Direction, Divider, PaneId, PaneRect, SplitId, SplitLayout, SplitTree,
};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::icon::{logical, outline};

// ========================================================================
// Constants
// ========================================================================

/// The thickness of a divider, in logical pixels.
const DIVIDER_THICKNESS: f32 = 1.0;

/// How far either side of a divider the pointer still grabs it, in logical
/// pixels: a one pixel line is too thin to aim at.
const DIVIDER_GRAB: f32 = 3.0;

/// The thickness of the outline around the focused pane, in logical pixels.
const FOCUS_STROKE: f32 = 1.0;

/// How near a pane's side, as a share of its size, a dragged tab splits
/// toward that side rather than joining the pane.
const EDGE_SHARE: f32 = 0.25;

// ========================================================================
// Data Structures
// ========================================================================

/// The pointer handling and drawing of a [`SplitTree`]. It keeps the hover
/// and drag state between events, and the layout of the last frame it drew,
/// which its hit testing uses.
#[derive(Clone, Debug, Default)]
pub struct SplitView {
    /// The area the tree was last drawn in.
    area: Rect,
    /// The divider being dragged, as it was when the drag began.
    dragging: Option<Divider>,
    /// The divider the pointer is over.
    hovered: Option<SplitId>,
    /// Where the panes and dividers were in the last frame drawn, or `None`
    /// before it.
    layout: Option<SplitLayout>,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// Physical pixels per logical pixel in the last frame drawn.
    scale_factor: f64,
}

/// What became of an input offered to a [`SplitView`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SplitViewResponse {
    /// A press in this pane focused it. The press is otherwise the app's:
    /// it may act on it inside the pane as well.
    Focused(PaneId),
    /// The view used the event (a divider hovered or dragged); the app
    /// should not act on it again.
    Handled,
    /// The event is not the view's; the app handles it as usual.
    Ignored,
}

/// Where in a pane a tab dragged over it would go: a side to split the
/// pane toward, or its center to join the pane's own tabs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DropZone {
    /// A new pane below.
    Bottom,
    /// The pane itself, among its tabs.
    Center,
    /// A new pane to the left.
    Left,
    /// A new pane to the right.
    Right,
    /// A new pane above.
    Top,
}

// ========================================================================
// SplitView Implementations
// ========================================================================

impl SplitView {
    /// The view lent `tree`, the panes it lays out, as a [`Widget`] to
    /// hand inputs and draw.
    pub fn lend<'a>(&'a mut self, tree: &'a mut SplitTree) -> Lent<'a, Self, &'a mut SplitTree> {
        Lent::new(self, tree)
    }

    /// A view that has drawn nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Focuses the nearest pane of `tree` in `direction` from the focused
    /// one, as laid out in the last frame drawn, and returns it; `None`
    /// when no pane lies that way, since focus never wraps around.
    pub fn focus_toward(&self, tree: &mut SplitTree, direction: Direction) -> Option<PaneId> {
        tree.focus_toward(direction, self.area, self.divider_thickness())
    }

    /// Draws an outline around the focused pane of `tree` in `theme`'s
    /// accent, where the last [`Widget::draw`] laid it out, when there is
    /// more than one pane. Call it after drawing the panes' content, which
    /// would otherwise paint over it.
    pub fn draw_focus_outline(&self, frame: &mut Frame, tree: &SplitTree, theme: &Theme) {
        let Some(layout) = &self.layout else {
            return;
        };
        let focused = layout.panes.iter().find(|pane| pane.id == tree.focused());
        if let (true, Some(focused)) = (layout.panes.len() > 1, focused) {
            let stroke = logical(FOCUS_STROKE, self.scale_factor);
            for edge in outline(focused.rect, stroke) {
                frame.fill_rect(edge, theme.accent);
            }
        }
    }

    /// The thickness of a divider in physical pixels.
    fn divider_thickness(&self) -> f32 {
        logical(DIVIDER_THICKNESS, self.scale_factor)
    }

    /// A left press: start dragging the divider under the pointer, or focus
    /// the pane under it.
    fn press(&mut self, cx: &mut EventCx<'_>, tree: &mut SplitTree) -> SplitViewResponse {
        if let Some(divider) = self.divider_under_pointer() {
            self.dragging = Some(divider);
            cx.request_redraw();
            return SplitViewResponse::Handled;
        }
        let (Some(point), Some(layout)) = (self.pointer, &self.layout) else {
            return SplitViewResponse::Ignored;
        };
        let Some(pane) = layout.panes.iter().find(|pane| pane.rect.contains(point)) else {
            return SplitViewResponse::Ignored;
        };
        if pane.id == tree.focused() {
            return SplitViewResponse::Ignored;
        }
        tree.focus(pane.id);
        cx.request_redraw();
        SplitViewResponse::Focused(pane.id)
    }

    /// The divider the pointer grabs, as last drawn.
    fn divider_under_pointer(&self) -> Option<Divider> {
        let point = self.pointer?;
        let grab = logical(DIVIDER_GRAB, self.scale_factor);
        divider_at(self.layout.as_ref()?, point, grab)
    }

    /// Moves the hover to `divider`, changing the pointer shape and
    /// redrawing when it changed.
    fn set_hovered(&mut self, cx: &mut EventCx<'_>, divider: Option<Divider>) {
        let id = divider.map(|divider| divider.id);
        if self.hovered == id {
            return;
        }
        self.hovered = id;
        let icon = match divider.map(|divider| divider.axis) {
            Some(Axis::Horizontal) => CursorIcon::ColResize,
            Some(Axis::Vertical) => CursorIcon::RowResize,
            None => CursorIcon::Default,
        };
        cx.set_cursor(icon);
        cx.request_redraw();
    }
}

impl Widget for Lent<'_, SplitView, &mut SplitTree> {
    type Output = Vec<PaneRect>;
    type Response = SplitViewResponse;

    /// Reacts to `input`: the resize pointer over a divider, dragging it to
    /// change the tree's ratios, and a press in an unfocused pane focusing
    /// it. Offer it every pointer input before acting on it.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> SplitViewResponse {
        let (view, tree) = (&mut *self.widget, &mut *self.model);
        match input {
            Input::PointerMoved(point) => {
                let point = *point;
                view.pointer = Some(point);
                if let Some(divider) = view.dragging {
                    tree.set_ratio(divider.id, divider.ratio_at(point));
                    cx.request_redraw();
                    return SplitViewResponse::Handled;
                }
                let hovered = view.divider_under_pointer();
                view.set_hovered(cx, hovered);
                match hovered {
                    Some(_) => SplitViewResponse::Handled,
                    None => SplitViewResponse::Ignored,
                }
            }
            Input::PointerLeft => {
                view.pointer = None;
                if view.dragging.is_none() {
                    view.set_hovered(cx, None);
                }
                SplitViewResponse::Ignored
            }
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                view.pointer = Some(button.position);
                match (button.pressed, view.dragging.is_some()) {
                    (true, _) => view.press(cx, tree),
                    (false, true) => {
                        view.dragging = None;
                        let hovered = view.divider_under_pointer();
                        view.set_hovered(cx, hovered);
                        cx.request_redraw();
                        SplitViewResponse::Handled
                    }
                    (false, false) => SplitViewResponse::Ignored,
                }
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => SplitViewResponse::Ignored,
        }
    }

    /// Lays the tree out in `area`, draws its dividers in `theme`'s colors
    /// (in the accent color while hovered or dragged), and returns where
    /// each pane is. The focused pane's outline is drawn over the panes'
    /// content by [`SplitView::draw_focus_outline`].
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Vec<PaneRect> {
        let (view, tree) = (&mut *self.widget, &*self.model);
        view.area = area;
        view.scale_factor = frame.scale_factor();
        let layout = tree.layout(area, view.divider_thickness());
        let lit = view.dragging.map(|divider| divider.id).or(view.hovered);
        for divider in &layout.dividers {
            let color = match lit == Some(divider.id) {
                true => theme.accent,
                false => theme.border,
            };
            frame.fill_rect(divider.rect, color);
        }
        let panes = layout.panes.clone();
        view.layout = Some(layout);
        panes
    }
}

impl Outcome for SplitViewResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// DropZone Implementations
// ========================================================================

impl DropZone {
    /// The zone of `pane` under `point`: a side when the point is within
    /// a quarter of the pane from it (the nearest side where two are), the
    /// center otherwise; `None` outside the pane.
    pub fn at(pane: Rect, point: Point) -> Option<Self> {
        if !pane.contains(point) {
            return None;
        }
        let across = (point.x - pane.x) / pane.width;
        let down = (point.y - pane.y) / pane.height;
        let sides = [
            (across, Self::Left),
            (1.0 - across, Self::Right),
            (down, Self::Top),
            (1.0 - down, Self::Bottom),
        ];
        let (distance, side) = sides.into_iter().min_by(|(a, _), (b, _)| a.total_cmp(b))?;
        Some(match distance < EDGE_SHARE {
            true => side,
            false => Self::Center,
        })
    }

    /// The part of `pane` a tab dropped in the zone would take: half of it
    /// toward the side, or all of it.
    pub fn rect(self, pane: Rect) -> Rect {
        let (half_width, half_height) = ((pane.width / 2.0).round(), (pane.height / 2.0).round());
        match self {
            Self::Bottom => Rect::new(
                pane.x,
                pane.y + pane.height - half_height,
                pane.width,
                half_height,
            ),
            Self::Center => pane,
            Self::Left => Rect::new(pane.x, pane.y, half_width, pane.height),
            Self::Right => Rect::new(
                pane.x + pane.width - half_width,
                pane.y,
                half_width,
                pane.height,
            ),
            Self::Top => Rect::new(pane.x, pane.y, pane.width, half_height),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The divider of `layout` that `point` grabs: on it or within `grab`
/// pixels of either side. Inner dividers win where they meet an outer one,
/// since the outer one is still grabbed along the rest of its length.
fn divider_at(layout: &SplitLayout, point: Point, grab: f32) -> Option<Divider> {
    layout.dividers.iter().rev().copied().find(|divider| {
        let rect = divider.rect;
        let reach = match divider.axis {
            Axis::Horizontal => {
                Rect::new(rect.x - grab, rect.y, rect.width + 2.0 * grab, rect.height)
            }
            Axis::Vertical => {
                Rect::new(rect.x, rect.y - grab, rect.width, rect.height + 2.0 * grab)
            }
        };
        reach.contains(point)
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// `a | (b over c)` in 1000 by 600 with a one pixel divider: the outer
    /// divider at x 500, the inner one at y 300 from x 501 on.
    fn layout() -> SplitLayout {
        let mut tree = SplitTree::new();
        let a = tree.focused();
        let b = tree.split(a, Axis::Horizontal).unwrap();
        tree.split(b, Axis::Vertical);
        tree.layout(Rect::new(0.0, 0.0, 1000.0, 600.0), 1.0)
    }

    #[test]
    fn test_a_divider_is_grabbed_within_reach_of_either_side() {
        let layout = layout();
        let outer = layout.dividers[0].id;
        let at = |x: f32, y: f32| divider_at(&layout, Point::new(x, y), 3.0).map(|d| d.id);
        assert_eq!(at(500.0, 100.0), Some(outer));
        assert_eq!(at(497.0, 100.0), Some(outer));
        assert_eq!(at(503.0, 100.0), Some(outer));
        assert_eq!(at(496.0, 100.0), None);
        assert_eq!(at(504.0, 100.0), None);
    }

    #[test]
    fn test_the_inner_divider_wins_where_it_meets_the_outer_one() {
        let layout = layout();
        let inner = layout.dividers[1].id;
        let at = |x: f32, y: f32| divider_at(&layout, Point::new(x, y), 3.0).map(|d| d.id);
        assert_eq!(at(502.0, 300.0), Some(inner));
        assert_eq!(at(800.0, 297.0), Some(inner));
        assert_eq!(at(800.0, 250.0), None);
    }

    #[test]
    fn test_a_drop_zone_is_the_near_side_or_the_center() {
        let pane = Rect::new(0.0, 0.0, 400.0, 200.0);
        assert_eq!(
            DropZone::at(pane, Point::new(20.0, 100.0)),
            Some(DropZone::Left)
        );
        assert_eq!(
            DropZone::at(pane, Point::new(390.0, 100.0)),
            Some(DropZone::Right)
        );
        assert_eq!(
            DropZone::at(pane, Point::new(200.0, 10.0)),
            Some(DropZone::Top)
        );
        assert_eq!(
            DropZone::at(pane, Point::new(200.0, 190.0)),
            Some(DropZone::Bottom)
        );
        assert_eq!(
            DropZone::at(pane, Point::new(200.0, 100.0)),
            Some(DropZone::Center)
        );
        assert_eq!(DropZone::at(pane, Point::new(500.0, 100.0)), None);
        assert_eq!(
            DropZone::Right.rect(pane),
            Rect::new(200.0, 0.0, 200.0, 200.0)
        );
        assert_eq!(
            DropZone::Bottom.rect(pane),
            Rect::new(0.0, 100.0, 400.0, 100.0)
        );
    }
}

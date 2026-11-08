//! [`FloatingPanel`]: a box floating over part of the window that the user
//! moves by its header, as a draggable toolbar or a card in a corner does.
//! It is not modal: the rest of the window stays usable, so register it
//! with a [`FocusTree`](crate::FocusTree) as an ordinary stop, not a trap.
//!
//! The panel draws its box, its header (a title and an optional close
//! button), and returns the rect below the header for the app to draw the
//! panel's content in. A left press on the header and a drag move it, kept
//! inside the area it is drawn in. Keys, while the app says it is focused:
//! Alt and an arrow move it a step that way. Made with
//! [`FloatingPanel::with_snapping`], it rests only in the area's corners:
//! released after a drag, it goes to the nearest one, and Alt and an arrow
//! move it to the next corner that way. A snapped panel keeps its corner
//! when the area changes size.
//!
//! Draw it after what it floats over, or inside [`Frame::overlay`] (and its
//! content with it) to float over popups too. Offer it the pointer before
//! what lies under it, and pass on what it ignores: the pointer over its
//! content comes back [`FloatingPanelResponse::Ignored`] for the app's
//! content to take, and [`FloatingPanel::contains`] says whether a point
//! is over it at all.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::{ButtonInput, Input, PointerButton};
use super::super::render::icon::{Icon, logical};
use super::button::face_height;
use super::label::draw_label;

// ========================================================================
// Constants
// ========================================================================

/// How far Alt and an arrow move a free panel, in logical pixels.
const MOVE_STEP: f32 = 16.0;

/// The gap between a snapped panel and the area's edges, in logical
/// pixels.
const SNAP_MARGIN: f32 = 8.0;

/// The blank cells left of the header's title.
const TITLE_INSET_CELLS: f32 = 1.0;

/// The close cross's size, in logical pixels.
const CLOSE_ICON_SIZE: f32 = 8.0;

/// The thickness of the close cross's strokes and of the line under the
/// header, in logical pixels.
const CLOSE_STROKE: f32 = 1.0;

/// The child index of the close button's accessibility node.
const CLOSE_NODE: u64 = 0;

// ========================================================================
// Data Structures
// ========================================================================

/// A non-modal box the user moves by its header. The app owns it, offers
/// it the window's pointer events (and its keys while it is focused), and
/// draws it, then its content, each frame.
#[derive(Clone, Debug)]
pub struct FloatingPanel {
    /// Whether the header has a close button.
    closable: bool,
    /// What screen readers name the close button, from the theme's words
    /// when last drawn.
    close_label: String,
    /// The corner a snapping panel rests in, or `None` for a free one.
    corner: Option<Corner>,
    /// Where the pointer holds the panel during a drag, from its top left
    /// corner in physical pixels; `None` when not dragging.
    grab: Option<Point>,
    /// Whether the app gave it the keyboard.
    focused: bool,
    /// The size of the panel, header included, in logical pixels.
    height: f32,
    /// Whether the pointer is over the close button.
    hovered_close: bool,
    /// Where it was drawn last, for the pointer and the keys.
    layout: Option<PanelLayout>,
    /// Where a free panel's top left corner is, in logical pixels from the
    /// area's top left corner.
    position: Point,
    /// What the header says, and screen readers name the panel.
    title: String,
    /// The width of the panel, in logical pixels.
    width: f32,
}

/// A corner of the area a snapping [`FloatingPanel`] rests in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Corner {
    /// The bottom left corner.
    BottomLeft,
    /// The bottom right corner.
    BottomRight,
    /// The top left corner.
    TopLeft,
    /// The top right corner.
    TopRight,
}

/// What became of an input offered to a [`FloatingPanel`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FloatingPanelResponse {
    /// The user clicked the close button: stop drawing the panel.
    Closed,
    /// The panel used the input; nothing more to do.
    Handled,
    /// The input is not the panel's: offer it to its content or what lies
    /// under it.
    Ignored,
    /// The user finished moving the panel: save
    /// [`FloatingPanel::position`] or [`FloatingPanel::corner`] if the
    /// app keeps it across runs.
    Moved,
}

/// Where a panel was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PanelLayout {
    /// The area it floats in.
    area: Rect,
    /// The close button, if it has one.
    close: Option<Rect>,
    /// The header.
    header: Rect,
    /// The whole panel.
    rect: Rect,
    /// Physical pixels per logical pixel.
    scale_factor: f32,
}

// ========================================================================
// FloatingPanel Implementations
// ========================================================================

impl FloatingPanel {
    /// A free panel titled `title`, `width` by `height` logical pixels
    /// (header included), at the top left of its area, without a close
    /// button.
    pub fn new(title: impl Into<String>, width: f32, height: f32) -> Self {
        Self {
            closable: false,
            close_label: String::new(),
            corner: None,
            focused: false,
            grab: None,
            height,
            hovered_close: false,
            layout: None,
            position: Point::default(),
            title: title.into(),
            width,
        }
    }

    /// This panel with a close button in its header.
    pub fn with_close_button(self) -> Self {
        Self {
            closable: true,
            ..self
        }
    }

    /// This panel at `position`, in logical pixels from its area's top
    /// left corner.
    pub fn with_position(self, position: Point) -> Self {
        Self { position, ..self }
    }

    /// This panel resting only in its area's corners, starting in
    /// `corner`.
    pub fn with_snapping(self, corner: Corner) -> Self {
        Self {
            corner: Some(corner),
            ..self
        }
    }

    /// Where a free panel's top left corner is, in logical pixels from its
    /// area's top left corner.
    pub fn position(&self) -> Point {
        self.position
    }

    /// The corner a snapping panel rests in; `None` for a free panel.
    pub fn corner(&self) -> Option<Corner> {
        self.corner
    }

    /// Whether `point` is over the panel as last drawn, header and content.
    pub fn contains(&self, point: Point) -> bool {
        self.layout
            .is_some_and(|layout| layout.rect.contains(point))
    }

    /// Describes the panel to screen readers as `id`: a pane named by its
    /// title, holding its close button and whatever `content` describes
    /// (call the content's own `access` in it).
    pub fn access(&self, frame: &mut Frame, id: FocusId, content: impl FnOnce(&mut Frame)) {
        let Some(layout) = self.layout else {
            return;
        };
        let mut pane = Node::new(Role::Pane);
        pane.set_label(self.title.as_str());
        frame.enter_access(id, layout.rect, pane);
        if let Some(close) = layout.close {
            let mut button = Node::new(Role::Button);
            button.set_label(self.close_label.as_str());
            button.add_action(Action::Click);
            frame.enter_access(id.child(CLOSE_NODE), close, button);
            frame.leave_access();
        }
        content(frame);
        frame.leave_access();
    }

    /// Does what a screen reader's `action` on the panel described as `id`
    /// asks: a click on its close button closes it.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> FloatingPanelResponse {
        match action {
            AccessAction::Click(target) if self.closable && target == id.child(CLOSE_NODE) => {
                FloatingPanelResponse::Closed
            }
            AccessAction::Click(_)
            | AccessAction::Decrement(_)
            | AccessAction::Focus(_)
            | AccessAction::Increment(_) => FloatingPanelResponse::Ignored,
        }
    }

    /// Reacts to a key while focused: Alt and an arrow move the panel.
    fn handle_key(&mut self, key: Key) -> FloatingPanelResponse {
        if !self.focused || key.mods != Mods::ALT {
            return FloatingPanelResponse::Ignored;
        }
        let (dx, dy) = match key.code {
            KeyCode::Down => (0.0, 1.0),
            KeyCode::Left => (-1.0, 0.0),
            KeyCode::Right => (1.0, 0.0),
            KeyCode::Up => (0.0, -1.0),
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Tab => return FloatingPanelResponse::Ignored,
        };
        let Some(layout) = self.layout else {
            return FloatingPanelResponse::Ignored;
        };
        match self.corner {
            Some(corner) => self.corner = Some(corner.toward(dx, dy)),
            None => {
                let step = MOVE_STEP * layout.scale_factor;
                let rect = layout.rect;
                self.move_to(Point::new(rect.x + dx * step, rect.y + dy * step), layout);
            }
        }
        FloatingPanelResponse::Moved
    }

    /// Reacts to a left press or release at `button.position`: a press on
    /// the header starts a drag, a release ends it (snapping a snapping
    /// panel), and a click on the close button closes the panel.
    fn press(&mut self, button: &ButtonInput) -> FloatingPanelResponse {
        let Some(layout) = self.layout else {
            return FloatingPanelResponse::Ignored;
        };
        let point = button.position;
        let on_close = layout.close.is_some_and(|close| close.contains(point));
        match (button.pressed, self.grab) {
            (true, _) if on_close => FloatingPanelResponse::Handled,
            (true, _) if layout.header.contains(point) => {
                let rect = layout.rect;
                self.move_to(Point::new(rect.x, rect.y), layout);
                self.grab = Some(Point::new(point.x - rect.x, point.y - rect.y));
                FloatingPanelResponse::Handled
            }
            (true, _) => FloatingPanelResponse::Ignored,
            (false, Some(_)) => {
                let rect = self.rect_in(layout);
                self.grab = None;
                if self.corner.is_some() {
                    self.corner = Some(Corner::nearest(rect, layout.area));
                }
                FloatingPanelResponse::Moved
            }
            (false, None) if on_close => FloatingPanelResponse::Closed,
            (false, None) => FloatingPanelResponse::Ignored,
        }
    }

    /// Follows the pointer to `point`: moves the panel during a drag and
    /// lights the close button under it.
    fn follow_pointer(&mut self, point: Point, cx: &mut EventCx<'_>) -> FloatingPanelResponse {
        let Some(layout) = self.layout else {
            return FloatingPanelResponse::Ignored;
        };
        let over_close = layout.close.is_some_and(|close| close.contains(point));
        if over_close != self.hovered_close {
            self.hovered_close = over_close;
            cx.request_redraw();
        }
        let Some(grab) = self.grab else {
            return match layout.header.contains(point) {
                true => FloatingPanelResponse::Handled,
                false => FloatingPanelResponse::Ignored,
            };
        };
        // A snapping panel moves freely while dragged, and snaps on release.
        self.move_to(Point::new(point.x - grab.x, point.y - grab.y), layout);
        cx.request_redraw();
        FloatingPanelResponse::Handled
    }

    /// Puts the panel's top left corner at `origin` (physical pixels),
    /// kept inside the area it was drawn in.
    fn move_to(&mut self, origin: Point, layout: PanelLayout) {
        let area = layout.area;
        let rect = layout.rect;
        let x = origin
            .x
            .clamp(area.x, (area.x + area.width - rect.width).max(area.x));
        let y = origin
            .y
            .clamp(area.y, (area.y + area.height - rect.height).max(area.y));
        self.position = Point::new(
            (x - area.x) / layout.scale_factor,
            (y - area.y) / layout.scale_factor,
        );
    }

    /// Where the panel goes in `area` at `scale_factor`: its size cut to
    /// the area, at its corner or position, kept inside.
    fn place(&self, area: Rect, scale_factor: f32) -> Rect {
        let width = (self.width * scale_factor).min(area.width).round();
        let height = (self.height * scale_factor).min(area.height).round();
        let room_x = (area.width - width).max(0.0);
        let room_y = (area.height - height).max(0.0);
        let margin = SNAP_MARGIN * scale_factor;
        let (x, y) = match (self.corner, self.grab) {
            (Some(corner), None) => {
                let (right, bottom) = corner.sides();
                let x = match right {
                    true => room_x - margin,
                    false => margin,
                };
                let y = match bottom {
                    true => room_y - margin,
                    false => margin,
                };
                (x.clamp(0.0, room_x), y.clamp(0.0, room_y))
            }
            (Some(_), Some(_)) | (None, _) => (
                (self.position.x * scale_factor).clamp(0.0, room_x),
                (self.position.y * scale_factor).clamp(0.0, room_y),
            ),
        };
        Rect::new((area.x + x).round(), (area.y + y).round(), width, height)
    }

    /// Where the panel is now in the area of `layout`.
    fn rect_in(&self, layout: PanelLayout) -> Rect {
        self.place(layout.area, layout.scale_factor)
    }
}

impl Widget for FloatingPanel {
    type Output = Rect;
    type Response = FloatingPanelResponse;

    /// Reacts to `input`: the pointer moving the panel by its header and
    /// clicking its close button, and Alt and the arrows while focused, as
    /// the module documentation says. Asks for a redraw whenever it used
    /// the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> FloatingPanelResponse {
        let response = match input {
            Input::Key(key) => self.handle_key(*key),
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.press(button)
            }
            Input::PointerMoved(point) => return self.follow_pointer(*point, cx),
            Input::PointerLeft if self.hovered_close => {
                self.hovered_close = false;
                FloatingPanelResponse::Handled
            }
            Input::FocusLost if self.grab.is_some() => {
                self.grab = None;
                FloatingPanelResponse::Moved
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
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::Wheel(_) => FloatingPanelResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the panel inside `area` (the part of the window it floats
    /// in): its floating box, its header with the title and close button,
    /// and returns the rect below the header for the content.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let scale_factor = frame.scale_factor();
        let scale = scale_factor as f32;
        let metrics = frame.cell_metrics();
        let rect = self.place(area, scale);
        let header_height = face_height(metrics).min(rect.height);
        let header = Rect::new(rect.x, rect.y, rect.width, header_height);
        let close = self.closable.then(|| {
            Rect::new(
                rect.x + rect.width - header_height,
                rect.y,
                header_height,
                header_height,
            )
        });
        frame.draw_box(
            rect,
            &theme
                .shape
                .floating_box(theme.surface, theme.border, scale_factor),
        );
        let title_x = rect.x + (TITLE_INSET_CELLS * metrics.width).round();
        let title_y = (rect.y + (header_height - metrics.height) / 2.0).round();
        let title_end = close.map_or(rect.x + rect.width, |close| close.x);
        let title_room = Rect::new(rect.x, rect.y, title_end - rect.x, header_height);
        frame.clipped(title_room, |frame| {
            draw_label(
                frame,
                theme,
                &self.title,
                Point::new(title_x, title_y),
                theme.muted,
            );
        });
        if let Some(close) = close {
            if self.hovered_close {
                frame.draw_box(close, &theme.shape.highlight(theme.hover, scale_factor));
            }
            let strokes = Icon::Close.strokes(close, CLOSE_ICON_SIZE, CLOSE_STROKE, scale_factor);
            for stroke in strokes {
                frame.fill_rect_over_text(stroke, theme.foreground);
            }
        }
        let line = logical(CLOSE_STROKE, scale_factor);
        frame.fill_rect(
            Rect::new(rect.x, header.y + header.height - line, rect.width, line),
            theme.border,
        );
        if self.close_label != theme.strings.close {
            self.close_label = theme.strings.close.to_string();
        }
        self.layout = Some(PanelLayout {
            area,
            close,
            header,
            rect,
            scale_factor: scale,
        });
        Rect::new(
            rect.x,
            header.y + header.height,
            rect.width,
            (rect.height - header.height).max(0.0),
        )
    }

    /// Tells it whether it has the keyboard: it takes Alt and the arrows
    /// only while it does.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Outcome for FloatingPanelResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Corner Implementations
// ========================================================================

impl Corner {
    /// The corner of `area` nearest the center of `rect`.
    fn nearest(rect: Rect, area: Rect) -> Self {
        let right = rect.x + rect.width / 2.0 > area.x + area.width / 2.0;
        let bottom = rect.y + rect.height / 2.0 > area.y + area.height / 2.0;
        Self::from_sides(right, bottom)
    }

    /// The corner one step from this one in the direction `dx`, `dy` (each
    /// -1, 0, or 1); this one when it is already that far.
    fn toward(self, dx: f32, dy: f32) -> Self {
        let (right, bottom) = self.sides();
        let right = match dx {
            dx if dx > 0.0 => true,
            dx if dx < 0.0 => false,
            _ => right,
        };
        let bottom = match dy {
            dy if dy > 0.0 => true,
            dy if dy < 0.0 => false,
            _ => bottom,
        };
        Self::from_sides(right, bottom)
    }

    /// Whether the corner is on the right, and whether at the bottom.
    fn sides(self) -> (bool, bool) {
        match self {
            Self::BottomLeft => (false, true),
            Self::BottomRight => (true, true),
            Self::TopLeft => (false, false),
            Self::TopRight => (true, false),
        }
    }

    /// The corner on the right (or left) and at the bottom (or top).
    fn from_sides(right: bool, bottom: bool) -> Self {
        match (right, bottom) {
            (false, false) => Self::TopLeft,
            (false, true) => Self::BottomLeft,
            (true, false) => Self::TopRight,
            (true, true) => Self::BottomRight,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::parse_key;
    use crate::testing::TestWindow;

    const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 300.0);

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn press(point: Point) -> Input {
        Input::PointerButton(ButtonInput::press(PointerButton::Left, point))
    }

    fn release(point: Point) -> Input {
        Input::PointerButton(ButtonInput::release(PointerButton::Left, point))
    }

    /// `panel` drawn in `AREA` of `window`, returning its content rect.
    fn draw(window: &TestWindow, panel: &mut FloatingPanel) -> Rect {
        let mut frame = window.frame();
        panel.draw(&mut frame, AREA, &Theme::dark())
    }

    fn drawn(panel: &FloatingPanel) -> PanelLayout {
        panel.layout.expect("drawn")
    }

    /// Drags `panel` by its header from its title to `to`.
    fn drag(
        window: &mut TestWindow,
        panel: &mut FloatingPanel,
        to: Point,
    ) -> FloatingPanelResponse {
        let header = drawn(panel).header;
        let from = Point::new(header.x + 4.0, header.y + 4.0);
        assert_eq!(
            window.feed(panel, &press(from)),
            FloatingPanelResponse::Handled
        );
        window.feed(panel, &Input::PointerMoved(to));
        draw(window, panel);
        window.feed(panel, &release(to))
    }

    #[test]
    fn test_a_drag_by_the_header_moves_the_panel_kept_inside_its_area() {
        let mut window = TestWindow::new(400, 300);
        let mut panel = FloatingPanel::new("Notes", 100.0, 80.0);
        let content = draw(&window, &mut panel);
        assert_eq!(
            content.y,
            drawn(&panel).header.y + drawn(&panel).header.height
        );
        assert_eq!(
            drag(&mut window, &mut panel, Point::new(54.0, 44.0)),
            FloatingPanelResponse::Moved
        );
        assert_eq!(panel.position(), Point::new(50.0, 40.0));
        drag(&mut window, &mut panel, Point::new(1000.0, -50.0));
        assert_eq!(panel.position(), Point::new(300.0, 0.0), "held inside");
        let body = Point::new(panel.position().x + 50.0, 70.0);
        assert_eq!(
            window.feed(&mut panel, &press(body)),
            FloatingPanelResponse::Ignored,
            "a press on the content is the content's"
        );
        assert!(panel.contains(body));
    }

    #[test]
    fn test_a_snapping_panel_lands_in_the_nearest_corner_and_keeps_it() {
        let mut window = TestWindow::new(400, 300);
        let mut panel = FloatingPanel::new("Card", 100.0, 80.0).with_snapping(Corner::TopLeft);
        draw(&window, &mut panel);
        assert_eq!(drawn(&panel).rect.x, SNAP_MARGIN);
        drag(&mut window, &mut panel, Point::new(330.0, 250.0));
        assert_eq!(panel.corner(), Some(Corner::BottomRight));
        draw(&window, &mut panel);
        let rect = drawn(&panel).rect;
        assert_eq!(rect.x + rect.width, AREA.width - SNAP_MARGIN);
        assert_eq!(rect.y + rect.height, AREA.height - SNAP_MARGIN);
    }

    #[test]
    fn test_alt_arrows_move_a_focused_panel_a_step_or_a_corner() {
        let mut window = TestWindow::new(400, 300);
        let mut free = FloatingPanel::new("Tools", 100.0, 80.0);
        draw(&window, &mut free);
        assert_eq!(
            window.feed(&mut free, &key("M-<right>")),
            FloatingPanelResponse::Ignored
        );
        free.set_focused(true);
        assert_eq!(
            window.feed(&mut free, &key("M-<right>")),
            FloatingPanelResponse::Moved
        );
        assert_eq!(free.position(), Point::new(MOVE_STEP, 0.0));
        assert_eq!(
            window.feed(&mut free, &key("<right>")),
            FloatingPanelResponse::Ignored
        );
        let mut card = FloatingPanel::new("Card", 100.0, 80.0).with_snapping(Corner::TopLeft);
        card.set_focused(true);
        draw(&window, &mut card);
        window.feed(&mut card, &key("M-<down>"));
        assert_eq!(card.corner(), Some(Corner::BottomLeft));
        window.feed(&mut card, &key("M-<right>"));
        assert_eq!(card.corner(), Some(Corner::BottomRight));
    }

    #[test]
    fn test_a_click_on_the_close_button_closes_and_a_press_held_off_it_does_not() {
        let mut window = TestWindow::new(400, 300);
        let mut panel = FloatingPanel::new("Notes", 100.0, 80.0).with_close_button();
        draw(&window, &mut panel);
        let close = drawn(&panel).close.expect("a close button");
        let on = Point::new(close.x + 2.0, close.y + 2.0);
        window.feed(&mut panel, &press(on));
        assert_eq!(
            window.feed(&mut panel, &release(on)),
            FloatingPanelResponse::Closed
        );
        let id = FocusId::new("notes");
        assert_eq!(
            panel.access_action(id, AccessAction::Click(id.child(CLOSE_NODE))),
            FloatingPanelResponse::Closed
        );
    }
}

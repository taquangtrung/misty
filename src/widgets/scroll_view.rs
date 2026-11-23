//! [`ScrollView`]: a viewport onto content larger than its area, with a
//! thin scrollbar along each side the content overflows. The app keeps
//! the content: it tells the view the content's size with
//! [`ScrollView::set_content_size`], draws the view, which returns the
//! viewport left beside the scrollbars, and draws its content shifted up
//! and left by [`ScrollView::offset`] inside
//! [`Frame::clipped`](crate::Frame::clipped) to that viewport.
//!
//! Keys, while the app says the view is focused: Up and Down (`k` and `j`)
//! scroll a line, Left and Right (`h` and `l`) a line sideways, PageUp and
//! PageDown (Shift+Space and Space) a viewport less a line, Home and End
//! (`g g` and `G`, as in lists) to the top and the bottom. The wheel over the view
//! scrolls three lines a notch, a touchpad by its pixels, and with Shift
//! held the vertical wheel scrolls sideways. A press on a scrollbar's
//! thumb drags it; a press on its track beside the thumb pages toward the
//! pointer. The thumb under the pointer widens in the theme's `scrollbar_hover`. Keys and
//! wheel notches glide there (see [`SmoothScroll`]); call its
//! [`Widget::about_to_wait`](crate::Widget::about_to_wait) so each glide
//! gets its frames. While motion is reduced ([`EventCx::reduced_motion`])
//! scrolls jump instead.
//!
//! A scroll that cannot move (the view is at its edge, or the content
//! fits) is [`ScrollViewResponse::Ignored`], so a view nested in another lets
//! the outer one scroll.

use std::mem;
use std::time::Instant;

use accesskit::{Node, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::clock;
use super::super::base::color::Rgb;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::scroll::SmoothScroll;
use super::super::base::scrollbar::{ScrollbarFade, ScrollbarShow};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton, WheelDelta, WheelInput};
use super::super::render::icon::logical;

// ========================================================================
// Constants
// ========================================================================

/// How wide a scrollbar's track is, the strip taken from the viewport, in
/// logical pixels.
pub(crate) const TRACK_WIDTH: f32 = 10.0;

/// How wide a resting thumb is, in logical pixels.
const THUMB_WIDTH: f32 = 4.0;

/// How wide the thumb under the pointer or being dragged is, in logical
/// pixels.
const THUMB_WIDTH_HOVER: f32 = 8.0;

/// The shortest a thumb gets, however long the content, in logical pixels.
const MIN_THUMB: f32 = 24.0;

/// The lines one notch of a mouse wheel scrolls.
const WHEEL_LINES: f32 = 3.0;

/// The key that, pressed twice, goes to the top, as in lists.
const GO_PREFIX: char = 'g';

// ========================================================================
// Data Structures
// ========================================================================

/// A scrolled viewport onto content the app draws, with scrollbars.
#[derive(Clone, Debug, Default)]
pub struct ScrollView {
    /// Whether the scrollbars are left out, the viewport taking the whole
    /// view.
    bars_hidden: bool,
    /// The content's size, in physical pixels.
    content: Point,
    /// The thumb being dragged, if any.
    drag: Option<ThumbDrag>,
    /// The offset the last frame was drawn at.
    drawn: Point,
    /// When the scrollbars show while the theme says
    /// [`ScrollbarShow::Auto`].
    fade: ScrollbarFade,
    /// Whether the view has the keyboard.
    focused: bool,
    /// Whether `g` was pressed, so a second `g` goes to the top.
    go_pending: bool,
    /// The scrollbar under the pointer, if any.
    hover: Option<Axis>,
    /// Where the parts were drawn last, for the pointer.
    layout: Option<ScrollLayout>,
    /// The sideways offset, easing toward its target.
    scroll_x: SmoothScroll,
    /// The vertical offset, easing toward its target.
    scroll_y: SmoothScroll,
    /// When the scrollbars show, as the theme last drawn in said.
    show: ScrollbarShow,
}

/// What became of an input offered to a [`ScrollView`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ScrollViewResponse {
    /// The view used the input without scrolling (a press on a thumb).
    Handled,
    /// The input is not the view's, or it could not scroll.
    Ignored,
    /// The view scrolled: draw again.
    Scrolled,
}

/// Where a scroll view's parts were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollLayout {
    /// The whole view.
    area: Rect,
    /// The track of the scrollbar along the bottom, when the content is
    /// wider than the viewport.
    horizontal: Option<Rect>,
    /// How far a line scrolls: one cell row.
    line: f32,
    /// The display's scale factor.
    scale_factor: f64,
    /// The track of the scrollbar along the right, when the content is
    /// taller than the viewport.
    vertical: Option<Rect>,
    /// The part of the view the content shows in.
    viewport: Rect,
}

/// A thumb being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ThumbDrag {
    /// Which scrollbar's thumb.
    axis: Axis,
    /// How far along the thumb the pointer took hold of it.
    grab: f32,
}

/// The direction a scrollbar scrolls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Axis {
    /// Sideways, along the bottom.
    Horizontal,
    /// Up and down, along the right.
    Vertical,
}

// ========================================================================
// ScrollView Implementations
// ========================================================================

impl ScrollView {
    // ------------------------------------------------------------------
    // State
    // ------------------------------------------------------------------

    /// A view of no content yet, scrolled to the top left.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets how big the content is, in physical pixels. The offset is
    /// held inside the new size when the view is next drawn.
    pub fn set_content_size(&mut self, width: f32, height: f32) {
        self.content = Point::new(width.max(0.0), height.max(0.0));
    }

    /// How far the content is scrolled, as the last frame was drawn (the
    /// target before the first): draw it shifted left by `x` and up by `y`.
    pub fn offset(&self) -> Point {
        self.drawn
    }

    /// Where the view is going: the offset a glide under way ends at, or
    /// the offset itself while none is.
    pub fn target(&self) -> Point {
        Point::new(self.scroll_x.target(), self.scroll_y.target())
    }

    /// Puts the view at `offset` at once, held inside the content when
    /// the view is next drawn.
    pub fn scroll_to(&mut self, offset: Point) {
        let before = Point::new(self.scroll_x.target(), self.scroll_y.target());
        self.scroll_x.jump_to(offset.x.max(0.0));
        self.scroll_y.jump_to(offset.y.max(0.0));
        self.drawn = Point::new(self.scroll_x.target(), self.scroll_y.target());
        if self.drawn != before {
            self.fade.wake(clock::now());
        }
    }

    /// Scrolls the least that shows `rect` of the content (a focused child,
    /// a search match), gliding there: its top left edge when it is larger
    /// than the viewport. Does nothing before the first frame, when the
    /// viewport is not known yet.
    pub fn reveal(&mut self, rect: Rect) {
        let Some(layout) = self.layout else {
            return;
        };
        let now = clock::now();
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let (start, length) = axis.span(rect);
            let (_, view) = axis.span(layout.viewport);
            let offset = self.motion(axis).target();
            let target = if start < offset || length > view {
                start
            } else if start + length > offset + view {
                start + length - view
            } else {
                offset
            };
            self.move_to(axis, target, true, now);
        }
    }

    /// Glides down (or up) to `y`, held inside the content; says whether
    /// that moved the view. Does nothing before the first frame.
    pub(crate) fn glide_to_y(&mut self, y: f32) -> bool {
        self.move_to(Axis::Vertical, y, true, clock::now())
    }

    /// Where the content is shown, as the last frame laid it out.
    pub(crate) fn viewport(&self) -> Option<Rect> {
        self.layout.map(|layout| layout.viewport)
    }

    /// Leaves the scrollbars out (`true`), the viewport taking the whole
    /// view, which still scrolls by keys and the wheel; or shows them
    /// again.
    pub(crate) fn set_bars_hidden(&mut self, hidden: bool) {
        self.bars_hidden = hidden;
        self.drag = None;
        self.hover = None;
    }

    /// Scrolls down by `delta` pixels (up for negative), gliding there,
    /// held inside the content; says whether that moved the view, so
    /// `false` at the edge it went toward. Does nothing before the first
    /// frame.
    pub(crate) fn scroll_by(&mut self, delta: f32) -> bool {
        let target = self.scroll_y.target() + delta;
        self.move_to(Axis::Vertical, target, true, clock::now())
    }

    /// Makes scrolls jump (`true`) rather than glide, as the window's
    /// reduced-motion preference says; turning it on ends a glide under
    /// way.
    fn follow_reduced_motion(&mut self, reduced_motion: bool) {
        self.scroll_x.set_reduced_motion(reduced_motion);
        self.scroll_y.set_reduced_motion(reduced_motion);
    }

    /// Describes the view as last drawn to screen readers as `id`: a scroll
    /// view saying how far it is scrolled each way and how far it can be.
    /// Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        let mut node = Node::new(Role::ScrollView);
        let max = self.max_offset(layout);
        node.set_scroll_x(f64::from(self.drawn.x));
        node.set_scroll_x_min(0.0);
        node.set_scroll_x_max(f64::from(max.x));
        node.set_scroll_y(f64::from(self.drawn.y));
        node.set_scroll_y_min(0.0);
        node.set_scroll_y_max(f64::from(max.y));
        frame.access(id, layout.area, node);
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    /// Reacts to `key` while focused, as the module documentation lists.
    fn handle_key(&mut self, key: Key) -> ScrollViewResponse {
        let Some(layout) = self.layout.filter(|_| self.focused) else {
            return ScrollViewResponse::Ignored;
        };
        let page = (layout.viewport.height - layout.line).max(layout.line);
        let plain = key.mods == Mods::NONE;
        let shifted = key.mods == Mods::SHIFT;
        let now = clock::now();
        let go_pending = mem::take(&mut self.go_pending);
        let (axis, target) = match key.code {
            KeyCode::Char(GO_PREFIX) if plain && go_pending => (Axis::Vertical, 0.0),
            KeyCode::Char(GO_PREFIX) if plain => {
                self.go_pending = true;
                return ScrollViewResponse::Handled;
            }
            KeyCode::Char(' ') if shifted => (Axis::Vertical, self.scroll_y.target() - page),
            KeyCode::Up | KeyCode::Char('k') if plain => {
                (Axis::Vertical, self.scroll_y.target() - layout.line)
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                (Axis::Vertical, self.scroll_y.target() + layout.line)
            }
            KeyCode::Left | KeyCode::Char('h') if plain => {
                (Axis::Horizontal, self.scroll_x.target() - layout.line)
            }
            KeyCode::Right | KeyCode::Char('l') if plain => {
                (Axis::Horizontal, self.scroll_x.target() + layout.line)
            }
            KeyCode::PageUp if plain => (Axis::Vertical, self.scroll_y.target() - page),
            KeyCode::PageDown | KeyCode::Char(' ') if plain => {
                (Axis::Vertical, self.scroll_y.target() + page)
            }
            KeyCode::Home if plain => (Axis::Vertical, 0.0),
            KeyCode::End | KeyCode::Char('G') if plain => (Axis::Vertical, f32::INFINITY),
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => return ScrollViewResponse::Ignored,
        };
        scroll_response(self.move_to(axis, target, true, now))
    }

    /// Scrolls by the wheel `wheel` over the view.
    fn handle_wheel(&mut self, wheel: &WheelInput) -> ScrollViewResponse {
        let Some(layout) = self
            .layout
            .filter(|layout| layout.area.contains(wheel.position))
        else {
            return ScrollViewResponse::Ignored;
        };
        let (delta, glide) = match wheel.delta {
            WheelDelta::Lines(lines) => (lines, true),
            WheelDelta::Pixels(pixels) => (pixels, false),
        };
        let scale = match wheel.delta {
            WheelDelta::Lines(_) => WHEEL_LINES * layout.line,
            WheelDelta::Pixels(_) => 1.0,
        };
        let (x, y) = match wheel.mods.contains(Mods::SHIFT) {
            true => (delta.x + delta.y, 0.0),
            false => (delta.x, delta.y),
        };
        let now = clock::now();
        // A positive delta moves the content right and down, toward the
        // top left.
        let across = self.scroll_x.target() - x * scale;
        let down = self.scroll_y.target() - y * scale;
        let moved_x = self.move_to(Axis::Horizontal, across, glide, now);
        let moved_y = self.move_to(Axis::Vertical, down, glide, now);
        scroll_response(moved_x || moved_y)
    }

    /// Reacts to pointer `input`: drags and pages with the scrollbars,
    /// lighting the one under the pointer.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ScrollViewResponse {
        match input {
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                match button.pressed {
                    true => self.press(button.position),
                    false => match self.drag.take() {
                        Some(_) => {
                            self.hover = self.bar_at(button.position);
                            ScrollViewResponse::Handled
                        }
                        None => ScrollViewResponse::Ignored,
                    },
                }
            }
            Input::PointerMoved(position) => match self.drag {
                Some(drag) => self.drag_to(drag, *position),
                None => {
                    self.set_hover(self.bar_at(*position), cx);
                    ScrollViewResponse::Ignored
                }
            },
            Input::PointerLeft => {
                self.set_hover(None, cx);
                ScrollViewResponse::Ignored
            }
            Input::FocusLost => {
                self.drag = None;
                ScrollViewResponse::Ignored
            }
            Input::Wheel(wheel) => self.handle_wheel(wheel),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_) => ScrollViewResponse::Ignored,
        }
    }

    /// Reacts to a left press at `point`: takes hold of the thumb under it,
    /// or pages toward it on a track.
    fn press(&mut self, point: Point) -> ScrollViewResponse {
        let Some(axis) = self.bar_at(point) else {
            return ScrollViewResponse::Ignored;
        };
        let Some(thumb) = self.thumb(axis) else {
            return ScrollViewResponse::Ignored;
        };
        let (start, length) = axis.span(thumb);
        let along = axis.pick(point);
        if (start..start + length).contains(&along) {
            self.drag = Some(ThumbDrag {
                axis,
                grab: along - start,
            });
            return ScrollViewResponse::Handled;
        }
        let Some(layout) = self.layout else {
            return ScrollViewResponse::Ignored;
        };
        let (_, view) = axis.span(layout.viewport);
        let direction = if along < start { -1.0 } else { 1.0 };
        let target = self.motion(axis).target() + direction * view;
        let moved = self.move_to(axis, target, true, clock::now());
        match moved {
            true => ScrollViewResponse::Scrolled,
            false => ScrollViewResponse::Handled,
        }
    }

    /// Moves the dragged thumb so the pointer at `point` holds it where it
    /// took hold, scrolling the content with it at once.
    fn drag_to(&mut self, drag: ThumbDrag, point: Point) -> ScrollViewResponse {
        let (Some(layout), Some(thumb)) = (self.layout, self.thumb(drag.axis)) else {
            return ScrollViewResponse::Ignored;
        };
        let Some(track) = drag.axis.track(&layout) else {
            return ScrollViewResponse::Ignored;
        };
        let (track_start, track_length) = drag.axis.span(track);
        let (_, length) = drag.axis.span(thumb);
        let room = (track_length - length).max(1.0);
        let share = (drag.axis.pick(point) - drag.grab - track_start) / room;
        let max = drag.axis.pick(self.max_offset(&layout));
        let moved = self.move_to(drag.axis, share * max, false, clock::now());
        if moved {
            self.drawn = Point::new(self.scroll_x.target(), self.scroll_y.target());
        }
        scroll_response(moved)
    }

    /// Lights the scrollbar `hover`, asking for a redraw when that changes.
    fn set_hover(&mut self, hover: Option<Axis>, cx: &mut EventCx<'_>) {
        if self.hover != hover {
            self.hover = hover;
            // Over a bar it shows; left, it holds a moment before fading.
            self.fade.wake(clock::now());
            cx.request_redraw();
        }
    }

    // ------------------------------------------------------------------
    // Geometry
    // ------------------------------------------------------------------

    /// Moves the offset along `axis` to `target`, held inside the content,
    /// gliding when `glide`; says whether the target changed.
    fn move_to(&mut self, axis: Axis, target: f32, glide: bool, now: Instant) -> bool {
        let max = self
            .layout
            .map_or(0.0, |layout| axis.pick(self.max_offset(&layout)));
        let target = target.clamp(0.0, max);
        let motion = self.motion_mut(axis);
        if target == motion.target() {
            return false;
        }
        match glide {
            true => motion.scroll_to(target, now),
            false => motion.jump_to(target),
        }
        self.fade.wake(now);
        true
    }

    /// The farthest the content scrolls each way in `layout`.
    fn max_offset(&self, layout: &ScrollLayout) -> Point {
        Point::new(
            (self.content.x - layout.viewport.width).max(0.0),
            (self.content.y - layout.viewport.height).max(0.0),
        )
    }

    /// The scrollbar whose track holds `point`, as last drawn.
    fn bar_at(&self, point: Point) -> Option<Axis> {
        let layout = self.layout?;
        [Axis::Vertical, Axis::Horizontal].into_iter().find(|axis| {
            axis.track(&layout)
                .is_some_and(|track| track.contains(point))
        })
    }

    /// Where the thumb of the scrollbar along `axis` lies, at its full
    /// track width, as last drawn.
    fn thumb(&self, axis: Axis) -> Option<Rect> {
        let layout = self.layout?;
        let track = axis.track(&layout)?;
        let (track_start, track_length) = axis.span(track);
        let (_, view) = axis.span(layout.viewport);
        let min = logical(MIN_THUMB, layout.scale_factor);
        let (start, length) = thumb_span(
            track_length,
            view,
            axis.pick(self.content),
            axis.pick(self.drawn),
            min,
        );
        Some(axis.with_span(track, track_start + start, length))
    }

    /// The easing of the offset along `axis`.
    fn motion(&self, axis: Axis) -> &SmoothScroll {
        match axis {
            Axis::Horizontal => &self.scroll_x,
            Axis::Vertical => &self.scroll_y,
        }
    }

    /// The easing of the offset along `axis`, to change.
    fn motion_mut(&mut self, axis: Axis) -> &mut SmoothScroll {
        match axis {
            Axis::Horizontal => &mut self.scroll_x,
            Axis::Vertical => &mut self.scroll_y,
        }
    }

    // ------------------------------------------------------------------
    // Drawing
    // ------------------------------------------------------------------

    /// Lays the view out in `area` of `frame` as of `now`, holding the
    /// offset inside the content, and draws its scrollbars; returns the
    /// viewport.
    pub(crate) fn draw_at(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        now: Instant,
    ) -> Rect {
        self.follow_reduced_motion(frame.reduced_motion());
        let scale_factor = frame.scale_factor();
        let track = match self.bars_hidden {
            true => 0.0,
            false => logical(TRACK_WIDTH, scale_factor),
        };
        let layout = lay_out(
            area,
            self.content,
            track,
            frame.cell_metrics().height,
            scale_factor,
        );
        self.layout = Some(layout);
        let max = self.max_offset(&layout);
        for (motion, max) in [(&mut self.scroll_x, max.x), (&mut self.scroll_y, max.y)] {
            if motion.target() > max {
                motion.jump_to(max);
            }
        }
        self.drawn = Point::new(
            self.scroll_x.position(now).round(),
            self.scroll_y.position(now).round(),
        );
        self.show = theme.scrollbar_show;
        if !self.bars_hidden {
            for axis in [Axis::Vertical, Axis::Horizontal] {
                self.draw_bar(frame, axis, theme, now);
            }
        }
        layout.viewport
    }

    /// Draws the scrollbar along `axis`, if the content overflows that way:
    /// its track while lit, and its thumb, wider while lit; faded toward
    /// the background as the theme's [`ScrollbarShow`] says, in full while
    /// lit unless it is never shown.
    fn draw_bar(&self, frame: &mut Frame, axis: Axis, theme: &Theme, now: Instant) {
        let (Some(layout), Some(thumb)) = (self.layout, self.thumb(axis)) else {
            return;
        };
        let Some(track) = axis.track(&layout) else {
            return;
        };
        let lit = self.hover == Some(axis) || self.drag.is_some_and(|drag| drag.axis == axis);
        let opacity = match (lit, self.show) {
            (true, ScrollbarShow::Auto) => 1.0,
            _ => self.fade.opacity(self.show, now),
        };
        if opacity <= 0.0 {
            return;
        }
        let faded = |color: Rgb| color.mix(theme.background, 1.0 - opacity);
        let scale_factor = layout.scale_factor;
        if lit {
            let fill = faded(theme.hover);
            frame.draw_box(track, &theme.shape.highlight(fill, scale_factor));
        }
        let (width, color) = match lit {
            true => (THUMB_WIDTH_HOVER, faded(theme.scrollbar_hover)),
            false => (THUMB_WIDTH, faded(theme.scrollbar)),
        };
        let width = logical(width, scale_factor);
        let shown = axis.across(thumb, width);
        frame.draw_box(shown, &theme.shape.highlight(color, scale_factor));
    }
}

impl Widget for ScrollView {
    type Output = Rect;
    type Response = ScrollViewResponse;

    /// Reacts to `input`: keys while focused, the wheel over the view, and
    /// the pointer on its scrollbars, as the module documentation lists.
    /// Asks for a redraw whenever it scrolled or used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ScrollViewResponse {
        self.follow_reduced_motion(cx.reduced_motion());
        let response = match input {
            Input::Key(key) => self.handle_key(*key),
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
            | Input::PointerMoved(_)
            | Input::Wheel(_) => self.handle_pointer(input, cx),
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the scrollbars the content needs inside `area` and returns the
    /// viewport left for the content.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        self.draw_at(frame, area, theme, clock::now())
    }

    /// Tells the view whether it has the keyboard; it takes keys only
    /// while it does.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the view has the keyboard, as the app last said.
    fn is_focused(&self) -> bool {
        self.focused
    }

    /// Asks for the next frame of a glide or a scrollbar's fade still under
    /// way, returning when it is due; `None` at rest. Call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait).
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.follow_reduced_motion(cx.reduced_motion());
        // Frames are drawn at whole pixels; the last one drawn may have been
        // a step short of the end.
        let target = Point::new(
            self.scroll_x.target().round(),
            self.scroll_y.target().round(),
        );
        let now = clock::now();
        let fade = self.fade.next_frame(self.show, now);
        let glide = (self.drawn != target).then(|| {
            [self.scroll_x, self.scroll_y]
                .iter()
                .filter_map(|motion| motion.next_frame(now))
                .min()
                .unwrap_or(now)
        });
        let next = [glide, fade].into_iter().flatten().min()?;
        cx.request_redraw_at(next);
        Some(next)
    }
}

impl Outcome for ScrollViewResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Axis Implementations
// ========================================================================

impl Axis {
    /// The part of `point` along this axis.
    fn pick(self, point: Point) -> f32 {
        match self {
            Self::Horizontal => point.x,
            Self::Vertical => point.y,
        }
    }

    /// Where `rect` starts along this axis, and how long it is.
    fn span(self, rect: Rect) -> (f32, f32) {
        match self {
            Self::Horizontal => (rect.x, rect.width),
            Self::Vertical => (rect.y, rect.height),
        }
    }

    /// `rect` cut along this axis to start at `start` and be `length` long.
    fn with_span(self, rect: Rect, start: f32, length: f32) -> Rect {
        match self {
            Self::Horizontal => Rect::new(start, rect.y, length, rect.height),
            Self::Vertical => Rect::new(rect.x, start, rect.width, length),
        }
    }

    /// `rect` narrowed across this axis to `width`, against its far edge.
    fn across(self, rect: Rect, width: f32) -> Rect {
        match self {
            Self::Horizontal => Rect::new(rect.x, rect.y + rect.height - width, rect.width, width),
            Self::Vertical => Rect::new(rect.x + rect.width - width, rect.y, width, rect.height),
        }
    }

    /// The track of this axis's scrollbar in `layout`, if it has one.
    fn track(self, layout: &ScrollLayout) -> Option<Rect> {
        match self {
            Self::Horizontal => layout.horizontal,
            Self::Vertical => layout.vertical,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `Scrolled` when the view `moved`, else `Ignored`.
fn scroll_response(moved: bool) -> ScrollViewResponse {
    match moved {
        true => ScrollViewResponse::Scrolled,
        false => ScrollViewResponse::Ignored,
    }
}

/// Lays out a view of `content` in `area`: a track `track` wide along each
/// side the content overflows (along the bottom taking room from the
/// right one's need, and the other way round), the rest the viewport.
fn lay_out(area: Rect, content: Point, track: f32, line: f32, scale_factor: f64) -> ScrollLayout {
    let mut vertical = content.y > area.height;
    let horizontal = content.x > area.width - if vertical { track } else { 0.0 };
    vertical = content.y > area.height - if horizontal { track } else { 0.0 };
    let width = (area.width - if vertical { track } else { 0.0 }).max(0.0);
    let height = (area.height - if horizontal { track } else { 0.0 }).max(0.0);
    let viewport = Rect::new(area.x, area.y, width, height);
    ScrollLayout {
        area,
        horizontal: horizontal.then(|| Rect::new(area.x, area.y + height, width, track)),
        line,
        scale_factor,
        vertical: vertical.then(|| Rect::new(area.x + width, area.y, track, height)),
        viewport,
    }
}

/// Where a thumb starts along a track `track` long, and how long it is,
/// for a viewport `view` long onto content `content` long scrolled to
/// `offset`: as long as the share of the content in sight, but at least
/// `min` (and no longer than the track).
fn thumb_span(track: f32, view: f32, content: f32, offset: f32, min: f32) -> (f32, f32) {
    if content <= 0.0 {
        return (0.0, track);
    }
    let length = (track * view / content).max(min).min(track);
    let max = (content - view).max(0.0);
    let share = if max > 0.0 { offset / max } else { 0.0 };
    ((track - length) * share.clamp(0.0, 1.0), length)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};

    const AREA: Rect = Rect::new(0.0, 0.0, 200.0, 100.0);

    /// A window `width` by `height` whose motion is reduced, so scrolls
    /// jump.
    fn still(width: u32, height: u32) -> TestWindow {
        let mut window = TestWindow::new(width, height);
        window.set_reduced_motion(true);
        window
    }

    /// A view of content 150 wide and 1000 tall (so only a vertical bar
    /// shows), drawn in [`AREA`] of `window`.
    fn view(window: &TestWindow) -> ScrollView {
        let mut view = ScrollView::new();
        view.set_content_size(150.0, 1000.0);
        draw(&mut view, window);
        view
    }

    fn draw(view: &mut ScrollView, window: &TestWindow) -> Rect {
        let mut frame = window.frame();
        view.draw(&mut frame, AREA, &Theme::dark())
    }

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    #[test]
    fn test_a_bar_shows_only_along_a_side_the_content_overflows() {
        let only_tall = lay_out(AREA, Point::new(150.0, 1000.0), 10.0, 16.0, 1.0);
        assert_eq!(only_tall.viewport, Rect::new(0.0, 0.0, 190.0, 100.0));
        assert_eq!(only_tall.horizontal, None);
        // 195 fits 200 but not the 190 left beside the vertical bar.
        let both = lay_out(AREA, Point::new(195.0, 1000.0), 10.0, 16.0, 1.0);
        assert_eq!(both.viewport, Rect::new(0.0, 0.0, 190.0, 90.0));
        assert_eq!(both.horizontal, Some(Rect::new(0.0, 90.0, 190.0, 10.0)));
        let fits = lay_out(AREA, Point::new(200.0, 100.0), 10.0, 16.0, 1.0);
        assert_eq!((fits.vertical, fits.horizontal), (None, None));
    }

    #[test]
    fn test_the_thumb_shows_the_share_in_sight_but_never_shrinks_past_its_minimum() {
        assert_eq!(thumb_span(100.0, 100.0, 400.0, 0.0, 10.0), (0.0, 25.0));
        assert_eq!(thumb_span(100.0, 100.0, 400.0, 300.0, 10.0), (75.0, 25.0));
        assert_eq!(thumb_span(100.0, 100.0, 400.0, 150.0, 10.0), (37.5, 25.0));
        assert_eq!(thumb_span(100.0, 10.0, 100_000.0, 0.0, 20.0).1, 20.0);
    }

    #[test]
    fn test_keys_scroll_only_while_focused_and_stop_at_the_ends() {
        let mut window = still(400, 300);
        let mut view = view(&window);
        assert_eq!(
            window.feed(&mut view, &key("j")),
            ScrollViewResponse::Ignored
        );
        view.set_focused(true);
        assert_eq!(
            window.feed(&mut view, &key("j")),
            ScrollViewResponse::Scrolled
        );
        assert!(window.take_redraw());
        draw(&mut view, &window);
        let line = window.frame().cell_metrics().height;
        assert_eq!(view.offset(), Point::new(0.0, line.round()));
        assert_eq!(
            window.feed(&mut view, &key("G")),
            ScrollViewResponse::Scrolled
        );
        draw(&mut view, &window);
        assert_eq!(view.offset().y, 900.0, "the content's end at the bottom");
        assert_eq!(
            window.feed(&mut view, &key("<pagedown>")),
            ScrollViewResponse::Ignored,
            "already at the end, so an outer view may scroll"
        );
        assert_eq!(
            window.feed(&mut view, &key("<home>")),
            ScrollViewResponse::Scrolled
        );
        draw(&mut view, &window);
        assert_eq!(view.offset().y, 0.0);
        window.feed(&mut view, &key("G"));
        assert_eq!(
            window.feed(&mut view, &key("g")),
            ScrollViewResponse::Handled
        );
        window.feed(&mut view, &key("j"));
        window.feed(&mut view, &key("g"));
        draw(&mut view, &window);
        assert!(view.offset().y > 0.0, "another key between cancels `g`");
        assert_eq!(
            window.feed(&mut view, &key("g")),
            ScrollViewResponse::Scrolled
        );
        draw(&mut view, &window);
        assert_eq!(view.offset().y, 0.0, "`g g` goes to the top");
    }

    #[test]
    fn test_the_wheel_scrolls_lines_or_pixels_and_shift_turns_it_sideways() {
        let mut window = still(400, 300);
        let mut view = view(&window);
        view.set_content_size(1000.0, 1000.0);
        draw(&mut view, &window);
        let inside = Point::new(50.0, 50.0);
        let pixels = WheelInput::new(WheelDelta::Pixels(Point::new(0.0, -30.0)), inside);
        window.feed(&mut view, &Input::Wheel(pixels));
        draw(&mut view, &window);
        assert_eq!(view.offset(), Point::new(0.0, 30.0));
        let notch = WheelDelta::Lines(Point::new(0.0, -1.0));
        let shifted = WheelInput::new(notch, inside).with_mods(Mods::SHIFT);
        window.feed(&mut view, &Input::Wheel(shifted));
        draw(&mut view, &window);
        let line = window.frame().cell_metrics().height;
        assert_eq!(
            view.offset(),
            Point::new((WHEEL_LINES * line).round(), 30.0)
        );
        let outside = WheelInput::new(notch, Point::new(300.0, 50.0));
        assert_eq!(
            window.feed(&mut view, &Input::Wheel(outside)),
            ScrollViewResponse::Ignored
        );
    }

    #[test]
    fn test_dragging_the_thumb_scrolls_in_proportion_and_a_track_press_pages() {
        let mut window = still(400, 300);
        let mut view = view(&window);
        let track = view
            .layout
            .and_then(|layout| layout.vertical)
            .expect("a bar");
        let thumb = view.thumb(Axis::Vertical).expect("a thumb");
        let x = track.x + track.width / 2.0;
        let grab = Point::new(x, thumb.y + 2.0);
        let press = ButtonInput::press(PointerButton::Left, grab);
        assert_eq!(
            window.feed(&mut view, &Input::PointerButton(press)),
            ScrollViewResponse::Handled
        );
        let room = track.height - thumb.height;
        let halfway = Point::new(x, grab.y + room / 2.0);
        assert_eq!(
            window.feed(&mut view, &Input::PointerMoved(halfway)),
            ScrollViewResponse::Scrolled
        );
        assert_eq!(view.offset().y, 450.0, "half of the 900 it can scroll");
        let release = ButtonInput::release(PointerButton::Left, halfway);
        window.feed(&mut view, &Input::PointerButton(release));
        draw(&mut view, &window);
        let below = Point::new(x, track.y + track.height - 1.0);
        let page = ButtonInput::press(PointerButton::Left, below);
        assert_eq!(
            window.feed(&mut view, &Input::PointerButton(page)),
            ScrollViewResponse::Scrolled
        );
        draw(&mut view, &window);
        assert_eq!(
            view.offset().y,
            450.0 + view.layout.expect("drawn").viewport.height
        );
    }

    #[test]
    fn test_reveal_scrolls_the_least_that_shows_the_rect() {
        let window = still(400, 300);
        let mut view = view(&window);
        view.reveal(Rect::new(0.0, 150.0, 10.0, 20.0));
        draw(&mut view, &window);
        assert_eq!(view.offset().y, 70.0, "its bottom at the viewport's");
        view.reveal(Rect::new(0.0, 100.0, 10.0, 20.0));
        draw(&mut view, &window);
        assert_eq!(view.offset().y, 70.0, "already in sight");
        view.reveal(Rect::new(0.0, 20.0, 10.0, 20.0));
        draw(&mut view, &window);
        assert_eq!(view.offset().y, 20.0, "its top at the viewport's");
    }

    #[test]
    fn test_a_shrunk_content_pulls_the_offset_back_inside() {
        let window = still(400, 300);
        let mut view = view(&window);
        view.scroll_to(Point::new(0.0, 800.0));
        view.set_content_size(150.0, 300.0);
        draw(&mut view, &window);
        assert_eq!(view.offset().y, 200.0);
    }

    #[test]
    fn test_a_key_glides_there_and_asks_for_frames_until_it_lands() {
        let mut window = TestWindow::new(400, 300);
        let mut view = view(&window);
        view.set_focused(true);
        window.feed(&mut view, &key("<pagedown>"));
        let start = Instant::now();
        let mut frame = window.frame();
        view.draw_at(&mut frame, AREA, &Theme::dark(), start);
        let target = view.scroll_y.target();
        assert!(view.offset().y < target, "on the way");
        assert!(view.about_to_wait(&mut window.cx()).is_some());
        let later = start + Duration::from_secs(1);
        view.draw_at(&mut frame, AREA, &Theme::dark(), later);
        assert_eq!(view.offset().y, target);
        assert_eq!(view.about_to_wait(&mut window.cx()), None);
    }

    #[test]
    fn test_the_view_tells_screen_readers_how_far_it_is_scrolled() {
        let window = still(400, 300);
        let mut view = view(&window);
        view.scroll_to(Point::new(0.0, 40.0));
        let mut frame = window.frame();
        view.draw(&mut frame, AREA, &Theme::dark());
        let id = FocusId::new("scroll");
        view.access(&mut frame, id);
        assert_eq!(ui_text(&frame, "t"), "Window \"t\" focused\n  ScrollView\n");
        let tree = crate::testing::access_tree(&frame, "t");
        let node = crate::testing::access_node(&tree, id).expect("described");
        assert_eq!(
            (node.scroll_y(), node.scroll_y_max()),
            (Some(40.0), Some(900.0))
        );
    }

    #[test]
    fn test_an_auto_scrollbar_shows_after_a_scroll_and_fades_out() {
        let theme = Theme {
            scrollbar_show: ScrollbarShow::Auto,
            ..Theme::dark()
        };
        let window = still(400, 300);
        let mut view = view(&window);
        let render = |view: &mut ScrollView, now: Instant| {
            offscreen::render(400, 300, |frame| {
                frame.clear(Rgba::from(theme.background));
                view.draw_at(frame, AREA, &theme, now);
            })
        };
        let Some(rest) = render(&mut view, clock::now()) else {
            return;
        };
        let thumb = view.thumb(Axis::Vertical).expect("a thumb");
        let at = (thumb.x + thumb.width - 1.0, thumb.y + 4.0);
        rest.assert_pixel(at.0, at.1, theme.background, "hidden before any scroll");
        view.scroll_to(Point::new(0.0, 40.0));
        let scrolled = clock::now();
        let shown = render(&mut view, scrolled).expect("rendered");
        let thumb = view.thumb(Axis::Vertical).expect("a thumb");
        let at = (thumb.x + thumb.width - 1.0, thumb.y + 4.0);
        shown.assert_pixel(at.0, at.1, theme.scrollbar, "shown after a scroll");
        let later =
            render(&mut view, scrolled + std::time::Duration::from_secs(3)).expect("rendered");
        later.assert_pixel(at.0, at.1, theme.background, "faded out");
    }

    #[test]
    fn test_the_thumb_is_drawn_in_the_border_color_and_lit_under_the_pointer() {
        let theme = Theme::dark();
        let mut window = still(400, 300);
        let mut view = view(&window);
        let Some(snapshot) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            view.draw(frame, AREA, &theme);
        }) else {
            return;
        };
        let thumb = view.thumb(Axis::Vertical).expect("a thumb");
        let right = thumb.x + thumb.width - 1.0;
        snapshot.assert_pixel(right, thumb.y + 4.0, theme.scrollbar, "resting thumb");
        snapshot.assert_pixel(
            thumb.x,
            thumb.y + 4.0,
            theme.background,
            "a thin thumb at rest",
        );
        window.feed(
            &mut view,
            &Input::PointerMoved(Point::new(right, thumb.y + 4.0)),
        );
        let Some(lit) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            view.draw(frame, AREA, &theme);
        }) else {
            return;
        };
        lit.assert_pixel(right, thumb.y + 4.0, theme.muted, "lit thumb");
        lit.assert_pixel(thumb.x, thumb.y + 4.0, theme.hover, "lit track");
    }
}

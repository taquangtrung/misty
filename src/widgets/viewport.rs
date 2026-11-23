//! [`Viewport`]: the pan and zoom of content drawn in an area of the
//! window, such as an image, a PDF page, or a graph of notes. It maps
//! points of the content (in its own units: an image's pixels, a page's
//! points) to the window and back, and turns the pointer and keys into
//! pans and zooms. The app draws its content itself: inside
//! [`Frame::with_transform`](crate::Frame::with_transform) with
//! [`Viewport::transform`], in content units, or at
//! [`Viewport::screen_rect`], in window pixels, where text keeps the
//! upright renderer's crisp glyphs.
//!
//! Input: the wheel pans (Shift turns it sideways), and with Ctrl (or Cmd
//! on macOS) it zooms around the pointer; a left or middle drag pans.
//! Keys, while the app says the viewport is focused: the arrows and `h`
//! `j` `k` `l` pan, `+` (or `=`) and `-` zoom around the middle, `0` fits
//! the content in the area, `w` fits its width, and `1` shows it at its
//! actual size, a content unit to a physical pixel.
//!
//! A viewport made with [`Viewport::new`] keeps its content in sight: it
//! centers content smaller than the area and stops a pan at the content's
//! edges. [`Viewport::with_free_pan`] lets it pan anywhere, for a canvas.
//! Until the user pans or zooms, it keeps the content fitted to the area
//! as the area changes size.

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::base::transform::Transform;
use super::super::input::event::{Input, PointerButton, WheelDelta};

// ========================================================================
// Constants
// ========================================================================

/// How much one wheel notch or one key zooms in.
const ZOOM_STEP: f32 = 1.25;

/// How many pixels of a touchpad zoom as much as a notch.
const PIXELS_PER_NOTCH: f32 = 50.0;

/// How far a key or a wheel notch pans, in logical pixels.
const PAN_STEP: f32 = 48.0;

/// The farthest out it zooms unless the app says otherwise: a sixteenth.
const DEFAULT_MIN_ZOOM: f32 = 1.0 / 16.0;

/// The farthest in it zooms unless the app says otherwise: 64 times.
const DEFAULT_MAX_ZOOM: f32 = 64.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The pan and zoom of content in an area. Content point `p` is drawn at
/// `area.x + offset.x + p.x * zoom`, likewise down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Where it is drawn, in physical pixels.
    area: Rect,
    /// The size of the content, in its own units.
    content: Point,
    /// The pointer's last position during a drag, or `None` when not
    /// dragging.
    drag: Option<Point>,
    /// Whether it keeps fitting the content to the area.
    fitted: bool,
    /// Whether the app gave it the keyboard.
    focused: bool,
    /// Whether it pans anywhere rather than keeping the content in sight.
    free_pan: bool,
    /// The farthest in it zooms.
    max_zoom: f32,
    /// The farthest out it zooms.
    min_zoom: f32,
    /// Where the content's origin is drawn, from the area's top left.
    offset: Point,
    /// Physical pixels per logical pixel, for the pan step.
    scale_factor: f32,
    /// Physical pixels per content unit.
    zoom: f32,
}

/// What became of an input offered to a [`Viewport`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ViewportResponse {
    /// It panned or zoomed: redraw the content.
    Changed,
    /// It used the input; nothing moved yet (a drag began).
    Handled,
    /// The input is not the viewport's.
    Ignored,
}

// ========================================================================
// Viewport Implementations
// ========================================================================

impl Viewport {
    /// A viewport of content `width` by `height` units, kept in sight and
    /// fitted to its area until the user pans or zooms.
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            area: Rect::default(),
            content: Point::new(width.max(0.0), height.max(0.0)),
            drag: None,
            fitted: true,
            focused: false,
            free_pan: false,
            max_zoom: DEFAULT_MAX_ZOOM,
            min_zoom: DEFAULT_MIN_ZOOM,
            offset: Point::default(),
            scale_factor: 1.0,
            zoom: 1.0,
        }
    }

    /// This viewport panning anywhere, at the actual size until it is
    /// fitted: for a canvas larger than any content drawn on it.
    pub fn with_free_pan(self) -> Self {
        Self {
            fitted: false,
            free_pan: true,
            ..self
        }
    }

    /// This viewport zooming no further out than `min` and in than `max`.
    pub fn with_zoom_range(self, min: f32, max: f32) -> Self {
        Self {
            max_zoom: max.max(min),
            min_zoom: min.max(f32::MIN_POSITIVE),
            ..self
        }
    }

    /// Sets the content's size, in its own units, refitting it when fitted.
    pub fn set_content_size(&mut self, width: f32, height: f32) {
        self.content = Point::new(width.max(0.0), height.max(0.0));
        self.settle();
    }

    /// The content's size, in its own units.
    pub fn content_size(&self) -> Point {
        self.content
    }

    /// Where the content is drawn, as the last [`Widget::draw`] took it.
    pub fn area(&self) -> Rect {
        self.area
    }

    /// Physical pixels per content unit.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Where content point `point` is drawn in the window.
    pub fn to_screen(&self, point: Point) -> Point {
        Point::new(
            self.area.x + self.offset.x + point.x * self.zoom,
            self.area.y + self.offset.y + point.y * self.zoom,
        )
    }

    /// The content point drawn at window point `point`.
    pub fn to_content(&self, point: Point) -> Point {
        Point::new(
            (point.x - self.area.x - self.offset.x) / self.zoom,
            (point.y - self.area.y - self.offset.y) / self.zoom,
        )
    }

    /// Where content rect `rect` is drawn in the window.
    pub fn screen_rect(&self, rect: Rect) -> Rect {
        let origin = self.to_screen(Point::new(rect.x, rect.y));
        Rect::new(
            origin.x,
            origin.y,
            rect.width * self.zoom,
            rect.height * self.zoom,
        )
    }

    /// The part of the content in sight, in its units: what to draw.
    pub fn visible_content(&self) -> Rect {
        let start = self.to_content(Point::new(self.area.x, self.area.y));
        Rect::new(
            start.x,
            start.y,
            self.area.width / self.zoom,
            self.area.height / self.zoom,
        )
    }

    /// The transform that draws content where the viewport shows it:
    /// scaled by the zoom, then moved to the area.
    pub fn transform(&self) -> Transform {
        Transform::scale(self.zoom, self.zoom).then(Transform::translate(
            self.area.x + self.offset.x,
            self.area.y + self.offset.y,
        ))
    }

    /// Zooms by `factor` (above 1 in, below 1 out), within the zoom range,
    /// keeping the content under window point `around` where it is.
    pub fn zoom_at(&mut self, factor: f32, around: Point) {
        let anchor = self.to_content(around);
        self.zoom = (self.zoom * factor).clamp(self.min_zoom, self.max_zoom);
        self.offset = Point::new(
            around.x - self.area.x - anchor.x * self.zoom,
            around.y - self.area.y - anchor.y * self.zoom,
        );
        self.fitted = false;
        self.settle();
    }

    /// Pans the content `dx` pixels right and `dy` down.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.offset = Point::new(self.offset.x + dx, self.offset.y + dy);
        self.fitted = false;
        self.settle();
    }

    /// Fits the whole content in the area, centered, and keeps it fitted
    /// as the area changes until the user pans or zooms.
    pub fn fit(&mut self) {
        self.fitted = true;
        self.settle();
    }

    /// Zooms so the content's width fills the area's, its top in sight.
    pub fn fit_width(&mut self) {
        if self.content.x > 0.0 {
            self.zoom = (self.area.width / self.content.x).clamp(self.min_zoom, self.max_zoom);
        }
        self.offset = Point::new(0.0, 0.0);
        self.fitted = false;
        self.settle();
    }

    /// Shows the content at its actual size, a content unit to a physical
    /// pixel, keeping the middle of the area where it is.
    pub fn actual_size(&mut self) {
        let middle = Point::new(
            self.area.x + self.area.width / 2.0,
            self.area.y + self.area.height / 2.0,
        );
        self.zoom_at(1.0 / self.zoom, middle);
    }

    /// Carries out a key while focused; whether it was the viewport's.
    fn handle_key(&mut self, key: Key) -> bool {
        if !self.focused || !(key.mods == Mods::NONE || key.mods == Mods::SHIFT) {
            return false;
        }
        let step = PAN_STEP * self.scale_factor;
        let middle = Point::new(
            self.area.x + self.area.width / 2.0,
            self.area.y + self.area.height / 2.0,
        );
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => self.pan_by(step, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.pan_by(-step, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.pan_by(0.0, step),
            KeyCode::Down | KeyCode::Char('j') => self.pan_by(0.0, -step),
            KeyCode::Char('+' | '=') => self.zoom_at(ZOOM_STEP, middle),
            KeyCode::Char('-') => self.zoom_at(1.0 / ZOOM_STEP, middle),
            KeyCode::Char('0') => self.fit(),
            KeyCode::Char('1') => self.actual_size(),
            KeyCode::Char('w') => self.fit_width(),
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
            | KeyCode::Tab => return false,
        }
        true
    }

    /// Fits the content while fitted, then keeps it in sight unless
    /// panning is free: centered on an axis it is smaller on, its edges
    /// held outside the area on an axis it is larger on.
    fn settle(&mut self) {
        if self.fitted && self.content.x > 0.0 && self.content.y > 0.0 {
            let fit = (self.area.width / self.content.x).min(self.area.height / self.content.y);
            self.zoom = fit.clamp(self.min_zoom, self.max_zoom);
        }
        if self.free_pan {
            return;
        }
        let hold = |offset: f32, view: f32, content: f32| match content <= view {
            true => ((view - content) / 2.0).round(),
            false => offset.clamp(view - content, 0.0),
        };
        self.offset = Point::new(
            hold(self.offset.x, self.area.width, self.content.x * self.zoom),
            hold(self.offset.y, self.area.height, self.content.y * self.zoom),
        );
    }
}

// ========================================================================
// Widget Implementations
// ========================================================================

impl Widget for Viewport {
    type Output = ();
    type Response = ViewportResponse;

    /// Reacts to `input`: the wheel and drags over the area, and keys
    /// while focused, as the module documentation lists. Asks for a redraw
    /// whenever it moved.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ViewportResponse {
        let before = (self.zoom, self.offset);
        let used = match input {
            Input::Key(key) => self.handle_key(*key),
            Input::Wheel(wheel) if self.area.contains(wheel.position) => {
                let (delta, notches) = match wheel.delta {
                    WheelDelta::Lines(lines) => (lines, lines.y),
                    WheelDelta::Pixels(pixels) => (pixels, pixels.y / PIXELS_PER_NOTCH),
                };
                match wheel.mods.contains(Mods::PRIMARY) {
                    true => self.zoom_at(ZOOM_STEP.powf(notches), wheel.position),
                    false => {
                        let step = match wheel.delta {
                            WheelDelta::Lines(_) => PAN_STEP * self.scale_factor,
                            WheelDelta::Pixels(_) => 1.0,
                        };
                        let (dx, dy) = match wheel.mods.contains(Mods::SHIFT) {
                            true => (delta.y, delta.x),
                            false => (delta.x, delta.y),
                        };
                        self.pan_by(dx * step, dy * step);
                    }
                }
                true
            }
            Input::PointerButton(button)
                if matches!(button.button, PointerButton::Left | PointerButton::Middle) =>
            {
                match (button.pressed, self.area.contains(button.position)) {
                    (true, true) => {
                        self.drag = Some(button.position);
                        true
                    }
                    (true, false) => false,
                    (false, _) => self.drag.take().is_some(),
                }
            }
            Input::PointerMoved(point) => match self.drag {
                Some(last) => {
                    self.pan_by(point.x - last.x, point.y - last.y);
                    self.drag = Some(*point);
                    true
                }
                None => false,
            },
            Input::FocusLost => {
                self.drag = None;
                false
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::Wheel(_) => false,
        };
        if (self.zoom, self.offset) != before {
            cx.request_redraw();
            return ViewportResponse::Changed;
        }
        match used {
            true => ViewportResponse::Handled,
            false => ViewportResponse::Ignored,
        }
    }

    /// Draws nothing: takes `area` (in physical pixels) as where the
    /// content is drawn this frame, at the frame's scale factor, so call it
    /// each frame before drawing the content. The app then draws its
    /// content at the viewport's [`Viewport::transform`] or
    /// [`Viewport::screen_rect`].
    fn draw(&mut self, frame: &mut Frame, area: Rect, _theme: &Theme) {
        self.area = area;
        self.scale_factor = frame.scale_factor() as f32;
        self.settle();
    }

    /// Tells it whether it has the keyboard; it takes keys only while it
    /// does.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Outcome for ViewportResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::parse_key;
    use crate::input::event::WheelInput;
    use crate::testing::TestWindow;

    const AREA: Rect = Rect::new(10.0, 20.0, 400.0, 300.0);

    fn viewport() -> Viewport {
        placed(Viewport::new(800.0, 300.0))
    }

    /// `viewport` drawn in [`AREA`], as a frame places it.
    fn placed(mut viewport: Viewport) -> Viewport {
        viewport.draw(&mut TestWindow::new(800, 600).frame(), AREA, &Theme::dark());
        viewport
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
    }

    #[test]
    fn test_mapping_to_the_screen_and_back_is_the_same_point() {
        let mut viewport = viewport();
        viewport.zoom_at(3.0, Point::new(100.0, 100.0));
        let point = Point::new(123.5, 45.25);
        assert!(close(viewport.to_content(viewport.to_screen(point)), point));
        let rect = viewport.screen_rect(Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(rect.width, 10.0 * viewport.zoom());
        assert!(close(
            viewport.transform().apply(point),
            viewport.to_screen(point)
        ));
    }

    #[test]
    fn test_a_zoom_keeps_the_point_under_the_pointer_still() {
        // Free, so no centering or edge moves the content after the zoom.
        let mut viewport = placed(Viewport::new(800.0, 300.0).with_free_pan());
        let pointer = Point::new(210.0, 170.0);
        let under = viewport.to_content(pointer);
        viewport.zoom_at(ZOOM_STEP, pointer);
        assert!(close(viewport.to_content(pointer), under));
        viewport.zoom_at(1000.0, pointer);
        assert_eq!(viewport.zoom(), DEFAULT_MAX_ZOOM, "held to the range");
    }

    #[test]
    fn test_fitting_centers_and_pans_stop_at_the_edges() {
        let mut viewport = viewport();
        assert_eq!(viewport.zoom(), 0.5, "the width limits the fit");
        assert_eq!(
            viewport.to_screen(Point::new(0.0, 0.0)),
            Point::new(10.0, 20.0 + 75.0),
            "centered down"
        );
        viewport.fit_width();
        assert_eq!(viewport.zoom(), 0.5);
        viewport.actual_size();
        assert_eq!(viewport.zoom(), 1.0);
        viewport.pan_by(1000.0, 0.0);
        assert_eq!(
            viewport.to_screen(Point::new(0.0, 0.0)).x,
            AREA.x,
            "the left edge stops"
        );
        viewport.pan_by(-5000.0, 0.0);
        let right = viewport.to_screen(Point::new(800.0, 0.0)).x;
        assert_eq!(right, AREA.x + AREA.width, "the right edge stops");
        let mut free = placed(Viewport::new(800.0, 300.0).with_free_pan());
        free.pan_by(1000.0, 0.0);
        assert_eq!(free.to_screen(Point::new(0.0, 0.0)).x, AREA.x + 1000.0);
    }

    #[test]
    fn test_ctrl_wheel_zooms_a_drag_pans_and_keys_need_focus() {
        let mut window = TestWindow::new(500, 400);
        let mut viewport = viewport();
        viewport.actual_size();
        let at = Point::new(100.0, 100.0);
        let wheel = |mods| {
            Input::Wheel(WheelInput {
                delta: WheelDelta::Lines(Point::new(0.0, 1.0)),
                mods,
                position: at,
            })
        };
        let response = window.send(&wheel(Mods::PRIMARY), |input, cx| {
            viewport.handle(input, cx)
        });
        assert_eq!(response, ViewportResponse::Changed);
        assert_eq!(viewport.zoom(), ZOOM_STEP);
        let before = viewport.to_screen(Point::new(0.0, 0.0));
        let press = crate::input::event::ButtonInput::press(PointerButton::Left, at);
        window.send(&Input::PointerButton(press), |input, cx| {
            viewport.handle(input, cx)
        });
        window.send(&Input::PointerMoved(Point::new(90.0, 95.0)), |input, cx| {
            viewport.handle(input, cx)
        });
        let after = viewport.to_screen(Point::new(0.0, 0.0));
        assert_eq!((after.x - before.x, after.y - before.y), (-10.0, -5.0));
        let key = Input::Key(parse_key("0").expect("a key"));
        assert_eq!(
            window.send(&key, |input, cx| viewport.handle(input, cx)),
            ViewportResponse::Ignored
        );
        viewport.set_focused(true);
        assert_eq!(
            window.send(&key, |input, cx| viewport.handle(input, cx)),
            ViewportResponse::Changed
        );
        assert_eq!(viewport.zoom(), 0.5, "fitted again");
    }
}

//! [`ImageView`]: an image preview to zoom, pan, turn, and mirror, as a
//! file manager's or a reader's: fitted to its area at first, on a
//! checkerboard where the image is transparent, its pixels drawn as sharp
//! squares once zoomed in far enough to see them. It plays an animated
//! GIF or WebP when the app calls [`ImageView::about_to_wait`].
//!
//! Input: as a [`Viewport`] takes it (the wheel, Ctrl+wheel, drags, and
//! keys while focused: the arrows and `h` `j` `k` `l` pan, `+` and `-`
//! zoom, `0` fits, `w` fits the width, `1` is the actual size), and while
//! focused `r` turns the image a quarter clockwise, Shift+R back, and `m`
//! mirrors it.

use std::time::{Duration, Instant};

use accesskit::{Node, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::icon::logical;
use super::super::render::image::{AnimatedImage, Image, ImageDraw};
use super::viewport::{Viewport, ViewportResponse};

// ========================================================================
// Constants
// ========================================================================

/// The zoom from which pixels are drawn as sharp squares.
const SHARP_ZOOM: f32 = 2.0;

/// The side of a checkerboard square, in logical pixels.
const CHECKER: f32 = 8.0;

/// The quarter turns of a whole turn.
const QUARTER_TURNS: u8 = 4;

/// How many percent a zoom of 1 is, for screen readers.
const PERCENT: f32 = 100.0;

// ========================================================================
// Data Structures
// ========================================================================

/// An image to zoom, pan, turn, and mirror. The app owns it, hands it the
/// window's pointer events (and its keys while it has the focus), and
/// draws it each frame.
#[derive(Clone, Debug)]
pub struct ImageView {
    /// What it shows, for screen readers.
    alt: String,
    /// The frames of an animated image, and when it started playing.
    animation: Option<(AnimatedImage, Instant)>,
    /// The image shown (an animation's first frame).
    image: Image,
    /// How it is turned and mirrored.
    style: ImageDraw,
    /// Its pan and zoom.
    viewport: Viewport,
}

// ========================================================================
// ImageView Implementations
// ========================================================================

impl ImageView {
    /// A view of `image`, described to screen readers as `alt`, fitted to
    /// its area.
    pub fn new(image: Image, alt: impl Into<String>) -> Self {
        let viewport = Viewport::new(image.width() as f32, image.height() as f32);
        Self {
            alt: alt.into(),
            animation: None,
            image,
            style: ImageDraw::default(),
            viewport,
        }
    }

    /// A view of `animation`, playing from now; a still image when it has
    /// no frames but one.
    pub fn animated(animation: AnimatedImage, alt: impl Into<String>) -> Option<Self> {
        let first = animation.frames().first()?.image.clone();
        let mut view = Self::new(first, alt);
        if animation.frames().len() > 1 {
            view.animation = Some((animation, clock::now()));
        }
        Some(view)
    }

    /// Its pan and zoom, to fit or zoom it from the app's own commands.
    pub fn viewport_mut(&mut self) -> &mut Viewport {
        &mut self.viewport
    }

    /// Physical pixels per image pixel.
    pub fn zoom(&self) -> f32 {
        self.viewport.zoom()
    }

    /// Turns the image `quarter_turns` quarters clockwise (negative back)
    /// and fits it again.
    pub fn turn(&mut self, quarter_turns: i32) {
        let turns = (i32::from(self.style.quarter_turns) + quarter_turns)
            .rem_euclid(i32::from(QUARTER_TURNS));
        self.style.quarter_turns = turns as u8;
        let (width, height) = self.turned_size();
        self.viewport.set_content_size(width, height);
        self.viewport.fit();
    }

    /// Mirrors the image left to right, or back.
    pub fn flip(&mut self) {
        self.style.flipped = !self.style.flipped;
    }

    /// How the image is turned and mirrored now.
    pub fn orientation(&self) -> ImageDraw {
        self.style
    }

    /// Describes the view to screen readers as `id`: an image named by its
    /// text, its zoom as its value.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let mut node = Node::new(Role::Image);
        node.set_label(self.alt.as_str());
        node.set_value(format!("{:.0}%", self.viewport.zoom() * PERCENT));
        frame.access(id, self.viewport.area(), node);
    }

    /// The image's size as turned.
    fn turned_size(&self) -> (f32, f32) {
        let (width, height) = (self.image.width() as f32, self.image.height() as f32);
        match self.style.quarter_turns % 2 {
            0 => (width, height),
            _ => (height, width),
        }
    }

    /// The frame shown at `now`.
    fn shown(&self, now: Instant) -> &Image {
        match &self.animation {
            Some((animation, start)) => {
                frame_at(animation, now.saturating_duration_since(*start)).0
            }
            None => &self.image,
        }
    }

    /// Draws a checkerboard of the theme's surface and hover colors over
    /// `rect`, squares counted from `origin`.
    fn draw_checkerboard(frame: &mut Frame, rect: Rect, origin: Point, theme: &Theme) {
        let side = logical(CHECKER, frame.scale_factor());
        frame.fill_rect(rect, theme.surface);
        let first_col = ((rect.x - origin.x) / side).floor() as i64;
        let first_row = ((rect.y - origin.y) / side).floor() as i64;
        let cols = (rect.width / side).ceil() as i64 + 1;
        let rows = (rect.height / side).ceil() as i64 + 1;
        frame.clipped(rect, |frame| {
            for row in first_row..first_row + rows {
                for col in first_col..first_col + cols {
                    if (row + col).rem_euclid(2) == 0 {
                        continue;
                    }
                    let square = Rect::new(
                        origin.x + col as f32 * side,
                        origin.y + row as f32 * side,
                        side,
                        side,
                    );
                    frame.fill_rect(square, theme.hover);
                }
            }
        });
    }
}

impl Widget for ImageView {
    type Output = ();
    type Response = ViewportResponse;

    /// Reacts to `input`: turning and mirroring keys while focused, and
    /// the pans and zooms of its [`Viewport`]. Asks for a redraw whenever
    /// something changed.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ViewportResponse {
        if let Input::Key(key) = input
            && self.viewport.is_focused()
        {
            let turn_back = Key::new(KeyCode::Char('R'), Mods::SHIFT);
            if *key == Key::new(KeyCode::Char('r'), Mods::NONE) {
                self.turn(1);
            } else if *key == turn_back {
                self.turn(-1);
            } else if *key == Key::new(KeyCode::Char('m'), Mods::NONE) {
                self.flip();
            } else {
                return self.viewport.handle(input, cx);
            }
            cx.request_redraw();
            return ViewportResponse::Changed;
        }
        self.viewport.handle(input, cx)
    }

    /// Draws the image in `area`: a checkerboard under it, the image
    /// turned and mirrored, sharp at a high zoom, cut to `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (width, height) = self.turned_size();
        self.viewport.set_content_size(width, height);
        self.viewport.draw(frame, area, theme);
        let rect = self
            .viewport
            .screen_rect(Rect::new(0.0, 0.0, width, height));
        let style = ImageDraw {
            nearest: self.viewport.zoom() >= SHARP_ZOOM,
            ..self.style
        };
        let image = self.shown(clock::now()).clone();
        frame.clipped(area, |frame| {
            if let Some(under) = rect.intersection(&area) {
                Self::draw_checkerboard(frame, under, Point::new(rect.x, rect.y), theme);
            }
            frame.draw_image_as(&image, rect, style);
        });
    }

    /// Tells it whether it has the keyboard; it takes keys only while it
    /// does.
    fn set_focused(&mut self, focused: bool) {
        self.viewport.set_focused(focused);
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.viewport.is_focused()
    }

    /// Asks for the redraw that shows an animation's next frame, returning
    /// when it is due; `None` for a still image. Call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait).
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let (animation, start) = self.animation.as_ref()?;
        let now = clock::now();
        let (_, left) = frame_at(animation, now.saturating_duration_since(*start));
        let next = now.checked_add(left)?;
        cx.request_redraw_at(next);
        Some(next)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The frame of `animation` showing `elapsed` after it started (looping),
/// and how long it shows on from then.
fn frame_at(animation: &AnimatedImage, elapsed: Duration) -> (&Image, Duration) {
    let frames = animation.frames();
    let total: Duration = frames.iter().map(|frame| frame.delay).sum();
    if total.is_zero() {
        return (&frames[0].image, Duration::MAX);
    }
    let mut at = Duration::from_nanos((elapsed.as_nanos() % total.as_nanos()) as u64);
    for frame in frames {
        if at < frame.delay {
            return (&frame.image, frame.delay - at);
        }
        at -= frame.delay;
    }
    (&frames[0].image, frames[0].delay)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::{Rgb, Rgba};
    use crate::base::key::parse_key;
    use crate::render::image::AnimationFrame;
    use crate::testing::TestWindow;

    const RED: Rgb = Rgb::new(255, 0, 0);
    const GREEN: Rgb = Rgb::new(0, 255, 0);

    /// A 2 by 1 image: red on the left, green on the right.
    fn pair() -> Image {
        let pixels = [RED, GREEN]
            .iter()
            .flat_map(|color| [color.r, color.g, color.b, u8::MAX])
            .collect();
        Image::from_rgba(2, 1, pixels).expect("an image")
    }

    #[test]
    fn test_turning_swaps_the_shape_and_keys_turn_and_mirror_while_focused() {
        let mut window = TestWindow::new(200, 100);
        let mut view = ImageView::new(pair(), "two pixels");
        let key = |spec: &str| Input::Key(parse_key(spec).expect("a key"));
        assert_eq!(window.feed(&mut view, &key("r")), ViewportResponse::Ignored);
        view.set_focused(true);
        assert_eq!(window.feed(&mut view, &key("r")), ViewportResponse::Changed);
        assert_eq!(view.orientation().quarter_turns, 1);
        assert_eq!(view.viewport.content_size(), Point::new(1.0, 2.0));
        window.feed(&mut view, &key("S-R"));
        window.feed(&mut view, &key("S-R"));
        assert_eq!(view.orientation().quarter_turns, 3, "back past zero");
        window.feed(&mut view, &key("m"));
        assert!(view.orientation().flipped);
    }

    #[test]
    fn test_an_animation_plays_its_frames_round_and_round() {
        let frames = [RED, GREEN].map(|color| AnimationFrame {
            delay: Duration::from_millis(100),
            image: Image::from_rgba(1, 1, vec![color.r, color.g, color.b, u8::MAX])
                .expect("an image"),
        });
        let animation = AnimatedImage::from_frames(frames.to_vec()).expect("frames");
        let (shown, left) = frame_at(&animation, Duration::from_millis(350));
        assert_eq!(shown.pixels()[..3], [GREEN.r, GREEN.g, GREEN.b]);
        assert_eq!(left, Duration::from_millis(50));
    }

    #[test]
    fn test_a_turned_image_fits_its_area_with_its_left_pixel_on_top() {
        let mut view = ImageView::new(pair(), "two pixels");
        view.turn(1);
        let theme = Theme::dark();
        let Some(snapshot) = offscreen::render(100, 100, |frame| {
            frame.clear(Rgba::from(theme.background));
            view.draw(frame, Rect::new(0.0, 0.0, 100.0, 100.0), &theme);
        }) else {
            return;
        };
        assert!(view.zoom() >= SHARP_ZOOM, "zoomed in: sharp pixels");
        snapshot.assert_pixel(50.0, 25.0, RED, "the left pixel, turned to the top");
        snapshot.assert_pixel(50.0, 75.0, GREEN, "the right pixel, turned to the bottom");
        snapshot.assert_pixel(10.0, 50.0, theme.background, "beside the fitted image");
    }
}

//! Offscreen rendering for tests: draws a [`Frame`] into a texture with no
//! window, reads the pixels back, and checks colors at chosen points, so a
//! widget's drawing can be tested by what reaches the screen.
//!
//! Every test shares one headless GPU context and one set of fonts. When no
//! adapter is found (no GPU, no software rasterizer), [`render`] prints why
//! and returns `None`, and the test skips its checks; with
//! `MISTY_REQUIRE_GPU` set, as `make test-render` sets it, it fails instead.

use std::env;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, Once, OnceLock, PoisonError, mpsc};
#[cfg(any(test, feature = "testing"))]
use std::time::{Duration, Instant};

use wgpu::{
    BufferDescriptor, BufferUsages, COPY_BYTES_PER_ROW_ALIGNMENT, CommandEncoderDescriptor,
    Extent3d, MapMode, PollType, TexelCopyBufferInfo, TexelCopyBufferLayout, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
};

use super::super::base::color::Rgb;
use super::super::base::geometry::Rect;
use super::super::render::font::{CellMetrics, Font};
use super::super::render::gpu::{Gpu, GpuContext};
use super::super::render::{Fonts, FrameTarget, Renderer, ShapingFont};
use super::frame::Frame;

// ========================================================================
// Constants
// ========================================================================

/// The format frames are drawn in: 8-bit sRGB, so a pixel read back holds
/// the same channel values as the theme color it was drawn in.
const FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

/// The bytes one pixel of [`FORMAT`] takes.
const BYTES_PER_PIXEL: u32 = 4;

/// The scale factor frames are drawn at.
const SCALE_FACTOR: f64 = 1.0;

/// How far a channel may stray from the expected color and still match:
/// backends round blending and sRGB conversion slightly differently.
const CHANNEL_TOLERANCE: u8 = 3;

/// The environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_VAR: &str = "MISTY_REQUIRE_GPU";

/// The label GPU debuggers show for the target texture.
const TARGET_LABEL: &str = "misty offscreen target";

/// The label GPU debuggers show for the readback buffer.
const READBACK_LABEL: &str = "misty offscreen readback";

/// The harness every offscreen test shares, or why there is none.
static HARNESS: OnceLock<Mutex<Result<Harness, String>>> = OnceLock::new();

thread_local! {
    /// The system fonts plain frames measure text with, and the size of a
    /// cell in the default font: loaded once a thread, as loading scans
    /// the system's fonts (tens of milliseconds), which a test drawing a
    /// frame an input would otherwise pay each frame.
    static PLAIN_FONTS: (Arc<Mutex<Fonts>>, CellMetrics) = {
        let mut fonts = Fonts::new();
        let metrics = fonts.measure_cell(&Font::default(), SCALE_FACTOR);
        (Arc::new(Mutex::new(fonts)), metrics)
    };
}

// ========================================================================
// Data Structures
// ========================================================================

/// What every offscreen test draws with.
struct Harness {
    /// The font frames are drawn in.
    font: Font,
    /// The installed fonts and rasterized glyphs.
    fonts: Arc<Mutex<Fonts>>,
    /// The headless GPU.
    gpu: GpuContext,
    /// The size of a cell in `font`.
    metrics: CellMetrics,
    /// The pipelines, made for [`FORMAT`].
    renderer: Renderer,
}

/// How long frames took to build and draw, GPU work included, as
/// [`time_frames`](crate::testing::time_frames) measures them.
#[cfg(any(test, feature = "testing"))]
#[derive(Clone, Copy, Debug)]
pub struct FrameTimes {
    /// The slowest frame.
    pub max: Duration,
    /// The average frame.
    pub mean: Duration,
}

/// The pixels of one rendered frame, to check colors at chosen points.
#[derive(Debug)]
pub struct Snapshot {
    /// The frame's height in pixels.
    height: u32,
    /// The size of a cell the frame was laid out with.
    metrics: CellMetrics,
    /// RGBA bytes, row by row, without padding.
    pixels: Vec<u8>,
    /// The frame's width in pixels.
    width: u32,
}

// ========================================================================
// Harness Implementations
// ========================================================================

impl Harness {
    /// The headless GPU, the system fonts, and pipelines for [`FORMAT`], or
    /// why there is no GPU to draw with.
    fn new() -> Result<Self, String> {
        let gpu = GpuContext::headless().map_err(|error| error.to_string())?;
        let mut fonts = Fonts::new();
        let font = Font::default();
        let metrics = fonts.measure_cell(&font, SCALE_FACTOR);
        let fonts = Arc::new(Mutex::new(fonts));
        let renderer = Renderer::new(&gpu, FORMAT);
        Ok(Self {
            font,
            fonts,
            gpu,
            metrics,
            renderer,
        })
    }

    /// An empty frame `width` by `height` pixels.
    fn frame(&self, width: u32, height: u32) -> Frame {
        Frame::new(
            (width, height),
            SCALE_FACTOR,
            self.metrics,
            Arc::clone(&self.fonts),
            self.font.physical_size(SCALE_FACTOR),
        )
    }

    /// A texture `width` by `height` pixels to draw frames into.
    fn target(&self, width: u32, height: u32) -> Texture {
        self.gpu.device.create_texture(&TextureDescriptor {
            dimension: TextureDimension::D2,
            format: FORMAT,
            label: Some(TARGET_LABEL),
            mip_level_count: 1,
            sample_count: 1,
            size: extent(width, height),
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Draws `frame` into `texture`, `width` by `height` pixels.
    fn render_into(&mut self, frame: &Frame, texture: &Texture, width: u32, height: u32) {
        let view = texture.create_view(&TextureViewDescriptor::default());
        let font = ShapingFont {
            family: self.font.family(),
            ligatures: self.font.has_ligatures(),
            metrics: self.metrics,
            size: self.font.physical_size(SCALE_FACTOR),
            weights: self.font.weights(),
        };
        let target = FrameTarget {
            clear: frame.clear_color,
            corner_radius: frame.corner_radius(),
            layers: [&frame.draw, &frame.overlay],
            size: (width, height),
            view: &view,
        };
        self.renderer.render(
            &self.gpu,
            &mut self.fonts.lock().unwrap_or_else(PoisonError::into_inner),
            &font,
            target,
        );
    }

    /// Draws `frame`, `width` by `height` pixels, and reads it back.
    fn draw(&mut self, frame: &Frame, width: u32, height: u32) -> Snapshot {
        let texture = self.target(width, height);
        self.render_into(frame, &texture, width, height);
        let device = &self.gpu.device;
        let size = extent(width, height);
        let row_bytes = width * BYTES_PER_PIXEL;
        let padded_row_bytes = row_bytes.next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(READBACK_LABEL),
            mapped_at_creation: false,
            size: u64::from(padded_row_bytes) * u64::from(height),
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some(READBACK_LABEL),
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            TexelCopyBufferInfo {
                buffer: &buffer,
                layout: TexelCopyBufferLayout {
                    bytes_per_row: Some(padded_row_bytes),
                    offset: 0,
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        self.gpu.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(MapMode::Read, move |result| {
            // The receiver waits below; it cannot be gone.
            let _ = sender.send(result);
        });
        device
            .poll(PollType::wait_indefinitely())
            .expect("the GPU finished the readback");
        receiver
            .recv()
            .expect("the readback callback ran")
            .expect("the readback buffer mapped");
        let mapped = slice
            .get_mapped_range()
            .expect("the mapped readback buffer is readable");
        let pixels = mapped
            .chunks(padded_row_bytes as usize)
            .flat_map(|row| &row[..row_bytes as usize])
            .copied()
            .collect();
        drop(mapped);
        buffer.unmap();
        Snapshot {
            height,
            metrics: self.metrics,
            pixels,
            width,
        }
    }
}

// ========================================================================
// Snapshot Implementations
// ========================================================================

impl Snapshot {
    /// The size of a cell the frame was laid out with.
    pub fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    /// The color of the pixel covering `x`, `y`.
    pub fn pixel(&self, x: f32, y: f32) -> Rgb {
        let (column, row) = (x.floor() as u32, y.floor() as u32);
        assert!(
            column < self.width && row < self.height,
            "{x}, {y} is outside the {}x{} frame",
            self.width,
            self.height
        );
        let start = ((row * self.width + column) * BYTES_PER_PIXEL) as usize;
        let [r, g, b] = [0, 1, 2].map(|channel| self.pixels[start + channel]);
        Rgb::new(r, g, b)
    }

    /// Panics unless the pixel covering `x`, `y` is `expected`, naming
    /// `what` was meant to be there.
    pub fn assert_pixel(&self, x: f32, y: f32, expected: Rgb, what: &str) {
        let actual = self.pixel(x, y);
        assert!(
            is_near(actual, expected),
            "{what} at {x}, {y}: expected {expected:?}, found {actual:?}"
        );
    }

    /// Whether some pixel inside `rect` is `color`: a glyph drawn there in
    /// that color, whatever its shape.
    pub fn contains_color(&self, rect: Rect, color: Rgb) -> bool {
        let rows = rect.y.floor() as u32..(rect.y + rect.height).ceil() as u32;
        rows.flat_map(|row| {
            let columns = rect.x.floor() as u32..(rect.x + rect.width).ceil() as u32;
            columns.map(move |column| (column, row))
        })
        .filter(|&(column, row)| column < self.width && row < self.height)
        .any(|(column, row)| is_near(self.pixel(column as f32, row as f32), color))
    }

    /// Whether every pixel inside `rect` is `color`: nothing drawn there
    /// over a background of that color. A glyph in its own colors (an
    /// emoji) shows as a pixel that is not.
    pub fn is_plain(&self, rect: Rect, color: Rgb) -> bool {
        let rows = rect.y.floor() as u32..(rect.y + rect.height).ceil() as u32;
        rows.flat_map(|row| {
            let columns = rect.x.floor() as u32..(rect.x + rect.width).ceil() as u32;
            columns.map(move |column| (column, row))
        })
        .filter(|&(column, row)| column < self.width && row < self.height)
        .all(|(column, row)| is_near(self.pixel(column as f32, row as f32), color))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A frame `width` by `height` pixels with no GPU behind it, measuring
/// text with the system fonts, which every plain frame on the thread
/// shares as a window's frames do: for tests of what widgets lay out and
/// describe rather than of the pixels they draw.
pub(crate) fn plain_frame(width: u32, height: u32) -> Frame {
    let (fonts, metrics) = PLAIN_FONTS.with(|(fonts, metrics)| (Arc::clone(fonts), *metrics));
    Frame::new(
        (width, height),
        SCALE_FACTOR,
        metrics,
        fonts,
        Font::default().physical_size(SCALE_FACTOR),
    )
}

/// The headless GPU frames are drawn with, as an app borrows it, to make
/// textures a frame draws; `None` when there is no adapter, which
/// [`render`] reports.
pub(crate) fn gpu() -> Option<Gpu> {
    let harness = HARNESS.get_or_init(|| Mutex::new(Harness::new()));
    let harness = harness.lock().unwrap_or_else(PoisonError::into_inner);
    harness.as_ref().ok().map(|harness| harness.gpu.lend())
}

/// A frame `width` by `height` pixels drawn by `draw` and read back, or
/// `None`, after printing why (once per test run), when no GPU adapter can
/// draw it.
pub(crate) fn render(width: u32, height: u32, draw: impl FnOnce(&mut Frame)) -> Option<Snapshot> {
    let harness = HARNESS.get_or_init(|| Mutex::new(Harness::new()));
    // A test that panicked while drawing leaves the harness usable.
    let mut harness = harness.lock().unwrap_or_else(PoisonError::into_inner);
    let harness = match harness.as_mut() {
        Ok(harness) => harness,
        Err(why) => {
            assert!(
                env::var_os(REQUIRE_GPU_VAR).is_none(),
                "no GPU adapter for offscreen rendering: {why}"
            );
            // Written past the test harness's capture, once per run, so a
            // passing run still says its pixels went unchecked.
            static TOLD: Once = Once::new();
            TOLD.call_once(|| {
                let note = format!(
                    "misty: skipping offscreen render tests, no GPU adapter: {why}. \
                     `make test-render` runs them on a virtual display with GL.\n"
                );
                // Nowhere to report a failed write to standard error.
                let _ = io::stderr().write_all(note.as_bytes());
            });
            return None;
        }
    };
    let mut frame = harness.frame(width, height);
    draw(&mut frame);
    Some(harness.draw(&frame, width, height))
}

/// Times `frames` frames `width` by `height` pixels, each built by `draw`
/// (handed the frame's number) and drawn, waiting for the GPU after each;
/// `None`, as [`render`] says, without an adapter. Nothing is read back, so
/// the times are those of drawing a window's frame.
#[cfg(any(test, feature = "testing"))]
pub fn time_frames(
    width: u32,
    height: u32,
    frames: usize,
    mut draw: impl FnMut(&mut Frame, usize),
) -> Option<FrameTimes> {
    let harness = HARNESS.get_or_init(|| Mutex::new(Harness::new()));
    let mut harness = harness.lock().unwrap_or_else(PoisonError::into_inner);
    let Ok(harness) = harness.as_mut() else {
        return None;
    };
    let texture = harness.target(width, height);
    let (mut total, mut max) = (Duration::ZERO, Duration::ZERO);
    for number in 0..frames {
        let started = Instant::now();
        let mut frame = harness.frame(width, height);
        draw(&mut frame, number);
        harness.render_into(&frame, &texture, width, height);
        harness
            .gpu
            .device
            .poll(PollType::wait_indefinitely())
            .expect("the GPU finished the frame");
        let took = started.elapsed();
        total += took;
        max = max.max(took);
    }
    let count = u32::try_from(frames.max(1)).unwrap_or(u32::MAX);
    Some(FrameTimes {
        max,
        mean: total / count,
    })
}

/// The size of a `width` by `height` texture.
fn extent(width: u32, height: u32) -> Extent3d {
    Extent3d {
        depth_or_array_layers: 1,
        height,
        width,
    }
}

/// Whether `actual` matches `expected` within [`CHANNEL_TOLERANCE`].
fn is_near(actual: Rgb, expected: Rgb) -> bool {
    [
        (actual.r, expected.r),
        (actual.g, expected.g),
        (actual.b, expected.b),
    ]
    .iter()
    .all(|&(a, e)| a.abs_diff(e) <= CHANNEL_TOLERANCE)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::box_style::{Border, BoxStyle, Shadow};
    use crate::base::color::Rgba;
    use crate::base::geometry::Point;
    use crate::base::theme::Theme;

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 48;

    #[test]
    fn test_fills_land_on_their_pixels_over_the_clear_color() {
        let theme = Theme::dark();
        let rect = Rect::new(8.0, 8.0, 16.0, 16.0);
        let Some(snapshot) = render(WIDTH, HEIGHT, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.fill_rect(rect, theme.accent);
        }) else {
            return;
        };
        snapshot.assert_pixel(8.0, 8.0, theme.accent, "the fill's first pixel");
        snapshot.assert_pixel(23.0, 23.0, theme.accent, "the fill's last pixel");
        snapshot.assert_pixel(24.0, 23.0, theme.background, "right of the fill");
        snapshot.assert_pixel(7.0, 8.0, theme.background, "left of the fill");
    }

    #[test]
    fn test_the_overlay_paints_over_base_decorations() {
        let theme = Theme::dark();
        let rect = Rect::new(0.0, 0.0, 32.0, 32.0);
        let Some(snapshot) = render(WIDTH, HEIGHT, |frame| {
            frame.fill_rect_over_text(rect, theme.danger);
            frame.overlay(|frame| frame.fill_rect(rect, theme.surface));
        }) else {
            return;
        };
        snapshot.assert_pixel(16.0, 16.0, theme.surface, "the overlay fill");
    }

    /// The box most shape tests draw: 32 pixels square at 8, 8.
    const BOX: Rect = Rect::new(8.0, 8.0, 32.0, 32.0);

    /// A plain style filled with `fill`, to change one part of in a test.
    fn plain_style(fill: Rgb) -> BoxStyle {
        BoxStyle {
            border: None,
            corner_radius: 0.0,
            fill: Rgba::from(fill),
            shadow: None,
        }
    }

    #[test]
    fn test_a_plain_box_draws_the_same_pixels_as_a_fill() {
        let theme = Theme::dark();
        // Fractional edges, where a softened edge would differ.
        let rect = Rect::new(8.4, 8.6, 15.3, 9.5);
        let background = Rgba::from(theme.background);
        let Some(filled) = render(WIDTH, HEIGHT, |frame| {
            frame.clear(background);
            frame.fill_rect(rect, theme.accent);
        }) else {
            return;
        };
        let Some(boxed) = render(WIDTH, HEIGHT, |frame| {
            frame.clear(background);
            frame.draw_box(rect, &plain_style(theme.accent));
        }) else {
            return;
        };
        filled.assert_pixel(10.0, 10.0, theme.accent, "the fill, so the frames drew");
        assert!(
            filled.pixels == boxed.pixels,
            "a plain box differs from a fill"
        );
    }

    #[test]
    fn test_rounded_corners_leave_the_corner_pixels_out_and_soften_the_arc() {
        let theme = Theme::dark();
        let style = BoxStyle {
            corner_radius: 8.0,
            ..plain_style(theme.accent)
        };
        let Some(snapshot) = render(WIDTH, HEIGHT, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.draw_box(BOX, &style);
        }) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(8.0, 8.0, background, "the top left corner");
        snapshot.assert_pixel(39.0, 39.0, background, "the bottom right corner");
        snapshot.assert_pixel(8.0, 24.0, theme.accent, "the left edge's middle");
        snapshot.assert_pixel(24.0, 8.0, theme.accent, "the top edge's middle");
        snapshot.assert_pixel(24.0, 24.0, theme.accent, "the center");
        let arc = snapshot.pixel(10.0, 10.0);
        assert!(
            !is_near(arc, background) && !is_near(arc, theme.accent),
            "a pixel the arc crosses is blended, found {arc:?}"
        );
    }

    #[test]
    fn test_a_border_lines_the_inside_of_the_edge_around_the_fill() {
        let theme = Theme::dark();
        let style = BoxStyle {
            border: Some(Border {
                color: Rgba::from(theme.border),
                width: 2.0,
            }),
            ..plain_style(theme.surface)
        };
        let Some(snapshot) = render(WIDTH, HEIGHT, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.draw_box(BOX, &style);
        }) else {
            return;
        };
        snapshot.assert_pixel(7.0, 8.0, theme.background, "outside the box");
        snapshot.assert_pixel(8.0, 8.0, theme.border, "the border's outer pixel");
        snapshot.assert_pixel(9.0, 9.0, theme.border, "the border's inner pixel");
        snapshot.assert_pixel(10.0, 10.0, theme.surface, "the fill inside it");
        snapshot.assert_pixel(39.0, 39.0, theme.border, "the far corner's border");
        snapshot.assert_pixel(37.0, 37.0, theme.surface, "the fill by the far corner");
    }

    #[test]
    fn test_a_shadow_darkens_below_the_box_and_fades_out() {
        let theme = Theme::dark();
        let rect = Rect::new(16.0, 8.0, 24.0, 16.0);
        let style = BoxStyle {
            shadow: Some(Shadow {
                blur: 4.0,
                color: Rgba::new(0, 0, 0, 128),
                offset: Point::new(0.0, 6.0),
            }),
            ..plain_style(theme.accent)
        };
        let Some(snapshot) = render(WIDTH, HEIGHT, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.draw_box(rect, &style);
        }) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(28.0, 16.0, theme.accent, "the box over its shadow");
        let shadowed = snapshot.pixel(28.0, 26.0);
        assert!(
            shadowed.r < background.r && shadowed.g < background.g && shadowed.b < background.b,
            "below the box is darker than {background:?}, found {shadowed:?}"
        );
        snapshot.assert_pixel(28.0, 40.0, background, "past the blur");
    }

    #[test]
    fn test_a_clip_cuts_a_rounded_box_without_rounding_the_cut() {
        let theme = Theme::dark();
        let style = BoxStyle {
            corner_radius: 8.0,
            ..plain_style(theme.accent)
        };
        let Some(snapshot) = render(WIDTH, HEIGHT, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.clipped(Rect::new(0.0, 0.0, 24.0, 48.0), |frame| {
                frame.draw_box(BOX, &style);
            });
        }) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(8.0, 8.0, background, "the box's own corner");
        snapshot.assert_pixel(23.0, 8.0, theme.accent, "the top edge at the cut");
        snapshot.assert_pixel(23.0, 24.0, theme.accent, "the last pixel inside the clip");
        snapshot.assert_pixel(24.0, 24.0, background, "the first pixel past the clip");
    }
}

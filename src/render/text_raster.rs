//! Text drawn inside a transform, as pictures: glyphon draws glyphs only
//! upright, so a [`TextSegment`] that carries a transform is shaped as any
//! other, its glyphs rasterized at the size the transform scales them to
//! and gathered into one image, and the image drawn through the transform
//! as a picture, in the segment's place in paint order. The images are
//! kept across frames in a [`ShapeCache`], so unchanged text is shaped and
//! rasterized once.

use glyphon::Metrics;
use glyphon::cosmic_text::{Color, SwashContent, SwashImage};

use super::super::base::geometry::Rect;
use super::super::base::transform::Transform;
use super::draw::{PictureDraw, PictureSource, SegmentAlign, TextRun, TextSegment, Wrapping};
use super::font::FaceWeights;
use super::image::{BYTES_PER_PIXEL, Image, ImageDraw};
use super::paragraph::TextAlign;
use super::shape_cache::ShapeCache;
use super::text::{Fonts, ShapingFont, place_segment, shape_text};

// ========================================================================
// Constants
// ========================================================================

/// The smallest scale text is rasterized at, for a transform shrinking it
/// to almost nothing.
const MIN_RASTER_SCALE: f32 = 0.25;

/// The largest scale text is rasterized at, keeping a zoomed-in label's
/// image within reason.
const MAX_RASTER_SCALE: f32 = 8.0;

/// The steps a raster's scale is rounded to, so a transform that only
/// turns, whose scale float error moves by a hair from frame to frame,
/// finds the image it made before.
const SCALE_STEPS: f32 = 256.0;

/// The longest side of a text image, in pixels; longer text is not drawn.
const MAX_SIDE: u32 = 8192;

/// The color of a glyph whose run set none: never, since every run does.
const NO_COLOR: Color = Color::rgb(0, 0, 0);

/// The largest value of a color channel.
const CHANNEL_MAX: f32 = 255.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The images of the transformed text of recent frames.
pub(crate) struct TextRasters {
    /// Each segment's image, by what its look depends on; `None` for one
    /// with nothing to draw.
    cache: ShapeCache<RasterKey, Option<Raster>>,
}

/// A segment's glyphs gathered into one image.
struct Raster {
    /// The glyphs, in their colors, on clear pixels.
    image: Image,
    /// Where the image lies, in the segment's own coordinates, from its
    /// origin.
    rect: Rect,
}

/// One glyph's image and where it goes in a raster.
struct PlacedGlyph {
    /// The run's color for a glyph drawn as coverage.
    color: Color,
    /// The rasterized glyph.
    image: SwashImage,
    /// The left edge, in raster pixels from the line's start.
    x: i32,
    /// The top edge, in raster pixels from the segment's top.
    y: i32,
}

/// What a segment's image depends on, as cached.
struct RasterKey {
    /// The baseline's depth in a cell, by its bits.
    baseline: u32,
    /// The width of the box a centered segment is centered in, by its
    /// bits, or `None` for a segment set from its origin.
    centered: Option<u32>,
    /// The font family, or `None` for the system monospace font.
    family: Option<String>,
    /// The line height's bits.
    height: u32,
    /// Whether the grid font's ligatures are drawn.
    ligatures: bool,
    /// The text, colors, and faces.
    runs: Vec<TextRun>,
    /// The scale the glyphs are rasterized at, by its bits.
    scale: u32,
    /// The font size's bits.
    size: u32,
    /// The weights of the grid font's regular and bold text.
    weights: FaceWeights,
    /// How a paragraph wraps: the width's bits, the alignment, and the
    /// line height's bits.
    wrap: Option<(u32, TextAlign, Option<u32>)>,
}

/// What a segment's image depends on, borrowed from the frame to look up a
/// [`RasterKey`] without copying.
#[derive(Hash)]
struct RasterQuery<'a> {
    /// The baseline's depth in a cell, by its bits.
    baseline: u32,
    /// The width of a centered segment's box, by its bits.
    centered: Option<u32>,
    /// The font family.
    family: Option<&'a str>,
    /// The line height's bits.
    height: u32,
    /// Whether the grid font's ligatures are drawn.
    ligatures: bool,
    /// The text, colors, and faces.
    runs: &'a [TextRun],
    /// The raster scale's bits.
    scale: u32,
    /// The font size's bits.
    size: u32,
    /// The weights of the grid font's regular and bold text.
    weights: FaceWeights,
    /// How a paragraph wraps: the width's bits, the alignment, and the
    /// line height's bits.
    wrap: Option<(u32, TextAlign, Option<u32>)>,
}

// ========================================================================
// TextRasters Implementations
// ========================================================================

impl TextRasters {
    /// No images yet.
    pub(crate) fn new() -> Self {
        Self {
            cache: ShapeCache::new(),
        }
    }

    /// The picture that draws `segment`, a transformed one, in `font`
    /// through its transform, cropped to its own clip; `None` for a segment
    /// not transformed, with nothing to draw, or clipped away.
    pub(crate) fn picture_of(
        &mut self,
        fonts: &mut Fonts,
        font: &ShapingFont<'_>,
        segment: &TextSegment,
    ) -> Option<PictureDraw> {
        let transform = segment.transform?;
        let scale = raster_scale(transform.matrix);
        let centered = match segment.align {
            SegmentAlign::Center(width) => Some(width.to_bits()),
            SegmentAlign::Start => None,
        };
        let query = RasterQuery {
            baseline: font.metrics.baseline.to_bits(),
            centered,
            family: font.family,
            height: font.metrics.height.to_bits(),
            ligatures: font.ligatures,
            runs: &segment.runs,
            scale: scale.to_bits(),
            size: font.size.to_bits(),
            weights: font.weights,
            wrap: segment.wrap.map(Wrapping::bits),
        };
        let slot = self
            .cache
            .find_or_make(&query, RasterKey::matches, |query| {
                (RasterKey::of(query), rasterize(fonts, font, segment, scale))
            });
        let raster = self.cache.value(slot)?.as_ref()?;
        let origin = segment.origin;
        let rect = Rect::new(
            origin.x + raster.rect.x,
            origin.y + raster.rect.y,
            raster.rect.width,
            raster.rect.height,
        );
        let crop = match segment.clip {
            Some(clip) => Some(rect.intersection(&clip)?),
            None => None,
        };
        Some(PictureDraw {
            crop,
            opacity: 1.0,
            rect,
            source: PictureSource::Image(raster.image.clone()),
            style: ImageDraw::default(),
            tint: None,
            transform: Some(transform),
        })
    }

    /// Ends the frame: drops the images no recent frame drew.
    pub(crate) fn finish_frame(&mut self) {
        self.cache.finish_frame();
    }
}

// ========================================================================
// RasterKey Implementations
// ========================================================================

impl RasterKey {
    /// The key `query` looks up, owned.
    fn of(query: &RasterQuery<'_>) -> Self {
        Self {
            baseline: query.baseline,
            centered: query.centered,
            family: query.family.map(str::to_owned),
            height: query.height,
            ligatures: query.ligatures,
            runs: query.runs.to_vec(),
            scale: query.scale,
            size: query.size,
            weights: query.weights,
            wrap: query.wrap,
        }
    }

    /// Whether `query` looks this key up.
    fn matches(&self, query: &RasterQuery<'_>) -> bool {
        self.baseline == query.baseline
            && self.centered == query.centered
            && self.family.as_deref() == query.family
            && self.height == query.height
            && self.ligatures == query.ligatures
            && self.scale == query.scale
            && self.size == query.size
            && self.weights == query.weights
            && self.wrap == query.wrap
            && self.runs == query.runs
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The scale text drawn through `matrix` is rasterized at: how much it
/// scales lengths, within bounds and rounded to steps.
fn raster_scale(matrix: Transform) -> f32 {
    let scale = matrix
        .length_scale()
        .clamp(MIN_RASTER_SCALE, MAX_RASTER_SCALE);
    (scale * SCALE_STEPS).round() / SCALE_STEPS
}

/// `segment` shaped in `font` and its glyphs rasterized `scale` times
/// their size into one image, placed as the upright text renderer places
/// the segment; `None` when it has no glyph to draw or its image would be
/// too large.
fn rasterize(
    fonts: &mut Fonts,
    font: &ShapingFont<'_>,
    segment: &TextSegment,
    scale: f32,
) -> Option<Raster> {
    let (font_system, swash_cache) = fonts.systems();
    let metrics = Metrics::new(font.size, font.metrics.height);
    let buffer = shape_text(
        font_system,
        metrics,
        font.family,
        &segment.runs,
        segment.wrap,
        font.ligatures,
        font.weights,
    );
    let line_width = buffer.layout_runs().next().map_or(0.0, |run| run.line_w);
    let placed = place_segment(segment, line_width, font.metrics.baseline);
    let glyph_scale = placed.scale * scale;
    let mut glyphs = Vec::new();
    for run in buffer.layout_runs() {
        let line_y = (run.line_y * glyph_scale).round() as i32;
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, 0.0), glyph_scale);
            let Some(image) = swash_cache.get_image(font_system, physical.cache_key) else {
                continue;
            };
            if image.placement.width == 0 || image.placement.height == 0 {
                continue;
            }
            glyphs.push(PlacedGlyph {
                color: glyph.color_opt.unwrap_or(NO_COLOR),
                image: image.clone(),
                x: physical.x + image.placement.left,
                y: line_y + physical.y - image.placement.top,
            });
        }
    }
    let left = glyphs.iter().map(|glyph| glyph.x).min()?;
    let top = glyphs.iter().map(|glyph| glyph.y).min()?;
    let right = glyphs
        .iter()
        .map(|glyph| glyph.x + glyph.image.placement.width as i32)
        .max()?;
    let bottom = glyphs
        .iter()
        .map(|glyph| glyph.y + glyph.image.placement.height as i32)
        .max()?;
    let width = u32::try_from(right - left).ok()?;
    let height = u32::try_from(bottom - top).ok()?;
    if width > MAX_SIDE || height > MAX_SIDE {
        log::warn!("transformed text {width}x{height} pixels large is not drawn");
        return None;
    }
    let mut pixels = vec![0; width as usize * height as usize * BYTES_PER_PIXEL];
    for glyph in &glyphs {
        paint_glyph(&mut pixels, width, glyph, (left, top));
    }
    let image = Image::from_rgba(width, height, pixels).ok()?;
    let rect = Rect::new(
        placed.x - segment.origin.x + left as f32 / scale,
        placed.y - segment.origin.y + top as f32 / scale,
        width as f32 / scale,
        height as f32 / scale,
    );
    Some(Raster { image, rect })
}

/// Paints `glyph` over `pixels`, an image `width` pixels wide whose top
/// left corner is at `origin` in raster pixels: straight alpha, each pixel
/// blended over what earlier glyphs left. A coverage glyph paints its
/// run's color; a color glyph (an emoji) its own colors, faded by the
/// run's alpha.
fn paint_glyph(pixels: &mut [u8], width: u32, glyph: &PlacedGlyph, (left, top): (i32, i32)) {
    let placement = glyph.image.placement;
    let channels = match glyph.image.content {
        SwashContent::Color => BYTES_PER_PIXEL,
        SwashContent::Mask => 1,
        SwashContent::SubpixelMask => return,
    };
    let fade = f32::from(glyph.color.a()) / CHANNEL_MAX;
    let run_rgb = [glyph.color.r(), glyph.color.g(), glyph.color.b()];
    for row in 0..placement.height as usize {
        for column in 0..placement.width as usize {
            let source = (row * placement.width as usize + column) * channels;
            let Some(texel) = glyph.image.data.get(source..source + channels) else {
                return;
            };
            let (rgb, alpha) = match glyph.image.content {
                SwashContent::Color => ([texel[0], texel[1], texel[2]], texel[3]),
                SwashContent::Mask | SwashContent::SubpixelMask => (run_rgb, texel[0]),
            };
            let x = (glyph.x - left) as usize + column;
            let y = (glyph.y - top) as usize + row;
            let target = (y * width as usize + x) * BYTES_PER_PIXEL;
            if let Some(pixel) = pixels.get_mut(target..target + BYTES_PER_PIXEL) {
                blend_over(pixel, rgb, f32::from(alpha) / CHANNEL_MAX * fade);
            }
        }
    }
}

/// Blends `rgb` at `alpha` (0 to 1) over `pixel`, straight alpha. A
/// pixel still clear takes `rgb` even where nothing covers it, so sampling
/// between it and a covered neighbor never darkens the glyph's edge.
fn blend_over(pixel: &mut [u8], rgb: [u8; 3], alpha: f32) {
    let below = f32::from(pixel[3]) / CHANNEL_MAX;
    let out = alpha + below * (1.0 - alpha);
    if out <= 0.0 {
        pixel[..3].copy_from_slice(&rgb);
        return;
    }
    for (channel, source) in pixel[..3].iter_mut().zip(rgb) {
        let mixed = (f32::from(source) * alpha + f32::from(*channel) * below * (1.0 - alpha)) / out;
        *channel = mixed.round() as u8;
    }
    pixel[3] = (out * CHANNEL_MAX).round() as u8;
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blending_over_a_clear_pixel_keeps_the_glyph_color_at_its_edge() {
        let mut pixel = [0, 0, 0, 0];
        blend_over(&mut pixel, [200, 100, 50], 0.0);
        assert_eq!(
            pixel,
            [200, 100, 50, 0],
            "uncovered, yet in the glyph's color"
        );
        blend_over(&mut pixel, [200, 100, 50], 0.5);
        assert_eq!(pixel, [200, 100, 50, 128]);
        blend_over(&mut pixel, [0, 0, 0], 1.0);
        assert_eq!(
            pixel,
            [0, 0, 0, 255],
            "an opaque glyph covers what was there"
        );
    }

    #[test]
    fn test_a_turn_rasterizes_at_the_same_scale_whatever_its_float_error() {
        let a = raster_scale(Transform::rotate(0.3));
        let b = raster_scale(Transform::rotate(1.234));
        assert_eq!(a.to_bits(), b.to_bits());
        assert_eq!(raster_scale(Transform::scale(2.0, 2.0)), 2.0);
    }
}

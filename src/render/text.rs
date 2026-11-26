//! Text on the GPU through glyphon: [`Fonts`], the installed fonts and
//! glyph rasterizer every window shares, and [`TextPipeline`], a window's
//! glyph atlas and the shaped [`TextSegment`]s it draws, kept across
//! frames in a [`ShapeCache`] so unchanged text is not shaped again. Each
//! batch of consecutive text segments is prepared in a glyph renderer of
//! its own, so text keeps its place in paint order among the other draws.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use glyphon::cosmic_text::{Align, FeatureTag, FontFeatures, LayoutGlyph, Wrap};
use glyphon::{
    Attrs, Buffer, Cache, Color, ColorMode, Family, FontSystem, Metrics, PrepareError, RenderError,
    Resolution, Shaping, Style, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport, Weight,
};
use wgpu::{MultisampleState, RenderPass, TextureFormat};

use super::super::base::geometry::Rect;
use super::super::base::styled::FontFamily;
use super::app_fonts;
use super::draw::{Face, InlineMetrics, SegmentAlign, TextRun, TextSegment, Wrapping};
use super::font::{CellMetrics, FaceWeights, Font};
use super::gpu::GpuContext;
use super::inline_font;
use super::paragraph::{ParagraphKey, ParagraphQuery, ParagraphShape, TextAlign};
use super::shape_cache::{ShapeCache, Slot};

// ========================================================================
// Constants
// ========================================================================

/// The character a cell is measured by: every glyph of a monospace font has
/// its advance.
const MEASURE_CHAR: &str = "0";

/// A cell's width as a share of the font size, for when the font has no
/// glyphs at all to measure (no fonts installed).
const FALLBACK_WIDTH_RATIO: f32 = 0.6;

/// A baseline's depth as a share of the line height, for the same case.
const FALLBACK_BASELINE_RATIO: f32 = 0.8;

/// The smallest font size an inline box's glyph is set at, in pixels, for
/// a box that reaches nothing above the baseline: a power of two, so the
/// letter spacing that widens it divides exactly.
const MIN_BOX_SIZE: f32 = 1.0 / 64.0;

/// The scale text areas are drawn at: segments are laid out in physical
/// pixels already.
const UNSCALED: f32 = 1.0;

/// The OpenType features that join characters into ligatures, turned off
/// for a font without them.
const LIGATURE_FEATURES: [FeatureTag; 4] = [
    FeatureTag::CONTEXTUAL_ALTERNATES,
    FeatureTag::CONTEXTUAL_LIGATURES,
    FeatureTag::DISCRETIONARY_LIGATURES,
    FeatureTag::STANDARD_LIGATURES,
];

// ========================================================================
// Data Structures
// ========================================================================

/// The installed fonts and the cache of rasterized glyph images. Loading
/// the fonts scans the system, so it happens once for all windows.
pub(crate) struct Fonts {
    /// How many of the app's fonts ([`app_fonts`]) are loaded.
    app_fonts: usize,
    /// The font database and shaper.
    font_system: FontSystem,
    /// Paragraphs shaped in recent frames, by their text, faces, width,
    /// and font.
    paragraphs: ShapeCache<ParagraphKey, ParagraphShape>,
    /// Rasterized glyphs, by font, glyph, and size.
    swash_cache: SwashCache,
    /// The cells fonts other than a window's were measured at, by family
    /// and the bits of the physical size and the line height.
    cells: HashMap<(Option<String>, u32, u32), CellMetrics>,
}

/// What one window needs to draw text: its glyph atlas, the shaped text
/// of recent frames for each layer, and a renderer for each batch of text
/// a frame draws.
pub(crate) struct TextPipeline {
    /// Where glyph images live on the GPU, shared by every renderer.
    atlas: TextAtlas,
    /// The layers, bottom to top.
    layers: Vec<TextLayer>,
    /// A renderer per batch of the frame's text, in paint order, layer
    /// after layer; kept across frames and added to when a frame draws
    /// more batches than any before.
    renderers: Vec<TextRenderer>,
    /// The target size, for converting pixels to clip space.
    viewport: Viewport,
}

/// The shaped text of one layer of recent frames.
struct TextLayer {
    /// How many of the app's fonts were loaded when `cache` was shaped.
    app_fonts: usize,
    /// The text shaped in recent frames, by its runs and font.
    cache: ShapeCache<SegmentKey, Shaped>,
}

/// One layer's text as the renderer draws it.
pub(crate) struct TextBatches<'a> {
    /// The ranges of `segments` drawn as one batch each, in paint order.
    pub(crate) batches: Vec<Range<usize>>,
    /// The layer's text segments, in the order drawn.
    pub(crate) segments: &'a [TextSegment],
}

/// What a segment's shaping depends on, as cached: its runs and the font.
struct SegmentKey {
    /// The font family, or `None` for the system monospace font.
    family: Option<String>,
    /// The line height's bits.
    height: u32,
    /// Whether the grid font's ligatures are drawn.
    ligatures: bool,
    /// The text, colors, and faces.
    runs: Vec<TextRun>,
    /// The font size's bits.
    size: u32,
    /// The weights of the grid font's regular and bold text.
    weights: FaceWeights,
    /// How a paragraph wraps: the width's bits, the alignment, and the
    /// line height's bits.
    wrap: Option<(u32, TextAlign, Option<u32>)>,
}

/// What a segment's shaping depends on, borrowed from the frame to look up
/// a [`SegmentKey`] without copying.
#[derive(Hash)]
struct SegmentQuery<'a> {
    /// The font family.
    family: Option<&'a str>,
    /// The line height's bits.
    height: u32,
    /// Whether the grid font's ligatures are drawn.
    ligatures: bool,
    /// The text, colors, and faces.
    runs: &'a [TextRun],
    /// The font size's bits.
    size: u32,
    /// The weights of the grid font's regular and bold text.
    weights: FaceWeights,
    /// How a paragraph wraps: the width's bits, the alignment, and the
    /// line height's bits.
    wrap: Option<(u32, TextAlign, Option<u32>)>,
}

/// A segment's text shaped once.
struct Shaped {
    /// The shaped glyphs.
    buffer: Buffer,
    /// The width of its line, for centering it.
    line_width: f32,
}

/// Where a segment's shaped line is drawn: its top left corner and the
/// scale its glyphs are drawn at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    /// The scale, 1 but for a line shrunk to fit its box.
    pub(super) scale: f32,
    /// The left edge.
    pub(super) x: f32,
    /// The top edge.
    pub(super) y: f32,
}

/// Where one layer's batches of text are prepared: the shared atlas and
/// viewport, and a renderer for each batch.
struct PrepareTarget<'a> {
    /// Where glyph images are uploaded.
    atlas: &'a mut TextAtlas,
    /// The layer's renderers, one per batch, in paint order.
    renderers: &'a mut [TextRenderer],
    /// The target size.
    viewport: &'a Viewport,
}

/// The font a frame's text is shaped in.
pub(crate) struct ShapingFont<'a> {
    /// The family, or `None` for the system monospace font.
    pub(crate) family: Option<&'a str>,
    /// Whether the cell grid's text draws the family's ligatures.
    pub(crate) ligatures: bool,
    /// The cell the font was measured to, whose height is the line height.
    pub(crate) metrics: CellMetrics,
    /// The size in physical pixels.
    pub(crate) size: f32,
    /// The weights of its regular and bold text.
    pub(crate) weights: FaceWeights,
}

// ========================================================================
// Fonts Implementations
// ========================================================================

impl Fonts {
    /// Loads the system's fonts.
    pub(crate) fn new() -> Self {
        let (font_system, app_fonts) = app_fonts::font_system();
        Self {
            app_fonts,
            font_system,
            paragraphs: ShapeCache::new(),
            swash_cache: SwashCache::new(),
            cells: HashMap::new(),
        }
    }

    /// Loads the fonts the app loaded since the last call, dropping the
    /// shapes made without them; returns how many app fonts are loaded.
    pub(crate) fn sync_app_fonts(&mut self) -> usize {
        let loaded = app_fonts::sync(&mut self.font_system, self.app_fonts);
        if loaded != self.app_fonts {
            self.app_fonts = loaded;
            self.paragraphs = ShapeCache::new();
        }
        loaded
    }

    /// The shape of the paragraph `query` asks for: shaped now unless a
    /// recent frame shaped the same.
    pub(crate) fn paragraph_shape(&mut self, query: &ParagraphQuery<'_>) -> &ParagraphShape {
        self.sync_app_fonts();
        let slot = self
            .paragraphs
            .find_or_make(query, ParagraphKey::matches, |query| {
                (ParagraphKey::of(query), query.shape(&mut self.font_system))
            });
        self.paragraphs
            .value(slot)
            .expect("a slot just found or made holds its value")
    }

    /// Ends a frame of any window: drops the paragraph shapes no recent
    /// frame used.
    pub(crate) fn finish_frame(&mut self) {
        self.paragraphs.finish_frame();
    }

    /// How many paragraph shapes are kept.
    #[cfg(test)]
    pub(crate) fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    /// The font database and the glyph rasterizer's cache, with the app's
    /// fonts loaded, to shape and rasterize text off the GPU.
    pub(super) fn systems(&mut self) -> (&mut FontSystem, &mut SwashCache) {
        self.sync_app_fonts();
        (&mut self.font_system, &mut self.swash_cache)
    }

    /// [`Fonts::measure_cell`], measured once for each family and size.
    pub(crate) fn cell_of(&mut self, font: &Font, scale_factor: f64) -> CellMetrics {
        let key = (
            font.family().map(str::to_owned),
            font.physical_size(scale_factor).to_bits(),
            font.line_height_px(font.physical_size(scale_factor))
                .to_bits(),
        );
        if let Some(metrics) = self.cells.get(&key) {
            return *metrics;
        }
        let metrics = self.measure_cell(font, scale_factor);
        self.cells.insert(key, metrics);
        metrics
    }

    /// The cell `font` draws in on a display scaled by `scale_factor`: as
    /// wide as the font's advance, as high as its line.
    pub(crate) fn measure_cell(&mut self, font: &Font, scale_factor: f64) -> CellMetrics {
        self.sync_app_fonts();
        measure_cell_in(&mut self.font_system, font, scale_factor)
    }
}

impl fmt::Debug for Fonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fonts").finish_non_exhaustive()
    }
}

// ========================================================================
// TextPipeline Implementations
// ========================================================================

impl TextPipeline {
    /// A pipeline drawing `layers` layers of text to textures of `format`.
    pub(crate) fn new(gpu: &GpuContext, format: TextureFormat, layers: usize) -> Self {
        let cache = Cache::new(&gpu.device);
        // Glyph colors are sRGB; an sRGB target wants them decoded first.
        let color_mode = match format.is_srgb() {
            true => ColorMode::Accurate,
            false => ColorMode::Web,
        };
        let atlas = TextAtlas::with_color_mode(&gpu.device, &gpu.queue, &cache, format, color_mode);
        let layers = (0..layers)
            .map(|_| TextLayer {
                app_fonts: 0,
                cache: ShapeCache::new(),
            })
            .collect();
        Self {
            atlas,
            layers,
            renderers: Vec::new(),
            viewport: Viewport::new(&gpu.device, &cache),
        }
    }

    /// Shapes each layer's segments of `layers` in `font` and uploads the
    /// glyphs of each of its batches to a renderer of its own, for a target
    /// `width` by `height` pixels. Layers past the pipeline's own are left
    /// out.
    pub(crate) fn prepare(
        &mut self,
        gpu: &GpuContext,
        fonts: &mut Fonts,
        font: &ShapingFont<'_>,
        (width, height): (u32, u32),
        layers: &[TextBatches<'_>],
    ) -> Result<(), PrepareError> {
        self.viewport
            .update(&gpu.queue, Resolution { height, width });
        let app_fonts = fonts.sync_app_fonts();
        for layer in &mut self.layers {
            if layer.app_fonts != app_fonts {
                layer.app_fonts = app_fonts;
                layer.cache = ShapeCache::new();
            }
        }
        let batches: usize = layers.iter().map(|layer| layer.batches.len()).sum();
        while self.renderers.len() < batches {
            self.renderers.push(TextRenderer::new(
                &mut self.atlas,
                &gpu.device,
                MultisampleState::default(),
                None,
            ));
        }
        let mut renderers = self.renderers.as_mut_slice();
        for (layer, text) in self.layers.iter_mut().zip(layers) {
            let (own, rest) = renderers.split_at_mut(text.batches.len());
            renderers = rest;
            let target = PrepareTarget {
                atlas: &mut self.atlas,
                renderers: own,
                viewport: &self.viewport,
            };
            layer.prepare(gpu, fonts, font, target, text)?;
        }
        Ok(())
    }

    /// Draws the prepared glyphs of text batch `batch` of the frame,
    /// counted in paint order across its layers, into `pass`.
    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, batch: usize) -> Result<(), RenderError> {
        match self.renderers.get(batch) {
            Some(renderer) => renderer.render(&self.atlas, &self.viewport, pass),
            None => Ok(()),
        }
    }

    /// Frees atlas space held by glyphs the last frame did not use.
    pub(crate) fn trim(&mut self) {
        self.atlas.trim();
    }
}

// ========================================================================
// TextLayer Implementations
// ========================================================================

impl TextLayer {
    /// Shapes the segments of `text` in `font` and uploads the glyphs of
    /// each of its batches to the atlas, through the batch's renderer of
    /// `target`.
    fn prepare(
        &mut self,
        gpu: &GpuContext,
        fonts: &mut Fonts,
        font: &ShapingFont<'_>,
        target: PrepareTarget<'_>,
        text: &TextBatches<'_>,
    ) -> Result<(), PrepareError> {
        let segments = text.segments;
        let metrics = Metrics::new(font.size, font.metrics.height);
        let slots: Vec<Slot> = segments
            .iter()
            .map(|segment| {
                let query = SegmentQuery {
                    family: font.family,
                    height: metrics.line_height.to_bits(),
                    ligatures: font.ligatures,
                    runs: &segment.runs,
                    size: metrics.font_size.to_bits(),
                    weights: font.weights,
                    wrap: segment.wrap.map(Wrapping::bits),
                };
                self.cache
                    .find_or_make(&query, SegmentKey::matches, |query| {
                        let buffer = shape_text(
                            &mut fonts.font_system,
                            metrics,
                            font.family,
                            query.runs,
                            segment.wrap,
                            query.ligatures,
                            query.weights,
                        );
                        let line_width = buffer.layout_runs().next().map_or(0.0, |run| run.line_w);
                        let shaped = Shaped { buffer, line_width };
                        (SegmentKey::of(query), shaped)
                    })
            })
            .collect();
        let cache = &self.cache;
        let prepared = text
            .batches
            .iter()
            .zip(target.renderers.iter_mut())
            .try_for_each(|(batch, renderer)| {
                let start = batch.start.min(segments.len());
                let end = batch.end.clamp(start, segments.len());
                let areas = segments[start..end]
                    .iter()
                    .zip(&slots[start..end])
                    .filter_map(|(segment, slot)| Some((segment, cache.value(*slot)?)))
                    .map(|(segment, shaped)| {
                        let placed =
                            place_segment(segment, shaped.line_width, font.metrics.baseline);
                        TextArea {
                            bounds: segment.clip.map_or_else(TextBounds::default, bounds_of),
                            buffer: &shaped.buffer,
                            custom_glyphs: &[],
                            default_color: Color::rgb(0, 0, 0),
                            left: placed.x,
                            scale: placed.scale,
                            top: placed.y,
                        }
                    });
                renderer.prepare(
                    &gpu.device,
                    &gpu.queue,
                    &mut fonts.font_system,
                    target.atlas,
                    target.viewport,
                    areas,
                    &mut fonts.swash_cache,
                )
            });
        self.cache.finish_frame();
        prepared
    }
}

// ========================================================================
// SegmentKey Implementations
// ========================================================================

impl SegmentKey {
    /// The key `query` looks up, owned.
    fn of(query: &SegmentQuery<'_>) -> Self {
        Self {
            family: query.family.map(str::to_owned),
            height: query.height,
            ligatures: query.ligatures,
            runs: query.runs.to_vec(),
            size: query.size,
            weights: query.weights,
            wrap: query.wrap,
        }
    }

    /// Whether `query` looks this key up.
    fn matches(&self, query: &SegmentQuery<'_>) -> bool {
        self.family.as_deref() == query.family
            && self.height == query.height
            && self.ligatures == query.ligatures
            && self.size == query.size
            && self.weights == query.weights
            && self.wrap == query.wrap
            && self.runs == query.runs
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The cell `font` draws in on a display scaled by `scale_factor`, shaped
/// with `font_system`: as wide as the font's advance, as high as its line.
pub(crate) fn measure_cell_in(
    font_system: &mut FontSystem,
    font: &Font,
    scale_factor: f64,
) -> CellMetrics {
    let size = font.physical_size(scale_factor);
    let height = font.line_height_px(size);
    let mut buffer = Buffer::new(font_system, Metrics::new(size, height));
    let attrs = Attrs::new().family(family_of(font.family()));
    buffer.set_text(MEASURE_CHAR, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let measured = buffer.layout_runs().next().and_then(|run| {
        let glyph = run.glyphs.first()?;
        Some((glyph.w, run.line_y))
    });
    let (width, baseline) = measured.unwrap_or((
        size * FALLBACK_WIDTH_RATIO,
        height * FALLBACK_BASELINE_RATIO,
    ));
    CellMetrics {
        baseline,
        height,
        width,
    }
}

/// `runs` shaped at `metrics`: as one line in `family` (the system
/// monospace family when `None`) without `wrap`, or with it as a
/// paragraph in the system's proportional family, wrapped and aligned.
/// Without `ligatures`, a line on the grid's font draws every character on
/// its own glyph; a paragraph keeps its ligatures. A run's own style ([`RunStyle`](super::draw::RunStyle)) overrides its
/// family, size, and alpha. Each glyph's metadata is the index of the run
/// it came from.
///
/// A run standing for an inline box is one invisible glyph of misty's
/// inline box font ([`inline_font`]): its font size is the box's ascent,
/// so the glyph lifts its line that far above the baseline, and its letter
/// spacing makes it as wide as the box. cosmic-text centers a line's
/// glyphs in its height, so a box reaching below the baseline gets its
/// room from a taller line: once shaped, each line holding a box is given
/// the height that fits it ([`settled_box_lines`]) and the text shaped
/// again. Only line heights change, so the lines break where they did.
pub(crate) fn shape_text(
    font_system: &mut FontSystem,
    metrics: Metrics,
    family: Option<&str>,
    runs: &[TextRun],
    wrap: Option<Wrapping>,
    ligatures: bool,
    weights: FaceWeights,
) -> Buffer {
    // A wrapped paragraph is in the proportional family, in the weights
    // it was given, so a light grid font gives light paragraphs too. Its
    // icon glyphs stay in the grid font, where an app keeps them.
    // A paragraph's own line height is a multiple of its font size; each
    // run's line then scales from it as it does from the font's own.
    let metrics = match wrap.and_then(|wrap| wrap.line_height) {
        Some(multiple) => Metrics::new(
            metrics.font_size,
            metrics.font_size * f32::from_bits(multiple),
        ),
        None => metrics,
    };
    let grid_family = family_of(family);
    let family = match wrap {
        Some(_) => Family::SansSerif,
        None => grid_family,
    };
    let mut buffer = Buffer::new(font_system, metrics);
    // No alignment sets each line against its own start edge: the left in
    // left-to-right text, the right in right-to-left text.
    let align = wrap.and_then(|wrap| {
        buffer.set_size(Some(wrap.width.max(0.0)), None);
        buffer.set_wrap(Wrap::WordOrGlyph);
        match wrap.align {
            TextAlign::Center => Some(Align::Center),
            TextAlign::End => Some(Align::End),
            TextAlign::Justify => Some(Align::Justified),
            TextAlign::Start => None,
        }
    });
    let features = (!ligatures && wrap.is_none()).then(|| {
        let mut features = FontFeatures::new();
        for tag in LIGATURE_FEATURES {
            features.disable(tag);
        }
        features
    });
    let families: Vec<Family<'_>> = runs
        .iter()
        .map(|run| match &run.style.family {
            Some(wanted) => span_family(font_system, wanted),
            None => family,
        })
        .collect();
    let spans = |box_lines: &[Option<f32>]| -> Vec<(&str, Attrs<'_>)> {
        let mut spans = Vec::new();
        for (index, (run, family)) in runs.iter().zip(&families).enumerate() {
            if let Some(inline) = run.style.inline {
                let line_height = box_lines.get(index).copied().flatten();
                spans.push((
                    run.text.as_str(),
                    box_attrs(inline, metrics, line_height).metadata(index),
                ));
                continue;
            }
            let attrs = |family| {
                text_attrs(run, family, metrics, weights, features.as_ref()).metadata(index)
            };
            // Only a run left in the paragraph's own family hands its icons
            // to the grid font; one that names its family keeps them.
            let splits_icons = wrap.is_some() && run.style.family.is_none();
            match splits_icons {
                true => {
                    for (piece, is_icon) in icon_pieces(&run.text) {
                        let family = match is_icon {
                            true => grid_family,
                            false => *family,
                        };
                        spans.push((piece, attrs(family)));
                    }
                }
                false => spans.push((run.text.as_str(), attrs(*family))),
            }
        }
        spans
    };
    let default_attrs = Attrs::new().family(family);
    buffer.set_rich_text(spans(&[]), &default_attrs, Shaping::Advanced, align);
    buffer.shape_until_scroll(font_system, false);
    if let Some(box_lines) = settled_box_lines(&buffer, runs, metrics) {
        buffer.set_rich_text(spans(&box_lines), &default_attrs, Shaping::Advanced, align);
        buffer.shape_until_scroll(font_system, false);
    }
    buffer
}

/// The attributes of the text `run` in `family`, at `metrics` unless it
/// sets its own size, in `weights`, with `features` when given.
fn text_attrs<'a>(
    run: &TextRun,
    family: Family<'a>,
    metrics: Metrics,
    weights: FaceWeights,
    features: Option<&FontFeatures>,
) -> Attrs<'a> {
    let alpha = run.style.alpha.unwrap_or(u8::MAX);
    let attrs = face_attrs(Attrs::new().family(family), run.face, weights).color(Color::rgba(
        run.color.r,
        run.color.g,
        run.color.b,
        alpha,
    ));
    // Every run carries its metrics: a line is as tall as the tallest
    // run that does, so one small run alone would shrink it.
    let size = run.style.size.map_or(metrics.font_size, f32::from_bits);
    let line_height = size * metrics.line_height / metrics.font_size;
    let attrs = attrs.metrics(Metrics::new(size, line_height.max(metrics.line_height)));
    match features {
        Some(features) => attrs.font_features(features.clone()),
        None => attrs,
    }
}

/// The attributes of the glyph standing for the inline box `inline`: as
/// high above the baseline as the box, as wide, invisible, on a line
/// `line_height` tall, or at first as tall as the box and at least a line
/// of `metrics`.
fn box_attrs(inline: InlineMetrics, metrics: Metrics, line_height: Option<f32>) -> Attrs<'static> {
    let size = f32::from_bits(inline.ascent).max(MIN_BOX_SIZE);
    let line_height = line_height.unwrap_or_else(|| box_height(inline).max(metrics.line_height));
    Attrs::new()
        .family(Family::Name(inline_font::FAMILY))
        .color(Color::rgba(0, 0, 0, 0))
        .metrics(Metrics::new(size, line_height))
        .letter_spacing(f32::from_bits(inline.width) / size)
}

/// The height of the inline box `inline`, above and below the baseline.
fn box_height(inline: InlineMetrics) -> f32 {
    f32::from_bits(inline.ascent) + f32::from_bits(inline.descent)
}

/// The line height each inline box of `runs` needs in `buffer`, shaped
/// with them at their first heights, by run; `None` when every box fits
/// its line already.
///
/// cosmic-text sets a line `L` tall with its glyphs' tallest ascent `A`
/// and deepest descent `D` centered in it, the baseline `(L + A - D) / 2`
/// below its top. A box's glyph adds its ascent `a` to `A` and nothing to
/// `D`, so the box fits above the baseline once `L` is at least
/// `2a - A + D`, and its descent `d` below once `L` is at least
/// `2d + A - D`; the line is never shorter than its text asks or a line
/// of `metrics`, and text taller than its line overflows it as it would
/// without the box.
fn settled_box_lines(
    buffer: &Buffer,
    runs: &[TextRun],
    metrics: Metrics,
) -> Option<Vec<Option<f32>>> {
    let inline_of = |glyph: &LayoutGlyph| runs.get(glyph.metadata).and_then(|run| run.style.inline);
    let mut box_lines = vec![None; runs.len()];
    let mut changed = false;
    let lines = buffer
        .lines
        .iter()
        .filter_map(|line| line.layout_opt())
        .flatten();
    for line in lines {
        let (ascent, descent) = (line.max_ascent, line.max_descent);
        let mut height = metrics.line_height;
        let mut has_box = false;
        for glyph in &line.glyphs {
            let needs = match inline_of(glyph) {
                Some(inline) => {
                    has_box = true;
                    let above = 2.0 * f32::from_bits(inline.ascent) - ascent + descent;
                    let below = 2.0 * f32::from_bits(inline.descent) + ascent - descent;
                    above.max(below)
                }
                None => glyph.line_height_opt.unwrap_or(0.0),
            };
            height = height.max(needs);
        }
        if !has_box {
            continue;
        }
        for glyph in &line.glyphs {
            let Some(inline) = inline_of(glyph) else {
                continue;
            };
            changed |= height != box_height(inline).max(metrics.line_height);
            if let Some(slot) = box_lines.get_mut(glyph.metadata) {
                *slot = Some(height);
            }
        }
    }
    changed.then_some(box_lines)
}

/// Where `segment`, shaped `line_width` wide with its baseline `baseline`
/// below its top, is drawn. A centered segment wider than its box (a
/// fallback font's glyph broader than its cells) is shrunk to the box's
/// width about its baseline, so it neither spills into the next cells nor
/// leaves the row's baseline.
pub(super) fn place_segment(segment: &TextSegment, line_width: f32, baseline: f32) -> Placement {
    let SegmentAlign::Center(box_width) = segment.align else {
        return Placement {
            scale: UNSCALED,
            x: segment.origin.x,
            y: segment.origin.y,
        };
    };
    let scale = match line_width > box_width && box_width > 0.0 {
        true => box_width / line_width,
        false => UNSCALED,
    };
    Placement {
        scale,
        x: segment.origin.x + (box_width - line_width * scale) / 2.0,
        y: segment.origin.y + baseline * (UNSCALED - scale),
    }
}

/// The font family named `name`, or the system monospace family.
fn family_of(name: Option<&str>) -> Family<'_> {
    match name {
        Some(name) => Family::Name(name),
        None => Family::Monospace,
    }
}

/// The family `wanted` names in `font_system`: a named one that no font
/// has is the proportional family.
fn span_family<'a>(font_system: &FontSystem, wanted: &'a FontFamily) -> Family<'a> {
    match wanted {
        FontFamily::Monospace => Family::Monospace,
        FontFamily::Named(name) => {
            let installed = font_system
                .db()
                .faces()
                .any(|face| face.families.iter().any(|(family, _)| family == name));
            match installed {
                true => Family::Name(name),
                false => Family::SansSerif,
            }
        }
        FontFamily::Proportional => Family::SansSerif,
        FontFamily::Serif => Family::Serif,
    }
}

/// `text` cut into its pieces of icon glyphs and of the rest, in order,
/// each said to be icons or not. Icon fonts (Nerd Fonts, Font Awesome) put
/// their glyphs in Unicode's Private Use Areas, which no text font fills.
fn icon_pieces(text: &str) -> Vec<(&str, bool)> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut in_icons = None;
    for (at, c) in text.char_indices() {
        let is_icon = is_private_use(c);
        if in_icons.is_some_and(|was| was != is_icon) {
            pieces.push((&text[start..at], !is_icon));
            start = at;
        }
        in_icons = Some(is_icon);
    }
    if let Some(is_icon) = in_icons {
        pieces.push((&text[start..], is_icon));
    }
    pieces
}

/// Whether `c` is in one of Unicode's Private Use Areas, where icon fonts
/// put their glyphs.
fn is_private_use(c: char) -> bool {
    matches!(u32::from(c), 0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD)
}

/// The text bounds of `clip`, widened to whole pixels so a glyph on the
/// clip's edge is cut there and not a pixel early.
fn bounds_of(clip: Rect) -> TextBounds {
    TextBounds {
        bottom: (clip.y + clip.height).ceil() as i32,
        left: clip.x.floor() as i32,
        right: (clip.x + clip.width).ceil() as i32,
        top: clip.y.floor() as i32,
    }
}

/// `attrs` in the weight and slant of `face`, its weight one of `weights`.
fn face_attrs(attrs: Attrs<'_>, face: Face, weights: FaceWeights) -> Attrs<'_> {
    let weight = match face.bold {
        true => weights.bold,
        false => weights.normal,
    };
    let attrs = attrs.weight(Weight(weight));
    match face.italic {
        true => attrs.style(Style::Italic),
        false => attrs,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::app::offscreen;
    use crate::base::color::{Rgb, Rgba};
    use crate::base::geometry::Point;
    use crate::base::styled::StyledText;
    use crate::base::theme::{Theme, ThemeToken};
    use crate::render::draw::RunStyle;
    use crate::render::paragraph::Paragraph;

    /// Wide, emoji, Hebrew, and Arabic text, each with the cells it takes.
    const SAMPLES: [(&str, usize); 4] = [("漢字", 4), ("😀", 2), ("שלום", 4), ("مرحبا", 5)];

    /// The size text is shaped at to look for its glyphs.
    const TEST_METRICS: Metrics = Metrics {
        font_size: 16.0,
        line_height: 20.0,
    };

    /// Whether `text` should draw: fontconfig says some installed font has
    /// every character of it, and then the fallback must find them all;
    /// where fontconfig is missing, whether the fallback found them. Says
    /// which text is skipped when not.
    fn has_glyphs(text: &str) -> bool {
        let mut fonts = Fonts::new();
        let runs = [TextRun {
            color: Rgb::new(0, 0, 0),
            face: Face::default(),
            style: RunStyle::default(),
            text: text.to_owned(),
        }];
        let buffer = shape_text(
            &mut fonts.font_system,
            TEST_METRICS,
            None,
            &runs,
            None,
            true,
            FaceWeights::default(),
        );
        let found = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .all(|glyph| glyph.glyph_id != 0);
        let charset: Vec<String> = text
            .chars()
            .map(|c| format!("{:x}", u32::from(c)))
            .collect();
        let listed = Command::new("fc-list")
            .arg(format!(":charset={}", charset.join(" ")))
            .arg("family")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| !output.stdout.is_empty());
        match listed {
            Some(true) => {
                assert!(
                    found,
                    "a font has {text:?}, but the fallback did not find it"
                );
                true
            }
            Some(false) | None if !found => {
                eprintln!("skipped {text:?}: no installed font has its glyphs");
                false
            }
            Some(false) | None => true,
        }
    }

    #[test]
    fn test_icon_glyphs_are_cut_out_of_the_text_around_them_in_order() {
        assert_eq!(icon_pieces("main"), [("main", false)]);
        assert_eq!(
            icon_pieces("\u{e0a0} main \u{f0e7}\u{f0e8}x"),
            [
                ("\u{e0a0}", true),
                (" main ", false),
                ("\u{f0e7}\u{f0e8}", true),
                ("x", false)
            ]
        );
        assert_eq!(
            icon_pieces("\u{f0001}"),
            [("\u{f0001}", true)],
            "the supplementary area"
        );
        assert!(icon_pieces("").is_empty());
    }

    #[test]
    fn test_regular_and_bold_runs_take_the_fonts_weights() {
        let weights = FaceWeights {
            bold: 500,
            normal: 300,
        };
        let regular = face_attrs(Attrs::new(), Face::default(), weights);
        let bold = Face {
            bold: true,
            ..Face::default()
        };
        let bold = face_attrs(Attrs::new(), bold, weights);
        assert_eq!(regular.weight, Weight(300));
        assert_eq!(bold.weight, Weight(500));
    }

    #[test]
    fn test_a_wrapped_paragraph_is_shaped_in_the_weights_it_is_given() {
        // A paragraph was set regular whatever the window font's weight,
        // so a page's words read heavier than a light grid's cells.
        let light = FaceWeights {
            bold: 500,
            normal: 300,
        };
        let runs = [TextRun {
            color: Rgb::new(0, 0, 0),
            face: Face::default(),
            style: RunStyle::default(),
            text: "light".to_owned(),
        }];
        let wrap = Wrapping {
            align: TextAlign::Start,
            line_height: None,
            width: 200.0,
        };
        let buffer = shape_text(
            &mut Fonts::new().font_system,
            TEST_METRICS,
            None,
            &runs,
            Some(wrap),
            true,
            light,
        );
        let attrs = buffer.lines[0].attrs_list().get_span(0);
        assert_eq!(attrs.weight, Weight(300));
    }

    #[test]
    fn test_wide_emoji_and_right_to_left_text_draw_from_fallback_fonts_on_the_grid() {
        let theme = Theme::dark();
        let background = theme.background;
        let samples: Vec<(&str, usize)> = SAMPLES
            .into_iter()
            .filter(|(text, _)| has_glyphs(text))
            .collect();
        let mut drawn = Vec::new();
        let Some(snapshot) = offscreen::render(200, 160, |frame| {
            frame.clear(Rgba::from(background));
            let metrics = frame.cell_metrics();
            for (index, (text, _)) in samples.iter().enumerate() {
                let origin = Point::new(4.0, 4.0 + index as f32 * metrics.height);
                let line = StyledText::plain(*text, ThemeToken::Foreground.into());
                drawn.push(frame.draw_text(&line, origin, frame.bounds(), &theme));
            }
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        for (index, ((text, cells), drawn)) in samples.iter().zip(drawn).enumerate() {
            assert_eq!(drawn, *cells, "{text} takes its cells");
            let top = 4.0 + index as f32 * metrics.height;
            let rect = Rect::new(4.0, top, *cells as f32 * metrics.width, metrics.height);
            assert!(!snapshot.is_plain(rect, background), "{text} drew glyphs");
        }
    }

    #[test]
    fn test_wide_emoji_and_right_to_left_text_draw_from_fallback_fonts_in_a_paragraph() {
        let theme = Theme::dark();
        let background = theme.background;
        let samples: Vec<&str> = SAMPLES
            .into_iter()
            .map(|(text, _)| text)
            .filter(|text| has_glyphs(text))
            .collect();
        let Some(snapshot) = offscreen::render(200, 160, |frame| {
            frame.clear(Rgba::from(background));
            let line = frame.cell_metrics().height;
            for (index, text) in samples.iter().enumerate() {
                let paragraph = Paragraph::plain(*text, ThemeToken::Foreground.into());
                let rect = Rect::new(4.0, 4.0 + index as f32 * line, 190.0, line);
                frame.draw_paragraph(&paragraph, rect, &theme);
            }
        }) else {
            return;
        };
        let line = snapshot.metrics().height;
        for (index, text) in samples.iter().enumerate() {
            let rect = Rect::new(4.0, 4.0 + index as f32 * line, 190.0, line);
            assert!(!snapshot.is_plain(rect, background), "{text} drew glyphs");
        }
    }

    #[test]
    fn test_text_drawn_after_a_fill_in_a_later_batch_paints_over_it() {
        let theme = Theme::dark();
        let Some(snapshot) = offscreen::render(120, 60, |frame| {
            frame.clear(Rgba::from(theme.background));
            let line = frame.cell_metrics().height;
            frame.draw_plain_text("MMMM", Point::new(4.0, 4.0), theme.foreground);
            frame.fill_rect(frame.bounds(), theme.accent);
            frame.draw_plain_text("MMMM", Point::new(4.0, 4.0 + line), theme.foreground);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        let line = |row: f32| {
            Rect::new(
                4.0,
                4.0 + row * metrics.height,
                4.0 * metrics.width,
                metrics.height,
            )
        };
        assert!(
            snapshot.is_plain(line(0.0), theme.accent),
            "the first line is under the fill drawn after it"
        );
        assert!(
            !snapshot.is_plain(line(1.0), theme.accent),
            "the second line is over the fill drawn before it"
        );
    }

    fn segment(align: SegmentAlign) -> TextSegment {
        TextSegment {
            align,
            clip: None,
            origin: Point::new(10.0, 40.0),
            runs: Vec::new(),
            transform: None,
            wrap: None,
        }
    }

    #[test]
    fn test_a_glyph_wider_than_its_cells_shrinks_to_them_on_the_baseline() {
        let baseline = 16.0;
        let wide = place_segment(&segment(SegmentAlign::Center(10.0)), 20.0, baseline);
        assert_eq!(wide.scale, 0.5, "half as wide");
        assert_eq!(wide.x, 10.0, "filling its box from the left edge");
        assert_eq!(
            wide.y + baseline * wide.scale,
            40.0 + baseline,
            "its baseline where the row's is"
        );
        let narrow = place_segment(&segment(SegmentAlign::Center(10.0)), 6.0, baseline);
        assert_eq!(
            narrow,
            Placement {
                scale: UNSCALED,
                x: 12.0,
                y: 40.0
            },
            "a narrower glyph keeps its size, centered"
        );
        let start = place_segment(&segment(SegmentAlign::Start), 200.0, baseline);
        assert_eq!(start.scale, UNSCALED, "a run of cells is never shrunk");
    }

    /// The glyphs `text` is shaped into on the grid in `family`.
    fn glyph_ids(fonts: &mut Fonts, family: &str, text: &str, ligatures: bool) -> Vec<u16> {
        let runs = [TextRun {
            color: Rgb::new(0, 0, 0),
            face: Face::default(),
            style: RunStyle::default(),
            text: text.to_owned(),
        }];
        let metrics = TEST_METRICS;
        let buffer = shape_text(
            &mut fonts.font_system,
            metrics,
            Some(family),
            &runs,
            None,
            ligatures,
            FaceWeights::default(),
        );
        buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.glyph_id))
            .collect()
    }

    #[test]
    fn test_a_font_without_ligatures_draws_each_character_on_its_own_glyph() {
        let family = "JetBrainsMono Nerd Font Mono";
        let mut fonts = Fonts::new();
        let installed = fonts
            .font_system
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == family));
        if !installed {
            eprintln!("skipped: {family} is not installed");
            return;
        }
        let joined = glyph_ids(&mut fonts, family, "a->b", true);
        let apart = glyph_ids(&mut fonts, family, "a->b", false);
        assert_ne!(joined, apart, "the arrow is a ligature only when asked");
        let typed: Vec<u16> = ["a", "-", ">", "b"]
            .into_iter()
            .flat_map(|text| glyph_ids(&mut fonts, family, text, true))
            .collect();
        assert_eq!(apart, typed, "each character on its own glyph");
    }
}

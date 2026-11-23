//! What a frame asks the GPU to draw, as plain data: filled rectangles,
//! plain or with rounded corners, a border, or a blur ([`Quad`]s), pictures ([`PictureDraw`]s), and positioned text
//! ([`TextSegment`]s), and vector paths tessellated into triangles
//! ([`PathDraw`]s), collected in a [`DrawList`] in the order they were
//! drawn, which is the order they are painted in: a [`DrawBatch`] per run
//! of consecutive draws of one kind. A [`SolidFill`] is the public value
//! for the plainest of them, a rectangle of one color, which an app keeps
//! and draws with [`Frame::fill_rect`](crate::Frame::fill_rect).
//!
//! Inside [`Frame::with_transform`](crate::Frame::with_transform) the list
//! holds the transform in effect: a quad, picture, or text segment pushed
//! then keeps its own coordinates and carries the transform
//! ([`Transformed`]), which the GPU applies; one that only moves is moved
//! as it is pushed instead, so it is drawn exactly as untransformed draws
//! are. Paths are tessellated already transformed.

use std::ops::Range;

use super::super::base::box_style::Border;
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::paint::Paint;
use super::super::base::styled::FontFamily;
use super::super::base::transform::Transform;
use super::image::{Image, ImageDraw, Svg};
use super::paragraph::TextAlign;
use super::texture::GpuTexture;

// ========================================================================
// Constants
// ========================================================================

/// How many kinds of draw a [`DrawList`] holds: the variants of
/// [`DrawKind`].
const DRAW_KINDS: usize = 5;

/// How many kinds of batch the renderer paints: the variants of
/// [`BatchKind`].
const BATCH_KINDS: usize = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// A rectangle filled with one color, as a value an app builds, keeps, and
/// compares: a cell's background, a separator, a cursor block. Draw it with
/// [`Frame::fill_rect`](crate::Frame::fill_rect)`(fill.rect, fill.color)`,
/// which paints it in call order, over what was drawn before it and under
/// what is drawn after; a run of fills drawn one after another is painted
/// as one batch.
///
/// ```
/// use misty::{Rect, Rgba, SolidFill};
///
/// let separator = SolidFill::new(Rect::new(0.0, 40.0, 300.0, 1.0), Rgba::new(80, 80, 80, 255));
/// assert_eq!(separator.rect.height, 1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidFill {
    /// The color, opaque or with alpha, which blends it over what is under
    /// it.
    pub color: Rgba,
    /// The rectangle filled, in physical pixels.
    pub rect: Rect,
}

/// Everything one layer of a frame draws, painted in the order it was
/// drawn: whatever is drawn later paints over what was drawn before, of
/// any kind. Each kind is kept in a list of its own, and `order` says how
/// the lists interleave, so the renderer paints consecutive draws of one
/// kind as one batch ([`DrawList::batches`]).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DrawList {
    /// Lines and marks over text: underlines, strikethroughs, thin cursors.
    decorations: Vec<Quad>,
    /// Boxes and backgrounds.
    fills: Vec<Quad>,
    /// The kind of every draw, in the order drawn.
    order: Vec<DrawKind>,
    /// Vector paths.
    paths: Vec<PathDraw>,
    /// Images, SVG documents, and app textures.
    pictures: Vec<PictureDraw>,
    /// The text.
    text: Vec<TextSegment>,
    /// The transform quads, pictures, and text pushed now are drawn
    /// through, set by [`Frame::with_transform`](crate::Frame::with_transform);
    /// `None` for none.
    transform: Option<Transform>,
}

/// Draws gathered by kind, for a builder that emits many interleaved
/// pieces (a cell grid, a line of text, a paragraph), added to a
/// [`DrawList`] kind after kind: fills, then paths, then text, then
/// decorations. The pieces paint as four batches, backgrounds under every
/// glyph and lines over them, whatever order they were gathered in.
#[derive(Debug, Default)]
pub(crate) struct DrawPhases {
    /// Lines and marks drawn over the text.
    pub(crate) decorations: Vec<Quad>,
    /// Backgrounds drawn under everything else.
    pub(crate) fills: Vec<Quad>,
    /// Vector paths drawn over the fills.
    pub(crate) paths: Vec<PathDraw>,
    /// The text.
    pub(crate) text: Vec<TextSegment>,
}

/// What one draw of a [`DrawList`] is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawKind {
    /// A quad of the decorations.
    Decoration,
    /// A quad of the fills.
    Fill,
    /// A vector path.
    Path,
    /// A picture.
    Picture,
    /// A text segment.
    Text,
}

/// A run of consecutive draws of a [`DrawList`] painted with one pipeline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DrawBatch {
    /// What the run draws.
    pub(crate) kind: BatchKind,
    /// Which of the list's draws of that kind it holds: for quads, an
    /// index range of [`DrawList::quads`]; otherwise of the kind's own
    /// list.
    pub(crate) range: Range<usize>,
}

/// What a [`DrawBatch`] draws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BatchKind {
    /// Vector paths.
    Paths,
    /// Pictures.
    Pictures,
    /// Fills and decorations, which share a pipeline.
    Quads,
    /// Text segments.
    Text,
}

/// How long each list of a [`DrawList`] was, to find what was drawn
/// after.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DrawMarks {
    /// The decorations drawn before.
    decorations: usize,
    /// The fills drawn before.
    fills: usize,
    /// The draws of every kind before.
    order: usize,
    /// The paths drawn before.
    paths: usize,
    /// The pictures drawn before.
    pictures: usize,
    /// The text segments drawn before.
    text: usize,
}

/// A rectangle filled with one color, its corners rounded by `radius`, a
/// `border` along the inside of its edge, and its edge faded over `blur`
/// pixels each way (a shadow). With none of these it is a plain rectangle,
/// drawn as the pixels whose centers it covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Quad {
    /// How far the edge fades, in pixels each way; 0 for a sharp edge.
    pub(crate) blur: f32,
    /// The line along the inside of the edge, if any.
    pub(crate) border: Option<Border>,
    /// The area outside which nothing is drawn, for a quad a clip cannot
    /// simply cut without changing its shape; `None` for the whole window.
    pub(crate) clip: Option<Rect>,
    /// The fill color.
    pub(crate) color: Rgba,
    /// The radius of the corners' rounding, in pixels; 0 for square.
    pub(crate) radius: f32,
    /// Where the rectangle is.
    pub(crate) rect: Rect,
    /// How the quad, its `rect` and `clip` in coordinates of its own, lands
    /// in the window; `None` for drawn as it is.
    pub(crate) transform: Option<Transformed>,
}

/// How a quad, picture, or text segment drawn inside
/// [`Frame::with_transform`](crate::Frame::with_transform) lands in the
/// window: its rectangle and its own clip are in coordinates of its own,
/// which `matrix` maps to window pixels, so a clip set inside the
/// transform turns with what it clips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transformed {
    /// The axis-aligned area in window pixels outside which nothing is
    /// drawn, set by a clip outside the transform; `None` for the whole
    /// window.
    pub(crate) clip: Option<Rect>,
    /// Maps the draw's own coordinates to window pixels.
    pub(crate) matrix: Transform,
}

/// Where a clip given in the coordinates a frame is drawing in falls for
/// one draw.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cut {
    /// In the draw's own coordinates, where it is exact.
    Own(Rect),
    /// In window pixels: the clip's axis-aligned bounds, for a draw turned
    /// against the clip.
    Window(Rect),
}

/// A vector path, filled or stroked, as antialiased triangles in window
/// pixels: the triangles inside it are wholly covered, and a pixel wide
/// fringe around its edge fades from covered to clear.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PathDraw {
    /// The area the triangles touch.
    pub(crate) bounds: Rect,
    /// The area outside which nothing is drawn; `None` for the whole
    /// window.
    pub(crate) clip: Option<Rect>,
    /// The triangles, three indices into `vertices` each.
    pub(crate) indices: Vec<u32>,
    /// What multiplies the paint's alpha everywhere: under 1 for a line
    /// thinner than a pixel, drawn a pixel wide.
    pub(crate) opacity: f32,
    /// The color or gradient.
    pub(crate) paint: Paint,
    /// The triangles' corners.
    pub(crate) vertices: Vec<PathVertex>,
}

/// A corner of a [`PathDraw`]'s triangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PathVertex {
    /// How much of a pixel here the path covers, 0 to 1.
    pub(crate) coverage: f32,
    /// Where it falls along the paint's gradient, 0 at its start and 1 at
    /// its end; unused for one color.
    pub(crate) gradient: f32,
    /// Where it is, in window pixels.
    pub(crate) position: Point,
}

/// A picture drawn over a rectangle.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PictureDraw {
    /// The part of `rect` shown, cut by a clip; `None` for all of it.
    pub(crate) crop: Option<Rect>,
    /// What multiplies every pixel's alpha: 1 for the picture as it is,
    /// under 1 inside [`Frame::with_opacity`](crate::Frame::with_opacity).
    pub(crate) opacity: f32,
    /// Where it is drawn: stretched over it for an image; for an SVG, the
    /// box it is rasterized into, keeping its shape.
    pub(crate) rect: Rect,
    /// What is drawn.
    pub(crate) source: PictureSource,
    /// How an image is turned, mirrored, and sampled.
    pub(crate) style: ImageDraw,
    /// The color every pixel takes, keeping its alpha, for a one-color
    /// icon drawn in a theme color; `None` for the picture's own colors.
    pub(crate) tint: Option<Rgb>,
    /// How the picture, its `rect` and `crop` in coordinates of its own,
    /// lands in the window; `None` for drawn as it is.
    pub(crate) transform: Option<Transformed>,
}

/// A picture's pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PictureSource {
    /// An image, uploaded as is.
    Image(Image),
    /// An SVG document, rasterized at the size drawn.
    Svg(Svg),
    /// A texture the app renders into, drawn as it is now.
    Texture(GpuTexture),
}

/// A piece of text laid out as one line from a fixed position. Its runs
/// follow each other at their natural advance.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextSegment {
    /// Where the line is placed horizontally relative to `origin`.
    pub(crate) align: SegmentAlign,
    /// The area outside which no part of a glyph is drawn, or `None` for
    /// the whole window.
    pub(crate) clip: Option<Rect>,
    /// The top left corner of the line box.
    pub(crate) origin: Point,
    /// The text, a run per change of color or face.
    pub(crate) runs: Vec<TextRun>,
    /// How the segment, its `origin` and `clip` in coordinates of its own,
    /// lands in the window; `None` for drawn as it is.
    pub(crate) transform: Option<Transformed>,
    /// How a paragraph wraps, in the proportional font; `None` for one
    /// line on the cell grid's font.
    pub(crate) wrap: Option<Wrapping>,
}

/// How a paragraph's text wraps and aligns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wrapping {
    /// Where each line sits in the width.
    pub(crate) align: TextAlign,
    /// How tall a line is as a multiple of its font size, by its bits, or
    /// `None` for the font's own line height.
    pub(crate) line_height: Option<u32>,
    /// The width lines wrap at, in pixels.
    pub(crate) width: f32,
}

/// Text in one color and face.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TextRun {
    /// The glyphs' color.
    pub(crate) color: Rgb,
    /// The weight and slant.
    pub(crate) face: Face,
    /// What a paragraph's span sets beyond its color and face.
    pub(crate) style: RunStyle,
    /// The text itself.
    pub(crate) text: String,
}

/// What a paragraph's span sets beyond its color and face; the default
/// leaves everything to the segment.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct RunStyle {
    /// The glyphs' alpha, or `None` for opaque.
    pub(crate) alpha: Option<u8>,
    /// The family, or `None` for the segment's.
    pub(crate) family: Option<FontFamily>,
    /// The box the run stands for, or `None` for a run of text.
    pub(crate) inline: Option<InlineMetrics>,
    /// The bits of the font size in physical pixels, or `None` for the
    /// segment's.
    pub(crate) size: Option<u32>,
}

/// The size of an inline box a run stands for, in physical pixels, by the
/// bits of each number.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InlineMetrics {
    /// How far it reaches above the baseline.
    pub(crate) ascent: u32,
    /// How far it reaches below the baseline.
    pub(crate) descent: u32,
    /// Its width.
    pub(crate) width: u32,
}

/// The weight and slant text is drawn in.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct Face {
    /// Whether the text is bold.
    pub(crate) bold: bool,
    /// Whether the text is italic.
    pub(crate) italic: bool,
}

/// How a [`TextSegment`] sits horizontally.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SegmentAlign {
    /// Centered in a box this many pixels wide starting at the origin, for a
    /// glyph whose advance may not match its cells (wide or fallback).
    Center(f32),
    /// Starting at the origin.
    Start,
}

// ========================================================================
// SolidFill Implementations
// ========================================================================

impl SolidFill {
    /// `rect` filled with `color`: a theme color, or one with alpha.
    pub fn new(rect: Rect, color: impl Into<Rgba>) -> Self {
        Self {
            color: color.into(),
            rect,
        }
    }
}

// ========================================================================
// Quad Implementations
// ========================================================================

impl Quad {
    /// A plain rectangle `rect` filled with `color`.
    pub(crate) fn solid(rect: Rect, color: Rgba) -> Self {
        Self {
            blur: 0.0,
            border: None,
            clip: None,
            color,
            radius: 0.0,
            rect,
            transform: None,
        }
    }

    /// Whether the quad is a plain rectangle, which a clip cuts to a
    /// smaller plain rectangle.
    fn is_plain(&self) -> bool {
        self.radius <= 0.0 && self.blur <= 0.0 && self.border.is_none()
    }

    /// The area the quad may touch: its rectangle, grown by its blur.
    fn bounds(&self) -> Rect {
        let blur = self.blur.max(0.0);
        let rect = self.rect;
        Rect::new(
            rect.x - blur,
            rect.y - blur,
            rect.width + 2.0 * blur,
            rect.height + 2.0 * blur,
        )
    }

    /// The quad moved right by `dx` and down by `dy` in the window, its
    /// clips with it.
    fn translated(self, dx: f32, dy: f32) -> Self {
        if let Some(transform) = self.transform {
            return Self {
                transform: Some(transform.translated(dx, dy)),
                ..self
            };
        }
        Self {
            clip: self.clip.map(|clip| offset(clip, dx, dy)),
            rect: offset(self.rect, dx, dy),
            ..self
        }
    }

    /// The quad as pushed while `transform` is in effect: moved by it when
    /// it only moves, carrying it otherwise.
    fn placed(self, transform: Option<Transform>) -> Self {
        let Some(matrix) = transform else {
            return self;
        };
        match matrix.translation() {
            Some((dx, dy)) => self.translated(dx, dy),
            None => Self {
                transform: Some(Transformed { clip: None, matrix }),
                ..self
            },
        }
    }

    /// The quad kept inside `clip`, given in coordinates `space` maps to
    /// the window, or `None` when it lies wholly outside.
    fn clipped_in(self, clip: Rect, space: Option<Transform>) -> Option<Self> {
        match (cut_of(clip, space, self.matrix()), self.transform) {
            (Cut::Own(clip), _) | (Cut::Window(clip), None) => self.clipped(clip),
            (Cut::Window(clip), Some(transform)) => {
                let transform = transform.clipped(clip, self.bounds())?;
                Some(Self {
                    transform: Some(transform),
                    ..self
                })
            }
        }
    }

    /// What maps the quad's coordinates to the window, if anything does.
    fn matrix(&self) -> Option<Transform> {
        self.transform.map(|transform| transform.matrix)
    }

    /// The quad with its fill's and border's alpha multiplied by
    /// `opacity`.
    fn faded(self, opacity: f32) -> Self {
        Self {
            border: self.border.map(|border| Border {
                color: fade(border.color, opacity),
                ..border
            }),
            color: fade(self.color, opacity),
            ..self
        }
    }

    /// The quad kept inside `clip`, or `None` when it lies wholly outside:
    /// a plain rectangle is cut to it, any other shape keeps its rectangle
    /// and is clipped where it is drawn.
    fn clipped(self, clip: Rect) -> Option<Self> {
        if self.is_plain() {
            let rect = self.rect.intersection(&clip)?;
            return Some(Self { rect, ..self });
        }
        let clip = match self.clip {
            Some(own) => own.intersection(&clip)?,
            None => clip,
        };
        self.bounds().intersection(&clip)?;
        Some(Self {
            clip: Some(clip),
            ..self
        })
    }
}

// ========================================================================
// DrawList Implementations
// ========================================================================

impl DrawList {
    /// Draws what is pushed from now on through `transform`, after which
    /// quads, pictures, and text land in the window as [`Transformed`]
    /// says; the identity for drawn as they are.
    pub(crate) fn set_transform(&mut self, transform: Transform) {
        self.transform = (transform != Transform::IDENTITY).then_some(transform);
    }

    /// Adds `quad` to the fills: a box or a background.
    pub(crate) fn push_fill(&mut self, quad: Quad) {
        self.fills.push(quad.placed(self.transform));
        self.order.push(DrawKind::Fill);
    }

    /// Adds `quad` to the decorations: a line or a mark over text.
    pub(crate) fn push_decoration(&mut self, quad: Quad) {
        self.decorations.push(quad.placed(self.transform));
        self.order.push(DrawKind::Decoration);
    }

    /// Adds a vector path.
    pub(crate) fn push_path(&mut self, path: PathDraw) {
        self.paths.push(path);
        self.order.push(DrawKind::Path);
    }

    /// Adds a picture.
    pub(crate) fn push_picture(&mut self, picture: PictureDraw) {
        self.pictures.push(picture.placed(self.transform));
        self.order.push(DrawKind::Picture);
    }

    /// Adds a text segment.
    pub(crate) fn push_text(&mut self, segment: TextSegment) {
        self.text.push(segment.placed(self.transform));
        self.order.push(DrawKind::Text);
    }

    /// This list with each transformed text segment replaced, in its
    /// place in paint order, by the picture `picture` makes of it, or left
    /// out when it makes none; `None` when no segment is transformed, for
    /// the list to be drawn as it is.
    pub(crate) fn with_text_as_pictures(
        &self,
        mut picture: impl FnMut(&TextSegment) -> Option<PictureDraw>,
    ) -> Option<DrawList> {
        if self.text.iter().all(|segment| segment.transform.is_none()) {
            return None;
        }
        let mut list = DrawList::default();
        let mut fills = self.fills.iter();
        let mut decorations = self.decorations.iter();
        let mut paths = self.paths.iter();
        let mut pictures = self.pictures.iter();
        let mut text = self.text.iter();
        for kind in &self.order {
            match kind {
                DrawKind::Decoration => list.decorations.extend(decorations.next().copied()),
                DrawKind::Fill => list.fills.extend(fills.next().copied()),
                DrawKind::Path => list.paths.extend(paths.next().cloned()),
                DrawKind::Picture => list.pictures.extend(pictures.next().cloned()),
                DrawKind::Text => {
                    let Some(segment) = text.next() else {
                        continue;
                    };
                    if segment.transform.is_none() {
                        list.text.push(segment.clone());
                    } else if let Some(made) = picture(segment) {
                        list.pictures.push(made);
                        list.order.push(DrawKind::Picture);
                        continue;
                    } else {
                        continue;
                    }
                }
            }
            list.order.push(*kind);
        }
        Some(list)
    }

    /// The decorations, in the order drawn.
    #[cfg(test)]
    pub(crate) fn decorations(&self) -> &[Quad] {
        &self.decorations
    }

    /// The fills, in the order drawn.
    #[cfg(test)]
    pub(crate) fn fills(&self) -> &[Quad] {
        &self.fills
    }

    /// The vector paths, in the order drawn.
    pub(crate) fn paths(&self) -> &[PathDraw] {
        &self.paths
    }

    /// The pictures, in the order drawn.
    pub(crate) fn pictures(&self) -> &[PictureDraw] {
        &self.pictures
    }

    /// The text segments, in the order drawn.
    pub(crate) fn text(&self) -> &[TextSegment] {
        &self.text
    }

    /// The fills and decorations together, in the order drawn: the quads
    /// [`BatchKind::Quads`] ranges index.
    pub(crate) fn quads(&self) -> impl Iterator<Item = &Quad> {
        let mut fills = self.fills.iter();
        let mut decorations = self.decorations.iter();
        self.order.iter().filter_map(move |kind| match kind {
            DrawKind::Decoration => decorations.next(),
            DrawKind::Fill => fills.next(),
            DrawKind::Path | DrawKind::Picture | DrawKind::Text => None,
        })
    }

    /// How many quads [`DrawList::quads`] yields.
    pub(crate) fn quad_count(&self) -> usize {
        self.fills.len() + self.decorations.len()
    }

    /// The draws as the renderer paints them, in order: one batch per run
    /// of consecutive draws of one kind, fills and decorations counting as
    /// one kind.
    pub(crate) fn batches(&self) -> Vec<DrawBatch> {
        let mut next = [0; BATCH_KINDS];
        let mut batches: Vec<DrawBatch> = Vec::new();
        for kind in &self.order {
            let kind = kind.batch_kind();
            let slot = &mut next[kind.index()];
            let start = *slot;
            *slot += 1;
            match batches.last_mut() {
                Some(batch) if batch.kind == kind => batch.range.end = *slot,
                Some(_) | None => batches.push(DrawBatch {
                    kind,
                    range: start..*slot,
                }),
            }
        }
        batches
    }

    /// How long each list is now.
    pub(crate) fn marks(&self) -> DrawMarks {
        DrawMarks {
            decorations: self.decorations.len(),
            fills: self.fills.len(),
            order: self.order.len(),
            paths: self.paths.len(),
            pictures: self.pictures.len(),
            text: self.text.len(),
        }
    }

    /// Keeps what was drawn since `marks` inside `clip`: plain rectangles
    /// are cut to it, other shapes, paths, and text are clipped to it, and
    /// what lies wholly outside is dropped. A picture partly inside is
    /// cropped to it.
    ///
    /// `clip` is in the coordinates of the transform in effect, so it
    /// turns with what is drawn inside it: exactly for what was drawn in
    /// that transform (or one differing from it by a move, a scale, or a
    /// quarter turn), and for paths, which are cut along its turned edges;
    /// a quad, picture, or text drawn inside a further turn is kept inside
    /// the clip's axis-aligned bounds in the window instead.
    pub(crate) fn clip_since(&mut self, marks: DrawMarks, clip: Rect) {
        let space = self.transform;
        let mut kept: [Vec<bool>; DRAW_KINDS] = Default::default();
        kept[DrawKind::Fill.index()] = retain_since(&mut self.fills, marks.fills, |quad| {
            quad.clipped_in(clip, space)
        });
        kept[DrawKind::Decoration.index()] =
            retain_since(&mut self.decorations, marks.decorations, |quad| {
                quad.clipped_in(clip, space)
            });
        kept[DrawKind::Path.index()] = retain_since(&mut self.paths, marks.paths, |path| {
            path.clipped_in(clip, space)
        });
        kept[DrawKind::Picture.index()] =
            retain_since(&mut self.pictures, marks.pictures, |picture| {
                picture.clipped_in(clip, space)
            });
        kept[DrawKind::Text.index()] = retain_since(&mut self.text, marks.text, |segment| {
            segment.clipped_in(clip, space)
        });
        let start = marks.order.min(self.order.len());
        let mut seen = [0; DRAW_KINDS];
        let order: Vec<DrawKind> = self
            .order
            .drain(start..)
            .filter(|kind| {
                let slot = &mut seen[kind.index()];
                let is_kept = kept[kind.index()].get(*slot).copied().unwrap_or(true);
                *slot += 1;
                is_kept
            })
            .collect();
        self.order.extend(order);
    }

    /// Multiplies the alpha of everything drawn since `marks` by
    /// `opacity` (0 to 1): fills, borders, paths, pictures, text, and
    /// decorations, each on its own, so overlapping shapes inside show
    /// through each other where a layer faded as one would not.
    pub(crate) fn fade_since(&mut self, marks: DrawMarks, opacity: f32) {
        let opacity = opacity.clamp(0.0, 1.0);
        let start = marks.fills.min(self.fills.len());
        for quad in &mut self.fills[start..] {
            *quad = quad.faded(opacity);
        }
        let start = marks.decorations.min(self.decorations.len());
        for quad in &mut self.decorations[start..] {
            *quad = quad.faded(opacity);
        }
        let start = marks.paths.min(self.paths.len());
        for path in &mut self.paths[start..] {
            path.opacity *= opacity;
        }
        let start = marks.pictures.min(self.pictures.len());
        for picture in &mut self.pictures[start..] {
            picture.opacity *= opacity;
        }
        let start = marks.text.min(self.text.len());
        for run in self.text[start..]
            .iter_mut()
            .flat_map(|segment| &mut segment.runs)
        {
            let alpha = f32::from(run.style.alpha.unwrap_or(u8::MAX)) * opacity;
            run.style.alpha = Some(alpha.round() as u8);
        }
    }

    /// Moves everything drawn since `marks` right by `dx` and down by `dy`
    /// pixels, clips included.
    pub(crate) fn translate_since(&mut self, marks: DrawMarks, dx: f32, dy: f32) {
        let start = marks.fills.min(self.fills.len());
        for quad in &mut self.fills[start..] {
            *quad = quad.translated(dx, dy);
        }
        let start = marks.decorations.min(self.decorations.len());
        for quad in &mut self.decorations[start..] {
            *quad = quad.translated(dx, dy);
        }
        let start = marks.paths.min(self.paths.len());
        for path in &mut self.paths[start..] {
            path.bounds = offset(path.bounds, dx, dy);
            path.clip = path.clip.map(|clip| offset(clip, dx, dy));
            for vertex in &mut path.vertices {
                vertex.position = Point::new(vertex.position.x + dx, vertex.position.y + dy);
            }
        }
        let start = marks.pictures.min(self.pictures.len());
        for picture in &mut self.pictures[start..] {
            picture.translate(dx, dy);
        }
        let start = marks.text.min(self.text.len());
        for segment in &mut self.text[start..] {
            segment.translate(dx, dy);
        }
    }
}

// ========================================================================
// Transformed Implementations
// ========================================================================

impl Transformed {
    /// The same, moved right by `dx` and down by `dy` in the window, its
    /// window clip with it.
    fn translated(self, dx: f32, dy: f32) -> Self {
        Self {
            clip: self.clip.map(|clip| offset(clip, dx, dy)),
            matrix: self.matrix.then(Transform::translate(dx, dy)),
        }
    }

    /// The same kept inside `clip`, in window pixels, for a draw covering
    /// `bounds` of its own coordinates; `None` when it lies wholly outside.
    fn clipped(self, clip: Rect, bounds: Rect) -> Option<Self> {
        let clip = match self.clip {
            Some(own) => own.intersection(&clip)?,
            None => clip,
        };
        self.matrix.bounds_of(bounds).intersection(&clip)?;
        Some(Self {
            clip: Some(clip),
            ..self
        })
    }
}

// ========================================================================
// PathDraw Implementations
// ========================================================================

impl PathDraw {
    /// The path kept inside `clip`, given in coordinates `space` maps to
    /// the window, or `None` when it lies wholly outside: clipped to it
    /// where it is drawn when it stays axis-aligned in the window, its
    /// triangles cut along its turned edges otherwise.
    fn clipped_in(self, clip: Rect, space: Option<Transform>) -> Option<Self> {
        let (bounds, turned) = match space {
            None => (clip, None),
            Some(space) if space.is_axis_aligned() => (space.bounds_of(clip), None),
            Some(space) => (space.bounds_of(clip), Some(space.corners(clip))),
        };
        let window = match self.clip {
            Some(own) => own.intersection(&bounds)?,
            None => bounds,
        };
        self.bounds.intersection(&window)?;
        let path = match turned {
            Some(corners) => cut_path(self, corners),
            None => self,
        };
        Some(PathDraw {
            clip: Some(window),
            ..path
        })
    }
}

// ========================================================================
// PictureDraw Implementations
// ========================================================================

impl PictureDraw {
    /// The picture as pushed while `transform` is in effect: moved by it
    /// when it only moves, carrying it otherwise.
    fn placed(mut self, transform: Option<Transform>) -> Self {
        let Some(matrix) = transform else {
            return self;
        };
        match matrix.translation() {
            Some((dx, dy)) => self.translate(dx, dy),
            None => self.transform = Some(Transformed { clip: None, matrix }),
        }
        self
    }

    /// Moves the picture right by `dx` and down by `dy` in the window, its
    /// crop or window clip with it.
    fn translate(&mut self, dx: f32, dy: f32) {
        if let Some(transform) = self.transform {
            self.transform = Some(transform.translated(dx, dy));
            return;
        }
        self.rect = offset(self.rect, dx, dy);
        self.crop = self.crop.map(|crop| offset(crop, dx, dy));
    }

    /// The picture kept inside `clip`, given in coordinates `space` maps
    /// to the window, or `None` when it lies wholly outside.
    fn clipped_in(self, clip: Rect, space: Option<Transform>) -> Option<Self> {
        let matrix = self.transform.map(|transform| transform.matrix);
        let shown = self.crop.unwrap_or(self.rect);
        match (cut_of(clip, space, matrix), self.transform) {
            (Cut::Own(clip), _) | (Cut::Window(clip), None) => Some(PictureDraw {
                crop: Some(shown.intersection(&clip)?),
                ..self
            }),
            (Cut::Window(clip), Some(transform)) => Some(PictureDraw {
                transform: Some(transform.clipped(clip, shown)?),
                ..self
            }),
        }
    }
}

// ========================================================================
// TextSegment Implementations
// ========================================================================

impl TextSegment {
    /// The segment as pushed while `transform` is in effect: moved by it
    /// when it only moves, carrying it otherwise.
    fn placed(mut self, transform: Option<Transform>) -> Self {
        let Some(matrix) = transform else {
            return self;
        };
        match matrix.translation() {
            Some((dx, dy)) => self.translate(dx, dy),
            None => self.transform = Some(Transformed { clip: None, matrix }),
        }
        self
    }

    /// Moves the segment right by `dx` and down by `dy` in the window, its
    /// clips with it.
    fn translate(&mut self, dx: f32, dy: f32) {
        if let Some(transform) = self.transform {
            self.transform = Some(transform.translated(dx, dy));
            return;
        }
        self.origin = Point::new(self.origin.x + dx, self.origin.y + dy);
        self.clip = self.clip.map(|clip| offset(clip, dx, dy));
    }

    /// The segment kept inside `clip`, given in coordinates `space` maps
    /// to the window. Text has no bounds of its own to drop it by, so it is
    /// dropped only when two clips leave nothing between them.
    fn clipped_in(self, clip: Rect, space: Option<Transform>) -> Option<Self> {
        let matrix = self.transform.map(|transform| transform.matrix);
        match (cut_of(clip, space, matrix), self.transform) {
            (Cut::Own(clip), _) | (Cut::Window(clip), None) => {
                let clip = match self.clip {
                    Some(own) => own.intersection(&clip)?,
                    None => clip,
                };
                Some(TextSegment {
                    clip: Some(clip),
                    ..self
                })
            }
            (Cut::Window(clip), Some(transform)) => {
                let clip = match transform.clip {
                    Some(own) => own.intersection(&clip)?,
                    None => clip,
                };
                Some(TextSegment {
                    transform: Some(Transformed {
                        clip: Some(clip),
                        ..transform
                    }),
                    ..self
                })
            }
        }
    }
}

// ========================================================================
// Wrapping Implementations
// ========================================================================

impl Wrapping {
    /// What the wrapping's shape depends on, to key a cache with: the
    /// width's bits, the alignment, and the line height's bits.
    pub(crate) fn bits(self) -> (u32, TextAlign, Option<u32>) {
        (self.width.to_bits(), self.align, self.line_height)
    }
}

// ========================================================================
// DrawPhases Implementations
// ========================================================================

impl DrawPhases {
    /// Adds the gathered draws to `draw`: the fills, then the paths, then
    /// the text, then the decorations.
    pub(crate) fn append_to(self, draw: &mut DrawList) {
        for quad in self.fills {
            draw.push_fill(quad);
        }
        for path in self.paths {
            draw.push_path(path);
        }
        for segment in self.text {
            draw.push_text(segment);
        }
        for quad in self.decorations {
            draw.push_decoration(quad);
        }
    }
}

// ========================================================================
// DrawKind Implementations
// ========================================================================

impl DrawKind {
    /// The kind's place among the [`DRAW_KINDS`].
    fn index(self) -> usize {
        match self {
            Self::Decoration => 0,
            Self::Fill => 1,
            Self::Path => 2,
            Self::Picture => 3,
            Self::Text => 4,
        }
    }

    /// The kind of batch a draw of this kind is painted in.
    fn batch_kind(self) -> BatchKind {
        match self {
            Self::Decoration | Self::Fill => BatchKind::Quads,
            Self::Path => BatchKind::Paths,
            Self::Picture => BatchKind::Pictures,
            Self::Text => BatchKind::Text,
        }
    }
}

// ========================================================================
// BatchKind Implementations
// ========================================================================

impl BatchKind {
    /// The kind's place among the [`BATCH_KINDS`].
    fn index(self) -> usize {
        match self {
            Self::Paths => 0,
            Self::Pictures => 1,
            Self::Quads => 2,
            Self::Text => 3,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `rect` moved right by `dx` and down by `dy`.
fn offset(rect: Rect, dx: f32, dy: f32) -> Rect {
    Rect::new(rect.x + dx, rect.y + dy, rect.width, rect.height)
}

/// Where `clip`, in the coordinates `space` maps to the window (`None` for
/// window pixels), falls for a draw whose coordinates `matrix` maps there:
/// in the draw's own coordinates when the two differ by no turn, which is
/// exact, and as its bounds in the window otherwise.
fn cut_of(clip: Rect, space: Option<Transform>, matrix: Option<Transform>) -> Cut {
    if space == matrix {
        return Cut::Own(clip);
    }
    let space = space.unwrap_or(Transform::IDENTITY);
    let to_own = match matrix {
        Some(matrix) => matrix.inverse().map(|inverse| space.then(inverse)),
        None => Some(space),
    };
    match to_own {
        Some(to_own) if to_own.is_axis_aligned() => Cut::Own(to_own.bounds_of(clip)),
        Some(_) | None => Cut::Window(space.bounds_of(clip)),
    }
}

/// `path` with its triangles cut to the convex quadrilateral `corners`,
/// given in order around it either way: each triangle is cut along every
/// edge, and what is left inside split back into triangles, their
/// coverage and gradient interpolated where they are cut.
fn cut_path(path: PathDraw, corners: [Point; 4]) -> PathDraw {
    let twice_area: f32 = (0..corners.len())
        .map(|index| {
            let (from, to) = (corners[index], corners[(index + 1) % corners.len()]);
            from.x * to.y - to.x * from.y
        })
        .sum();
    let turn = twice_area.signum();
    let mut vertices: Vec<PathVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for triangle in path.indices.as_chunks::<3>().0 {
        let mut polygon: Vec<PathVertex> = triangle
            .iter()
            .filter_map(|&index| path.vertices.get(index as usize).copied())
            .collect();
        for index in 0..corners.len() {
            let edge = (corners[index], corners[(index + 1) % corners.len()]);
            polygon = cut_polygon(&polygon, edge, turn);
        }
        if polygon.len() < 3 {
            continue;
        }
        let first = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        vertices.extend(&polygon);
        for next in 1..polygon.len() - 1 {
            let next = u32::try_from(next).unwrap_or(u32::MAX);
            indices.extend([first, first + next, first + next + 1]);
        }
    }
    PathDraw {
        indices,
        vertices,
        ..path
    }
}

/// The part of the convex `polygon` on the inner side of `edge`, the side
/// a polygon turning `turn` (1 or -1) way keeps.
fn cut_polygon(polygon: &[PathVertex], (from, to): (Point, Point), turn: f32) -> Vec<PathVertex> {
    let side = |vertex: &PathVertex| {
        let (x, y) = (vertex.position.x - from.x, vertex.position.y - from.y);
        turn * ((to.x - from.x) * y - (to.y - from.y) * x)
    };
    let mut kept = Vec::with_capacity(polygon.len() + 1);
    for (index, current) in polygon.iter().enumerate() {
        let next = &polygon[(index + 1) % polygon.len()];
        let (here, there) = (side(current), side(next));
        if here >= 0.0 {
            kept.push(*current);
        }
        if (here >= 0.0) != (there >= 0.0) {
            kept.push(lerp_vertex(current, next, here / (here - there)));
        }
    }
    kept
}

/// The vertex `t` of the way from `from` to `to`.
fn lerp_vertex(from: &PathVertex, to: &PathVertex, t: f32) -> PathVertex {
    let mix = |a: f32, b: f32| a + (b - a) * t;
    PathVertex {
        coverage: mix(from.coverage, to.coverage),
        gradient: mix(from.gradient, to.gradient),
        position: Point::new(
            mix(from.position.x, to.position.x),
            mix(from.position.y, to.position.y),
        ),
    }
}

/// Keeps the items of `items` from `start` on that `keep` returns, as it
/// returns them; whether each was kept, in order.
fn retain_since<T>(
    items: &mut Vec<T>,
    start: usize,
    mut keep: impl FnMut(T) -> Option<T>,
) -> Vec<bool> {
    let start = start.min(items.len());
    let drained: Vec<T> = items.drain(start..).collect();
    drained
        .into_iter()
        .map(|item| match keep(item) {
            Some(kept) => {
                items.push(kept);
                true
            }
            None => false,
        })
        .collect()
}

/// `color` with its alpha multiplied by `opacity`.
fn fade(color: Rgba, opacity: f32) -> Rgba {
    let alpha = (f32::from(color.a) * opacity).round() as u8;
    Rgba { a: alpha, ..color }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;

    fn quad(rect: Rect) -> Quad {
        Quad::solid(rect, Rgba::BLACK)
    }

    #[test]
    fn test_clipping_cuts_only_what_was_drawn_since_the_marks() {
        let mut list = DrawList::default();
        list.push_fill(quad(Rect::new(0.0, 0.0, 100.0, 100.0)));
        let marks = list.marks();
        list.push_fill(quad(Rect::new(0.0, 0.0, 100.0, 100.0)));
        list.push_decoration(quad(Rect::new(200.0, 0.0, 5.0, 5.0)));
        list.push_text(TextSegment {
            align: SegmentAlign::Start,
            clip: Some(Rect::new(40.0, 40.0, 100.0, 100.0)),
            origin: Point::new(40.0, 40.0),
            runs: Vec::new(),
            transform: None,
            wrap: None,
        });
        let clip = Rect::new(10.0, 10.0, 50.0, 50.0);
        list.clip_since(marks, clip);
        assert_eq!(
            list.fills()[0].rect,
            Rect::new(0.0, 0.0, 100.0, 100.0),
            "before the marks"
        );
        assert_eq!(list.fills()[1].rect, clip);
        assert!(list.decorations().is_empty(), "wholly outside");
        assert_eq!(
            list.text()[0].clip,
            Some(Rect::new(40.0, 40.0, 20.0, 20.0)),
            "both clips"
        );
    }

    #[test]
    fn test_clipping_keeps_a_rounded_shape_and_clips_it_where_drawn() {
        let mut list = DrawList::default();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let rounded = Quad {
            radius: 8.0,
            ..quad(rect)
        };
        list.push_fill(rounded);
        list.push_fill(Quad {
            rect: Rect::new(300.0, 0.0, 10.0, 10.0),
            ..rounded
        });
        list.clip_since(DrawMarks::default(), Rect::new(10.0, 10.0, 50.0, 50.0));
        assert_eq!(list.fills().len(), 1, "the one wholly outside is dropped");
        assert_eq!(
            list.fills()[0].rect,
            rect,
            "the corners stay where they were"
        );
        assert_eq!(
            list.fills()[0].clip,
            Some(Rect::new(10.0, 10.0, 50.0, 50.0))
        );
    }

    #[test]
    fn test_a_blurred_quad_survives_a_clip_its_blur_reaches() {
        let mut list = DrawList::default();
        list.push_fill(Quad {
            blur: 8.0,
            ..quad(Rect::new(0.0, 0.0, 10.0, 10.0))
        });
        list.clip_since(DrawMarks::default(), Rect::new(14.0, 0.0, 10.0, 10.0));
        assert_eq!(
            list.fills().len(),
            1,
            "its fading edge reaches into the clip"
        );
    }

    fn path(bounds: Rect) -> PathDraw {
        PathDraw {
            bounds,
            clip: None,
            indices: Vec::new(),
            opacity: 1.0,
            paint: Paint::Solid(Rgba::BLACK),
            vertices: Vec::new(),
        }
    }

    #[test]
    fn test_clipping_keeps_a_path_between_the_fills_around_it() {
        let mut list = DrawList::default();
        list.push_fill(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let marks = list.marks();
        list.push_fill(quad(Rect::new(500.0, 0.0, 10.0, 10.0)));
        list.push_fill(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.push_path(path(Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.push_path(path(Rect::new(500.0, 0.0, 10.0, 10.0)));
        list.push_fill(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.clip_since(marks, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert_eq!(list.fills().len(), 3, "the fill outside is dropped");
        assert_eq!(list.paths().len(), 1, "the path outside is dropped");
        let kinds: Vec<(BatchKind, Range<usize>)> = list
            .batches()
            .into_iter()
            .map(|batch| (batch.kind, batch.range))
            .collect();
        assert_eq!(
            kinds,
            [
                (BatchKind::Quads, 0..2),
                (BatchKind::Paths, 0..1),
                (BatchKind::Quads, 2..3)
            ],
            "still over the fills drawn before it and under the one after"
        );
        assert_eq!(
            list.paths()[0].clip,
            Some(Rect::new(0.0, 0.0, 100.0, 100.0))
        );
    }

    #[test]
    fn test_consecutive_draws_of_a_kind_batch_and_quads_keep_their_call_order() {
        let mut list = DrawList::default();
        let fill = quad(Rect::new(0.0, 0.0, 10.0, 10.0));
        let line = Quad::solid(Rect::new(0.0, 9.0, 10.0, 1.0), Rgba::rgb(1, 2, 3));
        list.push_fill(fill);
        list.push_text(TextSegment {
            align: SegmentAlign::Start,
            clip: None,
            origin: Point::new(0.0, 0.0),
            runs: Vec::new(),
            transform: None,
            wrap: None,
        });
        list.push_decoration(line);
        list.push_fill(fill);
        let kinds: Vec<(BatchKind, Range<usize>)> = list
            .batches()
            .into_iter()
            .map(|batch| (batch.kind, batch.range))
            .collect();
        assert_eq!(
            kinds,
            [
                (BatchKind::Quads, 0..1),
                (BatchKind::Text, 0..1),
                (BatchKind::Quads, 1..3)
            ],
            "the decoration and the fill after it are one batch"
        );
        let quads: Vec<Quad> = list.quads().copied().collect();
        assert_eq!(quads, [fill, line, fill], "in the order drawn");
    }

    #[test]
    fn test_a_solid_fill_drawn_after_a_picture_paints_over_it_in_call_order() {
        let red = Image::from_rgba(1, 1, vec![255, 0, 0, 255]).expect("one pixel");
        let fill = SolidFill::new(Rect::new(10.0, 10.0, 20.0, 20.0), Rgb::new(0, 0, 255));
        let draw = |frame: &mut crate::Frame| {
            frame.clear(Rgba::BLACK);
            frame.draw_image(&red, Rect::new(0.0, 0.0, 40.0, 40.0));
            frame.fill_rect(fill.rect, fill.color);
        };
        let mut frame = offscreen::plain_frame(40, 40);
        draw(&mut frame);
        let kinds: Vec<BatchKind> = frame
            .draw
            .batches()
            .iter()
            .map(|batch| batch.kind)
            .collect();
        assert_eq!(kinds, [BatchKind::Pictures, BatchKind::Quads]);
        assert_eq!(frame.draw.fills(), [Quad::solid(fill.rect, fill.color)]);
        let Some(snapshot) = offscreen::render(40, 40, draw) else {
            return;
        };
        snapshot.assert_pixel(20.0, 20.0, Rgb::new(0, 0, 255), "the fill over the picture");
        snapshot.assert_pixel(5.0, 5.0, Rgb::new(255, 0, 0), "the picture around it");
    }

    #[test]
    fn test_fading_multiplies_alpha_only_of_what_was_drawn_since() {
        let mut list = DrawList::default();
        list.push_fill(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let marks = list.marks();
        list.push_fill(Quad {
            border: Some(Border {
                color: Rgba::new(0, 0, 0, 200),
                width: 1.0,
            }),
            ..quad(Rect::new(0.0, 0.0, 10.0, 10.0))
        });
        list.push_path(path(Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.push_text(TextSegment {
            align: SegmentAlign::Start,
            clip: None,
            origin: Point::new(0.0, 0.0),
            runs: vec![TextRun {
                color: Rgb::new(1, 2, 3),
                face: Face::default(),
                style: RunStyle::default(),
                text: "a".into(),
            }],
            transform: None,
            wrap: None,
        });
        list.fade_since(marks, 0.5);
        list.fade_since(marks, 0.5);
        assert_eq!(list.fills()[0].color.a, u8::MAX, "drawn before the marks");
        assert_eq!(list.fills()[1].color.a, 64, "nested fades multiply");
        assert_eq!(
            list.fills()[1].border.map(|border| border.color.a),
            Some(50)
        );
        assert_eq!(list.paths()[0].opacity, 0.25);
        assert_eq!(list.text()[0].runs[0].style.alpha, Some(64));
    }

    #[test]
    fn test_translating_moves_shapes_paths_and_their_clips() {
        let mut list = DrawList::default();
        list.push_fill(Quad {
            clip: Some(Rect::new(0.0, 0.0, 5.0, 5.0)),
            radius: 2.0,
            ..quad(Rect::new(0.0, 0.0, 10.0, 10.0))
        });
        let mut moved = path(Rect::new(0.0, 0.0, 10.0, 10.0));
        moved.vertices.push(PathVertex {
            coverage: 1.0,
            gradient: 0.0,
            position: Point::new(1.0, 2.0),
        });
        list.push_path(moved);
        list.translate_since(DrawMarks::default(), 3.0, -1.0);
        assert_eq!(list.fills()[0].rect, Rect::new(3.0, -1.0, 10.0, 10.0));
        assert_eq!(list.fills()[0].clip, Some(Rect::new(3.0, -1.0, 5.0, 5.0)));
        assert_eq!(list.paths()[0].vertices[0].position, Point::new(4.0, 1.0));
        assert_eq!(list.paths()[0].bounds, Rect::new(3.0, -1.0, 10.0, 10.0));
    }

    #[test]
    fn test_nested_clips_narrow_a_shape_to_both() {
        let mut list = DrawList::default();
        list.push_fill(Quad {
            radius: 4.0,
            ..quad(Rect::new(0.0, 0.0, 100.0, 100.0))
        });
        list.clip_since(DrawMarks::default(), Rect::new(0.0, 0.0, 50.0, 50.0));
        list.clip_since(DrawMarks::default(), Rect::new(25.0, 25.0, 50.0, 50.0));
        assert_eq!(
            list.fills()[0].clip,
            Some(Rect::new(25.0, 25.0, 25.0, 25.0))
        );
    }
}

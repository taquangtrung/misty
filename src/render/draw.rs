//! What a frame asks the GPU to draw, as plain data: filled rectangles,
//! plain or with rounded corners, a border, or a blur ([`Quad`]s), pictures ([`PictureDraw`]s), and positioned text
//! ([`TextSegment`]s), and vector paths tessellated into triangles
//! ([`PathDraw`]s), collected in a [`DrawList`] in the order they are
//! painted.

use super::super::base::box_style::Border;
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::paint::Paint;
use super::super::base::styled::FontFamily;
use super::image::{Image, ImageDraw, Svg};
use super::paragraph::TextAlign;
use super::texture::GpuTexture;

// ========================================================================
// Data Structures
// ========================================================================

/// Everything one frame draws, in four layers painted bottom to top: the
/// fills (vector paths painted in order among them), the pictures, then the text, then the decorations (underlines,
/// thin cursors), so a decoration is never hidden under a glyph or a
/// neighbor's fill, and an icon sits on its button's hover fill.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DrawList {
    /// Lines and marks drawn over the text.
    pub(crate) decorations: Vec<Quad>,
    /// Backgrounds drawn under the text.
    pub(crate) fills: Vec<Quad>,
    /// Vector paths, painted in order with the fills: each after the fills
    /// drawn before it and under those drawn after.
    pub(crate) paths: Vec<PathDraw>,
    /// Images and SVG documents, over the fills and under the text.
    pub(crate) pictures: Vec<PictureDraw>,
    /// The text.
    pub(crate) text: Vec<TextSegment>,
}

/// How long each list of a [`DrawList`] was, to find what was drawn
/// after.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DrawMarks {
    /// The decorations drawn before.
    decorations: usize,
    /// The fills drawn before.
    fills: usize,
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
    /// How many fills of its layer were drawn before it, which it is
    /// painted over; those drawn after are painted over it.
    pub(crate) fills_before: usize,
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
    /// How a paragraph wraps, in the proportional font; `None` for one
    /// line on the cell grid's font.
    pub(crate) wrap: Option<Wrapping>,
}

/// How a paragraph's text wraps and aligns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wrapping {
    /// Where each line sits in the width.
    pub(crate) align: TextAlign,
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
    /// The bits of the font size in physical pixels, or `None` for the
    /// segment's.
    pub(crate) size: Option<u32>,
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

    /// The quad moved right by `dx` and down by `dy`, its clip with it.
    fn translated(self, dx: f32, dy: f32) -> Self {
        Self {
            clip: self.clip.map(|clip| offset(clip, dx, dy)),
            rect: offset(self.rect, dx, dy),
            ..self
        }
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
    /// How long each list is now.
    pub(crate) fn marks(&self) -> DrawMarks {
        DrawMarks {
            decorations: self.decorations.len(),
            fills: self.fills.len(),
            paths: self.paths.len(),
            pictures: self.pictures.len(),
            text: self.text.len(),
        }
    }

    /// Keeps what was drawn since `marks` inside `clip`: plain rectangles
    /// are cut to it, other shapes, paths, and text are clipped to it, and
    /// what lies wholly outside is dropped. A picture partly inside is
    /// cropped to it.
    pub(crate) fn clip_since(&mut self, marks: DrawMarks, clip: Rect) {
        let fills_start = marks.fills.min(self.fills.len());
        // How many fills since the marks are kept before each one, to keep
        // every path between the same fills.
        let mut kept_before = vec![0];
        let drained: Vec<Quad> = self.fills.drain(fills_start..).collect();
        for quad in drained {
            self.fills.extend(quad.clipped(clip));
            kept_before.push(self.fills.len() - fills_start);
        }
        let start = marks.paths.min(self.paths.len());
        let paths: Vec<PathDraw> = self
            .paths
            .drain(start..)
            .filter_map(|path| {
                let bounds = match path.clip {
                    Some(own) => own.intersection(&clip)?,
                    None => clip,
                };
                path.bounds.intersection(&bounds)?;
                let since = path.fills_before.saturating_sub(fills_start);
                let kept = kept_before[since.min(kept_before.len() - 1)];
                Some(PathDraw {
                    clip: Some(bounds),
                    fills_before: fills_start + kept,
                    ..path
                })
            })
            .collect();
        self.paths.extend(paths);
        let start = marks.decorations.min(self.decorations.len());
        let clipped: Vec<Quad> = self
            .decorations
            .drain(start..)
            .filter_map(|quad| quad.clipped(clip))
            .collect();
        self.decorations.extend(clipped);
        let start = marks.pictures.min(self.pictures.len());
        let pictures: Vec<PictureDraw> = self
            .pictures
            .drain(start..)
            .filter_map(|picture| {
                let shown = picture.crop.unwrap_or(picture.rect).intersection(&clip)?;
                Some(PictureDraw {
                    crop: Some(shown),
                    ..picture
                })
            })
            .collect();
        self.pictures.extend(pictures);
        let start = marks.text.min(self.text.len());
        let text: Vec<TextSegment> = self
            .text
            .drain(start..)
            .filter_map(|segment| {
                let bounds = match segment.clip {
                    Some(own) => own.intersection(&clip)?,
                    None => clip,
                };
                Some(TextSegment {
                    clip: Some(bounds),
                    ..segment
                })
            })
            .collect();
        self.text.extend(text);
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
            picture.rect = offset(picture.rect, dx, dy);
            picture.crop = picture.crop.map(|crop| offset(crop, dx, dy));
        }
        let start = marks.text.min(self.text.len());
        for segment in &mut self.text[start..] {
            segment.origin = Point::new(segment.origin.x + dx, segment.origin.y + dy);
            segment.clip = segment.clip.map(|clip| offset(clip, dx, dy));
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

    fn quad(rect: Rect) -> Quad {
        Quad::solid(rect, Rgba::BLACK)
    }

    #[test]
    fn test_clipping_cuts_only_what_was_drawn_since_the_marks() {
        let mut list = DrawList::default();
        list.fills.push(quad(Rect::new(0.0, 0.0, 100.0, 100.0)));
        let marks = list.marks();
        list.fills.push(quad(Rect::new(0.0, 0.0, 100.0, 100.0)));
        list.decorations.push(quad(Rect::new(200.0, 0.0, 5.0, 5.0)));
        list.text.push(TextSegment {
            align: SegmentAlign::Start,
            clip: Some(Rect::new(40.0, 40.0, 100.0, 100.0)),
            origin: Point::new(40.0, 40.0),
            runs: Vec::new(),
            wrap: None,
        });
        let clip = Rect::new(10.0, 10.0, 50.0, 50.0);
        list.clip_since(marks, clip);
        assert_eq!(
            list.fills[0].rect,
            Rect::new(0.0, 0.0, 100.0, 100.0),
            "before the marks"
        );
        assert_eq!(list.fills[1].rect, clip);
        assert!(list.decorations.is_empty(), "wholly outside");
        assert_eq!(
            list.text[0].clip,
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
        list.fills.push(rounded);
        list.fills.push(Quad {
            rect: Rect::new(300.0, 0.0, 10.0, 10.0),
            ..rounded
        });
        list.clip_since(DrawMarks::default(), Rect::new(10.0, 10.0, 50.0, 50.0));
        assert_eq!(list.fills.len(), 1, "the one wholly outside is dropped");
        assert_eq!(list.fills[0].rect, rect, "the corners stay where they were");
        assert_eq!(list.fills[0].clip, Some(Rect::new(10.0, 10.0, 50.0, 50.0)));
    }

    #[test]
    fn test_a_blurred_quad_survives_a_clip_its_blur_reaches() {
        let mut list = DrawList::default();
        list.fills.push(Quad {
            blur: 8.0,
            ..quad(Rect::new(0.0, 0.0, 10.0, 10.0))
        });
        list.clip_since(DrawMarks::default(), Rect::new(14.0, 0.0, 10.0, 10.0));
        assert_eq!(list.fills.len(), 1, "its fading edge reaches into the clip");
    }

    fn path(fills_before: usize, bounds: Rect) -> PathDraw {
        PathDraw {
            bounds,
            clip: None,
            fills_before,
            indices: Vec::new(),
            opacity: 1.0,
            paint: Paint::Solid(Rgba::BLACK),
            vertices: Vec::new(),
        }
    }

    #[test]
    fn test_clipping_keeps_a_path_between_the_fills_around_it() {
        let mut list = DrawList::default();
        list.fills.push(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let marks = list.marks();
        list.fills.push(quad(Rect::new(500.0, 0.0, 10.0, 10.0)));
        list.fills.push(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.paths.push(path(3, Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.paths.push(path(3, Rect::new(500.0, 0.0, 10.0, 10.0)));
        list.fills.push(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.clip_since(marks, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert_eq!(list.fills.len(), 3, "the fill outside is dropped");
        assert_eq!(list.paths.len(), 1, "the path outside is dropped");
        assert_eq!(
            list.paths[0].fills_before, 2,
            "still over the fill drawn before it and under the one after"
        );
        assert_eq!(list.paths[0].clip, Some(Rect::new(0.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn test_fading_multiplies_alpha_only_of_what_was_drawn_since() {
        let mut list = DrawList::default();
        list.fills.push(quad(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let marks = list.marks();
        list.fills.push(Quad {
            border: Some(Border {
                color: Rgba::new(0, 0, 0, 200),
                width: 1.0,
            }),
            ..quad(Rect::new(0.0, 0.0, 10.0, 10.0))
        });
        list.paths.push(path(2, Rect::new(0.0, 0.0, 10.0, 10.0)));
        list.text.push(TextSegment {
            align: SegmentAlign::Start,
            clip: None,
            origin: Point::new(0.0, 0.0),
            runs: vec![TextRun {
                color: Rgb::new(1, 2, 3),
                face: Face::default(),
                style: RunStyle::default(),
                text: "a".into(),
            }],
            wrap: None,
        });
        list.fade_since(marks, 0.5);
        list.fade_since(marks, 0.5);
        assert_eq!(list.fills[0].color.a, u8::MAX, "drawn before the marks");
        assert_eq!(list.fills[1].color.a, 64, "nested fades multiply");
        assert_eq!(list.fills[1].border.map(|border| border.color.a), Some(50));
        assert_eq!(list.paths[0].opacity, 0.25);
        assert_eq!(list.text[0].runs[0].style.alpha, Some(64));
    }

    #[test]
    fn test_translating_moves_shapes_paths_and_their_clips() {
        let mut list = DrawList::default();
        list.fills.push(Quad {
            clip: Some(Rect::new(0.0, 0.0, 5.0, 5.0)),
            radius: 2.0,
            ..quad(Rect::new(0.0, 0.0, 10.0, 10.0))
        });
        let mut moved = path(0, Rect::new(0.0, 0.0, 10.0, 10.0));
        moved.vertices.push(PathVertex {
            coverage: 1.0,
            gradient: 0.0,
            position: Point::new(1.0, 2.0),
        });
        list.paths.push(moved);
        list.translate_since(DrawMarks::default(), 3.0, -1.0);
        assert_eq!(list.fills[0].rect, Rect::new(3.0, -1.0, 10.0, 10.0));
        assert_eq!(list.fills[0].clip, Some(Rect::new(3.0, -1.0, 5.0, 5.0)));
        assert_eq!(list.paths[0].vertices[0].position, Point::new(4.0, 1.0));
        assert_eq!(list.paths[0].bounds, Rect::new(3.0, -1.0, 10.0, 10.0));
    }

    #[test]
    fn test_nested_clips_narrow_a_shape_to_both() {
        let mut list = DrawList::default();
        list.fills.push(Quad {
            radius: 4.0,
            ..quad(Rect::new(0.0, 0.0, 100.0, 100.0))
        });
        list.clip_since(DrawMarks::default(), Rect::new(0.0, 0.0, 50.0, 50.0));
        list.clip_since(DrawMarks::default(), Rect::new(25.0, 25.0, 50.0, 50.0));
        assert_eq!(list.fills[0].clip, Some(Rect::new(25.0, 25.0, 25.0, 25.0)));
    }
}

//! [`Frame`]: one frame of one window, what [`Application::render`] and
//! every widget's `draw` draw into, and what they describe to screen
//! readers on the way.
//!
//! [`Application::render`]: crate::Application::render

use std::sync::{Arc, Mutex, PoisonError};

use accesskit::Node;

use super::super::base::box_style::BoxStyle;
use super::super::base::cell::CellSource;
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::paint::{LineCap, Paint, Stroke};
use super::super::base::path::VectorPath;
use super::super::base::styled::{FontFamily, StyledText};
use super::super::base::theme::Theme;
use super::super::base::transform::Transform;
use super::super::render::Fonts;
use super::super::render::draw::{
    DrawList, DrawMarks, Face, PathDraw, PictureDraw, PictureSource, Quad, RunStyle,
};
use super::super::render::font::{CellMetrics, Font};
use super::super::render::grid::lay_out_cells;
use super::super::render::image::{IconSet, Image, ImageDraw, Svg};
use super::super::render::paragraph::{self, Paragraph, ParagraphFont, ParagraphLayout};
use super::super::render::tessellate;
use super::super::render::text_layout::{LineRun, lay_out_line};
use super::super::render::texture::GpuTexture;
use super::access::AccessTree;
use super::focus::FocusId;

// ========================================================================
// Constants
// ========================================================================

/// How far back from an arrow's tip its line stops, as a fraction of the
/// head's length: inside the head, overlapping it enough to leave no seam.
const ARROW_SHAFT_INSIDE_HEAD: f32 = 0.8;

// ========================================================================
// Data Structures
// ========================================================================

/// One frame of one window, built by [`Application::render`](crate::Application::render). Positions
/// are in physical pixels from the window's top left corner. Whatever is
/// drawn, fills (cell backgrounds) are painted first, then text, then
/// decorations (underlines, strikethroughs, thin cursors). Vector paths
/// ([`Frame::fill_path`], [`Frame::stroke_path`]) are fills too, painted in
/// order with boxes: a path drawn between two boxes shows between them.
/// [`Frame::fill_rect`] and [`Frame::fill_rect_over_text`] put a plain
/// rectangle in the fills or the decorations. What is drawn inside [`Frame::overlay`] is painted the same way over
/// all of that, for popups, menus, and tooltips.
#[derive(Debug)]
pub struct Frame {
    /// The accessibility tree described while drawing.
    pub(crate) access: AccessTree,
    /// The window's size in physical pixels.
    bounds: Rect,
    /// The color the whole window is cleared to.
    pub(crate) clear_color: Rgba,
    /// What is drawn over the cleared window.
    pub(crate) draw: DrawList,
    /// The size of the window's font in physical pixels, which
    /// paragraphs are drawn at.
    font_size: f32,
    /// The fonts every window shares, for measuring paragraphs.
    fonts: Arc<Mutex<Fonts>>,
    /// Whether drawing goes to the overlay rather than the base layer.
    in_overlay: bool,
    /// Whether the window is maximized, as the window system last said.
    maximized: bool,
    /// The size of a cell in the window's font.
    metrics: CellMetrics,
    /// What is drawn over the base layer.
    pub(crate) overlay: DrawList,
    /// Whether widgets cut their animations short.
    reduced_motion: bool,
    /// Physical pixels per logical pixel on the window's display.
    scale_factor: f64,
    /// What moves the vector paths drawn now, set by
    /// [`Frame::with_transform`].
    transform: Transform,
}

// ========================================================================
// Frame Implementations
// ========================================================================

impl Frame {
    /// A frame of a window `width` by `height` physical pixels on a display
    /// scaled by `scale_factor`, whose font is `font_size` physical pixels
    /// and draws cells `metrics` in size, measuring text with `fonts`.
    /// Left as is, it clears the window to opaque black.
    pub(crate) fn new(
        (width, height): (u32, u32),
        scale_factor: f64,
        metrics: CellMetrics,
        fonts: Arc<Mutex<Fonts>>,
        font_size: f32,
    ) -> Self {
        Self {
            access: AccessTree::default(),
            bounds: Rect::new(0.0, 0.0, width as f32, height as f32),
            clear_color: Rgba::BLACK,
            draw: DrawList::default(),
            font_size,
            fonts,
            in_overlay: false,
            maximized: false,
            metrics,
            overlay: DrawList::default(),
            reduced_motion: false,
            scale_factor,
            transform: Transform::IDENTITY,
        }
    }

    /// Physical pixels per logical pixel on the window's display: what a
    /// size in logical pixels is multiplied by to draw it.
    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    /// Whether the window is maximized, as the window system last said:
    /// what a title bar shows a restore button for.
    pub fn is_maximized(&self) -> bool {
        self.maximized
    }

    /// Says whether the window is maximized.
    pub(crate) fn set_maximized(&mut self, maximized: bool) {
        self.maximized = maximized;
    }

    /// Whether to cut animations short while drawing: scrolls jump, fades
    /// and spinners hold still. It is what the window's
    /// [`EventCx::reduced_motion`](crate::EventCx::reduced_motion) says:
    /// the user's appearance setting unless the app overrode it with
    /// [`AppContext::set_reduced_motion`](crate::AppContext::set_reduced_motion).
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Says whether to cut animations short.
    pub(crate) fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    /// Describes `node` to screen readers as `id`, drawn in `rect`, inside
    /// the node [`Frame::enter_access`] opened last (or the window); its
    /// bounds and children are set from these calls. See [`AccessAction`](crate::AccessAction)
    /// for how the tree is built.
    pub fn access(&mut self, id: FocusId, rect: Rect, node: Node) {
        self.access.add(id, rect, node);
    }

    /// Describes `node` as [`Frame::access`] does and opens it: the nodes
    /// described until [`Frame::leave_access`] are its children.
    pub fn enter_access(&mut self, id: FocusId, rect: Rect, node: Node) {
        self.access.enter(id, rect, node);
    }

    /// Closes the node [`Frame::enter_access`] opened last.
    pub fn leave_access(&mut self) {
        self.access.leave();
    }

    /// Fills `rect` with `color` (a theme color, or one with alpha, which
    /// blends it over what is under it: a translucent backdrop) in the fill
    /// layer, with boxes and paths and under all pictures and text: a
    /// selection's highlight, a row's background, a scrollbar's thumb.
    /// It is [`Frame::draw_box`] with a plain style, without building one.
    pub fn fill_rect(&mut self, rect: Rect, color: impl Into<Rgba>) {
        self.target().fills.push(Quad::solid(rect, color.into()));
    }

    /// Fills `rect` with `color` in the decoration layer, over all text and
    /// pictures: an underline, a thin caret, a rule over the glyphs it
    /// crosses, which a fill would put under them.
    pub fn fill_rect_over_text(&mut self, rect: Rect, color: impl Into<Rgba>) {
        self.target()
            .decorations
            .push(Quad::solid(rect, color.into()));
    }

    /// Draws `text` on one line in the window's font and in `color`, its
    /// first cell's top left corner at `origin`, each character in the
    /// cells its width gives it (two for a wide character), with no clip:
    /// a glyph, a short label the caller already fitted to its room.
    /// [`Frame::draw_text`] draws styled text cut to a clip.
    pub fn draw_plain_text(&mut self, text: &str, origin: Point, color: Rgb) {
        let run = LineRun {
            color,
            face: Face::default(),
            text,
            underline: false,
        };
        let metrics = self.metrics;
        lay_out_line(&[run], origin, metrics, None, self.target());
    }

    /// The whole window, at 0, 0 and as large as the window, in physical
    /// pixels.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The size of one grid cell in the window's font, in physical pixels:
    /// what [`Frame::draw_cells`] lays cells out by.
    pub fn cell_metrics(&self) -> CellMetrics {
        self.metrics
    }

    /// Clears the whole window to `color`.
    pub fn clear(&mut self, color: Rgba) {
        self.clear_color = color;
    }

    /// Draws `source` as a grid of [`Frame::cell_metrics`] sized cells with
    /// the top left cell at `origin`: each cell's background under the
    /// source's highlights, its glyph in its foreground color, weight, and
    /// slant, its underline and strikethrough, and the source's cursor in
    /// [`Theme::cursor`]. [`CellColor`] values that name no exact color
    /// resolve against `theme`: the default ones to its foreground and
    /// background, an indexed one to its palette's entry.
    ///
    /// [`CellColor`]: crate::CellColor
    pub fn draw_cells(
        &mut self,
        source: &(impl CellSource + ?Sized),
        origin: Point,
        theme: &Theme,
    ) {
        let metrics = self.metrics;
        lay_out_cells(
            source,
            origin,
            metrics,
            RunStyle::default(),
            theme,
            self.target(),
        );
    }

    /// The size of a cell in `font` on the window's display, for laying out
    /// a grid drawn in a font other than the window's (a terminal's text,
    /// larger than the chrome around it) with [`Frame::draw_cells_in`].
    pub fn cell_metrics_of(&self, font: &Font) -> CellMetrics {
        let mut fonts = self.fonts.lock().unwrap_or_else(PoisonError::into_inner);
        fonts.cell_of(font, self.scale_factor)
    }

    /// [`Frame::draw_cells`] in `font` rather than the window's: cells
    /// [`Frame::cell_metrics_of`] `font` in size, glyphs in its family and
    /// size. The window font's ligature setting holds.
    pub fn draw_cells_in(
        &mut self,
        source: &(impl CellSource + ?Sized),
        origin: Point,
        font: &Font,
        theme: &Theme,
    ) {
        let metrics = self.cell_metrics_of(font);
        let run_style = RunStyle {
            alpha: None,
            family: Some(match font.family() {
                Some(family) => FontFamily::Named(family.to_owned()),
                None => FontFamily::Monospace,
            }),
            size: Some(font.physical_size(self.scale_factor).to_bits()),
        };
        lay_out_cells(source, origin, metrics, run_style, theme, self.target());
    }

    /// Draws `text` on one line, its first cell's top left corner at
    /// `origin`, each character in the cells its width gives it (two for a
    /// wide character), its colors resolved through `theme`. Only whole
    /// cells inside `clip` are drawn: a line too long for the room left in
    /// `clip` is cut at a whole character, ending in an ellipsis when
    /// [`StyledText::with_ellipsis`] asked for one, and glyphs are cut at
    /// the clip's top and bottom. Returns how many cells were drawn.
    pub fn draw_text(
        &mut self,
        text: &StyledText,
        origin: Point,
        clip: Rect,
        theme: &Theme,
    ) -> usize {
        let metrics = self.metrics;
        let room = metrics.cols_in(clip.x + clip.width - origin.x);
        let fitted = text.fit(room);
        let runs: Vec<LineRun<'_>> = fitted
            .spans()
            .iter()
            .map(|span| LineRun {
                color: span.color.resolve(theme),
                face: Face {
                    bold: span.bold,
                    italic: span.italic,
                },
                text: &span.text,
                underline: span.underline,
            })
            .collect();
        lay_out_line(&runs, origin, metrics, Some(clip), self.target());
        fitted.width()
    }

    /// How high `paragraph` is when wrapped at `width` pixels in the
    /// proportional font, for laying it out before drawing it.
    pub fn measure_paragraph(&self, paragraph: &Paragraph, width: f32) -> f32 {
        let mut fonts = self.fonts.lock().unwrap_or_else(PoisonError::into_inner);
        paragraph::measure(&mut fonts, self.paragraph_font(), paragraph, width)
    }

    /// How wide `paragraph` is set on one line in the proportional font,
    /// unwrapped: the room a label needs, for laying it out before drawing
    /// it.
    pub fn paragraph_width(&self, paragraph: &Paragraph) -> f32 {
        let mut fonts = self.fonts.lock().unwrap_or_else(PoisonError::into_inner);
        paragraph::measure_line(&mut fonts, self.paragraph_font(), paragraph)
    }

    /// Draws `paragraph` in the proportional font, wrapped to `rect`'s
    /// width from its top left and clipped to it, its colors resolved
    /// through `theme`, links and underlined spans underlined. Returns
    /// where its lines and links landed, to hit test links in.
    pub fn draw_paragraph(
        &mut self,
        paragraph: &Paragraph,
        rect: Rect,
        theme: &Theme,
    ) -> ParagraphLayout {
        let font = self.paragraph_font();
        let shaped = {
            let mut fonts = self.fonts.lock().unwrap_or_else(PoisonError::into_inner);
            paragraph::lay_out(&mut fonts, font, paragraph, rect, theme)
        };
        let target = self.target();
        target.fills.extend(shaped.fills);
        target.text.push(shaped.segment);
        target.decorations.extend(shaped.decorations);
        shaped.layout
    }

    /// Where `paragraph` would land drawn by [`Frame::draw_paragraph`] in
    /// `rect`, without drawing it: for hit testing and placing a caret or
    /// a selection in text laid out but not on screen.
    pub fn paragraph_layout(&self, paragraph: &Paragraph, rect: Rect) -> ParagraphLayout {
        let mut fonts = self.fonts.lock().unwrap_or_else(PoisonError::into_inner);
        paragraph::layout_of(&mut fonts, self.paragraph_font(), paragraph, rect)
    }

    /// The proportional font paragraphs are drawn in: the window font's
    /// size and line height.
    fn paragraph_font(&self) -> ParagraphFont {
        ParagraphFont {
            line_height: self.metrics.height,
            scale_factor: self.scale_factor as f32,
            size: self.font_size,
        }
    }

    /// Draws a box over `rect` in `style`: its shadow, then its fill with
    /// its corners rounded, then its border, all in the fill layer under
    /// pictures and text. A style with no radius, border, or shadow fills
    /// `rect` exactly as a plain fill does.
    pub fn draw_box(&mut self, rect: Rect, style: &BoxStyle) {
        let radius = style.corner_radius;
        if let Some(shadow) = style.shadow {
            let moved = Rect::new(
                rect.x + shadow.offset.x,
                rect.y + shadow.offset.y,
                rect.width,
                rect.height,
            );
            self.target().fills.push(Quad {
                blur: shadow.blur,
                radius,
                ..Quad::solid(moved, shadow.color)
            });
        }
        self.target().fills.push(Quad {
            border: style.border,
            radius,
            ..Quad::solid(rect, style.fill)
        });
    }

    /// Draws `image` stretched over `rect`; [`Image::fit`] gives the rect
    /// that keeps its shape. Pictures are painted over fills and under
    /// text.
    pub fn draw_image(&mut self, image: &Image, rect: Rect) {
        self.picture(PictureSource::Image(image.clone()), rect, None);
    }

    /// Draws `image` over `rect` as [`Frame::draw_image`] does, turned,
    /// mirrored, and sampled as `style` says: an image viewer's rotation,
    /// or sharp pixels at a high zoom.
    pub fn draw_image_as(&mut self, image: &Image, rect: Rect, style: ImageDraw) {
        self.picture(PictureSource::Image(image.clone()), rect, None);
        if let Some(picture) = self.target().pictures.last_mut() {
            picture.style = style;
        }
    }

    /// Draws `texture`, which the app renders into, stretched over `rect`
    /// as [`Frame::draw_image`] draws an image: what the texture holds once
    /// the work submitted before this frame is done. To show part of it,
    /// draw it over the whole rect it would fill inside
    /// [`Frame::clipped`] to the part.
    pub fn draw_texture(&mut self, texture: &GpuTexture, rect: Rect) {
        self.picture(PictureSource::Texture(texture.clone()), rect, None);
    }

    /// Draws `svg` in its own colors, rasterized to fill `rect` (rounded to
    /// whole pixels) keeping its shape, centered in the room left.
    pub fn draw_svg(&mut self, svg: &Svg, rect: Rect) {
        self.picture(PictureSource::Svg(svg.clone()), rect, None);
    }

    /// Draws `icon`, a one-color SVG, as [`Frame::draw_svg`] does but every
    /// pixel in `color` (a theme color), keeping only the icon's shape and
    /// its edges' softness.
    pub fn draw_icon(&mut self, icon: &Svg, rect: Rect, color: Rgb) {
        self.picture(PictureSource::Svg(icon.clone()), rect, Some(color));
    }

    /// Draws the icon named `name` in `icons` over `rect`: tinted in `tint`,
    /// or in its own colors where the set says so. Whether the set holds
    /// it.
    pub fn draw_named_icon(&mut self, icons: &IconSet, name: &str, rect: Rect, tint: Rgb) -> bool {
        let Some(svg) = icons.get(name) else {
            return false;
        };
        match icons.is_colored(name) {
            true => self.draw_svg(svg, rect),
            false => self.draw_icon(svg, rect, tint),
        }
        true
    }

    /// Runs `draw` with what it draws in the current layer kept inside
    /// `clip`: fills and decorations cut to it, text and pictures clipped
    /// to it, and what lies wholly outside dropped. What `draw` puts in the overlay from the base layer is not
    /// clipped, so a popup opened inside a clipped view still shows whole.
    pub fn clipped(&mut self, clip: Rect, draw: impl FnOnce(&mut Frame)) {
        self.then_since(draw, |layer, marks| layer.clip_since(marks, clip));
    }

    /// Runs `draw` with the alpha of everything it draws in the current
    /// layer multiplied by `opacity` (0 to 1): boxes, borders, paths,
    /// pictures, icons, and text, for fades and crossfades. Each shape is
    /// faded on its own, so where two overlap inside, the lower shows
    /// through the upper. Nested calls multiply.
    pub fn with_opacity(&mut self, opacity: f32, draw: impl FnOnce(&mut Frame)) {
        self.then_since(draw, |layer, marks| layer.fade_since(marks, opacity));
    }

    /// Runs `draw` with everything it draws in the current layer moved by
    /// `offset` pixels (right and down), clips included, for a panel
    /// sliding in or a page turning. Only what is drawn moves: a widget
    /// still takes the pointer where it was laid out, so slide content in
    /// and out while it is not meant to be clicked, or lay it out at the
    /// moved place instead. Paths move with it after any
    /// [`Frame::with_transform`].
    pub fn translated(&mut self, offset: Point, draw: impl FnOnce(&mut Frame)) {
        self.then_since(draw, |layer, marks| {
            layer.translate_since(marks, offset.x, offset.y);
        });
    }

    /// Runs `draw` with everything it draws going to the overlay: painted
    /// over the whole base layer, text and decorations included, whenever
    /// it is drawn. Popups, menus, and tooltips draw here.
    pub fn overlay(&mut self, draw: impl FnOnce(&mut Frame)) {
        let was_in_overlay = self.in_overlay;
        self.in_overlay = true;
        draw(self);
        self.in_overlay = was_in_overlay;
    }

    /// Fills the area inside `path` with `paint` (a theme color, or a
    /// [`LinearGradient`](crate::LinearGradient)), its edges antialiased,
    /// moved by the transforms of the [`Frame::with_transform`] calls it
    /// is inside. Like a box, it is a fill: painted over the fills drawn
    /// before it and under those drawn after, under pictures and text,
    /// kept inside a [`Frame::clipped`] clip, and in the overlay inside
    /// [`Frame::overlay`].
    pub fn fill_path(&mut self, path: &VectorPath, paint: impl Into<Paint>) {
        let draw = tessellate::fill(path, paint.into(), self.transform);
        self.push_path(draw);
    }

    /// Draws `path`'s outline as `stroke` says (width, joins, caps,
    /// dashes) with `paint`, as [`Frame::fill_path`] fills. A transform
    /// scales the width and dashes by how much it scales lengths (exactly
    /// for rotations and uniform scales). A one-pixel line is crisp when it
    /// runs along pixel centers (`x.5`), as with any antialiased renderer.
    pub fn stroke_path(&mut self, path: &VectorPath, stroke: &Stroke, paint: impl Into<Paint>) {
        let draw = tessellate::stroke(path, stroke, paint.into(), self.transform);
        self.push_path(draw);
    }

    /// Strokes `path` and fills an arrowhead `head` pixels long at its end
    /// ([`VectorPath::arrowhead`]), both with `paint`: a diagram's or a graph's
    /// edge. The line stops inside the head, so its end never shows past
    /// the tip; its open ends are cut square.
    pub fn draw_arrow(
        &mut self,
        path: &VectorPath,
        stroke: &Stroke,
        paint: impl Into<Paint>,
        head: f32,
    ) {
        let paint = paint.into();
        let Some(arrowhead) = path.arrowhead(head) else {
            return;
        };
        let shaft = path.trimmed_end(head * ARROW_SHAFT_INSIDE_HEAD);
        let stroke = stroke.clone().with_cap(LineCap::Butt);
        self.stroke_path(&shaft, &stroke, paint.clone());
        self.fill_path(&arrowhead, paint);
    }

    /// Runs `draw` with `transform` moving the vector paths it draws,
    /// after any transform already in effect: inside
    /// `with_transform(Transform::translate(x, y), ..)`, a nested
    /// `with_transform(Transform::rotate(a), ..)` rotates a path about the
    /// origin, then moves it by `x, y`. Only paths are moved: boxes, text,
    /// pictures, and clips stay in window pixels.
    pub fn with_transform(&mut self, transform: Transform, draw: impl FnOnce(&mut Frame)) {
        let outer = self.transform;
        self.transform = transform.then(outer);
        draw(self);
        self.transform = outer;
    }

    /// Runs `draw`, then `apply` to the layer it started in with that
    /// layer's lengths from before, so `apply` reaches just what `draw`
    /// added there: what [`Frame::clipped`], [`Frame::with_opacity`], and
    /// [`Frame::translated`] share.
    fn then_since(
        &mut self,
        draw: impl FnOnce(&mut Frame),
        apply: impl FnOnce(&mut DrawList, DrawMarks),
    ) {
        let in_overlay = self.in_overlay;
        let marks = self.target().marks();
        draw(self);
        let layer = match in_overlay {
            true => &mut self.overlay,
            false => &mut self.draw,
        };
        apply(layer, marks);
    }

    /// Adds a tessellated path to the current layer, after its fills.
    fn push_path(&mut self, draw: Option<PathDraw>) {
        let Some(draw) = draw else {
            return;
        };
        let target = self.target();
        let fills_before = target.fills.len();
        target.paths.push(PathDraw {
            fills_before,
            ..draw
        });
    }

    /// Adds a picture of `source` over `rect`, tinted `tint`.
    fn picture(&mut self, source: PictureSource, rect: Rect, tint: Option<Rgb>) {
        self.target().pictures.push(PictureDraw {
            crop: None,
            opacity: 1.0,
            rect,
            source,
            style: ImageDraw::default(),
            tint,
        });
    }

    /// The layer drawing goes to now.
    fn target(&mut self) -> &mut DrawList {
        match self.in_overlay {
            true => &mut self.overlay,
            false => &mut self.draw,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::cell::{Cell, CellBuffer, CellColor, CellStyle};
    use crate::base::color::Rgb;

    #[test]
    fn test_cells_in_another_font_take_its_size_and_carry_it_to_their_glyphs() {
        let mut frame = offscreen::plain_frame(400, 200);
        let big = Font::new(30.0);
        let metrics = frame.cell_metrics_of(&big);
        assert!(metrics.height > frame.cell_metrics().height, "{metrics:?}");
        let mut cells = CellBuffer::new(2, 1);
        let style = CellStyle {
            background: CellColor::Rgb(Rgb::new(1, 2, 3)),
            ..CellStyle::default()
        };
        cells.set(0, 0, Cell::new('a', style));
        cells.set(0, 1, Cell::new('b', style));
        frame.draw_cells_in(&cells, Point::new(0.0, 0.0), &big, &Theme::dark());
        let fill = frame.draw.fills.first().expect("the cells' background");
        assert_eq!(fill.rect.width, 2.0 * metrics.width);
        assert_eq!(fill.rect.height, metrics.height);
        let run = &frame.draw.text[0].runs[0];
        assert_eq!(
            run.style.size,
            Some(big.physical_size(frame.scale_factor()).to_bits())
        );
    }

    #[test]
    fn test_a_font_s_own_line_height_sets_its_cells_height() {
        let frame = offscreen::plain_frame(400, 200);
        let font = Font::new(15.0).with_line_height(4.0 / 3.0);
        let size = font.physical_size(frame.scale_factor());
        let metrics = frame.cell_metrics_of(&font);
        assert_eq!(metrics.height, (size * 4.0 / 3.0).round());
        let shorter = frame.cell_metrics_of(&Font::new(15.0));
        assert_eq!(shorter.height, (size * 1.3).round(), "the default");
    }
}

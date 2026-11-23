//! [`Frame`]: one frame of one window, what [`Application::render`] and
//! every widget's `draw` draw into, and what they describe to screen
//! readers on the way.
//!
//! [`Application::render`]: crate::Application::render

use std::sync::{Arc, Mutex, PoisonError};

use accesskit::Node;

use super::super::base::box_style::{Border, BoxStyle};
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
    DrawList, DrawMarks, DrawPhases, Face, PathDraw, PictureDraw, PictureSource, Quad, RunStyle,
};
use super::super::render::font::{CellMetrics, Font};
use super::super::render::grid::lay_out_cells;
use super::super::render::image::{IconSet, Image, ImageDraw, Svg};
use super::super::render::measurer::ParagraphMeasurer;
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
/// are in physical pixels from the window's top left corner. Everything is
/// painted in the order it is drawn: a box, a path, a picture, or text
/// drawn later paints over whatever was drawn before it, so a highlight
/// filled after an image covers the image, and a label drawn after its
/// background shows on it. Draw a background before what sits on it.
/// Consecutive draws of one kind are painted together, so drawing every
/// background, then every label, costs fewer GPU draw calls than
/// alternating them; within one call (a grid of cells, a line of text, a
/// paragraph) backgrounds are under its glyphs and its underlines over
/// them. What is drawn inside [`Frame::overlay`] is painted over all of
/// the base layer, in its own order, for popups, menus, and tooltips.
#[derive(Debug)]
pub struct Frame {
    /// The accessibility tree described while drawing.
    pub(crate) access: AccessTree,
    /// The window's size in physical pixels.
    bounds: Rect,
    /// The color the whole window is cleared to.
    pub(crate) clear_color: Rgba,
    /// The radius the window's corners are cut out by, in physical pixels;
    /// 0 for square corners.
    corner_radius: f32,
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
    /// The family and size the cell grid's glyphs are drawn in beyond the
    /// window font's, set by [`Frame::with_font`]; the default for the
    /// window font.
    run_style: RunStyle,
    /// Physical pixels per logical pixel on the window's display.
    scale_factor: f64,
    /// What moves everything drawn now, set by [`Frame::with_transform`].
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
            corner_radius: 0.0,
            draw: DrawList::default(),
            font_size,
            fonts,
            in_overlay: false,
            maximized: false,
            metrics,
            overlay: DrawList::default(),
            reduced_motion: false,
            run_style: RunStyle::default(),
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
        let rect = self.transform.bounds_of(rect);
        self.access.add(id, rect, node);
    }

    /// Describes `node` as [`Frame::access`] does and opens it: the nodes
    /// described until [`Frame::leave_access`] are its children.
    pub fn enter_access(&mut self, id: FocusId, rect: Rect, node: Node) {
        let rect = self.transform.bounds_of(rect);
        self.access.enter(id, rect, node);
    }

    /// Closes the node [`Frame::enter_access`] opened last.
    pub fn leave_access(&mut self) {
        self.access.leave();
    }

    /// How many screen reader nodes this frame described so far.
    pub(crate) fn access_len(&self) -> usize {
        self.access.len()
    }

    /// The screen reader nodes this frame described from the `from`th on
    /// that hint labels go over, as [`AccessTree::hint_targets`] lists.
    pub(crate) fn hint_targets(&self, from: usize) -> Vec<(FocusId, Rect, bool)> {
        self.access.hint_targets(from)
    }

    /// Fills `rect` with `color` (a theme color, or one with alpha, which
    /// blends it over what is under it: a translucent backdrop), over what
    /// was drawn before and under what is drawn after: a row's background
    /// drawn before its text, a search hit's highlight drawn after the page
    /// image it marks, a scrollbar's thumb. It is [`Frame::draw_box`] with
    /// a plain style, without building one.
    pub fn fill_rect(&mut self, rect: Rect, color: impl Into<Rgba>) {
        self.target().push_fill(Quad::solid(rect, color.into()));
    }

    /// Fills `rect` with `color`, as [`Frame::fill_rect`] does: over what
    /// was drawn before and under what is drawn after. Drawn after the
    /// text it marks, it is an underline, a thin caret, a rule over the
    /// glyphs it crosses.
    pub fn fill_rect_over_text(&mut self, rect: Rect, color: impl Into<Rgba>) {
        self.target()
            .push_decoration(Quad::solid(rect, color.into()));
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
            underline: None,
        };
        let font = (self.metrics, self.run_style.clone());
        lay_out_line(&[run], origin, font, None, self.target());
    }

    /// The whole window, at 0, 0 and as large as the window, in physical
    /// pixels.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The radius misty cuts the window's corners out by, in physical
    /// pixels, once the frame is drawn: a frameless window's rounded with
    /// [`WindowOptions::with_rounded_corners`](crate::WindowOptions::with_rounded_corners)
    /// on a system that does not round it, while it is not maximized; 0 for
    /// square corners.
    pub fn corner_radius(&self) -> f32 {
        self.corner_radius
    }

    /// Cuts the window's corners out by `radius` physical pixels once the
    /// frame is drawn.
    pub(crate) fn set_corner_radius(&mut self, radius: f32) {
        self.corner_radius = radius.max(0.0);
    }

    /// Draws a hairline of `color` around the whole window, a logical pixel
    /// thick, in the overlay: the outline a frameless window draws for
    /// itself where the system draws none, following the window's rounded
    /// corners ([`Frame::corner_radius`]) when it has them. Call it last, so
    /// nothing drawn after covers it. A maximized window, whose edges meet
    /// the screen's, gets none.
    pub fn draw_window_outline(&mut self, color: impl Into<Rgba>) {
        if self.is_maximized() {
            return;
        }
        let color = color.into();
        let bounds = self.bounds;
        let width = (self.scale_factor as f32).round().max(1.0);
        if self.corner_radius > 0.0 {
            let style = BoxStyle {
                border: Some(Border { color, width }),
                corner_radius: self.corner_radius,
                fill: Rgba::new(0, 0, 0, 0),
                shadow: None,
            };
            self.overlay(|frame| frame.draw_box(bounds, &style));
            return;
        }
        let edges = [
            Rect::new(bounds.x, bounds.y, bounds.width, width),
            Rect::new(
                bounds.x,
                bounds.y + bounds.height - width,
                bounds.width,
                width,
            ),
            Rect::new(bounds.x, bounds.y, width, bounds.height),
            Rect::new(
                bounds.x + bounds.width - width,
                bounds.y,
                width,
                bounds.height,
            ),
        ];
        self.overlay(|frame| {
            for edge in edges {
                frame.fill_rect(edge, color);
            }
        });
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
        let (metrics, style) = (self.metrics, self.run_style.clone());
        let scale_factor = self.scale_factor;
        lay_out_cells(
            source,
            origin,
            metrics,
            style,
            theme,
            scale_factor,
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
    /// size. The window font's ligature and weight settings hold.
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
            inline: None,
            size: Some(font.physical_size(self.scale_factor).to_bits()),
        };
        let scale_factor = self.scale_factor;
        lay_out_cells(
            source,
            origin,
            metrics,
            run_style,
            theme,
            scale_factor,
            self.target(),
        );
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
                underline: span
                    .underline
                    .then(|| span.underline_color.unwrap_or(span.color).resolve(theme)),
            })
            .collect();
        let font = (metrics, self.run_style.clone());
        lay_out_line(&runs, origin, font, Some(clip), self.target());
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
        self.draw_paragraph_over(paragraph, rect, theme, |_| {})
    }

    /// Draws `paragraph` as [`Frame::draw_paragraph`] does, running `under`
    /// between its spans' backgrounds and its glyphs: a search match's or
    /// a selection's fill over the backgrounds and under the text.
    pub(crate) fn draw_paragraph_over(
        &mut self,
        paragraph: &Paragraph,
        rect: Rect,
        theme: &Theme,
        under: impl FnOnce(&mut Frame),
    ) -> ParagraphLayout {
        let font = self.paragraph_font();
        let mut shaped = {
            let mut fonts = self.fonts.lock().unwrap_or_else(PoisonError::into_inner);
            paragraph::lay_out(&mut fonts, font, paragraph, rect, theme)
        };
        // Inside `with_font`, text of the window's size is the scope's size,
        // as it was measured: the renderer would draw the window font's.
        if self.run_style.size.is_some() {
            let size = Some(self.font_size.to_bits());
            for run in shaped
                .segments
                .iter_mut()
                .flat_map(|segment| &mut segment.runs)
            {
                run.style.size = run.style.size.or(size);
            }
        }
        let backgrounds = DrawPhases {
            fills: shaped.fills,
            ..DrawPhases::default()
        };
        backgrounds.append_to(self.target());
        under(self);
        let glyphs = DrawPhases {
            decorations: shaped.decorations,
            text: shaped.segments,
            ..DrawPhases::default()
        };
        glyphs.append_to(self.target());
        shaped.layout
    }

    /// Where `paragraph` would land drawn by [`Frame::draw_paragraph`] in
    /// `rect`, without drawing it: for hit testing and placing a caret or
    /// a selection in text laid out but not on screen.
    pub fn paragraph_layout(&self, paragraph: &Paragraph, rect: Rect) -> ParagraphLayout {
        let mut fonts = self.fonts.lock().unwrap_or_else(PoisonError::into_inner);
        paragraph::layout_of(&mut fonts, self.paragraph_font(), paragraph, rect)
    }

    /// A [`ParagraphMeasurer`] in this window's font and scale factor, to
    /// measure and lay out paragraphs on another thread exactly as
    /// [`Frame::measure_paragraph`] and [`Frame::paragraph_layout`] do
    /// here. Making one is cheap but for the first in a process.
    pub fn paragraph_measurer(&self) -> ParagraphMeasurer {
        ParagraphMeasurer::new(self.paragraph_font())
    }

    /// The proportional font paragraphs are drawn in: the window font's
    /// size and line height.
    pub(crate) fn paragraph_font(&self) -> ParagraphFont {
        ParagraphFont {
            line_height: self.metrics.height,
            scale_factor: self.scale_factor as f32,
            size: self.font_size,
        }
    }

    /// Draws a box over `rect` in `style`: its shadow, then its fill with
    /// its corners rounded, then its border, over what was drawn before
    /// and under what is drawn after. A style with no radius, border, or
    /// shadow fills `rect` exactly as a plain fill does.
    pub fn draw_box(&mut self, rect: Rect, style: &BoxStyle) {
        let radius = style.corner_radius;
        if let Some(shadow) = style.shadow {
            let moved = Rect::new(
                rect.x + shadow.offset.x,
                rect.y + shadow.offset.y,
                rect.width,
                rect.height,
            );
            self.target().push_fill(Quad {
                blur: shadow.blur,
                radius,
                ..Quad::solid(moved, shadow.color)
            });
        }
        self.target().push_fill(Quad {
            border: style.border,
            radius,
            ..Quad::solid(rect, style.fill)
        });
    }

    /// Draws `image` stretched over `rect`; [`Image::fit`] gives the rect
    /// that keeps its shape. Like every draw, it paints over what was
    /// drawn before it and under what is drawn after: fill a highlight
    /// after the image to mark part of it.
    pub fn draw_image(&mut self, image: &Image, rect: Rect) {
        self.draw_image_as(image, rect, ImageDraw::default());
    }

    /// Draws `image` over `rect` as [`Frame::draw_image`] does, turned,
    /// mirrored, and sampled as `style` says: an image viewer's rotation,
    /// or sharp pixels at a high zoom.
    pub fn draw_image_as(&mut self, image: &Image, rect: Rect, style: ImageDraw) {
        self.target().push_picture(PictureDraw {
            style,
            ..picture_of(PictureSource::Image(image.clone()), rect, None)
        });
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
    /// over the whole base layer, whenever it is drawn, and in the order
    /// drawn among the rest of the overlay. Popups, menus, and tooltips
    /// draw here.
    pub fn overlay(&mut self, draw: impl FnOnce(&mut Frame)) {
        let was_in_overlay = self.in_overlay;
        self.in_overlay = true;
        draw(self);
        self.in_overlay = was_in_overlay;
    }

    /// Fills the area inside `path` with `paint` (a theme color, or a
    /// [`LinearGradient`](crate::LinearGradient)), its edges antialiased,
    /// moved by the transforms of the [`Frame::with_transform`] calls it
    /// is inside. Like every draw, it is painted over what was drawn before
    /// it and under what is drawn after, kept inside a [`Frame::clipped`]
    /// clip, and in the overlay inside [`Frame::overlay`].
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

    /// Runs `draw` as if the window's font were `font`: its cells are
    /// [`Frame::cell_metrics_of`] `font` in size, and the text it draws on
    /// them, the proportional text sized by the window font, and every
    /// widget laid out by them take `font`'s size, and the cell grid its
    /// family. For a part of the window set in smaller type than the rest,
    /// such as a status bar beside a terminal's text.
    pub fn with_font(&mut self, font: &Font, draw: impl FnOnce(&mut Frame)) {
        let size = font.physical_size(self.scale_factor);
        let style = RunStyle {
            family: Some(match font.family() {
                Some(family) => FontFamily::Named(family.to_owned()),
                None => FontFamily::Monospace,
            }),
            size: Some(size.to_bits()),
            ..RunStyle::default()
        };
        let metrics = self.cell_metrics_of(font);
        let outer = (
            std::mem::replace(&mut self.metrics, metrics),
            std::mem::replace(&mut self.font_size, size),
            std::mem::replace(&mut self.run_style, style),
        );
        draw(self);
        (self.metrics, self.font_size, self.run_style) = outer;
    }

    /// Runs `draw` with the text it draws, and the cells it lays out,
    /// `scale` times the size they are now, in the same family: for a
    /// view zoomed in on its text, such as a table's cells. The cells are
    /// the current ones scaled, whole pixels high, rather than measured
    /// again.
    pub(crate) fn with_text_scale(&mut self, scale: f32, draw: impl FnOnce(&mut Frame)) {
        let size = self.font_size * scale;
        let metrics = CellMetrics {
            baseline: (self.metrics.baseline * scale).round(),
            height: (self.metrics.height * scale).round().max(1.0),
            width: self.metrics.width * scale,
        };
        let style = RunStyle {
            size: Some(size.to_bits()),
            ..self.run_style.clone()
        };
        let outer = (
            std::mem::replace(&mut self.metrics, metrics),
            std::mem::replace(&mut self.font_size, size),
            std::mem::replace(&mut self.run_style, style),
        );
        draw(self);
        (self.metrics, self.font_size, self.run_style) = outer;
    }

    /// Runs `draw` with `transform` moving everything it draws, in the
    /// base layer and the overlay alike: boxes (fills, borders, rounded
    /// corners, shadows), vector paths, text (cells, labels, styled text,
    /// paragraphs), and pictures (images, SVG documents, icons, app
    /// textures), each turned, scaled, mirrored, and moved about the
    /// origin, after any transform already in effect: inside
    /// `with_transform(Transform::translate(x, y), ..)`, a nested
    /// `with_transform(Transform::rotate(a), ..)` rotates what it draws
    /// about the origin, then moves it by `x, y`. To turn a shape about its
    /// own center `(cx, cy)`, move the center to the origin, turn, and move
    /// it back:
    ///
    /// ```
    /// use misty::{Frame, Point, Rect, Rgb, Transform};
    ///
    /// fn draw_turned_card(frame: &mut Frame, card: Rect, angle: f32) {
    ///     let (cx, cy) = (card.x + card.width / 2.0, card.y + card.height / 2.0);
    ///     let about_center = Transform::translate(-cx, -cy)
    ///         .then(Transform::rotate(angle))
    ///         .then(Transform::translate(cx, cy));
    ///     frame.with_transform(about_center, |frame| {
    ///         frame.fill_rect(card, Rgb::new(40, 40, 48));
    ///         frame.clipped(card, |frame| {
    ///             let origin = Point::new(card.x + 8.0, card.y + 8.0);
    ///             frame.draw_plain_text("Title", origin, Rgb::new(230, 230, 230));
    ///         });
    ///     });
    /// }
    ///
    /// // A press at `pointer` lands on the card when its point in the
    /// // card's own coordinates does.
    /// fn hits_card(pointer: Point, card: Rect, about_center: Transform) -> bool {
    ///     about_center
    ///         .inverse()
    ///         .is_some_and(|inverse| card.contains(inverse.apply(pointer)))
    /// }
    /// ```
    ///
    /// A transform that only moves draws everything exactly as
    /// [`Frame::translated`] would. Under any other, boxes and pictures
    /// keep antialiased edges, borders, radii, and blurs scale with the
    /// box, and text is rasterized at the size it is scaled to, then
    /// turned and sampled smoothly: a little softer than upright text,
    /// whose glyphs land on whole pixels.
    ///
    /// A [`Frame::clipped`] clip set inside the transform is in the
    /// transform's coordinates and turns with what it clips, its edges
    /// antialiased; one set outside stays an upright rectangle in the
    /// window. A clip set between two nested transforms clips what the
    /// inner one draws exactly when the inner one only moves, scales,
    /// mirrors, or turns by quarter turns; when it turns by another angle,
    /// such a clip is widened to its upright bounds in the window, though
    /// paths are still cut along its turned edges.
    ///
    /// Only drawing is moved: a widget still takes the pointer where it was
    /// laid out. To hit test a turned shape, map the pointer back into its
    /// coordinates with the transform's [`Transform::inverse`] and test it
    /// there, as `hits_card` does. Screen readers are given the upright
    /// bounds of the moved rectangle a [`Frame::access`] call inside names.
    pub fn with_transform(&mut self, transform: Transform, draw: impl FnOnce(&mut Frame)) {
        let outer = self.transform;
        self.set_transform(transform.then(outer));
        draw(self);
        self.set_transform(outer);
    }

    /// Makes `transform` the one everything is drawn through from now on.
    fn set_transform(&mut self, transform: Transform) {
        self.transform = transform;
        self.draw.set_transform(transform);
        self.overlay.set_transform(transform);
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

    /// Adds a tessellated path to the current layer.
    fn push_path(&mut self, draw: Option<PathDraw>) {
        if let Some(draw) = draw {
            self.target().push_path(draw);
        }
    }

    /// Adds a picture of `source` over `rect`, tinted `tint`.
    fn picture(&mut self, source: PictureSource, rect: Rect, tint: Option<Rgb>) {
        self.target().push_picture(picture_of(source, rect, tint));
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
// Functions
// ========================================================================

/// A picture of `source` over `rect`, tinted `tint`, as it is, uncropped
/// and unfaded.
fn picture_of(source: PictureSource, rect: Rect, tint: Option<Rgb>) -> PictureDraw {
    PictureDraw {
        crop: None,
        opacity: 1.0,
        rect,
        source,
        style: ImageDraw::default(),
        tint,
        transform: None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    use super::super::access;
    use super::*;
    use crate::app::offscreen;
    use crate::base::cell::{Cell, CellBuffer, CellColor, CellStyle};
    use crate::base::color::Rgb;
    use crate::base::styled::{Span, TextColor};
    use crate::base::theme::ThemeToken;
    use crate::render::paragraph::Paragraph;

    #[test]
    fn test_styled_text_on_cells_is_underlined_in_its_span_s_underline_color() {
        let mut frame = offscreen::plain_frame(400, 200);
        let theme = Theme::dark();
        let text = StyledText::from(
            Span::new("spelt", TextColor::Token(ThemeToken::Foreground))
                .underline_color(ThemeToken::Danger),
        )
        .with(Span::new(" ok", TextColor::Token(ThemeToken::Foreground)).underlined());
        let clip = Rect::new(0.0, 0.0, 400.0, 200.0);
        frame.draw_text(&text, Point::new(0.0, 0.0), clip, &theme);
        let colors: Vec<Rgba> = frame
            .draw
            .decorations()
            .iter()
            .map(|quad| quad.color)
            .collect();
        let error = Rgba::from(TextColor::Token(ThemeToken::Danger).resolve(&theme));
        let foreground = Rgba::from(TextColor::Token(ThemeToken::Foreground).resolve(&theme));
        assert_eq!(colors, [error, foreground]);
    }

    #[test]
    fn test_a_part_drawn_in_a_smaller_font_takes_its_cells_and_text_size_then_returns() {
        let mut frame = offscreen::plain_frame(400, 200);
        let label = Paragraph::plain("Status", TextColor::Token(ThemeToken::Foreground));
        let (cell, width) = (frame.cell_metrics(), frame.paragraph_width(&label));
        let small = Font::new(11.0);
        let mut inside = None;
        frame.with_font(&small, |frame| {
            inside = Some((frame.cell_metrics(), frame.paragraph_width(&label)));
        });
        let (small_cell, small_width) = inside.expect("drawn");
        assert_eq!(small_cell, frame.cell_metrics_of(&small));
        assert!(small_cell.height < cell.height && small_width < width);
        assert_eq!(frame.cell_metrics(), cell, "the window's cells after");
        assert_eq!(frame.paragraph_width(&label), width, "and its text size");
    }

    #[test]
    fn test_a_window_outline_lines_every_edge_unless_the_window_is_maximized() {
        let (fill, line) = (Rgb::new(10, 20, 30), Rgb::new(200, 100, 50));
        let drawn = |maximized: bool| {
            offscreen::render(100, 60, |frame| {
                frame.clear(Rgba::from(fill));
                frame.set_maximized(maximized);
                frame.draw_window_outline(line);
            })
        };
        let Some(outlined) = drawn(false) else {
            return;
        };
        for (x, y) in [(50.0, 0.0), (50.0, 59.0), (0.0, 30.0), (99.0, 30.0)] {
            outlined.assert_pixel(x, y, line, "an edge");
        }
        outlined.assert_pixel(50.0, 30.0, fill, "inside");
        let Some(maximized) = drawn(true) else {
            return;
        };
        maximized.assert_pixel(0.0, 30.0, fill, "no outline when maximized");
    }

    #[test]
    fn test_rounded_corners_are_cut_out_and_the_rest_left_as_drawn() {
        let fill = Rgb::new(200, 100, 50);
        let Some(snapshot) = offscreen::render(100, 60, |frame| {
            frame.clear(Rgba::from(fill));
            frame.set_corner_radius(10.0);
        }) else {
            return;
        };
        let cut = Rgb::new(0, 0, 0);
        for (x, y) in [(0.0, 0.0), (99.0, 0.0), (0.0, 59.0), (99.0, 59.0)] {
            snapshot.assert_pixel(x, y, cut, "a corner");
        }
        for (x, y) in [(50.0, 30.0), (50.0, 0.0), (0.0, 30.0), (10.0, 10.0)] {
            snapshot.assert_pixel(x, y, fill, "inside the rounded window");
        }
    }

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
        let fill = frame.draw.fills().first().expect("the cells' background");
        assert_eq!(fill.rect.width, 2.0 * metrics.width);
        assert_eq!(fill.rect.height, metrics.height);
        let run = &frame.draw.text()[0].runs[0];
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

    /// The side of the square frames the transform tests draw in.
    const SIDE: u32 = 120;

    /// The frames' center, which the transforms turn about.
    const CENTER: f32 = 60.0;

    const BACKGROUND: Rgb = Rgb::new(0, 0, 0);
    const INK: Rgb = Rgb::new(250, 250, 250);

    /// Turns by `radians` about the frames' center.
    fn turn_about_center(radians: f32) -> Transform {
        Transform::translate(-CENTER, -CENTER)
            .then(Transform::rotate(radians))
            .then(Transform::translate(CENTER, CENTER))
    }

    /// The 40 pixel square at the frames' center.
    fn center_square() -> Rect {
        Rect::new(CENTER - 20.0, CENTER - 20.0, 40.0, 40.0)
    }

    /// Checks `snapshot` shows the center square turned an eighth of a
    /// turn, a diamond: filled at its tips, clear at the square's corners.
    fn assert_diamond(snapshot: &offscreen::Snapshot, what: &str) {
        for (x, y) in [(60.0, 36.0), (84.0, 60.0), (60.0, 60.0), (40.0, 60.0)] {
            snapshot.assert_pixel(x, y, INK, what);
        }
        for (x, y) in [(43.0, 43.0), (77.0, 43.0), (43.0, 77.0), (77.0, 77.0)] {
            snapshot.assert_pixel(x, y, BACKGROUND, what);
        }
    }

    /// Whether `a` and `b`, frames `SIDE` pixels a side, match pixel for
    /// pixel.
    fn same_pixels(a: &offscreen::Snapshot, b: &offscreen::Snapshot) -> bool {
        (0..SIDE)
            .all(|y| (0..SIDE).all(|x| a.pixel(x as f32, y as f32) == b.pixel(x as f32, y as f32)))
    }

    #[test]
    fn test_a_turned_box_fills_its_turned_area() {
        let Some(snapshot) = offscreen::render(SIDE, SIDE, |frame| {
            frame.with_transform(turn_about_center(FRAC_PI_4), |frame| {
                frame.fill_rect(center_square(), INK);
            });
        }) else {
            return;
        };
        assert_diamond(&snapshot, "a turned fill");
    }

    #[test]
    fn test_a_clip_set_inside_a_turn_turns_with_what_it_clips() {
        let everything = Rect::new(0.0, 0.0, SIDE as f32, SIDE as f32);
        let square = VectorPath::rect(everything);
        let fill = offscreen::render(SIDE, SIDE, |frame| {
            frame.with_transform(turn_about_center(FRAC_PI_4), |frame| {
                frame.clipped(center_square(), |frame| frame.fill_rect(everything, INK));
            });
        });
        let path = offscreen::render(SIDE, SIDE, |frame| {
            frame.with_transform(turn_about_center(FRAC_PI_4), |frame| {
                frame.clipped(center_square(), |frame| frame.fill_path(&square, INK));
            });
        });
        let (Some(fill), Some(path)) = (fill, path) else {
            return;
        };
        assert_diamond(&fill, "a fill cut by a turned clip");
        assert_diamond(&path, "a path cut by a turned clip");
    }

    #[test]
    fn test_a_clip_set_inside_a_turn_cuts_a_picture_along_its_turned_edges() {
        let everything = Rect::new(0.0, 0.0, SIDE as f32, SIDE as f32);
        let white = Image::from_rgba(1, 1, vec![250, 250, 250, 255]).expect("1x1");
        let Some(snapshot) = offscreen::render(SIDE, SIDE, |frame| {
            frame.with_transform(turn_about_center(FRAC_PI_4), |frame| {
                frame.clipped(center_square(), |frame| {
                    frame.draw_image(&white, everything)
                });
            });
        }) else {
            return;
        };
        assert_diamond(&snapshot, "a picture cut by a turned clip");
    }

    #[test]
    fn test_a_clip_set_inside_a_turn_cuts_turned_text() {
        let mut metrics = None;
        let Some(snapshot) = offscreen::render(SIDE, SIDE, |frame| {
            let cell = frame.cell_metrics();
            metrics = Some(cell);
            let origin = Point::new(CENTER - 4.0 * cell.width, CENTER - cell.height / 2.0);
            // Only the line's left half is kept, which the quarter turn
            // puts above the center.
            let left_half = Rect::new(origin.x, origin.y, 4.0 * cell.width, cell.height);
            frame.with_transform(turn_about_center(FRAC_PI_2), |frame| {
                frame.clipped(left_half, |frame| {
                    frame.draw_plain_text("MMMMMMMM", origin, INK);
                });
            });
        }) else {
            return;
        };
        let cell = metrics.expect("drawn");
        let left = CENTER - cell.height / 2.0;
        let kept_top = CENTER - 4.0 * cell.width;
        let kept = Rect::new(left, kept_top, cell.height, 4.0 * cell.width);
        let cut = Rect::new(left, CENTER + 1.0, cell.height, SIDE as f32 - CENTER - 1.0);
        assert!(snapshot.contains_color(kept, INK), "the kept half is drawn");
        assert!(
            snapshot.is_plain(cut, BACKGROUND),
            "the clipped half is not"
        );
    }

    #[test]
    fn test_a_clip_set_outside_a_turn_stays_upright() {
        let Some(snapshot) = offscreen::render(SIDE, SIDE, |frame| {
            frame.clipped(Rect::new(0.0, 0.0, CENTER, SIDE as f32), |frame| {
                frame.with_transform(turn_about_center(FRAC_PI_4), |frame| {
                    frame.fill_rect(center_square(), INK);
                });
            });
        }) else {
            return;
        };
        snapshot.assert_pixel(59.0, 40.0, INK, "left of the upright clip's edge");
        snapshot.assert_pixel(60.0, 40.0, BACKGROUND, "right of it");
        snapshot.assert_pixel(80.0, 60.0, BACKGROUND, "the diamond's right tip");
    }

    #[test]
    fn test_turned_text_lands_inside_the_turned_box() {
        let mut metrics = None;
        let Some(snapshot) = offscreen::render(SIDE, SIDE, |frame| {
            let cell = frame.cell_metrics();
            metrics = Some(cell);
            let origin = Point::new(CENTER - 2.0 * cell.width, CENTER - cell.height / 2.0);
            frame.with_transform(turn_about_center(FRAC_PI_2), |frame| {
                frame.draw_plain_text("MMMM", origin, INK);
            });
        }) else {
            return;
        };
        let cell = metrics.expect("drawn");
        // A quarter turn about the center swaps the line's width and
        // height: it runs down the middle of the frame.
        let (width, height) = (4.0 * cell.width, cell.height);
        let turned = Rect::new(CENTER - height / 2.0, CENTER - width / 2.0, height, width);
        assert!(
            snapshot.contains_color(turned, INK),
            "glyph pixels in the turned box"
        );
        let left = Rect::new(0.0, 0.0, turned.x - 1.0, SIDE as f32);
        let right_edge = turned.x + turned.width + 1.0;
        let right = Rect::new(right_edge, 0.0, SIDE as f32 - right_edge, SIDE as f32);
        let above = Rect::new(0.0, 0.0, SIDE as f32, turned.y - 1.0);
        for outside in [left, right, above] {
            assert!(
                snapshot.is_plain(outside, BACKGROUND),
                "nothing in {outside:?}"
            );
        }
    }

    #[test]
    fn test_a_flipped_picture_is_mirrored() {
        let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        let image = Image::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).expect("2x1");
        let mirror = Transform::translate(-CENTER, 0.0)
            .then(Transform::scale(-1.0, 1.0))
            .then(Transform::translate(CENTER, 0.0));
        let Some(snapshot) = offscreen::render(SIDE, SIDE, |frame| {
            frame.draw_image(&image, Rect::new(20.0, 10.0, 80.0, 20.0));
            frame.with_transform(mirror, |frame| {
                frame.draw_image(&image, Rect::new(20.0, 50.0, 80.0, 20.0));
            });
        }) else {
            return;
        };
        snapshot.assert_pixel(25.0, 20.0, red, "upright, red on the left");
        snapshot.assert_pixel(95.0, 20.0, blue, "upright, blue on the right");
        snapshot.assert_pixel(25.0, 60.0, blue, "mirrored, blue on the left");
        snapshot.assert_pixel(95.0, 60.0, red, "mirrored, red on the right");
    }

    #[test]
    fn test_nested_transforms_apply_the_inner_one_first() {
        let outer = Transform::scale(2.0, 2.0).then(Transform::translate(40.0, 10.0));
        let Some(snapshot) = offscreen::render(SIDE, SIDE, |frame| {
            frame.with_transform(outer, |frame| {
                frame.with_transform(Transform::rotate(FRAC_PI_2), |frame| {
                    frame.fill_rect(Rect::new(0.0, 0.0, 10.0, 5.0), INK);
                });
            });
        }) else {
            return;
        };
        // Turned to 5 wide and 10 high left of the origin, doubled, then
        // moved: 30 to 40 across and 10 to 30 down.
        snapshot.assert_pixel(35.0, 20.0, INK, "turned, scaled, and moved");
        snapshot.assert_pixel(50.0, 15.0, BACKGROUND, "where it lies unturned");
        snapshot.assert_pixel(25.0, 20.0, BACKGROUND, "left of it");
    }

    #[test]
    fn test_the_identity_and_moves_draw_exactly_as_untransformed() {
        let theme = Theme::dark();
        let style = BoxStyle {
            border: Some(Border {
                color: Rgba::from(INK),
                width: 2.0,
            }),
            corner_radius: 6.0,
            fill: Rgba::new(30, 60, 90, 255),
            shadow: None,
        };
        let image = Image::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).expect("2x1");
        let label = Paragraph::plain("Moved", TextColor::Token(ThemeToken::Foreground));
        let scene = |frame: &mut Frame, at: Point| {
            frame.draw_box(Rect::new(at.x, at.y, 50.0, 30.0), &style);
            frame.draw_plain_text("ab", Point::new(at.x + 4.0, at.y + 4.0), INK);
            frame.draw_image(&image, Rect::new(at.x, at.y + 40.0, 30.0, 10.0));
            frame.draw_paragraph(&label, Rect::new(at.x, at.y + 55.0, 80.0, 30.0), &theme);
        };
        let plain = offscreen::render(SIDE, SIDE, |frame| scene(frame, Point::new(17.0, 9.0)));
        let identity = offscreen::render(SIDE, SIDE, |frame| {
            frame.with_transform(Transform::IDENTITY, |frame| {
                scene(frame, Point::new(17.0, 9.0))
            });
        });
        let moved = offscreen::render(SIDE, SIDE, |frame| {
            frame.with_transform(Transform::translate(7.0, 4.0), |frame| {
                scene(frame, Point::new(10.0, 5.0));
            });
        });
        let (Some(plain), Some(identity), Some(moved)) = (plain, identity, moved) else {
            return;
        };
        assert!(
            same_pixels(&plain, &identity),
            "the identity changes nothing"
        );
        assert!(
            same_pixels(&plain, &moved),
            "a move is drawn as moved coordinates"
        );
    }

    #[test]
    fn test_a_move_is_applied_to_coordinates_and_any_other_transform_carried() {
        let mut frame = offscreen::plain_frame(SIDE, SIDE);
        frame.with_transform(Transform::translate(5.0, 6.0), |frame| {
            frame.fill_rect(Rect::new(1.0, 2.0, 3.0, 4.0), INK);
        });
        frame.with_transform(Transform::rotate(0.5), |frame| {
            frame.fill_rect(Rect::new(1.0, 2.0, 3.0, 4.0), INK);
        });
        frame.fill_rect(Rect::new(1.0, 2.0, 3.0, 4.0), INK);
        let fills = frame.draw.fills();
        assert_eq!(fills[0].rect, Rect::new(6.0, 8.0, 3.0, 4.0));
        assert_eq!(fills[0].transform, None, "drawn by the upright pipeline");
        assert_eq!(
            fills[1].transform.map(|transform| transform.matrix),
            Some(Transform::rotate(0.5))
        );
        assert_eq!(fills[2].transform, None, "the transform ends with the call");
    }

    #[test]
    fn test_screen_readers_get_the_upright_bounds_of_a_turned_rect() {
        let mut frame = offscreen::plain_frame(SIDE, SIDE);
        let id = FocusId::new("turned button");
        frame.with_transform(turn_about_center(FRAC_PI_2), |frame| {
            frame.access(
                id,
                Rect::new(40.0, 50.0, 40.0, 20.0),
                Node::new(accesskit::Role::Button),
            );
        });
        let update = frame.access.tree_update("transformed");
        let node = access::find_node(&update, id).expect("described");
        let bounds = node.bounds().expect("with bounds");
        let expected = [50.0, 40.0, 70.0, 80.0];
        let found = [bounds.x0, bounds.y0, bounds.x1, bounds.y1];
        let close = found
            .iter()
            .zip(expected)
            .all(|(found, expected)| (found - expected).abs() < 1e-3);
        assert!(close, "{found:?}");
    }
}

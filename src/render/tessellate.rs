//! Turns [`VectorPath`]s into antialiased triangles ([`PathDraw`]s) on
//! the CPU: the path is moved into window pixels by the frame's
//! transform, dashed when its stroke asks, tessellated by lyon into
//! triangles that cover it, and given a fringe for antialiasing: every
//! edge on the outline of the triangles is pulled half a pixel inward and
//! a pixel wide strip is added across it whose coverage fades from 1
//! inside to 0 outside, so an edge's pixels blend by how much of them the
//! shape covers without multisampling.

use std::collections::HashMap;

use lyon_tessellation::math::{Point as LyonPoint, point};
use lyon_tessellation::path::iterator::PathIterator;
use lyon_tessellation::path::{Builder, Event, Path as LyonPath};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule as LyonFillRule, FillTessellator, FillVertex,
    LineCap as LyonLineCap, LineJoin as LyonLineJoin, StrokeOptions, StrokeTessellator,
    StrokeVertex, TessellationError, VertexBuffers,
};

use super::super::base::geometry::{Point, Rect};
use super::super::base::paint::{LineCap, LineJoin, Paint, Stroke};
use super::super::base::path::{FillRule, Segment, VectorPath};
use super::super::base::transform::Transform;
use super::draw::{PathDraw, PathVertex};

// ========================================================================
// Constants
// ========================================================================

/// How far, in pixels, a curve's flattened lines may stray from it.
const TOLERANCE: f32 = 0.1;

/// The thinnest a stroke is tessellated, in pixels: a thinner one is drawn
/// this wide and fainter by as much, which keeps its fringes from
/// crossing.
const MIN_STROKE_WIDTH: f32 = 1.0;

/// How far the antialiasing fringe reaches each way from an edge, in
/// pixels: half a pixel, so its ramp is a pixel wide.
const FRINGE_HALF_WIDTH: f32 = 0.5;

/// How far a fringe's corner may reach out at a sharp corner, in fringe
/// half widths, before it is cut short (as a stroke's miter limit does).
const FRINGE_MITER_LIMIT: f32 = 4.0;

/// How sharp a corner a miter join turns before it is drawn as a bevel:
/// the ratio of the miter's length to the line's width, SVG's default.
const MITER_LIMIT: f32 = 4.0;

/// The corners of a triangle.
const CORNERS: usize = 3;

/// The coverage of a vertex inside the path.
const COVERED: f32 = 1.0;

/// The coverage of a fringe's outer vertex.
const CLEAR: f32 = 0.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Triangles in window pixels with a coverage per corner.
#[derive(Debug, Default)]
struct Mesh {
    /// How much of a pixel each vertex covers.
    coverage: Vec<f32>,
    /// Three indices into the vertices per triangle.
    indices: Vec<u32>,
    /// Where each vertex is.
    positions: Vec<Point>,
}

/// A triangle edge with no neighbor across it: part of the outline.
#[derive(Clone, Copy, Debug)]
struct OutlineEdge {
    /// Its first vertex.
    from: u32,
    /// The unit normal pointing out of the triangle.
    normal: (f32, f32),
    /// Its second vertex.
    to: u32,
}

/// Where a walk along a dash pattern is.
#[derive(Debug)]
struct Dasher<'a> {
    /// Whether a dash is being drawn as a subpath now.
    drawing: bool,
    /// Which length of the pattern the walk is in.
    index: usize,
    /// The dash and gap lengths.
    pattern: &'a [f32],
    /// Where a dash starts that has drawn nothing yet: begun only once it
    /// does, so a dash starting where the subpath ends adds nothing.
    pending: Option<LyonPoint>,
    /// How much of the current length is left.
    remaining: f32,
}

/// The outline edges meeting at one vertex, for its fringe's direction.
#[derive(Clone, Copy, Debug, Default)]
struct Corner {
    /// How many outline edges meet here.
    count: u32,
    /// The first edge's outward normal.
    first: (f32, f32),
    /// The second edge's outward normal.
    second: (f32, f32),
    /// The sum of every edge's outward normal.
    sum: (f32, f32),
}

// ========================================================================
// Mesh Implementations
// ========================================================================

impl Mesh {
    /// The triangles lyon made, all wholly covered.
    fn covered(buffers: VertexBuffers<LyonPoint, u32>) -> Self {
        Self {
            coverage: vec![COVERED; buffers.vertices.len()],
            indices: buffers.indices,
            positions: buffers
                .vertices
                .iter()
                .map(|vertex| Point::new(vertex.x, vertex.y))
                .collect(),
        }
    }

    /// Adds the antialiasing fringe: each outline vertex moves half a pixel
    /// in, and a pixel wide strip fading to clear is laid across each
    /// outline edge.
    fn antialias(&mut self) {
        let outline = self.outline();
        let mut corners = vec![Corner::default(); self.positions.len()];
        for edge in &outline {
            for vertex in [edge.from, edge.to] {
                corners[vertex as usize].add(edge.normal);
            }
        }
        let mut outer = vec![0; self.positions.len()];
        for (vertex, corner) in corners.iter().enumerate() {
            if corner.count == 0 {
                continue;
            }
            let (mx, my) = corner.extrusion();
            let at = self.positions[vertex];
            let (dx, dy) = (mx * FRINGE_HALF_WIDTH, my * FRINGE_HALF_WIDTH);
            outer[vertex] = self.vertex(Point::new(at.x + dx, at.y + dy), CLEAR);
            self.positions[vertex] = Point::new(at.x - dx, at.y - dy);
        }
        for edge in &outline {
            let (from, to) = (edge.from, edge.to);
            let (from_out, to_out) = (outer[from as usize], outer[to as usize]);
            self.indices
                .extend([from, to, to_out, from, to_out, from_out]);
        }
    }

    /// The triangle edges no other triangle shares, in triangle order,
    /// with their outward normals: the outline of the covered area.
    fn outline(&self) -> Vec<OutlineEdge> {
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        let (triangles, _) = self.indices.as_chunks::<CORNERS>();
        for triangle in triangles {
            for (a, b) in edges(triangle) {
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let mut outline = Vec::new();
        for triangle in triangles {
            for ((from, to), third) in edges(triangle).into_iter().zip(thirds(triangle)) {
                if uses[&(from.min(to), from.max(to))] != 1 {
                    continue;
                }
                let (a, b, c) = (
                    self.positions[from as usize],
                    self.positions[to as usize],
                    self.positions[third as usize],
                );
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let length = dx.hypot(dy);
                if length <= 0.0 {
                    continue;
                }
                let (mut nx, mut ny) = (-dy / length, dx / length);
                if nx * (c.x - a.x) + ny * (c.y - a.y) > 0.0 {
                    (nx, ny) = (-nx, -ny);
                }
                outline.push(OutlineEdge {
                    from,
                    normal: (nx, ny),
                    to,
                });
            }
        }
        outline
    }

    /// Adds a vertex and returns its index.
    fn vertex(&mut self, position: Point, coverage: f32) -> u32 {
        self.positions.push(position);
        self.coverage.push(coverage);
        // No path comes near `u32::MAX` vertices; lyon indexes in `u32`
        // too and fails first.
        u32::try_from(self.positions.len() - 1).unwrap_or(u32::MAX)
    }

    /// The area the vertices span, or `None` for no vertices.
    fn bounds(&self) -> Option<Rect> {
        let first = self.positions.first()?;
        let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
        for position in &self.positions {
            left = left.min(position.x);
            top = top.min(position.y);
            right = right.max(position.x);
            bottom = bottom.max(position.y);
        }
        Some(Rect::new(left, top, right - left, bottom - top))
    }

    /// The mesh as what the renderer draws, painted with `paint` (whose
    /// gradient points `transform` moves) at `opacity`; `None` when it
    /// holds no triangles.
    fn into_draw(self, paint: Paint, opacity: f32, transform: Transform) -> Option<PathDraw> {
        if self.indices.is_empty() {
            return None;
        }
        let bounds = self.bounds()?;
        let along = gradient_projection(&paint, transform);
        let vertices = self
            .positions
            .iter()
            .zip(&self.coverage)
            .map(|(&position, &coverage)| PathVertex {
                coverage,
                gradient: along(position),
                position,
            })
            .collect();
        Some(PathDraw {
            bounds,
            clip: None,
            indices: self.indices,
            opacity,
            paint,
            vertices,
        })
    }
}

// ========================================================================
// Corner Implementations
// ========================================================================

impl Corner {
    /// Counts an outline edge with outward normal `normal` meeting here.
    fn add(&mut self, normal: (f32, f32)) {
        match self.count {
            0 => self.first = normal,
            1 => self.second = normal,
            _ => {}
        }
        self.count += 1;
        self.sum = (self.sum.0 + normal.0, self.sum.1 + normal.1);
    }

    /// The direction the fringe reaches out from this vertex, scaled so
    /// it reaches a fringe's width from both edges at a corner of two
    /// (a miter), cut short at very sharp corners.
    fn extrusion(&self) -> (f32, f32) {
        if self.count == 2 {
            let (a, b) = (self.first, self.second);
            let cos = a.0 * b.0 + a.1 * b.1;
            let denominator = (COVERED + cos).max(COVERED / FRINGE_MITER_LIMIT);
            return ((a.0 + b.0) / denominator, (a.1 + b.1) / denominator);
        }
        let length = self.sum.0.hypot(self.sum.1);
        match length > 0.0 {
            true => (self.sum.0 / length, self.sum.1 / length),
            false => self.first,
        }
    }
}

// ========================================================================
// Dasher Implementations
// ========================================================================

impl<'a> Dasher<'a> {
    /// A walk `offset` (under the pattern's total) into `pattern`.
    fn new(pattern: &'a [f32], offset: f32) -> Self {
        let mut index = 0;
        let mut left = offset;
        // Ends: `offset` is under the pattern's total, which is above 0.
        while left > 0.0 && left >= pattern[index] {
            left -= pattern[index];
            index = (index + 1) % pattern.len();
        }
        Self {
            drawing: false,
            index,
            pattern,
            pending: None,
            remaining: pattern[index] - left,
        }
    }

    /// Whether the walk is in a dash rather than a gap.
    fn in_dash(&self) -> bool {
        self.index.is_multiple_of(2)
    }

    /// Starts a subpath at `at`: a dash begins there if the walk is in one.
    fn start(&mut self, at: LyonPoint) {
        if self.in_dash() {
            self.pending = Some(at);
        }
    }

    /// Walks the line from `from` to `to`, drawing the dashes on it.
    fn line(&mut self, builder: &mut Builder, from: LyonPoint, to: LyonPoint) {
        let length = (to - from).length();
        if length <= 0.0 {
            return;
        }
        let mut walked = 0.0;
        // Ends: each turn uses up the line or one length of the pattern,
        // and the pattern has a length above 0.
        loop {
            let step = self.remaining.min(length - walked);
            walked += step;
            self.remaining -= step;
            let at = from.lerp(to, walked / length);
            if self.in_dash() && step > 0.0 {
                if let Some(begin) = self.pending.take() {
                    builder.begin(begin);
                    self.drawing = true;
                }
                builder.line_to(at);
            }
            if self.remaining > 0.0 {
                return;
            }
            // A dash of no length is a dot, which a round cap shows.
            if let Some(dot) = self.pending.take() {
                builder.begin(dot);
                self.drawing = true;
            }
            if self.drawing {
                builder.end(false);
                self.drawing = false;
            }
            self.index = (self.index + 1) % self.pattern.len();
            self.remaining = self.pattern[self.index];
            self.start(at);
        }
    }

    /// Ends the dash being drawn, if any, at the end of a subpath.
    fn finish(&mut self, builder: &mut Builder) {
        self.pending = None;
        if self.drawing {
            builder.end(false);
            self.drawing = false;
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `path` filled with `paint`, moved by `transform`, as antialiased
/// triangles; `None` when it covers nothing or cannot be tessellated
/// (logged as a warning).
pub(crate) fn fill(path: &VectorPath, paint: Paint, transform: Transform) -> Option<PathDraw> {
    let device = to_lyon(&path.transformed(transform));
    let options = FillOptions::tolerance(TOLERANCE).with_fill_rule(match path.fill_rule() {
        FillRule::EvenOdd => LyonFillRule::EvenOdd,
        FillRule::NonZero => LyonFillRule::NonZero,
    });
    let mut buffers = VertexBuffers::new();
    let result = FillTessellator::new().tessellate_path(
        &device,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| vertex.position()),
    );
    finish(result, buffers, paint, COVERED, transform)
}

/// `path` stroked by `stroke` with `paint`, moved by `transform` (which
/// scales the width and dashes by how much it scales lengths), as
/// antialiased triangles; `None` when it covers nothing or cannot be
/// tessellated (logged as a warning).
pub(crate) fn stroke(
    path: &VectorPath,
    stroke: &Stroke,
    paint: Paint,
    transform: Transform,
) -> Option<PathDraw> {
    let scale = transform.length_scale();
    let width = stroke.width() * scale;
    if width <= 0.0 || !width.is_finite() {
        return None;
    }
    let mut device = to_lyon(&path.transformed(transform));
    if !stroke.dash().is_empty() {
        let pattern: Vec<f32> = stroke.dash().iter().map(|length| length * scale).collect();
        device = dashed(&device, &pattern, stroke.dash_offset() * scale);
    }
    let cap = match stroke.cap() {
        LineCap::Butt => LyonLineCap::Butt,
        LineCap::Round => LyonLineCap::Round,
        LineCap::Square => LyonLineCap::Square,
    };
    let options = StrokeOptions::tolerance(TOLERANCE)
        .with_line_width(width.max(MIN_STROKE_WIDTH))
        .with_line_cap(cap)
        .with_line_join(match stroke.join() {
            LineJoin::Bevel => LyonLineJoin::Bevel,
            LineJoin::Miter => LyonLineJoin::Miter,
            LineJoin::Round => LyonLineJoin::Round,
        })
        .with_miter_limit(MITER_LIMIT);
    let mut buffers = VertexBuffers::new();
    let result = StrokeTessellator::new().tessellate_path(
        &device,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |vertex: StrokeVertex| vertex.position()),
    );
    let opacity = (width / MIN_STROKE_WIDTH).min(COVERED);
    finish(result, buffers, paint, opacity, transform)
}

/// The triangles lyon made, antialiased and painted, or `None` when it
/// failed (logged as a warning) or made none.
fn finish(
    result: Result<(), TessellationError>,
    buffers: VertexBuffers<LyonPoint, u32>,
    paint: Paint,
    opacity: f32,
    transform: Transform,
) -> Option<PathDraw> {
    if let Err(error) = result {
        log::warn!("path not drawn: {error:?}");
        return None;
    }
    let mut mesh = Mesh::covered(buffers);
    mesh.antialias();
    mesh.into_draw(paint, opacity, transform)
}

/// `path` as a lyon path.
fn to_lyon(path: &VectorPath) -> LyonPath {
    let mut builder = LyonPath::builder();
    let mut open = false;
    let to = |at: Point| point(at.x, at.y);
    for segment in path.segments() {
        match *segment {
            Segment::Close => {
                if open {
                    builder.end(true);
                    open = false;
                }
            }
            Segment::CubicTo(cubic) => {
                builder.cubic_bezier_to(to(cubic.control1), to(cubic.control2), to(cubic.to));
            }
            Segment::LineTo(at) => {
                builder.line_to(to(at));
            }
            Segment::MoveTo(at) => {
                if open {
                    builder.end(false);
                }
                builder.begin(to(at));
                open = true;
            }
            Segment::QuadTo(quad) => {
                builder.quadratic_bezier_to(to(quad.control), to(quad.to));
            }
        }
    }
    if open {
        builder.end(false);
    }
    builder.build()
}

/// `path` cut into dashes by `pattern` (dash and gap lengths alternating,
/// an even count adding up to more than 0), starting `offset` into it,
/// restarted on each subpath: each dash an open subpath of its own.
fn dashed(path: &LyonPath, pattern: &[f32], offset: f32) -> LyonPath {
    let total: f32 = pattern.iter().sum();
    let mut builder = LyonPath::builder();
    let mut dash = Dasher::new(pattern, offset.rem_euclid(total));
    for event in path.iter().flattened(TOLERANCE) {
        match event {
            Event::Begin { at } => {
                dash = Dasher::new(pattern, offset.rem_euclid(total));
                dash.start(at);
            }
            Event::Line { from, to } => dash.line(&mut builder, from, to),
            Event::End { last, first, close } => {
                if close {
                    dash.line(&mut builder, last, first);
                }
                dash.finish(&mut builder);
            }
            Event::Quadratic { .. } | Event::Cubic { .. } => {}
        }
    }
    builder.build()
}

/// How far along `paint`'s gradient each window pixel falls, the
/// gradient's points moved by `transform`: its projection on the gradient
/// line, undone through the transform so a skewed gradient stays true.
fn gradient_projection(paint: &Paint, transform: Transform) -> impl Fn(Point) -> f32 + use<> {
    let line = match paint {
        Paint::LinearGradient(gradient) => transform.inverse().map(|inverse| {
            let (dx, dy) = (
                gradient.end.x - gradient.start.x,
                gradient.end.y - gradient.start.y,
            );
            (inverse, gradient.start, dx, dy, dx * dx + dy * dy)
        }),
        Paint::Solid(_) => None,
    };
    move |position: Point| match line {
        Some((inverse, start, dx, dy, length2)) if length2 > 0.0 => {
            let local = inverse.apply(position);
            ((local.x - start.x) * dx + (local.y - start.y) * dy) / length2
        }
        Some(_) | None => 0.0,
    }
}

/// The three edges of `triangle`, in its winding order.
fn edges(triangle: &[u32; CORNERS]) -> [(u32, u32); 3] {
    [
        (triangle[0], triangle[1]),
        (triangle[1], triangle[2]),
        (triangle[2], triangle[0]),
    ]
}

/// The vertex across from each of [`edges`]' edges.
fn thirds(triangle: &[u32; CORNERS]) -> [u32; 3] {
    [triangle[2], triangle[0], triangle[1]]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::base::color::Rgba;
    use crate::base::paint::{GradientStop, LinearGradient};

    const BLACK: Paint = Paint::Solid(Rgba::BLACK);

    fn filled(path: &VectorPath) -> PathDraw {
        let Some(draw) = fill(path, BLACK, Transform::IDENTITY) else {
            panic!("the path covers something");
        };
        draw
    }

    #[test]
    fn test_a_fill_reaches_half_a_pixel_past_its_edge_with_its_fringe() {
        let draw = filled(&VectorPath::rect(Rect::new(10.0, 10.0, 20.0, 10.0)));
        assert_eq!(draw.bounds, Rect::new(9.5, 9.5, 21.0, 11.0));
        for vertex in &draw.vertices {
            let inside = vertex.position.x > 10.0 && vertex.position.x < 30.0;
            assert_eq!(vertex.coverage == COVERED, inside, "{vertex:?}");
        }
    }

    #[test]
    fn test_only_the_outline_of_a_curved_fill_gets_a_fringe() {
        let center = Point::new(50.0, 50.0);
        let radius = 30.0;
        let mut buffers = VertexBuffers::new();
        let result = FillTessellator::new().tessellate_path(
            &to_lyon(&VectorPath::circle(center, radius)),
            &FillOptions::tolerance(TOLERANCE),
            &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| vertex.position()),
        );
        assert!(result.is_ok());
        let mesh = Mesh::covered(buffers);
        let outline = mesh.outline();
        assert!(!outline.is_empty());
        for edge in outline {
            let (a, b) = (
                mesh.positions[edge.from as usize],
                mesh.positions[edge.to as usize],
            );
            for point in [a, b] {
                let distance = (point.x - center.x).hypot(point.y - center.y);
                assert!((distance - radius).abs() < 0.5, "an inner edge: {distance}");
            }
            let middle = Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
            let outward =
                (middle.x - center.x) * edge.normal.0 + (middle.y - center.y) * edge.normal.1;
            assert!(outward > 0.0, "the normal points out of the circle");
        }
    }

    fn dash_count(pattern: &[f32], offset: f32) -> usize {
        let line = VectorPath::polyline(&[Point::new(0.0, 0.0), Point::new(100.0, 0.0)]);
        dashed(&to_lyon(&line), pattern, offset)
            .iter()
            .filter(|event| matches!(event, Event::Begin { .. }))
            .count()
    }

    #[test]
    fn test_dashes_follow_the_pattern_from_its_offset() {
        assert_eq!(dash_count(&[10.0, 10.0], 0.0), 5);
        assert_eq!(dash_count(&[10.0, 10.0], 5.0), 6, "a half dash first");
        assert_eq!(dash_count(&[10.0, 10.0], 15.0), 5, "a half gap first");
    }

    #[test]
    fn test_a_dash_continues_around_a_corner() {
        let corner = VectorPath::polyline(&[
            Point::new(0.0, 0.0),
            Point::new(15.0, 0.0),
            Point::new(15.0, 15.0),
        ]);
        let dashes = dashed(&to_lyon(&corner), &[20.0, 10.0], 0.0);
        let lines = dashes
            .iter()
            .filter(|event| matches!(event, Event::Line { .. }))
            .count();
        let begins = dashes
            .iter()
            .filter(|event| matches!(event, Event::Begin { .. }))
            .count();
        assert_eq!(begins, 1, "one dash, 20 long, turns the corner");
        assert_eq!(lines, 2);
    }

    #[test]
    fn test_a_hairline_is_drawn_a_pixel_wide_and_fainter() {
        let line = VectorPath::polyline(&[Point::new(0.0, 0.5), Point::new(10.0, 0.5)]);
        let Some(draw) = stroke(&line, &Stroke::new(0.25), BLACK, Transform::IDENTITY) else {
            panic!("a hairline still draws");
        };
        assert_eq!(draw.opacity, 0.25);
        let Some(scaled) = stroke(&line, &Stroke::new(0.25), BLACK, Transform::scale(8.0, 8.0))
        else {
            panic!("a scaled line draws");
        };
        assert_eq!(scaled.opacity, COVERED, "scaled to two pixels wide");
    }

    #[test]
    fn test_a_gradient_turns_with_its_path() {
        let gradient = Paint::LinearGradient(LinearGradient {
            end: Point::new(10.0, 0.0),
            start: Point::new(0.0, 0.0),
            stops: vec![GradientStop {
                color: Rgba::BLACK,
                offset: 0.0,
            }],
        });
        let along = gradient_projection(&gradient, Transform::rotate(FRAC_PI_2));
        assert!((along(Point::new(0.0, 5.0)) - 0.5).abs() < 1e-5);
        assert!(along(Point::new(5.0, 0.0)).abs() < 1e-5);
    }
}

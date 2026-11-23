//! Vector [`VectorPath`]s: outlines of straight lines and Bezier curves, built
//! with a [`VectorPathBuilder`] or a shape helper ([`VectorPath::rect`],
//! [`VectorPath::circle`], [`VectorPath::polyline`]...), that a [`Frame`] fills or
//! strokes. Coordinates are physical pixels, like every other draw, and
//! angles are radians measured clockwise on screen from the positive `x`
//! axis (`y` grows downward).
//!
//! [`Frame`]: crate::Frame

use std::f32::consts::{FRAC_PI_2, PI};
use std::mem;

use super::geometry::{Point, Rect};
use super::transform::Transform;

// ========================================================================
// Constants
// ========================================================================

/// The widest arc one cubic Bezier stands for: a quarter turn, where the
/// approximation strays from the circle by under 0.03% of its radius.
const MAX_ARC_PER_CUBIC: f32 = FRAC_PI_2;

/// The cubic Bezier approximation of a circular arc: its control points
/// sit `4/3 * tan(angle / 4)` radii from the ends, along the tangents.
const ARC_HANDLE_FACTOR: f32 = 4.0 / 3.0;

/// The divisor of an arc's angle inside the tangent of
/// [`ARC_HANDLE_FACTOR`].
const ARC_HANDLE_ANGLE_DIVISOR: f32 = 4.0;

/// Half the width of an arrowhead's base, as a fraction of its length.
const ARROWHEAD_HALF_WIDTH: f32 = 0.4;

/// How many halvings the search for where to cut a curve takes: enough to
/// place the cut within a millionth of the curve.
const TRIM_SEARCH_STEPS: u32 = 24;

/// Half, for midpoints and bisection.
const HALF: f32 = 0.5;

/// The degree-elevation factor turning a quadratic Bezier's control point
/// into a cubic's: each cubic handle sits two thirds of the way to it.
const QUAD_TO_CUBIC: f32 = 2.0 / 3.0;

// ========================================================================
// Data Structures
// ========================================================================

/// An outline to fill or stroke: one or more subpaths, each a run of
/// straight lines and curves from a starting point, open or closed.
/// Build one with [`VectorPath::builder`] or a shape helper such as
/// [`VectorPath::rect`], [`VectorPath::rounded_rect`], [`VectorPath::circle`],
/// [`VectorPath::ellipse`], [`VectorPath::polyline`], or [`VectorPath::polygon`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VectorPath {
    /// Which areas inside the outline a fill covers.
    fill_rule: FillRule,
    /// The outline, in drawing order.
    segments: Vec<Segment>,
}

/// Builds a [`VectorPath`] one call at a time, as a pen would draw it:
/// `VectorPath::builder().move_to(a).line_to(b).line_to(c).close().build()`.
/// Like an HTML canvas, a drawing call with no subpath started begins one
/// (at the point drawn to for a line, at the first control point for a
/// curve, at the arc's start for an arc), and drawing after
/// [`VectorPathBuilder::close`] starts a new subpath where the closed one
/// began.
#[derive(Clone, Debug, Default)]
pub struct VectorPathBuilder {
    /// The pen's position, or `None` before anything is drawn.
    current: Option<Point>,
    /// Whether the last subpath is still open, so drawing continues it.
    open: bool,
    /// The path so far.
    path: VectorPath,
    /// Where the last subpath began, which closing returns to.
    start: Point,
}

/// Which areas a fill covers where a path's outlines overlap or nest.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FillRule {
    /// A point is inside when a ray from it crosses the outline an odd
    /// number of times: a circle inside another, drawn either way round,
    /// makes a ring.
    EvenOdd,
    /// A point is inside when the outline winds around it at all, counting
    /// clockwise turns against counterclockwise ones (the default): a hole
    /// must be drawn the other way round from its outline.
    #[default]
    NonZero,
}

/// One step of a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Segment {
    /// A straight line back to the subpath's start, ending it.
    Close,
    /// A cubic Bezier curve from the pen.
    CubicTo(CubicTo),
    /// A straight line from the pen.
    LineTo(Point),
    /// Lifts the pen and starts a subpath here.
    MoveTo(Point),
    /// A quadratic Bezier curve from the pen.
    QuadTo(QuadTo),
}

/// A cubic Bezier curve from the pen to `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CubicTo {
    /// The control point nearer the start.
    pub(crate) control1: Point,
    /// The control point nearer the end.
    pub(crate) control2: Point,
    /// Where the curve ends.
    pub(crate) to: Point,
}

/// A quadratic Bezier curve from the pen to `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct QuadTo {
    /// The control point.
    pub(crate) control: Point,
    /// Where the curve ends.
    pub(crate) to: Point,
}

// ========================================================================
// VectorPath Implementations
// ========================================================================

impl VectorPath {
    /// A builder for a path drawn call by call.
    pub fn builder() -> VectorPathBuilder {
        VectorPathBuilder::default()
    }

    /// The outline of `rect`, closed, drawn clockwise from its top left.
    pub fn rect(rect: Rect) -> Self {
        let (left, top) = (rect.x, rect.y);
        let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
        Self::polygon(&[
            Point::new(left, top),
            Point::new(right, top),
            Point::new(right, bottom),
            Point::new(left, bottom),
        ])
    }

    /// The outline of `rect` with its corners rounded by `radius` (cut to
    /// half the shorter side), closed, drawn clockwise.
    pub fn rounded_rect(rect: Rect, radius: f32) -> Self {
        let radius = radius.clamp(0.0, rect.width.min(rect.height) * HALF);
        if radius <= 0.0 {
            return Self::rect(rect);
        }
        let (left, top) = (rect.x, rect.y);
        let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
        Self::builder()
            .move_to(Point::new(left + radius, top))
            .line_to(Point::new(right - radius, top))
            .arc(
                Point::new(right - radius, top + radius),
                radius,
                -FRAC_PI_2,
                FRAC_PI_2,
            )
            .line_to(Point::new(right, bottom - radius))
            .arc(
                Point::new(right - radius, bottom - radius),
                radius,
                0.0,
                FRAC_PI_2,
            )
            .line_to(Point::new(left + radius, bottom))
            .arc(
                Point::new(left + radius, bottom - radius),
                radius,
                FRAC_PI_2,
                FRAC_PI_2,
            )
            .line_to(Point::new(left, top + radius))
            .arc(
                Point::new(left + radius, top + radius),
                radius,
                PI,
                FRAC_PI_2,
            )
            .close()
            .build()
    }

    /// A circle around `center`, closed, drawn clockwise.
    pub fn circle(center: Point, radius: f32) -> Self {
        Self::ellipse(center, radius, radius)
    }

    /// An ellipse around `center` reaching `rx` left and right and `ry` up
    /// and down, closed, drawn clockwise.
    pub fn ellipse(center: Point, rx: f32, ry: f32) -> Self {
        let mut builder = Self::builder().move_to(Point::new(center.x + rx, center.y));
        builder.elliptic_arc(center, (rx, ry), 0.0, PI + PI);
        builder.close().build()
    }

    /// Straight lines through `points` in order, left open: a line chart's
    /// series. Empty for no points.
    pub fn polyline(points: &[Point]) -> Self {
        let mut builder = Self::builder();
        for &point in points {
            builder = builder.line_to(point);
        }
        builder.build()
    }

    /// Straight lines through `points` in order and back to the first: a
    /// closed polygon. Empty for no points.
    pub fn polygon(points: &[Point]) -> Self {
        match points.is_empty() {
            true => Self::default(),
            false => {
                let mut path = Self::polyline(points);
                path.segments.push(Segment::Close);
                path
            }
        }
    }

    /// The path filled by `rule` instead of the default
    /// [`FillRule::NonZero`].
    pub fn with_fill_rule(self, rule: FillRule) -> Self {
        Self {
            fill_rule: rule,
            ..self
        }
    }

    /// A filled triangle pointing along the path's end, its tip on the
    /// path's last point and `size` pixels long, for diagrams and graph
    /// edges; `None` for a path that draws nothing.
    /// [`Frame::draw_arrow`] strokes a path with one, ending the line
    /// inside the head so its end never shows past the tip.
    ///
    /// [`Frame::draw_arrow`]: crate::Frame::draw_arrow
    pub fn arrowhead(&self, size: f32) -> Option<Self> {
        let (tip, from) = self.end_direction()?;
        let (dx, dy) = (tip.x - from.x, tip.y - from.y);
        let length = dx.hypot(dy);
        let (ux, uy) = (dx / length, dy / length);
        let base = Point::new(tip.x - ux * size, tip.y - uy * size);
        let half = size * ARROWHEAD_HALF_WIDTH;
        Some(Self::polygon(&[
            tip,
            Point::new(base.x - uy * half, base.y + ux * half),
            Point::new(base.x + uy * half, base.y - ux * half),
        ]))
    }

    /// Which areas inside the outline a fill covers.
    pub(crate) fn fill_rule(&self) -> FillRule {
        self.fill_rule
    }

    /// The outline, in drawing order.
    pub(crate) fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The path with every point moved by `transform`, which moves a
    /// Bezier curve exactly by moving its control points.
    pub(crate) fn transformed(&self, transform: Transform) -> Self {
        let map = |point: Point| transform.apply(point);
        let segments = self
            .segments
            .iter()
            .map(|segment| match *segment {
                Segment::Close => Segment::Close,
                Segment::CubicTo(cubic) => Segment::CubicTo(CubicTo {
                    control1: map(cubic.control1),
                    control2: map(cubic.control2),
                    to: map(cubic.to),
                }),
                Segment::LineTo(to) => Segment::LineTo(map(to)),
                Segment::MoveTo(to) => Segment::MoveTo(map(to)),
                Segment::QuadTo(quad) => Segment::QuadTo(QuadTo {
                    control: map(quad.control),
                    to: map(quad.to),
                }),
            })
            .collect();
        Self {
            fill_rule: self.fill_rule,
            segments,
        }
    }

    /// The path with its last drawn segment cut short so the path ends
    /// `length` pixels (in a straight line) before its end, where an
    /// arrowhead covers it. A segment shorter than `length` shrinks to its
    /// start; a closing segment becomes an open line.
    pub(crate) fn trimmed_end(&self, length: f32) -> Self {
        let Some((index, start)) = self.last_drawn() else {
            return self.clone();
        };
        let end = |segment: &Segment| match *segment {
            Segment::Close => self.subpath_start(index),
            Segment::CubicTo(cubic) => cubic.to,
            Segment::LineTo(to) | Segment::MoveTo(to) => to,
            Segment::QuadTo(quad) => quad.to,
        };
        let segment = &self.segments[index];
        let tip = end(segment);
        let trimmed = match *segment {
            Segment::Close | Segment::LineTo(_) | Segment::MoveTo(_) => {
                Segment::LineTo(trim_line(start, tip, length))
            }
            Segment::CubicTo(cubic) => Segment::CubicTo(trim_cubic(start, cubic, length)),
            Segment::QuadTo(quad) => {
                Segment::CubicTo(trim_cubic(start, quad_as_cubic(start, quad), length))
            }
        };
        let mut segments = self.segments[..index].to_vec();
        segments.push(trimmed);
        Self {
            fill_rule: self.fill_rule,
            segments,
        }
    }

    /// The path's last point and a point behind it along its end tangent,
    /// or `None` when nothing is drawn or the end has no direction.
    fn end_direction(&self) -> Option<(Point, Point)> {
        let (index, start) = self.last_drawn()?;
        let (tip, behind): (Point, &[Point]) = match self.segments[index] {
            Segment::Close => (self.subpath_start(index), &[start]),
            Segment::CubicTo(cubic) => (cubic.to, &[cubic.control2, cubic.control1, start]),
            Segment::LineTo(to) | Segment::MoveTo(to) => (to, &[start]),
            Segment::QuadTo(quad) => (quad.to, &[quad.control, start]),
        };
        let from = behind.iter().copied().find(|&point| point != tip)?;
        Some((tip, from))
    }

    /// The index of the last segment that draws (not a move) and the pen's
    /// position before it.
    fn last_drawn(&self) -> Option<(usize, Point)> {
        let mut pen = Point::default();
        let mut start = Point::default();
        let mut last = None;
        for (index, segment) in self.segments.iter().enumerate() {
            match *segment {
                Segment::Close => {
                    last = Some((index, pen));
                    pen = start;
                }
                Segment::CubicTo(CubicTo { to, .. })
                | Segment::LineTo(to)
                | Segment::QuadTo(QuadTo { to, .. }) => {
                    last = Some((index, pen));
                    pen = to;
                }
                Segment::MoveTo(to) => {
                    pen = to;
                    start = to;
                }
            }
        }
        last
    }

    /// Where the subpath holding the segment at `index` began.
    fn subpath_start(&self, index: usize) -> Point {
        self.segments[..index]
            .iter()
            .rev()
            .find_map(|segment| match *segment {
                Segment::MoveTo(to) => Some(to),
                Segment::Close | Segment::CubicTo(_) | Segment::LineTo(_) | Segment::QuadTo(_) => {
                    None
                }
            })
            .unwrap_or_default()
    }
}

// ========================================================================
// VectorPathBuilder Implementations
// ========================================================================

impl VectorPathBuilder {
    /// Lifts the pen to `point`, starting a new subpath there.
    pub fn move_to(mut self, point: Point) -> Self {
        self.path.segments.push(Segment::MoveTo(point));
        self.current = Some(point);
        self.start = point;
        self.open = true;
        self
    }

    /// Draws a straight line from the pen to `point`.
    pub fn line_to(mut self, point: Point) -> Self {
        if !self.ensure_subpath(point) {
            return self;
        }
        self.path.segments.push(Segment::LineTo(point));
        self.current = Some(point);
        self
    }

    /// Draws a quadratic Bezier curve from the pen to `to`, pulled toward
    /// `control`.
    pub fn quad_to(mut self, control: Point, to: Point) -> Self {
        self.ensure_subpath(control);
        self.path
            .segments
            .push(Segment::QuadTo(QuadTo { control, to }));
        self.current = Some(to);
        self
    }

    /// Draws a cubic Bezier curve from the pen to `to`, leaving toward
    /// `control1` and arriving from `control2`.
    pub fn cubic_to(mut self, control1: Point, control2: Point, to: Point) -> Self {
        self.ensure_subpath(control1);
        self.path.segments.push(Segment::CubicTo(CubicTo {
            control1,
            control2,
            to,
        }));
        self.current = Some(to);
        self
    }

    /// Draws a circular arc around `center` of `radius`, from the angle
    /// `start` turning by `sweep` (both in radians, clockwise on screen
    /// when positive). A straight line joins the pen to the arc's start
    /// first, as on an HTML canvas.
    pub fn arc(mut self, center: Point, radius: f32, start: f32, sweep: f32) -> Self {
        let first = Point::new(
            center.x + radius * start.cos(),
            center.y + radius * start.sin(),
        );
        self = self.line_to(first);
        self.elliptic_arc(center, (radius, radius), start, sweep);
        self
    }

    /// Closes the subpath with a straight line back to where it began.
    /// Does nothing when no subpath is open.
    pub fn close(mut self) -> Self {
        if self.open {
            self.path.segments.push(Segment::Close);
            self.current = Some(self.start);
            self.open = false;
        }
        self
    }

    /// The path drawn.
    pub fn build(self) -> VectorPath {
        self.path
    }

    /// Starts a subpath where none is open: at `at` before anything was
    /// drawn, or where the last closed subpath began. Returns whether one
    /// was open already (a line to `at` then has somewhere to start).
    fn ensure_subpath(&mut self, at: Point) -> bool {
        if self.open {
            return true;
        }
        let start = self.current.unwrap_or(at);
        let continues = self.current.is_some();
        *self = mem::take(self).move_to(start);
        continues
    }

    /// Draws an elliptic arc from the pen (which must sit on its start)
    /// around `center` with radii `(rx, ry)`, as cubic Beziers of at most
    /// a quarter turn each.
    fn elliptic_arc(&mut self, center: Point, (rx, ry): (f32, f32), start: f32, sweep: f32) {
        let pieces = (sweep.abs() / MAX_ARC_PER_CUBIC).ceil().max(1.0);
        let step = sweep / pieces;
        let handle = ARC_HANDLE_FACTOR * (step / ARC_HANDLE_ANGLE_DIVISOR).tan();
        let on_arc = |angle: f32| {
            let (sin, cos) = angle.sin_cos();
            (
                Point::new(center.x + rx * cos, center.y + ry * sin),
                (-rx * sin, ry * cos),
            )
        };
        let mut angle = start;
        // A whole number of pieces, at most a few for any sane sweep.
        for _ in 0..pieces as u32 {
            let (from, (tx0, ty0)) = on_arc(angle);
            angle += step;
            let (to, (tx1, ty1)) = on_arc(angle);
            let control1 = Point::new(from.x + handle * tx0, from.y + handle * ty0);
            let control2 = Point::new(to.x - handle * tx1, to.y - handle * ty1);
            self.path.segments.push(Segment::CubicTo(CubicTo {
                control1,
                control2,
                to,
            }));
            self.current = Some(to);
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The point `length` back from `to` toward `from`, or `from` when the
/// line is shorter.
fn trim_line(from: Point, to: Point, length: f32) -> Point {
    let total = (to.x - from.x).hypot(to.y - from.y);
    if total <= length {
        return from;
    }
    let keep = (total - length) / total;
    lerp(from, to, keep)
}

/// The part of the cubic from `start` that ends `length` (in a straight
/// line) before its end, found by bisection and cut exactly by de
/// Casteljau's construction; the curve shrunk to its start when it never
/// reaches that far from its end.
fn trim_cubic(start: Point, cubic: CubicTo, length: f32) -> CubicTo {
    let end = cubic.to;
    let distance = |t: f32| {
        let point = cubic_point(start, cubic, t);
        (point.x - end.x).hypot(point.y - end.y)
    };
    if distance(0.0) <= length {
        return CubicTo {
            control1: start,
            control2: start,
            to: start,
        };
    }
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..TRIM_SEARCH_STEPS {
        let middle = (low + high) * HALF;
        match distance(middle) > length {
            true => low = middle,
            false => high = middle,
        }
    }
    let t = low;
    let a = lerp(start, cubic.control1, t);
    let b = lerp(cubic.control1, cubic.control2, t);
    let ab = lerp(a, b, t);
    let bc = lerp(b, lerp(cubic.control2, end, t), t);
    CubicTo {
        control1: a,
        control2: ab,
        to: lerp(ab, bc, t),
    }
}

/// The point at `t` along the cubic from `start`.
fn cubic_point(start: Point, cubic: CubicTo, t: f32) -> Point {
    let a = lerp(start, cubic.control1, t);
    let b = lerp(cubic.control1, cubic.control2, t);
    let c = lerp(cubic.control2, cubic.to, t);
    lerp(lerp(a, b, t), lerp(b, c, t), t)
}

/// The quadratic from `start` as the cubic tracing the same curve.
fn quad_as_cubic(start: Point, quad: QuadTo) -> CubicTo {
    CubicTo {
        control1: lerp(start, quad.control, QUAD_TO_CUBIC),
        control2: lerp(quad.to, quad.control, QUAD_TO_CUBIC),
        to: quad.to,
    }
}

/// The point `t` of the way from `a` to `b`.
fn lerp(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
    }

    #[test]
    fn test_a_line_with_no_subpath_starts_one_where_it_lands() {
        let path = VectorPath::polyline(&[Point::new(1.0, 2.0), Point::new(3.0, 4.0)]);
        assert_eq!(
            path.segments(),
            &[
                Segment::MoveTo(Point::new(1.0, 2.0)),
                Segment::LineTo(Point::new(3.0, 4.0)),
            ]
        );
    }

    #[test]
    fn test_drawing_after_close_starts_where_the_closed_subpath_began() {
        let origin = Point::new(5.0, 5.0);
        let path = VectorPath::builder()
            .move_to(origin)
            .line_to(Point::new(9.0, 5.0))
            .close()
            .line_to(Point::new(5.0, 9.0))
            .build();
        assert_eq!(path.segments()[3], Segment::MoveTo(origin));
        assert_eq!(path.segments()[4], Segment::LineTo(Point::new(5.0, 9.0)));
    }

    #[test]
    fn test_arcs_stay_on_their_circle() {
        let center = Point::new(10.0, 10.0);
        let radius = 7.0;
        let path = VectorPath::circle(center, radius);
        let mut pen = Point::default();
        for segment in path.segments() {
            match *segment {
                Segment::CubicTo(cubic) => {
                    let middle = cubic_point(pen, cubic, HALF);
                    let distance = (middle.x - center.x).hypot(middle.y - center.y);
                    assert!((distance - radius).abs() < radius * 1e-3, "{distance}");
                    pen = cubic.to;
                }
                Segment::MoveTo(to) => pen = to,
                Segment::Close | Segment::LineTo(_) | Segment::QuadTo(_) => {}
            }
        }
        assert!(near(pen, Point::new(center.x + radius, center.y)), "closes");
    }

    #[test]
    fn test_an_arc_joins_the_pen_with_a_line() {
        let path = VectorPath::builder()
            .move_to(Point::new(0.0, 0.0))
            .arc(Point::new(10.0, 0.0), 5.0, PI, FRAC_PI_2)
            .build();
        let Segment::LineTo(start) = path.segments()[1] else {
            panic!("a line to the arc's start");
        };
        assert!(near(start, Point::new(5.0, 0.0)), "{start:?}");
        let Segment::CubicTo(cubic) = path.segments()[2] else {
            panic!("the arc follows the line");
        };
        assert!(near(cubic.to, Point::new(10.0, -5.0)), "{:?}", cubic.to);
    }

    #[test]
    fn test_a_rounded_rect_keeps_its_radius_inside_half_the_short_side() {
        let path = VectorPath::rounded_rect(Rect::new(0.0, 0.0, 20.0, 8.0), 100.0);
        assert_eq!(path.segments()[0], Segment::MoveTo(Point::new(4.0, 0.0)));
    }

    #[test]
    fn test_the_arrowhead_points_along_the_curve_end() {
        let path = VectorPath::builder()
            .move_to(Point::new(0.0, 0.0))
            .cubic_to(
                Point::new(50.0, 0.0),
                Point::new(100.0, 50.0),
                Point::new(100.0, 100.0),
            )
            .build();
        let Some(head) = path.arrowhead(10.0) else {
            panic!("a drawn path has an end");
        };
        assert_eq!(
            head.segments()[0],
            Segment::MoveTo(Point::new(100.0, 100.0))
        );
        let Segment::LineTo(corner) = head.segments()[1] else {
            panic!("a triangle");
        };
        assert!(near(corner, Point::new(96.0, 90.0)), "{corner:?}");
        assert_eq!(VectorPath::default().arrowhead(10.0), None);
    }

    #[test]
    fn test_trimming_a_curve_cuts_it_the_distance_short_of_its_end() {
        let start = Point::new(0.0, 0.0);
        let path = VectorPath::builder()
            .move_to(start)
            .quad_to(Point::new(50.0, 80.0), Point::new(100.0, 0.0))
            .build();
        let trimmed = path.trimmed_end(10.0);
        let Segment::CubicTo(cubic) = trimmed.segments()[1] else {
            panic!("a curve stays a curve");
        };
        let gap = (cubic.to.x - 100.0).hypot(cubic.to.y);
        assert!((gap - 10.0).abs() < 1e-3, "{gap}");
        let quad = quad_as_cubic(
            start,
            QuadTo {
                control: Point::new(50.0, 80.0),
                to: Point::new(100.0, 0.0),
            },
        );
        let middle = cubic_point(start, cubic, HALF);
        let off_curve = (0..=1000)
            .map(|step| cubic_point(start, quad, step as f32 / 1000.0))
            .map(|point| (point.x - middle.x).hypot(point.y - middle.y))
            .fold(f32::MAX, f32::min);
        assert!(
            off_curve < 0.2,
            "the cut keeps the curve's shape: {off_curve}"
        );
    }
}

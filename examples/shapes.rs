//! Vector drawing: a bar chart with a line series over it on the left, and
//! on the right two boxes joined by a curved arrow, all drawn with
//! `Frame::fill_path`, `Frame::stroke_path`, and `Frame::draw_arrow` in the
//! theme's colors. The bars fade from the accent color with a gradient,
//! the grid lines are dashed, and the diagram is drawn inside
//! `Frame::with_transform`, turned slightly to show transforms. `t`
//! switches between the dark and the light theme; Esc or the window's
//! close button quits. The boxes are paths too, so they turn with the
//! arrow; boxes drawn with `Frame::draw_box` would not.
//!
//! ```sh
//! cargo run -p misty --example shapes
//! ```

use misty::{
    AppContext, Application, Error, Frame, GradientStop, Input, Key, KeyCode, LineCap, LineJoin,
    LinearGradient, Mods, Point, Rect, Rgba, Stroke, Theme, Transform, VectorPath, WindowId,
    WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty shapes";

/// The values the bars show.
const VALUES: [f32; 7] = [3.0, 5.0, 4.0, 7.0, 6.0, 9.0, 8.0];

/// The values the line series shows, one point over each bar.
const TREND: [f32; 7] = [2.0, 3.5, 4.5, 5.0, 6.5, 7.0, 8.5];

/// The largest value the chart's height stands for.
const CHART_MAX: f32 = 10.0;

/// How many dashed grid lines cross the chart.
const GRID_LINES: usize = 4;

/// The room around each panel, in logical pixels.
const MARGIN: f32 = 32.0;

/// The fraction of each bar's slot the bar fills.
const BAR_FILL: f32 = 0.6;

/// The rounding of a bar's corners, in logical pixels.
const BAR_RADIUS: f32 = 4.0;

/// The width of the chart's axes, in logical pixels.
const AXIS_WIDTH: f32 = 1.0;

/// The width of the line series, in logical pixels.
const LINE_WIDTH: f32 = 2.5;

/// The radius of a line point's dot, in logical pixels.
const DOT_RADIUS: f32 = 4.0;

/// The dash and gap lengths of a grid line, in logical pixels.
const GRID_DASH: [f32; 2] = [4.0, 4.0];

/// The alpha the bars fade to at the bottom.
const BAR_FADE_ALPHA: u8 = 96;

/// The size of a diagram box, in logical pixels.
const NODE_SIZE: (f32, f32) = (120.0, 56.0);

/// How far the arrow's curve bulges, as a fraction of its length.
const ARROW_BEND: f32 = 0.5;

/// The width of the arrow's line, in logical pixels.
const ARROW_WIDTH: f32 = 2.0;

/// The length of the arrowhead, in logical pixels.
const ARROW_HEAD: f32 = 12.0;

/// How far the diagram is turned, in radians.
const DIAGRAM_TURN: f32 = -0.08;

/// Half, for centers.
const HALF: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a chart and a diagram in the current theme.
struct Shapes {
    /// The colors everything is drawn in.
    theme: Theme,
}

// ========================================================================
// Shapes Implementations
// ========================================================================

impl Shapes {
    /// Draws the bar chart with its line series into `area`.
    fn draw_chart(&self, frame: &mut Frame, area: Rect, scale: f32) {
        let theme = &self.theme;
        let bottom = area.y + area.height;
        let y_of = |value: f32| bottom - value / CHART_MAX * area.height;
        let grid = Stroke::new(AXIS_WIDTH * scale).with_dash(&GRID_DASH.map(|l| l * scale), 0.0);
        for line in 1..=GRID_LINES {
            let y = y_of(CHART_MAX * line as f32 / GRID_LINES as f32).round() + HALF;
            let path =
                VectorPath::polyline(&[Point::new(area.x, y), Point::new(area.x + area.width, y)]);
            frame.stroke_path(&path, &grid, theme.border);
        }
        let slot = area.width / VALUES.len() as f32;
        let accent = Rgba::from(theme.accent);
        let faded = Rgba {
            a: BAR_FADE_ALPHA,
            ..accent
        };
        let mut points = Vec::with_capacity(VALUES.len());
        for (index, (&value, &trend)) in VALUES.iter().zip(&TREND).enumerate() {
            let center = area.x + slot * (index as f32 + HALF);
            let width = slot * BAR_FILL;
            let top = y_of(value);
            let bar = Rect::new(center - width * HALF, top, width, bottom - top);
            let gradient = LinearGradient {
                end: Point::new(0.0, bottom),
                start: Point::new(0.0, top),
                stops: vec![
                    GradientStop {
                        color: accent,
                        offset: 0.0,
                    },
                    GradientStop {
                        color: faded,
                        offset: 1.0,
                    },
                ],
            };
            frame.fill_path(&VectorPath::rounded_rect(bar, BAR_RADIUS * scale), gradient);
            points.push(Point::new(center, y_of(trend)));
        }
        let series = Stroke::new(LINE_WIDTH * scale)
            .with_join(LineJoin::Round)
            .with_cap(LineCap::Round);
        frame.stroke_path(&VectorPath::polyline(&points), &series, theme.warning);
        for &point in &points {
            frame.fill_path(
                &VectorPath::circle(point, DOT_RADIUS * scale),
                theme.warning,
            );
        }
        let axes = VectorPath::polyline(&[
            Point::new(area.x + HALF, area.y),
            Point::new(area.x + HALF, bottom - HALF),
            Point::new(area.x + area.width, bottom - HALF),
        ]);
        frame.stroke_path(&axes, &Stroke::new(AXIS_WIDTH * scale), theme.muted);
    }

    /// Draws two boxes joined by a curved arrow into `area`, turned a
    /// little about its center.
    fn draw_diagram(&self, frame: &mut Frame, area: Rect, scale: f32) {
        let theme = &self.theme;
        let (width, height) = (NODE_SIZE.0 * scale, NODE_SIZE.1 * scale);
        let from = Rect::new(area.x, area.y, width, height);
        let to = Rect::new(
            area.x + area.width - width,
            area.y + area.height - height,
            width,
            height,
        );
        let center = Point::new(area.x + area.width * HALF, area.y + area.height * HALF);
        let turn = Transform::translate(-center.x, -center.y)
            .then(Transform::rotate(DIAGRAM_TURN))
            .then(Transform::translate(center.x, center.y));
        let radius = f32::from(theme.shape.corner_radius) * scale;
        frame.with_transform(turn, |frame| {
            for node in [from, to] {
                let outline = VectorPath::rounded_rect(node, radius);
                frame.fill_path(&outline, theme.surface);
                frame.stroke_path(&outline, &Stroke::new(AXIS_WIDTH * scale), theme.border);
            }
            let start = Point::new(from.x + from.width, from.y + from.height * HALF);
            let end = Point::new(to.x + to.width * HALF, to.y);
            let bend = (end.x - start.x) * ARROW_BEND;
            let curve = VectorPath::builder()
                .move_to(start)
                .cubic_to(
                    Point::new(start.x + bend, start.y),
                    Point::new(end.x, end.y - bend),
                    end,
                )
                .build();
            frame.draw_arrow(
                &curve,
                &Stroke::new(ARROW_WIDTH * scale),
                theme.foreground,
                ARROW_HEAD * scale,
            );
        });
    }
}

impl Application for Shapes {
    type Message = ();

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let escape = Input::Key(Key::new(KeyCode::Esc, Mods::NONE));
        let toggle = Input::Key(Key::new(KeyCode::Char('t'), Mods::NONE));
        if *input == escape || *input == Input::CloseRequested {
            cx.exit();
        } else if *input == toggle {
            self.theme = match self.theme == Theme::dark() {
                true => Theme::light(),
                false => Theme::dark(),
            };
            cx.event_cx(window).request_redraw();
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(Rgba::from(self.theme.background));
        let scale = frame.scale_factor() as f32;
        let bounds = frame.bounds();
        let margin = MARGIN * scale;
        let half = bounds.width * HALF;
        let panel = |x: f32| {
            Rect::new(
                x + margin,
                margin,
                half - 2.0 * margin,
                bounds.height - 2.0 * margin,
            )
        };
        self.draw_chart(frame, panel(0.0), scale);
        self.draw_diagram(frame, panel(half), scale);
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), Error> {
    misty::run(
        Shapes {
            theme: Theme::dark(),
        },
        WindowOptions::new(TITLE),
    )
}

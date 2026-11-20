//! Elements that show numbers as shapes, for a dashboard's cards: a
//! [`meter`] filled a share of the way in a color that says how it reads,
//! and a [`sparkline`] tracing a series over time. Each is drawn in the
//! room its place gives it, a row tall unless the app sets its height.

use super::super::base::box_style::BoxStyle;
use super::super::base::geometry::{Point, Rect};
use super::super::base::paint::Stroke;
use super::super::base::path::VectorPath;
use super::super::base::theme::ThemeToken;
use super::custom::canvas;
use super::element::Element;
use super::props::Extent;

// ========================================================================
// Constants
// ========================================================================

/// How tall a meter's bar is, as a share of the row it is drawn in.
const METER_SHARE: f32 = 0.5;

/// How thick a sparkline's line is, in pixels.
const LINE_WIDTH: f32 = 1.5;

/// How strongly a sparkline's area is tinted with its color, of 255.
const AREA_ALPHA: u8 = 48;

/// How many rows tall a sparkline is unless the app says otherwise.
const SPARKLINE_ROWS: f32 = 3.0;

// ========================================================================
// Functions
// ========================================================================

/// A bar `fraction` (0 to 1, held there) of the way along, in the theme's
/// `tone` color over a track: a share used (a core's load, a disk's
/// space), its color saying how it reads (the accent, the warning color
/// past a mark). A row tall, as wide as its place makes it.
pub fn meter<'a, A: 'static>(fraction: f32, tone: ThemeToken) -> Element<'a, A> {
    let bar: Element<'a, A> = canvas(move |frame, area, theme| {
        let (track, filled) = meter_rects(area, fraction);
        let radius = track.height / 2.0;
        let style = |color: ThemeToken| BoxStyle {
            border: None,
            corner_radius: radius,
            fill: theme.color(color).into(),
            shadow: None,
        };
        frame.draw_box(track, &style(ThemeToken::Hover));
        if filled.width > 0.0 {
            frame.draw_box(filled, &style(tone));
        }
    })
    .into();
    bar.height(Extent::Cells(1.0))
}

/// A line through `values` over time, the oldest at the left, scaled so
/// `ceiling` (or the largest value, where that is larger) reaches the top
/// and 0 the bottom, in the theme's `tone` color over a tint of it: a
/// load's history. Three rows tall unless its height is set.
pub fn sparkline<'a, A: 'static>(
    values: &'a [f32],
    ceiling: f32,
    tone: ThemeToken,
) -> Element<'a, A> {
    let line: Element<'a, A> = canvas(move |frame, area, theme| {
        let points = spark_points(area, values, ceiling);
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return;
        };
        let color = theme.color(tone);
        let mut trace = VectorPath::builder().move_to(*first);
        for point in &points[1..] {
            trace = trace.line_to(*point);
        }
        let mut area_path = trace.clone();
        area_path = area_path
            .line_to(Point::new(last.x, area.y + area.height))
            .line_to(Point::new(first.x, area.y + area.height))
            .close();
        frame.fill_path(&area_path.build(), color.with_alpha(AREA_ALPHA));
        frame.stroke_path(&trace.build(), &Stroke::new(LINE_WIDTH), color);
    })
    .into();
    line.height(Extent::Cells(SPARKLINE_ROWS))
}

/// The track of a meter drawn over `area` and its filled part
/// `fraction` of the way along: a bar centered down the area.
fn meter_rects(area: Rect, fraction: f32) -> (Rect, Rect) {
    let height = (area.height * METER_SHARE).round().max(1.0);
    let y = (area.y + (area.height - height) / 2.0).round();
    let track = Rect::new(area.x, y, area.width, height);
    let fraction = fraction.clamp(0.0, 1.0);
    let filled = Rect::new(area.x, y, (area.width * fraction).round(), height);
    (track, filled)
}

/// Where each of `values` falls in `area`, evenly across it, `ceiling`
/// (or the largest value) at the top and 0 at the bottom.
fn spark_points(area: Rect, values: &[f32], ceiling: f32) -> Vec<Point> {
    let top = values.iter().copied().fold(ceiling, f32::max);
    if top <= 0.0 {
        return Vec::new();
    }
    let steps = values.len().saturating_sub(1).max(1) as f32;
    values
        .iter()
        .enumerate()
        .map(|(at, value)| {
            let x = area.x + area.width * at as f32 / steps;
            let share = value.clamp(0.0, top) / top;
            Point::new(x, area.y + area.height * (1.0 - share))
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_meter_fills_its_share_held_to_the_track() {
        let area = Rect::new(10.0, 0.0, 100.0, 20.0);
        let (track, filled) = meter_rects(area, 0.25);
        assert_eq!(track, Rect::new(10.0, 5.0, 100.0, 10.0));
        assert_eq!(filled.width, 25.0);
        assert_eq!(meter_rects(area, 3.0).1.width, 100.0, "held at full");
        assert_eq!(meter_rects(area, -1.0).1.width, 0.0, "and at empty");
    }

    #[test]
    fn test_a_sparkline_spans_its_area_the_ceiling_at_the_top() {
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        let points = spark_points(area, &[0.0, 50.0, 100.0], 100.0);
        assert_eq!(
            points,
            [
                Point::new(0.0, 50.0),
                Point::new(50.0, 25.0),
                Point::new(100.0, 0.0)
            ]
        );
        let over = spark_points(area, &[0.0, 200.0], 100.0);
        assert_eq!(over[1].y, 0.0, "a value past the ceiling raises it");
        assert!(
            spark_points(area, &[0.0, 0.0], 0.0).is_empty(),
            "nothing to scale"
        );
    }
}

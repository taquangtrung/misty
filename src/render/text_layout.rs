//! Lays a line of styled text out on the cell grid: each character starts
//! at the column its predecessors' [`width`](crate::grid::text_width)s add
//! up to, so a wide character takes exactly two cells whatever advance the
//! font that draws it has, and text lines up with a grid drawn from the
//! same string.
//!
//! As in the cell grid, runs of printable ASCII (which the window font
//! advances exactly one cell) are shaped together, and every other
//! character is centered in its own cells.
//!
//! Right-to-left text (Hebrew, Arabic) is laid out in visual order by the
//! Unicode bidirectional algorithm, the line's direction taken from its
//! first strong character: each right-to-left run keeps the cells its
//! characters take, but is shaped as one piece centered across them, so
//! its letters read right to left and Arabic letters join.

use unicode_bidi::{BidiInfo, Level};

use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::text::char_width;
use super::draw::{DrawList, DrawPhases, Face, Quad, RunStyle, SegmentAlign, TextRun, TextSegment};
use super::font::CellMetrics;

// ========================================================================
// Constants
// ========================================================================

/// How far, in pixels, a cell may stick out of the clip and still be
/// drawn, so a rounding error does not drop the last cell.
const CLIP_SLACK: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// A run of text in one resolved color and face, ready to lay out.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineRun<'a> {
    /// The glyphs' color.
    pub(crate) color: Rgb,
    /// The weight and slant.
    pub(crate) face: Face,
    /// The text.
    pub(crate) text: &'a str,
    /// The underline's color, or `None` when the text is not underlined.
    pub(crate) underline: Option<Rgb>,
}

/// What the line is laid out from, in visual order, left to right.
enum Piece<'c, 'a> {
    /// A cluster of left-to-right text, or of text with no direction.
    Cluster(&'c Cluster<'a>),
    /// A right-to-left run, its clusters in logical order.
    RightToLeft(Vec<&'c Cluster<'a>>),
}

/// One character and the zero-width marks after it, in one run's style.
struct Cluster<'a> {
    /// The run the cluster comes from.
    run: LineRun<'a>,
    /// The characters.
    text: String,
    /// The cells it takes: one or two.
    width: usize,
}

/// The line being laid out and where its output goes.
struct LineLayout {
    /// The area drawn inside, or `None` for the whole window.
    clip: Option<Rect>,
    /// The output, gathered by kind so the line's underlines paint over
    /// all of its glyphs.
    draw: DrawPhases,
    /// The size of a cell.
    metrics: CellMetrics,
    /// The top left corner of the line's first cell.
    origin: Point,
    /// The segment of ASCII glyphs being extended, and the column it
    /// continues at.
    segment: Option<(TextSegment, usize)>,
    /// The family and size every glyph is drawn in, beyond the window
    /// font's; the default for the window font itself.
    style: RunStyle,
    /// The underline being extended: its color, left edge, and right edge.
    underline: Option<(Rgb, f32, f32)>,
}

// ========================================================================
// LineLayout Implementations
// ========================================================================

impl LineLayout {
    /// Adds `cluster` at column `col`, or drops it when it is outside the
    /// clip.
    fn place(&mut self, cluster: &Cluster<'_>, col: usize) {
        let left = self.origin.x + col as f32 * self.metrics.width;
        let width = cluster.width as f32 * self.metrics.width;
        if !self.is_inside_clip(left, width) {
            self.flush_segment();
            self.flush_underline();
            return;
        }
        let run = cluster.run;
        match run.underline {
            Some(color) => self.extend_underline(color, left, width),
            None => self.flush_underline(),
        }
        let origin = Point::new(left, self.origin.y);
        let text_run = TextRun {
            color: run.color,
            face: run.face,
            style: self.style.clone(),
            text: cluster.text.clone(),
        };
        if !is_one_cell_ascii(cluster) {
            self.flush_segment();
            self.draw.text.push(TextSegment {
                align: SegmentAlign::Center(width),
                clip: self.clip,
                origin,
                runs: vec![text_run],
                transform: None,
                wrap: None,
            });
            return;
        }
        if let Some((segment, next_col)) = &mut self.segment
            && *next_col == col
        {
            *next_col += 1;
            match segment.runs.last_mut() {
                Some(last) if last.color == run.color && last.face == run.face => {
                    last.text.push_str(&cluster.text);
                }
                Some(_) | None => segment.runs.push(text_run),
            }
            return;
        }
        self.flush_segment();
        let segment = TextSegment {
            align: SegmentAlign::Start,
            clip: self.clip,
            origin,
            runs: vec![text_run],
            transform: None,
            wrap: None,
        };
        self.segment = Some((segment, col + 1));
    }

    /// Adds the right-to-left run `group` (in logical order) at column
    /// `col` as one segment centered across its cells, or drops it when it
    /// is not wholly inside the clip.
    fn place_right_to_left(&mut self, group: &[&Cluster<'_>], col: usize) {
        let cells: usize = group.iter().map(|cluster| cluster.width).sum();
        let left = self.origin.x + col as f32 * self.metrics.width;
        let width = cells as f32 * self.metrics.width;
        self.flush_segment();
        if !self.is_inside_clip(left, width) {
            self.flush_underline();
            return;
        }
        // The underline follows the clusters as they show, right to left.
        let mut cluster_left = left;
        for cluster in group.iter().rev() {
            let cluster_width = cluster.width as f32 * self.metrics.width;
            match cluster.run.underline {
                Some(color) => self.extend_underline(color, cluster_left, cluster_width),
                None => self.flush_underline(),
            }
            cluster_left += cluster_width;
        }
        let mut runs: Vec<TextRun> = Vec::new();
        for cluster in group {
            let run = cluster.run;
            match runs.last_mut() {
                Some(last) if last.color == run.color && last.face == run.face => {
                    last.text.push_str(&cluster.text);
                }
                Some(_) | None => runs.push(TextRun {
                    color: run.color,
                    face: run.face,
                    style: self.style.clone(),
                    text: cluster.text.clone(),
                }),
            }
        }
        self.draw.text.push(TextSegment {
            align: SegmentAlign::Center(width),
            clip: self.clip,
            origin: Point::new(left, self.origin.y),
            runs,
            transform: None,
            wrap: None,
        });
    }

    /// Whether a cell span from `left`, `width` pixels wide, is inside the
    /// clip horizontally.
    fn is_inside_clip(&self, left: f32, width: f32) -> bool {
        self.clip.is_none_or(|clip| {
            left >= clip.x - CLIP_SLACK && left + width <= clip.x + clip.width + CLIP_SLACK
        })
    }

    /// Grows the underline by the cells from `left`, or starts a new one
    /// when its color differs or it does not touch the last.
    fn extend_underline(&mut self, color: Rgb, left: f32, width: f32) {
        match &mut self.underline {
            Some((line_color, _, right)) if *line_color == color && *right == left => {
                *right = left + width;
            }
            Some(_) | None => {
                self.flush_underline();
                self.underline = Some((color, left, left + width));
            }
        }
    }

    /// Emits the pending underline.
    fn flush_underline(&mut self) {
        if let Some((color, left, right)) = self.underline.take() {
            let top = self.origin.y + self.metrics.underline_top();
            self.draw.decorations.push(Quad::solid(
                Rect::new(left, top, right - left, self.metrics.line_thickness()),
                Rgba::from(color),
            ));
        }
    }

    /// Emits the pending ASCII segment.
    fn flush_segment(&mut self) {
        if let Some((segment, _)) = self.segment.take() {
            self.draw.text.push(segment);
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Adds `runs` to `draw` as one line, its first cell's top left corner at
/// `origin`, in cells of `metrics` and glyphs of `style`'s family and size
/// (the window font's for the default); cells not wholly inside `clip`
/// horizontally are left out, and glyphs are cut at its edges. Returns how
/// many cells the line takes, drawn or not.
pub(crate) fn lay_out_line(
    runs: &[LineRun<'_>],
    origin: Point,
    (metrics, style): (CellMetrics, RunStyle),
    clip: Option<Rect>,
    draw: &mut DrawList,
) -> usize {
    let mut layout = LineLayout {
        clip,
        draw: DrawPhases::default(),
        metrics,
        origin,
        segment: None,
        style,
        underline: None,
    };
    let clusters = clusters_of(runs);
    let mut col = 0;
    for piece in visual_order(&clusters) {
        match piece {
            Piece::Cluster(cluster) => {
                layout.place(cluster, col);
                col += cluster.width;
            }
            Piece::RightToLeft(group) => {
                layout.place_right_to_left(&group, col);
                col += group.iter().map(|cluster| cluster.width).sum::<usize>();
            }
        }
    }
    layout.flush_segment();
    layout.flush_underline();
    layout.draw.append_to(draw);
    col
}

/// The clusters of `runs`, in order: each character that takes a cell,
/// with the zero-width marks after it in the same run. Control characters
/// and marks with nothing before them are dropped.
fn clusters_of<'a>(runs: &[LineRun<'a>]) -> Vec<Cluster<'a>> {
    let mut clusters: Vec<Cluster<'a>> = Vec::new();
    for run in runs {
        let mut starts_run = true;
        for c in run.text.chars().filter(|c| !c.is_control()) {
            let width = char_width(c);
            match (width, clusters.last_mut()) {
                (0, Some(last)) if !starts_run => last.text.push(c),
                (0, Some(_) | None) => {}
                (_, Some(_) | None) => {
                    starts_run = false;
                    clusters.push(Cluster {
                        run: *run,
                        text: c.to_string(),
                        width,
                    });
                }
            }
        }
    }
    clusters
}

/// The pieces `clusters` (in logical order) show as, left to right: the
/// clusters themselves when the line has no right-to-left text, else
/// reordered by the bidirectional algorithm, each right-to-left run kept
/// together.
fn visual_order<'c, 'a>(clusters: &'c [Cluster<'a>]) -> Vec<Piece<'c, 'a>> {
    let text: String = clusters
        .iter()
        .map(|cluster| cluster.text.as_str())
        .collect();
    let info = BidiInfo::new(&text, None);
    let Some(paragraph) = info.paragraphs.first().filter(|_| info.has_rtl()) else {
        return clusters.iter().map(Piece::Cluster).collect();
    };
    let byte_levels = info.reordered_levels(paragraph, paragraph.range.clone());
    let mut start = 0;
    let levels: Vec<Level> = clusters
        .iter()
        .map(|cluster| {
            let level = byte_levels[start];
            start += cluster.text.len();
            level
        })
        .collect();
    let order = visual_indices(&levels);
    let mut pieces = Vec::new();
    let mut index = 0;
    while index < order.len() {
        let level = levels[order[index]];
        if !level.is_rtl() {
            pieces.push(Piece::Cluster(&clusters[order[index]]));
            index += 1;
            continue;
        }
        let end = (index..order.len())
            .find(|next| levels[order[*next]] != level)
            .unwrap_or(order.len());
        // Shown right to left, so the run's logical order is reversed.
        let group = order[index..end]
            .iter()
            .rev()
            .map(|cluster| &clusters[*cluster])
            .collect();
        pieces.push(Piece::RightToLeft(group));
        index = end;
    }
    pieces
}

/// The indices of items at `levels` in visual order, left to right: from
/// the highest level down to the lowest odd one, every stretch at that
/// level or above is reversed (rule L2 of the bidirectional algorithm).
fn visual_indices(levels: &[Level]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let numbers = || levels.iter().map(Level::number);
    let highest = numbers().max().unwrap_or(0);
    let lowest_odd = numbers().min().map_or(1, |lowest| lowest | 1);
    for level in (lowest_odd..=highest).rev() {
        let mut index = 0;
        while index < order.len() {
            if levels[order[index]].number() < level {
                index += 1;
                continue;
            }
            let start = index;
            while index < order.len() && levels[order[index]].number() >= level {
                index += 1;
            }
            order[start..index].reverse();
        }
    }
    order
}

/// Whether `cluster` is one printable ASCII character, which the window
/// font draws exactly one cell wide.
fn is_one_cell_ascii(cluster: &Cluster<'_>) -> bool {
    let mut chars = cluster.text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => c == ' ' || c.is_ascii_graphic(),
        (Some(_), Some(_)) | (None, _) => false,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const METRICS: CellMetrics = CellMetrics {
        baseline: 15.0,
        height: 20.0,
        width: 10.0,
    };

    const RED: Rgb = Rgb::new(0xff, 0, 0);

    const BLUE: Rgb = Rgb::new(0, 0, 0xff);

    fn run(text: &str, color: Rgb) -> LineRun<'_> {
        LineRun {
            color,
            face: Face::default(),
            text,
            underline: None,
        }
    }

    fn lay_out(runs: &[LineRun<'_>], clip: Option<Rect>) -> (DrawList, usize) {
        let mut draw = DrawList::default();
        let cells = lay_out_line(
            runs,
            Point::new(5.0, 7.0),
            (METRICS, RunStyle::default()),
            clip,
            &mut draw,
        );
        (draw, cells)
    }

    /// Each segment's left edge, alignment, and text.
    fn segments(draw: &DrawList) -> Vec<(f32, SegmentAlign, String)> {
        draw.text()
            .iter()
            .map(|segment| {
                let text = segment.runs.iter().map(|run| run.text.as_str()).collect();
                (segment.origin.x, segment.align, text)
            })
            .collect()
    }

    #[test]
    fn test_a_wide_character_takes_exactly_two_cells_and_the_rest_follows_it() {
        let (draw, cells) = lay_out(&[run("a漢b", RED)], None);
        assert_eq!(cells, 4);
        assert_eq!(
            segments(&draw),
            vec![
                (5.0, SegmentAlign::Start, "a".to_owned()),
                (15.0, SegmentAlign::Center(20.0), "漢".to_owned()),
                (35.0, SegmentAlign::Start, "b".to_owned()),
            ]
        );
    }

    #[test]
    fn test_a_color_change_starts_a_run_but_not_a_segment() {
        let (draw, _) = lay_out(&[run("ab", RED), run(" c", BLUE)], None);
        assert_eq!(draw.text().len(), 1);
        let colors: Vec<Rgb> = draw.text()[0].runs.iter().map(|run| run.color).collect();
        assert_eq!(colors, vec![RED, BLUE]);
    }

    #[test]
    fn test_cells_outside_the_clip_are_left_out_and_glyphs_carry_the_clip() {
        // Cells start at 5, 15, 25, 35: the wide one spans 15 to 35 and
        // sticks out of a clip ending at 30.
        let clip = Rect::new(5.0, 0.0, 25.0, 100.0);
        let (draw, cells) = lay_out(&[run("a漢b", RED)], Some(clip));
        assert_eq!(cells, 4, "the advance counts clipped cells");
        assert_eq!(
            segments(&draw),
            vec![(5.0, SegmentAlign::Start, "a".to_owned())]
        );
        assert_eq!(draw.text()[0].clip, Some(clip));
    }

    #[test]
    fn test_a_right_to_left_word_is_shaped_as_one_piece_across_its_cells() {
        let (draw, cells) = lay_out(&[run("שלום", RED)], None);
        assert_eq!(cells, 4);
        assert_eq!(
            segments(&draw),
            vec![(5.0, SegmentAlign::Center(40.0), "שלום".to_owned())],
            "in logical order, for the shaper to set right to left"
        );
    }

    #[test]
    fn test_mixed_lines_take_their_direction_from_the_first_strong_character() {
        let (draw, _) = lay_out(&[run("ab שלום cd", RED)], None);
        assert_eq!(
            segments(&draw),
            vec![
                (5.0, SegmentAlign::Start, "ab ".to_owned()),
                (35.0, SegmentAlign::Center(40.0), "שלום".to_owned()),
                (75.0, SegmentAlign::Start, " cd".to_owned()),
            ]
        );
        // A Hebrew line puts its English word at the left end, as it shows.
        let (draw, cells) = lay_out(&[run("שלום ab", RED)], None);
        assert_eq!(cells, 7);
        assert_eq!(
            segments(&draw),
            vec![
                (5.0, SegmentAlign::Start, "ab".to_owned()),
                (25.0, SegmentAlign::Center(50.0), "שלום ".to_owned()),
            ]
        );
    }

    #[test]
    fn test_numbers_in_right_to_left_text_still_read_left_to_right() {
        let (draw, _) = lay_out(&[run("עמוד 12 ב", RED)], None);
        let texts: Vec<String> = segments(&draw)
            .into_iter()
            .map(|(_, _, text)| text)
            .collect();
        assert_eq!(
            texts,
            [" ב", "12", "עמוד "],
            "right to left, the number kept whole"
        );
        let (draw, _) = lay_out(&[run("של", RED), run("ום", BLUE)], None);
        let colors: Vec<Rgb> = draw.text()[0].runs.iter().map(|run| run.color).collect();
        assert_eq!(colors, [RED, BLUE], "each run keeps its color in the piece");
    }

    #[test]
    fn test_combining_marks_stay_with_their_character_and_take_no_cell() {
        let (draw, cells) = lay_out(&[run("e\u{301}x", RED)], None);
        assert_eq!(cells, 2);
        assert_eq!(
            segments(&draw),
            vec![
                (5.0, SegmentAlign::Center(10.0), "e\u{301}".to_owned()),
                (15.0, SegmentAlign::Start, "x".to_owned()),
            ]
        );
    }

    #[test]
    fn test_an_underline_spans_its_runs_cells_as_one_line() {
        let underlined = LineRun {
            underline: Some(RED),
            ..run("a漢", RED)
        };
        let (draw, _) = lay_out(&[underlined, run("b", RED)], None);
        assert_eq!(draw.decorations().len(), 1);
        let line = draw.decorations()[0].rect;
        assert_eq!((line.x, line.width), (5.0, 30.0));
        assert_eq!(line.y, 7.0 + METRICS.underline_top());
    }

    #[test]
    fn test_an_underline_in_its_own_color_is_drawn_in_it_and_breaks_where_it_changes() {
        let blue_line = LineRun {
            underline: Some(BLUE),
            ..run("ab", RED)
        };
        let red_line = LineRun {
            underline: Some(RED),
            ..run("c", RED)
        };
        let (draw, _) = lay_out(&[blue_line, red_line], None);
        let lines: Vec<(f32, Rgba)> = draw
            .decorations()
            .iter()
            .map(|quad| (quad.rect.width, quad.color))
            .collect();
        assert_eq!(lines, [(20.0, Rgba::from(BLUE)), (10.0, Rgba::from(RED))]);
    }
}

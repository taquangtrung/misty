//! Labels in a widget's chrome, set in the theme's [`ChromeText`]: on the
//! cell grid in the monospace font, or in the proportional font. A widget
//! measures its labels with [`label_width`] to lay itself out and draws
//! them with [`draw_label`], so one theme switch changes every label. A
//! label of several styles (a bold title, a muted note after it) is a
//! [`StyledText`], measured with [`styled_label_width`] (or in whole cells
//! with [`styled_label_cells`], for a widget laid out on the cell grid) and
//! drawn with [`draw_styled_label`], which also cuts it to its room.

use super::super::app::frame::Frame;
use super::super::base::cell::CellColor;
use super::super::base::color::Rgb;
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::text;
use super::super::base::theme::{ChromeText, Theme, ThemeToken};
use super::super::render::paragraph::Paragraph;

// ========================================================================
// Constants
// ========================================================================

/// How much of the text color a keycap's chip mixes into the raised color.
const KEYCAP_SHARE: f32 = 0.14;

/// The room either side of a keycap's key, in cells.
const KEYCAP_PAD_CELLS: f32 = 0.5;

/// What the room left for a label is given beyond its clip, in pixels. A
/// label set to end at the clip's edge (right-aligned: its width taken off
/// the edge) finds a hair less room than its width once the subtraction
/// rounds, far enough from the window's origin, and would lose its last
/// character. The clip still bounds what is drawn.
const ROOM_SLACK: f32 = 0.5;

// ========================================================================
// Functions
// ========================================================================

/// How wide `text` is as a label in `theme`'s chrome font, in pixels.
pub fn label_width(frame: &Frame, theme: &Theme, text: &str) -> f32 {
    match theme.chrome_text {
        ChromeText::Cells => text::width(text) as f32 * frame.cell_metrics().width,
        ChromeText::Proportional => frame.paragraph_width(&paragraph(text, theme.foreground)),
    }
}

/// Draws `text` as a label in `color` (a theme color) on the line a cell
/// tall whose top left corner is `origin`, in `theme`'s chrome font.
pub fn draw_label(frame: &mut Frame, theme: &Theme, text: &str, origin: Point, color: Rgb) {
    match theme.chrome_text {
        ChromeText::Cells => frame.draw_plain_text(text, origin, color),
        ChromeText::Proportional => {
            let label = paragraph(text, color);
            // A pixel to spare, so rounding never wraps the label's last word.
            let width = frame.paragraph_width(&label).ceil() + 1.0;
            let rect = Rect::new(origin.x, origin.y, width, frame.cell_metrics().height);
            frame.draw_paragraph(&label, rect, theme);
        }
    }
}

/// How wide `text` is as a label in `theme`'s chrome font, in pixels, its
/// spans' weights and slants counted in the proportional font.
pub fn styled_label_width(frame: &Frame, theme: &Theme, text: &StyledText) -> f32 {
    match theme.chrome_text {
        ChromeText::Cells => text.width() as f32 * frame.cell_metrics().width,
        ChromeText::Proportional => frame.paragraph_width(&styled_paragraph(text)),
    }
}

/// How many whole cells `text` takes as a label in `theme`'s chrome font,
/// for a widget laid out on the cell grid: its cells there, or as many as
/// hold it in the proportional font.
pub(crate) fn styled_label_cells(frame: &Frame, theme: &Theme, text: &StyledText) -> usize {
    match theme.chrome_text {
        ChromeText::Cells => text.width(),
        ChromeText::Proportional => {
            let cells = styled_label_width(frame, theme, text) / frame.cell_metrics().width;
            // Rounding up is the point: the label must fit its cells.
            cells.ceil() as usize
        }
    }
}

/// `text` cut to fit `room` pixels as a label in `theme`'s chrome font, at
/// a whole character, ending in an ellipsis when
/// [`StyledText::with_ellipsis`] asked for one; whole when it fits. Measure
/// the cut label with [`styled_label_width`] to place it, as an aligned
/// label is.
pub fn fit_label(frame: &Frame, theme: &Theme, text: &StyledText, room: f32) -> StyledText {
    match theme.chrome_text {
        ChromeText::Cells => text.fit(frame.cell_metrics().cols_in(room)),
        ChromeText::Proportional => fit_proportional(frame, text, room),
    }
}

/// `text` cut to fit `room` pixels in the proportional font, as
/// [`fit_label`] cuts it there, whatever the theme's chrome font: for text
/// that is in the proportional font on its own account, such as a
/// proportional [`TextField`](super::text_field::TextField)'s placeholder.
pub(crate) fn fit_proportional(frame: &Frame, text: &StyledText, room: f32) -> StyledText {
    // Drop a character at a time until the text fits, as the cell grid's
    // cut does, so the ellipsis lands where it would there.
    let mut cells = text.width();
    loop {
        let fitted = text.fit(cells);
        if cells == 0 || frame.paragraph_width(&styled_paragraph(&fitted)) <= room {
            return fitted;
        }
        cells -= 1;
    }
}

/// Draws `text` as a label on the line a cell tall whose top left corner
/// is `origin`, in `theme`'s chrome font, its colors resolved through
/// `theme`. Only what fits inside `clip` is drawn: a label too long for
/// the room left is cut as [`fit_label`] cuts it. Returns how wide the
/// drawn label is, in pixels.
pub fn draw_styled_label(
    frame: &mut Frame,
    theme: &Theme,
    text: &StyledText,
    origin: Point,
    clip: Rect,
) -> f32 {
    match theme.chrome_text {
        ChromeText::Cells => {
            let room_clip = Rect::new(clip.x, clip.y, clip.width + ROOM_SLACK, clip.height);
            let cells = frame.draw_text(text, origin, room_clip, theme);
            cells as f32 * frame.cell_metrics().width
        }
        ChromeText::Proportional => draw_proportional_label(frame, theme, text, origin, clip),
    }
}

/// Draws `text` as [`draw_styled_label`] draws it in the proportional
/// font, whatever the theme's chrome font: for text that is in the
/// proportional font on its own account, such as a proportional
/// [`view::text`](crate::view::text). Returns how wide the drawn label is,
/// in pixels.
pub(crate) fn draw_proportional_label(
    frame: &mut Frame,
    theme: &Theme,
    text: &StyledText,
    origin: Point,
    clip: Rect,
) -> f32 {
    let room = (clip.x + clip.width - origin.x + ROOM_SLACK).max(0.0);
    let label = styled_paragraph(&fit_proportional(frame, text, room));
    let width = frame.paragraph_width(&label);
    // A pixel to spare, so rounding never wraps the label's last word.
    let rect = Rect::new(
        origin.x,
        origin.y,
        width.ceil() + 1.0,
        frame.cell_metrics().height,
    );
    frame.clipped(clip, |frame| {
        frame.draw_paragraph(&label, rect, theme);
    });
    width.min(room)
}

/// `text`'s spans, styles kept, as a paragraph.
pub(crate) fn styled_paragraph(text: &StyledText) -> Paragraph {
    text.spans()
        .iter()
        .fold(Paragraph::new(), |paragraph, span| {
            paragraph.with(span.clone())
        })
}

/// Draws `key` (`F3`, `C-x`) as a keycap on the line a cell tall whose top
/// left corner is `origin`, as a hints line names the keys it offers: the
/// key stressed as the theme stresses words, in its chrome font on a chip a shade off the raised
/// color, rounded as the theme rounds controls, kept inside `clip`. Returns
/// how wide the chip is, in pixels.
pub fn draw_keycap(frame: &mut Frame, theme: &Theme, key: &str, origin: Point, clip: Rect) -> f32 {
    let color = TextColor::Token(ThemeToken::Foreground);
    draw_keycap_in(frame, theme, key, color, origin, clip)
}

/// Draws `key` as a keycap as [`draw_keycap`] does, its key in `color`: a
/// key another command claims too in the danger color. Returns how wide
/// the chip is, in pixels.
pub(crate) fn draw_keycap_in(
    frame: &mut Frame,
    theme: &Theme,
    key: &str,
    color: TextColor,
    origin: Point,
    clip: Rect,
) -> f32 {
    let metrics = frame.cell_metrics();
    let pad = (KEYCAP_PAD_CELLS * metrics.width).round();
    let key = Span::new(key, color);
    let label = StyledText::from(key.emphasized(theme.emphasis));
    let width = (styled_label_width(frame, theme, &label) + 2.0 * pad).ceil();
    let chip = Rect::new(origin.x, origin.y, width, metrics.height);
    let fill = theme.raised.mix(theme.foreground, KEYCAP_SHARE);
    let style = theme.shape.highlight(fill, frame.scale_factor());
    frame.clipped(clip, |frame| frame.draw_box(chip, &style));
    draw_styled_label(
        frame,
        theme,
        &label,
        Point::new(origin.x + pad, origin.y),
        clip,
    );
    width
}

/// `text` as a one-span paragraph in `color`.
fn paragraph(text: &str, color: Rgb) -> Paragraph {
    Paragraph::plain(text, TextColor::Palette(CellColor::Rgb(color)))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::theme::ThemeToken;

    #[test]
    fn test_a_label_set_to_end_at_its_clips_edge_keeps_its_last_character() {
        for chrome_text in [ChromeText::Cells, ChromeText::Proportional] {
            let theme = theme(chrome_text);
            let mut frame = offscreen::plain_frame(900, 100);
            let count = StyledText::plain("1/1", TextColor::Token(ThemeToken::Muted));
            let width = styled_label_width(&frame, &theme, &count);
            let clip = Rect::new(13.0, 0.0, 860.0, 20.0);
            // The room a hair short of the width, as rounding leaves it when
            // the label's x is the clip's edge less its width.
            let x = clip.x + clip.width - width + 0.0001;
            draw_styled_label(&mut frame, &theme, &count, Point::new(x, 0.0), clip);
            let drawn: String = crate::testing::drawn_text(&frame).concat();
            assert_eq!(drawn, "1/1", "{chrome_text:?}");
        }
    }

    fn theme(chrome_text: ChromeText) -> Theme {
        Theme {
            chrome_text,
            ..Theme::dark()
        }
    }

    #[test]
    fn test_cell_labels_take_their_cells_and_proportional_ones_their_shapes() {
        let frame = offscreen::plain_frame(400, 100);
        let cells = theme(ChromeText::Cells);
        let cell = frame.cell_metrics().width;
        assert_eq!(label_width(&frame, &cells, "iiii"), 4.0 * cell);
        assert_eq!(label_width(&frame, &cells, "WWWW"), 4.0 * cell);
        let proportional = theme(ChromeText::Proportional);
        let narrow = label_width(&frame, &proportional, "iiii");
        let wide = label_width(&frame, &proportional, "WWWW");
        assert!(0.0 < narrow && narrow < wide, "{narrow} against {wide}");
    }

    #[test]
    fn test_a_proportional_label_is_drawn_unwrapped_in_the_proportional_font() {
        let mut frame = offscreen::plain_frame(400, 100);
        let origin = Point::new(10.0, 20.0);
        draw_label(
            &mut frame,
            &theme(ChromeText::Cells),
            "Open",
            origin,
            Rgb::new(1, 2, 3),
        );
        let label = "Open Recent Project";
        let proportional = theme(ChromeText::Proportional);
        draw_label(&mut frame, &proportional, label, origin, Rgb::new(1, 2, 3));
        let [cells, paragraph] = [&frame.draw.text()[0], &frame.draw.text()[1]];
        assert!(cells.wrap.is_none(), "the cell grid's font");
        let wrap = paragraph.wrap.expect("the proportional font");
        assert!(
            wrap.width > label_width(&frame, &proportional, label),
            "room for the whole label on one line"
        );
    }

    #[test]
    fn test_a_styled_label_is_cut_to_its_clip_in_either_font() {
        let text = StyledText::plain(
            "Open Recent Project",
            TextColor::Palette(CellColor::Default),
        )
        .with_ellipsis();
        for chrome_text in [ChromeText::Cells, ChromeText::Proportional] {
            let mut frame = offscreen::plain_frame(400, 100);
            let theme = theme(chrome_text);
            let whole = styled_label_width(&frame, &theme, &text);
            let origin = Point::new(10.0, 20.0);
            let clip = Rect::new(origin.x, 0.0, (whole / 2.0).floor(), 100.0);
            let drawn = draw_styled_label(&mut frame, &theme, &text, origin, clip);
            assert!(
                0.0 < drawn && drawn <= clip.width,
                "{chrome_text:?}: {drawn}"
            );
            let shown: String = frame
                .draw
                .text()
                .iter()
                .flat_map(|text| &text.runs)
                .map(|run| run.text.as_str())
                .collect();
            assert!(
                shown.ends_with(text::ELLIPSIS),
                "{chrome_text:?}: {shown:?}"
            );
        }
    }
}

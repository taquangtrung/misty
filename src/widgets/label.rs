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
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::text;
use super::super::base::theme::{ChromeText, Theme};
use super::super::render::paragraph::Paragraph;

// ========================================================================
// Functions
// ========================================================================

/// How wide `text` is as a label in `theme`'s chrome font, in pixels.
pub(crate) fn label_width(frame: &Frame, theme: &Theme, text: &str) -> f32 {
    match theme.chrome_text {
        ChromeText::Cells => text::width(text) as f32 * frame.cell_metrics().width,
        ChromeText::Proportional => frame.paragraph_width(&paragraph(text, theme.foreground)),
    }
}

/// Draws `text` as a label in `color` (a theme color) on the line a cell
/// tall whose top left corner is `origin`, in `theme`'s chrome font.
pub(crate) fn draw_label(frame: &mut Frame, theme: &Theme, text: &str, origin: Point, color: Rgb) {
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
pub(crate) fn styled_label_width(frame: &Frame, theme: &Theme, text: &StyledText) -> f32 {
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
pub(crate) fn fit_label(frame: &Frame, theme: &Theme, text: &StyledText, room: f32) -> StyledText {
    match theme.chrome_text {
        ChromeText::Cells => text.fit(frame.cell_metrics().cols_in(room)),
        ChromeText::Proportional => {
            // Drop a character at a time until the label fits, as the cell
            // grid's cut does, so the ellipsis lands where it would there.
            let mut cells = text.width();
            loop {
                let fitted = text.fit(cells);
                if cells == 0 || styled_label_width(frame, theme, &fitted) <= room {
                    return fitted;
                }
                cells -= 1;
            }
        }
    }
}

/// Draws `text` as a label on the line a cell tall whose top left corner
/// is `origin`, in `theme`'s chrome font, its colors resolved through
/// `theme`. Only what fits inside `clip` is drawn: a label too long for
/// the room left is cut as [`fit_label`] cuts it. Returns how wide the
/// drawn label is, in pixels.
pub(crate) fn draw_styled_label(
    frame: &mut Frame,
    theme: &Theme,
    text: &StyledText,
    origin: Point,
    clip: Rect,
) -> f32 {
    let room = (clip.x + clip.width - origin.x).max(0.0);
    match theme.chrome_text {
        ChromeText::Cells => {
            let cells = frame.draw_text(text, origin, clip, theme);
            cells as f32 * frame.cell_metrics().width
        }
        ChromeText::Proportional => {
            let label = styled_paragraph(&fit_label(frame, theme, text, room));
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
    }
}

/// `text`'s spans, styles kept, as a paragraph.
fn styled_paragraph(text: &StyledText) -> Paragraph {
    text.spans()
        .iter()
        .fold(Paragraph::new(), |paragraph, span| {
            paragraph.with(span.clone())
        })
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
        let [cells, paragraph] = [&frame.draw.text[0], &frame.draw.text[1]];
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
                .text
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

//! [`DropList`]: a list dropped over a view, lying on its cells one row to
//! a cell row, as a document viewer's outline drops under its toolbar. Each
//! row is indented to its depth behind an arrow where it folds, its label
//! dimmed when it leads nowhere, and a note at its right (the page an
//! outline entry is on); the selected row is lit.
//!
//! The rows lie exactly on the cells of the area the app gives the list,
//! its border drawn outside them, so a row is where the cell under it is.
//! The list shows what the app gives it and the row the app selects (its
//! own keys move the selection), scrolling to keep the selected row in
//! sight. A click on a row's arrow toggles its fold, and a click elsewhere
//! on a row chooses it, which [`Widget::handle`] returns.
//!
//! Use a `DropList` for a list the app places over its own view, on the
//! view's cell grid, whose rows may nest and fold (an outline, a table of
//! contents). For matches offered while the user types, sized to them and
//! hung from the text they complete, use a
//! [`CompletionList`](crate::widgets::CompletionList).

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::list::scroll_to_show;
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::text;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::popup::Popup;
use super::tree_view::{ARROW_CELLS, INDENT_CELLS, PADDING_CELLS, draw_chevron};

// ========================================================================
// Constants
// ========================================================================

/// The blank cells between a label and its note.
const NOTE_GAP_CELLS: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// A list lying over a view's cells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DropList {
    /// Where the rows in sight were drawn last, for the pointer.
    drawn: Option<DrawnRows>,
    /// The row under the pointer, lit as a menu's rows are; `None` while
    /// the pointer is off the rows.
    hovered: Option<usize>,
    /// The rows, in order.
    rows: Vec<DropRow>,
    /// The first row in sight.
    scroll: usize,
    /// The row lit, if any.
    selected: Option<usize>,
}

/// One row of a [`DropList`]: build one with [`DropRow::new`] and the
/// methods that follow it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DropRow {
    /// How many levels deep it is indented.
    depth: usize,
    /// Whether its label is dimmed: an entry that leads nowhere.
    dim: bool,
    /// Whether it folds, and if it does, whether it is open.
    fold: Option<bool>,
    /// What it reads as.
    label: String,
    /// What is shown dimmed at its right, such as the page it is on.
    note: Option<String>,
}

/// What became of an input offered to a [`DropList`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DropListResponse {
    /// The row at this index was clicked.
    Activated(usize),
    /// The input was the list's.
    Handled,
    /// The input was not the list's.
    Ignored,
    /// The arrow of the row at this index was clicked: open or close it.
    Toggled(usize),
}

/// Where a list's rows were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DrawnRows {
    /// The cells of one row: their width and height.
    cell: (f32, f32),
    /// The index of the first row in sight.
    first: usize,
    /// The rows in sight, together.
    rect: Rect,
}

// ========================================================================
// DropList Implementations
// ========================================================================

impl DropList {
    /// A list of `rows`, none of them lit.
    pub fn new(rows: Vec<DropRow>) -> Self {
        Self {
            rows,
            ..Self::default()
        }
    }

    /// Shows `rows` instead, as after a fold opened or closed, keeping the
    /// selection where it still names a row.
    pub fn set_rows(&mut self, rows: Vec<DropRow>) {
        self.rows = rows;
        self.selected = self.selected.filter(|index| *index < self.rows.len());
    }

    /// Lights the row at `selected`, or none; the list scrolls to keep it in
    /// sight when it is next drawn.
    pub fn set_selected(&mut self, selected: Option<usize>) {
        self.selected = selected.filter(|index| *index < self.rows.len());
    }

    /// The row lit, if any.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Scrolls so `shown` rows from the first in sight hold the selected
    /// row, and no further than the last rows fill them.
    fn scroll_to_selected(&mut self, shown: usize) {
        if let Some(index) = self.selected {
            self.scroll = scroll_to_show(self.scroll, index, shown);
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(shown));
    }

    /// The row drawn under `point`, as last drawn; `None` off the rows.
    fn row_at(&self, point: Point) -> Option<usize> {
        let drawn = self.drawn?;
        if !drawn.rect.contains(point) {
            return None;
        }
        let index = drawn.first + ((point.y - drawn.rect.y) / drawn.cell.1) as usize;
        (index < self.rows.len()).then_some(index)
    }

    /// Draws the row at `index` on the line at `top`, inside `rows`.
    fn draw_row(&self, frame: &mut Frame, index: usize, top: f32, rows: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let row = &self.rows[index];
        let lit = self.selected == Some(index);
        let line = Rect::new(rows.x, top, rows.width, metrics.height);
        let band = match (lit, self.hovered == Some(index)) {
            (true, _) => Some(theme.selected_row),
            (false, true) => Some(theme.hover),
            (false, false) => None,
        };
        if let Some(band) = band {
            let scale_factor = frame.scale_factor();
            frame.draw_box(
                theme.shape.highlight_band(line, scale_factor),
                &theme.shape.highlight(band, scale_factor),
            );
        }
        // Muted text would fade into the selection color.
        let (quiet, chevron) = match lit {
            true => (TextColor::Token(ThemeToken::Foreground), theme.foreground),
            false => (TextColor::Token(ThemeToken::Muted), theme.muted),
        };
        let arrow_x = rows.x + (PADDING_CELLS + row.depth * INDENT_CELLS) as f32 * metrics.width;
        if let Some(open) = row.fold {
            let area = Rect::new(arrow_x, top, metrics.width, metrics.height);
            draw_chevron(frame, area, open, chevron);
        }
        let label_x = arrow_x + ARROW_CELLS as f32 * metrics.width;
        let right = rows.x + rows.width - PADDING_CELLS as f32 * metrics.width;
        let mut label_right = right;
        if let Some(note) = &row.note {
            let note_x = right - text::width(note) as f32 * metrics.width;
            frame.draw_text(
                &StyledText::plain(note.as_str(), quiet),
                Point::new(note_x, top),
                line,
                theme,
            );
            label_right = note_x - NOTE_GAP_CELLS as f32 * metrics.width;
        }
        let color = TextColor::Token(match row.dim {
            true => ThemeToken::Muted,
            false => ThemeToken::Foreground,
        });
        let label = StyledText::plain(row.label.as_str(), color).with_ellipsis();
        let clip = Rect::new(
            label_x,
            top,
            (label_right - label_x).max(0.0),
            metrics.height,
        );
        frame.draw_text(&label, Point::new(label_x, top), clip, theme);
    }
}

impl Widget for DropList {
    type Output = ();
    type Response = DropListResponse;

    /// Toggles the fold of the row whose arrow is clicked, chooses the row
    /// clicked elsewhere, and lights the row under the pointer; takes no
    /// other input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> DropListResponse {
        let hovered = match input {
            Input::PointerMoved(point) => Some(self.row_at(*point)),
            Input::PointerLeft => Some(None),
            _ => None,
        };
        if let Some(hovered) = hovered {
            if hovered != self.hovered {
                self.hovered = hovered;
                cx.request_redraw();
            }
            return match hovered {
                Some(_) => DropListResponse::Handled,
                None => DropListResponse::Ignored,
            };
        }
        let Input::PointerButton(button) = input else {
            return DropListResponse::Ignored;
        };
        let Some(drawn) = self.drawn else {
            return DropListResponse::Ignored;
        };
        if button.button != PointerButton::Left || !drawn.rect.contains(button.position) {
            return DropListResponse::Ignored;
        }
        if !button.pressed {
            return DropListResponse::Handled;
        }
        let (cell_width, cell_height) = drawn.cell;
        let index = drawn.first + ((button.position.y - drawn.rect.y) / cell_height) as usize;
        let Some(row) = self.rows.get(index) else {
            return DropListResponse::Handled;
        };
        let col = ((button.position.x - drawn.rect.x) / cell_width) as usize;
        let arrow = PADDING_CELLS + row.depth * INDENT_CELLS;
        match row.fold.is_some() && (arrow..arrow + ARROW_CELLS).contains(&col) {
            true => DropListResponse::Toggled(index),
            false => DropListResponse::Activated(index),
        }
    }

    /// Draws the list in the overlay, its rows on the cells of `area` from
    /// its top, as many as fit, its border outside them.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let fits = (area.height / metrics.height).floor() as usize;
        self.scroll_to_selected(fits);
        let shown = self.rows.len().saturating_sub(self.scroll).min(fits);
        if shown == 0 {
            self.drawn = None;
            return;
        }
        let rows = Rect::new(area.x, area.y, area.width, shown as f32 * metrics.height);
        let inset = Popup::inset(&theme.shape, frame.scale_factor());
        let card = Rect::new(
            rows.x - inset,
            rows.y - inset,
            rows.width + 2.0 * inset,
            rows.height + 2.0 * inset,
        );
        let style = theme
            .shape
            .floating_box(theme.raised, theme.border, frame.scale_factor());
        let first = self.scroll;
        frame.overlay(|frame| {
            frame.draw_box(card, &style);
            for (at, index) in (first..first + shown).enumerate() {
                let top = rows.y + at as f32 * metrics.height;
                self.draw_row(frame, index, top, rows, theme);
            }
        });
        self.drawn = Some(DrawnRows {
            cell: (metrics.width, metrics.height),
            first,
            rect: rows,
        });
    }
}

// ========================================================================
// DropRow Implementations
// ========================================================================

impl DropRow {
    /// A row reading `label`, at the top level, folding nothing.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            depth: 0,
            dim: false,
            fold: None,
            label: label.into(),
            note: None,
        }
    }

    /// This row indented `depth` levels.
    pub fn with_depth(self, depth: usize) -> Self {
        Self { depth, ..self }
    }

    /// This row folding its children, open or not.
    pub fn with_fold(self, open: bool) -> Self {
        Self {
            fold: Some(open),
            ..self
        }
    }

    /// This row showing `note` dimmed at its right.
    pub fn with_note(self, note: impl Into<String>) -> Self {
        Self {
            note: Some(note.into()),
            ..self
        }
    }

    /// This row with its label dimmed: an entry that leads nowhere.
    pub fn dimmed(self) -> Self {
        Self { dim: true, ..self }
    }
}

// ========================================================================
// DropListResponse Implementations
// ========================================================================

impl Outcome for DropListResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::input::event::ButtonInput;
    use crate::testing::TestWindow;

    fn outline() -> Vec<DropRow> {
        vec![
            DropRow::new("Introduction").with_note("1"),
            DropRow::new("Methods").with_fold(true).with_note("4"),
            DropRow::new("Data").with_depth(1).with_note("5"),
            DropRow::new("Results").with_fold(false).with_note("9"),
        ]
    }

    fn click(list: &mut DropList, at: Point) -> DropListResponse {
        let press = Input::PointerButton(ButtonInput::press(PointerButton::Left, at));
        TestWindow::new(400, 300).send(&press, |input, cx| list.handle(input, cx))
    }

    #[test]
    fn test_the_selection_scrolls_into_the_rows_that_fit_and_no_past_the_end() {
        let rows = (0..20).map(|n| DropRow::new(n.to_string())).collect();
        let mut list = DropList::new(rows);
        list.set_selected(Some(12));
        list.scroll_to_selected(5);
        assert_eq!(list.scroll, 8, "the selected row last");
        list.set_selected(Some(3));
        list.scroll_to_selected(5);
        assert_eq!(list.scroll, 3, "the selected row first");
        list.scroll = 18;
        list.set_selected(None);
        list.scroll_to_selected(5);
        assert_eq!(list.scroll, 15, "the last rows fill the list");
        list.set_rows(Vec::new());
        list.set_selected(Some(0));
        assert_eq!(list.selected(), None, "no row to light");
    }

    #[test]
    fn test_the_row_under_the_pointer_is_lit_until_the_pointer_leaves() {
        let theme = Theme::dark();
        let mut list = DropList::new(outline());
        let area = Rect::new(40.0, 40.0, 240.0, 200.0);
        let draw = |list: &mut DropList| {
            offscreen::render(400, 300, |frame| {
                frame.clear(Rgba::from(theme.background));
                list.draw(frame, area, &theme);
            })
        };
        if draw(&mut list).is_none() {
            return;
        }
        let height = list.drawn.expect("drawn").cell.1;
        let over = Point::new(200.0, 40.0 + 2.5 * height);
        let moved = Input::PointerMoved(over);
        let mut window = TestWindow::new(400, 300);
        window.send(&moved, |input, cx| list.handle(input, cx));
        assert_eq!(list.hovered, Some(2));
        let Some(lit) = draw(&mut list) else {
            return;
        };
        lit.assert_pixel(over.x, over.y, theme.hover, "the row under the pointer");
        window.send(&Input::PointerLeft, |input, cx| list.handle(input, cx));
        assert_eq!(list.hovered, None, "unlit once the pointer leaves");
    }

    #[test]
    fn test_a_click_on_an_arrow_toggles_and_elsewhere_chooses() {
        let theme = Theme::dark();
        let mut list = DropList::new(outline());
        list.set_selected(Some(2));
        let area = Rect::new(40.0, 40.0, 240.0, 200.0);
        let Some(snapshot) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            list.draw(frame, area, &theme);
        }) else {
            return;
        };
        let drawn = list.drawn.expect("drawn");
        let (width, height) = drawn.cell;
        assert_eq!(
            drawn.rect,
            Rect::new(40.0, 40.0, 240.0, 4.0 * height),
            "on its cells"
        );
        let row = |index: usize| 40.0 + (index as f32 + 0.5) * height;
        let arrow = 40.0 + (PADDING_CELLS as f32 + 0.5) * width;
        assert_eq!(
            click(&mut list, Point::new(arrow, row(1))),
            DropListResponse::Toggled(1)
        );
        assert_eq!(
            click(&mut list, Point::new(arrow, row(2))),
            DropListResponse::Activated(2),
            "Data folds nothing"
        );
        assert_eq!(
            click(&mut list, Point::new(200.0, row(3))),
            DropListResponse::Activated(3)
        );
        assert_eq!(
            click(&mut list, Point::new(200.0, row(5))),
            DropListResponse::Ignored,
            "under it"
        );
        snapshot.assert_pixel(
            270.0,
            row(2) - 0.5 * height + 2.0,
            theme.selection,
            "the lit row",
        );
        snapshot.assert_pixel(277.0, row(0), theme.raised, "a row not lit");
    }
}

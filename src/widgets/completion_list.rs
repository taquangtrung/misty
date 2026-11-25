//! [`CompletionList`]: the popup of matches an editor offers while the user
//! types, under the cell the completed text starts at (above it when there
//! is no room below), each row a match's kind letter, its label, its
//! detail, and the source it came from, dimmed at the right, the selected
//! match lit.
//!
//! The list shows what the app gives it and the match the app selects (an
//! editor's own completion keys move the selection), scrolling to keep the
//! selected one in sight, at most [`MAX_ROWS`] rows at a time. A click on a
//! row chooses that match, which [`Widget::handle`] returns.
//!
//! Use a `CompletionList` for matches offered while the user types: it
//! sizes itself to them and hangs from the text they complete. For a list
//! the app lays over its own view, on its cell grid, with rows that nest
//! and fold (an outline), use a [`DropList`](crate::DropList).

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::color::Rgb;
use super::super::base::geometry::{Point, Rect};
use super::super::base::list::scroll_to_show;
use super::super::base::text;
use super::super::base::theme::{Shape, Theme};
use super::super::input::event::{Input, PointerButton};
use super::popup::{Popup, PopupSide};

// ========================================================================
// Constants
// ========================================================================

/// The fewest cells wide the popup is drawn, so short matches sit in a
/// readable box.
const MIN_WIDTH_CELLS: usize = 28;

/// The most rows shown at once; more scroll.
const MAX_ROWS: usize = 10;

/// The most cells a label shows; a longer one ends in an ellipsis.
const MAX_LABEL_CELLS: usize = 40;

/// The most cells a detail shows.
const MAX_DETAIL_CELLS: usize = 30;

/// The cells a row's kind letter takes, with the blank after it.
const KIND_CELLS: usize = 2;

/// The blank cells between a label and its detail, and between the detail
/// and the source.
const DETAIL_GAP: usize = 2;

/// The most cells a source shows.
const MAX_SOURCE_CELLS: usize = 12;

/// Maximum corner radius of the completion popup, in logical pixels.
const POPUP_CORNER_RADIUS: u16 = 2;

/// Maximum vertical padding above and below each row, in logical pixels.
const ROW_PADDING: u16 = 2;

/// Share of the foreground mixed into the popup background for a hovered
/// row's subtle highlight.
const HOVER_FILL_SHARE: f32 = 0.06;

// ========================================================================
// Data Structures
// ========================================================================

/// The popup list of completion matches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionList {
    /// The cell the completed text starts at, the list opening under it.
    anchor: Rect,
    /// Where the rows were drawn last and the index of the first, for the
    /// pointer.
    drawn: Option<(Rect, usize)>,
    /// The match under the pointer, lit as a menu's rows are; `None` while
    /// the pointer is off the list.
    hovered: Option<usize>,
    /// The matches, in order.
    items: Vec<CompletionRow>,
    /// The first row in sight.
    scroll: usize,
    /// The match lit, if any.
    selected: Option<usize>,
}

/// One match of a [`CompletionList`]: build one with
/// [`CompletionRow::new`] and the `with_` methods.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CompletionRow {
    /// More about the match, dimmed at the row's right: a type, a source.
    detail: Option<String>,
    /// A letter for its kind (`f` a function, `v` a variable), in the
    /// accent color at the row's left.
    kind: Option<char>,
    /// The match as shown.
    label: String,
    /// Where the match came from (`lsp`, `words`), dimmed at the far right.
    source: Option<String>,
}

/// What became of an input offered to a [`CompletionList`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CompletionListResponse {
    /// The match at this index was clicked.
    Activated(usize),
    /// The input was the list's.
    Handled,
    /// The input was not the list's.
    Ignored,
}

// ========================================================================
// CompletionList Implementations
// ========================================================================

impl CompletionList {
    /// A list of `items` under `anchor`, the cell the completed text
    /// starts at, none of them lit.
    pub fn new(items: Vec<CompletionRow>, anchor: Rect) -> Self {
        Self {
            anchor,
            items,
            ..Self::default()
        }
    }

    /// Lights the match at `selected`, or none, scrolling to keep it in
    /// sight.
    pub fn set_selected(&mut self, selected: Option<usize>) {
        self.selected = selected.filter(|index| *index < self.items.len());
        if let Some(index) = self.selected {
            self.scroll = scroll_to_show(self.scroll, index, MAX_ROWS);
        }
    }

    /// Moves the list to open under `anchor`.
    pub fn set_anchor(&mut self, anchor: Rect) {
        self.anchor = anchor;
    }

    /// The match lit, if any.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The cells the widest row takes, floored at the minimum popup width.
    fn width_cells(&self) -> usize {
        let label = self.items.iter().map(|item| text::width(&item.label)).max();
        let detail = self
            .items
            .iter()
            .filter_map(|item| item.detail.as_deref())
            .map(text::width)
            .max();
        let detail = detail.map_or(0, |cells| DETAIL_GAP + cells.min(MAX_DETAIL_CELLS));
        let content =
            KIND_CELLS + label.unwrap_or(0).min(MAX_LABEL_CELLS) + detail + self.source_cells();
        content.max(MIN_WIDTH_CELLS)
    }

    /// The cells the sources' column takes at the right, its gap included;
    /// 0 when no match names one.
    fn source_cells(&self) -> usize {
        self.items
            .iter()
            .filter_map(|item| item.source.as_deref())
            .map(text::width)
            .max()
            .map_or(0, |cells| DETAIL_GAP + cells.min(MAX_SOURCE_CELLS))
    }

    /// The rows in sight: their indices.
    fn visible(&self) -> std::ops::Range<usize> {
        self.scroll..(self.scroll + MAX_ROWS).min(self.items.len())
    }
    /// The match drawn under `point`, as last drawn; `None` off the list.
    fn row_at(&self, point: Point) -> Option<usize> {
        let (rows, first) = self.drawn?;
        if !rows.contains(point) {
            return None;
        }
        let count = self.visible().len().max(1);
        let row_height = rows.height / count as f32;
        let index = first + ((point.y - rows.y) / row_height) as usize;
        (index < self.items.len()).then_some(index)
    }
}

impl Widget for CompletionList {
    type Output = ();
    type Response = CompletionListResponse;

    /// Chooses the match clicked, and lights the match under the pointer;
    /// takes no other input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> CompletionListResponse {
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
                Some(_) => CompletionListResponse::Handled,
                None => CompletionListResponse::Ignored,
            };
        }
        let Input::PointerButton(button) = input else {
            return CompletionListResponse::Ignored;
        };
        let Some((rows, _)) = self.drawn else {
            return CompletionListResponse::Ignored;
        };
        if button.button != PointerButton::Left || !rows.contains(button.position) {
            return CompletionListResponse::Ignored;
        }
        if !button.pressed {
            return CompletionListResponse::Handled;
        }
        match self.row_at(button.position) {
            Some(index) => CompletionListResponse::Activated(index),
            None => CompletionListResponse::Handled,
        }
    }

    /// Draws the list in the overlay under its anchor, inside `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let visible = self.visible();
        if visible.is_empty() {
            self.drawn = None;
            return;
        }
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let shape = popup_shape(theme);
        let inset = Popup::inset(&shape, scale_factor);
        let pad_x = metrics.width.round();
        let pad_y = (f32::from(shape.row_padding) * scale_factor as f32).round();
        let row_height = metrics.height + 2.0 * pad_y;
        let width = self.width_cells() as f32 * metrics.width + 2.0 * (pad_x + inset);
        let height = visible.len() as f32 * row_height + 2.0 * inset;
        let popup = Popup::new(self.anchor, width, height, PopupSide::Below, area);
        let style = shape.floating_box(theme.raised, theme.border, scale_factor);
        frame.overlay(|frame| frame.draw_box(popup.rect(), &style));
        let content = popup.content(&shape, scale_factor);
        self.drawn = Some((content, visible.start));
        let source_cells = self.source_cells();
        let label_cells = self.width_cells() - KIND_CELLS - source_cells;
        for (row, index) in visible.enumerate() {
            let item = &self.items[index];
            let row_top = content.y + row as f32 * row_height;
            let top = row_top + pad_y;
            let text_left = content.x + pad_x;
            let is_selected = self.selected == Some(index);
            let is_hovered = !is_selected && self.hovered == Some(index);
            frame.overlay(|frame| {
                let row_rect = Rect::new(content.x, row_top, content.width, row_height);
                if is_selected {
                    frame.fill_rect(row_rect, theme.selected_row);
                } else if is_hovered {
                    frame.fill_rect(row_rect, hovered_row_fill(theme));
                }
                let (kind_color, secondary_color) = match is_selected {
                    true => (theme.foreground, theme.foreground),
                    false => (theme.accent, theme.muted),
                };
                if let Some(kind) = item.kind {
                    let at = Point::new(text_left, top);
                    frame.draw_plain_text(&kind.to_string(), at, kind_color);
                }
                let label = text::fit(&item.label, MAX_LABEL_CELLS);
                let at = Point::new(text_left + KIND_CELLS as f32 * metrics.width, top);
                frame.draw_plain_text(&label, at, theme.foreground);
                if let Some(detail) = &item.detail {
                    let detail = text::fit(detail, MAX_DETAIL_CELLS);
                    let cells = text::width(&detail);
                    let right = KIND_CELLS + label_cells;
                    let at = Point::new(text_left + (right - cells) as f32 * metrics.width, top);
                    frame.draw_plain_text(&detail, at, secondary_color);
                }
                if let Some(source) = &item.source {
                    let source = text::fit(source, MAX_SOURCE_CELLS);
                    let left = KIND_CELLS + label_cells + DETAIL_GAP;
                    let at = Point::new(text_left + left as f32 * metrics.width, top);
                    frame.draw_plain_text(&source, at, secondary_color);
                }
            });
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Crisp popup shape derived from `theme`: tighter corners, compact rows, and
/// full-width row bands while preserving square grid themes.
fn popup_shape(theme: &Theme) -> Shape {
    Shape {
        corner_radius: theme.shape.corner_radius.min(POPUP_CORNER_RADIUS),
        highlight_inset: 0,
        row_padding: theme.shape.row_padding.min(ROW_PADDING),
        ..theme.shape
    }
}

/// Subtle neutral fill for a hovered completion row.
fn hovered_row_fill(theme: &Theme) -> Rgb {
    theme.raised.mix(theme.foreground, HOVER_FILL_SHARE)
}

// ========================================================================
// CompletionRow Implementations
// ========================================================================

impl CompletionRow {
    /// A match shown as `label`, with no kind, detail, or source.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            detail: None,
            kind: None,
            label: label.into(),
            source: None,
        }
    }

    /// The same match with `detail` dimmed at the row's right: a type, a
    /// signature.
    pub fn with_detail(self, detail: impl Into<String>) -> Self {
        Self {
            detail: Some(detail.into()),
            ..self
        }
    }

    /// The same match with `kind` (`f` a function, `v` a variable) in the
    /// accent color at the row's left.
    pub fn with_kind(self, kind: char) -> Self {
        Self {
            kind: Some(kind),
            ..self
        }
    }

    /// The same match naming where it came from (`lsp`, `words`), dimmed
    /// at the far right.
    pub fn with_source(self, source: impl Into<String>) -> Self {
        Self {
            source: Some(source.into()),
            ..self
        }
    }
}

// ========================================================================
// CompletionListResponse Implementations
// ========================================================================

impl Outcome for CompletionListResponse {
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

    fn row(label: &str, detail: Option<&str>) -> CompletionRow {
        let row = CompletionRow::new(label).with_kind('f');
        match detail {
            Some(detail) => row.with_detail(detail),
            None => row,
        }
    }

    #[test]
    fn test_a_source_column_follows_the_details_when_any_match_names_one() {
        let mut items = vec![
            row("calculate_completion_width", Some("usize")),
            row("beta", None),
        ];
        let without = CompletionList::new(items.clone(), Rect::default()).width_cells();
        items[1] = items[1].clone().with_source("words");
        let with = CompletionList::new(items, Rect::default()).width_cells();
        assert_eq!(with, without + DETAIL_GAP + 5, "a gap and `words`");
    }

    #[test]
    fn test_short_matches_respect_the_minimum_width() {
        let list = CompletionList::new(vec![row("if", None), row("in", None)], Rect::default());
        assert_eq!(list.width_cells(), MIN_WIDTH_CELLS);
    }

    #[test]
    fn test_the_selection_scrolls_into_sight_both_ways() {
        let items = (0..25).map(|n| row(&n.to_string(), None)).collect();
        let mut list = CompletionList::new(items, Rect::default());
        list.set_selected(Some(14));
        assert_eq!(list.visible(), 5..15, "the selected one last");
        list.set_selected(Some(2));
        assert_eq!(list.visible(), 2..12, "the selected one first");
        list.set_selected(Some(99));
        assert_eq!(list.selected(), None, "past the end lights none");
    }

    #[test]
    fn test_the_list_opens_under_its_anchor_with_the_selected_row_lit() {
        let theme = Theme::dark();
        let mut list = CompletionList::new(
            vec![row("alpha", None), row("beta", Some("u8"))],
            Rect::new(10.0, 10.0, 8.0, 16.0),
        );
        list.set_selected(Some(1));
        let Some(snapshot) = crate::app::offscreen::render(300, 200, |frame| {
            frame.clear(crate::base::color::Rgba::from(theme.background));
            list.draw(frame, frame.bounds(), &theme);
        }) else {
            return;
        };
        let (content, first) = list.drawn.expect("drawn");
        assert_eq!(first, 0);
        assert!(content.y >= 26.0, "under the anchor: {content:?}");
        let metrics = snapshot.metrics();
        let shape = popup_shape(&theme);
        let row_height = metrics.height + 2.0 * f32::from(shape.row_padding);
        let second = content.y + row_height + 2.0;
        snapshot.assert_pixel(
            content.x + content.width / 2.0,
            second,
            theme.selected_row,
            "lit with selected row fill",
        );
        snapshot.assert_pixel(
            content.x + content.width - 1.0,
            content.y + 1.0,
            theme.raised,
            "not lit",
        );
    }

    #[test]
    fn test_the_width_holds_the_widest_label_and_detail() {
        let label = "calculate_completion_width";
        let detail = "usize";
        let list = CompletionList::new(
            vec![row(label, Some(detail)), row("lines", None)],
            Rect::default(),
        );
        assert_eq!(
            list.width_cells(),
            KIND_CELLS + label.len() + DETAIL_GAP + detail.len()
        );
    }
}

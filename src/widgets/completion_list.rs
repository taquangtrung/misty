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
use super::super::base::geometry::{Point, Rect};
use super::super::base::list::scroll_to_show;
use super::super::base::text;
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::popup::{Popup, PopupSide};

// ========================================================================
// Constants
// ========================================================================

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

    /// The cells the widest row takes: kind, label, gap, and detail.
    fn width_cells(&self) -> usize {
        let label = self.items.iter().map(|item| text::width(&item.label)).max();
        let detail = self
            .items
            .iter()
            .filter_map(|item| item.detail.as_deref())
            .map(text::width)
            .max();
        let detail = detail.map_or(0, |cells| DETAIL_GAP + cells.min(MAX_DETAIL_CELLS));
        KIND_CELLS + label.unwrap_or(0).min(MAX_LABEL_CELLS) + detail + self.source_cells()
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
}

impl Widget for CompletionList {
    type Output = ();
    type Response = CompletionListResponse;

    /// Chooses the match clicked; takes no other input.
    fn handle(&mut self, input: &Input, _cx: &mut EventCx<'_>) -> CompletionListResponse {
        let Input::PointerButton(button) = input else {
            return CompletionListResponse::Ignored;
        };
        let Some((rows, first)) = self.drawn else {
            return CompletionListResponse::Ignored;
        };
        if button.button != PointerButton::Left || !rows.contains(button.position) {
            return CompletionListResponse::Ignored;
        }
        if !button.pressed {
            return CompletionListResponse::Handled;
        }
        let count = self.visible().len().max(1);
        let row_height = rows.height / count as f32;
        let row = ((button.position.y - rows.y) / row_height) as usize;
        match first + row < self.items.len() {
            true => CompletionListResponse::Activated(first + row),
            false => CompletionListResponse::Handled,
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
        let inset = Popup::inset(&theme.shape, frame.scale_factor());
        let width = self.width_cells() as f32 * metrics.width + 2.0 * inset;
        let height = visible.len() as f32 * metrics.height + 2.0 * inset;
        let popup = Popup::new(self.anchor, width, height, PopupSide::Below, area);
        let style = theme
            .shape
            .floating_box(theme.raised, theme.border, frame.scale_factor());
        frame.overlay(|frame| frame.draw_box(popup.rect(), &style));
        let content = popup.content(&theme.shape, frame.scale_factor());
        self.drawn = Some((content, visible.start));
        let source_cells = self.source_cells();
        let label_cells = self.width_cells() - KIND_CELLS - source_cells;
        for (row, index) in visible.enumerate() {
            let item = &self.items[index];
            let top = content.y + row as f32 * metrics.height;
            let lit = self.selected == Some(index);
            frame.overlay(|frame| {
                if lit {
                    frame.fill_rect(
                        Rect::new(content.x, top, content.width, metrics.height),
                        theme.selection,
                    );
                }
                if let Some(kind) = item.kind {
                    let at = Point::new(content.x, top);
                    frame.draw_plain_text(&kind.to_string(), at, theme.accent);
                }
                let label = text::fit(&item.label, MAX_LABEL_CELLS);
                let at = Point::new(content.x + KIND_CELLS as f32 * metrics.width, top);
                frame.draw_plain_text(&label, at, theme.foreground);
                if let Some(detail) = &item.detail {
                    let detail = text::fit(detail, MAX_DETAIL_CELLS);
                    let cells = text::width(&detail);
                    let right = KIND_CELLS + label_cells;
                    let at = Point::new(content.x + (right - cells) as f32 * metrics.width, top);
                    frame.draw_plain_text(&detail, at, theme.muted);
                }
                if let Some(source) = &item.source {
                    let source = text::fit(source, MAX_SOURCE_CELLS);
                    let left = KIND_CELLS + label_cells + DETAIL_GAP;
                    let at = Point::new(content.x + left as f32 * metrics.width, top);
                    frame.draw_plain_text(&source, at, theme.muted);
                }
            });
        }
    }
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
        let mut items = vec![row("alpha", Some("u8")), row("beta", None)];
        let without = CompletionList::new(items.clone(), Rect::default()).width_cells();
        items[1] = items[1].clone().with_source("words");
        let with = CompletionList::new(items, Rect::default()).width_cells();
        assert_eq!(with, without + DETAIL_GAP + 5, "a gap and `words`");
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
        let second = content.y + metrics.height + 1.0;
        snapshot.assert_pixel(
            content.x + content.width - 1.0,
            second,
            theme.selection,
            "lit",
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
        let list = CompletionList::new(
            vec![row("len", Some("usize")), row("lines", None)],
            Rect::default(),
        );
        assert_eq!(list.width_cells(), KIND_CELLS + 5 + DETAIL_GAP + 5);
    }
}

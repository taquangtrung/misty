//! A [`ListState`]: the cursor, selection, and scroll of a list too long to
//! show at once, as plain numbers. Rows are named by index; what a row
//! shows is the app's.
//!
//! The cursor is always on screen: moving it scrolls the list so a margin
//! of rows stays visible around it, and scrolling the list (with the wheel)
//! drags the cursor along when it would leave the screen.
//!
//! A range selection runs from an anchor row: [`ListState::select_to`]
//! selects every row between the anchor and a row, on top of what was
//! selected when the anchor was set (Shift+click, Shift with the arrows),
//! and [`ListState::start_range`] makes the range follow the cursor, as
//! Vim's `v` does, until [`ListState::end_range`].

use std::collections::BTreeSet;
use std::ops::Range;

// ========================================================================
// Constants
// ========================================================================

/// The rows kept visible above and below the cursor unless
/// [`ListState::set_margin`] says otherwise.
const DEFAULT_MARGIN: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// The cursor, selection, and scroll of a list. It starts empty, with no
/// rows on screen; the app sets how many rows there are and the widget
/// drawing the list sets how many fit.
#[derive(Clone, Debug, PartialEq)]
pub struct ListState {
    /// The row a range selection runs from, or `None` when none runs.
    anchor: Option<usize>,
    /// The rows selected when the anchor was set, which a range is laid
    /// over.
    base: BTreeSet<usize>,
    /// How many rows the list has.
    count: usize,
    /// The row the cursor is on; 0 in an empty list.
    cursor: usize,
    /// Whether the range follows the cursor, as Vim's visual mode.
    is_ranging: bool,
    /// How many rows to keep visible above and below the cursor.
    margin: usize,
    /// How many leading rows stay on screen above the others, which
    /// scroll under them, as a spreadsheet freezes its first rows; 0 for
    /// none.
    pinned: usize,
    /// The first row on screen.
    scroll: usize,
    /// The selected rows.
    selected: BTreeSet<usize>,
    /// How many rows fit on screen.
    visible: usize,
}

// ========================================================================
// ListState Implementations
// ========================================================================

impl ListState {
    // --------------------------------------------------------------------
    // Setup
    // --------------------------------------------------------------------

    /// A state for a list of `count` rows, the cursor on the first.
    pub fn new(count: usize) -> Self {
        Self {
            anchor: None,
            base: BTreeSet::new(),
            count,
            cursor: 0,
            is_ranging: false,
            margin: DEFAULT_MARGIN,
            pinned: 0,
            scroll: 0,
            selected: BTreeSet::new(),
            visible: 0,
        }
    }

    /// Sets how many rows the list has. The cursor and scroll are kept
    /// where they are when they still fit and pulled back when they do not;
    /// selected rows past the end are deselected, and an anchor past the
    /// end ends its range.
    pub fn set_count(&mut self, count: usize) {
        self.count = count;
        self.selected.retain(|&row| row < count);
        self.base.retain(|&row| row < count);
        if self.anchor.is_some_and(|anchor| anchor >= count) {
            self.anchor = None;
            self.is_ranging = false;
        }
        self.cursor = self.cursor.min(self.last_row());
        self.reveal_cursor();
        self.follow_range();
    }

    /// Sets how many rows fit on screen. Called by the widget drawing the
    /// list each frame.
    pub fn set_visible(&mut self, visible: usize) {
        if self.visible != visible {
            self.visible = visible;
            self.reveal_cursor();
        }
    }

    /// Sets how many rows to keep visible above and below the cursor while
    /// it moves; fewer when the screen is too short for them.
    pub fn set_margin(&mut self, margin: usize) {
        self.margin = margin;
        self.reveal_cursor();
    }

    /// The rows kept visible above and below the cursor now: the margin
    /// [`ListState::set_margin`] set, or less when the screen is too short
    /// for it (at most half of the screen less the cursor's row, so the
    /// cursor can always reach the middle).
    pub fn margin(&self) -> usize {
        self.margin
            .min(self.visible.saturating_sub(1 + self.pinned) / 2)
    }

    /// Keeps the first `rows` rows on screen above the others, which
    /// scroll under them: the rows after them that the scroll hides are
    /// the ones under the pinned rows, and the cursor on a row after them
    /// is scrolled out from under them. Set by the widget drawing the
    /// list each frame, as [`ListState::set_visible`] is.
    pub(crate) fn set_pinned(&mut self, rows: usize) {
        if self.pinned != rows {
            self.pinned = rows;
            self.reveal_cursor();
        }
    }

    // --------------------------------------------------------------------
    // Cursor
    // --------------------------------------------------------------------

    /// Puts the cursor on `row`, or the last row when `row` is past it, and
    /// scrolls it into view.
    pub fn set_cursor(&mut self, row: usize) {
        self.cursor = row.min(self.last_row());
        self.reveal_cursor();
        self.follow_range();
    }

    /// Moves the cursor `delta` rows down (up when negative), stopping at
    /// either end.
    pub fn move_cursor(&mut self, delta: isize) {
        self.set_cursor(self.cursor.saturating_add_signed(delta));
    }

    /// Moves the view and the cursor `delta` pages down (up when negative),
    /// a page being a screen less one row, so the row at the edge stays in
    /// sight. At either end the cursor goes on to the first or last row.
    pub fn page(&mut self, delta: isize) {
        let page = self.visible.saturating_sub(1).max(1);
        let rows = delta.saturating_mul(page as isize);
        self.scroll = self
            .scroll
            .saturating_add_signed(rows)
            .min(self.max_scroll());
        self.cursor = self.cursor.saturating_add_signed(rows).min(self.last_row());
        self.reveal_cursor();
        self.follow_range();
    }

    /// The row the cursor is on; 0 in an empty list.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    // --------------------------------------------------------------------
    // Scroll
    // --------------------------------------------------------------------

    /// Scrolls the view `delta` rows down (up when negative) without moving
    /// the cursor, unless it would leave the screen, in which case it is
    /// dragged along to stay inside the margin.
    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll());
        let margin = self.margin();
        let top = match self.scroll {
            0 => 0,
            scroll => scroll + self.pinned + margin,
        };
        let bottom = match self.scroll == self.max_scroll() {
            true => self.last_row(),
            false => (self.scroll + self.visible).saturating_sub(1 + margin),
        };
        if self.cursor >= self.pinned {
            self.cursor = self.cursor.clamp(top.min(bottom), bottom);
        }
        self.follow_range();
    }

    /// The first row on screen.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// The rows on screen: the only ones a widget needs to draw.
    pub fn visible_rows(&self) -> Range<usize> {
        self.scroll..(self.scroll + self.visible).min(self.count)
    }

    // --------------------------------------------------------------------
    // Selection
    // --------------------------------------------------------------------

    /// Selects `row` when it is not selected and deselects it when it is;
    /// rows past the end are ignored. A range running keeps the change
    /// under it.
    pub fn toggle_selected(&mut self, row: usize) {
        if row >= self.count {
            return;
        }
        let selected = !self.selected.remove(&row);
        if selected {
            self.selected.insert(row);
        }
        if self.anchor.is_some() {
            match selected {
                true => self.base.insert(row),
                false => self.base.remove(&row),
            };
        }
    }

    /// Makes `rows` the selection, in place of what was selected; rows
    /// past the end are ignored. Ends a range.
    pub fn set_selected(&mut self, rows: impl IntoIterator<Item = usize>) {
        let count = self.count;
        self.selected = rows.into_iter().filter(|&row| row < count).collect();
        self.anchor = None;
        self.is_ranging = false;
    }

    /// Deselects every row and ends a range.
    pub fn clear_selection(&mut self) {
        self.set_selected([]);
    }

    /// The row a range selection runs from, or `None` when none runs.
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// Starts a range selection at `row` (held to the last row), laid over
    /// the rows selected now, or ends it with `None`; the selection stays
    /// as it is either way.
    pub fn set_anchor(&mut self, row: Option<usize>) {
        self.anchor = row
            .filter(|_| self.count > 0)
            .map(|row| row.min(self.last_row()));
        self.base = match self.anchor {
            Some(_) => self.selected.clone(),
            None => BTreeSet::new(),
        };
        self.is_ranging &= self.anchor.is_some();
    }

    /// Selects every row from the anchor to `row`, both included, on top
    /// of the rows selected when the anchor was set: rows an earlier call
    /// took in and this one does not are deselected again. With no anchor,
    /// the cursor's row becomes it first.
    pub fn select_to(&mut self, row: usize) {
        if self.count == 0 {
            return;
        }
        let anchor = match self.anchor {
            Some(anchor) => anchor,
            None => {
                self.set_anchor(Some(self.cursor));
                self.cursor
            }
        };
        let row = row.min(self.last_row());
        let mut selected = self.base.clone();
        selected.extend(anchor.min(row)..=anchor.max(row));
        self.selected = selected;
    }

    /// Starts a range at the cursor that follows it, as Vim's `v`: until
    /// [`ListState::end_range`], every move of the cursor selects the rows
    /// from the anchor to it, on top of what was selected before.
    pub fn start_range(&mut self) {
        self.set_anchor(Some(self.cursor));
        self.is_ranging = self.anchor.is_some();
        self.follow_range();
    }

    /// Stops the range following the cursor and drops its anchor; the rows
    /// it selected stay selected.
    pub fn end_range(&mut self) {
        self.set_anchor(None);
    }

    /// Whether a range started by [`ListState::start_range`] follows the
    /// cursor.
    pub fn is_ranging(&self) -> bool {
        self.is_ranging
    }

    /// Whether `row` is selected.
    pub fn is_selected(&self, row: usize) -> bool {
        self.selected.contains(&row)
    }

    /// The selected rows, in order.
    pub fn selected(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    /// How many rows the list has.
    pub fn count(&self) -> usize {
        self.count
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// The last row's index; 0 in an empty list.
    fn last_row(&self) -> usize {
        self.count.saturating_sub(1)
    }

    /// The furthest the view scrolls: the last row at the bottom of the
    /// screen.
    fn max_scroll(&self) -> usize {
        self.count.saturating_sub(self.visible)
    }

    /// Selects the rows from the anchor to the cursor while a range
    /// follows it.
    fn follow_range(&mut self) {
        if self.is_ranging {
            self.select_to(self.cursor);
        }
    }

    /// Scrolls as little as possible to show the cursor with the margin
    /// around it, then keeps the scroll in range.
    fn reveal_cursor(&mut self) {
        if self.visible == 0 {
            self.scroll = self.scroll.min(self.max_scroll());
            return;
        }
        if self.cursor < self.pinned || self.visible <= self.pinned {
            self.scroll = self.scroll.min(self.max_scroll());
            return;
        }
        // The rows after the pinned ones show from `scroll + pinned` on,
        // in the room the pinned ones leave.
        let (pinned, room) = (self.pinned, self.visible - self.pinned);
        let margin = self.margin();
        let top = self.cursor.saturating_sub(margin).max(pinned);
        let first = scroll_to_show(self.scroll + pinned, top, room);
        let first = scroll_to_show(first, self.cursor + margin, room);
        self.scroll = (first - pinned).min(self.max_scroll());
    }
}

impl Default for ListState {
    fn default() -> Self {
        Self::new(0)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The first row to show so row `target` is among the `visible` rows shown,
/// moving as little as needed from `first`: the same row when `target` is
/// already in sight, `target` itself when it is above, and the row that
/// puts `target` last when it is below. Rows can be lines, columns, or
/// cells; anything shown a run at a time.
pub(crate) fn scroll_to_show(first: usize, target: usize, visible: usize) -> usize {
    if target < first {
        target
    } else if target >= first + visible {
        (target + 1).saturating_sub(visible)
    } else {
        first
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A list of `count` rows with 10 on screen and a margin of 2.
    fn list(count: usize) -> ListState {
        let mut list = ListState::new(count);
        list.set_visible(10);
        list
    }

    #[test]
    fn test_moving_down_scrolls_to_keep_the_margin_below_the_cursor() {
        let mut list = list(100);
        list.move_cursor(7);
        assert_eq!(list.scroll(), 0, "row 7 still has two rows below it");
        list.move_cursor(1);
        assert_eq!(list.scroll(), 1);
        assert_eq!(list.visible_rows(), 1..11);
    }

    #[test]
    fn test_moving_up_scrolls_to_keep_the_margin_above_the_cursor() {
        let mut list = list(100);
        list.set_cursor(50);
        assert_eq!(list.scroll(), 43, "row 50 two rows from the bottom");
        list.move_cursor(-6);
        assert_eq!(list.scroll(), 42, "row 44 two rows from the top");
    }

    #[test]
    fn test_the_margin_gives_way_at_the_ends_of_the_list() {
        let mut list = list(100);
        list.set_cursor(99);
        assert_eq!(list.scroll(), 90);
        list.move_cursor(-200);
        assert_eq!((list.cursor(), list.scroll()), (0, 0));
    }

    #[test]
    fn test_a_short_screen_shrinks_the_margin() {
        let mut list = ListState::new(100);
        list.set_visible(3);
        list.set_margin(5);
        list.move_cursor(2);
        assert_eq!(list.scroll(), 1, "a margin of 1 on a 3-row screen");
        assert_eq!(list.margin(), 1);
    }

    #[test]
    fn test_paging_moves_the_view_and_the_cursor_a_screen_less_one_row() {
        let mut list = list(100);
        list.set_cursor(3);
        list.page(1);
        assert_eq!((list.cursor(), list.scroll()), (12, 9));
        list.page(-1);
        assert_eq!((list.cursor(), list.scroll()), (3, 0));
        list.page(100);
        assert_eq!((list.cursor(), list.scroll()), (99, 90));
    }

    #[test]
    fn test_the_wheel_drags_the_cursor_only_when_it_would_leave_the_screen() {
        let mut list = list(100);
        list.set_cursor(5);
        list.scroll_by(2);
        assert_eq!((list.cursor(), list.scroll()), (5, 2));
        list.scroll_by(5);
        assert_eq!(
            (list.cursor(), list.scroll()),
            (9, 7),
            "two rows below the top"
        );
        list.scroll_by(1000);
        assert_eq!(list.scroll(), 90);
        list.scroll_by(-1000);
        assert_eq!(list.scroll(), 0);
        assert_eq!(list.cursor(), 7, "two rows above the bottom");
    }

    #[test]
    fn test_shrinking_the_list_pulls_the_cursor_scroll_and_selection_back() {
        let mut list = list(100);
        list.set_cursor(80);
        list.toggle_selected(10);
        list.toggle_selected(90);
        list.set_count(50);
        assert_eq!((list.cursor(), list.scroll()), (49, 40));
        assert_eq!(list.selected().collect::<Vec<_>>(), [10]);
        list.set_count(0);
        assert_eq!((list.cursor(), list.scroll()), (0, 0));
        assert_eq!(list.visible_rows(), 0..0);
    }

    #[test]
    fn test_a_list_shorter_than_the_screen_never_scrolls() {
        let mut list = list(4);
        list.set_cursor(3);
        list.scroll_by(5);
        list.page(1);
        assert_eq!((list.cursor(), list.scroll()), (3, 0));
        assert_eq!(list.visible_rows(), 0..4);
    }

    #[test]
    fn test_toggling_selects_and_deselects_and_ignores_rows_past_the_end() {
        let mut list = list(5);
        list.toggle_selected(1);
        list.toggle_selected(3);
        list.toggle_selected(1);
        list.toggle_selected(7);
        assert_eq!(list.selected().collect::<Vec<_>>(), [3]);
        assert!(list.is_selected(3));
    }

    #[test]
    fn test_selecting_to_a_row_replaces_the_previous_range_over_the_base() {
        let mut list = list(20);
        list.toggle_selected(15);
        list.set_anchor(Some(5));
        list.select_to(8);
        assert_eq!(list.selected().collect::<Vec<_>>(), [5, 6, 7, 8, 15]);
        list.select_to(3);
        assert_eq!(
            list.selected().collect::<Vec<_>>(),
            [3, 4, 5, 15],
            "rows 6 to 8 leave the range, row 15 stays from before it"
        );
    }

    #[test]
    fn test_selecting_with_no_anchor_starts_the_range_at_the_cursor() {
        let mut list = list(20);
        list.set_cursor(4);
        list.select_to(2);
        assert_eq!(list.anchor(), Some(4));
        assert_eq!(list.selected().collect::<Vec<_>>(), [2, 3, 4]);
    }

    #[test]
    fn test_a_visual_range_follows_every_cursor_move_until_it_ends() {
        let mut list = list(100);
        list.set_cursor(10);
        list.start_range();
        assert_eq!(list.selected().collect::<Vec<_>>(), [10]);
        list.move_cursor(2);
        assert_eq!(list.selected().collect::<Vec<_>>(), [10, 11, 12]);
        list.move_cursor(-4);
        assert_eq!(list.selected().collect::<Vec<_>>(), [8, 9, 10]);
        list.page(1);
        assert_eq!(list.selected().count(), list.cursor() - 10 + 1);
        list.end_range();
        assert!(!list.is_ranging());
        list.move_cursor(5);
        assert_eq!(list.selected().last(), Some(list.cursor() - 5), "it stays");
    }

    #[test]
    fn test_a_toggle_inside_a_range_survives_the_next_extension() {
        let mut list = list(20);
        list.set_anchor(Some(2));
        list.select_to(4);
        list.toggle_selected(10);
        list.select_to(5);
        assert_eq!(list.selected().collect::<Vec<_>>(), [2, 3, 4, 5, 10]);
    }

    #[test]
    fn test_shrinking_the_list_past_the_anchor_ends_the_range() {
        let mut list = list(20);
        list.set_cursor(15);
        list.start_range();
        list.set_count(10);
        assert_eq!((list.anchor(), list.is_ranging()), (None, false));
        assert!(list.selected().all(|row| row < 10));
    }

    #[test]
    fn test_pinned_rows_never_scroll_and_the_cursor_is_kept_out_from_under_them() {
        let mut list = list(100);
        list.set_pinned(2);
        list.set_cursor(50);
        let scroll = list.scroll();
        assert!(
            (scroll + 2 + list.margin()..scroll + 10).contains(&50),
            "row 50 below the pinned rows, {scroll}"
        );
        list.set_cursor(1);
        assert_eq!(list.scroll(), scroll, "a pinned row is always in sight");
        list.set_cursor(20);
        let scroll = list.scroll();
        list.set_cursor(scroll + 2);
        assert_eq!(
            list.scroll() + 2 + list.margin(),
            scroll + 2,
            "scrolled up out from under the pinned rows"
        );
        list.scroll_by(10);
        assert_eq!(
            list.cursor(),
            list.scroll() + 2 + list.margin(),
            "dragged along"
        );
    }
}

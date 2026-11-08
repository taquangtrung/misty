//! [`WhichKey`]: a popup listing the keys that may follow a pending key
//! prefix and what each one does, so a user who typed the first key of a
//! sequence can read the rest instead of remembering it.
//!
//! The app tells the popup the prefix its resolver is waiting on after
//! every key and every tick, with a [`WhichKeyEntry`] for each key that
//! may follow, from whatever keymap the app resolves keys with. The popup shows only after the prefix has waited a
//! short delay, so a user typing a sequence they know never sees it, and
//! hides as soon as the prefix resolves or is reset (an empty prefix). The
//! delay runs on the event loop's deadlines: [`WhichKey::about_to_wait`],
//! called from
//! [`Application::about_to_wait`](crate::Application::about_to_wait), shows
//! the popup when it is due and returns when it will be.
//!
//! An app that times the hint itself, or names its keys in its own words,
//! shows it at once with [`WhichKey::show`]: a header of its choosing and
//! entries whose keys are written as it writes them
//! ([`WhichKeyEntry::written`]).
//!
//! A click on an entry answers [`WhichKeyResponse::Activated`], for the app
//! to act as if its key were typed; a click elsewhere while the popup shows
//! answers [`WhichKeyResponse::Closed`].
//!
//! The popup sits along the bottom of the area the app gives it, as wide as
//! the area, with a header naming the prefix and the entries in columns,
//! filled top to bottom then left to right.

use std::time::{Duration, Instant};

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, key_label};
use super::super::base::strings::{self, Strings};
use super::super::base::styled::{Span, StyledText};
use super::super::base::text;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::icon::logical;
use super::popup::Popup;

// ========================================================================
// Constants
// ========================================================================

/// How long a prefix waits before the popup shows, unless
/// [`WhichKey::with_delay`] says otherwise.
const DEFAULT_DELAY: Duration = Duration::from_millis(500);

/// What stands before the title of an entry that leads to more keys.
const MORE_KEYS_MARK: &str = "+";

/// The blank cells inside the popup's left and right borders.
const EDGE_CELLS: usize = 1;

/// The blank cells between an entry's key and its title.
const KEY_GAP_CELLS: usize = 1;

/// The blank cells between two columns.
const COLUMN_GAP_CELLS: usize = 3;

/// The most cells an entry's title takes; a longer one ends in an
/// ellipsis.
const MAX_TITLE_CELLS: usize = 32;

/// What separates the header's name from its note on keys not shown.
const HIDDEN_GAP: &str = "  ";

/// The rows the header takes.
const HEADER_ROWS: usize = 1;

/// The most of its area's height the popup takes.
const MAX_HEIGHT_SHARE: f32 = 0.5;

/// The space above and below the popup's rows, in logical pixels.
const PADDING_Y: f32 = 4.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The which-key popup of a window. The app owns it and tells it the
/// pending prefix after every key.
#[derive(Clone, Debug)]
pub struct WhichKey {
    /// How long a prefix waits before the popup shows.
    delay: Duration,
    /// Where each entry shown was drawn last, by index, and the whole box.
    drawn: Option<DrawnPopup>,
    /// What may follow the prefix.
    entries: Vec<WhichKeyEntry>,
    /// The header the app named, in place of the prefix's keys, while it
    /// shows the popup itself.
    header: Option<String>,
    /// The keys typed so far, empty when nothing is pending.
    prefix: Vec<Key>,
    /// Whether the popup is showing.
    shown: bool,
    /// When the prefix started waiting, or `None` when nothing is pending.
    since: Option<Instant>,
}

/// One key that may follow the prefix, and what it does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WhichKeyEntry {
    /// Whether more keys may follow this one.
    continues: bool,
    /// The key, as misty names keys or as the app wrote it.
    key: EntryKey,
    /// What typing the key runs, or what it leads to.
    title: String,
}

/// Where the popup was drawn, for the pointer.
#[derive(Clone, Debug, PartialEq)]
struct DrawnPopup {
    /// Each entry shown, by index.
    entries: Vec<Rect>,
    /// The whole box.
    rect: Rect,
}

/// What became of an input offered to a [`WhichKey`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WhichKeyResponse {
    /// The entry at this index was clicked: act as if its key were typed.
    Activated(usize),
    /// A click landed outside the popup while it showed: hide it.
    Closed,
    /// The input was the popup's.
    Handled,
    /// The input was not the popup's.
    Ignored,
}

/// An entry's key.
#[derive(Clone, Debug, Eq, PartialEq)]
enum EntryKey {
    /// A key misty names.
    Key(Key),
    /// A key as the app writes it.
    Written(String),
}

/// How the entries are spread over the popup, in cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EntryGrid {
    /// The width of one column, gap excluded.
    column_width: usize,
    /// How many columns there are.
    columns: usize,
    /// How many rows each column has.
    rows: usize,
    /// How many entries fit; the rest are left out.
    shown: usize,
}

// ========================================================================
// WhichKey Implementations
// ========================================================================

impl WhichKey {
    /// A hidden popup showing after half a second of waiting.
    pub fn new() -> Self {
        Self {
            delay: DEFAULT_DELAY,
            drawn: None,
            entries: Vec::new(),
            header: None,
            prefix: Vec::new(),
            shown: false,
            since: None,
        }
    }

    /// This popup showing after `delay` of waiting.
    pub fn with_delay(self, delay: Duration) -> Self {
        Self { delay, ..self }
    }

    /// Tells the popup the prefix the app's resolver waits on and what may
    /// follow it. A new prefix restarts the delay, or replaces the entries
    /// at once while the popup shows; the same prefix again changes
    /// nothing; an empty prefix (resolved or reset) hides the popup.
    /// Returns whether what is drawn changed, when the app should redraw.
    pub fn set_pending(&mut self, prefix: &[Key], entries: Vec<WhichKeyEntry>) -> bool {
        self.set_pending_at(prefix, entries, Instant::now())
    }

    /// Shows the popup at once, headed `header`, listing `entries`: for an
    /// app that times the hint itself and names its prefix in its own
    /// words. [`WhichKey::reset`] hides it. Whether what is drawn changed.
    pub fn show(&mut self, header: impl Into<String>, entries: Vec<WhichKeyEntry>) -> bool {
        let header = Some(header.into());
        let changed = !self.shown || self.header != header || self.entries != entries;
        self.header = header;
        self.entries = entries;
        self.prefix.clear();
        self.shown = true;
        self.since = None;
        changed
    }

    /// Hides the popup and forgets the prefix; whether it was showing.
    pub fn reset(&mut self) -> bool {
        self.set_pending(&[], Vec::new())
    }

    /// Whether the popup is showing.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// [`WhichKey::set_pending`] at `now`.
    fn set_pending_at(
        &mut self,
        prefix: &[Key],
        entries: Vec<WhichKeyEntry>,
        now: Instant,
    ) -> bool {
        if prefix.is_empty() {
            let was_shown = self.shown;
            self.header = None;
            self.entries.clear();
            self.prefix.clear();
            self.shown = false;
            self.since = None;
            return was_shown;
        }
        if prefix == self.prefix {
            return false;
        }
        self.prefix = prefix.to_vec();
        self.header = None;
        self.entries = entries;
        if !self.shown {
            self.since = Some(now);
        }
        self.shown
    }

    /// Shows the popup if a prefix has waited its delay by `now`; whether
    /// it did.
    fn reveal(&mut self, now: Instant) -> bool {
        let due = self.next_deadline().is_some_and(|deadline| deadline <= now);
        if due {
            self.shown = true;
        }
        due
    }

    /// When the popup is due to show, or `None` when it is showing or
    /// nothing is pending.
    fn next_deadline(&self) -> Option<Instant> {
        match self.shown {
            true => None,
            false => self.since.map(|since| since + self.delay),
        }
    }
}

impl Widget for WhichKey {
    type Output = ();
    type Response = WhichKeyResponse;

    /// Answers a left press while the popup shows: on an entry, which one;
    /// elsewhere in the popup, nothing more; outside it, that it should go.
    fn handle(&mut self, input: &Input, _cx: &mut EventCx<'_>) -> WhichKeyResponse {
        let (true, Some(drawn)) = (self.shown, &self.drawn) else {
            return WhichKeyResponse::Ignored;
        };
        let Input::PointerButton(button) = input else {
            return WhichKeyResponse::Ignored;
        };
        if !button.pressed || button.button != PointerButton::Left {
            return WhichKeyResponse::Ignored;
        }
        let point = button.position;
        match drawn.entries.iter().position(|rect| rect.contains(point)) {
            Some(index) => WhichKeyResponse::Activated(index),
            None if drawn.rect.contains(point) => WhichKeyResponse::Handled,
            None => WhichKeyResponse::Closed,
        }
    }

    /// Draws the popup along the bottom of `area`, in the overlay, when it
    /// is showing.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if !self.shown {
            self.drawn = None;
            return;
        }
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let inset = Popup::inset(&theme.shape, scale_factor);
        let padding_y = logical(PADDING_Y, scale_factor);
        let room = metrics
            .cols_in(area.width - 2.0 * inset)
            .saturating_sub(2 * EDGE_CELLS);
        let max_rows = metrics
            .rows_in(area.height * MAX_HEIGHT_SHARE - 2.0 * (inset + padding_y))
            .saturating_sub(HEADER_ROWS);
        let key_width = self
            .entries
            .iter()
            .map(|entry| text::width(&entry.key_label(&theme.strings)))
            .max()
            .unwrap_or(0);
        let title_width = self
            .entries
            .iter()
            .map(|entry| entry.title_width())
            .max()
            .unwrap_or(0)
            .min(MAX_TITLE_CELLS);
        let grid = EntryGrid::new(
            self.entries.len(),
            key_width + KEY_GAP_CELLS + title_width,
            room,
            max_rows,
        );
        let rows = HEADER_ROWS + grid.rows;
        let height = (rows as f32 * metrics.height + 2.0 * (inset + padding_y)).ceil();
        let rect = Rect::new(area.x, area.y + area.height - height, area.width, height);
        let popup = Popup::with_rect(rect);
        popup.draw(frame, theme);
        let content = popup.content(&theme.shape, scale_factor);
        let left = content.x + EDGE_CELLS as f32 * metrics.width;
        let top = content.y + padding_y;
        let named = match &self.header {
            Some(header) => header.clone(),
            None => key_label(&self.prefix, &theme.strings),
        };
        let mut header = StyledText::from(Span::new(named, ThemeToken::Accent.into()).bold());
        let hidden = self.entries.len() - grid.shown;
        if hidden > 0 {
            let note = strings::fill(&theme.strings.which_key_hidden, &[&hidden.to_string()]);
            header.push(Span::new(
                format!("{HIDDEN_GAP}{note}"),
                ThemeToken::Muted.into(),
            ));
        }
        let mut entry_rects = Vec::new();
        frame.overlay(|frame| {
            frame.draw_text(&header, Point::new(left, top), content, theme);
            for (index, entry) in self.entries.iter().take(grid.shown).enumerate() {
                let (column, row) = grid.position(index);
                let x =
                    left + (column * (grid.column_width + COLUMN_GAP_CELLS)) as f32 * metrics.width;
                let y = top + (HEADER_ROWS + row) as f32 * metrics.height;
                let cell = Rect::new(
                    x,
                    y,
                    grid.column_width as f32 * metrics.width,
                    metrics.height,
                );
                entry_rects.push(cell);
                let key =
                    StyledText::plain(entry.key_label(&theme.strings), ThemeToken::Accent.into());
                frame.draw_text(&key, Point::new(x, y), cell, theme);
                let title_x = x + (key_width + KEY_GAP_CELLS) as f32 * metrics.width;
                frame.draw_text(&entry.title_text(), Point::new(title_x, y), cell, theme);
            }
        });
        self.drawn = Some(DrawnPopup {
            entries: entry_rects,
            rect,
        });
    }

    /// Shows the popup when its delay is up, asking `cx` for a redraw of
    /// its window when it did, and returns when it will be due, for the app's
    /// [`Application::about_to_wait`](crate::Application::about_to_wait)
    /// to hand back.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        if self.reveal(Instant::now()) {
            cx.request_redraw();
        }
        self.next_deadline()
    }
}

impl Outcome for WhichKeyResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

impl Default for WhichKey {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// WhichKeyEntry Implementations
// ========================================================================

impl WhichKeyEntry {
    /// An entry for `key`, which runs `title` and, when `continues`, may
    /// also be followed by more keys.
    pub fn new(key: Key, title: impl Into<String>, continues: bool) -> Self {
        Self {
            continues,
            key: EntryKey::Key(key),
            title: title.into(),
        }
    }

    /// An entry for the key the app writes as `key` (`C-x`, `SPC`), which
    /// runs `title` and, when `continues`, may be followed by more keys.
    pub fn written(key: impl Into<String>, title: impl Into<String>, continues: bool) -> Self {
        Self {
            continues,
            key: EntryKey::Written(key.into()),
            title: title.into(),
        }
    }

    /// The key as the popup writes it, its modifiers in `strings`' words.
    fn key_label(&self, strings: &Strings) -> String {
        match &self.key {
            EntryKey::Key(key) => key_label(&[*key], strings),
            EntryKey::Written(written) => written.clone(),
        }
    }

    /// The title as the popup draws it: marked when more keys follow, cut
    /// to [`MAX_TITLE_CELLS`].
    fn title_text(&self) -> StyledText {
        let text = match self.continues {
            true => StyledText::new()
                .with(Span::new(MORE_KEYS_MARK, ThemeToken::Accent.into()))
                .with(Span::new(self.title.as_str(), ThemeToken::Muted.into())),
            false => StyledText::plain(self.title.as_str(), ThemeToken::Foreground.into()),
        };
        text.with_ellipsis().fit(MAX_TITLE_CELLS)
    }

    /// The cells the title takes, its mark included.
    fn title_width(&self) -> usize {
        let mark = match self.continues {
            true => text::width(MORE_KEYS_MARK),
            false => 0,
        };
        mark + text::width(&self.title)
    }
}

// ========================================================================
// EntryGrid Implementations
// ========================================================================

impl EntryGrid {
    /// The grid of `count` entries `width` cells wide in `room` cells
    /// across and at most `max_rows` rows: as many columns as fit (at least
    /// one, at most one per entry), the fewest rows that hold every entry,
    /// and the entries past `max_rows` full rows left out.
    fn new(count: usize, width: usize, room: usize, max_rows: usize) -> Self {
        let fitting = (room + COLUMN_GAP_CELLS) / (width + COLUMN_GAP_CELLS);
        let columns = fitting.clamp(1, count.max(1));
        let rows = count.div_ceil(columns).min(max_rows.max(1));
        Self {
            column_width: width,
            columns,
            rows,
            shown: count.min(rows * columns),
        }
    }

    /// The column and row of entry `index`, filling each column top to
    /// bottom before the next.
    fn position(&self, index: usize) -> (usize, usize) {
        (index / self.rows, index % self.rows)
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
    use crate::base::key::{KeyCode, Mods};
    use crate::base::theme::Shape;

    fn key(c: char) -> Key {
        Key::new(KeyCode::Char(c), Mods::NONE)
    }

    fn entries(count: usize) -> Vec<WhichKeyEntry> {
        ('a'..)
            .take(count)
            .map(|c| WhichKeyEntry::new(key(c), "run", false))
            .collect()
    }

    #[test]
    fn test_entries_fill_as_many_columns_as_fit_top_to_bottom() {
        // 10 wide plus a 3-cell gap: 3 columns fit in 36 cells.
        let grid = EntryGrid::new(7, 10, 36, 20);
        assert_eq!((grid.columns, grid.rows, grid.shown), (3, 3, 7));
        assert_eq!(grid.position(2), (0, 2));
        assert_eq!(grid.position(3), (1, 0));
        assert_eq!(grid.position(6), (2, 0));
    }

    #[test]
    fn test_a_short_area_leaves_out_what_does_not_fit_and_a_narrow_one_keeps_a_column() {
        let grid = EntryGrid::new(10, 10, 23, 2);
        assert_eq!((grid.columns, grid.rows, grid.shown), (2, 2, 4));
        let narrow = EntryGrid::new(3, 10, 4, 0);
        assert_eq!((narrow.columns, narrow.rows, narrow.shown), (1, 1, 1));
        let few = EntryGrid::new(2, 10, 200, 20);
        assert_eq!(few.columns, 2, "no empty columns");
    }

    #[test]
    fn test_an_app_shows_its_own_header_and_written_keys_at_once() {
        let mut which = WhichKey::new();
        let entries = vec![WhichKeyEntry::written("SPC f", "Find file", false)];
        assert!(which.show("Leader", entries.clone()));
        assert!(which.is_shown(), "no delay");
        assert!(
            !which.show("Leader", entries),
            "the same again changes nothing"
        );
        let mut frame = crate::app::offscreen::plain_frame(800, 600);
        let bounds = frame.bounds();
        which.draw(&mut frame, bounds, &Theme::dark());
        let text = crate::testing::drawn_text(&frame).join("|");
        assert!(text.contains("Leader") && text.contains("SPC f"), "{text}");
        let entry = which.drawn.as_ref().expect("drawn").entries[0];
        let mut window = crate::testing::TestWindow::new(800, 600);
        let press = |x: f32, y: f32| {
            Input::PointerButton(crate::input::event::ButtonInput::press(
                PointerButton::Left,
                Point::new(x, y),
            ))
        };
        let click = |which: &mut WhichKey, window: &mut crate::testing::TestWindow, x, y| {
            window.send(&press(x, y), |input, cx| which.handle(input, cx))
        };
        assert_eq!(
            click(&mut which, &mut window, entry.x + 1.0, entry.y + 1.0),
            WhichKeyResponse::Activated(0)
        );
        assert_eq!(
            click(&mut which, &mut window, 5.0, 5.0),
            WhichKeyResponse::Closed
        );
        assert!(which.reset());
        assert!(!which.is_shown());
        assert_eq!(
            click(&mut which, &mut window, 5.0, 5.0),
            WhichKeyResponse::Ignored
        );
    }

    #[test]
    fn test_the_popup_shows_after_the_delay_and_hides_when_the_prefix_resolves() {
        let mut which_key = WhichKey::new();
        let now = Instant::now();
        assert!(!which_key.set_pending_at(&[key('g')], entries(2), now));
        assert_eq!(which_key.next_deadline(), Some(now + DEFAULT_DELAY));
        assert!(!which_key.reveal(now + DEFAULT_DELAY / 2));
        // The same prefix again does not restart the delay.
        which_key.set_pending_at(&[key('g')], entries(2), now + DEFAULT_DELAY / 2);
        assert!(which_key.reveal(now + DEFAULT_DELAY));
        assert!(which_key.is_shown());
        assert_eq!(which_key.next_deadline(), None);
        assert!(which_key.set_pending_at(&[], Vec::new(), now + DEFAULT_DELAY));
        assert!(!which_key.is_shown());
        assert_eq!(which_key.next_deadline(), None);
    }

    #[test]
    fn test_a_longer_prefix_switches_at_once_while_shown_and_waits_while_hidden() {
        let mut which_key = WhichKey::new();
        let now = Instant::now();
        which_key.set_pending_at(&[key('g')], entries(2), now);
        which_key.reveal(now + DEFAULT_DELAY);
        let later = now + 2 * DEFAULT_DELAY;
        assert!(which_key.set_pending_at(&[key('g'), key('z')], entries(1), later));
        assert!(which_key.is_shown());
        assert_eq!(which_key.entries.len(), 1);
        which_key.reset();
        which_key.set_pending_at(&[key('z')], entries(1), later);
        assert!(!which_key.is_shown());
        assert_eq!(which_key.next_deadline(), Some(later + DEFAULT_DELAY));
    }

    #[test]
    fn test_a_long_title_is_cut_to_its_cells_ending_in_an_ellipsis() {
        let long = "x".repeat(MAX_TITLE_CELLS + 8);
        let title = WhichKeyEntry::new(key('a'), long, false).title_text();
        assert_eq!(title.width(), MAX_TITLE_CELLS);
        let last = title.spans().last().expect("a span");
        assert!(last.text.ends_with(text::ELLIPSIS), "{last:?}");
    }

    #[test]
    fn test_a_shown_popup_paints_a_bordered_box_along_the_bottom_with_accent_keys() {
        let theme = Theme::dark();
        let mut which_key = WhichKey::new();
        let now = Instant::now();
        let mut entries = entries(3);
        entries.push(WhichKeyEntry::new(key('W'), "MMMMMMMM", false));
        which_key.set_pending_at(&[key('g')], entries, now);
        which_key.reveal(now + DEFAULT_DELAY);
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let Some(snapshot) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            which_key.draw(frame, area, &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        snapshot.assert_pixel(200.0, 1.0, theme.background, "above the popup");
        snapshot.assert_pixel(200.0, 299.0, theme.border, "the bottom border");
        snapshot.assert_pixel(0.0, 290.0, theme.border, "the left border");
        snapshot.assert_pixel(399.0, 290.0, theme.border, "the right border");
        // The header row, then one row of four entries in four columns.
        let inset = Popup::inset(&Shape::GRID, 1.0) + logical(PADDING_Y, 1.0);
        let entry_row = Rect::new(0.0, 300.0 - inset - metrics.height, 400.0, metrics.height);
        assert!(snapshot.contains_color(entry_row, theme.accent), "the keys");
        assert!(
            snapshot.contains_color(entry_row, theme.foreground),
            "the titles"
        );
    }
}

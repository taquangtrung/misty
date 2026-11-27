//! The keys of every list-like widget: [`List`](super::List),
//! [`Table`](super::Table), [`TreeView`](super::TreeView), and
//! [`IconGrid`](super::IconGrid) read a key through [`ListKeys`], so they
//! all move alike. Every app's lists take the platform's keys: the arrows,
//! Page Up and Down, Home and End, Space, Enter, and Shift with a motion
//! key. With the app's [`KeyScheme`] taking the Vim keys they move as Vim
//! does over lines, and with the Emacs keys `C-n` and `C-p` move a row:
//!
//! - Down or `j` or `C-n`, Up or `k` or `C-p`: a row (a cell row in a
//!   grid); `g j` and `g k` too, a row being a screen line.
//! - `g g` and `G`: the first and the last row, or the count's row
//!   (`10G`, `10gg`); Home and End go to the ends whatever the count.
//! - `C-f` or Page Down, `C-b` or Page Up: a screen less two rows, the
//!   cursor dragged into sight; when the view cannot scroll, the cursor
//!   goes to the last or first row.
//! - `C-d` and `C-u`: scroll half a screen and move the cursor as far; a
//!   count, held to a screen, sets that amount from then on, as Vim's
//!   `scroll` option, until the screen changes height.
//! - `C-e` and `C-y`: scroll a row without moving the cursor, unless it
//!   would leave the screen.
//! - `H`, `M`, `L`: the row at the top, middle, or bottom of the screen
//!   (`H` and `L` the count's row from their edge), inside the scroll
//!   margin unless the list's end is in sight.
//! - `z t` (or `z Enter`), `z z` (`z .`), `z b` (`z -`): scroll the
//!   cursor's row to the top, middle, or bottom, the cursor staying put;
//!   with a count, first to the count's row. The view never scrolls past
//!   the last row, so near the end the row stays lower.
//! - Left or `h`, Right or `l`: sideways, which each widget reads its own
//!   way (a tree collapses and expands, a grid moves a cell); `0` and `$`
//!   go to the start and end of a line where a widget has lines (a grid's
//!   row, a table's columns).
//! - Space toggles the cursor's selection; Enter activates it.
//! - Shift with Down, Up, Page Down, Page Up, Home, or End moves as the
//!   key alone does and extends the selection from its anchor (the row
//!   the cursor was on when the range began) to the cursor, where a
//!   widget selects rows (a list, a table, a tree).
//!
//! A count is typed before a motion (`5j`, `3 C-d`); `0` only continues a
//! count. `g` and `z` wait, with no timeout, for the key that completes
//! them. Esc drops a pending count or prefix; any other key that does not
//! complete the prefix drops it and is swallowed, as Vim swallows it,
//! unless it is an app accelerator (a chord with Alt, Super, or Ctrl and
//! Shift), Tab, or a function key, which drop it and pass to the app. A
//! count before a key that is not a list key is dropped and the key
//! passes to the app. As in Vim, a press of any mouse button or a wheel
//! scroll over the widget drops a pending count or prefix, and so does
//! the window losing focus. The widgets expose what is pending through
//! [`ListKeys`] for a status bar or a which-key popup.
//!
//! The bare `Ctrl` letters are the vim paging chords and the emacs line
//! keys, never app commands, so a widget may take them while it has the
//! keyboard.

use super::super::base::key::{Key, KeyCode, KeyScheme, Mods};
use super::super::base::list::ListState;
use super::super::base::strings::Strings;
use super::super::input::event::Input;
use super::which_key::WhichKeyEntry;

// ========================================================================
// Constants
// ========================================================================

/// The largest count, as in Vim; further digits leave it there.
const MAX_COUNT: usize = 999_999_999;

/// The base a count is typed in.
const COUNT_RADIX: u32 = 10;

/// The rows of the old screen `C-f` and `C-b` keep in sight.
const PAGE_OVERLAP: usize = 2;

/// The prefix of `g g`, `g j`, and `g k`.
const GO_PREFIX: char = 'g';

/// The prefix of `z t`, `z z`, and `z b`.
const SCROLL_PREFIX: char = 'z';

// ========================================================================
// Data Structures
// ========================================================================

/// The keys typed toward a list motion that has not run yet: a count and
/// a prefix (`g`, `z`), plus the half-page amount a count before `C-d` or
/// `C-u` set. Each list-like widget keeps one; read it through the
/// widget's `keys` to show what is pending in a status bar or a
/// [`WhichKey`](crate::widgets::WhichKey) popup. The widget drops what is pending
/// on a mouse press or wheel scroll over it and when the window loses
/// focus, as Vim does.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListKeys {
    /// The count typed so far.
    count: Option<usize>,
    /// The rows `C-d` and `C-u` scroll, when a count set them.
    half_page: Option<HalfPage>,
    /// The prefix key waiting for the key that completes it.
    prefix: Option<Key>,
    /// Which keyboard conventions it reads keys by.
    scheme: KeyScheme,
}

/// The rows `C-d` and `C-u` scroll, set by a count, and the screen height
/// it was set for; another height drops it, as Vim resets `scroll`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HalfPage {
    /// The rows to scroll.
    rows: usize,
    /// The rows on screen when it was set.
    visible: usize,
}

/// What a key read by [`ListKeys::read`] asks of a widget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Read {
    /// The key started, continued, or dropped a count or prefix; the
    /// widget used it and has nothing more to do.
    Consumed,
    /// Not a list key: the app's.
    Ignored,
    /// A list key, with the count typed before it.
    Key(CountedKey),
}

/// A list key and the count typed before it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CountedKey {
    /// The count, or `None` when none was typed.
    pub(crate) count: Option<usize>,
    /// The key.
    pub(crate) key: ListKey,
}

/// What a key asks of a list-like widget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ListKey {
    /// Activate the cursor's item (Enter).
    Activate,
    /// Move the cursor and extend the selection to it (Shift with a
    /// motion key).
    Extend(Motion),
    /// Go left (Left, `h`).
    Left,
    /// Go to the end of the line (`$`).
    LineEnd,
    /// Go to the start of the line (`0`).
    LineStart,
    /// Move the cursor or the view.
    Move(Motion),
    /// Go right (Right, `l`).
    Right,
    /// Toggle the cursor's selection (Space).
    Toggle,
}

/// Where a key moves a cursor or a view up or down a list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Motion {
    /// A row down per count.
    Down,
    /// To the first row, or the count's.
    First,
    /// Half a screen (or the count's rows) down.
    HalfPageDown,
    /// Half a screen (or the count's rows) up.
    HalfPageUp,
    /// To the last row, or the count's.
    Last,
    /// A screen less two rows down per count.
    PageDown,
    /// A screen less two rows up per count.
    PageUp,
    /// The view moved to show the cursor's row at its bottom.
    PlaceBottom,
    /// The view moved to show the cursor's row in its middle.
    PlaceCenter,
    /// The view moved to show the cursor's row at its top.
    PlaceTop,
    /// To the row at the bottom of the screen.
    ScreenBottom,
    /// To the row in the middle of the screen.
    ScreenMiddle,
    /// To the row at the top of the screen.
    ScreenTop,
    /// The view a row down per count.
    ScrollDown,
    /// The view a row up per count.
    ScrollUp,
    /// A row up per count.
    Up,
}

// ========================================================================
// ListKeys Implementations
// ========================================================================

impl ListKeys {
    /// The count typed so far, waiting for its motion.
    pub fn count(&self) -> Option<usize> {
        self.count
    }

    /// Whether a count or a prefix is waiting for the key that completes
    /// it.
    pub fn is_pending(&self) -> bool {
        self.count.is_some() || self.prefix.is_some()
    }

    /// The prefix typed so far (`g` or `z`), waiting for the key that
    /// completes it; empty when none is pending.
    pub fn pending_keys(&self) -> &[Key] {
        self.prefix.as_slice()
    }

    /// The keys that may follow the pending prefix and what each does, in
    /// `strings`' words (pass the theme's, `&theme.strings`), for
    /// [`WhichKey::set_pending`](crate::widgets::WhichKey::set_pending); empty when
    /// no prefix is pending.
    pub fn hints(&self, strings: &Strings) -> Vec<WhichKeyEntry> {
        let hints: &[(char, &str)] = match self.prefix.map(|prefix| prefix.code) {
            Some(KeyCode::Char(GO_PREFIX)) => &[
                ('g', &strings.list_first_row),
                ('j', &strings.list_row_down),
                ('k', &strings.list_row_up),
            ],
            Some(KeyCode::Char(SCROLL_PREFIX)) => &[
                ('b', &strings.list_row_to_bottom),
                ('t', &strings.list_row_to_top),
                ('z', &strings.list_row_to_middle),
            ],
            Some(_) | None => &[],
        };
        hints
            .iter()
            .map(|&(key, title)| WhichKeyEntry::new(char_key(key), title, false))
            .collect()
    }

    /// Drops the pending count and prefix; whether there were any. Call it
    /// when the widget loses the keyboard to another widget, so the next
    /// key starts fresh; the widget drops them itself on a mouse press or
    /// wheel scroll over it and when the window loses focus.
    pub fn reset(&mut self) -> bool {
        let pending = self.is_pending();
        self.count = None;
        self.prefix = None;
        pending
    }

    /// Drops the pending count and prefix when `input` ends them as Vim
    /// does: a press of any pointer button or a wheel scroll while the
    /// pointer is `over` the widget, or the window losing focus; whether
    /// there were any, so the widget can redraw a status line.
    pub(crate) fn reset_on(&mut self, input: &Input, over: bool) -> bool {
        let ends = match input {
            Input::PointerButton(button) => button.pressed && over,
            Input::Wheel(_) => over,
            Input::FocusLost => true,
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerLeft
            | Input::PointerMoved(_) => false,
        };
        ends && self.reset()
    }

    /// Reads keys by `scheme` from now on: the app's, which its widget
    /// takes from the context of each input.
    pub(crate) fn set_scheme(&mut self, scheme: KeyScheme) {
        self.scheme = scheme;
    }

    /// Reads `key`, as the module documentation lists.
    pub(crate) fn read(&mut self, key: Key) -> Read {
        if let Some(prefix) = self.prefix.take() {
            let count = self.count.take();
            return match continuation(prefix, key) {
                Some(list_key) => counted(count, list_key),
                None if is_app_key(key) => Read::Ignored,
                None => Read::Consumed,
            };
        }
        let plain = key.mods == Mods::NONE;
        if let (KeyCode::Char(c), true, true) = (key.code, plain, self.scheme.vim) {
            match (c.to_digit(COUNT_RADIX), self.count) {
                (Some(0), None) | (None, _) => {}
                (Some(digit), count) => {
                    let count = count.unwrap_or(0).saturating_mul(COUNT_RADIX as usize);
                    self.count = Some(count.saturating_add(digit as usize).min(MAX_COUNT));
                    return Read::Consumed;
                }
            }
            if let GO_PREFIX | SCROLL_PREFIX = c {
                self.prefix = Some(key);
                return Read::Consumed;
            }
        }
        let count = self.count.take();
        if key.code == KeyCode::Esc && plain && count.is_some() {
            return Read::Consumed;
        }
        let count = count.filter(|_| !matches!(key.code, KeyCode::Home | KeyCode::End));
        match list_key(key).filter(|_| takes(self.scheme, key)) {
            Some(list_key) => counted(count, list_key),
            None => Read::Ignored,
        }
    }

    /// Moves the cursor or the view of `state` by `motion`, `count` times
    /// or to the count's row, as the module documentation lists.
    pub(crate) fn move_cursor(
        &mut self,
        state: &mut ListState,
        motion: Motion,
        count: Option<usize>,
    ) {
        let times = count.unwrap_or(1);
        let rows = state.visible_rows();
        let visible = rows.len();
        let last = state.count().saturating_sub(1);
        match motion {
            Motion::Down => state.move_cursor(signed(times)),
            Motion::First => state.set_cursor(count.map_or(0, |count| count - 1)),
            Motion::HalfPageDown => {
                if state.cursor() < last {
                    let rows = self.half_page_rows(count, visible);
                    scroll_with_cursor(state, signed(rows));
                }
            }
            Motion::HalfPageUp => {
                if state.cursor() > 0 {
                    let rows = self.half_page_rows(count, visible);
                    scroll_with_cursor(state, -signed(rows));
                }
            }
            Motion::Last => state.set_cursor(count.map_or(usize::MAX, |count| count - 1)),
            Motion::PageDown => {
                if !scroll_pages(state, signed(times)) {
                    state.set_cursor(last);
                }
            }
            Motion::PageUp => {
                if !scroll_pages(state, -signed(times)) {
                    state.set_cursor(0);
                }
            }
            Motion::PlaceBottom => place_cursor_row(state, count, |cursor, margin| {
                (cursor + margin + 1).saturating_sub(visible)
            }),
            Motion::PlaceCenter => place_cursor_row(state, count, |cursor, _| {
                cursor.saturating_sub(visible.saturating_sub(1) / 2)
            }),
            Motion::PlaceTop => {
                place_cursor_row(state, count, |cursor, margin| cursor.saturating_sub(margin));
            }
            Motion::ScreenBottom => {
                let kept = if rows.end == state.count() {
                    0
                } else {
                    state.margin()
                };
                let row = rows
                    .end
                    .saturating_sub(1)
                    .saturating_sub((times - 1).max(kept));
                state.set_cursor(row.max(rows.start));
            }
            Motion::ScreenMiddle => state.set_cursor(rows.start + visible.saturating_sub(1) / 2),
            Motion::ScreenTop => {
                let kept = if rows.start == 0 { 0 } else { state.margin() };
                let row = rows.start + (times - 1).max(kept);
                state.set_cursor(row.min(rows.end.saturating_sub(1)));
            }
            Motion::ScrollDown => state.scroll_by(signed(times)),
            Motion::ScrollUp => state.scroll_by(-signed(times)),
            Motion::Up => state.move_cursor(-signed(times)),
        }
    }

    /// The rows `C-d` and `C-u` scroll on a screen of `visible` rows: the
    /// count's, held to a screen as Vim holds `scroll`, which then stay, or
    /// half the screen.
    fn half_page_rows(&mut self, count: Option<usize>, visible: usize) -> usize {
        if let Some(count) = count {
            let rows = count.min(visible.max(1));
            self.half_page = Some(HalfPage { rows, visible });
        }
        match self.half_page {
            Some(half_page) if half_page.visible == visible => half_page.rows,
            Some(_) | None => {
                self.half_page = None;
                (visible / 2).max(1)
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What `key` asks of a list-like widget on its own, with no count or
/// prefix before it, or `None` when it is not a list key.
fn list_key(key: Key) -> Option<ListKey> {
    let plain = key.mods == Mods::NONE;
    let ctrl = key.mods == Mods::CTRL;
    let shift = key.mods == Mods::SHIFT;
    let list_key = match key.code {
        KeyCode::Down if shift => ListKey::Extend(Motion::Down),
        KeyCode::Up if shift => ListKey::Extend(Motion::Up),
        KeyCode::PageDown if shift => ListKey::Extend(Motion::PageDown),
        KeyCode::PageUp if shift => ListKey::Extend(Motion::PageUp),
        KeyCode::Home if shift => ListKey::Extend(Motion::First),
        KeyCode::End if shift => ListKey::Extend(Motion::Last),
        KeyCode::Down | KeyCode::Char('j') if plain => ListKey::Move(Motion::Down),
        KeyCode::Char('n') if ctrl => ListKey::Move(Motion::Down),
        KeyCode::Up | KeyCode::Char('k') if plain => ListKey::Move(Motion::Up),
        KeyCode::Char('p') if ctrl => ListKey::Move(Motion::Up),
        KeyCode::PageDown if plain => ListKey::Move(Motion::PageDown),
        KeyCode::Char('f') if ctrl => ListKey::Move(Motion::PageDown),
        KeyCode::PageUp if plain => ListKey::Move(Motion::PageUp),
        KeyCode::Char('b') if ctrl => ListKey::Move(Motion::PageUp),
        KeyCode::Char('d') if ctrl => ListKey::Move(Motion::HalfPageDown),
        KeyCode::Char('u') if ctrl => ListKey::Move(Motion::HalfPageUp),
        KeyCode::Char('e') if ctrl => ListKey::Move(Motion::ScrollDown),
        KeyCode::Char('y') if ctrl => ListKey::Move(Motion::ScrollUp),
        KeyCode::Home if plain => ListKey::Move(Motion::First),
        KeyCode::End | KeyCode::Char('G') if plain => ListKey::Move(Motion::Last),
        KeyCode::Char('H') if plain => ListKey::Move(Motion::ScreenTop),
        KeyCode::Char('M') if plain => ListKey::Move(Motion::ScreenMiddle),
        KeyCode::Char('L') if plain => ListKey::Move(Motion::ScreenBottom),
        KeyCode::Left | KeyCode::Char('h') if plain => ListKey::Left,
        KeyCode::Right | KeyCode::Char('l') if plain => ListKey::Right,
        KeyCode::Char('0') if plain => ListKey::LineStart,
        KeyCode::Char('$') if plain => ListKey::LineEnd,
        KeyCode::Char(' ') if plain => ListKey::Toggle,
        KeyCode::Enter if plain => ListKey::Activate,
        KeyCode::Backspace
        | KeyCode::Char(_)
        | KeyCode::Delete
        | KeyCode::Down
        | KeyCode::End
        | KeyCode::Enter
        | KeyCode::Esc
        | KeyCode::F(_)
        | KeyCode::Home
        | KeyCode::Insert
        | KeyCode::Left
        | KeyCode::PageDown
        | KeyCode::PageUp
        | KeyCode::Right
        | KeyCode::Tab
        | KeyCode::Up => return None,
    };
    Some(list_key)
}

/// Whether a list reading keys by `scheme` takes `key`: a letter or a
/// Ctrl chord only with the Vim keys, but `C-n` and `C-p` with the Emacs
/// keys; the platform's keys always.
fn takes(scheme: KeyScheme, key: Key) -> bool {
    match key.code {
        KeyCode::Char(' ') if key.mods == Mods::NONE => true,
        KeyCode::Char('n' | 'p') if key.mods == Mods::CTRL => scheme.emacs,
        KeyCode::Char(_) => scheme.vim,
        _ => true,
    }
}

/// What `key` completes after `prefix`, or `None` when it completes
/// nothing.
fn continuation(prefix: Key, key: Key) -> Option<ListKey> {
    if key.mods != Mods::NONE {
        return None;
    }
    let motion = match (prefix.code, key.code) {
        (KeyCode::Char(GO_PREFIX), KeyCode::Char('g')) => Motion::First,
        (KeyCode::Char(GO_PREFIX), KeyCode::Char('j') | KeyCode::Down) => Motion::Down,
        (KeyCode::Char(GO_PREFIX), KeyCode::Char('k') | KeyCode::Up) => Motion::Up,
        (KeyCode::Char(SCROLL_PREFIX), KeyCode::Char('t') | KeyCode::Enter) => Motion::PlaceTop,
        (KeyCode::Char(SCROLL_PREFIX), KeyCode::Char('z' | '.')) => Motion::PlaceCenter,
        (KeyCode::Char(SCROLL_PREFIX), KeyCode::Char('b' | '-')) => Motion::PlaceBottom,
        _ => return None,
    };
    Some(ListKey::Move(motion))
}

/// Whether `key` belongs to the app even while a prefix waits: an
/// accelerator chord, Tab, or a function key.
fn is_app_key(key: Key) -> bool {
    let accelerator =
        key.mods.intersects(Mods::ALT | Mods::SUPER) || key.mods.contains(Mods::CTRL | Mods::SHIFT);
    accelerator || matches!(key.code, KeyCode::Tab | KeyCode::F(_))
}

/// Scrolls the view of `state` `rows` down (up when negative) and moves
/// the cursor as far, or further when the scroll dragged it further to
/// keep the margin, as `C-d` and `C-u` do.
fn scroll_with_cursor(state: &mut ListState, rows: isize) {
    let target = state.cursor().saturating_add_signed(rows);
    state.scroll_by(rows);
    let dragged = state.cursor();
    state.set_cursor(match rows > 0 {
        true => target.max(dragged),
        false => target.min(dragged),
    });
}

/// Scrolls the view of `state` `pages` screens less [`PAGE_OVERLAP`] rows
/// down (up when negative), dragging the cursor into sight; whether the
/// view moved.
fn scroll_pages(state: &mut ListState, pages: isize) -> bool {
    let page = state
        .visible_rows()
        .len()
        .saturating_sub(PAGE_OVERLAP)
        .max(1);
    let before = state.scroll();
    state.scroll_by(pages.saturating_mul(signed(page)));
    state.scroll() != before
}

/// Scrolls the view of `state` to start at the row `top` gives for the
/// cursor's row and the scroll margin, after moving the cursor to the
/// count's row when there is a count.
fn place_cursor_row(
    state: &mut ListState,
    count: Option<usize>,
    top: impl Fn(usize, usize) -> usize,
) {
    if let Some(count) = count {
        state.set_cursor(count - 1);
    }
    let top = top(state.cursor(), state.margin());
    state.scroll_by(signed(top) - signed(state.scroll()));
}

/// `key` as read with `count`.
fn counted(count: Option<usize>, key: ListKey) -> Read {
    Read::Key(CountedKey { count, key })
}

/// The unmodified key that types `c`.
fn char_key(c: char) -> Key {
    Key::new(KeyCode::Char(c), Mods::NONE)
}

/// `rows` as a signed row delta; counts are held to [`MAX_COUNT`], so it
/// fits.
fn signed(rows: usize) -> isize {
    isize::try_from(rows).unwrap_or(isize::MAX)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::geometry::Point;
    use crate::base::key::parse_key_sequence;
    use crate::input::event::{ButtonInput, PointerButton, WheelDelta, WheelInput};

    /// A press of `button`, or its release when not `pressed`.
    fn mouse(pressed: bool, button: PointerButton) -> Input {
        let press = ButtonInput::press(button, Point::new(4.0, 4.0));
        Input::PointerButton(ButtonInput { pressed, ..press })
    }

    /// A reader taking the Vim and Emacs keys, as most tests here read.
    fn vim_keys() -> ListKeys {
        let mut keys = ListKeys::default();
        keys.set_scheme(KeyScheme::VIM_AND_EMACS);
        keys
    }

    #[test]
    fn test_the_standard_scheme_takes_the_platforms_keys_and_leaves_letters_to_the_app() {
        let read = |spec: &str| {
            let mut keys = ListKeys::default();
            *reads(&mut keys, spec).last().expect("a key")
        };
        assert_eq!(read("<down>"), moved(None, Motion::Down));
        assert_eq!(read("<pagedown>"), moved(None, Motion::PageDown));
        assert_eq!(
            read("S-<down>"),
            Read::Key(CountedKey {
                count: None,
                key: ListKey::Extend(Motion::Down)
            })
        );
        assert_eq!(
            read("<space>"),
            Read::Key(CountedKey {
                count: None,
                key: ListKey::Toggle
            })
        );
        for letters in ["j", "G", "g", "5", "C-d", "C-n"] {
            assert_eq!(read(letters), Read::Ignored, "{letters} is the app's");
        }
        // The Emacs keys alone give `C-n` and `C-p`, not the Vim letters.
        let mut emacs = ListKeys::default();
        emacs.set_scheme(KeyScheme {
            emacs: true,
            vim: false,
        });
        assert_eq!(reads(&mut emacs, "C-n")[0], moved(None, Motion::Down));
        assert_eq!(reads(&mut emacs, "j")[0], Read::Ignored);
    }

    /// A reader with the count 5 and the prefix `g` waiting.
    fn pending() -> ListKeys {
        let mut keys = vim_keys();
        reads(&mut keys, "5 g");
        keys
    }

    /// What each key of `spec` (keys separated by spaces) reads as, in
    /// order, through `keys`.
    fn reads(keys: &mut ListKeys, spec: &str) -> Vec<Read> {
        parse_key_sequence(spec)
            .expect("valid keys")
            .into_iter()
            .map(|key| keys.read(key))
            .collect()
    }

    /// The last thing the keys of `spec` read as, through a fresh reader.
    fn last_read(spec: &str) -> Read {
        *reads(&mut vim_keys(), spec).last().expect("a key")
    }

    /// A read of `motion` with `count`.
    fn moved(count: Option<usize>, motion: Motion) -> Read {
        counted(count, ListKey::Move(motion))
    }

    /// A list of `count` rows, `visible` of them on screen, with the
    /// default margin of 2.
    fn list(count: usize, visible: usize) -> ListState {
        let mut state = ListState::new(count);
        state.set_visible(visible);
        state
    }

    /// Runs the keys of `spec` on `state` as a widget does, through `keys`.
    fn run(keys: &mut ListKeys, state: &mut ListState, spec: &str) {
        for read in reads(keys, spec) {
            if let Read::Key(CountedKey {
                count,
                key: ListKey::Move(motion),
            }) = read
            {
                keys.move_cursor(state, motion, count);
            }
        }
    }

    /// The first row on screen and the cursor's row.
    fn top_and_cursor(state: &ListState) -> (usize, usize) {
        (state.scroll(), state.cursor())
    }

    #[test]
    fn test_vim_emacs_and_named_keys_agree_on_each_motion() {
        for spec in ["<down>", "j", "C-n", "g j"] {
            assert_eq!(last_read(spec), moved(None, Motion::Down), "{spec}");
        }
        for spec in ["<pagedown>", "C-f"] {
            assert_eq!(last_read(spec), moved(None, Motion::PageDown), "{spec}");
        }
        for spec in ["<end>", "G", "S-g"] {
            assert_eq!(last_read(spec), moved(None, Motion::Last), "{spec}");
        }
        for spec in ["z t", "z <cr>"] {
            assert_eq!(last_read(spec), moved(None, Motion::PlaceTop), "{spec}");
        }
        assert_eq!(last_read("C-e"), moved(None, Motion::ScrollDown));
        assert_eq!(last_read("M"), moved(None, Motion::ScreenMiddle));
        assert_eq!(last_read("h"), counted(None, ListKey::Left));
        assert_eq!(last_read("<space>"), counted(None, ListKey::Toggle));
    }

    #[test]
    fn test_other_chords_and_letters_are_not_list_keys() {
        for spec in ["C-j", "M-n", "C-g", "C-S-<down>", "x", "<esc>", "C-S-n"] {
            assert_eq!(last_read(spec), Read::Ignored, "{spec}");
        }
    }

    #[test]
    fn test_shift_with_a_motion_key_extends_and_keeps_its_count() {
        let extended = |count, motion| counted(count, ListKey::Extend(motion));
        assert_eq!(last_read("S-<down>"), extended(None, Motion::Down));
        assert_eq!(last_read("3 S-<up>"), extended(Some(3), Motion::Up));
        assert_eq!(last_read("S-<pagedown>"), extended(None, Motion::PageDown));
        assert_eq!(
            last_read("4 S-<end>"),
            extended(None, Motion::Last),
            "End goes to the end whatever the count"
        );
    }

    #[test]
    fn test_a_lone_g_waits_and_a_second_g_goes_to_the_top() {
        let mut keys = vim_keys();
        assert_eq!(reads(&mut keys, "g"), [Read::Consumed]);
        assert_eq!(keys.pending_keys(), parse_key_sequence("g").unwrap());
        let hints = keys.hints(&Strings::english());
        assert_eq!(hints.len(), 3, "g g, g j, and g k");
        assert_eq!(reads(&mut keys, "g"), [moved(None, Motion::First)]);
        assert!(keys.pending_keys().is_empty());
        assert!(keys.hints(&Strings::english()).is_empty());
    }

    #[test]
    fn test_a_key_completing_no_prefix_drops_it_and_runs_nothing() {
        let mut keys = vim_keys();
        assert_eq!(
            reads(&mut keys, "g x j"),
            [Read::Consumed, Read::Consumed, moved(None, Motion::Down)]
        );
        assert_eq!(
            reads(&mut keys, "5 z <esc> j"),
            [
                Read::Consumed,
                Read::Consumed,
                Read::Consumed,
                moved(None, Motion::Down)
            ]
        );
        assert_eq!(reads(&mut keys, "g G"), [Read::Consumed, Read::Consumed]);
        assert_eq!(reads(&mut keys, "g M-x"), [Read::Consumed, Read::Ignored]);
        assert_eq!(reads(&mut keys, "g <tab>"), [Read::Consumed, Read::Ignored]);
        assert!(keys.pending_keys().is_empty());
    }

    #[test]
    fn test_digits_build_a_count_and_zero_only_continues_one() {
        assert_eq!(last_read("5 j"), moved(Some(5), Motion::Down));
        assert_eq!(last_read("1 0 G"), moved(Some(10), Motion::Last));
        assert_eq!(last_read("1 0 g g"), moved(Some(10), Motion::First));
        assert_eq!(last_read("0"), counted(None, ListKey::LineStart));
        assert_eq!(last_read("3 <home>"), moved(None, Motion::First));
        let mut keys = vim_keys();
        assert_eq!(reads(&mut keys, "2 0"), [Read::Consumed, Read::Consumed]);
        assert_eq!(keys.count(), Some(20));
        assert_eq!(
            reads(&mut keys, "<esc> j"),
            [Read::Consumed, moved(None, Motion::Down)]
        );
        reads(&mut keys, &["9"; 12].join(" "));
        assert_eq!(keys.count(), Some(MAX_COUNT));
        assert_eq!(reads(&mut keys, "x"), [Read::Ignored]);
        assert_eq!(keys.count(), None, "a key that is not a list key drops it");
    }

    #[test]
    fn test_counted_motions_move_that_many_rows_or_to_that_row() {
        let mut keys = vim_keys();
        let mut state = list(100, 10);
        run(&mut keys, &mut state, "5 j");
        assert_eq!(state.cursor(), 5);
        run(&mut keys, &mut state, "3 k");
        assert_eq!(state.cursor(), 2);
        run(&mut keys, &mut state, "4 0 G");
        assert_eq!(state.cursor(), 39, "row 40, counting from 1");
        run(&mut keys, &mut state, "7 g g");
        assert_eq!(state.cursor(), 6);
        run(&mut keys, &mut state, "5 0 0 G");
        assert_eq!(state.cursor(), 99, "held to the last row");
        run(&mut keys, &mut state, "g g");
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn test_ctrl_e_and_ctrl_y_scroll_dragging_the_cursor_only_into_sight() {
        let mut keys = vim_keys();
        let mut state = list(100, 10);
        run(&mut keys, &mut state, "5 j C-e");
        assert_eq!(top_and_cursor(&state), (1, 5), "the cursor stays");
        run(&mut keys, &mut state, "5 C-e");
        assert_eq!(top_and_cursor(&state), (6, 8), "dragged inside the margin");
        run(&mut keys, &mut state, "C-y");
        assert_eq!(top_and_cursor(&state), (5, 8));
    }

    #[test]
    fn test_ctrl_d_scrolls_and_moves_half_a_screen_and_a_count_sets_the_amount() {
        let mut keys = vim_keys();
        let mut state = list(100, 10);
        run(&mut keys, &mut state, "j C-d");
        assert_eq!(top_and_cursor(&state), (5, 7), "kept inside the margin");
        run(&mut keys, &mut state, "3 C-d");
        assert_eq!(top_and_cursor(&state), (8, 10));
        run(&mut keys, &mut state, "C-u");
        assert_eq!(top_and_cursor(&state), (5, 7), "the count's 3 rows stay");
        state.set_visible(20);
        run(&mut keys, &mut state, "C-d");
        assert_eq!(state.cursor(), 17, "a new height resets it to half");
        run(&mut keys, &mut state, "G C-d");
        assert_eq!(top_and_cursor(&state), (80, 99), "nothing past the end");
    }

    #[test]
    fn test_a_ctrl_d_count_past_the_screen_height_is_held_to_it() {
        let mut keys = vim_keys();
        let mut state = list(100, 10);
        run(&mut keys, &mut state, "2 5 C-d");
        assert_eq!(top_and_cursor(&state), (10, 12), "a screen, not 25 rows");
        run(&mut keys, &mut state, "C-u");
        assert_eq!(top_and_cursor(&state), (0, 2), "the held 10 rows stay");
    }

    #[test]
    fn test_ctrl_f_pages_a_screen_less_two_rows_and_ends_on_the_last_row() {
        let mut keys = vim_keys();
        let mut state = list(30, 10);
        run(&mut keys, &mut state, "C-f");
        assert_eq!(top_and_cursor(&state), (8, 10), "dragged into sight");
        run(&mut keys, &mut state, "2 C-f");
        assert_eq!(top_and_cursor(&state), (20, 22));
        run(&mut keys, &mut state, "C-f");
        assert_eq!(top_and_cursor(&state), (20, 29), "no more to scroll");
        run(&mut keys, &mut state, "C-b");
        assert_eq!(top_and_cursor(&state), (12, 19));
    }

    #[test]
    fn test_h_m_and_l_go_to_the_screen_rows_inside_the_margin() {
        let mut keys = vim_keys();
        let mut state = list(100, 10);
        run(&mut keys, &mut state, "L");
        assert_eq!(top_and_cursor(&state), (0, 7));
        run(&mut keys, &mut state, "H");
        assert_eq!(top_and_cursor(&state), (0, 0), "the list's start in sight");
        run(&mut keys, &mut state, "2 0 j M");
        assert_eq!(top_and_cursor(&state), (13, 17));
        run(&mut keys, &mut state, "H");
        assert_eq!(top_and_cursor(&state), (13, 15));
        run(&mut keys, &mut state, "4 H");
        assert_eq!(top_and_cursor(&state), (13, 16));
        run(&mut keys, &mut state, "4 L");
        assert_eq!(top_and_cursor(&state), (13, 19));
        run(&mut keys, &mut state, "G L");
        assert_eq!(state.cursor(), 99, "the list's end in sight");
    }

    #[test]
    fn test_z_keys_scroll_the_cursor_row_to_the_top_middle_or_bottom() {
        let mut keys = vim_keys();
        let mut state = list(100, 10);
        run(&mut keys, &mut state, "3 0 G z t");
        assert_eq!(top_and_cursor(&state), (27, 29));
        run(&mut keys, &mut state, "z z");
        assert_eq!(top_and_cursor(&state), (25, 29));
        run(&mut keys, &mut state, "z b");
        assert_eq!(top_and_cursor(&state), (22, 29));
        run(&mut keys, &mut state, "5 0 z <cr>");
        assert_eq!(top_and_cursor(&state), (47, 49), "first to row 50");
        run(&mut keys, &mut state, "G z t");
        assert_eq!(top_and_cursor(&state), (90, 99), "never past the end");
    }

    #[test]
    fn test_reset_drops_a_pending_count_and_prefix() {
        let mut keys = vim_keys();
        reads(&mut keys, "3 g");
        assert!(keys.reset());
        assert!(!keys.reset());
        assert_eq!(
            reads(&mut keys, "g x"),
            [Read::Consumed, Read::Consumed],
            "starts afresh"
        );
    }

    #[test]
    fn test_a_press_or_wheel_over_the_widget_or_lost_focus_drops_what_is_pending() {
        let wheel = WheelInput::new(
            WheelDelta::Lines(Point::new(0.0, -1.0)),
            Point::new(4.0, 4.0),
        );
        let over = [
            mouse(true, PointerButton::Left),
            mouse(true, PointerButton::Right),
            mouse(true, PointerButton::Middle),
            Input::Wheel(wheel),
        ];
        for event in over {
            let mut keys = pending();
            assert!(keys.reset_on(&event, true), "{event:?}");
            assert!(!keys.is_pending(), "{event:?}");
        }
        let mut keys = pending();
        assert!(keys.reset_on(&Input::FocusLost, false));
        assert!(!keys.is_pending());
        assert!(!keys.reset_on(&Input::FocusLost, false), "nothing left");
    }

    #[test]
    fn test_a_press_elsewhere_a_release_or_a_move_keeps_what_is_pending() {
        let moved = Input::PointerMoved(Point::new(4.0, 4.0));
        let kept = [
            (mouse(true, PointerButton::Left), false),
            (mouse(false, PointerButton::Left), true),
            (moved, true),
            (Input::FocusGained, true),
        ];
        for (event, over) in kept {
            let mut keys = pending();
            assert!(!keys.reset_on(&event, over), "{event:?}");
            assert_eq!(keys.count(), Some(5), "{event:?}");
            assert_eq!(keys.pending_keys().len(), 1, "{event:?}");
        }
    }
}

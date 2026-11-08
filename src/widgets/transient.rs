//! [`Transient`]: a card of commands that take switches, as magit's
//! transients are: a title, the switches that change what the commands do
//! (each a key, a check box or radio dot, a label, and the flag it passes),
//! and the commands, each a key and a label.
//!
//! The card answers the same keys whatever it holds: Down or `C-n` and Up
//! or `C-p` move the selection, stopping at the ends; Enter or Space
//! chooses the selected row; Esc or `C-g` closes the card. Any other key
//! typed is the start of a row's own key: a row whose key is typed whole
//! is chosen at once (`-a` takes two keys, `c` one), a key no row's starts
//! with is not the card's. A click on a row chooses it.
//!
//! The card is centred in the area it is drawn in, and keeps the size it
//! grew to while it is open, so flipping a switch never jumps it. Its title
//! line moves it: drag it and it stays where it is put. A click outside the
//! card closes it. An app with keys of its own drives the selection with
//! [`Transient::set_selected`] and offers the card only the pointer.
//!
//! Choosing a switch does not flip it: the app decides what a switch
//! changes (radio switches turn the others in their group off), then says
//! so with [`Transient::set_checked`]. Choosing a command is the app's to
//! run, with the switches that are on.

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::text;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::icon::logical;
use super::checkbox::{LABEL_GAP_CELLS, MARK_SHARE, draw_check_box};
use super::popup::Popup;
use super::radio::draw_radio_dot;

// ========================================================================
// Constants
// ========================================================================

/// The blank cells inside the card's left and right edges.
const EDGE_CELLS: usize = 1;

/// The blank cells between a row's key and what follows it.
const KEY_GAP_CELLS: usize = 2;

/// The cells a switch's mark takes with the blank after it.
const MARK_CELLS: usize = 1 + LABEL_GAP_CELLS as usize;

/// The most cells a label takes; a longer one ends in an ellipsis.
const MAX_LABEL_CELLS: usize = 60;

/// The space above and below the card's lines, in logical pixels.
const PADDING_Y: f32 = 4.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A transient card: its switches, its commands, and the row selected.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transient {
    /// The commands, in order, under the switches.
    commands: Vec<TransientCommand>,
    /// Where each row was drawn last, for the pointer.
    drawn: Vec<(Rect, TransientRow)>,
    /// Where the title line was drawn last, which moves the card.
    drawn_title: Option<Rect>,
    /// Where the whole card was drawn last.
    drawn_card: Option<Rect>,
    /// Where a drag of the title began and the offset then, while it runs.
    grab: Option<(Point, Point)>,
    /// How far the user moved the card from the centre of its area.
    offset: Point,
    /// The keys typed so far toward a row's key.
    pending: String,
    /// The index of the row selected, counting the switches first.
    selected: usize,
    /// The size the card grew to while open, which it never shrinks below.
    size_floor: (f32, f32),
    /// The switches, in order.
    switches: Vec<TransientSwitch>,
    /// What the card is for, at its top; empty for no title.
    title: String,
}

/// A command of a [`Transient`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TransientCommand {
    /// The keys that choose it, as they are typed: `c`, `P`.
    pub key: String,
    /// What it does.
    pub label: String,
}

/// A switch of a [`Transient`], which changes what its commands do.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TransientSwitch {
    /// Whether it is on.
    pub checked: bool,
    /// What it passes to the commands when it is on, shown after its label:
    /// `--all`. `None` shows none.
    pub flag: Option<String>,
    /// The keys that choose it, as they are typed: `-a`.
    pub key: String,
    /// How it is drawn: a check box, or a radio dot for one of several of
    /// which one is on.
    pub kind: SwitchKind,
    /// What it changes.
    pub label: String,
}

/// How a [`TransientSwitch`] is drawn.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SwitchKind {
    /// A check box: a switch on its own.
    #[default]
    Checkbox,
    /// A radio dot: one of several switches of which at most one is on.
    Radio,
}

/// A row of a [`Transient`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransientRow {
    /// The command at this index.
    Command(usize),
    /// The switch at this index.
    Switch(usize),
}

/// What became of an input offered to a [`Transient`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransientResponse {
    /// This row was chosen: by its key, by Enter or Space, or by a click.
    Activated(TransientRow),
    /// Esc or `C-g` closed the card; the app drops it.
    Closed,
    /// The input was the card's.
    Handled,
    /// The input was not the card's.
    Ignored,
}

/// What a key does in any transient, whatever its rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CardKey {
    /// Choose the row selected.
    Choose,
    /// Close the card.
    Close,
    /// Select the next row.
    Next,
    /// Select the row before.
    Previous,
}

/// A line of a drawn card, top to bottom.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Line<'a> {
    /// A heading over a group of rows.
    Heading(&'a str),
    /// A row.
    Row(TransientRow),
    /// A blank line between the groups.
    Space,
    /// The card's title.
    Title,
}

// ========================================================================
// Transient Implementations
// ========================================================================

impl Transient {
    /// A card titled `title` (empty for none) with `switches` over
    /// `commands`, its first row selected.
    pub fn new(
        title: impl Into<String>,
        switches: Vec<TransientSwitch>,
        commands: Vec<TransientCommand>,
    ) -> Self {
        Self {
            commands,
            switches,
            title: title.into(),
            ..Self::default()
        }
    }

    /// Turns the switch at `index` on or off.
    pub fn set_checked(&mut self, index: usize, checked: bool) {
        if let Some(switch) = self.switches.get_mut(index) {
            switch.checked = checked;
        }
    }

    /// Selects `row`, as an app with keys of its own moves the selection;
    /// ignored for a row the card has not.
    pub fn set_selected(&mut self, row: TransientRow) {
        let index = match row {
            TransientRow::Command(at) => self.switches.len() + at,
            TransientRow::Switch(at) => at,
        };
        if self.row(index) == Some(row) {
            self.selected = index;
        }
    }

    /// Where the card was drawn last, for what takes its place (a list
    /// one of its commands opens, drawn on the card); `None` before then.
    pub fn card_rect(&self) -> Option<Rect> {
        self.drawn_card
    }

    /// The switches, in order, with whether each is on.
    pub fn switches(&self) -> &[TransientSwitch] {
        &self.switches
    }

    /// The row selected, if the card has any.
    pub fn selected(&self) -> Option<TransientRow> {
        self.row(self.selected)
    }

    /// The keys typed toward a row's key, for a status bar to show.
    pub fn pending(&self) -> &str {
        &self.pending
    }

    /// The row at `index`, counting the switches first.
    fn row(&self, index: usize) -> Option<TransientRow> {
        match index.checked_sub(self.switches.len()) {
            None => Some(TransientRow::Switch(index)),
            Some(command) if command < self.commands.len() => Some(TransientRow::Command(command)),
            Some(_) => None,
        }
    }

    /// The index of `row`, counting the switches first.
    fn index_of(&self, row: TransientRow) -> usize {
        match row {
            TransientRow::Command(index) => self.switches.len() + index,
            TransientRow::Switch(index) => index,
        }
    }

    /// The keys that choose `row`.
    fn key_of(&self, row: TransientRow) -> &str {
        match row {
            TransientRow::Command(index) => &self.commands[index].key,
            TransientRow::Switch(index) => &self.switches[index].key,
        }
    }

    /// Every row, top to bottom.
    fn rows(&self) -> impl Iterator<Item = TransientRow> + '_ {
        (0..self.switches.len() + self.commands.len()).filter_map(|index| self.row(index))
    }

    /// Chooses `row`, selecting it.
    fn choose(&mut self, row: TransientRow) -> TransientResponse {
        self.selected = self.index_of(row);
        self.pending.clear();
        TransientResponse::Activated(row)
    }

    /// Answers `key`: one every card answers alike, or a step toward a
    /// row's own key.
    fn press_key(&mut self, key: Key) -> TransientResponse {
        match card_key(key) {
            Some(CardKey::Close) if !self.pending.is_empty() => {
                self.pending.clear();
                return TransientResponse::Handled;
            }
            Some(CardKey::Close) => return TransientResponse::Closed,
            Some(CardKey::Choose) if self.pending.is_empty() => {
                return match self.selected() {
                    Some(row) => self.choose(row),
                    None => TransientResponse::Handled,
                };
            }
            Some(CardKey::Next) => {
                let last = (self.switches.len() + self.commands.len()).saturating_sub(1);
                self.selected = (self.selected + 1).min(last);
                return TransientResponse::Handled;
            }
            Some(CardKey::Previous) => {
                self.selected = self.selected.saturating_sub(1);
                return TransientResponse::Handled;
            }
            Some(CardKey::Choose) | None => {}
        }
        let typing = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
        let KeyCode::Char(c) = key.code else {
            return TransientResponse::Ignored;
        };
        if !typing {
            return TransientResponse::Ignored;
        }
        self.pending.push(c);
        let whole = self.rows().find(|row| self.key_of(*row) == self.pending);
        if let Some(row) = whole {
            return self.choose(row);
        }
        let started = self
            .rows()
            .any(|row| self.key_of(row).starts_with(&self.pending));
        match started {
            true => TransientResponse::Handled,
            false => {
                self.pending.clear();
                TransientResponse::Ignored
            }
        }
    }

    /// The card's lines, top to bottom: the title, then each group of rows,
    /// headed when there are both.
    fn lines<'a>(&self, theme: &'a Theme) -> Vec<Line<'a>> {
        let mut lines = Vec::new();
        if !self.title.is_empty() {
            lines.push(Line::Title);
        }
        let both = !self.switches.is_empty() && !self.commands.is_empty();
        if both {
            lines.push(Line::Heading(&theme.strings.transient_switches));
        }
        lines.extend((0..self.switches.len()).map(|index| Line::Row(TransientRow::Switch(index))));
        if both {
            lines.push(Line::Space);
            lines.push(Line::Heading(&theme.strings.transient_commands));
        }
        lines.extend((0..self.commands.len()).map(|index| Line::Row(TransientRow::Command(index))));
        lines
    }

    /// The cells the widest key takes.
    fn key_cells(&self) -> usize {
        let keys = self.rows().map(|row| text::width(self.key_of(row)));
        keys.max().unwrap_or(0)
    }

    /// The cells `row` takes past its key and the gap after it.
    fn row_cells(&self, row: TransientRow) -> usize {
        match row {
            TransientRow::Command(index) => {
                text::width(&self.commands[index].label).min(MAX_LABEL_CELLS)
            }
            TransientRow::Switch(index) => {
                let switch = &self.switches[index];
                let flag = switch
                    .flag
                    .as_deref()
                    .map_or(0, |flag| 1 + text::width(&flag_text(flag)));
                MARK_CELLS + text::width(&switch.label).min(MAX_LABEL_CELLS) + flag
            }
        }
    }

    /// Draws `row` on the line starting at `origin`, inside `clip`.
    fn draw_row(
        &self,
        frame: &mut Frame,
        row: TransientRow,
        origin: Point,
        clip: Rect,
        theme: &Theme,
    ) {
        let metrics = frame.cell_metrics();
        let key = StyledText::plain(self.key_of(row), TextColor::Token(ThemeToken::Accent));
        frame.draw_text(&key, origin, clip, theme);
        let after_key = origin.x + (self.key_cells() + KEY_GAP_CELLS) as f32 * metrics.width;
        let foreground = TextColor::Token(ThemeToken::Foreground);
        match row {
            TransientRow::Command(index) => {
                let label = StyledText::plain(&self.commands[index].label, foreground);
                let label = label.with_ellipsis().fit(MAX_LABEL_CELLS);
                frame.draw_text(&label, Point::new(after_key, origin.y), clip, theme);
            }
            TransientRow::Switch(index) => {
                let switch = &self.switches[index];
                let size = (metrics.height * MARK_SHARE).round();
                let y = (origin.y + (metrics.height - size) / 2.0).round();
                let mark = Rect::new(after_key.round(), y, size, size);
                match switch.kind {
                    SwitchKind::Checkbox => draw_check_box(
                        frame,
                        mark,
                        switch.checked,
                        theme.accent,
                        theme.raised,
                        theme,
                    ),
                    SwitchKind::Radio => {
                        let edge = match switch.checked {
                            true => theme.accent,
                            false => theme.border,
                        };
                        draw_radio_dot(frame, mark, switch.checked, edge, theme.raised);
                    }
                }
                let flag = TextColor::Token(match switch.checked {
                    true => ThemeToken::Accent,
                    false => ThemeToken::Muted,
                });
                let mut line = StyledText::plain(&switch.label, foreground).with_ellipsis();
                line = line.fit(MAX_LABEL_CELLS);
                if let Some(text) = &switch.flag {
                    line.push(Span::new(format!(" {}", flag_text(text)), flag));
                }
                let label_x = after_key + MARK_CELLS as f32 * metrics.width;
                frame.draw_text(&line, Point::new(label_x, origin.y), clip, theme);
            }
        }
    }
}

impl Widget for Transient {
    type Output = ();
    type Response = TransientResponse;

    /// Takes the card's keys and clicks on its rows.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TransientResponse {
        let response = match input {
            Input::Key(key) => self.press_key(*key),
            Input::PointerMoved(point) if self.grab.is_some() => {
                if let Some((origin, offset)) = self.grab {
                    self.offset =
                        Point::new(offset.x + point.x - origin.x, offset.y + point.y - origin.y);
                }
                TransientResponse::Handled
            }
            Input::PointerMoved(point) => {
                let hovered = self.drawn.iter().find(|(rect, _)| rect.contains(*point));
                match hovered.map(|(_, row)| *row) {
                    Some(row) if self.selected() != Some(row) => {
                        self.set_selected(row);
                        TransientResponse::Handled
                    }
                    Some(_) => TransientResponse::Handled,
                    None => TransientResponse::Ignored,
                }
            }
            Input::PointerButton(button) if !button.pressed && self.grab.is_some() => {
                self.grab = None;
                TransientResponse::Handled
            }
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Left =>
            {
                let point = button.position;
                let hit = self.drawn.iter().find(|(rect, _)| rect.contains(point));
                match hit.map(|(_, row)| *row) {
                    Some(row) => self.choose(row),
                    None if self.drawn_title.is_some_and(|title| title.contains(point)) => {
                        self.grab = Some((point, self.offset));
                        TransientResponse::Handled
                    }
                    None if self.drawn_card.is_some_and(|card| card.contains(point)) => {
                        TransientResponse::Handled
                    }
                    None if self.drawn_card.is_some() => TransientResponse::Closed,
                    None => TransientResponse::Ignored,
                }
            }
            Input::PointerButton(_)
            | Input::PointerLeft
            | Input::Wheel(_)
            | Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_) => TransientResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the card in the overlay, centered in `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let inset = Popup::inset(&theme.shape, scale_factor);
        let padding_y = logical(PADDING_Y, scale_factor);
        let lines = self.lines(theme);
        let key_cells = self.key_cells();
        let rows = self
            .rows()
            .map(|row| key_cells + KEY_GAP_CELLS + self.row_cells(row));
        let headings = lines.iter().filter_map(|line| match line {
            Line::Heading(heading) => Some(text::width(heading)),
            Line::Row(_) | Line::Space | Line::Title => None,
        });
        let title = text::width(&self.title);
        let cells = rows.chain(headings).chain([title]).max().unwrap_or(0) + 2 * EDGE_CELLS;
        let width = (cells as f32 * metrics.width + 2.0 * inset)
            .ceil()
            .max(self.size_floor.0)
            .min(area.width);
        let height = (lines.len() as f32 * metrics.height + 2.0 * (padding_y + inset))
            .max(self.size_floor.1)
            .min(area.height);
        self.size_floor = (width, height);
        let x = (area.x + (area.width - width) / 2.0 + self.offset.x).round();
        let y = (area.y + (area.height - height) / 2.0 + self.offset.y).round();
        let popup = Popup::with_rect(Rect::new(x, y, width, height));
        self.drawn_card = Some(popup.rect());
        let style = theme
            .shape
            .floating_box(theme.raised, theme.border, scale_factor);
        let content = popup.content(&theme.shape, scale_factor);
        let left = content.x + EDGE_CELLS as f32 * metrics.width;
        let selected = self.selected();
        let mut drawn = Vec::new();
        let mut drawn_title = None;
        frame.overlay(|frame| {
            frame.draw_box(popup.rect(), &style);
            for (at, line) in lines.iter().enumerate() {
                let top = content.y + padding_y + at as f32 * metrics.height;
                let origin = Point::new(left, top);
                match *line {
                    Line::Heading(heading) => {
                        let muted = TextColor::Token(ThemeToken::Muted);
                        let heading = StyledText::new().with(Span::new(heading, muted).bold());
                        frame.draw_text(&heading, origin, content, theme);
                    }
                    Line::Row(row) => {
                        let band = Rect::new(content.x, top, content.width, metrics.height);
                        if selected == Some(row) {
                            frame.fill_rect(band, theme.selection);
                        }
                        self.draw_row(frame, row, origin, content, theme);
                        drawn.push((band, row));
                    }
                    Line::Space => {}
                    Line::Title => {
                        drawn_title =
                            Some(Rect::new(content.x, top, content.width, metrics.height));
                        let foreground = TextColor::Token(ThemeToken::Foreground);
                        let title =
                            StyledText::new().with(Span::new(&self.title, foreground).bold());
                        frame.draw_text(&title, origin, content, theme);
                    }
                }
            }
        });
        self.drawn = drawn;
        self.drawn_title = drawn_title;
    }
}

// ========================================================================
// TransientCommand Implementations
// ========================================================================

impl TransientCommand {
    /// A command chosen by typing `key`, doing what `label` says.
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

// ========================================================================
// TransientSwitch Implementations
// ========================================================================

impl TransientSwitch {
    /// A check box switch, off, chosen by typing `key`, changing what
    /// `label` says.
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            checked: false,
            flag: None,
            key: key.into(),
            kind: SwitchKind::Checkbox,
            label: label.into(),
        }
    }

    /// This switch drawn as a radio dot, one of several of which one is on.
    pub fn radio(self) -> Self {
        Self {
            kind: SwitchKind::Radio,
            ..self
        }
    }

    /// This switch on or off.
    pub fn with_checked(self, checked: bool) -> Self {
        Self { checked, ..self }
    }

    /// This switch passing `flag` when it is on, shown after its label.
    pub fn with_flag(self, flag: impl Into<String>) -> Self {
        Self {
            flag: Some(flag.into()),
            ..self
        }
    }
}

// ========================================================================
// TransientResponse Implementations
// ========================================================================

impl Outcome for TransientResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Free Functions
// ========================================================================

/// What `key` does in any transient, if it is one of the keys every card
/// answers alike; `None` for a key toward a row's own.
fn card_key(key: Key) -> Option<CardKey> {
    match (key.mods, key.code) {
        (Mods::NONE, KeyCode::Enter | KeyCode::Char(' ')) => Some(CardKey::Choose),
        (Mods::NONE, KeyCode::Esc) | (Mods::CTRL, KeyCode::Char('g')) => Some(CardKey::Close),
        (Mods::NONE, KeyCode::Down) | (Mods::CTRL, KeyCode::Char('n')) => Some(CardKey::Next),
        (Mods::NONE, KeyCode::Up) | (Mods::CTRL, KeyCode::Char('p')) => Some(CardKey::Previous),
        _ => None,
    }
}

/// A switch's `flag` as it is shown after the label: `(--all)`.
fn flag_text(flag: &str) -> String {
    format!("({flag})")
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

    fn commit() -> Transient {
        Transient::new(
            "Commit",
            vec![
                TransientSwitch::new("-a", "Stage all modified files")
                    .with_flag("--all")
                    .with_checked(true),
                TransientSwitch::new("-e", "Allow empty commit").with_flag("--allow-empty"),
            ],
            vec![
                TransientCommand::new("c", "Commit"),
                TransientCommand::new("a", "Amend"),
            ],
        )
    }

    fn send(window: &mut TestWindow, card: &mut Transient, input: Input) -> TransientResponse {
        window.send(&input, |input, cx| card.handle(input, cx))
    }

    fn key(code: KeyCode, mods: Mods) -> Input {
        Input::Key(Key::new(code, mods))
    }

    fn char(c: char) -> Input {
        key(KeyCode::Char(c), Mods::NONE)
    }

    #[test]
    fn test_a_rows_whole_key_chooses_it_and_a_key_no_row_starts_with_is_ignored() {
        let mut window = TestWindow::new(400, 300);
        let mut card = commit();
        assert_eq!(
            send(&mut window, &mut card, char('-')),
            TransientResponse::Handled
        );
        assert_eq!(card.pending(), "-");
        assert_eq!(
            send(&mut window, &mut card, char('e')),
            TransientResponse::Activated(TransientRow::Switch(1))
        );
        assert_eq!(card.pending(), "");
        assert_eq!(
            card.selected(),
            Some(TransientRow::Switch(1)),
            "chosen is selected"
        );
        assert_eq!(
            send(&mut window, &mut card, char('a')),
            TransientResponse::Activated(TransientRow::Command(1)),
            "a alone is Amend, not the start of -a"
        );
        assert_eq!(
            send(&mut window, &mut card, char('z')),
            TransientResponse::Ignored
        );
        assert_eq!(card.pending(), "", "a key no row has is dropped");
    }

    #[test]
    fn test_the_selection_walks_both_groups_and_stops_at_the_ends() {
        let mut window = TestWindow::new(400, 300);
        let mut card = commit();
        send(&mut window, &mut card, key(KeyCode::Up, Mods::NONE));
        assert_eq!(card.selected(), Some(TransientRow::Switch(0)));
        for _ in 0..2 {
            send(&mut window, &mut card, key(KeyCode::Char('n'), Mods::CTRL));
        }
        assert_eq!(
            card.selected(),
            Some(TransientRow::Command(0)),
            "into the commands"
        );
        for _ in 0..5 {
            send(&mut window, &mut card, key(KeyCode::Down, Mods::NONE));
        }
        assert_eq!(card.selected(), Some(TransientRow::Command(1)), "no wrap");
        assert_eq!(
            send(&mut window, &mut card, char(' ')),
            TransientResponse::Activated(TransientRow::Command(1))
        );
    }

    #[test]
    fn test_escape_drops_a_pending_key_before_it_closes() {
        let mut window = TestWindow::new(400, 300);
        let mut card = commit();
        send(&mut window, &mut card, char('-'));
        let escape = key(KeyCode::Esc, Mods::NONE);
        assert_eq!(
            send(&mut window, &mut card, escape),
            TransientResponse::Handled
        );
        assert_eq!(card.pending(), "");
        assert_eq!(
            send(&mut window, &mut card, key(KeyCode::Char('g'), Mods::CTRL)),
            TransientResponse::Closed
        );
    }

    #[test]
    fn test_headings_show_only_when_there_are_switches_and_commands() {
        let theme = Theme::dark();
        let lines = commit().lines(&theme);
        assert_eq!(lines.len(), 1 + 1 + 2 + 1 + 1 + 2);
        let commands = Transient::new("", Vec::new(), vec![TransientCommand::new("c", "Commit")]);
        assert_eq!(
            commands.lines(&theme),
            [Line::Row(TransientRow::Command(0))]
        );
    }

    #[test]
    fn test_a_click_on_a_drawn_row_chooses_it_and_the_selected_row_is_lit() {
        let theme = Theme::dark();
        let mut card = commit();
        let Some(snapshot) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            card.draw(frame, frame.bounds(), &theme);
        }) else {
            return;
        };
        let (first, _) = card.drawn[0];
        let right = first.x + first.width - 2.0;
        snapshot.assert_pixel(
            right,
            first.y + 2.0,
            theme.selection,
            "the first row is lit",
        );
        let (amend, row) = card.drawn[3];
        assert_eq!(row, TransientRow::Command(1));
        snapshot.assert_pixel(right, amend.y + 2.0, theme.raised, "the others are not");
        let mut window = TestWindow::new(400, 300);
        let at = Point::new(amend.x + 4.0, amend.y + 4.0);
        let click = Input::PointerButton(ButtonInput::press(PointerButton::Left, at));
        assert_eq!(
            send(&mut window, &mut card, click),
            TransientResponse::Activated(TransientRow::Command(1))
        );
    }

    #[test]
    fn test_the_app_moves_the_selection_and_the_title_moves_the_card() {
        let mut card = Transient::new(
            "Push",
            vec![TransientSwitch::new("-f", "Force")],
            vec![TransientCommand::new("p", "Push")],
        );
        card.set_selected(TransientRow::Command(0));
        assert_eq!(card.selected(), Some(TransientRow::Command(0)));
        card.set_selected(TransientRow::Command(5));
        assert_eq!(
            card.selected(),
            Some(TransientRow::Command(0)),
            "no such row"
        );
        let mut window = crate::testing::TestWindow::new(800, 600);
        let area = Rect::new(0.0, 0.0, 800.0, 600.0);
        card.draw(&mut window.frame(), area, &Theme::dark());
        let before = card.drawn_card.expect("drawn");
        let title = card.drawn_title.expect("a title line");
        let (x, y) = (title.x + 4.0, title.y + 2.0);
        let press = |x: f32, y: f32, pressed: bool| {
            let input =
                crate::input::event::ButtonInput::press(PointerButton::Left, Point::new(x, y));
            Input::PointerButton(crate::input::event::ButtonInput { pressed, ..input })
        };
        for input in [
            press(x, y, true),
            Input::PointerMoved(Point::new(x + 25.0, y + 10.0)),
            press(x + 25.0, y + 10.0, false),
        ] {
            window.send(&input, |input, cx| card.handle(input, cx));
        }
        card.draw(&mut window.frame(), area, &Theme::dark());
        let after = card.drawn_card.expect("drawn");
        assert_eq!((after.x, after.y), (before.x + 25.0, before.y + 10.0));
        let row = card.drawn[0].0;
        let over = Input::PointerMoved(Point::new(row.x + 2.0, row.y + 2.0));
        window.send(&over, |input, cx| card.handle(input, cx));
        assert_eq!(
            card.selected(),
            Some(card.drawn[0].1),
            "the row under the pointer"
        );
        assert_eq!(card.card_rect(), card.drawn_card);
        let outside = window.send(&press(2.0, 2.0, true), |input, cx| card.handle(input, cx));
        assert_eq!(outside, TransientResponse::Closed);
    }

    #[test]
    fn test_the_card_keeps_the_size_it_grew_to() {
        let mut card = Transient::new(
            "t",
            vec![],
            vec![TransientCommand::new("p", "A long label to show")],
        );
        let mut frame = crate::app::offscreen::plain_frame(800, 600);
        let area = Rect::new(0.0, 0.0, 800.0, 600.0);
        card.draw(&mut frame, area, &Theme::dark());
        let wide = card.drawn_card.expect("drawn");
        card.commands = vec![TransientCommand::new("p", "Short")];
        card.draw(&mut frame, area, &Theme::dark());
        assert_eq!(card.drawn_card.expect("drawn").width, wide.width);
    }
}

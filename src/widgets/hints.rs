//! [`Hints`]: hint labels, as Vimium puts on links, over targets the app
//! names by their rectangles (rows in sight, buttons, panes), to pick one
//! from the keyboard with a letter or two.
//!
//! [`Hints::show`] gives each target a short label from the alphabet, home
//! row first, no label the start of another, so typing a whole label
//! picks its target at once. Keys, while hints show: a letter of the
//! alphabet (either case) narrows them to the labels starting with what
//! was typed, and completes one with [`HintsResponse::Activated`]; a letter no
//! label goes on with is swallowed; Backspace takes back the last letter;
//! Esc ends with [`HintsResponse::Cancelled`]. Every other key is
//! swallowed while hints show, so the widgets under them do not move.
//! The pointer: a press or a wheel turn ends with
//! [`HintsResponse::Cancelled`], since the targets may move under it.
//!
//! Hand the hints every input before the widgets under them while
//! [`Hints::is_active`], and draw them last, over the targets.

use std::collections::BTreeSet;

use accesskit::{Live, Node, Role};
use thiserror::Error;

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{KeyCode, Mods};
use super::super::base::styled::{Span, StyledText};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::Input;
use super::super::render::icon::logical;

// ========================================================================
// Constants
// ========================================================================

/// The letters labels are made of unless the app says otherwise, the home
/// row first, then the rows above and below.
const DEFAULT_ALPHABET: &str = "asdfjklghqweruioptyzxcvbnm";

/// The fewest different letters an alphabet needs to label more than one
/// target without one label starting another.
const MIN_ALPHABET: usize = 2;

/// The space left and right of a label inside its badge, in logical
/// pixels.
const BADGE_PADDING: f32 = 3.0;

/// The color of the letters typed so far.
const TYPED_COLOR: ThemeToken = ThemeToken::Surface;

/// The color of the letters still to type.
const LABEL_COLOR: ThemeToken = ThemeToken::Background;

// ========================================================================
// Data Structures
// ========================================================================

/// Hint labels over targets, narrowed as the user types. It keeps the
/// targets, their labels, and what was typed.
#[derive(Clone, Debug)]
pub struct Hints {
    /// The letters labels are made of, in the order they are handed out.
    alphabet: Vec<char>,
    /// Where the hints were drawn last, or `None` before the first frame.
    area: Option<Rect>,
    /// Each target's label, in the order of `targets`.
    labels: Vec<String>,
    /// The targets, as the app gave them.
    targets: Vec<Rect>,
    /// The letters typed so far.
    typed: String,
}

/// What became of an input offered to [`Hints`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HintsResponse {
    /// The label of the target at this index (in the order given to
    /// [`Hints::show`]) was typed; the hints are gone.
    Activated(usize),
    /// Esc, a press, or a wheel turn ended the hints without a choice.
    Cancelled,
    /// The hints used the input.
    Handled,
    /// The hints are not showing, or the input is not theirs.
    Ignored,
}

/// Why an alphabet cannot make hint labels.
#[derive(Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum HintsError {
    /// Fewer than two different letters.
    #[error(
        "a hint alphabet needs at least two different letters to label targets apart; \
         pass one such as \"asdfjkl\""
    )]
    ShortAlphabet,
}

// ========================================================================
// Hints Implementations
// ========================================================================

impl Hints {
    /// Hints labeled from the home row first (`a`, `s`, `d`, `f`, `j`, `k`,
    /// `l`, ...), showing nothing until [`Hints::show`].
    pub fn new() -> Self {
        Self {
            alphabet: DEFAULT_ALPHABET.chars().collect(),
            area: None,
            labels: Vec::new(),
            targets: Vec::new(),
            typed: String::new(),
        }
    }

    /// Hints labeled from the letters of `alphabet`, in its order (most
    /// wanted first), case and repeats ignored. Fails when it holds fewer
    /// than two different letters.
    pub fn with_alphabet(alphabet: &str) -> Result<Self, HintsError> {
        let mut seen = BTreeSet::new();
        let letters: Vec<char> = alphabet
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|letter| !letter.is_whitespace() && seen.insert(*letter))
            .collect();
        if letters.len() < MIN_ALPHABET {
            return Err(HintsError::ShortAlphabet);
        }
        Ok(Self {
            alphabet: letters,
            ..Self::new()
        })
    }

    /// Shows a label over each of `targets` (rectangles in physical pixels,
    /// such as [`List::row_rect`](crate::List::row_rect) gives), forgetting
    /// what was typed. No targets shows nothing.
    pub fn show(&mut self, targets: Vec<Rect>) {
        self.labels = labels(targets.len(), &self.alphabet);
        self.targets = targets;
        self.typed.clear();
    }

    /// Takes the hints away.
    pub fn hide(&mut self) {
        self.labels.clear();
        self.targets.clear();
        self.typed.clear();
    }

    /// Whether hints show, waiting for a label.
    pub fn is_active(&self) -> bool {
        !self.targets.is_empty()
    }

    /// The letters typed so far, to show in a status bar.
    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// Reads `letter` toward a label: the response to it.
    fn type_letter(&mut self, letter: char) -> HintsResponse {
        let mut typed = self.typed.clone();
        typed.push(letter);
        if !self.labels.iter().any(|label| label.starts_with(&typed)) {
            return HintsResponse::Handled;
        }
        match self.labels.iter().position(|label| *label == typed) {
            Some(index) => {
                self.hide();
                HintsResponse::Activated(index)
            }
            None => {
                self.typed = typed;
                HintsResponse::Handled
            }
        }
    }

    /// Describes the hints to screen readers as `id` while they show: a
    /// polite live status saying the hint mode is on (the theme's
    /// [`Strings::hints`](crate::Strings::hints)), its value the
    /// letters typed so far.
    pub fn access(&self, frame: &mut Frame, id: FocusId, theme: &Theme) {
        let (true, Some(area)) = (self.is_active(), self.area) else {
            return;
        };
        let mut node = Node::new(Role::Status);
        node.set_live(Live::Polite);
        node.set_label(theme.strings.hints.as_ref());
        node.set_value(self.typed.as_str());
        frame.access(id, area, node);
    }
}

impl Default for Hints {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Hints {
    type Output = ();
    type Response = HintsResponse;

    /// Reacts to `input` while hints show, as the module documentation
    /// lists; [`HintsResponse::Ignored`] while they do not. Asks for a
    /// redraw whenever what is drawn changed.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> HintsResponse {
        if !self.is_active() {
            return HintsResponse::Ignored;
        }
        let response = match input {
            Input::Key(key) => {
                let typing = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
                match key.code {
                    KeyCode::Esc => {
                        self.hide();
                        HintsResponse::Cancelled
                    }
                    KeyCode::Backspace => {
                        self.typed.pop();
                        HintsResponse::Handled
                    }
                    KeyCode::Char(c) if typing => {
                        let letter = c.to_lowercase().next().unwrap_or(c);
                        match self.alphabet.contains(&letter) {
                            true => self.type_letter(letter),
                            false => HintsResponse::Handled,
                        }
                    }
                    KeyCode::Char(_)
                    | KeyCode::Delete
                    | KeyCode::Down
                    | KeyCode::End
                    | KeyCode::Enter
                    | KeyCode::F(_)
                    | KeyCode::Home
                    | KeyCode::Insert
                    | KeyCode::Left
                    | KeyCode::PageDown
                    | KeyCode::PageUp
                    | KeyCode::Right
                    | KeyCode::Tab
                    | KeyCode::Up => HintsResponse::Handled,
                }
            }
            Input::PointerButton(button) if button.pressed => {
                self.hide();
                HintsResponse::Cancelled
            }
            Input::Wheel(_) | Input::FocusLost => {
                self.hide();
                HintsResponse::Cancelled
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_) => HintsResponse::Ignored,
        };
        if !matches!(response, HintsResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws, in the overlay over everything else, a badge on the accent
    /// color at the left of each target (centered top to bottom) whose
    /// label starts with what was typed, the typed letters dimmed; only the
    /// part inside `area` shows.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.area = Some(area);
        if !self.is_active() {
            return;
        }
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let padding = logical(BADGE_PADDING, scale_factor);
        let style = theme.shape.highlight(theme.accent, scale_factor);
        let typed = self.typed.as_str();
        frame.overlay(|frame| {
            frame.clipped(area, |frame| {
                for (label, target) in self.labels.iter().zip(&self.targets) {
                    let Some(rest) = label.strip_prefix(typed) else {
                        continue;
                    };
                    let text = StyledText::new()
                        .with(Span::new(typed, TYPED_COLOR.into()))
                        .with(Span::new(rest, LABEL_COLOR.into()).bold());
                    let width = text.width() as f32 * metrics.width;
                    let y = (target.y + (target.height - metrics.height) / 2.0).round();
                    let badge =
                        Rect::new(target.x.round(), y, width + 2.0 * padding, metrics.height);
                    frame.draw_box(badge, &style);
                    let origin = Point::new(badge.x + padding, y);
                    frame.draw_text(&text, origin, badge, theme);
                }
            });
        });
    }
}

impl Outcome for HintsResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `count` labels from `alphabet`, none the start of another, the shortest
/// first: single letters while they last, and as few longer ones as
/// needed, each made by giving one letter (the earliest still single)
/// every letter after it, as Vimium does.
fn labels(count: usize, alphabet: &[char]) -> Vec<String> {
    if count == 0 || alphabet.len() < MIN_ALPHABET {
        return Vec::new();
    }
    let mut labels = vec![String::new()];
    let mut taken = 0;
    while labels.len() - taken < count || labels.len() == 1 {
        let prefix = labels[taken].clone();
        taken += 1;
        labels.extend(alphabet.iter().map(|letter| format!("{prefix}{letter}")));
    }
    labels.drain(taken..taken + count).collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::key::parse_key;
    use crate::testing::{TestWindow, drawn_text, ui_text};

    /// Hints over `count` rows 20 pixels high from the top left.
    fn shown(count: usize) -> Hints {
        let mut hints = Hints::new();
        let rows = (0..count)
            .map(|row| Rect::new(0.0, row as f32 * 20.0, 200.0, 20.0))
            .collect();
        hints.show(rows);
        hints
    }

    /// Feeds the key of `spec` to `hints`.
    fn press(window: &mut TestWindow, hints: &mut Hints, spec: &str) -> HintsResponse {
        window.feed(hints, &Input::Key(parse_key(spec).expect("a key")))
    }

    #[test]
    fn test_labels_are_prefix_free_shortest_first_and_home_row_first() {
        let alphabet: Vec<char> = DEFAULT_ALPHABET.chars().collect();
        assert_eq!(labels(3, &alphabet), ["a", "s", "d"]);
        for count in [1, 26, 27, 100, 700, 800] {
            let made = labels(count, &alphabet);
            assert_eq!(made.len(), count);
            let unique: BTreeSet<&String> = made.iter().collect();
            assert_eq!(unique.len(), count, "{count} labels all differ");
            for a in &made {
                for b in &made {
                    assert!(a == b || !b.starts_with(a.as_str()), "{a} starts {b}");
                }
            }
            assert!(made.windows(2).all(|pair| pair[0].len() <= pair[1].len()));
        }
        assert!(
            labels(27, &alphabet)
                .iter()
                .filter(|label| label.len() == 1)
                .count()
                == 25
        );
    }

    #[test]
    fn test_typing_a_whole_label_chooses_its_target_and_hides_the_hints() {
        let mut window = TestWindow::new(400, 400);
        let mut hints = shown(30);
        let index = hints
            .labels
            .iter()
            .position(|label| label.len() == 2)
            .expect("a pair");
        let label = hints.labels[index].clone();
        let mut letters = label.chars();
        let first = letters.next().expect("a letter").to_string();
        let second = letters.next().expect("a letter").to_uppercase().to_string();
        assert_eq!(
            press(&mut window, &mut hints, &first),
            HintsResponse::Handled
        );
        assert_eq!(hints.typed(), first);
        assert!(window.take_redraw());
        assert_eq!(
            press(&mut window, &mut hints, &second),
            HintsResponse::Activated(index),
            "either case"
        );
        assert!(!hints.is_active());
        assert_eq!(press(&mut window, &mut hints, "a"), HintsResponse::Ignored);
    }

    #[test]
    fn test_a_letter_no_label_goes_on_with_is_swallowed_and_backspace_takes_one_back() {
        let mut window = TestWindow::new(400, 400);
        let mut hints = shown(30);
        let prefix = hints.labels[29]
            .chars()
            .next()
            .expect("a letter")
            .to_string();
        press(&mut window, &mut hints, &prefix);
        let other = hints
            .alphabet
            .iter()
            .find(|letter| {
                !hints
                    .labels
                    .iter()
                    .any(|label| *label == format!("{prefix}{letter}"))
            })
            .copied();
        if let Some(other) = other {
            assert_eq!(
                press(&mut window, &mut hints, &other.to_string()),
                HintsResponse::Handled
            );
            assert_eq!(hints.typed(), prefix, "the stray letter is not kept");
        }
        assert_eq!(
            press(&mut window, &mut hints, "<bs>"),
            HintsResponse::Handled
        );
        assert_eq!(hints.typed(), "");
        assert_eq!(
            press(&mut window, &mut hints, "j"),
            HintsResponse::Activated(3),
            "s, d, f, j: a leads the pairs"
        );
    }

    #[test]
    fn test_esc_or_a_press_cancels_and_other_keys_are_swallowed() {
        let mut window = TestWindow::new(400, 400);
        let mut hints = shown(3);
        assert_eq!(
            press(&mut window, &mut hints, "<down>"),
            HintsResponse::Handled
        );
        assert_eq!(
            press(&mut window, &mut hints, "C-x"),
            HintsResponse::Handled
        );
        assert_eq!(
            press(&mut window, &mut hints, "<esc>"),
            HintsResponse::Cancelled
        );
        assert!(!hints.is_active());
        let mut hints = shown(3);
        let click = crate::input::event::ButtonInput::press(
            crate::input::event::PointerButton::Left,
            Point::new(5.0, 5.0),
        );
        assert_eq!(
            window.feed(&mut hints, &Input::PointerButton(click)),
            HintsResponse::Cancelled
        );
    }

    #[test]
    fn test_an_alphabet_of_one_letter_is_refused_with_how_to_fix_it() {
        let error = Hints::with_alphabet("aAa ").expect_err("one letter");
        assert_eq!(error, HintsError::ShortAlphabet);
        assert!(error.to_string().contains("asdfjkl"));
        let hints = Hints::with_alphabet("JKjk").expect("two letters");
        assert_eq!(hints.alphabet, ['j', 'k']);
    }

    #[test]
    fn test_only_the_labels_matching_what_was_typed_are_drawn_and_the_mode_is_announced() {
        let mut window = TestWindow::new(400, 400);
        let mut hints = shown(30);
        press(&mut window, &mut hints, "a");
        let mut frame = offscreen::plain_frame(400, 800);
        let theme = Theme::dark();
        hints.draw(&mut frame, Rect::new(0.0, 0.0, 400.0, 800.0), &theme);
        assert_eq!(drawn_text(&frame), ["aa", "as", "ad", "af", "aj"]);
        hints.access(&mut frame, FocusId::new("hints"), &theme);
        assert!(
            ui_text(&frame, "t").contains("Hint mode"),
            "{}",
            ui_text(&frame, "t")
        );
    }
}

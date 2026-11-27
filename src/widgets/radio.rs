//! [`RadioGroup`]: a column of options of which the user picks one, each
//! a round dot with its label to the right, the chosen one's dot filled
//! with the accent.
//!
//! Keys, while the app says the group is focused, move the choice and
//! choose as they go, as native radio groups do: Down, Right, `j`, and `l`
//! to the next option and Up, Left, `k`, and `h` to the one before, both
//! wrapping; Home and End to the first and the last; Space chooses the
//! first option when none is chosen yet. A click on an option chooses it,
//! focused or not. A disabled group is drawn muted and leaves every input
//! alone.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::checkbox::{LABEL_GAP_CELLS, MARK_BORDER, MARK_SHARE, label_color, pill};
use super::label::{draw_label, label_width};

// ========================================================================
// Constants
// ========================================================================

/// The share of a dot the chosen option's inner dot takes.
const INNER_DOT_SHARE: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// A column of options, each handing back a value of type `T`, of which
/// the user picks one. The app owns it, hands it the window's pointer
/// events (and its keys while it has the focus), and draws it each frame.
#[derive(Clone, Debug)]
pub struct RadioGroup<T> {
    /// Whether it takes input; a disabled group is drawn muted.
    enabled: bool,
    /// Whether the app gave it the keyboard.
    focused: bool,
    /// The option under the pointer.
    hovered: Option<usize>,
    /// The name screen readers give the group, if any.
    label: Option<String>,
    /// Where the options were drawn last, for the pointer.
    layout: Option<RadioLayout>,
    /// The options, top to bottom.
    options: Vec<RadioOption<T>>,
    /// The index of the chosen option, if one is.
    selected: Option<usize>,
}

/// What became of an input offered to a [`RadioGroup`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RadioGroupResponse<T> {
    /// The user chose the option with this value.
    Changed(T),
    /// The group used the input; nothing changed.
    Handled,
    /// The input is not the group's.
    Ignored,
}

/// What the app's state says a [`RadioGroup`] shows, for
/// [`Control::sync`].
#[derive(Debug)]
pub struct RadioGroupProps<'a, T> {
    /// The value of the chosen option; a value no option hands back
    /// leaves none chosen.
    pub chosen: &'a T,
    /// Whether it takes input.
    pub enabled: bool,
    /// The name screen readers give the group, if any.
    pub label: Option<&'a str>,
    /// The options, top to bottom: what choosing each hands back, and what
    /// it says.
    pub options: &'a [(T, &'a str)],
}

/// One option of a [`RadioGroup`].
#[derive(Clone, Debug)]
struct RadioOption<T> {
    /// What it says.
    label: String,
    /// What choosing it hands back.
    value: T,
}

/// Where a group's options were drawn.
#[derive(Clone, Debug, PartialEq)]
struct RadioLayout {
    /// Each option's dot.
    dots: Vec<Rect>,
    /// The whole group.
    group: Rect,
    /// Each option's line, dot and label: what a click chooses.
    rows: Vec<Rect>,
}

// ========================================================================
// RadioGroup Implementations
// ========================================================================

impl<T: Clone> RadioGroup<T> {
    /// An enabled group with no options yet.
    pub fn new() -> Self {
        Self {
            enabled: true,
            focused: false,
            hovered: None,
            label: None,
            layout: None,
            options: Vec::new(),
            selected: None,
        }
    }

    /// This group with an option saying `label` that hands back `value`
    /// added below the others.
    pub fn with_option(mut self, value: T, label: impl Into<String>) -> Self {
        self.options.push(RadioOption {
            label: label.into(),
            value,
        });
        self
    }

    /// This group named `label` for screen readers, as the label beside
    /// it says what it chooses.
    pub fn with_label(self, label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..self
        }
    }

    /// This group taking input (`true`), or disabled: drawn muted and
    /// leaving every input alone.
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// The value of the chosen option, if one is.
    pub fn selected(&self) -> Option<&T> {
        self.selected.map(|index| &self.options[index].value)
    }

    /// The index of the chosen option, if one is.
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Chooses option `index`, as the app's state says, without a
    /// response; an index past the last option clears the choice.
    pub fn select(&mut self, index: usize) {
        self.selected = (index < self.options.len()).then_some(index);
    }

    /// Whether it takes input.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Lets it take input (`true`), or disables it.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.hovered = None;
        }
    }

    /// Describes the group as last drawn to screen readers as `id`: a
    /// radio group named by its label, holding its options as radio buttons (the `index`th is
    /// child `index` of `id`), each named by its label and clickable, the
    /// chosen one checked and focused while the group is. Nothing before
    /// the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        let mut group = Node::new(Role::RadioGroup);
        if let Some(label) = &self.label {
            group.set_label(label.as_str());
        }
        if !self.enabled {
            group.set_disabled();
        }
        frame.enter_access(id, layout.group, group);
        for (index, (option, row)) in self.options.iter().zip(&layout.rows).enumerate() {
            let mut node = Node::new(Role::RadioButton);
            node.set_label(option.label.as_str());
            node.set_toggled((self.selected == Some(index)).into());
            match self.enabled {
                true => node.add_action(Action::Click),
                false => node.set_disabled(),
            }
            frame.access(id.child(index as u64), *row, node);
        }
        frame.leave_access();
        if let Some(index) = self.focus_index() {
            frame.access.set_inner_focus(id, id.child(index as u64));
        }
    }

    /// Does what a screen reader's `action` on the group described as `id`
    /// asks: a click on an option chooses it. Ignored for other actions
    /// and nodes.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> RadioGroupResponse<T> {
        match action {
            AccessAction::Click(target) => {
                match (0..self.options.len()).find(|index| id.child(*index as u64) == target) {
                    Some(index) => self.choose(index),
                    None => RadioGroupResponse::Ignored,
                }
            }
            AccessAction::Decrement(_) | AccessAction::Focus(_) | AccessAction::Increment(_) => {
                RadioGroupResponse::Ignored
            }
        }
    }

    /// How tall it is drawn with `metrics`: a line per option.
    fn lines_height(&self, metrics: CellMetrics) -> f32 {
        self.options.len() as f32 * metrics.height.ceil()
    }

    /// Chooses option `index`, when the group is enabled.
    fn choose(&mut self, index: usize) -> RadioGroupResponse<T> {
        if !self.enabled {
            return RadioGroupResponse::Ignored;
        }
        if self.selected == Some(index) {
            return RadioGroupResponse::Handled;
        }
        self.selected = Some(index);
        RadioGroupResponse::Changed(self.options[index].value.clone())
    }

    /// Chooses the option `delta` from the chosen one, wrapping; from no
    /// choice, the first option going forward and the last going back.
    fn step(&mut self, delta: isize) -> RadioGroupResponse<T> {
        let count = self.options.len() as isize;
        if count == 0 {
            return RadioGroupResponse::Handled;
        }
        let index = match self.selected {
            Some(index) => (index as isize + delta).rem_euclid(count),
            None if delta > 0 => 0,
            None => count - 1,
        };
        self.choose(index as usize)
    }

    /// Chooses option `index` if there is one.
    fn jump(&mut self, index: usize) -> RadioGroupResponse<T> {
        match index < self.options.len() {
            true => self.choose(index),
            false => RadioGroupResponse::Handled,
        }
    }

    /// Reacts to `key`, as the module documentation lists, while focused.
    fn handle_key(&mut self, key: Key, vim: bool) -> RadioGroupResponse<T> {
        if !self.focused || key.mods != Mods::NONE {
            return RadioGroupResponse::Ignored;
        }
        match key.code {
            KeyCode::Down | KeyCode::Right => self.step(1),
            KeyCode::Char('j' | 'l') if vim => self.step(1),
            KeyCode::Up | KeyCode::Left => self.step(-1),
            KeyCode::Char('k' | 'h') if vim => self.step(-1),
            KeyCode::Home => self.jump(0),
            KeyCode::End => self.jump(self.options.len().saturating_sub(1)),
            KeyCode::Char(' ') if self.selected.is_none() => self.jump(0),
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Insert
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Tab => RadioGroupResponse::Ignored,
        }
    }

    /// The option under `point`, as last drawn.
    fn option_at(&self, point: Point) -> Option<usize> {
        self.layout
            .as_ref()?
            .rows
            .iter()
            .position(|row| row.contains(point))
    }

    /// Follows the pointer to `point` (`None` outside the window), asking a
    /// redraw when it moved to another option; says which it is over.
    fn follow_pointer(&mut self, point: Option<Point>, cx: &mut EventCx<'_>) -> Option<usize> {
        let over = point
            .filter(|_| self.enabled)
            .and_then(|point| self.option_at(point));
        if over != self.hovered {
            self.hovered = over;
            cx.request_redraw();
        }
        over
    }

    /// The option a screen reader's focus sits on: the
    /// chosen one, else the first.
    fn focus_index(&self) -> Option<usize> {
        self.selected
            .or_else(|| (!self.options.is_empty()).then_some(0))
    }

    /// Draws option `index`'s dot in `dot`: a ring with an inner dot in the
    /// accent when chosen, an empty ring otherwise.
    fn draw_dot(&self, frame: &mut Frame, index: usize, dot: Rect, theme: &Theme) {
        let chosen = self.selected == Some(index);
        let fill = match self.hovered == Some(index) {
            true => theme.hover,
            false => theme.background,
        };
        let edge = match (chosen, self.enabled) {
            (true, true) => theme.accent,
            (true, false) => theme.muted,
            (false, _) => theme.border,
        };
        draw_radio_dot(frame, dot, chosen, edge, fill);
    }
}

impl<T: Clone> Default for RadioGroup<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Widget for RadioGroup<T> {
    type Output = Rect;
    type Response = RadioGroupResponse<T>;

    /// Reacts to `input`: keys as the module documentation lists while
    /// focused, a left press on an option choosing it, and the pointer
    /// lighting the option under it. Asks for a redraw whenever it used
    /// the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> RadioGroupResponse<T> {
        let response = match input {
            Input::Key(key) if self.enabled => self.handle_key(*key, cx.key_scheme().vim),
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Left && self.enabled =>
            {
                match self.option_at(button.position) {
                    Some(index) => self.choose(index),
                    None => RadioGroupResponse::Ignored,
                }
            }
            Input::PointerMoved(position) => match self.follow_pointer(Some(*position), cx) {
                Some(_) => RadioGroupResponse::Handled,
                None => RadioGroupResponse::Ignored,
            },
            Input::PointerLeft => {
                self.follow_pointer(None, cx);
                RadioGroupResponse::Ignored
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => RadioGroupResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the options a line each from the top left of `area`, and
    /// returns the area the group took.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let metrics = frame.cell_metrics();
        let line = metrics.height.ceil();
        let size = (metrics.height * MARK_SHARE).round();
        let label_x = (area.x + size + LABEL_GAP_CELLS * metrics.width).round();
        let color = label_color(self.enabled, theme);
        let mut dots = Vec::with_capacity(self.options.len());
        let mut rows = Vec::with_capacity(self.options.len());
        for (index, option) in self.options.iter().enumerate() {
            let top = area.y + index as f32 * line;
            let dot = Rect::new(area.x, (top + (line - size) / 2.0).round(), size, size);
            self.draw_dot(frame, index, dot, theme);
            draw_label(frame, theme, &option.label, Point::new(label_x, top), color);
            let width = label_x + label_width(frame, theme, &option.label) - area.x;
            dots.push(dot);
            rows.push(Rect::new(area.x, top, width, line));
        }
        let width = rows.iter().map(|row| row.width).fold(0.0, f32::max);
        let group = Rect::new(area.x, area.y, width, self.lines_height(metrics));
        self.layout = Some(RadioLayout { dots, group, rows });
        group
    }

    /// Tells it whether it has the keyboard: it takes keys only while it
    /// does (a [`FocusTree`](crate::FocusTree)
    /// draws the focus ring around it).
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl<T: Clone + PartialEq> Control for RadioGroup<T> {
    type Props<'a>
        = RadioGroupProps<'a, T>
    where
        T: 'a;
    type Value = T;

    /// Takes the options, the choice, whether it is enabled, and its name
    /// from `props`. New options forget where the last ones were drawn, so the
    /// pointer finds them only once they are drawn.
    fn sync(&mut self, props: RadioGroupProps<'_, T>) {
        let options = self
            .options
            .iter()
            .map(|option| (&option.value, option.label.as_str()));
        if !is_same_options(options, props.options) {
            self.options = props
                .options
                .iter()
                .map(|(value, label)| RadioOption {
                    label: (*label).to_owned(),
                    value: value.clone(),
                })
                .collect();
            self.hovered = None;
            self.layout = None;
        }
        self.selected = props
            .options
            .iter()
            .position(|(value, _)| value == props.chosen);
        self.set_enabled(props.enabled);
        if self.label.as_deref() != props.label {
            self.label = props.label.map(str::to_owned);
        }
    }

    /// A line per option, whatever the width.
    fn measure(&self, _width: f32, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        self.lines_height(metrics)
    }

    /// The value of the option the user chose.
    fn changed(&self, response: RadioGroupResponse<T>) -> Option<T> {
        match response {
            RadioGroupResponse::Changed(value) => Some(value),
            RadioGroupResponse::Handled | RadioGroupResponse::Ignored => None,
        }
    }
}

impl<T> Outcome for RadioGroupResponse<T> {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether the options a control holds, `have` as values and labels, are
/// `want`, in order.
pub(super) fn is_same_options<'a, T: PartialEq + 'a>(
    have: impl ExactSizeIterator<Item = (&'a T, &'a str)>,
    want: &[(T, &str)],
) -> bool {
    have.len() == want.len()
        && have
            .zip(want)
            .all(|((value, label), (wanted, wanted_label))| {
                value == wanted && label == *wanted_label
            })
}

/// Draws a radio dot in `dot`: a ring filled with `fill` and edged in
/// `edge` (theme colors), with an inner dot in `edge` when `chosen`.
pub(crate) fn draw_radio_dot(frame: &mut Frame, dot: Rect, chosen: bool, edge: Rgb, fill: Rgb) {
    let scale_factor = frame.scale_factor();
    frame.draw_box(dot, &ring(fill, edge, dot.height, scale_factor));
    if chosen {
        let size = (dot.height * INNER_DOT_SHARE).round();
        let inset = ((dot.height - size) / 2.0).round();
        let inner = Rect::new(dot.x + inset, dot.y + inset, size, size);
        frame.draw_box(inner, &pill(edge, size));
    }
}

/// The style of a dot `size` across: a circle filled with `fill` and
/// ringed with `edge` (theme colors).
fn ring(fill: Rgb, edge: Rgb, size: f32, scale_factor: f64) -> BoxStyle {
    BoxStyle {
        border: Some(Border {
            color: Rgba::from(edge),
            width: logical(MARK_BORDER, scale_factor),
        }),
        ..pill(fill, size)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::Focusable;
    use crate::app::offscreen;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};

    fn group() -> RadioGroup<&'static str> {
        RadioGroup::new()
            .with_option("lf", "Unix (LF)")
            .with_option("crlf", "Windows (CRLF)")
            .with_option("cr", "Classic Mac (CR)")
    }

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    /// `group` drawn at the top left of `window`, returning its area.
    fn draw(window: &TestWindow, group: &mut RadioGroup<&'static str>) -> Rect {
        let mut frame = window.frame();
        let bounds = frame.bounds();
        group.draw(&mut frame, bounds, &Theme::dark())
    }

    #[test]
    fn test_arrow_keys_choose_as_they_move_and_wrap_both_ways() {
        let mut window = TestWindow::keyboard_first(400, 120);
        let mut radio = group();
        assert_eq!(
            window.feed(&mut radio, &key("j")),
            RadioGroupResponse::Ignored,
            "unfocused"
        );
        radio.set_focused(true);
        assert_eq!(
            window.feed(&mut radio, &key("<down>")),
            RadioGroupResponse::Changed("lf")
        );
        assert_eq!(
            window.feed(&mut radio, &key("l")),
            RadioGroupResponse::Changed("crlf")
        );
        assert_eq!(
            window.feed(&mut radio, &key("<end>")),
            RadioGroupResponse::Changed("cr")
        );
        assert_eq!(
            window.feed(&mut radio, &key("j")),
            RadioGroupResponse::Changed("lf"),
            "wrapped"
        );
        assert_eq!(
            window.feed(&mut radio, &key("<up>")),
            RadioGroupResponse::Changed("cr"),
            "wrapped back"
        );
        assert_eq!(
            window.feed(&mut radio, &key("<end>")),
            RadioGroupResponse::Handled,
            "already there"
        );
        assert_eq!(
            window.feed(&mut radio, &key("<home>")),
            RadioGroupResponse::Changed("lf")
        );
        assert_eq!(radio.selected(), Some(&"lf"));
        assert_eq!(
            window.feed(&mut radio, &key("<cr>")),
            RadioGroupResponse::Ignored
        );
    }

    #[test]
    fn test_up_from_no_choice_picks_the_last_and_space_the_first() {
        let mut window = TestWindow::keyboard_first(400, 120);
        let mut radio = group();
        radio.set_focused(true);
        assert_eq!(
            window.feed(&mut radio, &key("k")),
            RadioGroupResponse::Changed("cr")
        );
        let mut fresh = group();
        fresh.set_focused(true);
        assert_eq!(
            window.feed(&mut fresh, &key("<space>")),
            RadioGroupResponse::Changed("lf")
        );
        assert_eq!(
            window.feed(&mut fresh, &key("<space>")),
            RadioGroupResponse::Ignored,
            "chosen already"
        );
    }

    #[test]
    fn test_a_click_chooses_the_option_under_it_and_a_disabled_group_ignores_it() {
        let mut window = TestWindow::keyboard_first(400, 120);
        let mut radio = group();
        let area = draw(&window, &mut radio);
        let line = area.height / 3.0;
        let second = Point::new(area.x + 2.0, area.y + line * 1.5);
        let press = |point| Input::PointerButton(ButtonInput::press(PointerButton::Left, point));
        assert_eq!(
            window.feed(&mut radio, &press(second)),
            RadioGroupResponse::Changed("crlf")
        );
        assert!(window.take_redraw());
        let below = Point::new(area.x + 2.0, area.y + area.height + 2.0);
        assert_eq!(
            window.feed(&mut radio, &press(below)),
            RadioGroupResponse::Ignored
        );
        radio.set_enabled(false);
        let first = Point::new(area.x + 2.0, area.y + line / 2.0);
        assert_eq!(
            window.feed(&mut radio, &press(first)),
            RadioGroupResponse::Ignored
        );
        assert_eq!(radio.selected(), Some(&"crlf"));
    }

    #[test]
    fn test_the_group_describes_its_name_and_options_and_focuses_the_chosen_one() {
        let window = TestWindow::keyboard_first(400, 120);
        let mut radio = group().with_label("Line endings");
        radio.select(1);
        let mut frame = window.frame();
        let bounds = frame.bounds();
        let rect = radio.draw(&mut frame, bounds, &Theme::dark());
        let id = FocusId::new("eol");
        radio.access(&mut frame, id);
        let focusable = Focusable {
            id,
            parent: None,
            rect,
        };
        frame.access.set_focus(vec![focusable], Some(id));
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\"\n\
             \x20 RadioGroup \"Line endings\"\n\
             \x20   RadioButton \"Unix (LF)\" unchecked\n\
             \x20   RadioButton \"Windows (CRLF)\" checked focused\n\
             \x20   RadioButton \"Classic Mac (CR)\" unchecked\n"
        );
        let response = radio.access_action(id, AccessAction::Click(id.child(2)));
        assert_eq!(response, RadioGroupResponse::Changed("cr"));
        assert_eq!(
            radio.access_action(id, AccessAction::Click(id)),
            RadioGroupResponse::Ignored
        );
    }

    #[test]
    fn test_the_chosen_dot_is_filled_in_the_accent_and_the_others_are_empty() {
        let theme = Theme::dark();
        let mut radio = group();
        radio.select(0);
        let Some(snapshot) = offscreen::render(400, 120, |frame| {
            frame.clear(Rgba::from(theme.background));
            radio.draw(frame, Rect::new(10.0, 10.0, 300.0, 100.0), &theme);
        }) else {
            return;
        };
        let dots = radio.layout.as_ref().expect("drawn").dots.clone();
        let center = |dot: Rect| (dot.x + dot.width / 2.0, dot.y + dot.height / 2.0);
        let (x, y) = center(dots[0]);
        snapshot.assert_pixel(x, y, theme.accent, "the chosen option's inner dot");
        let (x, y) = center(dots[1]);
        snapshot.assert_pixel(x, y, theme.background, "an empty dot");
    }

    #[test]
    fn test_sync_chooses_by_value_and_new_options_forget_the_old_rows() {
        let mut window = TestWindow::keyboard_first(400, 120);
        let mut radio = group();
        draw(&window, &mut radio);
        let third = radio.layout.as_ref().expect("drawn").rows[2];
        let options = [("lf", "Unix (LF)"), ("crlf", "Windows (CRLF)")];
        radio.sync(RadioGroupProps {
            chosen: &"crlf",
            enabled: true,
            label: None,
            options: &options,
        });
        assert_eq!(radio.selected(), Some(&"crlf"));
        let press = Input::PointerButton(ButtonInput::press(
            PointerButton::Left,
            Point::new(third.x + 2.0, third.y + 2.0),
        ));
        assert_eq!(
            window.feed(&mut radio, &press),
            RadioGroupResponse::Ignored,
            "no third option to choose"
        );
        radio.sync(RadioGroupProps {
            chosen: &"cr",
            enabled: true,
            label: Some("Line endings"),
            options: &options,
        });
        assert_eq!(radio.selected(), None, "no such option");
    }
}

//! [`Select`]: a dropdown that shows the chosen one of its options in a
//! box with a chevron, and opens a list of all of them below the box (above
//! it when only there it fits) to choose another.
//!
//! Keys, while the app says the select has the focus:
//!
//! - closed: Enter, Space, or Alt+Down opens the list with the current
//!   option highlighted; Up and Down choose the option above or below
//!   straight away, and Home and End the first or the last, without
//!   opening it;
//! - open: Up and Down (or `k` and `j`) move the highlight, wrapping, and
//!   Home and End move it to the first or the last option; any other
//!   letter or digit moves it to the next option starting with it; Enter
//!   or Space chooses the highlighted option; Esc closes the list leaving
//!   the value as it was, and so does Tab, which the select then ignores
//!   so the app moves the focus on.
//!
//! A click on the box opens or closes the list. While the list is open
//! every pointer input is the select's: hovering highlights an option, a
//! click on one chooses it, and a click outside the list closes it. A
//! disabled select is drawn muted and ignores every input.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::color::Rgba;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::list::scroll_to_show;
use super::super::base::theme::{Shape, Theme};
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::FOCUS_STROKE;
use super::label::{draw_label, label_width};
use super::popup::{Popup, PopupSide};
use super::radio::is_same_options;

// ========================================================================
// Constants
// ========================================================================

/// The thickness of the box's border, in logical pixels.
const BORDER: f32 = 1.0;

/// The space above and below the label inside the box, in logical pixels.
const PADDING_Y: f32 = 3.0;

/// The blank cells between the box's edges and its label or chevron.
const PADDING_CELLS: f32 = 1.0;

/// The width of the chevron, in logical pixels.
const CHEVRON_WIDTH: f32 = 8.0;

/// The thickness of the chevron's strokes, in logical pixels.
const CHEVRON_STROKE: f32 = 2.0;

/// An option's row height as a multiple of the cell height.
const ROW_HEIGHT_RATIO: f32 = 1.5;

/// The space above the first and below the last option, in logical pixels.
const LIST_PADDING_Y: f32 = 4.0;

/// The width of the mark beside the current option, in logical pixels.
const MARK_WIDTH: f32 = 2.0;

/// The child index of the list's accessibility node; the options' count
/// from 0.
pub(crate) const LIST_NODE: u64 = 1 << 32;

// ========================================================================
// Data Structures
// ========================================================================

/// A dropdown of options with values of type `T`. The app owns it, hands
/// it the window's keys while it has the focus and its pointer inputs, and
/// draws it each frame; [`SelectResponse::Changed`] says the value changed.
#[derive(Clone, Debug)]
pub struct Select<T> {
    /// Whether it takes input.
    enabled: bool,
    /// Whether the app said it has the keyboard.
    focused: bool,
    /// The option highlighted in the open list.
    highlighted: usize,
    /// The name screen readers give it, if any.
    label: Option<String>,
    /// Where its parts were drawn last, for the pointer.
    layout: Option<SelectLayout>,
    /// Whether the list is open.
    open: bool,
    /// The options, top to bottom.
    options: Vec<SelectOption<T>>,
    /// Where the pointer is, or `None` outside the window.
    pointer: Option<Point>,
    /// The first option shown in the open list, when it holds fewer rows
    /// than there are options.
    scroll: usize,
    /// The index of the chosen option, or `None` without options.
    selected: Option<usize>,
}

/// What the app's state says a [`Select`] shows, for [`Control::sync`].
#[derive(Debug)]
pub struct SelectProps<'a, T> {
    /// The value of the chosen option; a value no option hands back
    /// chooses the first.
    pub chosen: &'a T,
    /// Whether it takes input.
    pub enabled: bool,
    /// The name screen readers give it, if any.
    pub label: Option<&'a str>,
    /// The options, top to bottom: what choosing each hands back, and what
    /// it says.
    pub options: &'a [(T, &'a str)],
}

/// One option of a [`Select`].
#[derive(Clone, Debug)]
struct SelectOption<T> {
    /// What it says.
    label: String,
    /// What choosing it hands back.
    value: T,
}

/// What became of an input offered to a [`Select`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SelectResponse<T> {
    /// The option with this value was chosen in place of another.
    Changed(T),
    /// The list closed leaving the value as it was.
    Closed,
    /// The select used the input.
    Handled,
    /// The input is not the select's.
    Ignored,
    /// The list opened.
    Opened,
}

/// Where a select's parts were drawn.
#[derive(Clone, Debug, PartialEq)]
struct SelectLayout {
    /// The box.
    field: Rect,
    /// The open list, or `None` while closed.
    list: Option<ListLayout>,
}

/// Where an open list's parts were drawn.
#[derive(Clone, Debug, PartialEq)]
struct ListLayout {
    /// The rows' room inside the box: rows outside it are not shown.
    content: Rect,
    /// The box.
    popup: Popup,
    /// Each option's row, in order, scrolled.
    rows: Vec<Rect>,
}

// ========================================================================
// Select Implementations
// ========================================================================

impl<T: Clone> Select<T> {
    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// An enabled, closed select with no options yet.
    pub fn new() -> Self {
        Self {
            enabled: true,
            focused: false,
            highlighted: 0,
            label: None,
            layout: None,
            open: false,
            options: Vec::new(),
            pointer: None,
            scroll: 0,
            selected: None,
        }
    }

    /// This select with an option saying `label` that hands back `value`
    /// added below the others. The first option added is the chosen one.
    pub fn with_option(mut self, value: T, label: impl Into<String>) -> Self {
        self.options.push(SelectOption {
            label: label.into(),
            value,
        });
        self.selected.get_or_insert(0);
        self
    }

    /// This select named `label` for screen readers (the select does not
    /// draw it; the app draws its own label beside it).
    pub fn with_label(self, label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..self
        }
    }

    /// This select enabled or not; see [`Select::set_enabled`].
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.set_enabled(enabled);
        self
    }

    // --------------------------------------------------------------------
    // State
    // --------------------------------------------------------------------

    /// Enables the select, or disables it: drawn muted, it ignores every
    /// input, and its list closes.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.close();
        }
    }

    /// Whether the select takes input.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether the list is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The chosen option's value, or `None` without options.
    pub fn selected(&self) -> Option<&T> {
        self.selected.map(|index| &self.options[index].value)
    }

    /// Chooses the option at `index`; an index past the last option is
    /// ignored.
    pub fn select(&mut self, index: usize) {
        if index < self.options.len() {
            self.selected = Some(index);
        }
    }

    // --------------------------------------------------------------------
    // Accessibility
    // --------------------------------------------------------------------

    /// Describes the select as last drawn to screen readers as `id`: a
    /// combo box named by its label, its value the chosen option's label,
    /// expanded while open. The open list is child `1 << 32` of `id`, a
    /// list box holding the options (the `index`th is child `index` of
    /// `id`), the highlighted one selected and focused while the select
    /// is. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        let mut combo = Node::new(Role::ComboBox);
        if let Some(label) = &self.label {
            combo.set_label(label.as_str());
        }
        if let Some(index) = self.selected {
            combo.set_value(self.options[index].label.as_str());
        }
        combo.set_expanded(self.open);
        match self.enabled {
            true => combo.add_action(Action::Click),
            false => combo.set_disabled(),
        }
        frame.enter_access(id, layout.field, combo);
        if let Some(list) = &layout.list {
            frame.enter_access(
                id.child(LIST_NODE),
                list.popup.rect(),
                Node::new(Role::ListBox),
            );
            for (index, (option, row)) in self.options.iter().zip(&list.rows).enumerate() {
                let mut node = Node::new(Role::ListBoxOption);
                node.set_label(option.label.as_str());
                node.set_selected(index == self.highlighted);
                node.add_action(Action::Click);
                frame.access(id.child(index as u64), *row, node);
            }
            frame.leave_access();
            frame
                .access
                .set_inner_focus(id, id.child(self.highlighted as u64));
        }
        frame.leave_access();
    }

    /// Does what a screen reader's `action` on the select described as
    /// `id` asks: a click on the box opens or closes the list, and a click
    /// on an option of the open list chooses it. Ignored for other actions
    /// and nodes, and while disabled.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> SelectResponse<T> {
        let AccessAction::Click(target) = action else {
            return SelectResponse::Ignored;
        };
        if !self.enabled {
            return SelectResponse::Ignored;
        }
        if target == id {
            return self.toggle_open();
        }
        match (0..self.options.len()).find(|index| id.child(*index as u64) == target) {
            Some(index) if self.open => self.choose(index),
            Some(_) | None => SelectResponse::Ignored,
        }
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Reacts to `key` as the module documentation lists.
    fn handle_key(&mut self, key: Key, vim: bool) -> SelectResponse<T> {
        if !self.focused && !self.open {
            return SelectResponse::Ignored;
        }
        match self.open {
            true => self.handle_open_key(key, vim),
            false => self.handle_closed_key(key),
        }
    }

    /// Reacts to `key` while the list is closed.
    fn handle_closed_key(&mut self, key: Key) -> SelectResponse<T> {
        let plain = key.mods == Mods::NONE;
        match key.code {
            KeyCode::Down if key.mods == Mods::ALT => self.open_list(),
            KeyCode::Enter | KeyCode::Char(' ') if plain => self.open_list(),
            KeyCode::Down if plain => self.step_selected(1),
            KeyCode::Up if plain => self.step_selected(-1),
            KeyCode::Home if plain => self.choose_closed(0),
            KeyCode::End if plain => self.choose_closed(self.options.len().saturating_sub(1)),
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
            | KeyCode::Up => SelectResponse::Ignored,
        }
    }

    /// Reacts to `key` while the list is open, `j` and `k` moving the
    /// highlight where `vim`, else typed toward an option as any letter.
    fn handle_open_key(&mut self, key: Key, vim: bool) -> SelectResponse<T> {
        let plain = key.mods == Mods::NONE;
        let typed = plain || key.mods == Mods::SHIFT;
        match key.code {
            KeyCode::Down if plain => self.move_highlight(1),
            KeyCode::Char('j') if plain && vim => self.move_highlight(1),
            KeyCode::Up if plain => self.move_highlight(-1),
            KeyCode::Char('k') if plain && vim => self.move_highlight(-1),
            KeyCode::Home if plain => self.highlight(0),
            KeyCode::End if plain => self.highlight(self.options.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char(' ') if plain => self.choose(self.highlighted),
            KeyCode::Esc if plain => {
                self.close();
                SelectResponse::Closed
            }
            KeyCode::Tab => {
                self.close();
                SelectResponse::Ignored
            }
            KeyCode::Char(letter) if typed && letter.is_alphanumeric() => self.jump_to(letter),
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
            | KeyCode::Up => SelectResponse::Ignored,
        }
    }

    /// Reacts to a left press at the pointer: on the box it opens or
    /// closes the list, on an option of the open list it chooses it, and
    /// anywhere else it closes the open list.
    fn press(&mut self) -> SelectResponse<T> {
        let (Some(point), Some(layout)) = (self.pointer, &self.layout) else {
            return self.dismiss();
        };
        if layout.field.contains(point) {
            return self.toggle_open();
        }
        match &layout.list {
            Some(list) if list.popup.rect().contains(point) => match list.option_at(point) {
                Some(index) => self.choose(index),
                None => SelectResponse::Handled,
            },
            Some(_) | None => self.dismiss(),
        }
    }

    /// Reacts to pointer `input` and the window losing the focus.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> SelectResponse<T> {
        let was_hovered = self.is_hovered();
        if let Some(position) = input.position() {
            self.pointer = Some(position);
        }
        match input {
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Left =>
            {
                self.press()
            }
            Input::PointerLeft => {
                self.pointer = None;
                self.open_or_ignored()
            }
            Input::PointerMoved(_) => {
                if let Some(index) = self.option_under_pointer() {
                    self.highlighted = index;
                }
                if was_hovered != self.is_hovered() {
                    cx.request_redraw();
                }
                self.open_or_ignored()
            }
            Input::FocusLost if self.open => {
                self.close();
                SelectResponse::Closed
            }
            Input::PointerButton(_) | Input::Wheel(_) => self.open_or_ignored(),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_) => SelectResponse::Ignored,
        }
    }

    // --------------------------------------------------------------------
    // Changes
    // --------------------------------------------------------------------

    /// Opens the list with the chosen option highlighted.
    fn open_list(&mut self) -> SelectResponse<T> {
        if self.options.is_empty() {
            return SelectResponse::Handled;
        }
        self.open = true;
        self.highlighted = self.selected.unwrap_or(0);
        SelectResponse::Opened
    }

    /// Closes the list, leaving the value as it is.
    fn close(&mut self) {
        self.open = false;
        if let Some(layout) = &mut self.layout {
            layout.list = None;
        }
    }

    /// Opens the closed list, or closes the open one.
    fn toggle_open(&mut self) -> SelectResponse<T> {
        match self.open {
            true => self.dismiss(),
            false => self.open_list(),
        }
    }

    /// Closes the open list (a click outside it), or ignores the press
    /// while it is closed.
    fn dismiss(&mut self) -> SelectResponse<T> {
        match self.open {
            true => {
                self.close();
                SelectResponse::Closed
            }
            false => SelectResponse::Ignored,
        }
    }

    /// Chooses the option at `index` and closes the list: changed when it
    /// was not the chosen one already, closed otherwise.
    fn choose(&mut self, index: usize) -> SelectResponse<T> {
        self.close();
        match self.set_selected(index) {
            Some(value) => SelectResponse::Changed(value),
            None => SelectResponse::Closed,
        }
    }

    /// Chooses the option at `index` without opening the list.
    fn choose_closed(&mut self, index: usize) -> SelectResponse<T> {
        match self.set_selected(index) {
            Some(value) => SelectResponse::Changed(value),
            None => SelectResponse::Handled,
        }
    }

    /// Makes the option at `index` the chosen one, returning its value
    /// when that changed the value; `None` when it was chosen already or
    /// there is no such option.
    fn set_selected(&mut self, index: usize) -> Option<T> {
        let option = self.options.get(index)?;
        if self.selected == Some(index) {
            return None;
        }
        self.selected = Some(index);
        Some(option.value.clone())
    }

    /// Chooses the option `delta` below the chosen one (above when
    /// negative), stopping at the ends.
    fn step_selected(&mut self, delta: isize) -> SelectResponse<T> {
        let Some(current) = self.selected else {
            return SelectResponse::Handled;
        };
        let last = self.options.len() as isize - 1;
        self.choose_closed((current as isize + delta).clamp(0, last) as usize)
    }

    /// Moves the highlight `delta` options down (up when negative),
    /// wrapping.
    fn move_highlight(&mut self, delta: isize) -> SelectResponse<T> {
        let count = self.options.len() as isize;
        let index = (self.highlighted as isize + delta).rem_euclid(count.max(1));
        self.highlight(index as usize)
    }

    /// Highlights the option at `index`.
    fn highlight(&mut self, index: usize) -> SelectResponse<T> {
        if index < self.options.len() {
            self.highlighted = index;
        }
        SelectResponse::Handled
    }

    /// Highlights the next option after the highlighted one whose label
    /// starts with `letter`, in either case, wrapping.
    fn jump_to(&mut self, letter: char) -> SelectResponse<T> {
        let count = self.options.len();
        if count == 0 {
            return SelectResponse::Handled;
        }
        let starts = |index: usize| {
            self.options[index]
                .label
                .chars()
                .next()
                .is_some_and(|first| first.to_lowercase().eq(letter.to_lowercase()))
        };
        let found = (1..=count)
            .map(|step| (self.highlighted + step) % count)
            .find(|index| starts(*index));
        match found {
            Some(index) => self.highlight(index),
            None => SelectResponse::Handled,
        }
    }

    // --------------------------------------------------------------------
    // Hit Testing
    // --------------------------------------------------------------------

    /// Handled while the list is open, which takes every pointer input;
    /// ignored while it is closed.
    fn open_or_ignored(&self) -> SelectResponse<T> {
        match self.open {
            true => SelectResponse::Handled,
            false => SelectResponse::Ignored,
        }
    }

    /// Whether the pointer is over the closed box, as last drawn.
    fn is_hovered(&self) -> bool {
        match (self.pointer, &self.layout) {
            (Some(point), Some(layout)) => layout.field.contains(point),
            (None, _) | (_, None) => false,
        }
    }

    /// The option of the open list under the pointer, as last drawn.
    fn option_under_pointer(&self) -> Option<usize> {
        let point = self.pointer?;
        self.layout.as_ref()?.list.as_ref()?.option_at(point)
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws the box in `area`: its label, its chevron, and the focus ring
    /// while focused.
    fn draw_field(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let fill = match self.enabled && !self.open && self.is_hovered() {
            true => theme.hover,
            false => theme.background,
        };
        let border = match self.focused {
            true => Border {
                color: Rgba::from(theme.accent),
                width: logical(FOCUS_STROKE, scale_factor),
            },
            false => Border {
                color: Rgba::from(theme.border),
                width: logical(BORDER, scale_factor),
            },
        };
        let style = BoxStyle {
            border: Some(border),
            ..theme.shape.highlight(fill, scale_factor)
        };
        frame.draw_box(area, &style);
        let color = match self.enabled {
            true => theme.foreground,
            false => theme.muted,
        };
        let padding = (PADDING_CELLS * metrics.width).round();
        let chevron_width = logical(CHEVRON_WIDTH, scale_factor);
        let chevron = Rect::new(
            area.x + area.width - padding - chevron_width,
            area.y,
            chevron_width,
            area.height,
        );
        for stroke in chevron_strokes(chevron, scale_factor) {
            frame.fill_rect(stroke, color);
        }
        let Some(index) = self.selected else {
            return;
        };
        let room = Rect::new(
            area.x + padding,
            area.y,
            (chevron.x - padding - area.x - padding).max(0.0),
            area.height,
        );
        let origin = Point::new(
            room.x,
            (area.y + (area.height - metrics.height) / 2.0).round(),
        );
        let label = &self.options[index].label;
        frame.clipped(room, |frame| draw_label(frame, theme, label, origin, color));
    }

    /// Draws the open list as laid out in `list`: the highlighted option on
    /// the selection color, the chosen one marked in the accent.
    fn draw_list(&self, frame: &mut Frame, list: &ListLayout, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let padding = (PADDING_CELLS * metrics.width).round();
        let mark = logical(MARK_WIDTH, scale_factor);
        list.popup.draw(frame, theme);
        frame.overlay(|frame| {
            frame.clipped(list.content, |frame| {
                for (index, (option, &row)) in self.options.iter().zip(&list.rows).enumerate() {
                    if !rows_overlap(row, list.content) {
                        continue;
                    }
                    if index == self.highlighted {
                        frame.draw_box(
                            row,
                            &theme.shape.highlight(theme.selected_row, scale_factor),
                        );
                    }
                    if Some(index) == self.selected {
                        let bar = Rect::new(row.x, row.y, mark, row.height);
                        frame.fill_rect_over_text(bar, theme.accent);
                    }
                    let top = row.y + ((row.height - metrics.height) / 2.0).round();
                    let origin = Point::new(row.x + padding, top);
                    draw_label(frame, theme, &option.label, origin, theme.foreground);
                }
            });
        });
    }
}

impl<T: Clone> Default for Select<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Widget for Select<T> {
    type Output = Option<Rect>;
    type Response = SelectResponse<T>;

    /// Reacts to `input`: keys while focused or open, as the module
    /// documentation lists, and the pointer on the box or the open list.
    /// A disabled select ignores everything. Asks for a redraw whenever it
    /// used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> SelectResponse<T> {
        if !self.enabled {
            return SelectResponse::Ignored;
        }
        let was_open = self.open;
        let response = match input {
            Input::Key(key) => self.handle_key(*key, cx.key_scheme().vim),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => self.handle_pointer(input, cx),
        };
        let moved = matches!(input, Input::PointerMoved(_) | Input::PointerLeft);
        if (!response.is_ignored() && !moved) || was_open != self.open {
            cx.request_redraw();
        }
        response
    }

    /// Draws the box in `area` and, while open, the list below it (above
    /// when only there it fits) in the frame's overlay, kept inside the
    /// window. Returns where the open list was drawn (for a
    /// [`FocusTree`](crate::FocusTree) trap), or `None` while closed.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Option<Rect> {
        self.draw_field(frame, area, theme);
        let list = self.open.then(|| {
            let metrics = frame.cell_metrics();
            let scale_factor = frame.scale_factor();
            let widest = self
                .options
                .iter()
                .map(|option| label_width(frame, theme, &option.label))
                .fold(0.0, f32::max);
            let window = frame.bounds();
            ListLayout::new(
                self.options.len(),
                widest,
                area,
                window,
                metrics,
                &theme.shape,
                scale_factor,
            )
        });
        let list = list.map(|list| {
            self.scroll = scroll_to_show(self.scroll, self.highlighted, list.visible_rows());
            list.scrolled(self.scroll)
        });
        if let Some(list) = &list {
            self.draw_list(frame, list, theme);
        }
        let rect = list.as_ref().map(|list| list.popup.rect());
        self.layout = Some(SelectLayout { field: area, list });
        rect
    }

    /// Tells the select whether it has the keyboard, which it shows with a
    /// ring in the accent color. Losing it closes the list.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.close();
        }
    }

    /// Whether the app said the select has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl<T: Clone + PartialEq> Control for Select<T> {
    type Props<'a>
        = SelectProps<'a, T>
    where
        T: 'a;
    type Value = T;

    /// Takes the options, the choice, whether it is enabled, and its name
    /// from `props`. New options close an open list.
    fn sync(&mut self, props: SelectProps<'_, T>) {
        let options = self
            .options
            .iter()
            .map(|option| (&option.value, option.label.as_str()));
        if !is_same_options(options, props.options) {
            self.close();
            self.options = props
                .options
                .iter()
                .map(|(value, label)| SelectOption {
                    label: (*label).to_owned(),
                    value: value.clone(),
                })
                .collect();
            self.highlighted = 0;
            self.scroll = 0;
        }
        self.selected = props
            .options
            .iter()
            .position(|(value, _)| value == props.chosen)
            .or_else(|| (!props.options.is_empty()).then_some(0));
        if self.label.as_deref() != props.label {
            self.label = props.label.map(str::to_owned);
        }
        self.set_enabled(props.enabled);
    }

    /// One row inside its box, whatever the width.
    fn measure(&self, _width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        let frame = logical(BORDER, scale_factor) + logical(PADDING_Y, scale_factor);
        (metrics.height + 2.0 * frame).round()
    }

    /// The value of the option the user chose.
    fn changed(&self, response: SelectResponse<T>) -> Option<T> {
        match response {
            SelectResponse::Changed(value) => Some(value),
            SelectResponse::Closed
            | SelectResponse::Handled
            | SelectResponse::Ignored
            | SelectResponse::Opened => None,
        }
    }
}

impl<T> Outcome for SelectResponse<T> {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// ListLayout Implementations
// ========================================================================

impl ListLayout {
    /// The layout of a list of `count` options whose widest label is
    /// `widest` pixels, opened below `field` (above when only there it
    /// fits) inside `window`, at least as wide as `field`, in cells of
    /// `metrics` and a box of `shape` on a display scaled by
    /// `scale_factor`; unscrolled.
    fn new(
        count: usize,
        widest: f32,
        field: Rect,
        window: Rect,
        metrics: CellMetrics,
        shape: &Shape,
        scale_factor: f64,
    ) -> Self {
        let row_height = (metrics.height * ROW_HEIGHT_RATIO).round();
        let padding_y = logical(LIST_PADDING_Y, scale_factor);
        let inset = Popup::inset(shape, scale_factor);
        let padding = (PADDING_CELLS * metrics.width).round();
        let width = (widest + 2.0 * (padding + inset)).ceil().max(field.width);
        let height = count as f32 * row_height + 2.0 * (padding_y + inset);
        let popup = Popup::new(field, width, height, PopupSide::Below, window);
        let inside = popup.content(shape, scale_factor);
        let content = Rect::new(
            inside.x,
            inside.y + padding_y,
            inside.width,
            (inside.height - 2.0 * padding_y).max(0.0),
        );
        let rows = (0..count)
            .map(|index| {
                let y = content.y + index as f32 * row_height;
                Rect::new(content.x, y, content.width, row_height)
            })
            .collect();
        Self {
            content,
            popup,
            rows,
        }
    }

    /// How many whole rows the list shows at once, at least one.
    fn visible_rows(&self) -> usize {
        let row_height = self.rows.first().map_or(1.0, |row| row.height);
        ((self.content.height / row_height).floor() as usize).max(1)
    }

    /// This layout with its rows moved up so the `first`th is at the top.
    fn scrolled(self, first: usize) -> Self {
        let shift = self
            .rows
            .get(first)
            .map_or(0.0, |row| row.y - self.content.y);
        let rows = self
            .rows
            .into_iter()
            .map(|row| Rect::new(row.x, row.y - shift, row.width, row.height))
            .collect();
        Self { rows, ..self }
    }

    /// The option whose shown row holds `point`.
    fn option_at(&self, point: Point) -> Option<usize> {
        if !self.content.contains(point) {
            return None;
        }
        self.rows.iter().position(|row| row.contains(point))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `row` reaches into `room` vertically.
fn rows_overlap(row: Rect, room: Rect) -> bool {
    row.y < room.y + room.height && row.y + row.height > room.y
}

/// The squares that draw a downward chevron [`CHEVRON_WIDTH`] logical
/// pixels wide, as two diagonals of stair steps, centered in `area` on a
/// display scaled by `scale_factor`.
fn chevron_strokes(area: Rect, scale_factor: f64) -> Vec<Rect> {
    let stroke = logical(CHEVRON_STROKE, scale_factor);
    let width = logical(CHEVRON_WIDTH, scale_factor);
    let steps = (width / stroke / 2.0).round() as usize;
    let height = steps as f32 * stroke;
    let left = (area.x + (area.width - width) / 2.0).round();
    let top = (area.y + (area.height - height) / 2.0).round();
    (0..steps)
        .flat_map(|step| {
            let offset = step as f32 * stroke;
            let y = top + offset;
            [
                Rect::new(left + offset, y, stroke, stroke),
                Rect::new(left + width - offset - stroke, y, stroke, stroke),
            ]
        })
        .collect()
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

    const ID: FocusId = FocusId::new("theme");

    /// A focused select of `Dark`, `Light`, and `Dusk`, `Dark` chosen.
    fn select() -> Select<&'static str> {
        let mut select = Select::new()
            .with_label("Theme")
            .with_option("dark", "Dark")
            .with_option("light", "Light")
            .with_option("dusk", "Dusk");
        select.set_focused(true);
        select
    }

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn click(x: f32, y: f32) -> Input {
        Input::PointerButton(ButtonInput::press(PointerButton::Left, Point::new(x, y)))
    }

    /// Draws `select` in a box at the top left of `window`'s frame.
    fn draw(select: &mut Select<&'static str>, window: &TestWindow) -> (Rect, Option<Rect>) {
        let mut frame = window.frame();
        let field = Rect::new(10.0, 10.0, 200.0, 30.0);
        let list = select.draw(&mut frame, field, &Theme::dark());
        (field, list)
    }

    #[test]
    fn test_closed_arrows_change_the_value_in_place_and_stop_at_the_ends() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut select = select();
        assert_eq!(
            window.feed(&mut select, &key("<down>")),
            SelectResponse::Changed("light")
        );
        assert!(!select.is_open(), "changed without opening");
        assert_eq!(
            window.feed(&mut select, &key("<end>")),
            SelectResponse::Changed("dusk")
        );
        assert_eq!(
            window.feed(&mut select, &key("<down>")),
            SelectResponse::Handled,
            "no wrap at the end"
        );
        assert_eq!(select.selected(), Some(&"dusk"));
        select.set_focused(false);
        assert_eq!(
            window.feed(&mut select, &key("<up>")),
            SelectResponse::Ignored,
            "keys only while focused"
        );
    }

    #[test]
    fn test_open_keys_move_wrap_and_jump_then_choose_or_close_unchanged() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut select = select();
        assert_eq!(
            window.feed(&mut select, &key("<space>")),
            SelectResponse::Opened
        );
        assert_eq!(window.feed(&mut select, &key("k")), SelectResponse::Handled);
        assert_eq!(select.highlighted, 2, "wrapped to the last");
        window.feed(&mut select, &key("L"));
        assert_eq!(select.highlighted, 1, "jumped to Light");
        window.feed(&mut select, &key("d"));
        assert_eq!(select.highlighted, 2, "the next starting with d is Dusk");
        assert_eq!(
            window.feed(&mut select, &key("<cr>")),
            SelectResponse::Changed("dusk")
        );
        assert!(!select.is_open());
        window.feed(&mut select, &key("<cr>"));
        assert_eq!(
            window.feed(&mut select, &key("<cr>")),
            SelectResponse::Closed,
            "same"
        );
        window.feed(&mut select, &key("M-<down>"));
        window.feed(&mut select, &key("j"));
        assert_eq!(
            window.feed(&mut select, &key("<esc>")),
            SelectResponse::Closed
        );
        assert_eq!(select.selected(), Some(&"dusk"), "Esc leaves the value");
        window.feed(&mut select, &key("<cr>"));
        assert_eq!(
            window.feed(&mut select, &key("<tab>")),
            SelectResponse::Ignored,
            "Tab is the app's, to move the focus"
        );
        assert!(!select.is_open(), "but it closes the list");
    }

    #[test]
    fn test_clicks_open_choose_and_close_and_the_open_list_takes_every_press() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut select = select();
        draw(&mut select, &window);
        assert_eq!(
            window.feed(&mut select, &click(20.0, 20.0)),
            SelectResponse::Opened
        );
        let (_, list) = draw(&mut select, &window);
        let list = list.expect("drawn open");
        assert!(list.y >= 40.0, "below the box");
        let rows = select
            .layout
            .as_ref()
            .and_then(|l| l.list.clone())
            .expect("open")
            .rows;
        let light = rows[1];
        let (x, y) = (light.x + 5.0, light.y + light.height / 2.0);
        window.feed(&mut select, &Input::PointerMoved(Point::new(x, y)));
        assert_eq!(select.highlighted, 1, "hover highlights");
        assert_eq!(
            window.feed(&mut select, &click(x, y)),
            SelectResponse::Changed("light")
        );
        draw(&mut select, &window);
        window.feed(&mut select, &click(20.0, 20.0));
        draw(&mut select, &window);
        assert_eq!(
            window.feed(&mut select, &click(390.0, 290.0)),
            SelectResponse::Closed,
            "a click outside closes and is consumed"
        );
        assert_eq!(
            window.feed(&mut select, &click(390.0, 290.0)),
            SelectResponse::Ignored,
            "closed, a click elsewhere is not the select's"
        );
    }

    #[test]
    fn test_a_disabled_select_ignores_everything() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut select = select().with_enabled(false);
        draw(&mut select, &window);
        assert_eq!(
            window.feed(&mut select, &key("<cr>")),
            SelectResponse::Ignored
        );
        assert_eq!(
            window.feed(&mut select, &click(20.0, 20.0)),
            SelectResponse::Ignored
        );
        assert!(!window.take_redraw());
    }

    #[test]
    fn test_a_long_list_cut_by_the_window_scrolls_to_keep_the_highlight_shown() {
        let window = TestWindow::keyboard_first(400, 120);
        let mut select = (0..20).fold(Select::new(), |select, index| {
            select.with_option(index, format!("Item {index}"))
        });
        select.set_focused(true);
        let mut frame = window.frame();
        select.handle_key(parse_key("<cr>").expect("a valid key"), true);
        select.handle_key(parse_key("<end>").expect("a valid key"), true);
        let field = Rect::new(0.0, 0.0, 100.0, 30.0);
        select.draw(&mut frame, field, &Theme::dark());
        let list = select
            .layout
            .as_ref()
            .and_then(|l| l.list.clone())
            .expect("open");
        let last = list.rows[19];
        assert!(
            last.y >= list.content.y
                && last.y + last.height <= list.content.y + list.content.height,
            "the last row is shown: {last:?} in {:?}",
            list.content
        );
        assert!(list.rows[0].y < list.content.y, "the first is scrolled off");
    }

    #[test]
    fn test_the_select_describes_itself_and_its_open_list_to_screen_readers() {
        let window = TestWindow::keyboard_first(400, 300);
        let mut select = select();
        select.handle_key(parse_key("<cr>").expect("a valid key"), true);
        select.handle_key(parse_key("j").expect("a valid key"), true);
        let mut frame = window.frame();
        let field = Rect::new(10.0, 10.0, 200.0, 30.0);
        select.draw(&mut frame, field, &Theme::dark());
        select.access(&mut frame, ID);
        let focusable = Focusable {
            id: ID,
            parent: None,
            rect: field,
        };
        frame.access.set_focus(vec![focusable], Some(ID));
        assert_eq!(
            ui_text(&frame, "Prefs"),
            "Window \"Prefs\"\n\
             \x20 ComboBox \"Theme\" value=\"Dark\" expanded\n\
             \x20   ListBox\n\
             \x20     ListBoxOption \"Dark\"\n\
             \x20     ListBoxOption \"Light\" selected focused\n\
             \x20     ListBoxOption \"Dusk\"\n"
        );
        assert_eq!(
            select.access_action(ID, AccessAction::Click(ID.child(2))),
            SelectResponse::Changed("dusk")
        );
        assert_eq!(
            select.access_action(ID, AccessAction::Click(ID)),
            SelectResponse::Opened
        );
        assert_eq!(
            select.access_action(ID, AccessAction::Click(ID)),
            SelectResponse::Closed
        );
        assert_eq!(
            select.access_action(ID, AccessAction::Click(ID.child(0))),
            SelectResponse::Ignored,
            "no option to click while closed"
        );
    }

    #[test]
    fn test_the_focused_box_is_ringed_and_the_open_list_lights_its_highlight() {
        let theme = Theme::dark();
        let mut select = select();
        select.handle_key(parse_key("<cr>").expect("a valid key"), true);
        select.handle_key(parse_key("j").expect("a valid key"), true);
        let field = Rect::new(10.0, 10.0, 200.0, 30.0);
        let Some(snapshot) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            select.draw(frame, field, &theme);
        }) else {
            return;
        };
        snapshot.assert_pixel(field.x, field.y + 15.0, theme.accent, "the focus ring");
        let list = select
            .layout
            .as_ref()
            .and_then(|l| l.list.clone())
            .expect("open");
        let [dark, light] = [list.rows[0], list.rows[1]];
        snapshot.assert_pixel(
            dark.x,
            dark.y + 2.0,
            theme.accent,
            "the current option's mark",
        );
        snapshot.assert_pixel(
            light.x + light.width - 4.0,
            light.y + 2.0,
            theme.selection,
            "the highlight",
        );
        snapshot.assert_pixel(
            dark.x + dark.width - 4.0,
            dark.y + 2.0,
            theme.raised,
            "plain",
        );
    }

    #[test]
    fn test_sync_with_new_options_closes_the_list_and_chooses_by_value() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut select = select();
        draw(&mut select, &window);
        window.feed(&mut select, &key("<space>"));
        assert!(select.is_open());
        let options = [("light", "Light"), ("dark", "Dark")];
        let props = |chosen| SelectProps {
            chosen,
            enabled: true,
            label: Some("Theme"),
            options: &options,
        };
        select.sync(props(&"dark"));
        assert!(!select.is_open(), "new options close the list");
        assert_eq!(select.selected(), Some(&"dark"));
        select.sync(props(&"dusk"));
        assert_eq!(
            select.selected(),
            Some(&"light"),
            "no such option: the first"
        );
        assert_eq!(
            select.changed(SelectResponse::Changed("dark")),
            Some("dark")
        );
        assert_eq!(select.changed(SelectResponse::Opened), None);
    }
}

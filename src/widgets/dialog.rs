//! [`Dialog`]: a modal box with a title, a body of rich [`Paragraph`]s
//! wrapped to its width, and a row of buttons, laid out with the
//! [`layout`](super::layout) engine and centered over a dimmed backdrop.
//!
//! Keys: Tab and Shift+Tab (or Right and Left, `l` and `h`) move the focus
//! between the buttons, wrapping; Enter or Space chooses the focused
//! button; Esc dismisses the dialog. The first primary button has the
//! focus when the dialog opens. A click on a button chooses it and a click
//! on a link in the body hands back its target; a click elsewhere does
//! nothing, so a stray click never answers a question.
//!
//! A dialog built [`Dialog::with_content`] keeps room under its body for a
//! widget of the app's, such as a [`List`](crate::List) to choose from or
//! toggle rows in. The dialog draws its box around the room and says where
//! it is ([`Dialog::content_rect`]); the app draws its widget there after
//! the dialog, in the frame's overlay. The content has the focus when the
//! dialog opens, and Tab and Shift+Tab move it on to the buttons and back.
//! While the content has the focus, hand each key to the content's widget
//! first and to the dialog only the keys it ignored: the dialog then
//! answers Tab, Shift+Tab, and Esc and ignores the rest, so Enter, Space,
//! and the letters stay the content's. Pointer input over the content is
//! [`DialogResponse::Ignored`] (the content takes the focus), for the app
//! to hand to its widget.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::layout::{Align, Edges, Element, Justify, Length, Style};
use super::super::render::paragraph::{Paragraph, ParagraphLayout};
use super::button::{ButtonFace, ButtonVariant, face_height};
use super::label::{draw_styled_label, label_width};
use super::popup::{Popup, draw_backdrop};

// ========================================================================
// Constants
// ========================================================================

/// The child index of the first body paragraph's accessibility node; the
/// buttons' count from 0.
const PARAGRAPH_NODES: u64 = 1 << 32;

/// The widest the dialog grows, in cells.
const MAX_COLS: usize = 64;

/// The blank margin kept between the dialog and the window's edges, in
/// cells.
const WINDOW_MARGIN_CELLS: f32 = 2.0;

/// The blank cells on each side of a button's label.
const BUTTON_PADDING_CELLS: usize = 2;

/// The title's color.
const TITLE_COLOR: TextColor = TextColor::Token(ThemeToken::Foreground);

// ========================================================================
// Data Structures
// ========================================================================

/// A modal dialog whose buttons hand back values of type `T`.
#[derive(Clone, Debug)]
pub struct Dialog<T> {
    /// The body's paragraphs, top to bottom.
    body: Vec<Paragraph>,
    /// The buttons, left to right.
    buttons: Vec<DialogButton<T>>,
    /// How many rows of room the app's content takes under the body, or
    /// `None` for a dialog with no content.
    content_rows: Option<usize>,
    /// Whether the content, rather than a button, has the focus.
    content_focused: bool,
    /// The index of the focused button.
    focus: usize,
    /// Where the parts were drawn last, for the pointer.
    layout: Option<DialogLayout>,
    /// Where the pointer is, or `None` outside the window.
    pointer: Option<Point>,
    /// The heading.
    title: String,
}

/// One button of a [`Dialog`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DialogButton<T> {
    /// What it says.
    label: String,
    /// Whether it is the answer the dialog proposes, drawn in the accent.
    primary: bool,
    /// What choosing it hands back.
    value: T,
}

/// What became of a key or event offered to a [`Dialog`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DialogResponse<T> {
    /// The button with this value was chosen: close the dialog and act.
    Activated(T),
    /// The user dismissed the dialog with Esc: close it, doing nothing.
    Closed,
    /// The dialog used the key or event.
    Handled,
    /// The key is not the dialog's.
    Ignored,
    /// The link to this target in the body was clicked: follow it; the
    /// dialog stays open.
    Link(String),
}

/// Where a dialog's parts were drawn.
#[derive(Clone, Debug, PartialEq)]
struct DialogLayout {
    /// Each button.
    buttons: Vec<Rect>,
    /// The room kept for the app's content, in a dialog with content.
    content: Option<Rect>,
    /// The whole dialog.
    page: Rect,
    /// Where each body paragraph was drawn.
    paragraph_rects: Vec<Rect>,
    /// Where each body paragraph's text and links landed.
    paragraphs: Vec<ParagraphLayout>,
}

/// The parts of a dialog's layout tree.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Part {
    /// The body paragraph at this index.
    Paragraph(usize),
    /// The button at this index.
    Button(usize),
    /// The room kept for the app's content.
    Content,
    /// The whole dialog.
    Page,
    /// The heading.
    Title,
}

// ========================================================================
// Dialog Implementations
// ========================================================================

impl<T: Clone> Dialog<T> {
    /// A dialog headed `title` showing the paragraphs of `body`, with no
    /// buttons yet.
    pub fn new(title: impl Into<String>, body: Vec<Paragraph>) -> Self {
        Self {
            body,
            buttons: Vec::new(),
            content_rows: None,
            content_focused: false,
            focus: 0,
            layout: None,
            pointer: None,
            title: title.into(),
        }
    }

    /// This dialog with `button` added at the right of its buttons. The
    /// first primary button takes the focus.
    pub fn with_button(mut self, button: DialogButton<T>) -> Self {
        let has_primary = self.buttons.iter().any(|button| button.primary);
        if button.primary && !has_primary {
            self.focus = self.buttons.len();
        }
        self.buttons.push(button);
        self
    }

    /// This dialog with room `rows` rows tall under its body for a widget
    /// of the app's (a list of rows to toggle, a table, a form), which has
    /// the focus when the dialog opens. See the module documentation for
    /// drawing it and routing its input.
    pub fn with_content(self, rows: usize) -> Self {
        Self {
            content_rows: Some(rows),
            content_focused: true,
            ..self
        }
    }

    /// Where the room for the app's content was in the last frame drawn, in
    /// physical pixels, to draw the content's widget there; `None` before
    /// the first frame and in a dialog with no content.
    pub fn content_rect(&self) -> Option<Rect> {
        self.layout.as_ref().and_then(|layout| layout.content)
    }

    /// Whether the app's content, rather than a button, has the focus: the
    /// content's widget then takes the keys first and draws as focused.
    pub fn is_content_focused(&self) -> bool {
        self.content_focused
    }

    /// The value of the focused button, or `None` without buttons or while
    /// the content has the focus.
    pub fn focused(&self) -> Option<&T> {
        match self.content_focused {
            true => None,
            false => self.buttons.get(self.focus).map(|button| &button.value),
        }
    }

    /// Reacts to `key`. Hand the dialog every key while it is open; while
    /// its content has the focus, only those the content's widget ignored.
    pub(crate) fn handle_key(&mut self, key: Key) -> DialogResponse<T> {
        let plain = key.mods == Mods::NONE;
        let shifted = key.mods == Mods::SHIFT;
        if self.content_focused {
            return match key.code {
                KeyCode::Esc if plain => DialogResponse::Closed,
                KeyCode::Tab if shifted => self.focus_button(self.buttons.len().saturating_sub(1)),
                KeyCode::Tab if plain => self.focus_button(0),
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
                | KeyCode::Up => DialogResponse::Ignored,
            };
        }
        match key.code {
            KeyCode::Esc if plain => DialogResponse::Closed,
            KeyCode::Enter | KeyCode::Char(' ') if plain => match self.focused() {
                Some(value) => DialogResponse::Activated(value.clone()),
                None => DialogResponse::Handled,
            },
            KeyCode::Tab if shifted => self.move_focus(-1),
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') if plain => self.move_focus(1),
            KeyCode::Left | KeyCode::Char('h') if plain => self.move_focus(-1),
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
            | KeyCode::Up => DialogResponse::Ignored,
        }
    }

    /// Reacts to pointer `input`: a click on a button chooses
    /// it. The dialog is modal: every pointer press is its own.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> DialogResponse<T> {
        match input {
            Input::PointerButton(button) if self.is_over_content(button.position) => {
                self.pointer = Some(button.position);
                if button.pressed && !self.content_focused {
                    self.content_focused = true;
                    cx.request_redraw();
                }
                DialogResponse::Ignored
            }
            Input::Wheel(wheel) if self.is_over_content(wheel.position) => DialogResponse::Ignored,
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Left =>
            {
                self.pointer = Some(button.position);
                cx.request_redraw();
                if let Some(target) = self.link_at_pointer() {
                    return DialogResponse::Link(target.to_owned());
                }
                match self.button_at_pointer() {
                    Some(index) => {
                        self.focus = index;
                        self.content_focused = false;
                        DialogResponse::Activated(self.buttons[index].value.clone())
                    }
                    None => DialogResponse::Handled,
                }
            }
            Input::PointerLeft => {
                self.pointer = None;
                match self.content_rows {
                    Some(_) => DialogResponse::Ignored,
                    None => DialogResponse::Handled,
                }
            }
            Input::PointerMoved(position) => {
                self.pointer = Some(*position);
                match self.is_over_content(*position) {
                    true => DialogResponse::Ignored,
                    false => DialogResponse::Handled,
                }
            }
            Input::PointerButton(_) | Input::Wheel(_) => DialogResponse::Handled,
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_) => DialogResponse::Ignored,
        }
    }

    /// Describes the dialog as last drawn to screen readers as `id`: a
    /// modal dialog named by its title, holding its body paragraphs (the
    /// `index`th is child `1 << 32` plus `index` of `id`) and its buttons
    /// (the `index`th is child `index`), each clickable; the focused button
    /// has the focus while the dialog does. Nothing before the first
    /// frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(self.title.as_str());
        dialog.set_modal();
        frame.enter_access(id, layout.page, dialog);
        for (index, (paragraph, rect)) in self.body.iter().zip(&layout.paragraph_rects).enumerate()
        {
            let mut node = Node::new(Role::Paragraph);
            node.set_label(paragraph.text());
            frame.access(id.child(PARAGRAPH_NODES + index as u64), *rect, node);
        }
        for (index, (button, rect)) in self.buttons.iter().zip(&layout.buttons).enumerate() {
            let mut node = Node::new(Role::Button);
            node.set_label(button.label.as_str());
            node.add_action(Action::Click);
            frame.access(id.child(index as u64), *rect, node);
        }
        frame.leave_access();
        // The content's widget describes itself, and its focus with it.
        if !self.content_focused {
            frame
                .access
                .set_inner_focus(id, id.child(self.focus as u64));
        }
    }

    /// Does what a screen reader's `action` on the dialog described as `id`
    /// asks: a click on a button chooses it. Ignored for other actions and
    /// nodes.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> DialogResponse<T> {
        let AccessAction::Click(target) = action else {
            return DialogResponse::Ignored;
        };
        match (0..self.buttons.len()).find(|index| id.child(*index as u64) == target) {
            Some(index) => {
                self.focus = index;
                self.content_focused = false;
                DialogResponse::Activated(self.buttons[index].value.clone())
            }
            None => DialogResponse::Ignored,
        }
    }

    /// Draws button `index` in `rect`: the primary one filled with the
    /// accent, the focused one outlined in it.
    fn draw_button(&self, frame: &mut Frame, index: usize, rect: Rect, theme: &Theme) {
        let button = &self.buttons[index];
        let variant = match button.primary {
            true => ButtonVariant::Primary,
            false => ButtonVariant::Default,
        };
        let face = ButtonFace {
            active: false,
            enabled: true,
            focus_ring: !self.content_focused && index == self.focus,
            hover: 0.0,
            icon: None,
            label: &button.label,
            neutral_icon: false,
            pressed: false,
            variant,
        };
        face.draw(frame, rect, theme);
    }

    /// Moves the focus `delta` buttons, wrapping, or onto the content from
    /// the last button forward or the first back, in a dialog with content.
    fn move_focus(&mut self, delta: isize) -> DialogResponse<T> {
        let count = self.buttons.len();
        if count == 0 {
            return DialogResponse::Handled;
        }
        let next = self.focus as isize + delta;
        if self.content_rows.is_some() && !(0..count as isize).contains(&next) {
            self.content_focused = true;
            return DialogResponse::Handled;
        }
        self.focus = next.rem_euclid(count as isize) as usize;
        DialogResponse::Handled
    }

    /// Gives the focus to button `index` from the content, or keeps it on
    /// the content in a dialog with no buttons.
    fn focus_button(&mut self, index: usize) -> DialogResponse<T> {
        if index < self.buttons.len() {
            self.focus = index;
            self.content_focused = false;
        }
        DialogResponse::Handled
    }

    /// Whether `point` is over the room kept for the content, as last drawn.
    fn is_over_content(&self, point: Point) -> bool {
        self.content_rect().is_some_and(|rect| rect.contains(point))
    }

    /// The target of the body link under the pointer, as last drawn.
    fn link_at_pointer(&self) -> Option<&str> {
        let point = self.pointer?;
        self.layout
            .as_ref()?
            .paragraphs
            .iter()
            .find_map(|paragraph| paragraph.link_at(point))
    }

    /// The button under the pointer, as last drawn.
    fn button_at_pointer(&self) -> Option<usize> {
        let point = self.pointer?;
        self.layout
            .as_ref()?
            .buttons
            .iter()
            .position(|rect| rect.contains(point))
    }
}

impl<T: Clone> Widget for Dialog<T> {
    type Output = Rect;
    type Response = DialogResponse<T>;

    /// Reacts to `input`: keys as the module documentation lists, and a
    /// click on a button choosing it. The dialog is modal: every pointer
    /// press is its own. Asks for a redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> DialogResponse<T> {
        let response = match input {
            Input::Key(key) => self.handle_key(*key),
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
            | Input::Wheel(_) => return self.handle_pointer(input, cx),
        };
        if !matches!(response, DialogResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the dialog centered in `area` in the frame's overlay, over a
    /// dimmed backdrop across `area`, and returns where the dialog itself was drawn (for a
    /// [`FocusTree`](crate::FocusTree) trap).
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let window = area;
        let (cell, row) = (metrics.width, metrics.height);
        let inset = Popup::inset(&theme.shape, scale_factor);
        let margin = WINDOW_MARGIN_CELLS * cell;
        let width = (window.width - 2.0 * margin)
            .min(MAX_COLS as f32 * cell)
            .max(0.0)
            .floor();
        let padding = cell.round() + inset;
        let button_height = face_height(metrics);
        let buttons = self
            .buttons
            .iter()
            .enumerate()
            .map(|(index, button)| {
                let padding = 2.0 * BUTTON_PADDING_CELLS as f32 * cell;
                let width = (label_width(frame, theme, &button.label) + padding).ceil();
                let style = Style::new()
                    .with_width(Length::Fixed(width))
                    .with_height(Length::Fixed(button_height));
                Element::leaf(style).with_key(Part::Button(index))
            })
            .collect();
        let line = |rows: f32| Style::new().with_height(Length::Fixed(rows * row));
        let content = self
            .content_rows
            .map(|rows| Element::leaf(line(rows as f32)).with_key(Part::Content));
        let (page, title, paragraphs, button_rects, content_rect) = {
            let measurer: &Frame = frame;
            let body = self
                .body
                .iter()
                .enumerate()
                .map(|(index, paragraph)| {
                    let measure = move |width| measurer.measure_paragraph(paragraph, width);
                    Element::measured(Style::new(), measure).with_key(Part::Paragraph(index))
                })
                .collect();
            let tree = Element::column(
                Style::new()
                    .with_width(Length::Fixed(width))
                    .with_padding(Edges::all(padding))
                    .with_gap(row / 2.0),
                [
                    Element::leaf(line(1.0)).with_key(Part::Title),
                    Element::column(Style::new().with_gap(row / 2.0), body),
                ]
                .into_iter()
                .chain(content)
                .chain([Element::row(
                    Style::new()
                        .with_justify(Justify::End)
                        .with_align(Align::Center)
                        .with_gap(cell),
                    buttons,
                )])
                .collect(),
            )
            .with_key(Part::Page);
            let probe = tree.compute(window);
            let size = probe.rect(&Part::Page).unwrap_or_default();
            let x = (window.x + (window.width - size.width) / 2.0).round();
            let y = (window.y + (window.height - size.height).max(0.0) / 2.0).round();
            let layout = tree.compute(Rect::new(x, y, size.width, window.height));
            let rect = |part| layout.rect(&part).unwrap_or_default();
            let paragraphs: Vec<Rect> = (0..self.body.len())
                .map(|index| rect(Part::Paragraph(index)))
                .collect();
            let buttons: Vec<Rect> = (0..self.buttons.len())
                .map(|index| rect(Part::Button(index)))
                .collect();
            let content = self.content_rows.map(|_| rect(Part::Content));
            (
                rect(Part::Page),
                rect(Part::Title),
                paragraphs,
                buttons,
                content,
            )
        };
        draw_backdrop(frame, window, theme);
        Popup::with_rect(page).draw(frame, theme);
        let mut drawn = Vec::with_capacity(self.body.len());
        frame.overlay(|frame| {
            let heading = Span::new(self.title.as_str(), TITLE_COLOR).emphasized(theme.emphasis);
            let heading = StyledText::from(heading);
            draw_styled_label(
                frame,
                theme,
                &heading.with_ellipsis(),
                Point::new(title.x, title.y),
                title,
            );
            for (paragraph, rect) in self.body.iter().zip(&paragraphs) {
                drawn.push(frame.draw_paragraph(paragraph, *rect, theme));
            }
            for (index, rect) in button_rects.iter().enumerate() {
                self.draw_button(frame, index, *rect, theme);
            }
        });
        self.layout = Some(DialogLayout {
            buttons: button_rects,
            content: content_rect,
            page,
            paragraph_rects: paragraphs,
            paragraphs: drawn,
        });
        page
    }
}

impl<T> Outcome for DialogResponse<T> {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// DialogButton Implementations
// ========================================================================

impl<T> DialogButton<T> {
    /// A button saying `label` that hands back `value`.
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            primary: false,
            value,
        }
    }

    /// This button as the answer the dialog proposes: drawn in the accent,
    /// focused when the dialog opens.
    pub fn primary(self) -> Self {
        Self {
            primary: true,
            ..self
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::{Focusable, find_node, node_id};
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::TestWindow;

    fn dialog() -> Dialog<&'static str> {
        Dialog::new(
            "Unsaved changes",
            vec![Paragraph::plain(
                "Save notes.txt before closing?",
                ThemeToken::Foreground.into(),
            )],
        )
        .with_button(DialogButton::new("discard", "Don't Save"))
        .with_button(DialogButton::new("cancel", "Cancel"))
        .with_button(DialogButton::new("save", "Save").primary())
    }

    fn press(dialog: &mut Dialog<&'static str>, spec: &str) -> DialogResponse<&'static str> {
        dialog.handle_key(parse_key(spec).expect("a valid key"))
    }

    #[test]
    fn test_the_primary_button_starts_focused_and_tab_wraps_both_ways() {
        let mut dialog = dialog();
        assert_eq!(dialog.focused(), Some(&"save"));
        press(&mut dialog, "<tab>");
        assert_eq!(dialog.focused(), Some(&"discard"), "wrapped forward");
        press(&mut dialog, "S-<tab>");
        press(&mut dialog, "h");
        assert_eq!(dialog.focused(), Some(&"cancel"));
        assert_eq!(
            press(&mut dialog, "<cr>"),
            DialogResponse::Activated("cancel")
        );
        assert_eq!(press(&mut dialog, "<esc>"), DialogResponse::Closed);
        assert_eq!(press(&mut dialog, "x"), DialogResponse::Ignored);
    }

    #[test]
    fn test_the_dialog_is_centered_with_its_buttons_right_aligned_and_lit() {
        let theme = Theme::dark();
        let mut dialog = dialog();
        press(&mut dialog, "<tab>");
        let (width, height) = (800, 400);
        let Some(snapshot) = offscreen::render(width, height, |frame| {
            frame.clear(Rgba::from(theme.background));
            let bounds = frame.bounds();
            dialog.draw(frame, bounds, &theme);
        }) else {
            return;
        };
        let buttons = dialog.layout.as_ref().expect("drawn").buttons.clone();
        let [discard, cancel, save] = [buttons[0], buttons[1], buttons[2]];
        assert!(discard.x < cancel.x && cancel.x < save.x, "left to right");
        let page_right = (width as f32 + 64.0 * snapshot.metrics().width) / 2.0;
        assert!(save.x + save.width <= page_right && save.x + save.width > page_right - 40.0);
        snapshot.assert_pixel(
            save.x + 4.0,
            save.y + save.height / 2.0,
            theme.accent,
            "primary",
        );
        let middle = discard.y + discard.height / 2.0;
        snapshot.assert_pixel(discard.x, middle, theme.accent, "the focus ring");
        snapshot.assert_pixel(discard.x + 4.0, middle, theme.hover, "inside the ring");
        snapshot.assert_pixel(
            cancel.x + 4.0,
            cancel.y + 4.0,
            theme.hover,
            "a plain button",
        );
        snapshot.assert_pixel(
            cancel.x + 1.0,
            cancel.y + cancel.height / 2.0,
            theme.hover,
            "no ring",
        );
    }

    #[test]
    fn test_the_dialog_describes_its_text_and_buttons_and_its_focused_button_has_focus() {
        let id = FocusId::new("dialog");
        let mut dialog = dialog();
        let mut frame = offscreen::plain_frame(800, 600);
        let bounds = frame.bounds();
        let rect = dialog.draw(&mut frame, bounds, &Theme::dark());
        dialog.access(&mut frame, id);
        let focusable = Focusable {
            id,
            parent: None,
            rect,
        };
        frame.access.set_focus(vec![focusable], Some(id));
        let update = frame.access.tree_update("t");
        let node = |id| find_node(&update, id).expect("described");
        let page = node(id);
        assert_eq!(
            (page.role(), page.label(), page.is_modal()),
            (Role::Dialog, Some("Unsaved changes"), true)
        );
        let body = node(id.child(PARAGRAPH_NODES));
        assert_eq!(body.label(), Some("Save notes.txt before closing?"));
        assert_eq!(node(id.child(0)).label(), Some("Don't Save"));
        assert_eq!(update.focus, node_id(id.child(2)), "the primary button");
        assert_eq!(
            dialog.access_action(id, AccessAction::Click(id.child(1))),
            DialogResponse::Activated("cancel")
        );
        assert_eq!(
            dialog.access_action(id, AccessAction::Click(id.child(PARAGRAPH_NODES))),
            DialogResponse::Ignored,
            "a paragraph is no button"
        );
    }

    /// A dialog with five rows of content above a Close and a primary Add
    /// button.
    fn with_content() -> Dialog<&'static str> {
        Dialog::new("Dictionaries", Vec::new())
            .with_content(5)
            .with_button(DialogButton::new("close", "Close"))
            .with_button(DialogButton::new("add", "Add").primary())
    }

    #[test]
    fn test_content_starts_focused_leaves_its_keys_alone_and_tab_steps_through_the_buttons() {
        let mut dialog = with_content();
        assert!(dialog.is_content_focused());
        assert_eq!(dialog.focused(), None, "no button while the content is");
        for spec in ["<cr>", "<space>", "l", "j", "<down>"] {
            assert_eq!(press(&mut dialog, spec), DialogResponse::Ignored, "{spec}");
        }
        assert!(dialog.is_content_focused());
        press(&mut dialog, "<tab>");
        assert_eq!(dialog.focused(), Some(&"close"), "the first button");
        press(&mut dialog, "<tab>");
        assert_eq!(press(&mut dialog, "<cr>"), DialogResponse::Activated("add"));
        press(&mut dialog, "<tab>");
        assert!(dialog.is_content_focused(), "past the last button");
        press(&mut dialog, "S-<tab>");
        assert_eq!(dialog.focused(), Some(&"add"), "back to the last button");
        press(&mut dialog, "S-<tab>");
        press(&mut dialog, "S-<tab>");
        assert!(dialog.is_content_focused(), "before the first button");
        assert_eq!(press(&mut dialog, "<esc>"), DialogResponse::Closed);
    }

    #[test]
    fn test_the_content_room_sits_between_body_and_buttons_and_its_pointer_is_the_app_s() {
        let mut window = TestWindow::new(800, 600);
        let mut plain = dialog();
        plain.draw(
            &mut window.frame(),
            Rect::new(0.0, 0.0, 800.0, 600.0),
            &Theme::dark(),
        );
        assert_eq!(plain.content_rect(), None, "no content, no room");
        let mut dialog = with_content();
        press(&mut dialog, "<tab>");
        let mut frame = window.frame();
        let bounds = frame.bounds();
        let page = dialog.draw(&mut frame, bounds, &Theme::dark());
        let content = dialog.content_rect().expect("room for the content");
        let metrics = frame.cell_metrics();
        assert_eq!(content.height, 5.0 * metrics.height, "five rows");
        let buttons = dialog.layout.as_ref().expect("drawn").buttons.clone();
        assert!(
            content.y + content.height <= buttons[0].y,
            "above the buttons"
        );
        assert!(
            page.contains(Point::new(content.x, content.y)),
            "inside the box"
        );
        let inside = Point::new(content.x + 4.0, content.y + 4.0);
        let press_at = |point| Input::PointerButton(ButtonInput::press(PointerButton::Left, point));
        assert_eq!(
            window.feed(&mut dialog, &press_at(inside)),
            DialogResponse::Ignored,
            "the content's widget takes it"
        );
        assert!(dialog.is_content_focused(), "and the focus with it");
        let add = Point::new(buttons[1].x + 2.0, buttons[1].y + 2.0);
        assert_eq!(
            window.feed(&mut dialog, &press_at(add)),
            DialogResponse::Activated("add")
        );
        assert!(!dialog.is_content_focused());
    }

    #[test]
    fn test_one_handle_takes_keys_and_clicks_and_asks_a_redraw_only_for_what_it_used() {
        let mut window = TestWindow::new(800, 600);
        let mut dialog = dialog();
        let key = |spec| Input::Key(parse_key(spec).expect("a valid key"));
        assert_eq!(window.feed(&mut dialog, &key("x")), DialogResponse::Ignored);
        assert!(!window.take_redraw(), "nothing changed");
        assert_eq!(
            window.feed(&mut dialog, &key("<tab>")),
            DialogResponse::Handled
        );
        assert!(window.take_redraw());
        let mut frame = window.frame();
        let rect = dialog.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 800.0, 600.0),
            &Theme::dark(),
        );
        let outside = ButtonInput::press(PointerButton::Left, Point::new(rect.x - 5.0, 5.0));
        let response = window.feed(&mut dialog, &Input::PointerButton(outside));
        assert_eq!(
            response,
            DialogResponse::Handled,
            "modal: the press is its own"
        );
        assert_eq!(
            window.feed(&mut dialog, &key("<cr>")),
            DialogResponse::Activated("discard")
        );
    }
}

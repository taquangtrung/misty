//! The elements that float over the window: [`dialog`], a modal question
//! with buttons; [`menu`], a button that opens a menu of commands;
//! [`context_menu`], a menu open where the user asked for it;
//! [`which_key`], the keys that may follow a prefix the app's keymap waits
//! on; and [`toasts`], the cards of the app's notifier in a corner. Each
//! wraps the widget of the same name, keeping its view state (the focused
//! button, the open menu and its highlight, the card under the pointer) in
//! misty's store, while what it shows comes from the app's state, and what
//! the user does reaches the app through the callbacks it was given.

use std::rc::Rc;
use std::time::{Duration, Instant};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::clock;
use super::super::base::geometry::Rect;
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::notice::{Notifier, ProgressId, ToastId};
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::super::render::paragraph::Paragraph;
use super::super::widgets::button::{self, ButtonProps, ButtonResponse};
use super::super::widgets::dialog::{self, DialogButton, DialogResponse};
use super::super::widgets::floating_panel::Corner;
use super::super::widgets::menu::{self, MenuEntry, MenuResponse};
use super::super::widgets::popup::PopupSide;
use super::super::widgets::text_field::{self, TextFieldProps, TextFieldResponse};
use super::super::widgets::toast::{self, ToastEffect, ToastsResponse};
use super::super::widgets::which_key::{self, WhichKeyEntry, WhichKeyPlacement, WhichKeyResponse};
use super::controls::{Callback, call};
use super::element::{Element, Kind, MenuRequest, Overlay, overlay};
use super::node::{Call, Drawn, Spec, layer_id};

// ========================================================================
// Constants
// ========================================================================

/// The child index, under a dialog's node, of its text field's node,
/// apart from the dialog's buttons (counted from 0) and paragraphs.
const FIELD_NODE: u64 = 1 << 33;

// ========================================================================
// Data Structures
// ========================================================================

/// A modal dialog: a title, a body of paragraphs, a text field if it
/// asks for one, and a row of buttons, centered over a dimmed backdrop.
/// Made by [`dialog`], it becomes a modal [`Overlay`] where
/// [`Element::overlay`] takes one.
pub struct Dialog<'a, A> {
    /// The body's paragraphs, top to bottom.
    body: Vec<Paragraph>,
    /// The buttons, left to right.
    buttons: Vec<Choice<'a, A>>,
    /// The text field under the body, if it has one.
    field: Option<DialogField<'a, A>>,
    /// What Esc runs.
    on_close: Option<Callback<A, ()>>,
    /// What a click on a link in the body runs, with its target.
    on_link: Option<Callback<A, String>>,
    /// The heading.
    title: &'a str,
}

/// A button that opens a menu of commands with ids of type `I` under it,
/// as an element. Made by [`menu`].
pub struct Menu<'a, A, I> {
    /// Whether it takes input.
    enabled: bool,
    /// The menu's items and separators, top to bottom.
    entries: Vec<MenuEntry<I>>,
    /// What the button says.
    label: &'a str,
    /// What choosing an item runs, with its id.
    on_choose: Option<Callback<A, I>>,
}

/// A menu of commands with ids of type `I`, open where the user asked for
/// it, as a layer. Made by [`context_menu`], it becomes an [`Overlay`]
/// where [`Element::overlay`] takes one.
pub struct ContextMenu<A, I> {
    /// The menu's items and separators, top to bottom.
    entries: Vec<MenuEntry<I>>,
    /// What choosing an item runs, with its id.
    on_choose: Option<Callback<A, I>>,
    /// What closing the menu without choosing runs.
    on_close: Option<Callback<A, ()>>,
    /// Where the menu opens.
    request: MenuRequest,
}

/// The keys that may follow a prefix the app's keymap waits on, and what
/// each does, as a layer along the window's bottom. Made by [`which_key`],
/// it becomes an [`Overlay`] where [`Element::overlay`] takes one.
pub struct WhichKey<'a, A> {
    /// How long the prefix waits before the hints show, or `None` for the
    /// popup's own delay.
    delay: Option<Duration>,
    /// What may follow the prefix.
    entries: Vec<WhichKeyEntry>,
    /// The header the app names its hints by, shown at once, or `None` to
    /// name the prefix after its delay.
    header: Option<&'a str>,
    /// What a click on an entry runs, with its index.
    on_activate: Option<Callback<A, usize>>,
    /// What a click outside the hints runs.
    on_close: Option<Callback<A, ()>>,
    /// Where the hints sit in the window.
    placement: WhichKeyPlacement,
    /// The keys typed so far, empty when nothing is pending.
    prefix: &'a [Key],
}

/// The cards of the app's notifier, stacked in a corner of the window.
/// Made by [`toasts`], it becomes an [`Overlay`] where
/// [`Element::overlay`] takes one.
pub struct Toasts<'a, A> {
    /// The corner the stack grows from.
    corner: Corner,
    /// The notifier whose toasts and tasks the cards show.
    notifier: &'a Notifier,
    /// The same notifier in the app's state, where misty holds, dismisses,
    /// and expires its toasts.
    notifier_mut: fn(&mut A) -> &mut Notifier,
    /// What a task's Cancel button runs, with the task.
    on_cancel: Option<Callback<A, ProgressId>>,
    /// What a click on a toast runs, with the toast.
    on_open: Option<Callback<A, ToastId>>,
}

/// The text field of a [`Dialog`].
struct DialogField<'a, A> {
    /// What typing runs.
    on_change: Callback<A, String>,
    /// What it shows while empty.
    placeholder: &'a str,
    /// Its text.
    text: &'a str,
}

/// One button of a [`Dialog`].
struct Choice<'a, A> {
    /// What it says.
    label: &'a str,
    /// What choosing it runs.
    on_click: Callback<A, ()>,
    /// Whether it is the answer the dialog proposes.
    primary: bool,
}

/// The widget kept for a [`Dialog`]: misty's dialog, and what it was made
/// from, to make it again when that changes.
pub(crate) struct DialogBox {
    /// The body it was made with.
    body: Vec<Paragraph>,
    /// Its buttons' labels and whether each is primary.
    buttons: Vec<(String, bool)>,
    /// The dialog, its buttons handing back their index.
    dialog: dialog::Dialog<usize>,
    /// The text field in the dialog's content room, and the text the app
    /// last gave it; `None` for a dialog without one.
    field: Option<(text_field::TextField, String)>,
    /// Whether the dialog has the keyboard.
    focused: bool,
    /// The title it was made with.
    title: String,
}

/// What the app's state says a [`DialogBox`] shows.
pub(crate) struct DialogProps<'a> {
    /// The body.
    body: &'a [Paragraph],
    /// Each button's label and whether it is primary.
    buttons: Vec<(&'a str, bool)>,
    /// The text field's text and placeholder, for a dialog with one.
    field: Option<(&'a str, &'a str)>,
    /// The title.
    title: &'a str,
}

/// What the user did with a dialog.
pub(crate) enum DialogAnswer {
    /// Chose the button at this index.
    Button(usize),
    /// Pressed Esc.
    Closed,
    /// Clicked the link to this target.
    Link(String),
    /// Changed the text field's text to this.
    Typed(String),
}

/// The widget kept for a [`Menu`]: the button and the menu it opens.
pub(crate) struct MenuButton<I> {
    /// Where the button was drawn last, which the menu opens under.
    anchor: Rect,
    /// The button.
    button: button::Button,
    /// The menu.
    menu: menu::Menu<I>,
}

/// What the app's state says a [`MenuButton`] shows.
pub(crate) struct MenuButtonProps<'a, I> {
    /// The button's props.
    button: ButtonProps<'a>,
    /// The menu's entries.
    entries: &'a [MenuEntry<I>],
}

/// The widget kept for a [`ContextMenu`]: the menu, and where it was
/// opened.
pub(crate) struct ContextBox<I> {
    /// Whether the menu has had the keyboard since it opened.
    focused: bool,
    /// The menu.
    menu: menu::Menu<I>,
    /// Where it was last opened, or `None` before it first opens.
    request: Option<MenuRequest>,
}

/// What the app's state says a [`ContextBox`] shows.
pub(crate) struct ContextProps<'a, I> {
    /// The menu's entries.
    entries: &'a [MenuEntry<I>],
    /// Where it opens.
    request: MenuRequest,
}

/// What the user did with a context menu.
pub(crate) enum ContextAnswer<I> {
    /// Chose the item with this id.
    Chosen(I),
    /// Closed it without choosing.
    Closed,
}

/// The widget kept for a [`WhichKey`]: the popup, and what it was last
/// told.
pub(crate) struct WhichBox {
    /// What the popup was made with and last told.
    told: WhichTold,
    /// The popup.
    popup: which_key::WhichKey,
}

/// What a [`WhichBox`]'s popup was made with and last told, to tell it
/// again only what changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WhichTold {
    /// Its delay, or `None` for its own.
    delay: Option<Duration>,
    /// The entries last shown at once with a header.
    entries: Vec<WhichKeyEntry>,
    /// The header last shown at once.
    header: Option<String>,
    /// Where it sits.
    placement: WhichKeyPlacement,
    /// The prefix last told.
    prefix: Vec<Key>,
}

/// What the app's state says a [`WhichBox`] shows.
pub(crate) struct WhichProps<'a> {
    /// The popup's delay.
    delay: Option<Duration>,
    /// What may follow the prefix.
    entries: &'a [WhichKeyEntry],
    /// The header shown at once, if any.
    header: Option<&'a str>,
    /// Where it sits.
    placement: WhichKeyPlacement,
    /// The keys typed so far.
    prefix: &'a [Key],
}

/// The widget kept for [`Toasts`]: misty's cards and the corner they were
/// made for.
pub(crate) struct ToastCards {
    /// The cards.
    pub(crate) cards: toast::Toasts,
    /// The corner they grow from.
    corner: Corner,
}

/// What an input over the cards answered: the response, and what it does
/// to the notifier.
pub(crate) struct CardsAnswer {
    /// What it does to the notifier's toasts.
    effect: ToastEffect,
    /// The cards' response.
    response: ToastsResponse,
}

// ========================================================================
// Dialog Implementations
// ========================================================================

impl<'a, A> Dialog<'a, A> {
    /// This dialog with a button saying `label` at the right of its
    /// buttons, running `apply` when chosen. The app closes the dialog in
    /// `apply` when the answer should close it.
    pub fn button(mut self, label: &'a str, apply: impl Fn(&mut A) + 'static) -> Self {
        self.buttons.push(choice(label, apply, false));
        self
    }

    /// This dialog with a button saying `label`, as [`Dialog::button`]
    /// adds, drawn in the accent as the answer it proposes; the first such
    /// button has the focus when the dialog opens.
    pub fn primary(mut self, label: &'a str, apply: impl Fn(&mut A) + 'static) -> Self {
        self.buttons.push(choice(label, apply, true));
        self
    }

    /// What runs when the user dismisses the dialog with Esc, usually
    /// closing it. Without one, Esc does nothing.
    pub fn on_close(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_close: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }

    /// What runs, with the link's target, when the user clicks a link in
    /// the body; the dialog stays open.
    pub fn on_link(self, apply: impl Fn(&mut A, String) + 'static) -> Self {
        Self {
            on_link: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This dialog asking for a line of text: a text field under the body
    /// showing `text`, the app's, running `apply` with the new text as
    /// the user edits it, as [`text_field`](crate::view::text_field) does.
    /// The field has the keyboard when the dialog opens; Tab and Shift+Tab
    /// move on to the buttons and back, Enter in the field chooses the
    /// first primary button, and Esc runs [`Dialog::on_close`].
    pub fn field(self, text: &'a str, apply: impl Fn(&mut A, String) + 'static) -> Self {
        let field = DialogField {
            on_change: Rc::new(apply),
            placeholder: "",
            text,
        };
        Self {
            field: Some(field),
            ..self
        }
    }

    /// This dialog's text field ([`Dialog::field`]) showing `placeholder`
    /// while it is empty; nothing without a field.
    pub fn field_placeholder(mut self, placeholder: &'a str) -> Self {
        if let Some(field) = &mut self.field {
            field.placeholder = placeholder;
        }
        self
    }
}

impl<A: 'static> Spec<A> for Dialog<'_, A> {
    type Widget = DialogBox;

    const KIND: &'static str = "dialog";

    fn create(&self) -> DialogBox {
        DialogBox {
            body: Vec::new(),
            buttons: Vec::new(),
            dialog: dialog::Dialog::new("", Vec::new()),
            field: None,
            focused: false,
            title: String::new(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> DialogProps<'b> {
        DialogProps {
            body: &self.body,
            buttons: self
                .buttons
                .iter()
                .map(|choice| (choice.label, choice.primary))
                .collect(),
            field: self
                .field
                .as_ref()
                .map(|field| (field.text, field.placeholder)),
            title: self.title,
        }
    }

    fn call(&self, answer: DialogAnswer) -> Option<Call<A>> {
        match answer {
            DialogAnswer::Button(index) => call(self.buttons.get(index).map(|b| &b.on_click), ()),
            DialogAnswer::Closed => call(self.on_close.as_ref(), ()),
            DialogAnswer::Link(target) => call(self.on_link.as_ref(), target),
            DialogAnswer::Typed(text) => {
                call(self.field.as_ref().map(|field| &field.on_change), text)
            }
        }
    }

    /// Draws the dialog centered in `rect`, the whole window, over its
    /// backdrop; it takes the pointer where its box is.
    fn draw(&self, widget: &mut DialogBox, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.draw(frame, rect, theme).into()
    }

    fn describe(&self, widget: &DialogBox, frame: &mut Frame, id: FocusId) {
        widget.dialog.access(frame, id);
        if let Some((field, _)) = &widget.field {
            let field_id = id.child(FIELD_NODE);
            field.access(frame, field_id);
            if widget.dialog.is_content_focused() {
                frame.access.set_inner_focus(id, field_id);
            }
        }
    }

    fn caret(&self, widget: &DialogBox) -> Option<Rect> {
        let (field, _) = widget.field.as_ref()?;
        widget
            .dialog
            .is_content_focused()
            .then(|| field.cursor_area())
            .flatten()
    }

    fn act(
        &self,
        widget: &mut DialogBox,
        id: FocusId,
        action: AccessAction,
    ) -> Option<DialogResponse<usize>> {
        Some(widget.dialog.access_action(id, action))
    }
}

impl<'a, A: 'static> From<Dialog<'a, A>> for Overlay<'a, A> {
    /// The dialog as a modal layer.
    fn from(dialog: Dialog<'a, A>) -> Self {
        overlay(Element::new(Kind::Control(Box::new(dialog)))).modal()
    }
}

// ========================================================================
// DialogBox Implementations
// ========================================================================

impl Widget for DialogBox {
    type Output = Rect;
    type Response = DialogResponse<usize>;

    /// Hands a key to the text field first while it has the keyboard,
    /// and the rest to the dialog: Enter the field leaves chooses the
    /// first primary button. Pointer input the dialog leaves (over the
    /// field) goes to the field.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> DialogResponse<usize> {
        let Some((field, _)) = &mut self.field else {
            return self.dialog.handle(input, cx);
        };
        let typing = matches!(input, Input::Key(_) | Input::Ime(_));
        if typing && self.dialog.is_content_focused() {
            match field.handle(input, cx) {
                TextFieldResponse::Changed | TextFieldResponse::Handled => {
                    return DialogResponse::Handled;
                }
                TextFieldResponse::Ignored => {}
            }
            let primary = self.buttons.iter().position(|(_, primary)| *primary);
            if let (Some(index), Some(key)) = (primary, input.key())
                && key == Key::new(KeyCode::Enter, Mods::NONE)
            {
                return DialogResponse::Activated(index);
            }
        }
        let response = self.dialog.handle(input, cx);
        if typing || response != DialogResponse::Ignored {
            return response;
        }
        match field.handle(input, cx) {
            TextFieldResponse::Changed | TextFieldResponse::Handled => DialogResponse::Handled,
            TextFieldResponse::Ignored => DialogResponse::Ignored,
        }
    }

    /// Draws the dialog, and its text field in the room it keeps, focused
    /// while the dialog has the keyboard and the field has the dialog's.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let drawn = self.dialog.draw(frame, area, theme);
        if let (Some((field, _)), Some(room)) = (&mut self.field, self.dialog.content_rect()) {
            field.set_focused(self.focused && self.dialog.is_content_focused());
            frame.overlay(|frame| field.draw(frame, room, theme));
        }
        drawn
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let (field, _) = self.field.as_mut()?;
        field.about_to_wait(cx)
    }
}

impl Control for DialogBox {
    type Props<'a> = DialogProps<'a>;
    type Value = DialogAnswer;

    /// Makes the dialog again when its title, body, or buttons changed,
    /// which moves its focus back to its first primary button.
    fn sync(&mut self, props: DialogProps<'_>) {
        let same_buttons = self.buttons.len() == props.buttons.len()
            && self.buttons.iter().zip(&props.buttons).all(
                |((label, primary), (new_label, new_primary))| {
                    label == new_label && primary == new_primary
                },
            );
        if let (Some((field, given)), Some((text, placeholder))) = (&mut self.field, props.field) {
            if given != text {
                text.clone_into(given);
            }
            field.sync(TextFieldProps {
                label: Some(props.title),
                placeholder,
                text,
            });
        }
        let same_field = self.field.is_some() == props.field.is_some();
        if same_buttons && same_field && self.title == props.title && self.body == props.body {
            return;
        }
        self.field = props.field.map(|(text, placeholder)| {
            let mut field = text_field::TextField::new();
            field.sync(TextFieldProps {
                label: Some(props.title),
                placeholder,
                text,
            });
            (field, text.to_owned())
        });
        self.title = props.title.to_owned();
        self.body = props.body.to_vec();
        self.buttons = props
            .buttons
            .iter()
            .map(|(label, primary)| ((*label).to_owned(), *primary))
            .collect();
        let made = dialog::Dialog::new(props.title, self.body.clone());
        let made = match self.field.is_some() {
            true => made.with_content(1),
            false => made,
        };
        self.dialog =
            self.buttons
                .iter()
                .enumerate()
                .fold(made, |dialog, (index, (label, primary))| {
                    let button = DialogButton::new(index, label.as_str());
                    dialog.with_button(match primary {
                        true => button.primary(),
                        false => button,
                    })
                });
    }

    /// Nothing: a dialog floats over the window rather than taking room in
    /// it.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, response: DialogResponse<usize>) -> Option<DialogAnswer> {
        match response {
            DialogResponse::Activated(index) => Some(DialogAnswer::Button(index)),
            DialogResponse::Closed => Some(DialogAnswer::Closed),
            DialogResponse::Link(target) => Some(DialogAnswer::Link(target)),
            DialogResponse::Handled => {
                let (field, given) = self.field.as_ref()?;
                (field.text() != given).then(|| DialogAnswer::Typed(field.text().to_owned()))
            }
            DialogResponse::Ignored => None,
        }
    }
}

// ========================================================================
// Menu Implementations
// ========================================================================

impl<'a, A, I> Menu<'a, A, I> {
    /// This menu button taking input (`true`, the default), or disabled.
    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// What runs on the app's state with the item's id when the user
    /// chooses an item of the menu.
    pub fn on_choose(self, apply: impl Fn(&mut A, I) + 'static) -> Self {
        Self {
            on_choose: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static, I: Copy + PartialEq + 'static> Spec<A> for Menu<'_, A, I> {
    type Widget = MenuButton<I>;

    const KIND: &'static str = "menu";

    fn create(&self) -> MenuButton<I> {
        MenuButton {
            anchor: Rect::default(),
            button: button::Button::new(self.label),
            menu: menu::Menu::new(Vec::new()),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> MenuButtonProps<'b, I> {
        MenuButtonProps {
            button: ButtonProps {
                active: None,
                enabled: self.enabled && self.on_choose.is_some(),
                label: self.label,
            },
            entries: &self.entries,
        }
    }

    fn call(&self, id: I) -> Option<Call<A>> {
        call(self.on_choose.as_ref(), id)
    }

    /// Draws the button in `rect` and, while open, the menu under it in the
    /// frame's overlay: the popup the store traps the focus in.
    fn draw(
        &self,
        widget: &mut MenuButton<I>,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        let layer = widget.draw(frame, rect, theme);
        Drawn {
            hit: widget.anchor,
            layer,
            over: None,
        }
    }

    /// Describes the button as `id` and, while open, the menu as the node
    /// its popup is registered under.
    fn describe(&self, widget: &MenuButton<I>, frame: &mut Frame, id: FocusId) {
        widget.button.access(frame, id);
        widget.menu.access(frame, layer_id(id));
    }

    fn act(
        &self,
        widget: &mut MenuButton<I>,
        id: FocusId,
        action: AccessAction,
    ) -> Option<MenuResponse<I>> {
        if widget.menu.is_open() {
            return Some(widget.menu.access_action(layer_id(id), action));
        }
        let response = widget.button.access_action(id, action);
        Some(widget.press(response, false))
    }

    fn has_layer(widget: &MenuButton<I>) -> bool {
        widget.menu.is_open()
    }
}

impl<'a, A: 'static, I: Copy + PartialEq + 'static> From<Menu<'a, A, I>> for Element<'a, A> {
    fn from(menu: Menu<'a, A, I>) -> Self {
        Element::new(Kind::Control(Box::new(menu)))
    }
}

// ========================================================================
// MenuButton Implementations
// ========================================================================

impl<I: Copy> MenuButton<I> {
    /// What the button's `response` does: a click opens the menu under it,
    /// its first item highlighted when the keyboard opened it.
    fn press(&mut self, response: ButtonResponse, by_key: bool) -> MenuResponse<I> {
        match response {
            ButtonResponse::Clicked | ButtonResponse::Toggled(_) => {
                self.menu.open(self.anchor, PopupSide::Below);
                if by_key {
                    self.menu.move_highlight(1);
                }
                MenuResponse::Handled
            }
            ButtonResponse::Handled => MenuResponse::Handled,
            ButtonResponse::Ignored => MenuResponse::Ignored,
        }
    }
}

impl<I: Copy> Widget for MenuButton<I> {
    type Output = Option<Rect>;
    type Response = MenuResponse<I>;

    /// Hands `input` to the open menu, which takes every input while it
    /// shows; otherwise to the button, whose click opens the menu.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> MenuResponse<I> {
        if self.menu.is_open() {
            return self.menu.handle(input, cx);
        }
        let response = self.button.handle(input, cx);
        let by_key = input
            .key()
            .is_some_and(|key| key.mods == Mods::NONE && key.code != KeyCode::Esc);
        let answer = self.press(response, by_key);
        if self.menu.is_open() {
            cx.request_redraw();
        }
        answer
    }

    /// Draws the button in `area` and the open menu under it, kept inside
    /// the window; returns where the menu was drawn, or `None` while closed.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Option<Rect> {
        self.anchor = self.button.draw(frame, area, theme);
        let window = frame.bounds();
        self.menu.draw(frame, window, theme)
    }

    /// Tells the button whether it has the keyboard; losing it closes the
    /// menu.
    fn set_focused(&mut self, focused: bool) {
        self.button.set_focused(focused);
        if !focused {
            self.menu.close();
        }
    }

    fn is_focused(&self) -> bool {
        self.button.is_focused()
    }

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.button.about_to_wait(cx)
    }
}

impl<I: Copy + PartialEq> Control for MenuButton<I> {
    type Props<'a>
        = MenuButtonProps<'a, I>
    where
        I: 'a;
    type Value = I;

    /// Takes the button's props, and the menu's entries when they changed,
    /// which closes it; a disabled button closes it too.
    fn sync(&mut self, props: MenuButtonProps<'_, I>) {
        self.button.sync(props.button);
        if self.menu.entries() != props.entries {
            self.menu = menu::Menu::new(props.entries.to_vec());
        }
        if !props.button.enabled {
            self.menu.close();
        }
    }

    fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        self.button.measure(width, metrics, scale_factor)
    }

    fn changed(&self, response: MenuResponse<I>) -> Option<I> {
        match response {
            MenuResponse::Activated(id) => Some(id),
            MenuResponse::Closed | MenuResponse::Handled | MenuResponse::Ignored => None,
        }
    }
}

// ========================================================================
// ContextMenu Implementations
// ========================================================================

impl<A, I> ContextMenu<A, I> {
    /// What runs on the app's state with the item's id when the user
    /// chooses an item; the menu closes. Forget the [`MenuRequest`] here
    /// too, so the layer goes.
    pub fn on_choose(self, apply: impl Fn(&mut A, I) + 'static) -> Self {
        Self {
            on_choose: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs when the user closes the menu without choosing (Esc, or a
    /// click outside it): forget the [`MenuRequest`], so the layer goes.
    pub fn on_close(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_close: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }
}

impl<A: 'static, I: Copy + PartialEq + 'static> Spec<A> for ContextMenu<A, I> {
    type Widget = ContextBox<I>;

    const KIND: &'static str = "context menu";

    fn create(&self) -> ContextBox<I> {
        ContextBox {
            focused: false,
            menu: menu::Menu::new(Vec::new()),
            request: None,
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> ContextProps<'b, I> {
        ContextProps {
            entries: &self.entries,
            request: self.request,
        }
    }

    fn call(&self, answer: ContextAnswer<I>) -> Option<Call<A>> {
        match answer {
            ContextAnswer::Chosen(id) => call(self.on_choose.as_ref(), id),
            ContextAnswer::Closed => call(self.on_close.as_ref(), ()),
        }
    }

    /// Draws the open menu beside its anchor, kept inside the window: the
    /// popup the store traps the focus in.
    fn draw(
        &self,
        widget: &mut ContextBox<I>,
        frame: &mut Frame,
        _rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        let window = frame.bounds();
        Drawn {
            hit: Rect::default(),
            layer: widget.menu.draw(frame, window, theme),
            over: None,
        }
    }

    /// Describes the open menu as the node its popup is registered under.
    fn describe(&self, widget: &ContextBox<I>, frame: &mut Frame, id: FocusId) {
        widget.menu.access(frame, layer_id(id));
    }

    fn act(
        &self,
        widget: &mut ContextBox<I>,
        id: FocusId,
        action: AccessAction,
    ) -> Option<MenuResponse<I>> {
        widget
            .menu
            .is_open()
            .then(|| widget.menu.access_action(layer_id(id), action))
    }

    /// Nothing but its open menu takes the keyboard.
    fn is_focusable(&self) -> bool {
        false
    }

    fn has_layer(widget: &ContextBox<I>) -> bool {
        widget.menu.is_open()
    }
}

impl<'a, A: 'static, I: Copy + PartialEq + 'static> From<ContextMenu<A, I>> for Overlay<'a, A> {
    /// The menu as a layer that is not modal: the open menu itself keeps
    /// the keyboard and the pointer until it closes.
    fn from(menu: ContextMenu<A, I>) -> Self {
        overlay(Element::new(Kind::Control(Box::new(menu))))
    }
}

// ========================================================================
// ContextBox Implementations
// ========================================================================

impl<I: Copy> Widget for ContextBox<I> {
    type Output = ();
    type Response = MenuResponse<I>;

    /// Hands `input` to the open menu, which takes every input while it
    /// shows.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> MenuResponse<I> {
        match self.menu.is_open() {
            true => self.menu.handle(input, cx),
            false => MenuResponse::Ignored,
        }
    }

    /// Nothing: the [`ContextMenu`] element draws the menu over the whole
    /// window.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    /// Tells the menu whether it has the keyboard; losing it once it had
    /// it closes the menu.
    fn set_focused(&mut self, focused: bool) {
        if self.focused && !focused {
            self.menu.close();
        }
        self.focused = focused;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl<I: Copy + PartialEq> Control for ContextBox<I> {
    type Props<'a>
        = ContextProps<'a, I>
    where
        I: 'a;
    type Value = ContextAnswer<I>;

    /// Opens the menu where the app's request says, the first time and
    /// whenever the request or the entries changed; its first item is
    /// highlighted when a key asked for it.
    fn sync(&mut self, props: ContextProps<'_, I>) {
        if self.request == Some(props.request) && self.menu.entries() == props.entries {
            return;
        }
        self.menu = menu::Menu::new(props.entries.to_vec());
        self.menu.open(props.request.anchor, PopupSide::Below);
        if props.request.by_key {
            self.menu.move_highlight(1);
        }
        self.request = Some(props.request);
        self.focused = false;
    }

    /// Nothing: the menu floats over the window.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, response: MenuResponse<I>) -> Option<ContextAnswer<I>> {
        match response {
            MenuResponse::Activated(id) => Some(ContextAnswer::Chosen(id)),
            MenuResponse::Closed => Some(ContextAnswer::Closed),
            MenuResponse::Handled | MenuResponse::Ignored => None,
        }
    }
}

// ========================================================================
// WhichKey Implementations
// ========================================================================

impl<'a, A> WhichKey<'a, A> {
    /// These hints shown at once under `header`, the app's own name for
    /// its prefix, rather than after the prefix has waited a moment: for
    /// an app that times its hints itself.
    pub fn header(self, header: &'a str) -> Self {
        Self {
            header: Some(header),
            ..self
        }
    }

    /// These hints shown once the prefix has waited `delay`, rather than
    /// half a second.
    pub fn delay(self, delay: Duration) -> Self {
        Self {
            delay: Some(delay),
            ..self
        }
    }

    /// These hints placed as `placement` says: along the bottom (the
    /// default), or centered as a box.
    pub fn placement(self, placement: WhichKeyPlacement) -> Self {
        Self { placement, ..self }
    }

    /// What runs, with the entry's index, when the user clicks an entry:
    /// act as if its key were typed.
    pub fn on_activate(self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        Self {
            on_activate: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs when the user clicks outside the hints while they show:
    /// drop the prefix.
    pub fn on_close(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_close: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for WhichKey<'_, A> {
    type Widget = WhichBox;

    const KIND: &'static str = "which key";

    fn create(&self) -> WhichBox {
        WhichBox {
            told: WhichTold::default(),
            popup: which_key::WhichKey::new(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> WhichProps<'b> {
        WhichProps {
            delay: self.delay,
            entries: &self.entries,
            header: self.header,
            placement: self.placement,
            prefix: self.prefix,
        }
    }

    fn call(&self, response: WhichKeyResponse) -> Option<Call<A>> {
        match response {
            WhichKeyResponse::Activated(index) => call(self.on_activate.as_ref(), index),
            WhichKeyResponse::Closed => call(self.on_close.as_ref(), ()),
            WhichKeyResponse::Handled | WhichKeyResponse::Ignored => None,
        }
    }

    /// Draws the hints in `rect`, the whole window, while they show; they
    /// take the pointer then.
    fn draw(&self, widget: &mut WhichBox, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.popup.draw(frame, rect, theme);
        match widget.popup.is_shown() {
            true => rect.into(),
            false => Rect::default().into(),
        }
    }

    /// Nothing: the hints describe nothing to screen readers yet.
    fn describe(&self, _widget: &WhichBox, _frame: &mut Frame, _id: FocusId) {}

    fn act(
        &self,
        _widget: &mut WhichBox,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<WhichKeyResponse> {
        None
    }

    fn is_focusable(&self) -> bool {
        false
    }

    fn takes_pointer(&self) -> bool {
        true
    }
}

impl<'a, A: 'static> From<WhichKey<'a, A>> for Overlay<'a, A> {
    /// The hints as a layer that is not modal: the keys go on to the app,
    /// whose keymap resolves the prefix.
    fn from(hints: WhichKey<'a, A>) -> Self {
        overlay(Element::new(Kind::Control(Box::new(hints))))
    }
}

// ========================================================================
// WhichBox Implementations
// ========================================================================

impl Widget for WhichBox {
    type Output = ();
    type Response = WhichKeyResponse;

    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> WhichKeyResponse {
        self.popup.handle(input, cx)
    }

    /// Nothing: the [`WhichKey`] element draws the popup.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    /// Shows the hints once the prefix has waited its delay, returning when
    /// that is.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.popup.about_to_wait(cx)
    }
}

impl Control for WhichBox {
    type Props<'a> = WhichProps<'a>;
    type Value = WhichKeyResponse;

    /// Makes the popup again when its delay or placement changed, then
    /// tells it what changed: the hints to show at once under a header, or
    /// the prefix waiting (an empty one hides it).
    fn sync(&mut self, props: WhichProps<'_>) {
        if props.delay != self.told.delay || props.placement != self.told.placement {
            let popup = which_key::WhichKey::new().with_placement(props.placement);
            self.popup = match props.delay {
                Some(delay) => popup.with_delay(delay),
                None => popup,
            };
            self.told = WhichTold {
                delay: props.delay,
                placement: props.placement,
                ..WhichTold::default()
            };
        }
        match props.header {
            Some(header) => {
                let same = self.told.header.as_deref() == Some(header)
                    && self.told.entries == props.entries;
                if !same {
                    self.popup.show(header, props.entries.to_vec());
                    self.told.header = Some(header.to_owned());
                    self.told.entries = props.entries.to_vec();
                    self.told.prefix.clear();
                }
            }
            None => {
                if self.told.header.is_some() {
                    self.popup.reset();
                    self.told.header = None;
                    self.told.entries.clear();
                }
                if self.told.prefix != props.prefix {
                    self.popup.set_pending(props.prefix, props.entries.to_vec());
                    self.told.prefix = props.prefix.to_vec();
                }
            }
        }
    }

    /// Nothing: the hints float over the window.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, response: WhichKeyResponse) -> Option<WhichKeyResponse> {
        matches!(
            response,
            WhichKeyResponse::Activated(_) | WhichKeyResponse::Closed
        )
        .then_some(response)
    }
}

// ========================================================================
// Toasts Implementations
// ========================================================================

impl<'a, A> Toasts<'a, A> {
    /// These cards stacked from `corner` (the bottom right unless this
    /// says another): down from a top corner, up from a bottom one.
    pub fn corner(self, corner: Corner) -> Self {
        Self { corner, ..self }
    }

    /// What runs, with the toast, when the user clicks a toast outside its
    /// close button: show more of it.
    pub fn on_open(self, apply: impl Fn(&mut A, ToastId) + 'static) -> Self {
        Self {
            on_open: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs, with the task, when the user clicks a task's Cancel
    /// button: stop its work, then finish it on the notifier.
    pub fn on_cancel(self, apply: impl Fn(&mut A, ProgressId) + 'static) -> Self {
        Self {
            on_cancel: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for Toasts<'_, A> {
    type Widget = ToastCards;

    const KIND: &'static str = "toasts";

    fn create(&self) -> ToastCards {
        ToastCards {
            cards: toast::Toasts::new().with_corner(self.corner),
            corner: self.corner,
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> Corner {
        self.corner
    }

    /// The call holding or dismissing toasts on the app's notifier, then
    /// running the callback of a toast opened or a task cancelled.
    fn call(&self, answer: CardsAnswer) -> Option<Call<A>> {
        let notifier_mut = self.notifier_mut;
        let then = match answer.response {
            ToastsResponse::Cancelled(task) => call(self.on_cancel.as_ref(), task),
            ToastsResponse::Opened(toast) => call(self.on_open.as_ref(), toast),
            ToastsResponse::Closed(_) | ToastsResponse::Handled | ToastsResponse::Ignored => None,
        };
        let effect = answer.effect;
        Some(Box::new(move |app: &mut A| {
            effect.apply(notifier_mut(app), clock::now());
            if let Some(then) = then {
                then(app);
            }
        }))
    }

    /// Draws the notifier's cards in the corner of `rect`, the whole
    /// window; they take the pointer where they are.
    fn draw(&self, widget: &mut ToastCards, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.cards.draw_cards(self.notifier, frame, rect, theme);
        widget.cards.cards_rect().unwrap_or_default().into()
    }

    /// Nothing: the cards describe nothing to screen readers yet.
    fn describe(&self, _widget: &ToastCards, _frame: &mut Frame, _id: FocusId) {}

    fn act(
        &self,
        _widget: &mut ToastCards,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<CardsAnswer> {
        None
    }

    fn is_focusable(&self) -> bool {
        false
    }

    fn takes_pointer(&self) -> bool {
        true
    }

    fn is_ticking(&self) -> bool {
        true
    }

    /// The call expiring the toasts whose time is up, or when the next one
    /// is.
    fn tick(&self, now: Instant) -> (Option<Call<A>>, Option<Instant>) {
        match self.notifier.toasts().next_expiry() {
            Some(due) if due <= now => {
                let notifier_mut = self.notifier_mut;
                let expire: Call<A> = Box::new(move |app: &mut A| {
                    notifier_mut(app).toasts_mut().expire(now);
                });
                (Some(expire), None)
            }
            due => (None, due),
        }
    }
}

impl<'a, A: 'static> From<Toasts<'a, A>> for Overlay<'a, A> {
    /// The cards as a layer that is not modal: the pointer reaches what is
    /// under them wherever there is no card.
    fn from(toasts: Toasts<'a, A>) -> Self {
        overlay(Element::new(Kind::Control(Box::new(toasts))))
    }
}

// ========================================================================
// ToastCards Implementations
// ========================================================================

impl Widget for ToastCards {
    type Output = ();
    type Response = CardsAnswer;

    /// Follows the pointer over the cards as last drawn.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> CardsAnswer {
        let (response, effect) = self.cards.follow(input, cx);
        CardsAnswer { effect, response }
    }

    /// Nothing: the [`Toasts`] element draws the cards, lending them the
    /// app's notifier.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.cards.step_bars(cx)
    }
}

impl Control for ToastCards {
    type Props<'a> = Corner;
    type Value = CardsAnswer;

    /// Stacks the cards from another corner when it changed.
    fn sync(&mut self, corner: Corner) {
        if corner != self.corner {
            *self = Self {
                cards: toast::Toasts::new().with_corner(corner),
                corner,
            };
        }
    }

    /// Nothing: the cards float over the window.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    /// Every answer that changes the notifier or the app.
    fn changed(&self, answer: CardsAnswer) -> Option<CardsAnswer> {
        let acts = answer.effect != ToastEffect::default()
            || matches!(
                answer.response,
                ToastsResponse::Cancelled(_) | ToastsResponse::Opened(_)
            );
        acts.then_some(answer)
    }
}

impl Outcome for CardsAnswer {
    fn is_ignored(&self) -> bool {
        self.response.is_ignored()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A modal dialog headed `title` showing the paragraphs of `body`, with no
/// buttons yet: [`Dialog::button`] and [`Dialog::primary`] add them, and
/// [`Dialog::on_close`] says what Esc does. Show it with
/// [`Element::overlay_if`] while the app's state says it is open:
///
/// ```
/// use misty::view::{column, dialog, text};
/// use misty::{App, Element, Paragraph, Theme, ThemeToken, WindowId};
///
/// struct Editor {
///     confirming: bool,
///     theme: Theme,
/// }
///
/// impl App for Editor {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         let body = vec![Paragraph::plain("Save before closing?", ThemeToken::Foreground.into())];
///         column([text("editing").into()]).overlay_if(
///             self.confirming,
///             dialog("Unsaved changes", body)
///                 .button("Discard", |app: &mut Self| app.confirming = false)
///                 .primary("Save", |app: &mut Self| app.confirming = false)
///                 .on_close(|app: &mut Self| app.confirming = false),
///         )
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
/// ```
///
/// Tab and Shift+Tab (or Left and Right) move between the buttons, Enter or
/// Space chooses one, a click on one chooses it, and a click elsewhere does
/// nothing. While it shows, no key or click reaches the window under it;
/// when it goes, the focus returns to the element that had it.
pub fn dialog<'a, A>(title: &'a str, body: Vec<Paragraph>) -> Dialog<'a, A> {
    Dialog {
        body,
        buttons: Vec::new(),
        field: None,
        on_close: None,
        on_link: None,
        title,
    }
}

/// A button saying `label` that opens a menu of `entries` under it (Enter,
/// Space, or a click), its first item highlighted when opened by a key. The
/// open menu keeps the keyboard: Up and Down move the highlight, Enter or
/// a click chooses an item, running [`Menu::on_choose`] with its id, and
/// Esc or a click outside closes it; the focus then goes back to the
/// button.
pub fn menu<'a, A, I: Copy>(label: &'a str, entries: Vec<MenuEntry<I>>) -> Menu<'a, A, I> {
    Menu {
        enabled: true,
        entries,
        label,
        on_choose: None,
    }
}

/// A menu of `entries` open where `request` says, as a layer to show with
/// [`Element::overlay_if`] while the app keeps the request: the
/// [`MenuRequest`] a right click on an element reported through
/// [`Element::on_context_menu`], or a title bar's menu button through its
/// `on_menu`. The open menu keeps the keyboard and the pointer: Up and
/// Down move the highlight (its first item is highlighted when a key asked
/// for the menu), Enter or a click chooses an item, running
/// [`ContextMenu::on_choose`] with its id, and Esc or a click outside closes
/// it, running [`ContextMenu::on_close`]; the focus then goes back to the
/// element that had it.
///
/// ```
/// use misty::view::{MenuRequest, column, context_menu, text};
/// use misty::{App, Element, MenuEntry, MenuItem, Theme, WindowId};
///
/// struct Notes {
///     menu: Option<MenuRequest>,
///     theme: Theme,
/// }
///
/// impl App for Notes {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         let entries = vec![MenuEntry::from(MenuItem::new(1, "Copy"))];
///         let menu = self.menu.map(|at| {
///             context_menu(entries, at)
///                 .on_choose(|app: &mut Self, _id| app.menu = None)
///                 .on_close(|app: &mut Self| app.menu = None)
///         });
///         let root = column([Element::from(text("Right-click me"))
///             .on_context_menu(|app: &mut Self, at| app.menu = Some(at))]);
///         match menu {
///             Some(menu) => root.overlay(menu),
///             None => root,
///         }
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
/// ```
pub fn context_menu<A, I: Copy>(
    entries: Vec<MenuEntry<I>>,
    request: MenuRequest,
) -> ContextMenu<A, I> {
    ContextMenu {
        entries,
        on_choose: None,
        on_close: None,
        request,
    }
}

/// Hints listing `entries`, the keys that may follow `prefix`, the keys
/// the app's keymap has typed so far and waits on, as a layer along the
/// window's bottom: misty resolves no keys, so the app tells it the prefix
/// and the entries from its own keymap on every rebuild. The hints show
/// once the same prefix has waited half a second (so a user typing a
/// sequence they know never sees them) and hide when the prefix is empty
/// again. Keys go on to the app as before; a click on an entry runs
/// [`WhichKey::on_activate`], one outside the hints [`WhichKey::on_close`].
pub fn which_key<'a, A>(prefix: &'a [Key], entries: Vec<WhichKeyEntry>) -> WhichKey<'a, A> {
    WhichKey {
        delay: None,
        entries,
        header: None,
        on_activate: None,
        on_close: None,
        placement: WhichKeyPlacement::default(),
        prefix,
    }
}

/// The cards of `notifier`, the app's notifier, stacked in the bottom right
/// corner of the window: its toasts, held while the pointer is over them
/// and dismissed by their close button, and its tasks under way. Misty
/// holds, dismisses, and expires toasts on the notifier `notifier_mut`
/// reaches in the app's state (the same one), once the tree is dropped:
///
/// ```
/// use misty::view::{column, text, toasts};
/// use misty::{App, Element, Notifier, Theme, WindowId};
///
/// struct Demo {
///     notifier: Notifier,
///     theme: Theme,
/// }
///
/// impl App for Demo {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         column([text("working").into()])
///             .overlay(toasts(&self.notifier, |app: &mut Self| &mut app.notifier))
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
/// ```
pub fn toasts<'a, A>(
    notifier: &'a Notifier,
    notifier_mut: fn(&mut A) -> &mut Notifier,
) -> Toasts<'a, A> {
    Toasts {
        corner: Corner::BottomRight,
        notifier,
        notifier_mut,
        on_cancel: None,
        on_open: None,
    }
}

/// A dialog button saying `label`, running `apply`, primary when `primary`.
fn choice<'a, A>(label: &'a str, apply: impl Fn(&mut A) + 'static, primary: bool) -> Choice<'a, A> {
    Choice {
        label,
        on_click: Rc::new(move |app: &mut A, ()| apply(app)),
        primary,
    }
}

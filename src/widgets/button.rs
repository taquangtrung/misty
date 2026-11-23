//! [`Button`]: a push button showing a label, an icon, or both, in one of
//! four looks ([`ButtonVariant`]): the default filled one, the accent of a
//! primary action, the danger color of a destructive one, or a ghost
//! (flat) button that is filled only under the pointer, as in a toolbar.
//! Made with [`Button::toggle`], it stays pressed in or out, as a
//! view-mode button or an "Aa" match-case switch does.
//!
//! Keys, while the app says the button is focused: Space or Enter
//! activates it. A left press on it shows it pressed in, and releasing the
//! button over it activates it; released elsewhere, nothing happens, so a
//! press can still be taken back by moving away. The pointer over it lights
//! it. A disabled button is drawn muted and leaves every input alone.
//!
//! A button keeps a tooltip text for the app to show with a
//! [`Tooltip`](crate::Tooltip): [`Button::tooltip_hover`] says where and
//! what while the pointer is over it.

use std::time::Instant;

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::clock;
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::motion::{DEFAULT_HOVER_FADE, HoverFade};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::super::render::image::Svg;
use super::FOCUS_STROKE;
use super::label::{draw_label, label_width};

// ========================================================================
// Constants
// ========================================================================

/// The share of a row added above and below a button's label.
const PADDING_ROWS: f32 = 0.25;

/// The blank cells on each side of a labelled button's content.
const PADDING_CELLS: f32 = 1.0;

/// The blank cells between a button's icon and its label.
const ICON_GAP_CELLS: f32 = 0.5;

/// How thick the line under a shortcut letter is, in logical pixels.
const SHORTCUT_STROKE: f32 = 1.0;

/// How opaque the foreground laid over a button under the pointer is, out
/// of 255.
const HOVER_TINT: u8 = 28;

/// How opaque the foreground laid over a pressed button is, out of 255.
const PRESS_TINT: u8 = 56;

// ========================================================================
// Data Structures
// ========================================================================

/// A push button. The app owns it, hands it the window's pointer events
/// (and its keys while it has the focus), and draws it each frame.
#[derive(Clone, Debug)]
pub struct Button {
    /// Whether a toggle button is pressed in, or `None` for a push button.
    active: Option<bool>,
    /// Whether the app schedules its hover fade (see
    /// [`Button::about_to_wait`]); until it does, the hover shows at once.
    animated: bool,
    /// Whether it takes input; a disabled button is drawn muted.
    enabled: bool,
    /// The light of the pointer over it, fading in and out.
    fade: HoverFade<()>,
    /// Whether the app gave it the keyboard.
    focused: bool,
    /// Whether the pointer is over it.
    hovered: bool,
    /// The icon shown left of the label, if any.
    icon: Option<Svg>,
    /// What it says; empty for an icon-only button.
    label: String,
    /// Where it was drawn last, for the pointer.
    layout: Option<Rect>,
    /// Whether its icon is drawn in the theme's text color whatever its
    /// state, as a tree's button draws it, rather than in its label's.
    neutral_icon: bool,
    /// Whether a left press on it is held.
    pressed: bool,
    /// The help shown while the pointer rests on it, if any.
    tooltip: Option<String>,
    /// How it looks.
    variant: ButtonVariant,
}

/// How a [`Button`] (or a toolbar's button) looks.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ButtonVariant {
    /// Filled with the danger color, for an action that destroys
    /// something.
    Danger,
    /// Filled with the hover color: an ordinary button.
    #[default]
    Default,
    /// Flat: filled only under the pointer or while pressed in, as in a
    /// toolbar.
    Ghost,
    /// Filled with the accent, for the action a form or dialog proposes.
    Primary,
}

/// What became of an input offered to a [`Button`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ButtonResponse {
    /// The user activated the push button: act on it.
    Clicked,
    /// The button used the input; nothing happened yet.
    Handled,
    /// The input is not the button's.
    Ignored,
    /// The user pressed the toggle button in (`true`) or out.
    Toggled(bool),
}

/// What the app's state says a [`Button`] shows, for [`Control::sync`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonProps<'a> {
    /// Whether a toggle button is pressed in, or `None` for a push button.
    pub active: Option<bool>,
    /// Whether it takes input.
    pub enabled: bool,
    /// What it says; empty for an icon-only button.
    pub label: &'a str,
}

/// How one button is painted: what it shows and the state it is in. The
/// widgets that draw buttons of their own (a dialog, a toolbar) paint them
/// through it, so every button looks alike.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ButtonFace<'a> {
    /// Whether it is a toggle pressed in.
    pub(crate) active: bool,
    /// Whether it takes input; drawn muted when not.
    pub(crate) enabled: bool,
    /// Whether it is lined with the accent, as its owner's focused button.
    pub(crate) focus_ring: bool,
    /// How lit it is by the pointer over it, from 0 (not at all) to 1,
    /// part way while a hover fades in or out.
    pub(crate) hover: f32,
    /// The icon left of the label, if any.
    pub(crate) icon: Option<&'a Svg>,
    /// What it says; may be empty.
    pub(crate) label: &'a str,
    /// Whether the icon is drawn in the theme's text color (muted while
    /// disabled) rather than in the label's, which a toggle pressed in
    /// draws in the accent.
    pub(crate) neutral_icon: bool,
    /// Whether it is held down under the pointer.
    pub(crate) pressed: bool,
    /// The letter that chooses it, underlined in the label where the label
    /// has it, as desktop dialogs mark their keys.
    pub(crate) shortcut: Option<char>,
    /// How it looks.
    pub(crate) variant: ButtonVariant,
}

// ========================================================================
// Button Implementations
// ========================================================================

impl Button {
    /// An enabled push button saying `label`, in the default look.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            active: None,
            animated: false,
            enabled: true,
            fade: HoverFade::new(DEFAULT_HOVER_FADE),
            focused: false,
            hovered: false,
            icon: None,
            label: label.into(),
            layout: None,
            neutral_icon: false,
            pressed: false,
            tooltip: None,
            variant: ButtonVariant::Default,
        }
    }

    /// An enabled push button showing only `icon`, a one-color SVG drawn
    /// in the label's color, as square as it is tall. Give it a tooltip
    /// with [`Button::with_tooltip`]: screen readers name it by that.
    pub fn icon(icon: Svg) -> Self {
        Self {
            icon: Some(icon),
            ..Self::new(String::new())
        }
    }

    /// This button showing `icon`, a one-color SVG, left of its label.
    pub fn with_icon(self, icon: Svg) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// This button in the look `variant`.
    pub fn with_variant(self, variant: ButtonVariant) -> Self {
        Self { variant, ..self }
    }

    /// This button taking input (`true`), or disabled: drawn muted and
    /// leaving every input alone.
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// This button as a toggle that stays pressed in (`active`) or out,
    /// flipping each time it is activated.
    pub fn toggle(self, active: bool) -> Self {
        Self {
            active: Some(active),
            ..self
        }
    }

    /// This button with `tooltip` as the help the app shows while the
    /// pointer rests on it (see [`Button::tooltip_hover`]). Screen readers
    /// read it as the button's description, or as its name when it shows
    /// only an icon.
    pub fn with_tooltip(self, tooltip: impl Into<String>) -> Self {
        Self {
            tooltip: Some(tooltip.into()),
            ..self
        }
    }

    /// Whether a toggle button is pressed in; `false` for a push button.
    pub fn is_active(&self) -> bool {
        self.active.unwrap_or(false)
    }

    /// Presses a toggle button in (`true`) or out, as the app's state says,
    /// without a response. Does nothing to a push button.
    pub fn set_active(&mut self, active: bool) {
        if let Some(current) = &mut self.active {
            *current = active;
        }
    }

    /// Whether it takes input.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Lets it take input (`true`), or disables it.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.hovered &= enabled;
        self.pressed &= enabled;
        if !enabled {
            self.fade.set_hovered(None, clock::now());
        }
    }

    /// How wide it is drawn in `frame` with `theme`'s chrome font when it
    /// has the room: its icon and label with padding on both sides, or a
    /// square for an icon-only button. Lay buttons out with it before
    /// drawing them.
    pub fn width(&self, frame: &Frame, theme: &Theme) -> f32 {
        self.face().width(frame, theme)
    }

    /// Where the button was drawn and its tooltip text, while the pointer
    /// is over it and it has a tooltip; hand them to
    /// [`Tooltip::hover`](crate::Tooltip::hover), and call
    /// [`Tooltip::leave`](crate::Tooltip::leave) when this is `None`.
    pub fn tooltip_hover(&self) -> Option<(Rect, &str)> {
        let rect = self.layout.filter(|_| self.hovered)?;
        Some((rect, self.tooltip.as_deref()?))
    }

    /// Shows `icon` left of its label (none for `None`), drawn in the
    /// theme's text color whatever its state, with `name` as its tooltip,
    /// which names a button without a label to screen readers: as a tree's
    /// button shows them, copied in at each frame.
    pub(crate) fn show_icon(&mut self, icon: Option<&Svg>, name: Option<&str>) {
        if self.icon.as_ref() != icon {
            self.icon = icon.cloned();
        }
        if self.tooltip.as_deref() != name {
            self.tooltip = name.map(str::to_owned);
        }
        self.neutral_icon = true;
    }

    /// Puts it in the look `variant`, as a tree's button is told at each
    /// frame.
    pub(crate) fn show_variant(&mut self, variant: ButtonVariant) {
        self.variant = variant;
    }

    /// Where the button was drawn last, or `None` before the first frame.
    pub(crate) fn drawn(&self) -> Option<Rect> {
        self.layout
    }

    /// Describes the button as last drawn to screen readers as `id`: a
    /// button named by its label (or its tooltip when it shows only an
    /// icon), described by its tooltip, saying whether a toggle is pressed
    /// in, clickable while enabled. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(rect) = self.layout else {
            return;
        };
        let mut node = Node::new(Role::Button);
        match (self.label.is_empty(), &self.tooltip) {
            (true, Some(tooltip)) => node.set_label(tooltip.as_str()),
            (false, Some(tooltip)) => {
                node.set_label(self.label.as_str());
                node.set_description(tooltip.as_str());
            }
            (_, None) => node.set_label(self.label.as_str()),
        }
        if let Some(active) = self.active {
            node.set_toggled(active.into());
        }
        match self.enabled {
            true => node.add_action(Action::Click),
            false => node.set_disabled(),
        }
        frame.access(id, rect, node);
    }

    /// Does what a screen reader's `action` on the button described as
    /// `id` asks: a click activates it. Ignored for other actions and
    /// nodes, and while disabled.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> ButtonResponse {
        match action {
            AccessAction::Click(target) if target == id => self.activate(),
            AccessAction::Click(_)
            | AccessAction::Decrement(_)
            | AccessAction::Focus(_)
            | AccessAction::Increment(_) => ButtonResponse::Ignored,
        }
    }

    /// Activates it, when enabled: flips a toggle, clicks a push button.
    fn activate(&mut self) -> ButtonResponse {
        if !self.enabled {
            return ButtonResponse::Ignored;
        }
        match &mut self.active {
            Some(active) => {
                *active = !*active;
                ButtonResponse::Toggled(*active)
            }
            None => ButtonResponse::Clicked,
        }
    }

    /// Reacts to `key`, as the module documentation lists, while focused.
    fn handle_key(&mut self, key: Key) -> ButtonResponse {
        if !self.focused {
            return ButtonResponse::Ignored;
        }
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter if key.mods == Mods::NONE => self.activate(),
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
            | KeyCode::Up => ButtonResponse::Ignored,
        }
    }

    /// Reacts to a left press or release at `point`: a press over it holds
    /// it down, and the release activates it when over it.
    fn press(&mut self, pressed: bool, point: Point) -> ButtonResponse {
        let over = self.is_over(point);
        match (pressed, self.pressed) {
            (true, _) if over => {
                self.pressed = true;
                ButtonResponse::Handled
            }
            (false, true) => {
                self.pressed = false;
                match over {
                    true => self.activate(),
                    false => ButtonResponse::Handled,
                }
            }
            (true, _) | (false, false) => ButtonResponse::Ignored,
        }
    }

    /// Whether `point` is over the button as last drawn.
    fn is_over(&self, point: Point) -> bool {
        self.layout.is_some_and(|rect| rect.contains(point))
    }

    /// Follows the pointer to `point` (`None` outside the window), asking a
    /// redraw when it entered or left; says whether it is over.
    fn follow_pointer(&mut self, point: Option<Point>, cx: &mut EventCx<'_>) -> bool {
        let over = self.enabled && point.is_some_and(|point| self.is_over(point));
        if over != self.hovered {
            self.hovered = over;
            self.fade.set_reduced_motion(cx.reduced_motion());
            self.fade.set_hovered(over.then_some(()), clock::now());
            cx.request_redraw();
        }
        over
    }

    /// How the button is painted now.
    fn face(&self) -> ButtonFace<'_> {
        ButtonFace {
            active: self.is_active(),
            enabled: self.enabled,
            focus_ring: false,
            hover: match self.animated {
                true => self.fade.level(&(), clock::now()),
                false => hover_light(self.hovered),
            },
            icon: self.icon.as_ref(),
            label: &self.label,
            neutral_icon: self.neutral_icon,
            pressed: self.pressed && self.hovered,
            shortcut: None,
            variant: self.variant,
        }
    }
}

impl Widget for Button {
    type Output = Rect;
    type Response = ButtonResponse;

    /// Reacts to `input`: keys as the module documentation lists while
    /// focused, a left press and release over it activating it, and the
    /// pointer lighting it. Asks for a redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ButtonResponse {
        let response = match input {
            Input::Key(key) if self.enabled => self.handle_key(*key),
            Input::PointerButton(button)
                if button.button == PointerButton::Left && self.enabled =>
            {
                self.follow_pointer(Some(button.position), cx);
                self.press(button.pressed, button.position)
            }
            Input::PointerMoved(position) => match self.follow_pointer(Some(*position), cx) {
                true => ButtonResponse::Handled,
                false => ButtonResponse::Ignored,
            },
            Input::PointerLeft => {
                self.follow_pointer(None, cx);
                ButtonResponse::Ignored
            }
            Input::FocusLost if self.pressed => {
                self.pressed = false;
                ButtonResponse::Handled
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
            | Input::Wheel(_) => ButtonResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the button at the left of `area`, centered down it, as wide as
    /// [`Button::width`] says (cut to `area`), and returns where it was
    /// drawn.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let metrics = frame.cell_metrics();
        let width = self.width(frame, theme).min(area.width);
        let height = face_height(metrics).min(area.height);
        let y = (area.y + (area.height - height) / 2.0).round();
        let rect = Rect::new(area.x, y, width, height);
        self.fade.set_duration(theme.motion.hover_fade);
        self.fade.set_reduced_motion(frame.reduced_motion());
        self.face().draw(frame, rect, theme);
        self.layout = Some(rect);
        rect
    }

    /// Tells it whether it has the keyboard: it takes keys only while it
    /// does (a [`FocusTree`](crate::FocusTree) draws the focus ring around
    /// it).
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }

    /// Asks for the next frame of a hover fade under way, returning when
    /// it is due; `None` at rest. A button whose app never calls it shows
    /// its hover at once, as it does while motion is reduced.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.animated = true;
        self.fade.set_reduced_motion(cx.reduced_motion());
        let next = self.fade.next_frame(clock::now())?;
        cx.request_redraw_at(next);
        Some(next)
    }
}

impl Control for Button {
    type Props<'a> = ButtonProps<'a>;
    type Value = ();

    /// Makes it a push button or a toggle pressed in or out, enables or
    /// disables it, and relabels it as `props` say.
    fn sync(&mut self, props: ButtonProps<'_>) {
        self.active = props.active;
        self.set_enabled(props.enabled);
        if self.label != props.label {
            props.label.clone_into(&mut self.label);
        }
    }

    /// A row and a little padding, whatever the width.
    fn measure(&self, _width: f32, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        face_height(metrics)
    }

    /// An activation: a push button clicked or a toggle flipped. A toggle's
    /// new state is the opposite of the `active` its props gave it.
    fn changed(&self, response: ButtonResponse) -> Option<()> {
        match response {
            ButtonResponse::Clicked | ButtonResponse::Toggled(_) => Some(()),
            ButtonResponse::Handled | ButtonResponse::Ignored => None,
        }
    }
}

impl Outcome for ButtonResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// ButtonFace Implementations
// ========================================================================

impl ButtonFace<'_> {
    /// How wide the button is drawn: its content with padding on both
    /// sides, or a square as tall as [`face_height`] without a label.
    pub(crate) fn width(&self, frame: &Frame, theme: &Theme) -> f32 {
        let metrics = frame.cell_metrics();
        if self.label.is_empty() {
            return face_height(metrics);
        }
        let padding = PADDING_CELLS * metrics.width;
        (self.content_width(frame, theme) + 2.0 * padding).ceil()
    }

    /// Paints the button over `rect`: its fill for its look and state, the
    /// tint of the pointer over it or of a press, the focus ring, and the
    /// icon and label centered in it.
    pub(crate) fn draw(&self, frame: &mut Frame, rect: Rect, theme: &Theme) {
        let scale_factor = frame.scale_factor();
        let (fill, ink) = self.colors(theme);
        if let Some(fill) = fill {
            let style = theme.shape.highlight(fill, scale_factor);
            // A ghost button's hover is its fill, so the fill fades with it.
            let opacity = match self.is_ghost_hover() {
                true => self.hover,
                false => 1.0,
            };
            frame.with_opacity(opacity, |frame| frame.draw_box(rect, &style));
        }
        if let Some(alpha) = self.tint() {
            let tint = theme.foreground;
            let style = BoxStyle {
                fill: tint.with_alpha(alpha),
                ..theme.shape.highlight(tint, scale_factor)
            };
            frame.draw_box(rect, &style);
        }
        if self.focus_ring {
            let ring = theme.accent;
            let style = BoxStyle {
                border: Some(Border {
                    color: Rgba::from(ring),
                    width: logical(FOCUS_STROKE, scale_factor),
                }),
                fill: ring.with_alpha(0),
                ..theme.shape.highlight(ring, scale_factor)
            };
            frame.draw_box(rect, &style);
        }
        let metrics = frame.cell_metrics();
        let content = self.content_width(frame, theme);
        let mut x = (rect.x + (rect.width - content).max(0.0) / 2.0).round();
        let y = (rect.y + (rect.height - metrics.height) / 2.0).round();
        frame.clipped(rect, |frame| {
            if let Some(icon) = self.icon {
                let side = metrics.height.round();
                let ink = match self.neutral_icon && self.enabled {
                    true => theme.foreground,
                    false => ink,
                };
                frame.draw_icon(icon, Rect::new(x, y, side, side), ink);
                x += side + self.icon_gap(metrics);
            }
            if !self.label.is_empty() {
                draw_label(frame, theme, self.label, Point::new(x, y), ink);
                self.underline_shortcut(frame, theme, Point::new(x, y), ink);
            }
        });
    }

    /// Underlines the first letter of the label that is its shortcut,
    /// matched without regard to case, with the label drawn at `at`.
    fn underline_shortcut(&self, frame: &mut Frame, theme: &Theme, at: Point, ink: Rgb) {
        let Some(shortcut) = self.shortcut else {
            return;
        };
        let wanted = shortcut.to_lowercase().next().unwrap_or(shortcut);
        let Some((start, letter)) = self
            .label
            .char_indices()
            .find(|(_, ch)| ch.to_lowercase().next() == Some(wanted))
        else {
            return;
        };
        let scale_factor = frame.scale_factor();
        let metrics = frame.cell_metrics();
        let left = label_width(frame, theme, &self.label[..start]);
        let width = label_width(frame, theme, &self.label[start..start + letter.len_utf8()]);
        let stroke = logical(SHORTCUT_STROKE, scale_factor).max(1.0);
        let rect = Rect::new(
            (at.x + left).round(),
            (at.y + metrics.height - stroke).round(),
            width.round().max(1.0),
            stroke,
        );
        let style = BoxStyle {
            border: None,
            corner_radius: 0.0,
            fill: Rgba::from(ink),
            shadow: None,
        };
        frame.draw_box(rect, &style);
    }

    /// The width of the icon and label side by side, without padding.
    fn content_width(&self, frame: &Frame, theme: &Theme) -> f32 {
        let metrics = frame.cell_metrics();
        let label = match self.label.is_empty() {
            true => 0.0,
            false => label_width(frame, theme, self.label),
        };
        let icon = match self.icon {
            Some(_) => metrics.height.round() + self.icon_gap(metrics),
            None => 0.0,
        };
        icon + label
    }

    /// The blank between the icon and the label: none without a label.
    fn icon_gap(&self, metrics: CellMetrics) -> f32 {
        match self.label.is_empty() {
            true => 0.0,
            false => (ICON_GAP_CELLS * metrics.width).round(),
        }
    }

    /// The button's fill (`None` for none) and the color of its label and
    /// icon, from `theme`.
    fn colors(&self, theme: &Theme) -> (Option<Rgb>, Rgb) {
        let lit = self.hover > 0.0 || self.pressed;
        match (self.enabled, self.variant) {
            (false, ButtonVariant::Ghost) => (None, theme.muted),
            (false, ButtonVariant::Danger | ButtonVariant::Default | ButtonVariant::Primary) => {
                (Some(theme.hover), theme.muted)
            }
            (true, ButtonVariant::Default | ButtonVariant::Ghost) if self.active => {
                (Some(theme.selection), theme.accent)
            }
            (true, ButtonVariant::Default) => (Some(theme.hover), theme.foreground),
            (true, ButtonVariant::Ghost) => (lit.then_some(theme.hover), theme.foreground),
            (true, ButtonVariant::Primary) => (Some(theme.accent), theme.background),
            (true, ButtonVariant::Danger) => (Some(theme.danger), theme.on_danger),
        }
    }

    /// How opaque the foreground laid over the fill is, or `None` for no
    /// tint: a press (or a filled toggle pressed in) tints more than the
    /// pointer resting over it. A ghost button's hover is its fill.
    fn tint(&self) -> Option<u8> {
        if !self.enabled {
            return None;
        }
        let filled_toggle =
            self.active && matches!(self.variant, ButtonVariant::Danger | ButtonVariant::Primary);
        if self.pressed || filled_toggle {
            return Some(PRESS_TINT);
        }
        if self.hover <= 0.0 {
            return None;
        }
        match self.variant {
            ButtonVariant::Danger | ButtonVariant::Default | ButtonVariant::Primary => {
                Some((f32::from(HOVER_TINT) * self.hover).round() as u8)
            }
            ButtonVariant::Ghost => None,
        }
    }

    /// Whether its fill is only there for the pointer over it: a ghost
    /// button, enabled, neither pressed nor pressed in.
    fn is_ghost_hover(&self) -> bool {
        self.variant == ButtonVariant::Ghost && self.enabled && !self.pressed && !self.active
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The light of a button `hovered` or not, drawn at once without a fade.
pub(crate) fn hover_light(hovered: bool) -> f32 {
    match hovered {
        true => 1.0,
        false => 0.0,
    }
}

/// How wide a button showing `label` and `icon` is drawn in `frame` with
/// `theme` when it has the room, as [`Button::width`] measures one: its
/// face's width, whatever its state or look.
pub(crate) fn natural_width(frame: &Frame, theme: &Theme, label: &str, icon: Option<&Svg>) -> f32 {
    let face = ButtonFace {
        active: false,
        enabled: true,
        focus_ring: false,
        hover: 0.0,
        icon,
        label,
        neutral_icon: true,
        pressed: false,
        shortcut: None,
        variant: ButtonVariant::Default,
    };
    face.width(frame, theme)
}

/// How tall a button is with `metrics`: a row and a little padding above
/// and below.
pub(crate) fn face_height(metrics: CellMetrics) -> f32 {
    (metrics.height * (1.0 + 2.0 * PADDING_ROWS)).round()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};
    use crate::widgets::toolbar::{ToolButton, Toolbar, ToolbarItem};

    /// A one-color square, as an icon.
    const SQUARE: &[u8] =
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>"#;

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn left(pressed: bool, point: Point) -> Input {
        let input = match pressed {
            true => ButtonInput::press(PointerButton::Left, point),
            false => ButtonInput::release(PointerButton::Left, point),
        };
        Input::PointerButton(input)
    }

    /// `button` drawn at the top left of `window`, returning where.
    fn draw(window: &TestWindow, button: &mut Button) -> Rect {
        let mut frame = window.frame();
        let bounds = frame.bounds();
        button.draw(&mut frame, bounds, &Theme::dark())
    }

    fn center(rect: Rect) -> Point {
        Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    #[test]
    fn test_space_and_enter_activate_only_while_focused_and_a_toggle_flips() {
        let mut window = TestWindow::new(300, 60);
        let mut button = Button::new("Save");
        assert_eq!(
            window.feed(&mut button, &key("<space>")),
            ButtonResponse::Ignored
        );
        button.set_focused(true);
        assert_eq!(
            window.feed(&mut button, &key("<cr>")),
            ButtonResponse::Clicked
        );
        assert!(window.take_redraw());
        assert_eq!(
            window.feed(&mut button, &key("C-<cr>")),
            ButtonResponse::Ignored
        );
        let mut case = Button::new("Aa").toggle(false);
        case.set_focused(true);
        assert_eq!(
            window.feed(&mut case, &key("<space>")),
            ButtonResponse::Toggled(true)
        );
        assert_eq!(
            window.feed(&mut case, &key("<space>")),
            ButtonResponse::Toggled(false)
        );
        assert!(!case.is_active());
    }

    #[test]
    fn test_a_click_activates_on_release_inside_and_not_when_released_outside() {
        let mut window = TestWindow::new(300, 60);
        let mut button = Button::new("Save");
        let rect = draw(&window, &mut button);
        let inside = center(rect);
        let outside = Point::new(rect.x + rect.width + 20.0, inside.y);
        assert_eq!(
            window.feed(&mut button, &left(true, outside)),
            ButtonResponse::Ignored
        );
        assert_eq!(
            window.feed(&mut button, &left(true, inside)),
            ButtonResponse::Handled
        );
        assert_eq!(
            window.feed(&mut button, &left(false, inside)),
            ButtonResponse::Clicked
        );
        window.feed(&mut button, &left(true, inside));
        window.feed(&mut button, &Input::PointerMoved(outside));
        assert_eq!(
            window.feed(&mut button, &left(false, outside)),
            ButtonResponse::Handled,
            "taken back by moving away"
        );
        assert_eq!(
            window.feed(&mut button, &left(false, inside)),
            ButtonResponse::Ignored,
            "a release without a press"
        );
    }

    #[test]
    fn test_a_disabled_button_leaves_keys_clicks_and_readers_alone() {
        let mut window = TestWindow::new(300, 60);
        let mut button = Button::new("Save").with_enabled(false);
        button.set_focused(true);
        let inside = center(draw(&window, &mut button));
        window.feed(&mut button, &left(true, inside));
        assert_eq!(
            window.feed(&mut button, &left(false, inside)),
            ButtonResponse::Ignored
        );
        assert_eq!(
            window.feed(&mut button, &key("<space>")),
            ButtonResponse::Ignored
        );
        let id = FocusId::new("save");
        assert_eq!(
            button.access_action(id, AccessAction::Click(id)),
            ButtonResponse::Ignored
        );
    }

    #[test]
    fn test_buttons_describe_their_names_toggle_state_and_availability() {
        let window = TestWindow::new(400, 60);
        let mut frame = window.frame();
        let bounds = frame.bounds();
        let svg = Svg::parse(SQUARE).expect("a valid svg");
        let mut grid = Button::icon(svg).with_tooltip("Grid view").toggle(true);
        let mut delete = Button::new("Delete")
            .with_variant(ButtonVariant::Danger)
            .with_enabled(false);
        grid.draw(&mut frame, bounds, &Theme::dark());
        delete.draw(&mut frame, bounds, &Theme::dark());
        let (grid_id, delete_id) = (FocusId::new("grid"), FocusId::new("delete"));
        grid.access(&mut frame, grid_id);
        delete.access(&mut frame, delete_id);
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n\
             \x20 Button \"Grid view\" checked\n\
             \x20 Button \"Delete\" disabled\n"
        );
        assert_eq!(
            grid.access_action(grid_id, AccessAction::Click(grid_id)),
            ButtonResponse::Toggled(false)
        );
    }

    #[test]
    fn test_the_tooltip_is_offered_only_while_the_pointer_is_over() {
        let mut window = TestWindow::new(300, 60);
        let mut button = Button::new("Save").with_tooltip("Save the file");
        let rect = draw(&window, &mut button);
        assert_eq!(button.tooltip_hover(), None);
        window.feed(&mut button, &Input::PointerMoved(center(rect)));
        assert_eq!(button.tooltip_hover(), Some((rect, "Save the file")));
        window.feed(&mut button, &Input::PointerLeft);
        assert_eq!(button.tooltip_hover(), None);
    }

    #[test]
    fn test_faces_keep_their_widths_and_height_and_a_toolbar_its_natural_width() {
        // Pinned before tree buttons took icons, so the faces a toolbar
        // lays out by stay as they were: a label padded a cell each side,
        // an icon a row square half a cell from its label, and a row and
        // a half tall.
        let window = TestWindow::new(400, 60);
        let frame = window.frame();
        let theme = Theme::dark();
        let metrics = frame.cell_metrics();
        let svg = Svg::parse(SQUARE).expect("a valid svg");
        let width = |button: Button| button.width(&frame, &theme);
        let label = |text: &str| label_width(&frame, &theme, text);
        let padded = |content: f32| (content + 2.0 * metrics.width).ceil();
        let icon = metrics.height.round() + (0.5 * metrics.width).round();
        let height = (metrics.height * 1.5).round();
        assert_eq!(face_height(metrics), height);
        assert_eq!(width(Button::new("Save")), padded(label("Save")));
        assert_eq!(width(Button::icon(svg.clone())), height, "a square");
        assert_eq!(
            width(Button::new("Save").with_icon(svg.clone())),
            padded(icon + label("Save"))
        );
        let toolbar = Toolbar::new(vec![
            ToolButton::new(0).with_label("Open").into(),
            ToolButton::new(1)
                .with_icon(svg.clone())
                .with_tooltip("Back")
                .into(),
            ToolbarItem::Separator,
            ToolbarItem::Group(vec![
                ToolButton::new(2)
                    .with_label("Grid")
                    .with_icon(svg)
                    .toggle(true),
                ToolButton::new(3).with_label("List"),
            ]),
        ]);
        // Four items, a cell wide separator, and three gaps of 2 pixels.
        let faces =
            padded(label("Open")) + height + padded(icon + label("Grid")) + padded(label("List"));
        assert_eq!(
            toolbar.natural_width(&frame, &theme),
            faces + metrics.width.round() + 3.0 * 2.0
        );
    }

    #[test]
    fn test_a_tree_s_icon_keeps_the_text_color_where_the_label_takes_the_accent() {
        let theme = Theme::dark();
        let svg = Svg::parse(SQUARE).expect("a valid svg");
        let mut own = Button::icon(svg.clone()).toggle(true);
        let mut tree = Button::new("").toggle(true);
        tree.show_icon(Some(&svg), Some("Grid"));
        let row = |index: f32| Rect::new(10.0, 10.0 + 40.0 * index, 300.0, 40.0);
        let Some(snapshot) = offscreen::render(400, 100, |frame| {
            frame.clear(Rgba::from(theme.background));
            own.draw(frame, row(0.0), &theme);
            tree.draw(frame, row(1.0), &theme);
        }) else {
            return;
        };
        let middle = |button: &Button| center(button.layout.expect("drawn"));
        let at = middle(&own);
        snapshot.assert_pixel(at.x, at.y, theme.accent, "its own: the accent");
        let at = middle(&tree);
        snapshot.assert_pixel(at.x, at.y, theme.foreground, "a tree's: the text color");
        assert_eq!(tree.tooltip.as_deref(), Some("Grid"), "named by it");
    }

    #[test]
    fn test_looks_fill_by_variant_and_a_ghost_only_under_the_pointer() {
        let theme = Theme::dark();
        let mut window = TestWindow::new(400, 200);
        let mut primary = Button::new("Go").with_variant(ButtonVariant::Primary);
        let mut ghost = Button::new("Find").with_variant(ButtonVariant::Ghost);
        let mut lit = Button::new("Find").with_variant(ButtonVariant::Ghost);
        let mut active = Button::new("Aa").toggle(true);
        let row = |index: f32| Rect::new(10.0, 10.0 + 40.0 * index, 300.0, 40.0);
        let rect = draw(&window, &mut lit);
        window.feed(&mut lit, &Input::PointerMoved(center(rect)));
        let Some(snapshot) = offscreen::render(400, 200, |frame| {
            frame.clear(Rgba::from(theme.background));
            primary.draw(frame, row(0.0), &theme);
            ghost.draw(frame, row(1.0), &theme);
            lit.draw(frame, row(2.0), &theme);
            active.draw(frame, row(3.0), &theme);
        }) else {
            return;
        };
        let corner = |button: &Button| {
            let rect = button.layout.expect("drawn");
            (rect.x + 2.0, rect.y + 2.0)
        };
        let (x, y) = corner(&primary);
        snapshot.assert_pixel(x, y, theme.accent, "primary: the accent");
        let (x, y) = corner(&ghost);
        snapshot.assert_pixel(x, y, theme.background, "a ghost at rest");
        let (x, y) = corner(&active);
        snapshot.assert_pixel(x, y, theme.selection, "a toggle pressed in");
        let (x, y) = corner(&lit);
        assert_ne!(
            snapshot.pixel(x, y),
            theme.background,
            "a ghost under the pointer is filled"
        );
    }
}

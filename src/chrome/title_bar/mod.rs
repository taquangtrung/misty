//! [`TitleBar`]: the chrome a frameless window draws in place of the
//! system's: the title, minimize, maximize, and close buttons, a drag
//! region that moves the window (double-click to maximize), the resize
//! border around the whole window, and, when asked for, a menu button at
//! the left that opens the app's menu.
//!
//! The window buttons sit at the right by default; at the left with
//! [`WindowButtons::Left`] (the menu button then moves to the right edge), or
//! not at all with [`WindowButtons::None`], for a window the system already
//! decorates, which has its own buttons, border, and drag region.
//!
//! Two more parts make it the window's only bar, as editors draw theirs:
//!
//! - A menubar ([`TitleBar::with_menus`]): the titles of the app's menus
//!   (`File`, `Edit`, `View`) after the menu button. A press on one answers
//!   [`TitleBarResponse::OpenMenuTitle`]; the app opens its [`Menu`]
//!   under [`TitleBar::menu_title`] and says which is open with
//!   [`TitleBar::set_open_menu`], and while one is, the pointer moving onto
//!   another title answers the same, so the open menu follows it.
//! - Tabs ([`TitleBar::with_tabs`]): a [`TabStrip`] in the room between the
//!   menus and the window buttons, in place of the title. The strip is the
//!   app's to fill through [`TitleBar::tabs_mut`], and what it answers
//!   comes back as [`TitleBarResponse::Tabs`]; the bar around the tabs
//!   still moves the window. The bar's [`Widget::about_to_wait`] turns
//!   the strip's spinners.
//!
//! The app may add buttons of its own ([`TitleBar::with_app_buttons`]):
//! [`TitleBarButton`]s showing an icon, a label, or both, with a tooltip,
//! at the end of the room beside the menu button (beside the window
//! buttons when those sit at the right). A click on one answers
//! [`TitleBarResponse::AppButton`] with its index; screen readers click
//! it too, and the hover's help comes from [`TitleBar::tooltip_hover`].
//!
//! [`Menu`]: crate::Menu
//!
//! The app owns a `TitleBar`, offers it each window event first with
//! [`TitleBar::handle`](crate::Widget::handle), and draws it each frame with
//! [`TitleBar::draw`](crate::Widget::draw), which returns the area left for content.

mod layout;

use std::time::Instant;

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::clock;
use super::super::base::color::Rgb;
use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
use super::super::base::strings::Strings;
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::theme::Theme;
use super::super::input::click::is_double_click;
use super::super::input::event::{Input, PointerButton};
use super::super::platform::resize::ResizeEdge;
use super::super::render::font::CellMetrics;
use super::super::render::icon::{Icon, logical};
use super::super::render::image::Svg;
use super::super::widgets::label::{draw_label, draw_styled_label, label_width};
use super::tab_strip::{TabStrip, TabStripResponse};
use layout::{TitleBarLayout, bar_height};

// ========================================================================
// Constants
// ========================================================================

/// The side of a button's icon, in logical pixels.
const ICON_SIZE: f32 = 10.0;

/// The thickness of an icon's strokes and of the bar's bottom border, in
/// logical pixels.
const STROKE: f32 = 1.0;

/// The child index of the menu button's accessibility node.
const MENU_NODE: u64 = 0;

/// The child index of the minimize button's accessibility node.
const MINIMIZE_NODE: u64 = 1;

/// The child index of the maximize or restore button's accessibility
/// node.
const MAXIMIZE_NODE: u64 = 2;

/// The child index of the close button's accessibility node.
const CLOSE_NODE: u64 = 3;

/// The child index of the tab strip's accessibility node.
const TABS_NODE: u64 = 4;

/// The child index of the first menu title's accessibility node; the
/// others follow it.
const FIRST_MENU_TITLE_NODE: u64 = 5;

/// The blank cells at each side of a menu title.
pub(super) const MENU_TITLE_PADDING_CELLS: usize = 1;

/// The child index of the first app button's accessibility node; the
/// others follow it, far past the menu titles'.
pub(crate) const FIRST_APP_BUTTON_NODE: u64 = 1 << 16;

/// The blank cells at each side of an app button with a label.
pub(super) const APP_BUTTON_PADDING_CELLS: usize = 1;

/// The blank between an app button's icon and its label, in cells.
const APP_ICON_GAP_CELLS: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// A custom title bar for a frameless window. It keeps the hover, press,
/// and maximize state between events, and the layout of the last frame it
/// drew, which its hit testing uses.
#[derive(Clone, Debug)]
pub struct TitleBar {
    /// The app's buttons, left to right.
    app_buttons: Vec<TitleBarButton>,
    /// Where the window buttons go.
    buttons: WindowButtons,
    /// Whether the window has keyboard focus; an unfocused title is muted.
    focused: bool,
    /// Whether the bar shows a menu button at its left.
    has_menu_button: bool,
    /// What the pointer is over.
    hovered: Option<TitleBarHit>,
    /// When and where the drag region was last pressed, for double clicks.
    last_press: Option<(Instant, Point)>,
    /// The titles of the menubar's menus, left to right.
    menus: Vec<String>,
    /// The menubar's menu the app has open, by index.
    open_menu: Option<usize>,
    /// Where the parts were in the last frame drawn, or `None` before it.
    layout: Option<TitleBarLayout>,
    /// Whether the window is maximized: no resize border, and a restore
    /// icon in place of the maximize one.
    maximized: bool,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// The button pressed and not yet released.
    pressed: Option<WindowButton>,
    /// The app button pressed and not yet released, by index.
    pressed_app: Option<usize>,
    /// The words of the theme last drawn in, for the buttons' names.
    strings: Strings,
    /// The tabs shown in the bar in place of the title, or `None`.
    tabs: Option<TabStrip>,
    /// The text shown in the bar.
    title: String,
    /// Where the title sits in the bar.
    title_align: TitleAlign,
}

/// A button of the app's own in a [`TitleBar`]: an icon, a label, or both,
/// and the help its tooltip shows. An icon alone is as wide as a window
/// button; give it a tooltip, which also names it for screen readers.
#[derive(Clone, Debug, PartialEq)]
pub struct TitleBarButton {
    /// The one-color SVG icon it shows, left of its label, if any.
    icon: Option<Svg>,
    /// What it says; may be empty.
    label: String,
    /// The help shown while the pointer rests on it, if any.
    tooltip: Option<String>,
}

/// What a point in the window means to the title bar.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum TitleBarHit {
    /// One of the app's buttons, by index.
    AppButton(usize),
    /// One of the window buttons.
    Button(WindowButton),
    /// The rest of the bar, which moves the window.
    Drag,
    /// The menu button.
    MenuButton,
    /// A menubar title, by index.
    MenuTitle(usize),
    /// The resize border, at this edge or corner.
    Resize(ResizeEdge),
}

/// A window button in the title bar.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum WindowButton {
    /// Asks to close the window.
    Close,
    /// Maximizes the window, or restores it.
    Maximize,
    /// Minimizes the window.
    Minimize,
}

/// Where a [`TitleBar`] sets its title.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TitleAlign {
    /// Centered over the whole window, as GNOME and macOS set it; moved
    /// aside only as far as the buttons and menus beside it need. The
    /// default on Linux and macOS.
    Center,
    /// At the start of the room beside the buttons and menus, as Windows
    /// sets it. The default on Windows.
    Start,
}

/// Where a [`TitleBar`] puts the window buttons.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum WindowButtons {
    /// At the left edge, as macOS and GNOME's left layout place them:
    /// close outermost, then minimize, then maximize. The menu button
    /// moves to the right edge.
    Left,
    /// Nowhere: the system decorates the window, with its own buttons,
    /// resize border, and drag region, so the bar has none of these.
    None,
    /// At the right edge, as most systems place them: close outermost,
    /// then maximize, then minimize, so they read minimize, maximize,
    /// close.
    #[default]
    Right,
}

/// What became of an input offered to a [`TitleBar`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum TitleBarResponse {
    /// The app's button at this index was clicked (pressed and released
    /// over it), or clicked by a screen reader.
    AppButton(usize),
    /// The close button was clicked: the app decides what closing means,
    /// as it does for [`Input::CloseRequested`].
    Close,
    /// The title bar used the event; the app should not act on it again.
    Handled,
    /// The event is not the title bar's; the app handles it as usual.
    Ignored,
    /// The menu button was pressed: open the app's menu under
    /// [`TitleBar::menu_button`].
    OpenMenu,
    /// The menubar title at this index was pressed, or the pointer moved
    /// onto it while another menu is open: open its menu under
    /// [`TitleBar::menu_title`], closing any other.
    OpenMenuTitle(usize),
    /// The bar's tab strip answered this; act on it as on a
    /// [`TabStrip`]'s answer.
    Tabs(TabStripResponse),
}

// ========================================================================
// TitleBar Implementations
// ========================================================================

impl TitleBar {
    /// A title bar showing `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            app_buttons: Vec::new(),
            buttons: WindowButtons::Right,
            focused: true,
            has_menu_button: false,
            hovered: None,
            last_press: None,
            layout: None,
            maximized: false,
            menus: Vec::new(),
            open_menu: None,
            pointer: None,
            pressed: None,
            pressed_app: None,
            strings: Strings::english(),
            tabs: None,
            title: title.into(),
            title_align: TitleAlign::default(),
        }
    }

    /// This title bar with its title set where `align` says.
    pub fn with_title_align(self, title_align: TitleAlign) -> Self {
        Self {
            title_align,
            ..self
        }
    }

    /// This title bar with its window buttons placed as `buttons` says.
    pub fn with_window_buttons(self, buttons: WindowButtons) -> Self {
        Self { buttons, ..self }
    }

    /// How tall a title bar is for cells of `metrics`, for laying out what
    /// sits below it before it is drawn. The scale factor is unused here;
    /// every bar takes it, so an app asks each bar's height the same way.
    pub fn height(metrics: CellMetrics, _scale_factor: f64) -> f32 {
        bar_height(metrics)
    }

    /// This title bar with a menubar of menus titled `titles`, left to
    /// right, after the menu button.
    pub fn with_menus(self, titles: Vec<String>) -> Self {
        Self {
            menus: titles,
            ..self
        }
    }

    /// This title bar with `tabs` in place of its title.
    pub fn with_tabs(self, tabs: TabStrip) -> Self {
        Self {
            tabs: Some(tabs),
            ..self
        }
    }

    /// The bar's tab strip, or `None` when it shows its title.
    pub fn tabs(&self) -> Option<&TabStrip> {
        self.tabs.as_ref()
    }

    /// The bar's tab strip, to add, close, and mark tabs; `None` when it
    /// shows its title.
    pub fn tabs_mut(&mut self) -> Option<&mut TabStrip> {
        self.tabs.as_mut()
    }

    /// Where the menubar title at `index` was last drawn, for opening its
    /// menu under it; `None` before the first frame or past the last.
    pub fn menu_title(&self, index: usize) -> Option<Rect> {
        self.layout.as_ref()?.menu_titles.get(index).copied()
    }

    /// Says which menubar menu the app has open, by index, or that none
    /// is: the open one's title stays lit, and the pointer moving onto
    /// another title opens that one instead. Ask for a redraw after.
    pub fn set_open_menu(&mut self, index: Option<usize>) {
        self.open_menu = index;
    }

    /// This title bar with a menu button at its left (at its right when the
    /// window buttons are at the left), which answers a press with
    /// [`TitleBarResponse::OpenMenu`].
    pub fn with_menu_button(self) -> Self {
        Self {
            has_menu_button: true,
            ..self
        }
    }

    /// Where the menu button was last drawn, for opening the menu under
    /// it; `None` before the first frame or when the bar has none.
    pub fn menu_button(&self) -> Option<Rect> {
        self.layout.as_ref()?.menu
    }

    /// Where window button `button` was last drawn, for its tooltip to hang
    /// under; `None` before the first frame or without window buttons.
    pub fn button_rect(&self, button: WindowButton) -> Option<Rect> {
        self.layout.as_ref()?.button(button)
    }

    /// This title bar with `buttons` of the app's own, left to right, at
    /// the end of the room beside the menu button (beside the window
    /// buttons when those sit at the right). A click on one answers
    /// [`TitleBarResponse::AppButton`] with its index.
    pub fn with_app_buttons(self, buttons: Vec<TitleBarButton>) -> Self {
        Self {
            app_buttons: buttons,
            ..self
        }
    }

    /// Changes the app's buttons, as a theme button's icon flips with the
    /// theme. Ask for a redraw after.
    pub fn set_app_buttons(&mut self, buttons: Vec<TitleBarButton>) {
        if self.app_buttons != buttons {
            self.app_buttons = buttons;
            self.pressed_app = None;
        }
    }

    /// Where the app's button at `index` was last drawn; `None` before the
    /// first frame, past the last button, or for a button that did not
    /// fit.
    pub fn app_button(&self, index: usize) -> Option<Rect> {
        self.layout.as_ref()?.app_buttons.get(index).copied()
    }

    /// Where the app's button under the pointer was drawn and its tooltip,
    /// while the pointer is over one that has a tooltip, for the app's
    /// [`Tooltip`](crate::Tooltip) to show.
    pub fn tooltip_hover(&self) -> Option<(Rect, String)> {
        let Some(TitleBarHit::AppButton(index)) = self.hovered else {
            return None;
        };
        let tooltip = self.app_buttons.get(index)?.tooltip.clone()?;
        Some((self.app_button(index)?, tooltip))
    }

    /// Where each app button with a tooltip was last drawn, with its help.
    pub(crate) fn app_button_tips(&self) -> Vec<(Rect, String)> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        layout
            .app_buttons
            .iter()
            .zip(&self.app_buttons)
            .filter_map(|(rect, button)| Some((*rect, button.tooltip.clone()?)))
            .collect()
    }

    /// Changes the text shown in the bar.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// What `point` hits in the window as last drawn, or `None` for the
    /// content area or before the first frame. The resize border comes
    /// first, except on a maximized window, which has none.
    pub fn hit_test(&self, point: Point) -> Option<TitleBarHit> {
        self.layout.as_ref()?.hit(point, self.maximized)
    }

    /// Draws `button`'s hover background and icon, the icon in
    /// `icon_color` unless it sits on the close button's danger color.
    fn draw_button(&self, frame: &mut Frame, theme: &Theme, button: WindowButton, icon_color: Rgb) {
        let Some(rect) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.button(button))
        else {
            return;
        };
        let scale_factor = self
            .layout
            .as_ref()
            .map_or(1.0, |layout| layout.scale_factor);
        let lit = self.hovered == Some(TitleBarHit::Button(button)) || self.pressed == Some(button);
        let icon_color = match (lit, button) {
            (true, WindowButton::Close) => {
                frame.fill_rect(rect, theme.danger);
                theme.on_danger
            }
            (true, WindowButton::Maximize | WindowButton::Minimize) => {
                frame.fill_rect(rect, theme.hover);
                icon_color
            }
            (false, WindowButton::Close | WindowButton::Maximize | WindowButton::Minimize) => {
                icon_color
            }
        };
        let icon = match button {
            WindowButton::Close => Icon::Close,
            WindowButton::Maximize if self.maximized => Icon::Restore,
            WindowButton::Maximize => Icon::Maximize,
            WindowButton::Minimize => Icon::Minimize,
        };
        for stroke in icon.strokes(rect, ICON_SIZE, STROKE, scale_factor) {
            frame.fill_rect_over_text(stroke, icon_color);
        }
    }

    /// Moves the hover to `hit`, redrawing when a button's hover changed and
    /// changing the pointer shape when it should look different.
    fn set_hovered(&mut self, cx: &mut EventCx<'_>, hit: Option<TitleBarHit>) {
        if self.hovered == hit {
            return;
        }
        if cursor_for(self.hovered) != cursor_for(hit) {
            cx.set_cursor(cursor_for(hit));
        }
        let is_button = |hit: Option<TitleBarHit>| {
            matches!(
                hit,
                Some(
                    TitleBarHit::AppButton(_)
                        | TitleBarHit::Button(_)
                        | TitleBarHit::MenuButton
                        | TitleBarHit::MenuTitle(_)
                )
            )
        };
        if is_button(self.hovered) || is_button(hit) {
            cx.request_redraw();
        }
        self.hovered = hit;
    }

    /// A left press: start a resize or a move, maximize on a double click,
    /// or press a button.
    fn press(&mut self, cx: &mut EventCx<'_>) -> TitleBarResponse {
        let Some(point) = self.pointer else {
            return TitleBarResponse::Ignored;
        };
        let Some(hit) = self.hit_test(point) else {
            return TitleBarResponse::Ignored;
        };
        match hit {
            TitleBarHit::AppButton(index) => {
                self.pressed_app = Some(index);
                cx.request_redraw();
            }
            TitleBarHit::Button(button) => {
                self.pressed = Some(button);
                cx.request_redraw();
            }
            TitleBarHit::Drag => {
                let now = clock::now();
                let scale_factor = self
                    .layout
                    .as_ref()
                    .map_or(1.0, |layout| layout.scale_factor);
                if is_double_click(self.last_press, now, point, scale_factor) {
                    self.last_press = None;
                    self.toggle_maximize(cx);
                } else {
                    self.last_press = Some((now, point));
                    cx.drag_window();
                }
            }
            TitleBarHit::MenuButton => {
                cx.request_redraw();
                return TitleBarResponse::OpenMenu;
            }
            TitleBarHit::MenuTitle(index) => {
                cx.request_redraw();
                return TitleBarResponse::OpenMenuTitle(index);
            }
            TitleBarHit::Resize(edge) => cx.drag_resize_window(edge),
        }
        TitleBarResponse::Handled
    }

    /// Describes the bar as last drawn to screen readers as `id`: a title
    /// bar named by its title, holding its buttons, each clickable (the
    /// `index`th child of `id` by [`FocusId::child`]: menu 0, minimize 1,
    /// maximize 2, close 3), its tab strip (child 4, described as
    /// [`TabStrip::access`] does), its menubar titles (5 on), and the
    /// app's buttons (from `FIRST_APP_BUTTON_NODE`, 65536, on), each named
    /// by its label, else its tooltip, and described by its tooltip.
    /// Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        let mut bar = Node::new(Role::TitleBar);
        bar.set_label(self.title.as_str());
        frame.enter_access(id, layout.bar, bar);
        let strings = &self.strings;
        let maximize = match self.maximized {
            true => &strings.restore,
            false => &strings.maximize,
        };
        let buttons = [
            layout.menu.map(|rect| (MENU_NODE, rect, &strings.menu)),
            layout
                .minimize
                .map(|rect| (MINIMIZE_NODE, rect, &strings.minimize)),
            layout.maximize.map(|rect| (MAXIMIZE_NODE, rect, maximize)),
            layout.close.map(|rect| (CLOSE_NODE, rect, &strings.close)),
        ];
        for (index, rect, label) in buttons.into_iter().flatten() {
            let mut button = Node::new(Role::Button);
            button.set_label(label.as_ref());
            button.add_action(Action::Click);
            frame.access(id.child(index), rect, button);
        }
        for (index, (rect, title)) in layout.menu_titles.iter().zip(&self.menus).enumerate() {
            let mut item = Node::new(Role::MenuItem);
            item.set_label(title.as_str());
            item.add_action(Action::Click);
            frame.access(id.child(FIRST_MENU_TITLE_NODE + index as u64), *rect, item);
        }
        for (index, (rect, app)) in layout.app_buttons.iter().zip(&self.app_buttons).enumerate() {
            let mut button = Node::new(Role::Button);
            button.set_label(app.name());
            if let Some(tooltip) = app.tooltip.as_deref().filter(|tip| *tip != app.name()) {
                button.set_description(tooltip);
            }
            button.add_action(Action::Click);
            frame.access(
                id.child(FIRST_APP_BUTTON_NODE + index as u64),
                *rect,
                button,
            );
        }
        if let Some(tabs) = &self.tabs {
            tabs.access(frame, id.child(TABS_NODE));
        }
        frame.leave_access();
    }

    /// Does what a screen reader's `action` on the bar described as `id`
    /// asks: a click on a button acts as clicking it does. Ignored for
    /// other actions and nodes.
    pub fn access_action(
        &mut self,
        id: FocusId,
        action: AccessAction,
        cx: &mut EventCx<'_>,
    ) -> TitleBarResponse {
        if let Some(tabs) = &mut self.tabs {
            let answer = tabs.access_action(id.child(TABS_NODE), action);
            if !answer.is_ignored() {
                return TitleBarResponse::Tabs(answer);
            }
        }
        let AccessAction::Click(target) = action else {
            return TitleBarResponse::Ignored;
        };
        let menu_title = (0..self.menus.len())
            .find(|index| target == id.child(FIRST_MENU_TITLE_NODE + *index as u64));
        if let Some(index) = menu_title {
            return TitleBarResponse::OpenMenuTitle(index);
        }
        let app_button = (0..self.app_buttons.len())
            .find(|index| target == id.child(FIRST_APP_BUTTON_NODE + *index as u64));
        if let Some(index) = app_button {
            return TitleBarResponse::AppButton(index);
        }
        match target {
            target if target == id.child(MENU_NODE) && self.has_menu_button => {
                TitleBarResponse::OpenMenu
            }
            target if target == id.child(MINIMIZE_NODE) => {
                cx.minimize();
                TitleBarResponse::Handled
            }
            target if target == id.child(MAXIMIZE_NODE) => {
                self.toggle_maximize(cx);
                TitleBarResponse::Handled
            }
            target if target == id.child(CLOSE_NODE) => TitleBarResponse::Close,
            _ => TitleBarResponse::Ignored,
        }
    }

    /// A left release: click the pressed button if the pointer is still on
    /// it.
    fn release(&mut self, cx: &mut EventCx<'_>) -> TitleBarResponse {
        if let Some(index) = self.pressed_app.take() {
            cx.request_redraw();
            let over = self.pointer.and_then(|point| self.hit_test(point));
            return match over == Some(TitleBarHit::AppButton(index)) {
                true => TitleBarResponse::AppButton(index),
                false => TitleBarResponse::Handled,
            };
        }
        let Some(button) = self.pressed.take() else {
            return TitleBarResponse::Ignored;
        };
        cx.request_redraw();
        let over = self.pointer.and_then(|point| self.hit_test(point));
        if over != Some(TitleBarHit::Button(button)) {
            return TitleBarResponse::Handled;
        }
        match button {
            WindowButton::Close => return TitleBarResponse::Close,
            WindowButton::Maximize => self.toggle_maximize(cx),
            WindowButton::Minimize => cx.minimize(),
        }
        TitleBarResponse::Handled
    }

    /// Maximizes or restores the window, showing the new state at once
    /// rather than waiting for the window system, which not every platform
    /// reports; the next resize corrects it if the guess was wrong.
    fn toggle_maximize(&mut self, cx: &mut EventCx<'_>) {
        cx.toggle_maximize();
        self.maximized = !self.maximized;
        cx.request_redraw();
    }

    /// Offers `input` to the bar's tab strip, if it has one, and returns
    /// what the strip answered when the input was the strip's: a press on
    /// a tab or its buttons, or anything while a tab is pressed or dragged.
    /// A press on the resize border or a window button, or on the strip's
    /// empty room, stays the bar's.
    fn offer_tabs(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<TabStripResponse> {
        let point = match input {
            Input::PointerButton(button) => Some(button.position),
            Input::PointerMoved(point) => Some(*point),
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
            | Input::PointerLeft
            | Input::Wheel(_) => self.pointer,
        };
        let bar_hit = point.and_then(|point| self.hit_test(point));
        let tabs = self.tabs.as_mut()?;
        let is_press = matches!(input, Input::PointerButton(button) if button.pressed);
        let on_bar_part = matches!(
            bar_hit,
            Some(
                TitleBarHit::AppButton(_)
                    | TitleBarHit::Button(_)
                    | TitleBarHit::MenuButton
                    | TitleBarHit::MenuTitle(_)
                    | TitleBarHit::Resize(_)
            )
        );
        if is_press && on_bar_part {
            return None;
        }
        let on_tab = point.and_then(|point| tabs.hit_test(point)).is_some();
        let engaged = tabs.is_engaged();
        let answer = tabs.handle(input, cx);
        match answer {
            TabStripResponse::Ignored => None,
            TabStripResponse::Handled
                if !(on_tab || engaged || matches!(input, Input::Wheel(_))) =>
            {
                None
            }
            TabStripResponse::Activated(_)
            | TabStripResponse::Close(_)
            | TabStripResponse::Dragging(_)
            | TabStripResponse::DroppedOutside(_)
            | TabStripResponse::Handled
            | TabStripResponse::Moved(_)
            | TabStripResponse::NewTab => Some(answer),
        }
    }
}

impl Widget for TitleBar {
    type Output = Rect;
    type Response = TitleBarResponse;

    /// Reacts to `input`: hover and pointer shapes, button
    /// clicks, moving and resizing the window, double-click to maximize,
    /// and keeping the focused state current (the maximized state comes
    /// with each frame). Offer it every input before acting on it, and
    /// leave alone what it handled.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TitleBarResponse {
        if let Some(answer) = self.offer_tabs(input, cx) {
            if let Input::PointerMoved(point) = input {
                self.pointer = Some(*point);
                self.set_hovered(cx, None);
            }
            return TitleBarResponse::Tabs(answer);
        }
        match input {
            Input::FocusGained | Input::FocusLost => {
                self.focused = *input == Input::FocusGained;
                cx.request_redraw();
                TitleBarResponse::Ignored
            }
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.pointer = Some(button.position);
                match button.pressed {
                    true => self.press(cx),
                    false => self.release(cx),
                }
            }
            Input::PointerLeft => {
                self.pointer = None;
                self.set_hovered(cx, None);
                TitleBarResponse::Ignored
            }
            Input::PointerMoved(point) => {
                self.pointer = Some(*point);
                let hit = self.hit_test(*point);
                let entered = hit != self.hovered;
                self.set_hovered(cx, hit);
                match hit {
                    Some(TitleBarHit::MenuTitle(index))
                        if entered && self.open_menu.is_some_and(|open| open != index) =>
                    {
                        TitleBarResponse::OpenMenuTitle(index)
                    }
                    Some(_) => TitleBarResponse::Handled,
                    None => TitleBarResponse::Ignored,
                }
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => TitleBarResponse::Ignored,
        }
    }

    /// Draws the bar along the top of `area` in `theme`'s colors and
    /// returns the part of `area` below it, which is the app's to draw
    /// content in. Its resize border runs along the window's edges, so
    /// `area` is the whole window in a frameless window.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        self.maximized = frame.is_maximized();
        if self.strings != theme.strings {
            self.strings = theme.strings.clone();
        }
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        // Measured as they are drawn, so each title's hit rect covers it.
        let menu_widths: Vec<f32> = self
            .menus
            .iter()
            .map(|title| label_width(frame, theme, title))
            .collect();
        let app_widths: Vec<f32> = self
            .app_buttons
            .iter()
            .map(|button| button.content_width(frame, theme))
            .collect();
        let layout = TitleBarLayout::new(
            area,
            metrics,
            scale_factor,
            self.has_menu_button,
            &menu_widths,
            &app_widths,
            self.buttons,
        );
        let stroke = logical(STROKE, scale_factor);
        let bar = layout.bar;
        frame.fill_rect(bar, theme.surface);
        let text_color = match self.focused {
            true => theme.foreground,
            false => theme.muted,
        };
        let top = ((bar.height - metrics.height) / 2.0).round();
        let middle = layout.middle;
        let bottom = bar.y + bar.height - stroke;
        // Drawn last, so the hover fills of titles and buttons leave it
        // whole.
        let borders = match &mut self.tabs {
            Some(tabs) => {
                tabs.draw(frame, middle, theme);
                // The strip draws its own bottom line, open under the
                // active tab; the bar's runs either side of it.
                let border =
                    |from: f32, to: f32| Rect::new(from, bottom, (to - from).max(0.0), stroke);
                vec![
                    border(bar.x, middle.x),
                    border(middle.x + middle.width, bar.x + bar.width),
                ]
            }
            None => {
                let padding = metrics.width;
                let room = (middle.width - 2.0 * padding).max(0.0);
                // Whole characters only: a cut-off one reads worse than none.
                let title =
                    StyledText::plain(self.title.as_str(), TextColor::Rgba(text_color.into()))
                        .with_ellipsis();
                let start = middle.x + padding;
                let x = match self.title_align {
                    TitleAlign::Center => {
                        let width = label_width(frame, theme, &self.title);
                        centered_x(bar, start, room, width)
                    }
                    TitleAlign::Start => start,
                };
                let room = room - (x - start);
                let origin = Point::new(x, bar.y + top);
                let clip = Rect::new(origin.x, bar.y, room, bar.height);
                draw_styled_label(frame, theme, &title, origin, clip);
                vec![Rect::new(bar.x, bottom, bar.width, stroke)]
            }
        };
        for (index, (rect, title)) in layout.menu_titles.iter().zip(&self.menus).enumerate() {
            let lit = self.open_menu == Some(index)
                || self.hovered == Some(TitleBarHit::MenuTitle(index));
            if lit {
                frame.fill_rect(*rect, theme.hover);
            }
            let x = rect.x + MENU_TITLE_PADDING_CELLS as f32 * metrics.width;
            draw_label(frame, theme, title, Point::new(x, bar.y + top), text_color);
        }
        self.layout = Some(layout.clone());
        if let Some(menu) = layout.menu {
            if self.hovered == Some(TitleBarHit::MenuButton) {
                frame.fill_rect(menu, theme.hover);
            }
            for stroke in Icon::Menu.strokes(menu, ICON_SIZE, STROKE, scale_factor) {
                frame.fill_rect_over_text(stroke, text_color);
            }
        }
        for (index, (rect, button)) in layout.app_buttons.iter().zip(&self.app_buttons).enumerate()
        {
            let lit = self.hovered == Some(TitleBarHit::AppButton(index))
                || self.pressed_app == Some(index);
            if lit {
                frame.fill_rect(*rect, theme.hover);
            }
            button.draw(frame, theme, *rect, text_color);
        }
        for button in [
            WindowButton::Minimize,
            WindowButton::Maximize,
            WindowButton::Close,
        ] {
            self.draw_button(frame, theme, button, text_color);
        }
        for border in borders {
            frame.fill_rect_over_text(border, theme.border);
        }
        Rect::new(
            area.x,
            bar.y + bar.height,
            area.width,
            (area.height - bar.height).max(0.0),
        )
    }

    /// Turns the spinners of the tabs' busy tabs, returning when they next
    /// turn; `None` without tabs or while none is busy.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.tabs.as_mut()?.about_to_wait(cx)
    }
}

impl Outcome for TitleBarResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// TitleBarButton Implementations
// ========================================================================

impl TitleBarButton {
    /// A button showing `icon`, a one-color SVG, with `tooltip` as its
    /// help and its name for screen readers.
    pub fn icon(icon: Svg, tooltip: impl Into<String>) -> Self {
        Self {
            icon: Some(icon),
            label: String::new(),
            tooltip: Some(tooltip.into()),
        }
    }

    /// A button saying `label`.
    pub fn label(label: impl Into<String>) -> Self {
        Self {
            icon: None,
            label: label.into(),
            tooltip: None,
        }
    }

    /// This button showing `icon`, a one-color SVG, left of its label.
    pub fn with_icon(self, icon: Svg) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// This button with `tooltip` as its help.
    pub fn with_tooltip(self, tooltip: impl Into<String>) -> Self {
        Self {
            tooltip: Some(tooltip.into()),
            ..self
        }
    }

    /// What screen readers call it: its label, else its tooltip.
    fn name(&self) -> &str {
        match (self.label.is_empty(), self.tooltip.as_deref()) {
            (true, Some(tooltip)) => tooltip,
            (true, None) | (false, _) => &self.label,
        }
    }

    /// How wide its icon and label are side by side in `theme`'s chrome
    /// font, without padding; 0 for an icon alone.
    fn content_width(&self, frame: &Frame, theme: &Theme) -> f32 {
        if self.label.is_empty() {
            return 0.0;
        }
        let metrics = frame.cell_metrics();
        let icon = match self.icon {
            Some(_) => metrics.height.round() + (APP_ICON_GAP_CELLS * metrics.width).round(),
            None => 0.0,
        };
        icon + label_width(frame, theme, &self.label)
    }

    /// Draws its icon and label centered in `rect`, in `color`.
    fn draw(&self, frame: &mut Frame, theme: &Theme, rect: Rect, color: Rgb) {
        let metrics = frame.cell_metrics();
        let side = metrics.height.round();
        let content = match self.label.is_empty() {
            true => side,
            false => self.content_width(frame, theme),
        };
        let mut x = (rect.x + (rect.width - content).max(0.0) / 2.0).round();
        let y = (rect.y + (rect.height - metrics.height) / 2.0).round();
        frame.clipped(rect, |frame| {
            if let Some(icon) = &self.icon {
                frame.draw_icon(icon, Rect::new(x, y, side, side), color);
                x += side + (APP_ICON_GAP_CELLS * metrics.width).round();
            }
            if !self.label.is_empty() {
                draw_label(frame, theme, &self.label, Point::new(x, y), color);
            }
        });
    }
}

// ========================================================================
// TitleAlign Implementations
// ========================================================================

impl Default for TitleAlign {
    /// Where the platform sets window titles: at the start on Windows,
    /// centered elsewhere.
    fn default() -> Self {
        match cfg!(windows) {
            true => Self::Start,
            false => Self::Center,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where a title `width` wide starts when centered over `bar`, held inside
/// the room `room` wide from `start`: centered over the room where the bar's
/// middle would put it under the buttons or menus, and at `start` when it
/// fills the room.
fn centered_x(bar: Rect, start: f32, room: f32, width: f32) -> f32 {
    let centered = bar.x + (bar.width - width) / 2.0;
    let last = start + (room - width).max(0.0);
    centered.clamp(start, last).round()
}

/// The pointer shape over `hit`: a resize arrow on the border, the
/// default elsewhere.
fn cursor_for(hit: Option<TitleBarHit>) -> CursorIcon {
    match hit {
        Some(TitleBarHit::Resize(edge)) => edge.cursor(),
        Some(
            TitleBarHit::AppButton(_)
            | TitleBarHit::Button(_)
            | TitleBarHit::Drag
            | TitleBarHit::MenuButton
            | TitleBarHit::MenuTitle(_),
        )
        | None => CursorIcon::Default,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::find_node;
    use crate::app::context::WindowAction;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::text;
    use crate::base::theme::ChromeText;
    use crate::input::event::ButtonInput;
    use crate::testing::TestWindow;

    const METRICS: CellMetrics = CellMetrics {
        baseline: 15.0,
        height: 20.0,
        width: 10.0,
    };

    const WINDOW: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    /// The bar is 32 pixels high and each button 48 wide, so the buttons
    /// start at 800 - 144 = 656.
    fn layout() -> TitleBarLayout {
        TitleBarLayout::new(WINDOW, METRICS, 1.0, false, &[], &[], WindowButtons::Right)
    }

    #[test]
    fn test_buttons_sit_at_the_right_in_minimize_maximize_close_order() {
        let layout = layout();
        assert_eq!(layout.bar.height, 32.0);
        let [close, maximize, minimize] =
            [layout.close, layout.maximize, layout.minimize].map(|rect| rect.expect("buttons"));
        assert_eq!(close, Rect::new(752.0, 0.0, 48.0, 32.0));
        assert_eq!(maximize.x + maximize.width, close.x);
        assert_eq!(minimize.x + minimize.width, maximize.x);
    }

    #[test]
    fn test_each_button_is_hit_inside_its_own_rect() {
        let layout = layout();
        let hit = |x: f32| layout.hit(Point::new(x, 16.0), false);
        assert_eq!(hit(780.0), Some(TitleBarHit::Button(WindowButton::Close)));
        assert_eq!(hit(752.0), Some(TitleBarHit::Button(WindowButton::Close)));
        assert_eq!(
            hit(751.0),
            Some(TitleBarHit::Button(WindowButton::Maximize))
        );
        assert_eq!(
            hit(660.0),
            Some(TitleBarHit::Button(WindowButton::Minimize))
        );
        assert_eq!(hit(655.0), Some(TitleBarHit::Drag));
    }

    #[test]
    fn test_the_bar_drags_and_the_content_below_is_not_the_title_bars() {
        let layout = layout();
        assert_eq!(
            layout.hit(Point::new(300.0, 20.0), false),
            Some(TitleBarHit::Drag)
        );
        assert_eq!(layout.hit(Point::new(300.0, 32.0), false), None);
        assert_eq!(layout.hit(Point::new(300.0, 300.0), false), None);
    }

    #[test]
    fn test_the_resize_border_wins_over_the_bar_and_its_buttons() {
        let layout = layout();
        let hit = |x: f32, y: f32| layout.hit(Point::new(x, y), false);
        assert_eq!(
            hit(300.0, 2.0),
            Some(TitleBarHit::Resize(ResizeEdge::North))
        );
        assert_eq!(
            hit(798.0, 2.0),
            Some(TitleBarHit::Resize(ResizeEdge::NorthEast))
        );
        assert_eq!(
            hit(798.0, 20.0),
            Some(TitleBarHit::Resize(ResizeEdge::East))
        );
        assert_eq!(
            hit(2.0, 598.0),
            Some(TitleBarHit::Resize(ResizeEdge::SouthWest))
        );
        assert_eq!(hit(2.0, 300.0), Some(TitleBarHit::Resize(ResizeEdge::West)));
    }

    #[test]
    fn test_a_maximized_window_has_no_resize_border() {
        let layout = layout();
        let hit = |x: f32, y: f32| layout.hit(Point::new(x, y), true);
        assert_eq!(hit(300.0, 2.0), Some(TitleBarHit::Drag));
        assert_eq!(
            hit(798.0, 2.0),
            Some(TitleBarHit::Button(WindowButton::Close))
        );
        assert_eq!(hit(2.0, 300.0), None);
    }

    #[test]
    fn test_the_menu_button_sits_at_the_left_inside_the_resize_border() {
        let layout =
            TitleBarLayout::new(WINDOW, METRICS, 1.0, true, &[], &[], WindowButtons::Right);
        let hit = |x: f32, y: f32| layout.hit(Point::new(x, y), false);
        assert_eq!(hit(20.0, 16.0), Some(TitleBarHit::MenuButton));
        assert_eq!(hit(47.0, 16.0), Some(TitleBarHit::MenuButton));
        assert_eq!(hit(48.0, 16.0), Some(TitleBarHit::Drag));
        assert_eq!(hit(2.0, 20.0), Some(TitleBarHit::Resize(ResizeEdge::West)));
        assert_eq!(
            layout.hit(Point::new(2.0, 20.0), true),
            Some(TitleBarHit::MenuButton)
        );
        assert_eq!(hit(20.0, 40.0), None);
    }

    #[test]
    fn test_a_centered_title_sits_mid_window_and_moves_aside_only_for_what_is_beside_it() {
        let bar = Rect::new(0.0, 0.0, 800.0, 32.0);
        // Room from 100 to 700: a 200 wide title fits centered at 300.
        assert_eq!(centered_x(bar, 100.0, 600.0, 200.0), 300.0);
        // Room from 450 to 700: the middle would put it under the menus.
        assert_eq!(centered_x(bar, 450.0, 250.0, 200.0), 450.0);
        // Room from 100 to 400: it would run into the buttons.
        assert_eq!(centered_x(bar, 100.0, 300.0, 200.0), 200.0);
        // Wider than the room: it starts at the room's start.
        assert_eq!(centered_x(bar, 100.0, 150.0, 200.0), 100.0);
    }

    #[test]
    fn test_buttons_at_the_left_read_close_minimize_maximize_with_the_menu_at_the_right() {
        let layout = TitleBarLayout::new(
            WINDOW,
            METRICS,
            1.0,
            true,
            &menu_widths(),
            &[],
            WindowButtons::Left,
        );
        let hit = |x: f32| layout.hit(Point::new(x, 16.0), true);
        assert_eq!(hit(20.0), Some(TitleBarHit::Button(WindowButton::Close)));
        assert_eq!(hit(60.0), Some(TitleBarHit::Button(WindowButton::Minimize)));
        assert_eq!(
            hit(110.0),
            Some(TitleBarHit::Button(WindowButton::Maximize))
        );
        assert_eq!(hit(780.0), Some(TitleBarHit::MenuButton));
        // The titles follow the buttons; the title gets the rest, up to the
        // menu button.
        assert_eq!(layout.menu_titles[0].x, 144.0);
        assert_eq!(layout.middle.x + layout.middle.width, 752.0);
    }

    #[test]
    fn test_without_window_buttons_the_system_frame_keeps_its_border_and_drag() {
        let layout = TitleBarLayout::new(
            WINDOW,
            METRICS,
            1.0,
            true,
            &menu_widths(),
            &[],
            WindowButtons::None,
        );
        let hit = |x: f32, y: f32| layout.hit(Point::new(x, y), false);
        assert_eq!(layout.close, None);
        assert_eq!(hit(2.0, 16.0), Some(TitleBarHit::MenuButton), "no border");
        assert_eq!(hit(60.0, 16.0), Some(TitleBarHit::MenuTitle(0)));
        assert_eq!(hit(780.0, 16.0), None, "the empty bar moves nothing");
        assert_eq!(layout.middle.x + layout.middle.width, 800.0);
    }

    #[test]
    fn test_the_height_laid_out_before_drawing_is_the_drawn_bar_s() {
        assert_eq!(TitleBar::height(METRICS, 1.0), layout().bar.height);
    }

    /// A menubar of `File`, `Edit`, and `View`.
    fn menus() -> Vec<String> {
        ["File", "Edit", "View"].map(String::from).to_vec()
    }

    /// The widths of [`menus`]'s titles in cells of [`METRICS`]: 4 cells
    /// each.
    fn menu_widths() -> Vec<f32> {
        vec![4.0 * METRICS.width; 3]
    }

    #[test]
    fn test_a_busy_tab_in_the_bar_keeps_the_loop_turning() {
        let mut strip = TabStrip::new();
        let tab = strip.push("build");
        let mut bar = TitleBar::new("t").with_tabs(strip);
        let mut window = TestWindow::new(800, 600);
        assert_eq!(bar.about_to_wait(&mut window.cx()), None, "nothing busy");
        bar.tabs_mut().expect("tabs").set_busy(tab, true);
        assert!(bar.about_to_wait(&mut window.cx()).is_some());
    }

    #[test]
    fn test_a_wide_title_in_the_proportional_font_fits_its_hit_rect() {
        let theme = Theme {
            chrome_text: ChromeText::Proportional,
            ..Theme::dark()
        };
        let title = "WWWWWWWWWW";
        let mut bar = TitleBar::new("t").with_menus(vec![title.to_owned()]);
        let mut frame = offscreen::plain_frame(800, 600);
        bar.draw(&mut frame, WINDOW, &theme);
        let width = label_width(&frame, &theme, title);
        let cells = text::width(title) as f32 * frame.cell_metrics().width;
        assert!(
            width > cells,
            "wider than its cells: {width} against {cells}"
        );
        let rect = bar.layout.expect("drawn").menu_titles[0];
        let x = rect.x + MENU_TITLE_PADDING_CELLS as f32 * frame.cell_metrics().width;
        assert!(x + width <= rect.x + rect.width, "{rect:?} holds {width}");
    }

    #[test]
    fn test_menu_titles_follow_the_menu_button_and_the_title_gets_the_rest() {
        let layout = TitleBarLayout::new(
            WINDOW,
            METRICS,
            1.0,
            true,
            &menu_widths(),
            &[],
            WindowButtons::Right,
        );
        // Each title is 4 cells and a cell of padding either side: 60 wide.
        let xs: Vec<(f32, f32)> = layout
            .menu_titles
            .iter()
            .map(|rect| (rect.x, rect.width))
            .collect();
        assert_eq!(xs, [(48.0, 60.0), (108.0, 60.0), (168.0, 60.0)]);
        assert_eq!(layout.middle, Rect::new(228.0, 0.0, 656.0 - 228.0, 32.0));
        let hit = |x: f32| layout.hit(Point::new(x, 16.0), false);
        assert_eq!(hit(110.0), Some(TitleBarHit::MenuTitle(1)));
        assert_eq!(hit(230.0), Some(TitleBarHit::Drag));
    }

    #[test]
    fn test_menu_titles_that_would_reach_the_window_buttons_are_left_out() {
        let narrow = Rect::new(0.0, 0.0, 300.0, 600.0);
        // The buttons start at 300 - 144 = 156: two 60 pixel titles fit.
        let layout = TitleBarLayout::new(
            narrow,
            METRICS,
            1.0,
            false,
            &menu_widths(),
            &[],
            WindowButtons::Right,
        );
        assert_eq!(layout.menu_titles.len(), 2);
        assert_eq!(layout.middle.x, 120.0);
    }

    #[test]
    fn test_a_title_opens_its_menu_and_the_open_menu_follows_the_pointer() {
        let mut window = TestWindow::new(800, 600);
        let mut bar = TitleBar::new("misty").with_menus(menus());
        bar.layout = Some(TitleBarLayout::new(
            WINDOW,
            METRICS,
            1.0,
            false,
            &menu_widths(),
            &[],
            WindowButtons::Right,
        ));
        let mut send = |input: Input, bar: &mut TitleBar| {
            window.send(&input, |input, cx| bar.handle(input, cx))
        };
        let moved = |x: f32| Input::PointerMoved(Point::new(x, 16.0));
        assert_eq!(
            send(moved(70.0), &mut bar),
            TitleBarResponse::Handled,
            "no menu open"
        );
        assert_eq!(
            send(left(true, 70.0, 16.0), &mut bar),
            TitleBarResponse::OpenMenuTitle(1)
        );
        bar.set_open_menu(Some(1));
        assert_eq!(
            send(moved(80.0), &mut bar),
            TitleBarResponse::Handled,
            "the same title"
        );
        assert_eq!(
            send(moved(130.0), &mut bar),
            TitleBarResponse::OpenMenuTitle(2)
        );
        bar.set_open_menu(Some(2));
        assert_eq!(
            send(moved(10.0), &mut bar),
            TitleBarResponse::OpenMenuTitle(0)
        );
        assert_eq!(bar.menu_title(0), Some(Rect::new(0.0, 0.0, 60.0, 32.0)));
    }

    #[test]
    fn test_tabs_in_the_bar_answer_for_themselves_and_their_empty_room_moves_the_window() {
        let mut window = TestWindow::new(800, 600);
        let mut tabs = TabStrip::new();
        let first = tabs.push("one");
        tabs.push("two");
        let mut bar = TitleBar::new("misty").with_tabs(tabs);
        bar.draw(&mut window.frame(), WINDOW, &Theme::dark());
        let strip_bar = bar
            .tabs()
            .and_then(|tabs| tabs.hit_test(Point::new(20.0, 16.0)));
        assert_eq!(strip_bar, Some(crate::TabStripHit::Tab(first)));
        let send = |window: &mut TestWindow, input: Input, bar: &mut TitleBar| {
            window.send(&input, |input, cx| bar.handle(input, cx))
        };
        assert_eq!(
            send(&mut window, left(true, 20.0, 16.0), &mut bar),
            TitleBarResponse::Tabs(TabStripResponse::Activated(first))
        );
        send(&mut window, left(false, 20.0, 16.0), &mut bar);
        send(&mut window, left(true, 500.0, 16.0), &mut bar);
        assert_eq!(window.take_actions(), [WindowAction::Drag], "past the tabs");
        send(&mut window, left(false, 500.0, 16.0), &mut bar);
        send(&mut window, left(true, 770.0, 16.0), &mut bar);
        assert_eq!(
            send(&mut window, left(false, 770.0, 16.0), &mut bar),
            TitleBarResponse::Close,
            "the window buttons stay the bar's"
        );
    }

    #[test]
    fn test_the_bar_describes_its_menu_titles_and_a_click_opens_one() {
        let id = FocusId::new("title-bar");
        let mut bar = TitleBar::new("notes").with_menus(menus());
        let mut window = TestWindow::new(800, 600);
        let mut frame = offscreen::plain_frame(800, 600);
        bar.draw(&mut frame, WINDOW, &Theme::dark());
        bar.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let edit = find_node(&update, id.child(FIRST_MENU_TITLE_NODE + 1)).expect("described");
        assert_eq!((edit.role(), edit.label()), (Role::MenuItem, Some("Edit")));
        let action = AccessAction::Click(id.child(FIRST_MENU_TITLE_NODE + 1));
        let answer = bar.access_action(id, action, &mut window.cx());
        assert_eq!(answer, TitleBarResponse::OpenMenuTitle(1));
    }

    #[test]
    fn test_a_title_bar_never_drawn_hits_nothing() {
        let title_bar = TitleBar::new("t");
        assert_eq!(title_bar.hit_test(Point::new(300.0, 10.0)), None);
    }

    #[test]
    fn test_the_bar_paints_surface_a_bottom_border_and_a_hovered_close_in_danger() {
        let theme = Theme::dark();
        let mut title_bar = TitleBar::new("t");
        title_bar.hovered = Some(TitleBarHit::Button(WindowButton::Close));
        let Some(snapshot) = offscreen::render(320, 80, |frame| {
            frame.clear(Rgba::from(theme.background));
            title_bar.draw(frame, Rect::new(0.0, 0.0, 320.0, 80.0), &theme);
        }) else {
            return;
        };
        let layout = title_bar.layout.clone().expect("drawn");
        let bar = layout.bar;
        snapshot.assert_pixel(1.0, 1.0, theme.surface, "the bar");
        snapshot.assert_pixel(1.0, bar.height - 1.0, theme.border, "the bottom border");
        snapshot.assert_pixel(1.0, bar.height, theme.background, "below the bar");
        let close = layout.close.expect("buttons");
        snapshot.assert_pixel(
            close.x + 1.0,
            close.y + 1.0,
            theme.danger,
            "the hovered close",
        );
        let minimize = layout.minimize.expect("buttons");
        snapshot.assert_pixel(
            minimize.x + 1.0,
            minimize.y + 1.0,
            theme.surface,
            "minimize",
        );
    }

    #[test]
    fn test_the_bar_describes_its_title_and_clickable_buttons() {
        let id = FocusId::new("title-bar");
        let mut bar = TitleBar::new("notes").with_menu_button();
        let mut frame = offscreen::plain_frame(800, 600);
        bar.draw(&mut frame, WINDOW, &Theme::dark());
        bar.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let node = |id| find_node(&update, id).expect("described");
        assert_eq!(
            (node(id).role(), node(id).label()),
            (Role::TitleBar, Some("notes"))
        );
        let labels: Vec<Option<&str>> = node(id)
            .children()
            .iter()
            .filter_map(|child| update.nodes.iter().find(|(node, _)| node == child))
            .map(|(_, node)| node.label())
            .collect();
        assert_eq!(
            labels,
            [
                Some("Menu"),
                Some("Minimize"),
                Some("Maximize"),
                Some("Close")
            ]
        );
        assert!(node(id.child(CLOSE_NODE)).supports_action(Action::Click));
    }

    #[test]
    fn test_the_buttons_take_their_names_from_the_theme_last_drawn_in() {
        let id = FocusId::new("title-bar");
        let mut bar = TitleBar::new("notes").with_menu_button();
        let theme = Theme {
            strings: Strings {
                close: "Schließen".into(),
                menu: "Menü".into(),
                ..Strings::english()
            },
            ..Theme::dark()
        };
        let mut frame = offscreen::plain_frame(800, 600);
        bar.draw(&mut frame, WINDOW, &theme);
        bar.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let label = |child| find_node(&update, id.child(child)).and_then(Node::label);
        assert_eq!(label(MENU_NODE), Some("Menü"));
        assert_eq!(label(CLOSE_NODE), Some("Schließen"));
        assert_eq!(
            label(MINIMIZE_NODE),
            Some("Minimize"),
            "the rest in English"
        );
    }

    /// A press, or a release when not `pressed`, of the left button at `x`, `y`.
    fn left(pressed: bool, x: f32, y: f32) -> Input {
        let press = ButtonInput::press(PointerButton::Left, Point::new(x, y));
        Input::PointerButton(ButtonInput { pressed, ..press })
    }

    /// A bar laid out as [`layout`] places it, as if drawn.
    fn laid_out() -> TitleBar {
        let mut bar = TitleBar::new("misty");
        bar.layout = Some(layout());
        bar
    }

    #[test]
    fn test_a_window_button_acts_only_when_released_over_the_button_pressed() {
        let mut window = TestWindow::new(800, 600);
        let mut bar = laid_out();
        let mut send = |input: Input, bar: &mut TitleBar| {
            window.send(&input, |input, cx| bar.handle(input, cx))
        };
        send(left(true, 770.0, 10.0), &mut bar);
        assert_eq!(
            send(left(false, 770.0, 10.0), &mut bar),
            TitleBarResponse::Close
        );
        send(left(true, 770.0, 10.0), &mut bar);
        assert_eq!(
            send(left(false, 400.0, 10.0), &mut bar),
            TitleBarResponse::Handled,
            "released off the button"
        );
    }

    #[test]
    fn test_the_drag_region_moves_the_window_and_a_double_click_maximizes_it() {
        let mut window = TestWindow::new(800, 600);
        let mut bar = laid_out();
        window.send(&left(true, 300.0, 16.0), |input, cx| bar.handle(input, cx));
        assert_eq!(window.take_actions(), [WindowAction::Drag]);
        window.send(&left(false, 300.0, 16.0), |input, cx| bar.handle(input, cx));
        window.send(&left(true, 300.0, 16.0), |input, cx| bar.handle(input, cx));
        assert_eq!(window.take_actions(), [WindowAction::ToggleMaximize]);
        assert!(bar.maximized, "shown at once");
    }

    #[test]
    fn test_a_press_on_the_border_resizes_and_hovering_it_shows_the_resize_pointer() {
        let mut window = TestWindow::new(800, 600);
        let mut bar = laid_out();
        let corner = Input::PointerMoved(Point::new(1.0, 1.0));
        window.send(&corner, |input, cx| bar.handle(input, cx));
        assert_eq!(window.cursor(), Some(ResizeEdge::NorthWest.cursor()));
        window.send(&left(true, 1.0, 1.0), |input, cx| bar.handle(input, cx));
        assert_eq!(
            window.take_actions(),
            [WindowAction::Resize(ResizeEdge::NorthWest)]
        );
    }

    #[test]
    fn test_app_buttons_sit_beside_the_menu_button_and_take_from_the_title_s_room() {
        // An icon alone is a window button wide (48); a 60 wide label gets
        // a cell of padding at each side (80).
        let layout = TitleBarLayout::new(
            WINDOW,
            METRICS,
            1.0,
            true,
            &[],
            &[0.0, 60.0],
            WindowButtons::Left,
        );
        assert_eq!(layout.app_buttons[0], Rect::new(624.0, 0.0, 48.0, 32.0));
        assert_eq!(layout.app_buttons[1], Rect::new(672.0, 0.0, 80.0, 32.0));
        assert_eq!(
            layout.menu.map(|menu| menu.x),
            Some(752.0),
            "the menu stays outermost"
        );
        assert_eq!(layout.middle.x + layout.middle.width, 624.0);
        assert_eq!(
            layout.hit(Point::new(700.0, 16.0), true),
            Some(TitleBarHit::AppButton(1))
        );
        let right = TitleBarLayout::new(
            WINDOW,
            METRICS,
            1.0,
            true,
            &[],
            &[0.0],
            WindowButtons::Right,
        );
        assert_eq!(
            right.app_buttons[0].x + right.app_buttons[0].width,
            right.minimize.expect("buttons").x,
            "beside the window buttons when they sit at the right"
        );
    }

    #[test]
    fn test_app_buttons_that_would_reach_the_other_side_are_left_out_from_the_last() {
        let narrow = Rect::new(0.0, 0.0, 300.0, 600.0);
        let layout = TitleBarLayout::new(
            narrow,
            METRICS,
            1.0,
            false,
            &[],
            &[0.0; 4],
            WindowButtons::Right,
        );
        // The window buttons take 144 of 300: three 48 wide buttons fit.
        assert_eq!(layout.app_buttons.len(), 3);
        assert_eq!(layout.app_buttons[0].x, 12.0);
        assert_eq!(layout.app_buttons[2].x + 48.0, 156.0);
    }

    #[test]
    fn test_an_app_button_answers_when_released_over_it_and_describes_itself() {
        let id = FocusId::new("title-bar");
        let mut bar = TitleBar::new("notes").with_app_buttons(vec![
            TitleBarButton::label("Sync").with_tooltip("Review and sync"),
            TitleBarButton::label("Panel"),
        ]);
        let mut frame = offscreen::plain_frame(800, 600);
        bar.draw(&mut frame, WINDOW, &Theme::dark());
        bar.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let sync = find_node(&update, id.child(FIRST_APP_BUTTON_NODE)).expect("described");
        assert_eq!((sync.role(), sync.label()), (Role::Button, Some("Sync")));
        assert_eq!(sync.description(), Some("Review and sync"));
        let mut window = TestWindow::new(800, 600);
        let rect = bar.app_button(1).expect("drawn");
        let (x, y) = (rect.x + 10.0, rect.y + 16.0);
        let moved = Input::PointerMoved(Point::new(x, y));
        window.send(&moved, |input, cx| bar.handle(input, cx));
        assert_eq!(bar.tooltip_hover(), None, "no tooltip of its own");
        window.send(&left(true, x, y), |input, cx| bar.handle(input, cx));
        let released = window.send(&left(false, x, y), |input, cx| bar.handle(input, cx));
        assert_eq!(released, TitleBarResponse::AppButton(1));
        window.send(&left(true, x, y), |input, cx| bar.handle(input, cx));
        let off = window.send(&left(false, 300.0, 10.0), |input, cx| bar.handle(input, cx));
        assert_eq!(off, TitleBarResponse::Handled, "released off the button");
        let sync_rect = bar.app_button(0).expect("drawn");
        let over_sync = Input::PointerMoved(Point::new(sync_rect.x + 10.0, 16.0));
        window.send(&over_sync, |input, cx| bar.handle(input, cx));
        assert_eq!(
            bar.tooltip_hover(),
            Some((sync_rect, "Review and sync".to_owned()))
        );
        let click = AccessAction::Click(id.child(FIRST_APP_BUTTON_NODE));
        let answer = bar.access_action(id, click, &mut window.cx());
        assert_eq!(answer, TitleBarResponse::AppButton(0));
    }
}

//! The chrome around a window's content as elements: [`title_bar`], the
//! bar a frameless window draws in place of the system's; [`tab_strip`],
//! a row of the app's [`tab`]s; [`split`], two elements sharing an area
//! beside a divider the user drags; and [`status_bar`], the line along
//! the window's bottom. Each wraps the chrome widget of the same name,
//! keeping its view state (the hover, a tab or a divider being dragged,
//! the window buttons pressed) in misty's store, while what it shows
//! comes from the app's state, and what the user does reaches the app
//! through the callbacks it was given.

use std::any::Any;
use std::rc::Rc;
use std::time::Instant;

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{KeyCode, Mods};
use super::super::base::notice::Notifier;
use super::super::base::spinner;
use super::super::base::split::{Axis, SplitId, SplitTree};
use super::super::base::theme::Theme;
use super::super::chrome::split_view::{SplitView, SplitViewResponse};
use super::super::chrome::status_bar::{self, StatusBarResponse, StatusLine};
use super::super::chrome::tab_strip::{self, TabDrag, TabId, TabStripHit, TabStripResponse};
use super::super::chrome::title_bar::{
    self, FIRST_APP_BUTTON_NODE, TitleAlign, TitleBarButton, TitleBarResponse, WindowButtons,
};
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::{logical, outline};
use super::super::render::image::IconSet;
use super::super::widgets::drag::Dropped;
use super::controls::{Callback, call};
use super::element::{Element, Kind, MenuRequest, SplitParts};
use super::node::{Call, ControlDrag, Drawn, MovingDrag, Spec};
use super::props::Payload;

// ========================================================================
// Constants
// ========================================================================

/// The child of a title bar's id its menu button is described as, by
/// [`TitleBar::access`](title_bar::TitleBar::access)'s numbering.
const MENU_NODE: u64 = 0;

/// The child of a title bar's id its close button is described as.
const CLOSE_NODE: u64 = 3;

/// The child of a title bar's id its first menubar title is described as;
/// the others count up from it.
const FIRST_MENU_TITLE_NODE: u64 = 5;

/// The thickness of a split's divider, in logical pixels, as
/// [`SplitView`] draws it: what a drag's ratio is measured against.
const DIVIDER_THICKNESS: f32 = 1.0;

/// The width of the outline around the tab the keyboard is on while it is
/// not the active tab, in logical pixels: the focus ring's.
const TAB_CURSOR_STROKE: f32 = 2.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The bar a frameless window draws in place of the system's, as an
/// element: its title, the window buttons, the drag region and resize
/// border, a menu button and menubar titles that ask the app for its
/// menus, and buttons of the app's own. Made by [`title_bar`].
pub struct TitleBar<'a, A> {
    /// The app's buttons, left to right, and what each runs.
    app_buttons: Vec<(TitleBarButton, Callback<A, ()>)>,
    /// Where the window buttons go.
    buttons: WindowButtons,
    /// The menubar's titles, left to right.
    menus: &'a [&'a str],
    /// What the close button runs, or `None` to close the window.
    on_close: Option<Callback<A, ()>>,
    /// What the menu button runs; without one the bar has none.
    on_menu: Option<Callback<A, MenuRequest>>,
    /// What a menubar title runs, with its index.
    on_menu_title: Option<Callback<A, (usize, MenuRequest)>>,
    /// The menubar title whose menu the app has open.
    open_menu: Option<usize>,
    /// The text shown in the bar.
    title: &'a str,
    /// Where the title sits, or `None` for the platform's place.
    title_align: Option<TitleAlign>,
}

/// One tab of a [`tab_strip`], from the app's state: the key the app knows
/// it by, its title, and its marks. Made by [`tab`].
#[derive(Clone, Debug, PartialEq)]
pub struct Tab<'a, K> {
    /// Whether its work is under way, which shows a spinner.
    busy: bool,
    /// Whether it can be closed from the strip.
    closable: bool,
    /// The name of its icon in the strip's [`TabStrip::icons`], if any.
    icon: Option<&'a str>,
    /// What the app knows it by.
    key: K,
    /// Whether it holds unsaved changes, which shows a dot.
    modified: bool,
    /// What it says.
    title: &'a str,
}

/// A tab dragged off a [`tab_strip`] element, as the payload an
/// [`Element::drop_target`] takes: the key the app gave it. Another
/// strip, or a pane taking a tab to split it, accepts a `DraggedTab<K>`
/// of its tabs' key type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraggedTab<K> {
    /// The key the app gave the tab.
    pub key: K,
}

/// A row of the app's tabs, as an element. Made by [`tab_strip`].
pub struct TabStrip<'a, A, K> {
    /// The key of the tab the app shows, if any.
    active: Option<K>,
    /// Whether the strip's pane has the keyboard, which marks its active
    /// tab with the accent.
    focused: bool,
    /// The strip's height as a multiple of the cell height, or `None` for
    /// the default.
    height_ratio: Option<f32>,
    /// The icons its tabs name, if they name any.
    icons: Option<&'a IconSet>,
    /// What clicking a tab runs, with its key.
    on_activate: Option<Callback<A, K>>,
    /// What a tab's close button or a middle click runs, with its key.
    on_close: Option<Callback<A, K>>,
    /// What a right click on a tab runs, with its key.
    on_context_menu: Option<Callback<A, (K, MenuRequest)>>,
    /// What dropping a dragged tab in a new place runs, with its key and
    /// new place.
    on_move: Option<Callback<A, (K, usize)>>,
    /// What the new-tab button runs.
    on_new: Option<Callback<A, ()>>,
    /// What dropping a tab dragged off another strip runs, with its key
    /// and the place it lands at.
    on_tab_drop: Option<Callback<A, (K, usize)>>,
    /// The tabs, left to right.
    tabs: Vec<Tab<'a, K>>,
}

/// Two elements side by side or one above the other, a divider between
/// them that the pointer drags. Made by [`split`]; it becomes an
/// [`Element`] with `.into()`, or where a function takes one.
pub struct Split<'a, A> {
    /// The two elements and how they share the area.
    parts: SplitParts<'a, A>,
}

/// The line along a window's bottom, as an element: the app's
/// [`StatusLine`] and the messages and tasks of its notifier. Made by
/// [`status_bar`].
pub struct StatusBar<'a, A> {
    /// What the bar shows.
    line: StatusLine,
    /// The notifier whose newest message and tasks the bar shows, if any.
    notifier: Option<&'a Notifier>,
    /// What a click on a segment with an id runs, with the id.
    on_click: Option<Callback<A, &'static str>>,
    /// What a click on the tasks' item runs.
    on_progress: Option<Callback<A, ()>>,
}

/// The widget kept for a [`TitleBar`]: misty's bar, what it was built
/// with, and whether its buttons have the keyboard and which of them.
pub(crate) struct TitleBox {
    /// Where the bar was last drawn: its row, down to the window's bottom,
    /// along whose edges its resize border runs.
    area: Rect,
    /// The bar.
    bar: title_bar::TitleBar,
    /// What the bar was built with, to build it again when that changes.
    built: TitleBuild,
    /// Whether its buttons have the keyboard.
    focused: bool,
    /// The button the keyboard is on, by its place among the bar's
    /// buttons left to right.
    stop: usize,
    /// The title it shows.
    title: String,
}

/// A button of a title bar the keyboard reaches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TitleStop {
    /// The app's button at this index.
    App(usize),
    /// The menu button.
    Menu,
}

/// What a [`TitleBox`]'s bar is built with, rather than told later.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TitleBuild {
    /// Where the window buttons go.
    buttons: WindowButtons,
    /// Whether it has a menu button.
    has_menu_button: bool,
    /// The menubar's titles.
    menus: Vec<String>,
    /// Where the title sits.
    title_align: Option<TitleAlign>,
}

/// What the app's state says a [`TitleBox`] shows.
pub(crate) struct TitleProps<'a> {
    /// The app's buttons.
    app_buttons: Vec<TitleBarButton>,
    /// What the bar is built with.
    built: TitleBuild,
    /// The menubar title whose menu is open.
    open_menu: Option<usize>,
    /// The title.
    title: &'a str,
}

/// What a title bar answered an input with.
pub(crate) enum TitleReply {
    /// What the bar answered.
    Bar(TitleBarResponse),
    /// Enter or Space on its focused menu button.
    MenuKey,
}

/// What the user did with a title bar that reaches the app.
pub(crate) enum TitleAnswer {
    /// Clicked the app's button at this index.
    AppButton(usize),
    /// Clicked the close button.
    Close,
    /// Asked for the menu button's menu.
    Menu(MenuRequest),
    /// Asked for the menu of the menubar title at this index.
    MenuTitle(usize, MenuRequest),
}

/// The widget kept for a [`TabStrip`]: misty's strip, the strip's id of
/// each of the app's tabs, and the tab the keyboard is on.
pub(crate) struct TabsBox<K> {
    /// The tab Left and Right last moved the keyboard to, if it is still
    /// open; else the keyboard is on the active tab.
    cursor: Option<TabId>,
    /// Whether the strip has the keyboard.
    focused: bool,
    /// Whether the strip has taken the app's icons, which it does once.
    has_icons: bool,
    /// Each app tab's key and the strip's id for it, in no order.
    keys: Vec<(K, TabId)>,
    /// The strip.
    strip: tab_strip::TabStrip,
}

/// What the app's state says a [`TabsBox`] shows.
pub(crate) struct TabsProps<'a, K> {
    /// The key of the active tab.
    active: Option<&'a K>,
    /// Whether the strip's pane has the keyboard.
    focused: bool,
    /// The strip's height as a multiple of the cell height.
    height_ratio: Option<f32>,
    /// The icons its tabs name.
    icons: Option<&'a IconSet>,
    /// The tabs, left to right.
    tabs: &'a [Tab<'a, K>],
}

/// What a tab strip answered an input with.
pub(crate) enum TabsReply {
    /// A right press on this tab, at this point.
    Context(TabId, Point),
    /// What the strip answered.
    Strip(TabStripResponse),
}

/// What the user did with a tab strip that reaches the app, naming tabs
/// by the app's keys.
pub(crate) enum TabEvent<K> {
    /// Clicked this tab.
    Activate(K),
    /// Asked to close this tab.
    Close(K),
    /// Asked for this tab's context menu.
    Context(K, MenuRequest),
    /// Dropped this tab at this place.
    Move(K, usize),
    /// Clicked the new-tab button.
    New,
}

/// The widget kept for a [`StatusBar`]: misty's bar, and when the tasks
/// it last showed started, for their spinner.
pub(crate) struct StatusBox {
    /// The bar.
    bar: status_bar::StatusBar,
    /// The notifier a bar the app gave none shows: always empty.
    empty: Notifier,
    /// When the oldest task shown last started, or `None` while none ran.
    task_started: Option<Instant>,
}

/// The widget kept for a [`Split`]: misty's split view over a tree of one
/// split, and what a drag of its divider needs.
pub(crate) struct SplitBox {
    /// Where the split was last drawn.
    area: Rect,
    /// How its two sides are placed.
    axis: Axis,
    /// Whether the pointer holds its divider.
    held: bool,
    /// Physical pixels per logical pixel in the last frame drawn.
    scale_factor: f64,
    /// Its one split.
    split: Option<SplitId>,
    /// The tree of two panes the view lays out.
    tree: SplitTree,
    /// The divider's hover and drag, and its drawing.
    view: SplitView,
}

// ========================================================================
// TitleBar Implementations
// ========================================================================

impl<'a, A> TitleBar<'a, A> {
    /// This bar with its window buttons where `buttons` says: at the right
    /// (the default), at the left, or none, for a window the system
    /// decorates.
    pub fn window_buttons(self, buttons: WindowButtons) -> Self {
        Self { buttons, ..self }
    }

    /// This bar with its title set where `align` says, rather than where
    /// the platform sets titles.
    pub fn title_align(self, align: TitleAlign) -> Self {
        Self {
            title_align: Some(align),
            ..self
        }
    }

    /// This bar with a menubar of `titles` after its menu button, left to
    /// right; a press on one runs [`TitleBar::on_menu_title`].
    pub fn menus(self, titles: &'a [&'a str]) -> Self {
        Self {
            menus: titles,
            ..self
        }
    }

    /// Lights the menubar title at `index` while the app shows its menu;
    /// `None` (the default) lights none.
    pub fn open_menu(self, index: Option<usize>) -> Self {
        Self {
            open_menu: index,
            ..self
        }
    }

    /// This bar with a menu button, which runs `apply` with where its menu
    /// opens when clicked, or on Enter or Space while it has the keyboard:
    /// the app keeps the [`MenuRequest`] and shows its menu there with
    /// [`context_menu`](crate::view::context_menu). The menu button is a
    /// tab stop, visited after the bar's siblings unless the app gives the
    /// bar another [`Element::order`].
    pub fn on_menu(self, apply: impl Fn(&mut A, MenuRequest) + 'static) -> Self {
        Self {
            on_menu: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs, with the title's index and where its menu opens, when the
    /// user presses a menubar title.
    pub fn on_menu_title(self, apply: impl Fn(&mut A, (usize, MenuRequest)) + 'static) -> Self {
        Self {
            on_menu_title: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This bar with `button`, one of the app's own, after those added
    /// before it, at the end of the room beside the menu button (beside
    /// the window buttons when those sit at the right); `apply` runs when
    /// it is clicked, clicked by a screen reader, or pressed with Enter or
    /// Space while it has the keyboard. The bar's buttons are one tab
    /// stop, as the menu button alone is: Left and Right move between
    /// them, and a button's tooltip shows while the pointer rests on it.
    pub fn app_button(mut self, button: TitleBarButton, apply: impl Fn(&mut A) + 'static) -> Self {
        let apply: Callback<A, ()> = Rc::new(move |app: &mut A, ()| apply(app));
        self.app_buttons.push((button, apply));
        self
    }

    /// What runs when the user clicks the close button, which otherwise
    /// closes the window: ask first, or keep the app running.
    pub fn on_close(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_close: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }

    /// What the bar is built with.
    fn build(&self) -> TitleBuild {
        TitleBuild {
            buttons: self.buttons,
            has_menu_button: self.on_menu.is_some(),
            menus: self.menus.iter().map(|title| (*title).to_owned()).collect(),
            title_align: self.title_align,
        }
    }

    /// What the app's buttons show.
    fn app_button_faces(&self) -> Vec<TitleBarButton> {
        self.app_buttons
            .iter()
            .map(|(button, _)| button.clone())
            .collect()
    }
}

impl<A: 'static> Spec<A> for TitleBar<'_, A> {
    type Widget = TitleBox;

    const KIND: &'static str = "title bar";

    fn create(&self) -> TitleBox {
        let built = self.build();
        TitleBox {
            area: Rect::default(),
            bar: built
                .bar(self.title)
                .with_app_buttons(self.app_button_faces()),
            built,
            focused: false,
            stop: 0,
            title: self.title.to_owned(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> TitleProps<'b> {
        TitleProps {
            app_buttons: self.app_button_faces(),
            built: self.build(),
            open_menu: self.open_menu,
            title: self.title,
        }
    }

    fn call(&self, answer: TitleAnswer) -> Option<Call<A>> {
        match answer {
            TitleAnswer::AppButton(index) => {
                call(self.app_buttons.get(index).map(|(_, apply)| apply), ())
            }
            TitleAnswer::Close => call(self.on_close.as_ref(), ()),
            TitleAnswer::Menu(request) => call(self.on_menu.as_ref(), request),
            TitleAnswer::MenuTitle(index, request) => {
                call(self.on_menu_title.as_ref(), (index, request))
            }
        }
    }

    /// The close button closes the window when the app gave it nothing to
    /// run.
    fn asks_close(&self, answer: &TitleAnswer) -> bool {
        matches!(answer, TitleAnswer::Close) && self.on_close.is_none()
    }

    /// Draws the bar along the top of `rect`, its resize border along the
    /// edges of `rect` stretched to the window's bottom; it takes the
    /// pointer there over everything else, the keyboard on the button
    /// the keyboard is on.
    fn draw(&self, widget: &mut TitleBox, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        let bottom = frame.bounds().y + frame.bounds().height;
        let area = Rect::new(rect.x, rect.y, rect.width, (bottom - rect.y).max(0.0));
        widget.area = area;
        widget.bar.draw(frame, area, theme);
        Drawn {
            hit: widget.stop_rect().unwrap_or_default(),
            layer: None,
            over: Some(area),
        }
    }

    fn tips(&self, widget: &TitleBox) -> Vec<(Rect, String)> {
        widget.bar.app_button_tips()
    }

    fn describe(&self, widget: &TitleBox, frame: &mut Frame, id: FocusId) {
        widget.bar.access(frame, id);
    }

    /// A screen reader's click on the menu button or a menubar title asks
    /// for its menu, and one on the close button closes, as the pointer's
    /// do; minimizing and maximizing are left to the system's own window
    /// actions, since a screen reader's action reaches no window here.
    fn act(&self, _widget: &mut TitleBox, id: FocusId, action: AccessAction) -> Option<TitleReply> {
        let AccessAction::Click(target) = action else {
            return None;
        };
        let title = (0..self.menus.len())
            .find(|index| target == id.child(FIRST_MENU_TITLE_NODE + *index as u64));
        let app_button = (0..self.app_buttons.len())
            .find(|index| target == id.child(FIRST_APP_BUTTON_NODE + *index as u64));
        let response = match (title, app_button) {
            (Some(index), _) => TitleBarResponse::OpenMenuTitle(index),
            (None, Some(index)) => TitleBarResponse::AppButton(index),
            (None, None) if target == id.child(MENU_NODE) && self.on_menu.is_some() => {
                TitleBarResponse::OpenMenu
            }
            (None, None) if target == id.child(CLOSE_NODE) => TitleBarResponse::Close,
            (None, None) => TitleBarResponse::Ignored,
        };
        Some(TitleReply::Bar(response))
    }

    fn is_focusable(&self) -> bool {
        self.on_menu.is_some() || !self.app_buttons.is_empty()
    }
}

impl<'a, A: 'static> From<TitleBar<'a, A>> for Element<'a, A> {
    /// The bar as an element visited by Tab after its siblings, so a
    /// window's first tab stop is its content rather than the menu button.
    fn from(bar: TitleBar<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(bar))).order(1)
    }
}

// ========================================================================
// TitleBuild Implementations
// ========================================================================

impl TitleBuild {
    /// A bar showing `title`, built as this says.
    fn bar(&self, title: &str) -> title_bar::TitleBar {
        let bar = title_bar::TitleBar::new(title)
            .with_window_buttons(self.buttons)
            .with_menus(self.menus.clone());
        let bar = match self.title_align {
            Some(align) => bar.with_title_align(align),
            None => bar,
        };
        match self.has_menu_button {
            true => bar.with_menu_button(),
            false => bar,
        }
    }
}

// ========================================================================
// TitleBox Implementations
// ========================================================================

impl Widget for TitleBox {
    type Output = ();
    type Response = TitleReply;

    /// While its buttons have the keyboard, Enter or Space presses the one
    /// the keyboard is on (the menu button asks for its menu) and Left and
    /// Right move between them, wrapping; everything else goes to the bar:
    /// the pointer over its parts and its resize border, and the window's
    /// focus.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TitleReply {
        let key = input
            .key()
            .filter(|key| self.focused && key.mods == Mods::NONE);
        let stops = self.stops();
        let step = match key.map(|key| key.code) {
            Some(KeyCode::Enter | KeyCode::Char(' ')) => {
                return match stops.get(self.stop.min(stops.len().saturating_sub(1))) {
                    Some(TitleStop::Menu) => TitleReply::MenuKey,
                    Some(TitleStop::App(index)) => {
                        TitleReply::Bar(TitleBarResponse::AppButton(*index))
                    }
                    None => TitleReply::Bar(self.bar.handle(input, cx)),
                };
            }
            Some(KeyCode::Left) if stops.len() > 1 => stops.len() - 1,
            Some(KeyCode::Right) if stops.len() > 1 => 1,
            _ => return TitleReply::Bar(self.bar.handle(input, cx)),
        };
        self.stop = (self.stop.min(stops.len() - 1) + step) % stops.len();
        cx.request_redraw();
        TitleReply::Bar(TitleBarResponse::Handled)
    }

    /// Nothing: the [`TitleBar`] element draws the bar in the area it
    /// stretches to the window's bottom.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Control for TitleBox {
    type Props<'a> = TitleProps<'a>;
    type Value = TitleAnswer;

    /// Builds the bar again when its buttons, menu button, menubar, or
    /// title's place changed (forgetting its hover), and takes the title,
    /// the app's buttons, and the open menu.
    fn sync(&mut self, props: TitleProps<'_>) {
        if props.built != self.built {
            self.bar = props.built.bar(props.title);
            self.built = props.built;
            self.title = props.title.to_owned();
        }
        self.bar.set_app_buttons(props.app_buttons);
        if self.title != props.title {
            self.title = props.title.to_owned();
            self.bar.set_title(props.title);
        }
        self.bar.set_open_menu(props.open_menu);
    }

    fn measure(&self, _width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        title_bar::TitleBar::height(metrics, scale_factor)
    }

    /// The close button's click, and a menu asked for, opening under the
    /// part that asked.
    fn changed(&self, reply: TitleReply) -> Option<TitleAnswer> {
        let below = |anchor: Option<Rect>, by_key: bool| MenuRequest {
            anchor: anchor.unwrap_or_default(),
            by_key,
        };
        match reply {
            TitleReply::MenuKey => Some(TitleAnswer::Menu(below(self.bar.menu_button(), true))),
            TitleReply::Bar(TitleBarResponse::AppButton(index)) => {
                Some(TitleAnswer::AppButton(index))
            }
            TitleReply::Bar(TitleBarResponse::Close) => Some(TitleAnswer::Close),
            TitleReply::Bar(TitleBarResponse::OpenMenu) => {
                Some(TitleAnswer::Menu(below(self.bar.menu_button(), false)))
            }
            TitleReply::Bar(TitleBarResponse::OpenMenuTitle(index)) => Some(
                TitleAnswer::MenuTitle(index, below(self.bar.menu_title(index), false)),
            ),
            TitleReply::Bar(_) => None,
        }
    }
}

impl TitleBox {
    /// The bar's buttons the keyboard reaches, left to right as last
    /// drawn: the app's buttons that showed, and the menu button.
    fn stops(&self) -> Vec<TitleStop> {
        let apps =
            (0..).map_while(|index| Some((self.bar.app_button(index)?, TitleStop::App(index))));
        let menu = self.bar.menu_button().map(|rect| (rect, TitleStop::Menu));
        let mut stops: Vec<(Rect, TitleStop)> = apps.chain(menu).collect();
        stops.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        stops.into_iter().map(|(_, stop)| stop).collect()
    }

    /// Where the button the keyboard is on was last drawn.
    fn stop_rect(&self) -> Option<Rect> {
        let stops = self.stops();
        match stops.get(self.stop.min(stops.len().checked_sub(1)?))? {
            TitleStop::App(index) => self.bar.app_button(*index),
            TitleStop::Menu => self.bar.menu_button(),
        }
    }
}

impl Outcome for TitleReply {
    fn is_ignored(&self) -> bool {
        match self {
            Self::Bar(response) => response.is_ignored(),
            Self::MenuKey => false,
        }
    }
}

// ========================================================================
// Tab Implementations
// ========================================================================

impl<'a, K> Tab<'a, K> {
    /// This tab drawn with the icon named `name` in the strip's
    /// [`TabStrip::icons`], before its title.
    pub fn icon(self, name: &'a str) -> Self {
        Self {
            icon: Some(name),
            ..self
        }
    }

    /// This tab marked as holding unsaved changes: a dot in place of its
    /// close button until the pointer is over it.
    pub fn modified(self, modified: bool) -> Self {
        Self { modified, ..self }
    }

    /// This tab marked as busy: a spinner before its title while its work
    /// is under way.
    pub fn busy(self, busy: bool) -> Self {
        Self { busy, ..self }
    }

    /// This tab closable from the strip (`true`, the default), or not: a
    /// pinned tab shows no close button and a middle click leaves it open.
    pub fn closable(self, closable: bool) -> Self {
        Self { closable, ..self }
    }
}

// ========================================================================
// TabStrip Implementations
// ========================================================================

impl<'a, A, K> TabStrip<'a, A, K> {
    /// This strip drawing the icons its tabs name ([`Tab::icon`]) from
    /// `icons`, taken the first time the strip is drawn.
    pub fn icons(self, icons: &'a IconSet) -> Self {
        Self {
            icons: Some(icons),
            ..self
        }
    }

    /// This strip in a pane that has the keyboard (`true`, the default),
    /// its active tab marked with the accent, or in one that has not, as
    /// one of several editor groups.
    pub fn focused(self, focused: bool) -> Self {
        Self { focused, ..self }
    }

    /// This strip `ratio` times the cell height tall (at least once),
    /// rather than misty's default, for an app that wants roomier tabs.
    pub fn height_ratio(self, ratio: f32) -> Self {
        Self {
            height_ratio: Some(ratio),
            ..self
        }
    }

    /// What runs on the app's state, with the tab's key, when the user
    /// clicks a tab: show it. Without one the active tab stays the app's.
    pub fn on_activate(self, apply: impl Fn(&mut A, K) + 'static) -> Self {
        Self {
            on_activate: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs, with the tab's key, when the user clicks a tab's close
    /// button or middle-clicks it: close it, or ask first.
    pub fn on_close(self, apply: impl Fn(&mut A, K) + 'static) -> Self {
        Self {
            on_close: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs, with the tab's key and the place among this strip's
    /// tabs it lands at, when the user drops on this strip a tab dragged
    /// off another one ([`DraggedTab`] of the same key type): move it here
    /// in the app's tabs. While such a drag is over the strip a marker
    /// shows the gap it would land in. Without one the strip takes no
    /// such drop, and an element's [`Element::drop_target`] around it may.
    pub fn on_tab_drop(self, apply: impl Fn(&mut A, (K, usize)) + 'static) -> Self {
        Self {
            on_tab_drop: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs, with the tab's key and its new place among the tabs, when
    /// the user drops a tab dragged along the strip: move it there in the
    /// app's tabs. Without one a dragged tab goes back.
    pub fn on_move(self, apply: impl Fn(&mut A, (K, usize)) + 'static) -> Self {
        Self {
            on_move: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs when the user clicks the new-tab button: add a tab and
    /// show it.
    pub fn on_new(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_new: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }

    /// What runs, with the tab's key and where its menu opens, when the user
    /// right-clicks a tab; the app shows its menu there with
    /// [`context_menu`](crate::view::context_menu).
    pub fn on_context_menu(self, apply: impl Fn(&mut A, (K, MenuRequest)) + 'static) -> Self {
        Self {
            on_context_menu: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static, K: Clone + PartialEq + 'static> Spec<A> for TabStrip<'_, A, K> {
    type Widget = TabsBox<K>;

    /// A tab dragged off the strip is a drag between elements carrying a
    /// [`DraggedTab`] with its key, labelled with its title: an element
    /// whose [`Element::drop_target`] takes one is marked under it and
    /// gets it where it is dropped. Back on the strip, or cancelled, the
    /// drag ends.
    fn takes_drops(&self) -> bool {
        self.on_tab_drop.is_some()
    }

    fn takes_drop(&self, payload: &dyn Any) -> bool {
        payload.downcast_ref::<DraggedTab<K>>().is_some()
    }

    /// The strip's own marker, at the gap the tab would land in.
    fn mark_drop(&self, widget: &mut TabsBox<K>, point: Option<Point>) {
        widget.strip.show_drop_marker(point);
    }

    fn drop_call(&self, widget: &TabsBox<K>, payload: &dyn Any, point: Point) -> Option<Call<A>> {
        let dragged = payload.downcast_ref::<DraggedTab<K>>()?;
        let index = widget.strip.drop_index(point)?;
        call(self.on_tab_drop.as_ref(), (dragged.key.clone(), index))
    }

    fn dragged(&self, widget: &TabsBox<K>, response: &TabsReply) -> Option<ControlDrag> {
        let carried = |drag: &TabDrag| {
            let key = widget.key(drag.tab)?.clone();
            let payload: Payload = Rc::new(DraggedTab { key });
            Some((payload, drag.position))
        };
        match response {
            TabsReply::Strip(TabStripResponse::Dragging(drag)) => {
                let (payload, position) = carried(drag)?;
                let label = widget.strip.title(drag.tab).unwrap_or_default().to_owned();
                Some(ControlDrag::Moving(MovingDrag {
                    label,
                    payload,
                    position,
                }))
            }
            TabsReply::Strip(TabStripResponse::DroppedOutside(drag)) => {
                let (payload, position) = carried(drag)?;
                Some(ControlDrag::Dropped(Dropped { payload, position }))
            }
            TabsReply::Context(..) | TabsReply::Strip(_) => Some(ControlDrag::Ended),
        }
    }

    const KIND: &'static str = "tab strip";

    fn create(&self) -> TabsBox<K> {
        TabsBox {
            cursor: None,
            focused: false,
            has_icons: false,
            keys: Vec::new(),
            strip: tab_strip::TabStrip::new(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> TabsProps<'b, K> {
        TabsProps {
            active: self.active.as_ref(),
            focused: self.focused,
            height_ratio: self.height_ratio,
            icons: self.icons,
            tabs: &self.tabs,
        }
    }

    fn call(&self, event: TabEvent<K>) -> Option<Call<A>> {
        match event {
            TabEvent::Activate(key) => call(self.on_activate.as_ref(), key),
            TabEvent::Close(key) => call(self.on_close.as_ref(), key),
            TabEvent::Context(key, request) => call(self.on_context_menu.as_ref(), (key, request)),
            TabEvent::Move(key, index) => call(self.on_move.as_ref(), (key, index)),
            TabEvent::New => call(self.on_new.as_ref(), ()),
        }
    }

    /// Draws the strip, outlining the tab the keyboard is on while the
    /// strip has the keyboard and that tab is not the active one; the
    /// focus ring goes around the whole strip, as around a list.
    fn draw(&self, widget: &mut TabsBox<K>, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.strip.draw(frame, rect, theme);
        let marked = widget
            .cursor()
            .filter(|cursor| widget.focused && Some(*cursor) != widget.strip.active())
            .and_then(|cursor| widget.strip.tab_rect(cursor));
        if let Some(tab) = marked {
            let stroke = logical(TAB_CURSOR_STROKE, frame.scale_factor());
            for edge in outline(tab, stroke) {
                frame.fill_rect(edge, theme.accent);
            }
        }
        rect.into()
    }

    /// Each tab whose title the last frame cut short, its title whole.
    fn tips(&self, widget: &TabsBox<K>) -> Vec<(Rect, String)> {
        widget
            .keys
            .iter()
            .filter(|(_, id)| widget.strip.title_cut(*id))
            .filter_map(|(_, id)| {
                Some((
                    widget.strip.tab_rect(*id)?,
                    widget.strip.title(*id)?.to_owned(),
                ))
            })
            .collect()
    }

    fn describe(&self, widget: &TabsBox<K>, frame: &mut Frame, id: FocusId) {
        widget.strip.access(frame, id);
    }

    fn act(&self, widget: &mut TabsBox<K>, id: FocusId, action: AccessAction) -> Option<TabsReply> {
        Some(TabsReply::Strip(widget.strip.access_action(id, action)))
    }

    /// A strip with tabs is a tab stop, so Tab and
    /// [`AppCx::focus`](crate::AppCx::focus) reach it.
    fn is_focusable(&self) -> bool {
        !self.tabs.is_empty()
    }

    fn takes_pointer(&self) -> bool {
        true
    }
}

impl<'a, A: 'static, K: Clone + PartialEq + 'static> From<TabStrip<'a, A, K>> for Element<'a, A> {
    /// The strip as an element visited by Tab after its siblings, as a
    /// title bar is, so a window's first tab stop is its content rather
    /// than its tabs; [`AppCx::focus`](crate::AppCx::focus) reaches
    /// it wherever it is.
    fn from(strip: TabStrip<'a, A, K>) -> Self {
        Element::new(Kind::Control(Box::new(strip))).order(1)
    }
}

// ========================================================================
// TabsBox Implementations
// ========================================================================

impl<K: PartialEq> TabsBox<K> {
    /// The app's key of the strip's tab `id`.
    fn key(&self, id: TabId) -> Option<&K> {
        self.keys
            .iter()
            .find(|(_, tab)| *tab == id)
            .map(|(key, _)| key)
    }

    /// The strip's id of the app's tab `key`.
    fn id(&self, key: &K) -> Option<TabId> {
        self.keys
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, id)| *id)
    }
}

impl<K> TabsBox<K> {
    /// The tab the keyboard is on: the one Left and Right moved to while
    /// it is open, else the active tab, else the first.
    fn cursor(&self) -> Option<TabId> {
        self.cursor
            .filter(|cursor| self.strip.ids().any(|id| id == *cursor))
            .or_else(|| self.strip.active())
            .or_else(|| self.strip.ids().next())
    }

    /// Moves the keyboard `delta` tabs right of the tab it is on (left
    /// when negative), wrapping around both ends.
    fn step_cursor(&mut self, delta: isize, cx: &mut EventCx<'_>) -> TabsReply {
        let ids: Vec<TabId> = self.strip.ids().collect();
        if let Some(at) = self
            .cursor()
            .and_then(|cursor| ids.iter().position(|id| *id == cursor))
        {
            let step = delta.rem_euclid(ids.len() as isize) as usize;
            self.cursor = Some(ids[(at + step) % ids.len()]);
            cx.request_redraw();
        }
        TabsReply::Strip(TabStripResponse::Handled)
    }

    /// What a key the strip took while it has the keyboard asks: Left
    /// and Right move between the tabs, wrapping, and Enter or Space
    /// activates the tab the keyboard is on, as a click on it does.
    /// `None` for every other key.
    fn take_key(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<TabsReply> {
        let key = input
            .key()
            .filter(|key| self.focused && key.mods == Mods::NONE)?;
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                let cursor = self.cursor()?;
                self.strip.activate(cursor);
                Some(TabsReply::Strip(TabStripResponse::Activated(cursor)))
            }
            KeyCode::Left => Some(self.step_cursor(-1, cx)),
            KeyCode::Right => Some(self.step_cursor(1, cx)),
            _ => None,
        }
    }
}

impl<K: 'static> Widget for TabsBox<K> {
    type Output = ();
    type Response = TabsReply;

    /// A right press on a tab asks for its menu, and while the strip has
    /// the keyboard Left, Right, Enter, and Space go to its tabs
    /// ([`TabsBox::take_key`]); everything else goes to the strip. A tab
    /// activated by the pointer is the one the keyboard is on next.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TabsReply {
        if let Some(press) = input.press(PointerButton::Right) {
            let point = press.position;
            if let Some(TabStripHit::Tab(id) | TabStripHit::Close(id)) = self.strip.hit_test(point)
            {
                return TabsReply::Context(id, point);
            }
        }
        if let Some(reply) = self.take_key(input, cx) {
            return reply;
        }
        let response = self.strip.handle(input, cx);
        if let TabStripResponse::Activated(id) = response {
            self.cursor = Some(id);
        }
        TabsReply::Strip(response)
    }

    /// Nothing: the [`TabStrip`] element draws the strip.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    /// Notes whether the strip has the keyboard; the keyboard comes to
    /// the active tab. The accent on the active tab stays the app's to say
    /// ([`TabStrip::focused`]).
    fn set_focused(&mut self, focused: bool) {
        if focused && !self.focused {
            self.cursor = None;
        }
        self.focused = focused;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.strip.about_to_wait(cx)
    }
}

impl<K: Clone + PartialEq + 'static> Control for TabsBox<K> {
    type Props<'a> = TabsProps<'a, K>;
    type Value = TabEvent<K>;

    /// Brings the strip in step with the app's tabs: closes the tabs the
    /// app no longer has, opens the new ones, puts each in its place,
    /// takes their titles and marks, and activates the app's active tab.
    fn sync(&mut self, props: TabsProps<'_, K>) {
        self.strip.set_height_ratio(props.height_ratio);
        if let Some(icons) = props.icons.filter(|_| !self.has_icons) {
            self.strip.set_icons(icons.clone());
            self.has_icons = true;
        }
        let gone: Vec<TabId> = self
            .keys
            .iter()
            .filter(|(key, _)| !props.tabs.iter().any(|tab| tab.key == *key))
            .map(|(_, id)| *id)
            .collect();
        for id in gone {
            self.strip.remove(id);
            self.keys.retain(|(_, tab)| *tab != id);
        }
        for (index, tab) in props.tabs.iter().enumerate() {
            let id = match self.id(&tab.key) {
                Some(id) => id,
                None => {
                    let id = self.strip.insert(index, tab.title);
                    self.keys.push((tab.key.clone(), id));
                    id
                }
            };
            if self.strip.ids().nth(index) != Some(id) {
                self.strip.move_tab(id, index);
            }
            if self.strip.title(id) != Some(tab.title) {
                self.strip.set_title(id, tab.title);
            }
            self.strip.set_modified(id, tab.modified);
            self.strip.set_busy(id, tab.busy);
            self.strip.set_closable(id, tab.closable);
            self.strip.set_icon(id, tab.icon.map(str::to_owned));
        }
        let active = props.active.and_then(|key| self.id(key));
        if let Some(active) = active
            && self.strip.active() != Some(active)
        {
            self.strip.activate(active);
        }
        Widget::set_focused(&mut self.strip, props.focused);
    }

    fn measure(&self, _width: f32, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        self.strip.bar_height(metrics)
    }

    /// What the strip's answer asks of the app, naming tabs by its keys.
    fn changed(&self, reply: TabsReply) -> Option<TabEvent<K>> {
        let key = |id: TabId| self.key(id).cloned();
        match reply {
            TabsReply::Context(id, point) => {
                Some(TabEvent::Context(key(id)?, MenuRequest::at(point)))
            }
            TabsReply::Strip(TabStripResponse::Activated(id)) => Some(TabEvent::Activate(key(id)?)),
            TabsReply::Strip(TabStripResponse::Close(id)) => Some(TabEvent::Close(key(id)?)),
            TabsReply::Strip(TabStripResponse::Moved(id)) => {
                let index = self.strip.ids().position(|tab| tab == id)?;
                Some(TabEvent::Move(key(id)?, index))
            }
            TabsReply::Strip(TabStripResponse::NewTab) => Some(TabEvent::New),
            TabsReply::Strip(_) => None,
        }
    }
}

impl Outcome for TabsReply {
    fn is_ignored(&self) -> bool {
        match self {
            Self::Context(..) => false,
            Self::Strip(response) => response.is_ignored(),
        }
    }
}

// ========================================================================
// Split Implementations
// ========================================================================

impl<A> Split<'_, A> {
    /// What runs on the app's state, with the new ratio (the share of the
    /// area the first element takes, between 0.1 and 0.9), on each move
    /// of the pointer dragging the divider: store it. Without one the
    /// divider stays where the app put it.
    pub fn on_resize(mut self, apply: impl Fn(&mut A, f32) + 'static) -> Self {
        self.parts.on_resize = Some(Rc::new(apply));
        self
    }
}

impl<'a, A> From<Split<'a, A>> for Element<'a, A> {
    fn from(split: Split<'a, A>) -> Self {
        Element::new(Kind::Split(split.parts))
    }
}

// ========================================================================
// SplitBox Implementations
// ========================================================================

impl SplitBox {
    /// A split view that has drawn nothing yet.
    pub(crate) fn new() -> Self {
        Self {
            area: Rect::default(),
            axis: Axis::Horizontal,
            held: false,
            scale_factor: 1.0,
            split: None,
            tree: SplitTree::new(),
            view: SplitView::new(),
        }
    }

    /// Takes the app's `axis` and `ratio`, splitting the tree again when
    /// the axis changed.
    pub(crate) fn sync(&mut self, axis: Axis, ratio: f32) {
        if self.split.is_none() || axis != self.axis {
            let mut tree = SplitTree::new();
            tree.split(tree.focused(), axis);
            self.split = tree
                .layout(Rect::default(), 0.0)
                .dividers
                .first()
                .map(|d| d.id);
            self.tree = tree;
            self.axis = axis;
            self.held = false;
        }
        if let Some(split) = self.split {
            self.tree.set_ratio(split, ratio);
        }
    }

    /// Draws the divider in `area` and returns where the two sides are.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> [Rect; 2] {
        self.area = area;
        self.scale_factor = frame.scale_factor();
        let panes = self.view.lend(&mut self.tree).draw(frame, area, theme);
        let side = |index: usize| {
            panes
                .get(index)
                .map_or_else(Rect::default, |pane| pane.rect)
        };
        [side(0), side(1)]
    }

    /// Hands `input` to the divider: whether it used it, and the ratio a
    /// drag moved it to. A press in a side is never used, so it reaches
    /// the elements there.
    pub(crate) fn follow(&mut self, input: &Input, cx: &mut EventCx<'_>) -> (bool, Option<f32>) {
        let response = self.view.lend(&mut self.tree).handle(input, cx);
        let used = response == SplitViewResponse::Handled;
        match input {
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.held = button.pressed && used;
                (used, None)
            }
            Input::PointerMoved(point) if self.held => (used, Some(self.ratio_at(*point))),
            _ => (used, None),
        }
    }

    /// The ratio that puts the divider at `point`, as the view's drag sets
    /// it.
    fn ratio_at(&self, point: Point) -> f32 {
        let thickness = logical(DIVIDER_THICKNESS, self.scale_factor);
        self.tree
            .layout(self.area, thickness)
            .dividers
            .first()
            .map_or(0.5, |divider| divider.ratio_at(point))
    }
}

impl Widget for SplitBox {
    type Output = ();
    type Response = SplitViewResponse;

    /// Nothing: the store hands a split's input to [`SplitBox::follow`].
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) -> SplitViewResponse {
        SplitViewResponse::Ignored
    }

    /// Nothing: the store draws a split with [`SplitBox::draw`].
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}
}

// ========================================================================
// StatusBar Implementations
// ========================================================================

impl<'a, A> StatusBar<'a, A> {
    /// This bar showing `notifier`'s newest message in place of its primary
    /// text while it lasts, and its tasks under way at the right.
    pub fn notifier(self, notifier: &'a Notifier) -> Self {
        Self {
            notifier: Some(notifier),
            ..self
        }
    }

    /// What runs, with the segment's id, when the user clicks a segment
    /// given an id ([`StatusSegment::with_id`](crate::StatusSegment::with_id)).
    pub fn on_click(self, apply: impl Fn(&mut A, &'static str) + 'static) -> Self {
        Self {
            on_click: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs when the user clicks the tasks' item: show the tasks as
    /// cards ([`Notifier::show_task_cards`]).
    pub fn on_progress(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_progress: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for StatusBar<'_, A> {
    type Widget = StatusBox;

    const KIND: &'static str = "status bar";

    fn create(&self) -> StatusBox {
        StatusBox {
            bar: status_bar::StatusBar::new(),
            empty: Notifier::new(),
            task_started: None,
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) {}

    fn call(&self, response: StatusBarResponse) -> Option<Call<A>> {
        match response {
            StatusBarResponse::Clicked(id) => call(self.on_click.as_ref(), id),
            StatusBarResponse::Progress => call(self.on_progress.as_ref(), ()),
            StatusBarResponse::Handled | StatusBarResponse::Ignored => None,
        }
    }

    fn draw(&self, widget: &mut StatusBox, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        let notifier = self.notifier.unwrap_or(&widget.empty);
        widget.task_started = notifier.tasks().first().map(|task| task.started);
        widget
            .bar
            .lend(&self.line, notifier)
            .draw(frame, rect, theme);
        rect.into()
    }

    /// Nothing: the bar describes nothing to screen readers yet.
    fn describe(&self, _widget: &StatusBox, _frame: &mut Frame, _id: FocusId) {}

    fn act(
        &self,
        _widget: &mut StatusBox,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<StatusBarResponse> {
        None
    }

    fn handle(
        &self,
        widget: &mut StatusBox,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> StatusBarResponse {
        let notifier = self.notifier.unwrap_or(&widget.empty);
        widget.bar.lend(&self.line, notifier).handle(input, cx)
    }

    fn is_focusable(&self) -> bool {
        false
    }

    fn takes_pointer(&self) -> bool {
        true
    }
}

impl<'a, A: 'static> From<StatusBar<'a, A>> for Element<'a, A> {
    fn from(bar: StatusBar<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(bar)))
    }
}

// ========================================================================
// StatusBox Implementations
// ========================================================================

impl Widget for StatusBox {
    type Output = ();
    type Response = StatusBarResponse;

    /// Nothing: the [`StatusBar`] element hands its bar inputs lent the
    /// app's line.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) -> StatusBarResponse {
        StatusBarResponse::Ignored
    }

    /// Nothing: the [`StatusBar`] element draws its bar lent the app's
    /// line.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    /// Asks for a redraw while tasks run, and returns when their spinner
    /// turns next; `None` while none runs or motion is reduced.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let started = self.task_started?;
        if cx.reduced_motion() {
            return None;
        }
        cx.request_redraw();
        Some(spinner::next_turn(started, clock::now()))
    }
}

impl Control for StatusBox {
    type Props<'a> = ();
    type Value = StatusBarResponse;

    /// Nothing: the line and the notifier are lent at each draw.
    fn sync(&mut self, (): ()) {}

    fn measure(&self, _width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        status_bar::StatusBar::height(metrics, scale_factor)
    }

    fn changed(&self, response: StatusBarResponse) -> Option<StatusBarResponse> {
        matches!(
            response,
            StatusBarResponse::Clicked(_) | StatusBarResponse::Progress
        )
        .then_some(response)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A title bar showing `title`, for a frameless window
/// ([`WindowOptions::with_decorations`](crate::WindowOptions::with_decorations)):
/// the window buttons at the right, a drag region that moves the window
/// (double-click to maximize), and the resize border along the window's
/// edges, which it takes over the content. Put it first in the window's
/// root [`column`](crate::view::column): it draws along the top of its
/// place and its border runs down to the window's bottom.
/// [`TitleBar::on_menu`] adds a menu button and [`TitleBar::menus`] a
/// menubar, both asking the app to show its menu.
pub fn title_bar<'a, A>(title: &'a str) -> TitleBar<'a, A> {
    TitleBar {
        app_buttons: Vec::new(),
        buttons: WindowButtons::default(),
        menus: &[],
        on_close: None,
        on_menu: None,
        on_menu_title: None,
        open_menu: None,
        title,
        title_align: None,
    }
}

/// A tab the app knows by `key`, saying `title`, for a [`tab_strip`]:
/// closable, with no marks until [`Tab::modified`] or [`Tab::busy`] says.
pub fn tab<K>(key: K, title: &str) -> Tab<'_, K> {
    Tab {
        busy: false,
        closable: true,
        icon: None,
        key,
        modified: false,
        title,
    }
}

/// A row of `tabs` from the app's state, left to right, the tab `active`
/// names marked, a close button on it and on the tab under the pointer,
/// and a new-tab button after the last. Every tab and its order are the
/// app's: a click on a tab runs [`TabStrip::on_activate`], a close button
/// [`TabStrip::on_close`], a tab dragged to a new place
/// [`TabStrip::on_move`], and the new-tab button [`TabStrip::on_new`], and
/// the strip shows only what the app's state says next. Give it an id
/// ([`Element::id`]) when tabs come and go around it.
pub fn tab_strip<'a, A, K: Clone>(
    tabs: impl IntoIterator<Item = Tab<'a, K>>,
    active: Option<&K>,
) -> TabStrip<'a, A, K> {
    TabStrip {
        active: active.cloned(),
        focused: true,
        height_ratio: None,
        icons: None,
        on_activate: None,
        on_close: None,
        on_context_menu: None,
        on_move: None,
        on_tab_drop: None,
        on_new: None,
        tabs: tabs.into_iter().collect(),
    }
}

/// `first` and `second` sharing the room their parent leaves, placed as
/// `axis` says (side by side for [`Axis::Horizontal`], one above the other
/// for [`Axis::Vertical`]), the first taking `ratio` of it (held between
/// 0.1 and 0.9), a divider between them. The ratio is the app's: dragging
/// the divider runs [`Split::on_resize`] on each move. Either side may be
/// another split, for nested panes.
pub fn split<'a, A>(
    axis: Axis,
    ratio: f32,
    first: impl Into<Element<'a, A>>,
    second: impl Into<Element<'a, A>>,
) -> Split<'a, A> {
    Split {
        parts: SplitParts {
            axis,
            children: vec![first.into(), second.into()],
            on_resize: None,
            ratio,
        },
    }
}

/// A status bar showing `line`, built from the app's state for this
/// rebuild: its mode, pending keys, segments, and primary text. Put it last
/// in the window's root [`column`](crate::view::column).
/// [`StatusBar::notifier`] shows the app's messages and tasks in it.
pub fn status_bar<'a, A>(line: StatusLine) -> StatusBar<'a, A> {
    StatusBar {
        line,
        notifier: None,
        on_click: None,
        on_progress: None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use accesskit::Role;

    use super::*;
    use crate::app::context::WindowAction;
    use crate::base::styled::StyledText;
    use crate::base::theme::ThemeToken;
    use crate::chrome::status_bar::StatusSegment;
    use crate::platform::resize::ResizeEdge;
    use crate::platform::window_id::WindowId;
    use crate::testing::{TestWindow, TreeWindow, ui_text};
    use crate::view::app::App;
    use crate::view::controls::button;
    use crate::view::element::{Element, column, row, text};

    /// The window's width in the tests, in physical pixels.
    const WIDTH: u32 = 800;

    /// The window's height in the tests.
    const HEIGHT: u32 = 400;

    /// The window's title.
    const TITLE: &str = "chrome";

    /// The menubar's titles.
    const MENUS: [&str; 2] = ["File", "Edit"];

    /// An app whose tree each test picks, recording what reaches it.
    struct Probe {
        /// The key of the tab shown.
        active: Option<u32>,
        /// What reached the app, in order.
        log: Vec<String>,
        /// The tabs, left to right: key and title.
        tabs: Vec<(u32, &'static str)>,
        /// The colors it is drawn in.
        theme: Theme,
        /// The ratio of the probe's split.
        ratio: f32,
        /// The test's tree.
        view: fn(&Probe) -> Element<'_, Probe>,
    }

    impl App for Probe {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            (self.view)(self)
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window showing the tree `view` over three tabs, the first shown.
    fn window(view: fn(&Probe) -> Element<'_, Probe>) -> TreeWindow<Probe> {
        let probe = Probe {
            active: Some(1),
            log: Vec::new(),
            tabs: vec![(1, "one"), (2, "two"), (3, "three")],
            theme: Theme::dark(),
            ratio: 0.5,
            view,
        };
        TreeWindow::new(probe, TITLE, WIDTH, HEIGHT)
    }

    /// A title bar logging what reaches the app, with a menu button and a
    /// menubar.
    fn bar(_probe: &Probe) -> Element<'_, Probe> {
        title_bar(TITLE)
            .menus(&MENUS)
            .on_menu(|probe: &mut Probe, at| {
                probe.log.push(format!("menu by_key={}", at.by_key));
            })
            .on_menu_title(|probe: &mut Probe, (index, _)| {
                probe.log.push(format!("title {index}"));
            })
            .into()
    }

    /// The probe's tabs as a strip, every callback logging and changing
    /// the probe as an app would.
    fn strip(probe: &Probe) -> Element<'_, Probe> {
        let tabs = probe.tabs.iter().map(|(key, title)| tab(*key, title));
        tab_strip(tabs, probe.active.as_ref())
            .on_activate(|probe: &mut Probe, key| probe.active = Some(key))
            .on_close(|probe: &mut Probe, key| probe.tabs.retain(|(k, _)| *k != key))
            .on_move(|probe: &mut Probe, (key, index)| {
                let from = probe.tabs.iter().position(|(k, _)| *k == key);
                if let Some(from) = from {
                    let moved = probe.tabs.remove(from);
                    probe.tabs.insert(index, moved);
                }
            })
            .on_new(|probe: &mut Probe| probe.tabs.push((9, "new")))
            .on_context_menu(|probe: &mut Probe, (key, _)| {
                probe.log.push(format!("tab menu {key}"));
            })
            .into()
    }

    /// `text` in the foreground color.
    fn plain(text: &str) -> StyledText {
        StyledText::plain(text, ThemeToken::Foreground.into())
    }

    /// The titles of the tabs `window` describes, left to right.
    fn tab_titles(window: &TreeWindow<Probe>) -> Vec<String> {
        window
            .ui_text()
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("Tab \""))
            .map(|rest| rest.split('"').next().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn test_the_close_button_closes_the_window_unless_the_app_runs_something() {
        let mut window = window(|_| column([title_bar(TITLE).into()]));
        window.click(Role::Button, "Close");
        assert!(window.is_closed());
        let mut asking = self::window(|_| {
            column([title_bar(TITLE)
                .on_close(|probe: &mut Probe| probe.log.push("close".into()))
                .into()])
        });
        asking.click(Role::Button, "Close");
        assert!(!asking.is_closed());
        assert_eq!(asking.app().log, ["close"]);
    }

    #[test]
    fn test_the_menu_button_asks_for_the_menu_by_pointer_and_by_key() {
        let mut window = window(|probe| column([bar(probe), button("Content").into()]));
        assert!(
            window
                .ui_text()
                .contains("Button \"Content\" disabled focused"),
            "the content, not the menu button, is the first tab stop: {}",
            window.ui_text()
        );
        window.press("<tab> RET");
        window.click(Role::Button, "Menu");
        assert_eq!(window.app().log, ["menu by_key=true", "menu by_key=false"]);
    }

    /// A title bar with its window buttons at the left, a menu button at
    /// the right, and two app buttons beside it, each logging.
    fn app_bar(_probe: &Probe) -> Element<'_, Probe> {
        title_bar(TITLE)
            .window_buttons(WindowButtons::Left)
            .app_button(
                TitleBarButton::label("Left").with_tooltip("Show the left sidebar"),
                |probe: &mut Probe| probe.log.push("left".into()),
            )
            .app_button(TitleBarButton::label("Theme"), |probe: &mut Probe| {
                probe.log.push("theme".into());
            })
            .on_menu(|probe: &mut Probe, at| {
                probe.log.push(format!("menu by_key={}", at.by_key));
            })
            .into()
    }

    #[test]
    fn test_app_buttons_run_their_callbacks_by_pointer_screen_reader_and_keys() {
        let mut window = window(|probe| column([app_bar(probe), button("Content").into()]));
        window.click(Role::Button, "Theme");
        let left = window.find(Role::Button, "Left");
        window.act(AccessAction::Click(left));
        assert_eq!(window.app().log, ["theme", "left"]);
        // The bar is one tab stop after the content; Left and Right move
        // between its buttons, left to right, wrapping.
        window.press("<tab> RET <right> RET <right> RET <right> RET <left> RET");
        assert_eq!(
            window.app().log[2..],
            [
                "left",
                "theme",
                "menu by_key=true",
                "left",
                "menu by_key=true"
            ]
        );
    }

    #[test]
    fn test_a_menubar_title_asks_for_its_menu_and_lights_while_open() {
        let mut window = window(|probe| column([bar(probe)]));
        window.click(Role::MenuItem, "Edit");
        assert_eq!(window.app().log, ["title 1"]);
        let edit = window.find(Role::MenuItem, "Edit");
        window.act(AccessAction::Click(edit));
        assert_eq!(window.app().log, ["title 1", "title 1"]);
    }

    #[test]
    fn test_the_resize_border_takes_the_pointer_over_the_content() {
        let mut window = window(|probe| {
            column([
                bar(probe),
                Element::from(
                    button("Wide").on_click(|probe: &mut Probe| probe.log.push("clicked".into())),
                ),
            ])
        });
        let wide = window.center(Role::Button, "Wide");
        window.window().take_actions();
        window.click_at(Point::new(1.0, wide.y), PointerButton::Left);
        assert!(window.app().log.is_empty(), "{:?}", window.app().log);
        assert_eq!(
            window.window().take_actions(),
            [WindowAction::Resize(ResizeEdge::West)]
        );
        window.click(Role::Button, "Wide");
        assert_eq!(window.app().log, ["clicked"], "inside the border it clicks");
    }

    #[test]
    fn test_a_click_activates_a_tab_and_the_app_decides_what_shows() {
        let mut window = window(strip);
        window.click(Role::Tab, "two");
        assert_eq!(window.app().active, Some(2));
        assert!(
            window.ui_text().contains("Tab \"two\" selected"),
            "{}",
            window.ui_text()
        );
        let mut fixed = self::window(|probe| {
            let tabs = probe.tabs.iter().map(|(key, title)| tab(*key, title));
            tab_strip(tabs, probe.active.as_ref()).into()
        });
        fixed.click(Role::Tab, "two");
        assert!(
            fixed.ui_text().contains("Tab \"one\" selected"),
            "without on_activate the app's tab stays: {}",
            fixed.ui_text()
        );
    }

    #[test]
    fn test_left_and_right_move_between_tabs_and_enter_or_space_activates() {
        let mut window = window(|probe| column([strip(probe), button("After").into()]));
        window.press("<tab>");
        assert!(
            window.ui_text().contains("TabList focused"),
            "the strip is a tab stop after its siblings: {}",
            window.ui_text()
        );
        window.press("<right> <right>");
        assert_eq!(window.app().active, Some(1), "moving does not activate");
        window.press("RET");
        assert_eq!(window.app().active, Some(3));
        window.press("<right> <space>");
        assert_eq!(window.app().active, Some(1), "Right wraps to the first tab");
        window.press("<left> <left> <space>");
        assert_eq!(window.app().active, Some(2), "Left wraps to the last tab");
        window.click(Role::Tab, "three");
        window.press("<right> RET");
        assert_eq!(
            window.app().active,
            Some(1),
            "the keyboard moves on from the tab clicked last"
        );
        window.press("<tab> <right> RET");
        assert_eq!(
            window.app().active,
            Some(1),
            "keys reach it only while focused"
        );
        window.press("<tab>");
        assert!(window.ui_text().contains("TabList focused"));
        window.press("<right> RET");
        assert_eq!(
            window.app().active,
            Some(2),
            "the keyboard comes back to the active tab"
        );
    }

    #[test]
    fn test_a_strip_with_no_tabs_is_no_tab_stop() {
        let window = window(|probe| {
            let none = std::iter::empty::<Tab<'_, u32>>();
            column([
                tab_strip(none, probe.active.as_ref()).into(),
                button("After").into(),
            ])
        });
        assert!(
            window
                .ui_text()
                .contains("Button \"After\" disabled focused"),
            "{}",
            window.ui_text()
        );
    }

    #[test]
    fn test_tabs_close_open_and_follow_the_apps_list() {
        let mut window = window(strip);
        let new_tab = window.find(Role::Button, "New Tab");
        window.act(AccessAction::Click(new_tab));
        assert_eq!(tab_titles(&window), ["one", "two", "three", "new"]);
        window.update(|probe| probe.tabs.retain(|(key, _)| *key != 2));
        assert_eq!(tab_titles(&window), ["one", "three", "new"]);
        window.update(|probe| probe.tabs[0].1 = "renamed");
        assert_eq!(tab_titles(&window), ["renamed", "three", "new"]);
    }

    #[test]
    fn test_the_active_tabs_close_button_asks_the_app_to_close_it() {
        let mut window = window(strip);
        let one = window.center(Role::Tab, "one");
        window.send(&Input::PointerMoved(one));
        let tabs = window
            .store()
            .widget::<TabsBox<u32>>()
            .map(|tabs| tabs.strip.clone())
            .expect("the strip was kept");
        let close = (0..WIDTH)
            .map(|x| Point::new(x as f32, one.y))
            .find(|point| matches!(tabs.hit_test(*point), Some(TabStripHit::Close(_))))
            .expect("the active tab shows its close button");
        window.click_at(close, PointerButton::Left);
        assert_eq!(tab_titles(&window), ["two", "three"]);
    }

    #[test]
    fn test_a_tab_dragged_along_the_strip_moves_where_the_app_puts_it() {
        let mut window = window(strip);
        let one = window.center(Role::Tab, "one");
        let three = window.center(Role::Tab, "three");
        window.drag(one, Point::new(three.x + 20.0, three.y));
        assert_eq!(tab_titles(&window), ["two", "three", "one"]);
        let mut fixed = self::window(|probe| {
            let tabs = probe.tabs.iter().map(|(key, title)| tab(*key, title));
            tab_strip(tabs, probe.active.as_ref()).into()
        });
        let one = fixed.center(Role::Tab, "one");
        let three = fixed.center(Role::Tab, "three");
        fixed.drag(one, Point::new(three.x + 20.0, three.y));
        assert_eq!(
            tab_titles(&fixed),
            ["one", "two", "three"],
            "without on_move the tab goes back"
        );
    }

    /// A tab strip over a pane taking dragged tabs, recording each drop.
    fn strip_over_pane(probe: &Probe) -> Element<'_, Probe> {
        let tabs = probe.tabs.iter().map(|(key, title)| tab(*key, title));
        let pane = Element::from(text("pane"))
            .grow(1.0)
            .drop_target(
                |_: &DraggedTab<u32>| true,
                |probe: &mut Probe, dropped: DraggedTab<u32>, _| {
                    probe.log.push(format!("dropped {}", dropped.key));
                },
            )
            .drop_marker(|rect, _| Rect::new(rect.x, rect.y, rect.width / 2.0, rect.height));
        column([tab_strip(tabs, probe.active.as_ref()).into(), pane])
    }

    #[test]
    fn test_a_tab_dragged_off_its_strip_drops_on_a_pane_that_takes_it() {
        let mut window = window(strip_over_pane);
        let two = window.center(Role::Tab, "two");
        window.drag(two, Point::new(two.x, two.y + 200.0));
        assert_eq!(window.app().log, ["dropped 2"]);
        assert_eq!(
            tab_titles(&window),
            ["one", "two", "three"],
            "the strip keeps it"
        );
    }

    /// The probe's strip beside a strip of two tabs of its own taking
    /// dropped tabs, recording each drop and where it lands.
    fn two_strips(probe: &Probe) -> Element<'_, Probe> {
        let tabs = probe.tabs.iter().map(|(key, title)| tab(*key, title));
        let other = [tab(7_u32, "seven"), tab(8, "eight")];
        row([
            Element::from(tab_strip(tabs, probe.active.as_ref())).grow(1.0),
            Element::from(tab_strip(other, Some(&7)).on_tab_drop(
                |probe: &mut Probe, (key, index): (u32, usize)| {
                    probe.log.push(format!("drop {key} at {index}"));
                },
            ))
            .grow(1.0),
        ])
    }

    #[test]
    fn test_a_tab_dropped_on_another_strip_lands_between_its_tabs() {
        let mut window = window(two_strips);
        let two = window.center(Role::Tab, "two");
        let eight = window.center(Role::Tab, "eight");
        window.drag(two, Point::new(eight.x - 20.0, eight.y));
        assert_eq!(window.app().log, ["drop 2 at 1"]);
    }

    #[test]
    fn test_a_right_click_on_a_tab_asks_for_its_menu() {
        let mut window = window(strip);
        window.right_click(Role::Tab, "three");
        assert_eq!(window.app().log, ["tab menu 3"]);
    }

    #[test]
    fn test_dragging_the_divider_reports_the_ratio_and_the_sides_follow_it() {
        let mut window = window(|probe| {
            split(
                Axis::Horizontal,
                probe.ratio,
                button("Left"),
                Element::from(
                    button("Right").on_click(|probe: &mut Probe| probe.log.push("right".into())),
                ),
            )
            .on_resize(|probe: &mut Probe, ratio| probe.ratio = ratio)
            .into()
        });
        let middle = WIDTH as f32 / 2.0;
        let right_before = window.center(Role::Button, "Right");
        assert!(right_before.x > middle, "{right_before:?}");
        window.drag(Point::new(middle, 200.0), Point::new(600.0, 200.0));
        assert!(
            (window.app().ratio - 0.75).abs() < 0.01,
            "{}",
            window.app().ratio
        );
        let right_after = window.center(Role::Button, "Right");
        assert!(right_after.x > right_before.x, "{right_after:?}");
        window.click(Role::Button, "Right");
        assert_eq!(window.app().log, ["right"], "a press in a side reaches it");
    }

    #[test]
    fn test_nested_splits_lay_out_three_panes_and_a_fixed_ratio_holds() {
        let mut window = window(|probe| {
            split(
                Axis::Horizontal,
                probe.ratio,
                button("A"),
                split(Axis::Vertical, 0.5, button("B"), button("C")),
            )
            .into()
        });
        let (a, b, c) = (
            window.center(Role::Button, "A"),
            window.center(Role::Button, "B"),
            window.center(Role::Button, "C"),
        );
        assert!(
            a.x < b.x && (b.x - c.x).abs() < 1.0 && b.y < c.y,
            "{a:?} {b:?} {c:?}"
        );
        let middle = WIDTH as f32 / 2.0;
        window.drag(Point::new(middle, 300.0), Point::new(600.0, 300.0));
        assert_eq!(window.app().ratio, 0.5, "no on_resize, no move");
    }

    #[test]
    fn test_a_status_segment_click_reaches_the_app() {
        let mut window = window(|_| {
            let line = StatusLine::new(plain("ready"))
                .with_segment(StatusSegment::new(plain("UTF-8")).with_id("encoding"));
            column([
                text("content").into(),
                status_bar(line)
                    .on_click(|probe: &mut Probe, id| probe.log.push(id.into()))
                    .into(),
            ])
        });
        let segment = window
            .store()
            .widget::<StatusBox>()
            .and_then(|status| status.bar.segment_rect("encoding"))
            .expect("the segment was drawn");
        let at = Point::new(segment.x + 2.0, segment.y + segment.height / 2.0);
        window.click_at(at, PointerButton::Left);
        assert_eq!(window.app().log, ["encoding"]);
    }

    #[test]
    fn test_the_chrome_describes_itself_as_the_hand_driven_widgets_do() {
        let theme = Theme::dark();
        let line = StatusLine::new(plain("ready"));
        let test = TestWindow::new(WIDTH, HEIGHT);
        let mut frame = test.frame();
        let mut by_hand = title_bar::TitleBar::new(TITLE)
            .with_menu_button()
            .with_menus(MENUS.iter().map(|title| (*title).to_owned()).collect());
        let bounds = frame.bounds();
        let below = by_hand.draw(&mut frame, bounds, &theme);
        by_hand.access(&mut frame, FocusId::new("bar"));
        let mut tabs = tab_strip::TabStrip::new();
        for title in ["one", "two", "three"] {
            tabs.push(title);
        }
        let first = tabs.ids().next().expect("a tab");
        tabs.activate(first);
        tabs.draw(&mut frame, below, &theme);
        tabs.access(&mut frame, FocusId::new("tabs"));
        let mut status = status_bar::StatusBar::new();
        status
            .lend(&line, &Notifier::new())
            .draw(&mut frame, below, &theme);
        let hand_driven = ui_text(&frame, TITLE);
        let window = window(|probe| {
            column([
                bar(probe),
                strip(probe),
                text("").into(),
                status_bar(StatusLine::new(plain("ready"))).into(),
            ])
        });
        // The window's tab stops are the menu button and the strip, both
        // after their siblings, so the bar's comes first and has the
        // keyboard: its own node, the one id the tree registers it by,
        // says so where the hand-driven window did.
        let tree = window.ui_text().replace(
            &format!("TitleBar \"{TITLE}\" focused"),
            &format!("TitleBar \"{TITLE}\""),
        );
        let hand_driven = hand_driven.replace(
            &format!("Window \"{TITLE}\" focused"),
            &format!("Window \"{TITLE}\""),
        );
        assert_eq!(tree, hand_driven);
    }
}

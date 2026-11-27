//! [`Menu`]: a popup list of commands, each with a label and an optional
//! accelerator hint, split into groups by separators, any of them disabled.
//! It opens beside an anchor (under a menu button, at the pointer for a
//! context menu) and hands back the id of the item chosen.
//!
//! A heading ([`MenuEntry::Heading`]) names the items under it, as a
//! hamburger menu shows its File, Edit, and View as sections rather than
//! submenus: it is drawn muted and never highlighted or chosen, so Up and
//! Down and the pointer pass over it, and screen readers hear it as a
//! heading.
//!
//! An item may carry a check mark ([`MenuItem::with_checked`]) or a radio
//! dot ([`MenuItem::with_radio`]) for an option it turns on, or hold a
//! submenu ([`MenuItem::with_submenu`]) that opens beside it, on its right,
//! or on its left when the window has no room there. Keep submenus few and
//! shallow: a keyboard-first app reaches deep commands through a command
//! palette.
//!
//! [`Menu::handle`](crate::Widget::handle) drives an open menu: hovering
//! highlights, and resting on an item with a submenu for a moment opens it
//! (resting on another item closes it again); a click chooses an item or
//! opens its submenu at once; a click outside or Escape closes (Escape
//! closes only the innermost submenu). Up and Down move the highlight,
//! Enter chooses it, Right or `l` opens the highlighted item's submenu and
//! Left or `h` closes the submenu the highlight is in. Any other key is the
//! app's to bind to [`Menu::move_highlight`], [`Menu::choose`],
//! [`Menu::open_submenu`], and [`Menu::close_submenu`], so the menu carries
//! no keymap of its own beyond those.
//!
//! A menu stays inside the window it is drawn in: one taller than the room
//! below its anchor flips above it, or moves up, as every popup does, and
//! one taller than the window fills its height and scrolls. The wheel over
//! it scrolls it, and the highlight moved by keys is scrolled into view.
//! Submenus and context menus do the same. A menu that fits is drawn as it
//! always was.

use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use accesskit::{Action, HasPopup, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::theme::{Shape, Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton, WheelDelta, WheelInput};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::label::{draw_label, draw_styled_label, label_width};
use super::popup::{Popup, PopupSide};

// ========================================================================
// Constants
// ========================================================================

/// An item's height as a multiple of the cell height.
const ITEM_HEIGHT_RATIO: f32 = 1.5;

/// A separator's height as a multiple of the cell height.
const SEPARATOR_HEIGHT_RATIO: f32 = 0.5;

/// The space above the first and below the last entry, in logical pixels.
const PADDING_Y: f32 = 4.0;

/// The cells between an item's label and its accelerator.
const ACCELERATOR_GAP_CELLS: usize = 3;

/// The fewest cells of text a menu is wide.
const MIN_TEXT_CELLS: usize = 16;

/// The thickness of a separator line, in logical pixels.
const SEPARATOR_STROKE: f32 = 1.0;

/// The cells kept left of the labels for check marks and radio dots, when
/// any item has one.
const MARK_CELLS: f32 = 2.0;

/// What a checked item shows left of its label.
const CHECK_GLYPH: &str = "\u{2713}";

/// What a selected radio item shows left of its label.
const RADIO_GLYPH: &str = "\u{2022}";

/// What an item holding a submenu shows at its right.
const SUBMENU_GLYPH: &str = "\u{203a}";

/// How long the pointer rests on an item before its submenu opens, or
/// before an open submenu closes for another item.
const SUBMENU_DELAY: Duration = Duration::from_millis(300);

/// The child index, under a submenu item's node, of its submenu's node.
const SUBMENU_NODE: u64 = 0;

/// The items a notch of the wheel scrolls a menu taller than the window.
const WHEEL_ITEMS: f32 = 3.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A popup menu of items with ids of type `I`. It keeps its entries,
/// whether and where it is open, the highlighted item, and the layout of
/// the last frame it drew, which its hit testing uses.
#[derive(Clone, Debug)]
pub struct Menu<I> {
    /// The items and separators, top to bottom.
    entries: Vec<MenuEntry<I>>,
    /// The index of the highlighted entry, always an enabled item.
    highlighted: Option<usize>,
    /// The submenu to open (or close) once the pointer has rested on an
    /// entry for a moment.
    hover_switch: Option<HoverSwitch>,
    /// Where the menu was in the last frame drawn, or `None` before it or
    /// while closed.
    layout: Option<MenuLayout>,
    /// Where the menu opens, or `None` while it is closed.
    opened_at: Option<MenuAnchor>,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// Whether the next frame scrolls the highlighted entry into view, as
    /// it does after the keys moved the highlight.
    reveal: bool,
    /// How far the rows of a menu taller than the window are scrolled up,
    /// in pixels; 0 for a menu that fits.
    scroll: f32,
    /// The open submenu, if any.
    submenu: Option<Box<Submenu<I>>>,
}

/// A submenu to switch to once the pointer has rested on an entry.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HoverSwitch {
    /// When the switch is due.
    at: Instant,
    /// The entry rested on: its submenu opens, or, for an entry without
    /// one, the open submenu closes.
    entry: usize,
}

/// An open submenu, beside the entry holding it.
#[derive(Clone, Debug)]
struct Submenu<I> {
    /// The index of the entry holding it.
    entry: usize,
    /// The submenu itself.
    menu: Menu<I>,
}

/// What a menu opens beside.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MenuAnchor {
    /// The rectangle it opens beside.
    rect: Rect,
    /// The side of `rect` it opens on.
    side: PopupSide,
}

/// Where a menu's parts were drawn.
#[derive(Clone, Debug, PartialEq)]
struct MenuLayout {
    /// The room left of the labels for marks, in pixels; 0 when no item
    /// has one.
    gutter: f32,
    /// An item's height, in pixels: what a notch of the wheel scrolls by.
    item_height: f32,
    /// How far the rows can scroll, in pixels: how much taller they are
    /// than `viewport`; 0 when they fit.
    overflow: f32,
    /// The box.
    popup: Popup,
    /// Each entry's row, in order, moved up by `scroll`.
    rows: Vec<Rect>,
    /// How far the rows are scrolled up, in pixels, held between 0 and
    /// `overflow`.
    scroll: f32,
    /// The part of the box the rows show in: inside its padding.
    viewport: Rect,
}

/// One line of a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry<I> {
    /// A heading naming the items under it, drawn muted; it is skipped by
    /// the highlight and never chosen.
    Heading(String),
    /// A command.
    Item(MenuItem<I>),
    /// A line between groups of items.
    Separator,
}

/// A command in a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem<I> {
    /// The keys that run the command, shown muted at the right, such as
    /// `"Ctrl+T"`; only a hint, the menu does not bind them.
    accelerator: Option<String>,
    /// Whether the item can be chosen.
    enabled: bool,
    /// What [`Menu::choose`] and [`MenuResponse::Activated`] hand back.
    id: I,
    /// The text shown.
    label: String,
    /// The mark left of the label, if any.
    mark: Mark,
    /// The entries of the submenu it opens, or `None` for a command.
    submenu: Option<Vec<MenuEntry<I>>>,
}

/// The mark an item shows left of its label.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Mark {
    /// A check mark when `true`: an option turned on or off.
    Check(bool),
    /// No mark.
    Plain,
    /// A dot when `true`: the chosen one of a few options.
    Radio(bool),
}

/// What became of an input offered to a [`Menu`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum MenuResponse<I> {
    /// This item was clicked; the menu (with every submenu) has closed.
    Activated(I),
    /// The menu closed without a choice: a click outside it, or Escape.
    Closed,
    /// The menu used the event; while it is open it takes every pointer
    /// event, so nothing under it reacts.
    Handled,
    /// The event is not the menu's; the app handles it as usual.
    Ignored,
}

// ========================================================================
// Menu Implementations
// ========================================================================

impl<I: Copy> Menu<I> {
    /// A closed menu of `entries`, top to bottom.
    pub fn new(entries: Vec<MenuEntry<I>>) -> Self {
        Self {
            entries,
            highlighted: None,
            hover_switch: None,
            layout: None,
            opened_at: None,
            pointer: None,
            reveal: false,
            scroll: 0.0,
            submenu: None,
        }
    }

    /// Opens the menu on `side` of `anchor` with nothing highlighted, or
    /// moves it there when it is open.
    pub fn open(&mut self, anchor: Rect, side: PopupSide) {
        self.opened_at = Some(MenuAnchor { rect: anchor, side });
        self.highlighted = None;
        self.layout = None;
        self.hover_switch = None;
        self.scroll = 0.0;
        self.submenu = None;
    }

    /// Closes the menu and its submenus.
    pub fn close(&mut self) {
        self.opened_at = None;
        self.highlighted = None;
        self.layout = None;
        self.hover_switch = None;
        self.scroll = 0.0;
        self.submenu = None;
    }

    /// The items and separators, top to bottom.
    pub(crate) fn entries(&self) -> &[MenuEntry<I>] {
        &self.entries
    }

    /// Whether the menu is open.
    pub fn is_open(&self) -> bool {
        self.opened_at.is_some()
    }

    /// Moves the highlight `delta` enabled items down (up when negative)
    /// in the innermost open submenu (the menu itself when none is open),
    /// skipping separators and disabled items and wrapping around both
    /// ends. With nothing highlighted, a step down highlights the first
    /// item and a step up the last.
    pub fn move_highlight(&mut self, delta: isize) {
        self.innermost_mut().move_own_highlight(delta);
    }

    /// [`Menu::move_highlight`] in this menu, ignoring its submenus.
    fn move_own_highlight(&mut self, delta: isize) {
        let enabled: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| is_enabled(entry))
            .map(|(index, _)| index)
            .collect();
        if enabled.is_empty() || delta == 0 {
            return;
        }
        let count = enabled.len() as isize;
        let position = match self
            .highlighted
            .and_then(|index| enabled.iter().position(|&entry| entry == index))
        {
            Some(position) => (position as isize + delta).rem_euclid(count),
            None if delta > 0 => (delta - 1).rem_euclid(count),
            None => (count + delta).rem_euclid(count),
        };
        self.highlighted = Some(enabled[position as usize]);
        self.reveal = true;
    }

    /// The id of the item highlighted in the innermost open submenu (the
    /// menu itself when none is open), or `None` when nothing is
    /// highlighted there.
    pub fn highlighted(&self) -> Option<I> {
        let menu = self.innermost();
        let index = menu.highlighted?;
        match menu.entries.get(index)? {
            MenuEntry::Item(item) => Some(item.id),
            MenuEntry::Heading(_) | MenuEntry::Separator => None,
        }
    }

    /// Chooses the highlighted item of the innermost open submenu: closes
    /// the menu and returns its id. An item holding a submenu opens it
    /// instead, as [`Menu::open_submenu`] does, and returns `None`, as it
    /// does when nothing is highlighted.
    pub fn choose(&mut self) -> Option<I> {
        let menu = self.innermost_mut();
        let index = menu.highlighted?;
        if menu.open_own_submenu(index, true) {
            return None;
        }
        let id = menu.item_id(index)?;
        self.close();
        Some(id)
    }

    /// Opens the submenu of the item highlighted in the innermost open
    /// submenu, with its first item highlighted. Returns whether one
    /// opened: `false` when the highlighted item holds none.
    pub fn open_submenu(&mut self) -> bool {
        let menu = self.innermost_mut();
        menu.highlighted
            .is_some_and(|index| menu.open_own_submenu(index, true))
    }

    /// Closes the innermost open submenu, leaving the highlight on the
    /// item that held it. Returns whether one closed: `false` when no
    /// submenu is open.
    pub fn close_submenu(&mut self) -> bool {
        let Some(submenu) = &mut self.submenu else {
            return false;
        };
        if !submenu.menu.close_submenu() {
            self.hover_switch = None;
            self.submenu = None;
        }
        true
    }

    /// The innermost open submenu, or this menu when none is open.
    fn innermost(&self) -> &Self {
        match &self.submenu {
            Some(submenu) => submenu.menu.innermost(),
            None => self,
        }
    }

    /// The innermost open submenu, or this menu when none is open.
    fn innermost_mut(&mut self) -> &mut Self {
        match self.submenu {
            Some(ref mut submenu) => submenu.menu.innermost_mut(),
            None => self,
        }
    }

    /// Opens the submenu of entry `index` of this menu, closing any other,
    /// with its first item highlighted when `from_keys`. Returns whether
    /// the entry is an enabled item holding a submenu.
    fn open_own_submenu(&mut self, index: usize, from_keys: bool) -> bool {
        let Some(MenuEntry::Item(item)) = self.entries.get(index) else {
            return false;
        };
        let Some(entries) = item.submenu.as_ref().filter(|_| item.enabled) else {
            return false;
        };
        self.hover_switch = None;
        self.highlighted = Some(index);
        if self
            .submenu
            .as_ref()
            .is_some_and(|submenu| submenu.entry == index)
        {
            return true;
        }
        let mut menu = Menu::new(entries.clone());
        let anchor = self.submenu_anchor(index).unwrap_or_default();
        menu.open(anchor, PopupSide::Right);
        menu.pointer = self.pointer;
        if from_keys {
            menu.move_own_highlight(1);
        }
        self.submenu = Some(Box::new(Submenu { entry: index, menu }));
        true
    }

    /// What the submenu of entry `index` opens beside, as last drawn: the
    /// entry's row across the whole box, so the submenu's first item lines
    /// up with it.
    fn submenu_anchor(&self, index: usize) -> Option<Rect> {
        let layout = self.layout.as_ref()?;
        let row = layout.rows.get(index)?;
        let box_rect = layout.popup.rect();
        // The first row sits as far below the box's top as the submenu's
        // first row will below its own.
        let above_rows = layout.rows.first()?.y - box_rect.y;
        Some(Rect::new(
            box_rect.x,
            row.y - above_rows,
            box_rect.width,
            row.height,
        ))
    }

    /// Whether `point` is over this menu or one of its open submenus, as
    /// last drawn.
    fn contains(&self, point: Point) -> bool {
        let over_box = self
            .layout
            .as_ref()
            .is_some_and(|layout| layout.popup.rect().contains(point));
        over_box
            || self
                .submenu
                .as_ref()
                .is_some_and(|submenu| submenu.menu.contains(point))
    }

    /// Tells this menu and its open submenus where the pointer is.
    fn set_pointer(&mut self, point: Option<Point>) {
        self.pointer = point;
        if let Some(submenu) = &mut self.submenu {
            submenu.menu.set_pointer(point);
        }
    }

    /// Follows the pointer resting at its new place at `now`: highlights
    /// the item under it, and schedules opening that item's submenu, or
    /// closing the open one for another item, once it has rested a moment,
    /// asking a redraw for then. Over an open submenu, the submenu follows
    /// it instead.
    fn follow_pointer(&mut self, cx: &mut EventCx<'_>, now: Instant) {
        self.settle_hover(now);
        if let Some(submenu) = &mut self.submenu
            && self
                .pointer
                .is_some_and(|point| submenu.menu.contains(point))
        {
            self.hover_switch = None;
            if self.highlighted != Some(submenu.entry) {
                self.highlighted = Some(submenu.entry);
                cx.request_redraw();
            }
            submenu.menu.follow_pointer(cx, now);
            return;
        }
        let Some(index) = self.entry_under_pointer() else {
            return;
        };
        if self.highlighted != Some(index) {
            self.highlighted = Some(index);
            cx.request_redraw();
        }
        let open = self.submenu.as_ref().map(|submenu| submenu.entry);
        let switches = open != Some(index) && (open.is_some() || self.has_submenu(index));
        match switches {
            true if self.hover_switch.map(|switch| switch.entry) != Some(index) => {
                let at = now + SUBMENU_DELAY;
                self.hover_switch = Some(HoverSwitch { at, entry: index });
                cx.request_redraw_at(at);
            }
            true => {}
            false => self.hover_switch = None,
        }
    }

    /// Makes the submenu switch scheduled by the pointer when it is due
    /// at `now` and the pointer still rests on its entry, here and in the
    /// open submenus.
    fn settle_hover(&mut self, now: Instant) {
        if let Some(switch) = self.hover_switch
            && now >= switch.at
        {
            self.hover_switch = None;
            if self.highlighted == Some(switch.entry) && !self.open_own_submenu(switch.entry, false)
            {
                self.submenu = None;
            }
        }
        if let Some(submenu) = &mut self.submenu {
            submenu.menu.settle_hover(now);
        }
    }

    /// Whether entry `index` is an enabled item holding a submenu.
    fn has_submenu(&self, index: usize) -> bool {
        match self.entries.get(index) {
            Some(MenuEntry::Item(item)) => item.enabled && item.submenu.is_some(),
            Some(MenuEntry::Heading(_) | MenuEntry::Separator) | None => false,
        }
    }

    /// A press of `key` while the menu is open: Escape closes, Up and Down
    /// move the highlight, Enter chooses it, Right or `l` opens a submenu
    /// and Left or `h` closes one; any other key is the app's. An open
    /// submenu takes the key first, and Escape there closes only it.
    fn press_key(&mut self, cx: &mut EventCx<'_>, key: Key) -> MenuResponse<I> {
        if key.mods != Mods::NONE {
            return MenuResponse::Ignored;
        }
        if let Some(submenu) = &mut self.submenu {
            let response = match submenu.menu.press_key(cx, key) {
                MenuResponse::Activated(id) => {
                    self.close();
                    MenuResponse::Activated(id)
                }
                MenuResponse::Closed => {
                    self.submenu = None;
                    MenuResponse::Handled
                }
                MenuResponse::Handled => MenuResponse::Handled,
                MenuResponse::Ignored if matches!(key.code, KeyCode::Left | KeyCode::Char('h')) => {
                    self.submenu = None;
                    MenuResponse::Handled
                }
                MenuResponse::Ignored => MenuResponse::Ignored,
            };
            if !response.is_ignored() {
                cx.request_redraw();
            }
            return response;
        }
        let response = match key.code {
            KeyCode::Down => {
                self.move_own_highlight(1);
                MenuResponse::Handled
            }
            KeyCode::Enter => match self.choose() {
                Some(id) => MenuResponse::Activated(id),
                None => MenuResponse::Handled,
            },
            KeyCode::Esc => {
                self.close();
                MenuResponse::Closed
            }
            KeyCode::Right | KeyCode::Char('l') => match self.open_submenu() {
                true => MenuResponse::Handled,
                false => return MenuResponse::Ignored,
            },
            KeyCode::Up => {
                self.move_own_highlight(-1);
                MenuResponse::Handled
            }
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::End
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Tab => return MenuResponse::Ignored,
        };
        cx.request_redraw();
        response
    }

    /// Describes the open menu as last drawn to screen readers as `id`: a
    /// menu holding its items and headings (the `index`th entry is the
    /// `index`th child of `id`), each named by its label, an item
    /// clickable unless disabled; a
    /// checked or radio item says whether it is on, and an item holding a
    /// submenu whether it is expanded, holding the open submenu's menu as
    /// its child (child 0 of the item's node). Nothing while the menu is
    /// closed.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        self.describe(frame, id, id);
    }

    /// [`Menu::access`] for this menu as `id`, inside the menu described
    /// as `owner`, whose focus the innermost highlighted item takes.
    fn describe(&self, frame: &mut Frame, id: FocusId, owner: FocusId) {
        let (Some(_), Some(layout)) = (self.opened_at, &self.layout) else {
            return;
        };
        frame.enter_access(id, layout.popup.rect(), Node::new(Role::Menu));
        for (index, (entry, row)) in self.entries.iter().zip(&layout.rows).enumerate() {
            let item = match entry {
                MenuEntry::Item(item) => item,
                MenuEntry::Heading(label) => {
                    let mut node = Node::new(Role::Heading);
                    node.set_label(label.as_str());
                    frame.access(id.child(index as u64), *row, node);
                    continue;
                }
                MenuEntry::Separator => continue,
            };
            let role = match item.mark {
                Mark::Check(_) => Role::MenuItemCheckBox,
                Mark::Plain => Role::MenuItem,
                Mark::Radio(_) => Role::MenuItemRadio,
            };
            let mut node = Node::new(role);
            node.set_label(item.label.as_str());
            if let Mark::Check(on) | Mark::Radio(on) = item.mark {
                node.set_toggled(on.into());
            }
            match item.enabled {
                true => node.add_action(Action::Click),
                false => node.set_disabled(),
            }
            let item_id = id.child(index as u64);
            let open = self
                .submenu
                .as_ref()
                .filter(|submenu| submenu.entry == index);
            if item.submenu.is_some() {
                node.set_has_popup(HasPopup::Menu);
                node.set_expanded(open.is_some());
            }
            frame.enter_access(item_id, *row, node);
            if let Some(submenu) = open {
                submenu
                    .menu
                    .describe(frame, item_id.child(SUBMENU_NODE), owner);
            }
            frame.leave_access();
        }
        frame.leave_access();
        if let (Some(index), None) = (self.highlighted, &self.submenu) {
            frame.access.set_inner_focus(owner, id.child(index as u64));
        }
    }

    /// Does what a screen reader's `action` on the menu described as `id`
    /// asks: a click on an enabled item chooses it, closing the menu, and
    /// a click on an item holding a submenu opens it. Ignored for other
    /// actions and nodes, and while the menu is closed.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> MenuResponse<I> {
        let AccessAction::Click(target) = action else {
            return MenuResponse::Ignored;
        };
        if !self.is_open() {
            return MenuResponse::Ignored;
        }
        let response = self.click_node(id, target);
        if let MenuResponse::Activated(_) = response {
            self.close();
        }
        response
    }

    /// A screen reader's click on the node `target` of this menu described
    /// as `id`, or of its open submenus: the id of the enabled item
    /// clicked, or a submenu opened.
    fn click_node(&mut self, id: FocusId, target: FocusId) -> MenuResponse<I> {
        if let Some(index) = (0..self.entries.len()).find(|index| id.child(*index as u64) == target)
        {
            if self.open_own_submenu(index, true) {
                return MenuResponse::Handled;
            }
            return match self.item_id(index) {
                Some(item) => MenuResponse::Activated(item),
                None => MenuResponse::Ignored,
            };
        }
        match &mut self.submenu {
            Some(submenu) => {
                let item_id = id.child(submenu.entry as u64);
                submenu.menu.click_node(item_id.child(SUBMENU_NODE), target)
            }
            None => MenuResponse::Ignored,
        }
    }

    /// A press of `button`: choose the enabled item under the pointer (or
    /// open its submenu), do nothing on a disabled item or separator, or
    /// close when outside the menu and its submenus. A press over an open
    /// submenu is the submenu's.
    fn press(&mut self, cx: &mut EventCx<'_>, button: PointerButton) -> MenuResponse<I> {
        let inside = self.pointer.is_some_and(|point| self.contains(point));
        if !inside {
            self.close();
            cx.request_redraw();
            return MenuResponse::Closed;
        }
        if let Some(submenu) = &mut self.submenu
            && self
                .pointer
                .is_some_and(|point| submenu.menu.contains(point))
        {
            return match submenu.menu.press(cx, button) {
                MenuResponse::Activated(id) => {
                    self.close();
                    MenuResponse::Activated(id)
                }
                response @ (MenuResponse::Closed
                | MenuResponse::Handled
                | MenuResponse::Ignored) => response,
            };
        }
        let under = match button {
            PointerButton::Left => self.entry_under_pointer(),
            PointerButton::Back
            | PointerButton::Forward
            | PointerButton::Middle
            | PointerButton::Other(_)
            | PointerButton::Right => None,
        };
        let Some(index) = under else {
            return MenuResponse::Handled;
        };
        if self.open_own_submenu(index, false) {
            cx.request_redraw();
            return MenuResponse::Handled;
        }
        match self.item_id(index) {
            Some(id) => {
                self.close();
                cx.request_redraw();
                MenuResponse::Activated(id)
            }
            None => MenuResponse::Handled,
        }
    }

    /// Scrolls the menu or open submenu under the pointer by `wheel`, if
    /// it is taller than the window, closing the submenus of the one that
    /// scrolled, since their items would move away from them.
    fn wheel(&mut self, cx: &mut EventCx<'_>, wheel: &WheelInput) {
        if let Some(submenu) = &mut self.submenu
            && submenu.menu.contains(wheel.position)
        {
            submenu.menu.wheel(cx, wheel);
            return;
        }
        let Some(layout) = self
            .layout
            .as_ref()
            .filter(|layout| layout.popup.rect().contains(wheel.position))
        else {
            return;
        };
        let delta = match wheel.delta {
            WheelDelta::Lines(lines) => -lines.y * WHEEL_ITEMS * layout.item_height,
            WheelDelta::Pixels(pixels) => -pixels.y,
        };
        let scroll = (self.scroll + delta).clamp(0.0, layout.overflow);
        if scroll != self.scroll {
            self.scroll = scroll;
            self.submenu = None;
            self.hover_switch = None;
            cx.request_redraw();
        }
    }

    /// The enabled item under the pointer, as last drawn.
    fn entry_under_pointer(&self) -> Option<usize> {
        let point = self.pointer?;
        let index = self.layout.as_ref()?.entry_at(point)?;
        is_enabled(&self.entries[index]).then_some(index)
    }

    /// The id of the entry at `index` when it is an enabled item that can
    /// be chosen: not one holding a submenu.
    fn item_id(&self, index: usize) -> Option<I> {
        match self.entries.get(index)? {
            MenuEntry::Item(item) if item.enabled && item.submenu.is_none() => Some(item.id),
            MenuEntry::Heading(_) | MenuEntry::Item(_) | MenuEntry::Separator => None,
        }
    }
}

impl<I: Copy> Widget for Menu<I> {
    type Output = Option<Rect>;
    type Response = MenuResponse<I>;

    /// Reacts to `input` while the menu is open, as the module
    /// documentation lists: hover highlights and, after a moment, opens
    /// submenus; a left press chooses an item or opens its submenu; a press
    /// outside or Escape closes; Up, Down, Enter, Right, and Left (or `l`
    /// and `h`) drive the highlight and the submenus. Every pointer input
    /// is the menu's while it is open. A closed menu ignores everything.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> MenuResponse<I> {
        if let Some(position) = input.position() {
            self.set_pointer(Some(position));
        }
        if !self.is_open() {
            return MenuResponse::Ignored;
        }
        match input {
            Input::Key(key) => self.press_key(cx, *key),
            Input::PointerButton(button) if button.pressed => self.press(cx, button.button),
            Input::PointerLeft => {
                self.set_pointer(None);
                MenuResponse::Handled
            }
            Input::PointerMoved(_) => {
                self.follow_pointer(cx, clock::now());
                MenuResponse::Handled
            }
            Input::Wheel(wheel) => {
                self.wheel(cx, wheel);
                MenuResponse::Handled
            }
            Input::PointerButton(_) => MenuResponse::Handled,
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_) => MenuResponse::Ignored,
        }
    }

    /// Draws the open menu in the frame's overlay beside its anchor, kept
    /// inside `area` (usually the whole window), in `theme`'s colors:
    /// the highlighted item on the selection color, disabled items and
    /// accelerators muted, and the open submenu beside its item, over the
    /// menu. Opens or closes the submenu the pointer has rested long
    /// enough for (the menu asks for a redraw then). Returns where the
    /// menu itself was drawn (for a [`FocusTree`](crate::FocusTree)
    /// trap, which hands the menu every pointer input, its submenus'
    /// included); a closed menu draws nothing and returns `None`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Option<Rect> {
        let anchor = self.opened_at?;
        self.settle_hover(clock::now());
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let mut layout = MenuLayout::new(
            &self.entries,
            anchor,
            area,
            metrics,
            &theme.shape,
            scale_factor,
            |text| label_width(frame, theme, text),
        );
        let reveal = self.highlighted.filter(|_| self.reveal);
        self.reveal = false;
        let scroll = match reveal {
            Some(index) => {
                // The headings right above the item show with it.
                let headings = self.entries[..index]
                    .iter()
                    .rev()
                    .take_while(|entry| matches!(entry, MenuEntry::Heading(_)))
                    .count();
                layout.revealing(index - headings..=index, self.scroll)
            }
            None => self.scroll,
        };
        layout.scroll_to(scroll);
        self.scroll = layout.scroll;
        layout.popup.draw(frame, theme);
        let padding = metrics.width.round();
        let stroke = logical(SEPARATOR_STROKE, scale_factor);
        let text_top = |row: Rect| row.y + ((row.height - metrics.height) / 2.0).round();
        let muted = TextColor::Token(ThemeToken::Muted);
        let draw_rows = |frame: &mut Frame| {
            for (index, (entry, &row)) in self.entries.iter().zip(&layout.rows).enumerate() {
                let item = match entry {
                    MenuEntry::Item(item) => item,
                    MenuEntry::Heading(label) => {
                        let origin = Point::new(row.x + padding, text_top(row));
                        let heading = Span::new(label.as_str(), muted).emphasized(theme.emphasis);
                        let heading = StyledText::from(heading);
                        draw_styled_label(frame, theme, &heading, origin, row);
                        continue;
                    }
                    MenuEntry::Separator => {
                        let y = row.y + ((row.height - stroke) / 2.0).round();
                        let line = Rect::new(row.x + padding, y, row.width - 2.0 * padding, stroke);
                        frame.fill_rect_over_text(line, theme.border);
                        continue;
                    }
                };
                if self.highlighted == Some(index) {
                    let scale_factor = frame.scale_factor();
                    frame.draw_box(
                        theme.shape.highlight_band(row, scale_factor),
                        &theme.shape.highlight(theme.selected_row, scale_factor),
                    );
                }
                let color = match item.enabled {
                    true => theme.foreground,
                    false => theme.muted,
                };
                let mark = match item.mark {
                    Mark::Check(true) => Some(CHECK_GLYPH),
                    Mark::Radio(true) => Some(RADIO_GLYPH),
                    Mark::Check(false) | Mark::Plain | Mark::Radio(false) => None,
                };
                if let Some(mark) = mark {
                    let origin = Point::new(row.x + padding, text_top(row));
                    draw_label(frame, theme, mark, origin, color);
                }
                let origin = Point::new(row.x + padding + layout.gutter, text_top(row));
                draw_label(frame, theme, &item.label, origin, color);
                // The submenu's arrow is muted like an accelerator: both say
                // what an item leads to, not what it is.
                let hint = match item.submenu {
                    Some(_) => Some(SUBMENU_GLYPH),
                    None => item.accelerator.as_deref(),
                };
                if let Some(hint) = hint {
                    let width = label_width(frame, theme, hint);
                    let x = row.x + row.width - padding - width;
                    let origin = Point::new(x, text_top(row));
                    draw_label(frame, theme, hint, origin, theme.muted);
                }
            }
        };
        // Only a menu that scrolls is clipped, so one that fits draws as
        // it always did.
        frame.overlay(|frame| match layout.overflow > 0.0 {
            true => frame.clipped(layout.viewport, draw_rows),
            false => draw_rows(frame),
        });
        let rect = layout.popup.rect();
        self.layout = Some(layout);
        if let Some(entry) = self.submenu.as_ref().map(|submenu| submenu.entry) {
            let anchor = self.submenu_anchor(entry).unwrap_or_default();
            if let Some(submenu) = &mut self.submenu {
                submenu.menu.opened_at = Some(MenuAnchor {
                    rect: anchor,
                    side: PopupSide::Right,
                });
                submenu.menu.draw(frame, area, theme);
            }
        }
        Some(rect)
    }
}

impl<I> Outcome for MenuResponse<I> {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// MenuItem Implementations
// ========================================================================

impl<I> MenuItem<I> {
    /// An enabled item labelled `label` that hands back `id`.
    pub fn new(id: I, label: impl Into<String>) -> Self {
        Self {
            accelerator: None,
            enabled: true,
            id,
            label: label.into(),
            mark: Mark::Plain,
            submenu: None,
        }
    }

    /// This item as an option turned on or off, with a check mark left of
    /// its label while `checked`. Choosing it hands back its id as usual;
    /// the app turns the option over and rebuilds the menu.
    pub fn with_checked(self, checked: bool) -> Self {
        Self {
            mark: Mark::Check(checked),
            ..self
        }
    }

    /// This item as one of a few options of which one is chosen, with a
    /// dot left of its label while `selected`. Choosing it hands back its
    /// id as usual; the app moves the choice and rebuilds the menu.
    pub fn with_radio(self, selected: bool) -> Self {
        Self {
            mark: Mark::Radio(selected),
            ..self
        }
    }

    /// This item opening a submenu of `entries` beside it, rather than
    /// being chosen: its id is never handed back, and an accelerator gives
    /// way to the submenu's arrow.
    pub fn with_submenu(self, entries: Vec<MenuEntry<I>>) -> Self {
        Self {
            submenu: Some(entries),
            ..self
        }
    }

    /// This item showing `accelerator`, such as `"Ctrl+T"`, as the keys
    /// that run it.
    pub fn with_accelerator(self, accelerator: impl Into<String>) -> Self {
        Self {
            accelerator: Some(accelerator.into()),
            ..self
        }
    }

    /// This item enabled or not; a disabled item is shown muted and cannot
    /// be chosen.
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }
}

impl<I> From<MenuItem<I>> for MenuEntry<I> {
    fn from(item: MenuItem<I>) -> Self {
        Self::Item(item)
    }
}

// ========================================================================
// MenuLayout Implementations
// ========================================================================

impl MenuLayout {
    /// The layout of `entries` opened at `anchor` inside `window`, in cells
    /// of `metrics` and a box of `shape` on a display scaled by
    /// `scale_factor`, each label as wide as `measure` says in pixels.
    fn new<I>(
        entries: &[MenuEntry<I>],
        anchor: MenuAnchor,
        window: Rect,
        metrics: CellMetrics,
        shape: &Shape,
        scale_factor: f64,
        measure: impl Fn(&str) -> f32,
    ) -> Self {
        // Scaled straight, not by `logical`, which keeps a pixel even of none.
        let row_padding = (f32::from(shape.row_padding) * scale_factor as f32).round();
        let item_height = (metrics.height * ITEM_HEIGHT_RATIO).round() + 2.0 * row_padding;
        let separator_height =
            (metrics.height * SEPARATOR_HEIGHT_RATIO).round() + 2.0 * row_padding;
        let padding_y = logical(PADDING_Y, scale_factor);
        let inset = Popup::inset(shape, scale_factor);
        let items = entries.iter().filter_map(|entry| match entry {
            MenuEntry::Item(item) => Some(item),
            MenuEntry::Heading(_) | MenuEntry::Separator => None,
        });
        let headings = entries.iter().filter_map(|entry| match entry {
            MenuEntry::Heading(label) => Some(label.as_str()),
            MenuEntry::Item(_) | MenuEntry::Separator => None,
        });
        let label_width = items
            .clone()
            .map(|item| measure(&item.label))
            .chain(headings.map(&measure))
            .fold(0.0, f32::max);
        let gutter = match items.clone().any(|item| item.mark != Mark::Plain) {
            true => (MARK_CELLS * metrics.width).round(),
            false => 0.0,
        };
        let accelerator_width = items
            .filter_map(|item| match item.submenu {
                Some(_) => Some(SUBMENU_GLYPH),
                None => item.accelerator.as_deref(),
            })
            .map(&measure)
            .reduce(f32::max)
            .map_or(0.0, |width| {
                width + ACCELERATOR_GAP_CELLS as f32 * metrics.width
            });
        let text_width =
            (gutter + label_width + accelerator_width).max(MIN_TEXT_CELLS as f32 * metrics.width);
        let width = (text_width + 2.0 * metrics.width.round() + 2.0 * inset).ceil();
        let heights: Vec<f32> = entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Heading(_) | MenuEntry::Item(_) => item_height,
                MenuEntry::Separator => separator_height,
            })
            .collect();
        let height = heights.iter().sum::<f32>() + 2.0 * (padding_y + inset);
        // The popup moves as little as it can to stay inside the window,
        // and is cut to the window's height when taller: its rows then
        // scroll inside its padding.
        let popup = Popup::new(anchor.rect, width, height, anchor.side, window);
        let content = popup.content(shape, scale_factor);
        let viewport = Rect::new(
            content.x,
            content.y + padding_y,
            content.width,
            (content.height - 2.0 * padding_y).max(0.0),
        );
        let mut y = viewport.y;
        let rows: Vec<Rect> = heights
            .iter()
            .map(|&row_height| {
                let row = Rect::new(content.x, y, content.width, row_height);
                y += row_height;
                row
            })
            .collect();
        let overflow = (y - viewport.y - viewport.height).max(0.0);
        Self {
            gutter,
            item_height,
            overflow,
            popup,
            rows,
            scroll: 0.0,
            viewport,
        }
    }

    /// Scrolls the rows up by `scroll` pixels, held between 0 and how far
    /// they can scroll.
    fn scroll_to(&mut self, scroll: f32) {
        let scroll = scroll.clamp(0.0, self.overflow);
        let by = scroll - self.scroll;
        for row in &mut self.rows {
            row.y -= by;
        }
        self.scroll = scroll;
    }

    /// The scroll, from `scroll`, that shows the whole rows of the entries
    /// `entries`, moving as little as it can; their tops first when they
    /// are taller than the box.
    fn revealing(&self, entries: RangeInclusive<usize>, scroll: f32) -> f32 {
        let (Some(first), Some(last)) = (
            self.rows.get(*entries.start()),
            self.rows.get(*entries.end()),
        ) else {
            return scroll;
        };
        // Their top and bottom with the rows scrolled up by `scroll`.
        let top = first.y + self.scroll - scroll;
        let bottom = last.y + last.height + self.scroll - scroll;
        let view_bottom = self.viewport.y + self.viewport.height;
        if top < self.viewport.y {
            scroll - (self.viewport.y - top)
        } else if bottom > view_bottom {
            scroll + (bottom - view_bottom).min(top - self.viewport.y)
        } else {
            scroll
        }
    }

    /// The entry whose row holds `point`, inside the part of the box the
    /// rows show in.
    fn entry_at(&self, point: Point) -> Option<usize> {
        if !self.viewport.contains(point) {
            return None;
        }
        self.rows.iter().position(|row| row.contains(point))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `entry` is an item that can be chosen.
fn is_enabled<I>(entry: &MenuEntry<I>) -> bool {
    match entry {
        MenuEntry::Item(item) => item.enabled,
        MenuEntry::Heading(_) | MenuEntry::Separator => false,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::find_node;
    use crate::app::offscreen;
    use crate::base::text;
    use crate::base::theme::{LabelFont, Theme};
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};

    const METRICS: CellMetrics = CellMetrics {
        baseline: 15.0,
        height: 20.0,
        width: 10.0,
    };

    const WINDOW: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    /// `Open` (Ctrl+O), a separator, `Save` (disabled), `Quit`.
    fn entries() -> Vec<MenuEntry<u8>> {
        vec![
            MenuItem::new(1, "Open").with_accelerator("Ctrl+O").into(),
            MenuEntry::Separator,
            MenuItem::new(2, "Save").with_enabled(false).into(),
            MenuItem::new(3, "Quit").into(),
        ]
    }

    fn open_menu() -> Menu<u8> {
        let mut menu = Menu::new(entries());
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        menu
    }

    fn layout() -> MenuLayout {
        let anchor = MenuAnchor {
            rect: Rect::new(10.0, 0.0, 30.0, 30.0),
            side: PopupSide::Below,
        };
        MenuLayout::new(
            &entries(),
            anchor,
            WINDOW,
            METRICS,
            &Shape::GRID,
            1.0,
            |text| text::width(text) as f32 * METRICS.width,
        )
    }

    #[test]
    fn test_the_highlight_skips_separators_and_disabled_items_and_wraps() {
        let mut menu = open_menu();
        menu.move_highlight(1);
        assert_eq!(menu.highlighted(), Some(1));
        menu.move_highlight(1);
        assert_eq!(menu.highlighted(), Some(3), "past the separator and Save");
        menu.move_highlight(1);
        assert_eq!(menu.highlighted(), Some(1), "wrapped to the top");
        menu.move_highlight(-1);
        assert_eq!(menu.highlighted(), Some(3), "wrapped to the bottom");
    }

    #[test]
    fn test_a_first_step_up_highlights_the_last_item() {
        let mut menu = open_menu();
        menu.move_highlight(-1);
        assert_eq!(menu.highlighted(), Some(3));
    }

    #[test]
    fn test_choosing_closes_only_when_an_item_is_highlighted() {
        let mut menu = open_menu();
        assert_eq!(menu.choose(), None);
        assert!(menu.is_open());
        menu.move_highlight(2);
        assert_eq!(menu.choose(), Some(3));
        assert!(!menu.is_open());
    }

    #[test]
    fn test_the_menu_is_as_wide_as_its_longest_label_and_accelerator() {
        let layout = layout();
        // 16 cells at least (4 + 3 + 6 = 13 here), 2 padding, 2 border.
        assert_eq!(layout.popup.rect().width, 182.0);
        // Three 30 pixel items, a 10 pixel separator, padding and border.
        assert_eq!(layout.popup.rect().height, 110.0);
        assert_eq!(layout.popup.rect().y, 30.0, "under the anchor");
    }

    #[test]
    fn test_a_shapes_row_padding_heightens_items_and_separators_alike() {
        let anchor = MenuAnchor {
            rect: Rect::new(10.0, 0.0, 30.0, 30.0),
            side: PopupSide::Below,
        };
        let roomy = Shape {
            row_padding: 4,
            ..Shape::GRID
        };
        let layout = MenuLayout::new(&entries(), anchor, WINDOW, METRICS, &roomy, 1.0, |text| {
            text::width(text) as f32 * METRICS.width
        });
        let heights: Vec<f32> = layout.rows.iter().map(|row| row.height).collect();
        assert_eq!(
            heights,
            [38.0, 18.0, 38.0, 38.0],
            "each plus 4 above and below"
        );
    }

    #[test]
    fn test_rows_stack_inside_the_padding_and_are_hit_by_point() {
        let layout = layout();
        let top = layout.rows[0].y;
        assert_eq!(top, 35.0, "border 1 and padding 4 under y 30");
        assert_eq!(layout.rows[1].y, top + 30.0);
        assert_eq!(layout.rows[2].y, top + 40.0);
        assert_eq!(layout.entry_at(Point::new(50.0, top + 1.0)), Some(0));
        assert_eq!(layout.entry_at(Point::new(50.0, top + 35.0)), Some(1));
        assert_eq!(layout.entry_at(Point::new(50.0, top + 75.0)), Some(3));
        assert_eq!(layout.entry_at(Point::new(50.0, 31.0)), None, "padding");
    }

    #[test]
    fn test_an_open_menu_paints_a_bordered_box_and_its_highlight() {
        let theme = Theme::dark().with_grid_look();
        let mut menu = Menu::new(entries());
        menu.open(Rect::new(20.0, 20.0, 0.0, 0.0), PopupSide::Below);
        menu.move_highlight(1);
        let Some(snapshot) = offscreen::render(320, 240, |frame| {
            menu.draw(frame, WINDOW, &theme);
        }) else {
            return;
        };
        let layout = menu.layout.as_ref().expect("drawn");
        let rect = layout.popup.rect();
        snapshot.assert_pixel(
            rect.x,
            rect.y + rect.height / 2.0,
            theme.border,
            "the border",
        );
        let (open, quit) = (layout.rows[0], layout.rows[3]);
        let right = open.x + open.width - 1.0;
        snapshot.assert_pixel(right, open.y + 1.0, theme.selection, "the highlight");
        snapshot.assert_pixel(right, quit.y + 1.0, theme.raised, "an item");
    }

    #[test]
    fn test_the_menu_describes_its_items_and_a_click_chooses_an_enabled_one() {
        let id = FocusId::new("menu");
        let mut menu = open_menu();
        let mut frame = offscreen::plain_frame(800, 600);
        menu.draw(&mut frame, WINDOW, &Theme::dark());
        menu.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let node = |id| find_node(&update, id).expect("described");
        assert_eq!(node(id).role(), Role::Menu);
        assert_eq!(node(id).children().len(), 3, "no node for the separator");
        let save = node(id.child(2));
        assert_eq!(
            (save.role(), save.label(), save.is_disabled()),
            (Role::MenuItem, Some("Save"), true)
        );
        assert_eq!(
            menu.access_action(id, AccessAction::Click(id.child(2))),
            MenuResponse::Ignored,
            "a disabled item is not chosen"
        );
        assert_eq!(
            menu.access_action(id, AccessAction::Click(id.child(3))),
            MenuResponse::Activated(3)
        );
        assert!(!menu.is_open());
    }

    /// An open menu drawn in a test window, so it knows where its rows are.
    fn drawn_menu() -> (TestWindow, Menu<u8>) {
        let window = TestWindow::new(800, 600);
        let mut menu = open_menu();
        let mut frame = window.frame();
        menu.draw(&mut frame, WINDOW, &Theme::dark());
        (window, menu)
    }

    /// The input of pressing `code` with no modifier.
    fn key(code: KeyCode) -> Input {
        Input::Key(Key::new(code, Mods::NONE))
    }

    /// A hamburger menu of two sections: `File` over `Open` and `Quit`,
    /// then `Edit` over `Undo`.
    fn sectioned() -> Menu<u8> {
        let mut menu = Menu::new(vec![
            MenuEntry::Heading("File".into()),
            MenuItem::new(1, "Open").into(),
            MenuItem::new(2, "Quit").into(),
            MenuEntry::Heading("Edit".into()),
            MenuItem::new(3, "Undo").into(),
        ]);
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        menu
    }

    #[test]
    fn test_headings_are_skipped_by_keys_and_the_pointer_and_read_as_headings() {
        let id = FocusId::new("menu");
        let mut window = TestWindow::new(800, 600);
        let mut menu = sectioned();
        let mut frame = window.frame();
        menu.draw(&mut frame, WINDOW, &Theme::dark());
        menu.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let edit = find_node(&update, id.child(3)).expect("described");
        assert_eq!((edit.role(), edit.label()), (Role::Heading, Some("Edit")));
        let mut press = |menu: &mut Menu<u8>, code| {
            window.send(&key(code), |input, cx| menu.handle(input, cx));
            menu.highlighted()
        };
        assert_eq!(
            press(&mut menu, KeyCode::Down),
            Some(1),
            "past the File heading"
        );
        press(&mut menu, KeyCode::Down);
        assert_eq!(
            press(&mut menu, KeyCode::Down),
            Some(3),
            "past the Edit heading"
        );
        assert_eq!(press(&mut menu, KeyCode::Up), Some(2));
        let heading = menu.layout.as_ref().expect("drawn").rows[3];
        let over = Point::new(heading.x + 5.0, heading.y + 5.0);
        window.send(&Input::PointerMoved(over), |input, cx| {
            menu.handle(input, cx)
        });
        assert_eq!(menu.highlighted(), Some(2), "a heading takes no highlight");
        let click = Input::PointerButton(ButtonInput::press(PointerButton::Left, over));
        let clicked = window.send(&click, |input, cx| menu.handle(input, cx));
        assert_eq!(clicked, MenuResponse::Handled, "nor a choice");
        assert!(menu.is_open());
        let read = menu.access_action(id, AccessAction::Click(id.child(0)));
        assert_eq!(read, MenuResponse::Ignored);
    }

    #[test]
    fn test_hovering_highlights_and_a_click_chooses_the_item_under_the_pointer() {
        let (mut window, mut menu) = drawn_menu();
        let quit = menu.layout.as_ref().expect("drawn").rows[3];
        let over = Point::new(quit.x + 5.0, quit.y + 5.0);
        let moved = window.send(&Input::PointerMoved(over), |input, cx| {
            menu.handle(input, cx)
        });
        assert_eq!(moved, MenuResponse::Handled);
        assert_eq!(menu.highlighted(), Some(3));
        assert!(window.take_redraw());
        let click = Input::PointerButton(ButtonInput::press(PointerButton::Left, over));
        let chosen = window.send(&click, |input, cx| menu.handle(input, cx));
        assert_eq!(chosen, MenuResponse::Activated(3));
        assert!(!menu.is_open());
    }

    #[test]
    fn test_up_down_and_enter_drive_the_highlight_and_escape_or_a_click_outside_closes() {
        let (mut window, mut menu) = drawn_menu();
        let mut send = |input: Input, menu: &mut Menu<u8>| {
            window.send(&input, |input, cx| menu.handle(input, cx))
        };
        send(key(KeyCode::Down), &mut menu);
        send(key(KeyCode::Down), &mut menu);
        assert_eq!(menu.highlighted(), Some(3), "Save is disabled");
        assert_eq!(
            send(key(KeyCode::Char('q')), &mut menu),
            MenuResponse::Ignored
        );
        assert_eq!(
            send(key(KeyCode::Enter), &mut menu),
            MenuResponse::Activated(3)
        );
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        assert_eq!(send(key(KeyCode::Esc), &mut menu), MenuResponse::Closed);
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        let outside = ButtonInput::press(PointerButton::Left, Point::new(700.0, 500.0));
        assert_eq!(
            send(Input::PointerButton(outside), &mut menu),
            MenuResponse::Closed
        );
        assert_eq!(
            send(key(KeyCode::Down), &mut menu),
            MenuResponse::Ignored,
            "closed"
        );
    }

    #[test]
    fn test_a_modern_menu_rounds_its_highlight_inside_the_rounded_box() {
        let theme = Theme {
            shape: Shape::MODERN,
            ..Theme::dark()
        };
        let mut menu = Menu::new(entries());
        menu.open(Rect::new(20.0, 20.0, 0.0, 0.0), PopupSide::Below);
        menu.move_highlight(1);
        let Some(snapshot) = offscreen::render(320, 240, |frame| {
            menu.draw(frame, WINDOW, &theme);
        }) else {
            return;
        };
        let layout = menu.layout.as_ref().expect("drawn");
        let open = layout.rows[0];
        // The modern shape insets the highlight 4 pixels from the row's side.
        let right = open.x + open.width - 5.0;
        let middle = open.y + open.height / 2.0;
        snapshot.assert_pixel(right, middle, theme.selection, "the highlight's edge");
        snapshot.assert_pixel(open.x, open.y, theme.raised, "its rounded-off corner");
    }

    #[test]
    fn test_a_proportional_menu_fits_its_labels_shapes() {
        let entries = || -> Vec<MenuEntry<u8>> {
            vec![MenuItem::new(1, "illuminating little lists in italic").into()]
        };
        let width = |label_font| {
            let theme = Theme {
                label_font,
                ..Theme::dark()
            };
            let mut menu = Menu::new(entries());
            menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
            let mut frame = offscreen::plain_frame(800, 600);
            menu.draw(&mut frame, WINDOW, &theme);
            menu.layout.as_ref().expect("drawn").popup.rect().width
        };
        let (cells, proportional) = (width(LabelFont::Cells), width(LabelFont::Proportional));
        assert!(
            proportional < cells,
            "narrow letters take less room: {proportional} against {cells}"
        );
    }

    #[test]
    fn test_a_proportional_menu_paints_its_labels() {
        let theme = Theme {
            label_font: LabelFont::Proportional,
            ..Theme::dark()
        };
        let mut menu = open_menu();
        let Some(snapshot) = offscreen::render(320, 240, |frame| {
            menu.draw(frame, WINDOW, &theme);
        }) else {
            return;
        };
        let open = menu.layout.as_ref().expect("drawn").rows[0];
        assert!(
            snapshot.contains_color(open, theme.foreground),
            "the label's glyphs in the foreground color"
        );
    }

    /// `View` holding a submenu of `Grid` (radio, on) and `List` (radio),
    /// then `Wrap` (checked) and `Quit`.
    fn nested_entries() -> Vec<MenuEntry<u8>> {
        vec![
            MenuItem::new(10, "View")
                .with_submenu(vec![
                    MenuItem::new(11, "Grid").with_radio(true).into(),
                    MenuItem::new(12, "List").with_radio(false).into(),
                ])
                .into(),
            MenuItem::new(20, "Wrap").with_checked(true).into(),
            MenuItem::new(3, "Quit").into(),
        ]
    }

    /// An open nested menu at `anchor`, drawn in a test window.
    fn drawn_nested(anchor: Rect) -> (TestWindow, Menu<u8>) {
        let window = TestWindow::new(800, 600);
        let mut menu = Menu::new(nested_entries());
        menu.open(anchor, PopupSide::Below);
        let mut frame = window.frame();
        menu.draw(&mut frame, WINDOW, &Theme::dark());
        (window, menu)
    }

    /// `menu` drawn again in `window`, as the next frame.
    fn redraw(window: &TestWindow, menu: &mut Menu<u8>) -> Frame {
        let mut frame = window.frame();
        menu.draw(&mut frame, WINDOW, &Theme::dark());
        frame
    }

    #[test]
    fn test_right_opens_a_submenu_left_and_escape_close_it_and_enter_chooses_inside() {
        let (mut window, mut menu) = drawn_nested(Rect::new(10.0, 0.0, 30.0, 30.0));
        let mut send = |input: Input, menu: &mut Menu<u8>| {
            window.send(&input, |input, cx| menu.handle(input, cx))
        };
        send(key(KeyCode::Down), &mut menu);
        assert_eq!(menu.highlighted(), Some(10), "a submenu item highlights");
        assert_eq!(send(key(KeyCode::Right), &mut menu), MenuResponse::Handled);
        assert_eq!(menu.highlighted(), Some(11), "the submenu's first item");
        send(key(KeyCode::Down), &mut menu);
        assert_eq!(menu.highlighted(), Some(12));
        assert_eq!(send(key(KeyCode::Left), &mut menu), MenuResponse::Handled);
        assert_eq!(menu.highlighted(), Some(10), "back on the item");
        assert_eq!(
            send(key(KeyCode::Left), &mut menu),
            MenuResponse::Ignored,
            "no submenu to close"
        );
        send(key(KeyCode::Char('l')), &mut menu);
        assert_eq!(send(key(KeyCode::Esc), &mut menu), MenuResponse::Handled);
        assert!(menu.is_open(), "Escape closed only the submenu");
        assert_eq!(
            send(key(KeyCode::Enter), &mut menu),
            MenuResponse::Handled,
            "Enter on a submenu item opens it"
        );
        assert_eq!(
            send(key(KeyCode::Enter), &mut menu),
            MenuResponse::Activated(11)
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn test_resting_on_a_submenu_item_opens_it_after_the_delay_and_another_item_closes_it() {
        let (mut window, mut menu) = drawn_nested(Rect::new(10.0, 0.0, 30.0, 30.0));
        let rows = menu.layout.as_ref().expect("drawn").rows.clone();
        let middle = |row: Rect| Point::new(row.x + 5.0, row.y + row.height / 2.0);
        window.feed(&mut menu, &Input::PointerMoved(middle(rows[0])));
        assert!(menu.submenu.is_none(), "not at once");
        menu.settle_hover(Instant::now() + SUBMENU_DELAY);
        assert!(menu.submenu.is_some(), "open after the delay");
        assert_eq!(menu.highlighted(), None, "nothing highlighted inside yet");
        window.feed(&mut menu, &Input::PointerMoved(middle(rows[2])));
        assert!(menu.submenu.is_some(), "kept open while the pointer passes");
        menu.settle_hover(Instant::now() + SUBMENU_DELAY);
        assert!(menu.submenu.is_none(), "closed for the other item");
    }

    #[test]
    fn test_a_click_opens_a_submenu_at_once_and_a_click_inside_it_chooses() {
        let (mut window, mut menu) = drawn_nested(Rect::new(10.0, 0.0, 30.0, 30.0));
        let view = menu.layout.as_ref().expect("drawn").rows[0];
        let click =
            |point: Point| Input::PointerButton(ButtonInput::press(PointerButton::Left, point));
        let on_view = Point::new(view.x + 5.0, view.y + 5.0);
        assert_eq!(
            window.feed(&mut menu, &click(on_view)),
            MenuResponse::Handled
        );
        redraw(&window, &mut menu);
        let submenu = &menu.submenu.as_ref().expect("open").menu;
        let list = submenu.layout.as_ref().expect("drawn").rows[1];
        assert!(
            list.x >= view.x + view.width,
            "beside the item, on its right"
        );
        assert_eq!(
            list.y,
            view.y + view.height,
            "its rows line up with the item's"
        );
        let on_list = Point::new(list.x + 5.0, list.y + 5.0);
        assert_eq!(
            window.feed(&mut menu, &click(on_list)),
            MenuResponse::Activated(12)
        );
        assert!(!menu.is_open());
    }

    /// `count` items, their ids counting up from 0.
    fn long_entries(count: u8) -> Vec<MenuEntry<u8>> {
        (0..count)
            .map(|id| MenuItem::new(id, format!("Item {id}")).into())
            .collect()
    }

    /// Whether the whole of `row` shows in the part of `layout`'s box the
    /// rows show in.
    fn shows(layout: &MenuLayout, row: Rect) -> bool {
        let view = layout.viewport;
        row.y >= view.y && row.y + row.height <= view.y + view.height
    }

    /// Whether `inner` lies wholly inside `outer`.
    fn is_inside(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x + inner.width <= outer.x + outer.width
            && inner.y + inner.height <= outer.y + outer.height
    }

    /// A turn of the wheel by `notches` toward the top at `position`.
    fn wheel(notches: f32, position: Point) -> Input {
        Input::Wheel(WheelInput {
            delta: WheelDelta::Lines(Point::new(0.0, notches)),
            mods: Mods::NONE,
            position,
        })
    }

    #[test]
    fn test_a_menu_taller_than_the_window_stays_inside_it_and_scrolls_to_the_highlight() {
        let window = TestWindow::new(800, 600);
        let mut menu = Menu::new(long_entries(80));
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        redraw(&window, &mut menu);
        let layout = menu.layout.clone().expect("drawn");
        assert!(is_inside(layout.popup.rect(), WINDOW), "{layout:?}");
        assert!(layout.overflow > 0.0, "the rows do not fit");
        assert!(!shows(&layout, layout.rows[79]));
        menu.move_highlight(-1);
        redraw(&window, &mut menu);
        let layout = menu.layout.clone().expect("drawn");
        let last = layout.rows[79];
        assert!(shows(&layout, last), "the highlight scrolled into view");
        assert_eq!(layout.scroll, layout.overflow, "no further than it");
        let center = Point::new(last.x + last.width / 2.0, last.y + last.height / 2.0);
        assert_eq!(layout.entry_at(center), Some(79), "hit where it is drawn");
        let under = layout.rows[0];
        let hidden = Point::new(under.x + 1.0, under.y + under.height / 2.0);
        assert_eq!(
            layout.entry_at(hidden),
            None,
            "a row scrolled away is not hit"
        );
        menu.move_highlight(1);
        redraw(&window, &mut menu);
        assert_eq!(menu.layout.as_ref().expect("drawn").scroll, 0.0, "wrapped");
    }

    #[test]
    fn test_the_headings_over_the_highlight_scroll_into_view_with_it() {
        let window = TestWindow::new(800, 600);
        let mut entries = vec![MenuEntry::Heading("File".into())];
        entries.extend(long_entries(80));
        let mut menu = Menu::new(entries);
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        menu.move_highlight(-1);
        redraw(&window, &mut menu);
        menu.move_highlight(1);
        redraw(&window, &mut menu);
        let layout = menu.layout.as_ref().expect("drawn");
        assert!(shows(layout, layout.rows[0]), "the heading shows");
    }

    #[test]
    fn test_the_wheel_scrolls_a_menu_taller_than_the_window_and_not_one_that_fits() {
        let mut window = TestWindow::new(800, 600);
        let mut menu = Menu::new(long_entries(80));
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        redraw(&window, &mut menu);
        let layout = menu.layout.clone().expect("drawn");
        let inside = Point::new(layout.viewport.x + 4.0, layout.viewport.y + 4.0);
        let first = layout.rows[0];
        window.send(&wheel(-1.0, inside), |input, cx| menu.handle(input, cx));
        redraw(&window, &mut menu);
        let scrolled = menu.layout.clone().expect("drawn");
        let by = WHEEL_ITEMS * layout.item_height;
        assert_eq!(scrolled.scroll, by);
        assert_eq!(scrolled.rows[0].y, first.y - by, "the rows moved up");
        let outside = Point::new(700.0, 300.0);
        window.send(&wheel(-1.0, outside), |input, cx| menu.handle(input, cx));
        assert_eq!(menu.scroll, by, "only over the menu");
        window.send(&wheel(-100.0, inside), |input, cx| menu.handle(input, cx));
        assert_eq!(menu.scroll, layout.overflow, "held at the end");
        window.send(&wheel(100.0, inside), |input, cx| menu.handle(input, cx));
        assert_eq!(menu.scroll, 0.0, "held at the start");
        let (mut window, mut menu) = drawn_menu();
        let rect = menu.layout.as_ref().expect("drawn").popup.rect();
        let inside = Point::new(rect.x + 4.0, rect.y + 4.0);
        window.send(&wheel(-1.0, inside), |input, cx| menu.handle(input, cx));
        assert_eq!(menu.scroll, 0.0, "a menu that fits does not scroll");
    }

    #[test]
    fn test_a_submenu_taller_than_the_window_stays_inside_it_and_scrolls_to_the_highlight() {
        let window = TestWindow::new(800, 600);
        let more = MenuItem::new(100, "More").with_submenu(long_entries(80));
        let mut menu = Menu::new(vec![MenuItem::new(101, "Open").into(), more.into()]);
        menu.open(Rect::new(10.0, 300.0, 30.0, 30.0), PopupSide::Below);
        redraw(&window, &mut menu);
        menu.move_highlight(-1);
        assert!(menu.open_submenu());
        redraw(&window, &mut menu);
        menu.move_highlight(-1);
        redraw(&window, &mut menu);
        let submenu = &menu.submenu.as_ref().expect("open").menu;
        let layout = submenu.layout.as_ref().expect("drawn");
        assert!(is_inside(layout.popup.rect(), WINDOW), "{layout:?}");
        assert_eq!(submenu.highlighted, Some(79));
        assert!(shows(layout, layout.rows[79]));
    }

    #[test]
    fn test_a_scrolled_menu_paints_its_rows_only_inside_its_padding() {
        let theme = Theme::dark().with_grid_look();
        let window = Rect::new(0.0, 0.0, 320.0, 240.0);
        let mut menu = Menu::new(long_entries(40));
        menu.open(Rect::new(10.0, 0.0, 30.0, 30.0), PopupSide::Below);
        let mut frame = offscreen::plain_frame(320, 240);
        menu.draw(&mut frame, window, &theme);
        let layout = menu.layout.clone().expect("drawn");
        let bottom = layout.viewport.y + layout.viewport.height;
        // Highlight the first row cut by the bottom edge, as the pointer
        // would, which scrolls nothing.
        menu.highlighted = layout
            .rows
            .iter()
            .position(|row| row.y + row.height > bottom);
        let Some(snapshot) = offscreen::render(320, 240, |frame| {
            menu.draw(frame, window, &theme);
        }) else {
            return;
        };
        let rect = layout.popup.rect();
        let border = theme.shape.border(1.0);
        let padding = Rect::new(
            layout.viewport.x,
            bottom,
            layout.viewport.width,
            rect.y + rect.height - border - bottom,
        );
        assert!(
            snapshot.is_plain(padding, theme.raised),
            "nothing drawn under the rows: {padding:?}"
        );
        snapshot.assert_pixel(
            layout.viewport.x + layout.viewport.width - 1.0,
            bottom - 1.0,
            theme.selection,
            "the highlight shows down to the edge",
        );
    }

    #[test]
    fn test_a_menu_that_just_fits_draws_as_in_a_roomy_window() {
        let theme = Theme::dark().with_grid_look();
        let anchor = Rect::new(20.0, 20.0, 0.0, 0.0);
        let draw = |window: Rect| {
            let mut menu = Menu::new(entries());
            menu.open(anchor, PopupSide::Below);
            menu.move_highlight(1);
            let snapshot = offscreen::render(320, 240, |frame| {
                menu.draw(frame, window, &theme);
            });
            (snapshot, menu.layout)
        };
        // Without a GPU nothing draws, so there is nothing to compare.
        let (roomy, Some(layout)) = draw(WINDOW) else {
            return;
        };
        let rect = layout.popup.rect();
        let tight = Rect::new(0.0, 0.0, WINDOW.width, rect.y + rect.height);
        let (fitted, fitted_layout) = draw(tight);
        assert_eq!(
            fitted_layout,
            Some(layout),
            "the same place and rows, unscrolled"
        );
        let (Some(roomy), Some(fitted)) = (roomy, fitted) else {
            return;
        };
        for y in rect.y as u32..(rect.y + rect.height) as u32 {
            for x in rect.x as u32..(rect.x + rect.width) as u32 {
                let (x, y) = (x as f32, y as f32);
                assert_eq!(fitted.pixel(x, y), roomy.pixel(x, y), "at {x}, {y}");
            }
        }
    }

    #[test]
    fn test_a_submenu_without_room_on_the_right_opens_on_the_left() {
        let (window, mut menu) = drawn_nested(Rect::new(700.0, 0.0, 30.0, 30.0));
        menu.move_highlight(1);
        assert!(menu.open_submenu());
        redraw(&window, &mut menu);
        let parent = menu.layout.as_ref().expect("drawn").popup.rect();
        let submenu = &menu.submenu.as_ref().expect("open").menu;
        let child = submenu.layout.as_ref().expect("drawn").popup.rect();
        assert_eq!(child.x + child.width, parent.x, "flipped to the left");
        assert!(menu.close_submenu());
        assert!(!menu.close_submenu(), "none left to close");
    }

    #[test]
    fn test_marked_items_and_an_open_submenu_describe_their_state() {
        let (window, mut menu) = drawn_nested(Rect::new(10.0, 0.0, 30.0, 30.0));
        let id = FocusId::new("menu");
        let mut closed = redraw(&window, &mut menu);
        menu.access(&mut closed, id);
        assert_eq!(
            ui_text(&closed, "t"),
            "Window \"t\" focused\n\
             \x20 Menu\n\
             \x20   MenuItem \"View\" collapsed\n\
             \x20   MenuItemCheckBox \"Wrap\" checked\n\
             \x20   MenuItem \"Quit\"\n"
        );
        assert_eq!(
            menu.access_action(id, AccessAction::Click(id.child(0))),
            MenuResponse::Handled,
            "a reader's click opens the submenu"
        );
        let mut open = redraw(&window, &mut menu);
        menu.access(&mut open, id);
        assert_eq!(
            ui_text(&open, "t"),
            "Window \"t\" focused\n\
             \x20 Menu\n\
             \x20   MenuItem \"View\" expanded\n\
             \x20     Menu\n\
             \x20       MenuItemRadio \"Grid\" checked\n\
             \x20       MenuItemRadio \"List\" unchecked\n\
             \x20   MenuItemCheckBox \"Wrap\" checked\n\
             \x20   MenuItem \"Quit\"\n"
        );
        let list = id.child(0).child(SUBMENU_NODE).child(1);
        assert_eq!(
            menu.access_action(id, AccessAction::Click(list)),
            MenuResponse::Activated(12)
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn test_marks_and_submenu_arrows_widen_the_menu_only_when_present() {
        let layout = |entries: &[MenuEntry<u8>]| {
            let anchor = MenuAnchor {
                rect: Rect::new(10.0, 0.0, 30.0, 30.0),
                side: PopupSide::Below,
            };
            let measure = |text: &str| text::width(text) as f32 * METRICS.width;
            MenuLayout::new(entries, anchor, WINDOW, METRICS, &Shape::GRID, 1.0, measure)
        };
        let long = "a label of twenty cells"
            .chars()
            .take(20)
            .collect::<String>();
        let plain = layout(&[MenuItem::new(1, long.as_str()).into()]);
        let marked = layout(&[MenuItem::new(1, long.as_str()).with_checked(false).into()]);
        assert_eq!(plain.gutter, 0.0);
        assert_eq!(marked.gutter, MARK_CELLS * METRICS.width);
        assert_eq!(
            marked.popup.rect().width,
            plain.popup.rect().width + MARK_CELLS * METRICS.width
        );
    }
}

//! [`Toolbar`]: a row of flat buttons (icons, labels, or both) with
//! separators between groups, toggle groups of which one button is active
//! (view modes), and spacers that push what follows to the right. Each
//! button may be bound to a [`CommandRegistry`] command, whose title,
//! keys, and availability then label it, fill its tooltip, and enable it.
//!
//! When the toolbar is narrower than its items, the ones that do not fit
//! leave the row from the end and a "more" button at its right opens them
//! in a [`Menu`]: toggles there show a check mark, a group's buttons a
//! radio dot. Every command stays reachable by pointer and keys, which a
//! clipped toolbar would not allow.
//!
//! Keys, while the app says the toolbar is focused: Left and Right (or `h`
//! and `l`) move between its buttons, wrapping, and Home and End go to the
//! first and the last; Space or Enter activates the button with the focus.
//! A left press on a button and its release over it activate it; a press
//! on the "more" button opens its menu, which then takes every input as an
//! open [`Menu`] does. A disabled button is drawn muted and skipped.

use std::slice;

use accesskit::{Action, HasPopup, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods, key_label};
use super::super::base::strings::{Strings, fill};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::super::render::image::Svg;
use super::button::{ButtonFace, ButtonVariant, face_height, hover_light};
use super::command::CommandRegistry;
use super::menu::{Menu, MenuEntry, MenuItem, MenuResponse};
use super::popup::PopupSide;

// ========================================================================
// Constants
// ========================================================================

/// What the "more" button shows.
const MORE_GLYPH: &str = "\u{2026}";

/// The blank between two items, in logical pixels.
const ITEM_GAP: f32 = 2.0;

/// How wide a separator is, in cells.
const SEPARATOR_CELLS: f32 = 1.0;

/// The thickness of a separator's line, in logical pixels.
const SEPARATOR_STROKE: f32 = 1.0;

/// The share of the toolbar's height a separator's line is tall.
const SEPARATOR_SHARE: f32 = 0.6;

/// How many accessibility child ids each item takes, one per button of a
/// group: a button's node is child `item * MEMBER_NODES + member`.
const MEMBER_NODES: u64 = 1 << 16;

/// The child index of the "more" button's accessibility node.
const MORE_NODE: u64 = 1 << 40;

/// The child index, under the "more" button's node, of its menu's node.
const MENU_NODE: u64 = 0;

// ========================================================================
// Data Structures
// ========================================================================

/// A row of buttons with ids of type `I`. The app owns it, hands it the
/// window's pointer events (and its keys while it has the focus), and
/// draws it each frame; [`ToolbarResponse::Activated`] names the button
/// the user activated.
#[derive(Clone, Debug)]
pub struct Toolbar<I> {
    /// The button with the keyboard focus inside the toolbar.
    focus: Option<Stop>,
    /// Whether the app gave it the keyboard.
    focused: bool,
    /// The button under the pointer.
    hovered: Option<Stop>,
    /// The items, left to right.
    items: Vec<ToolbarItem<I>>,
    /// Where the parts were drawn last, for the pointer.
    layout: Option<ToolbarLayout>,
    /// The menu of the items that did not fit.
    menu: Menu<I>,
    /// The button a held left press started on.
    pressed: Option<Stop>,
    /// The words of the theme last drawn in, for tooltips and screen
    /// readers.
    strings: Strings,
}

/// One item of a [`Toolbar`].
#[derive(Clone, Debug, PartialEq)]
pub enum ToolbarItem<I> {
    /// A button.
    Button(ToolButton<I>),
    /// Buttons side by side of which one is active, as view modes are:
    /// activating one makes it the active one. Mark the first active one
    /// with [`ToolButton::toggle`] or [`Toolbar::set_active`].
    Group(Vec<ToolButton<I>>),
    /// A line between groups of buttons.
    Separator,
    /// Blank room taking what the row leaves over, pushing the items after
    /// it to the right; room left over is shared between spacers.
    Spacer,
}

/// A button of a [`Toolbar`].
#[derive(Clone, Debug, PartialEq)]
pub struct ToolButton<I> {
    /// Whether it is pressed in: a toggle that is on, or a group's active
    /// button.
    active: bool,
    /// The registry command it is bound to, if any.
    command: Option<&'static str>,
    /// Whether it takes input.
    enabled: bool,
    /// The icon it shows, if any.
    icon: Option<Svg>,
    /// What [`ToolbarResponse::Activated`] hands back.
    id: I,
    /// The first key sequence that runs its command, from the registry;
    /// labelled in the theme's words where it is shown.
    keys: Option<Vec<Key>>,
    /// What it says, as the app set it; may be empty.
    label: String,
    /// Its command's title, from the registry.
    title: Option<String>,
    /// Whether it stays pressed in or out, flipping each time it is
    /// activated.
    toggle: bool,
    /// The help shown while the pointer rests on it, as the app set it.
    tooltip: Option<String>,
}

/// What became of an input offered to a [`Toolbar`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ToolbarResponse<I> {
    /// The user activated the button with this id: act on it. A toggle
    /// has flipped and a group's button become the active one already. One
    /// chosen in the "more" menu closed the menu, too.
    Activated(I),
    /// The "more" menu closed without a choice.
    Closed,
    /// The toolbar used the input.
    Handled,
    /// The input is not the toolbar's.
    Ignored,
    /// The "more" menu opened; while it is, the toolbar takes every
    /// pointer input.
    Opened,
}

/// A place the toolbar's keyboard focus and the pointer stop at.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Stop {
    /// A button.
    Button(Slot),
    /// The "more" button.
    More,
}

/// Where a button is among the items.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Slot {
    /// The index of the item.
    item: usize,
    /// The index of the button in its group; 0 for a lone button.
    member: usize,
}

/// Where a toolbar's parts were drawn.
#[derive(Clone, Debug, PartialEq)]
struct ToolbarLayout {
    /// The whole toolbar.
    area: Rect,
    /// Each button shown, left to right.
    buttons: Vec<(Slot, Rect)>,
    /// The index of the first item that did not fit; the item count when
    /// all did.
    hidden_from: usize,
    /// The "more" button, when items did not fit.
    more: Option<Rect>,
    /// Each separator's line.
    separators: Vec<Rect>,
}

// ========================================================================
// Toolbar Implementations
// ========================================================================

impl<I: Copy + PartialEq> Toolbar<I> {
    /// A toolbar of `items`, left to right.
    pub fn new(items: Vec<ToolbarItem<I>>) -> Self {
        Self {
            focus: None,
            focused: false,
            hovered: None,
            items,
            layout: None,
            menu: Menu::new(Vec::new()),
            pressed: None,
            strings: Strings::english(),
        }
    }

    /// Labels, enables, and fills the tooltips of the buttons bound to
    /// commands from `registry`: each takes its command's title (as its
    /// label when it has no icon or label of its own, as its tooltip
    /// otherwise), its key hint, and whether it can run now. Call it again
    /// whenever the registry changes.
    pub fn sync_commands(&mut self, registry: &CommandRegistry) {
        for button in self.buttons_mut() {
            let Some(command) = button.command else {
                continue;
            };
            button.title = registry
                .command(command)
                .map(|found| found.title.to_owned());
            button.keys = registry.keys(command).first().cloned();
            button.enabled = registry.is_enabled(command);
        }
    }

    /// Lets the button `id` take input (`true`), or disables it. Unknown
    /// ids are ignored.
    pub fn set_enabled(&mut self, id: I, enabled: bool) {
        for button in self.buttons_mut().filter(|button| button.id == id) {
            button.enabled = enabled;
        }
    }

    /// Presses the button `id` in (`true`) or out, as the app's state
    /// says, without a response: a toggle turned on or off, or a group's
    /// button made the active one (the others in its group go out).
    /// Unknown ids are ignored.
    pub fn set_active(&mut self, id: I, active: bool) {
        for item in &mut self.items {
            match item {
                ToolbarItem::Button(button) if button.id == id => button.active = active,
                ToolbarItem::Group(buttons) if buttons.iter().any(|button| button.id == id) => {
                    for button in buttons {
                        button.active = match button.id == id {
                            true => active,
                            false => button.active && !active,
                        };
                    }
                }
                ToolbarItem::Button(_)
                | ToolbarItem::Group(_)
                | ToolbarItem::Separator
                | ToolbarItem::Spacer => {}
            }
        }
    }

    /// Whether the button `id` is pressed in; `false` for an unknown id.
    pub fn is_active(&self, id: I) -> bool {
        self.buttons()
            .any(|(_, button)| button.id == id && button.active)
    }

    /// How tall it is drawn with `metrics`: as tall as a button.
    /// The scale factor is unused here; every control takes it, so an app
    /// asks each control's height the same way.
    pub fn height(&self, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        face_height(metrics)
    }

    /// How wide its items are drawn in `frame` and `theme` with none
    /// hidden, in physical pixels: its buttons and groups at their faces'
    /// widths and its separators with the gaps between them all, its
    /// spacers taking no room. Drawn at least this wide, it shows every
    /// item and no "more" button, so an app can size it to its buttons and
    /// lay another widget beside it.
    pub fn natural_width(&self, frame: &Frame, theme: &Theme) -> f32 {
        let (separator, gap) = spacing(frame);
        let (_, total) = self.item_widths(separator, gap, |button| button.face_width(frame, theme));
        total
    }

    /// Whether the "more" menu is open.
    pub fn is_menu_open(&self) -> bool {
        self.menu.is_open()
    }

    /// Where the button under the pointer was drawn and its tooltip (its
    /// tooltip or command title, with its keys), while the pointer is over
    /// a button that has one; hand them to
    /// [`Tooltip::hover`](crate::Tooltip::hover), and call
    /// [`Tooltip::leave`](crate::Tooltip::leave) when this is `None`.
    pub fn tooltip_hover(&self) -> Option<(Rect, String)> {
        let stop = self.hovered?;
        let rect = self.stop_rect(stop)?;
        let text = match stop {
            Stop::Button(slot) => self.button(slot)?.tooltip_text(&self.strings)?,
            Stop::More => self.strings.more.to_string(),
        };
        Some((rect, text))
    }

    /// Describes the toolbar as last drawn to screen readers as `id`: a
    /// toolbar holding its buttons shown (the `member`th button of the
    /// `item`th item is child `item * 65536 + member` of `id`), each named
    /// by its label (or its tooltip or command title), described by its
    /// tooltip, saying whether a toggle or a group's button is pressed in,
    /// clickable unless disabled; and the "more" button (child `1 << 40`),
    /// holding its menu while open (child 0 of the button's node). The
    /// focused button has the focus while the toolbar does. Nothing before
    /// the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        frame.enter_access(id, layout.area, Node::new(Role::Toolbar));
        for (slot, rect) in &layout.buttons {
            let Some(button) = self.button(*slot) else {
                continue;
            };
            let mut node = Node::new(Role::Button);
            node.set_label(button.name());
            if let Some(tooltip) = button.tooltip_text(&self.strings)
                && tooltip != button.name()
            {
                node.set_description(tooltip);
            }
            if button.toggle || self.is_grouped(*slot) {
                node.set_toggled(button.active.into());
            }
            match button.enabled {
                true => node.add_action(Action::Click),
                false => node.set_disabled(),
            }
            frame.access(id.child(slot_node(*slot)), *rect, node);
        }
        if let Some(rect) = layout.more {
            let more = id.child(MORE_NODE);
            let mut node = Node::new(Role::Button);
            node.set_label(self.strings.more.as_ref());
            node.set_has_popup(HasPopup::Menu);
            node.set_expanded(self.menu.is_open());
            node.add_action(Action::Click);
            frame.enter_access(more, rect, node);
            self.menu.access(frame, more.child(MENU_NODE));
            frame.leave_access();
        }
        frame.leave_access();
        if let Some(stop) = self.focus.filter(|_| !self.menu.is_open()) {
            frame.access.set_inner_focus(id, stop_node(id, stop));
        }
    }

    /// Does what a screen reader's `action` on the toolbar described as
    /// `id` asks: a click on a button activates it, one on the "more"
    /// button opens its menu, and one on an item of that menu chooses it.
    /// Ignored for other actions and nodes.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> ToolbarResponse<I> {
        let AccessAction::Click(target) = action else {
            return ToolbarResponse::Ignored;
        };
        let more = id.child(MORE_NODE);
        if target == more {
            return self.open_menu();
        }
        if self.menu.is_open() {
            let response = self.menu.access_action(more.child(MENU_NODE), action);
            return self.settle_menu(response);
        }
        let shown = self
            .layout
            .as_ref()
            .and_then(|layout| {
                layout
                    .buttons
                    .iter()
                    .find(|(slot, _)| id.child(slot_node(*slot)) == target)
            })
            .map(|(slot, _)| *slot);
        match shown {
            Some(slot) => self.activate(Stop::Button(slot)),
            None => ToolbarResponse::Ignored,
        }
    }

    /// Every button with its slot, left to right.
    fn buttons(&self) -> impl Iterator<Item = (Slot, &ToolButton<I>)> {
        self.items.iter().enumerate().flat_map(|(item, entry)| {
            let members: &[ToolButton<I>] = match entry {
                ToolbarItem::Button(button) => slice::from_ref(button),
                ToolbarItem::Group(buttons) => buttons,
                ToolbarItem::Separator | ToolbarItem::Spacer => &[],
            };
            members
                .iter()
                .enumerate()
                .map(move |(member, button)| (Slot { item, member }, button))
        })
    }

    /// Every button, left to right.
    fn buttons_mut(&mut self) -> impl Iterator<Item = &mut ToolButton<I>> {
        self.items.iter_mut().flat_map(|entry| {
            let members: &mut [ToolButton<I>] = match entry {
                ToolbarItem::Button(button) => slice::from_mut(button),
                ToolbarItem::Group(buttons) => buttons,
                ToolbarItem::Separator | ToolbarItem::Spacer => &mut [],
            };
            members.iter_mut()
        })
    }

    /// The button at `slot`.
    fn button(&self, slot: Slot) -> Option<&ToolButton<I>> {
        match self.items.get(slot.item)? {
            ToolbarItem::Button(button) if slot.member == 0 => Some(button),
            ToolbarItem::Group(buttons) => buttons.get(slot.member),
            ToolbarItem::Button(_) | ToolbarItem::Separator | ToolbarItem::Spacer => None,
        }
    }

    /// Whether the button at `slot` is in a group.
    fn is_grouped(&self, slot: Slot) -> bool {
        matches!(self.items.get(slot.item), Some(ToolbarItem::Group(_)))
    }

    /// The places the keyboard focus stops at, as last drawn: the enabled
    /// buttons shown, then the "more" button.
    fn stops(&self) -> Vec<Stop> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let buttons = layout
            .buttons
            .iter()
            .filter(|(slot, _)| self.button(*slot).is_some_and(|button| button.enabled))
            .map(|(slot, _)| Stop::Button(*slot));
        let more = layout.more.map(|_| Stop::More);
        buttons.chain(more).collect()
    }

    /// Where `stop` was drawn last.
    fn stop_rect(&self, stop: Stop) -> Option<Rect> {
        let layout = self.layout.as_ref()?;
        match stop {
            Stop::Button(slot) => layout
                .buttons
                .iter()
                .find(|(shown, _)| *shown == slot)
                .map(|(_, rect)| *rect),
            Stop::More => layout.more,
        }
    }

    /// The stop under `point`, as last drawn.
    fn stop_at(&self, point: Point) -> Option<Stop> {
        let layout = self.layout.as_ref()?;
        if layout.more.is_some_and(|rect| rect.contains(point)) {
            return Some(Stop::More);
        }
        layout
            .buttons
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(slot, _)| Stop::Button(*slot))
            .filter(|stop| self.is_usable(*stop))
    }

    /// Whether `stop` takes input: the "more" button, or an enabled button.
    fn is_usable(&self, stop: Stop) -> bool {
        match stop {
            Stop::Button(slot) => self.button(slot).is_some_and(|button| button.enabled),
            Stop::More => true,
        }
    }

    /// Activates `stop`: opens the "more" menu, or flips a toggle, makes a
    /// group's button the active one, and hands back the button's id.
    fn activate(&mut self, stop: Stop) -> ToolbarResponse<I> {
        let slot = match stop {
            Stop::Button(slot) => slot,
            Stop::More => return self.open_menu(),
        };
        let grouped = self.is_grouped(slot);
        let Some(button) = self.button(slot).filter(|button| button.enabled) else {
            return ToolbarResponse::Ignored;
        };
        let (id, toggle, active) = (button.id, button.toggle, button.active);
        match (grouped, toggle) {
            (true, _) => self.set_active(id, true),
            (false, true) => self.set_active(id, !active),
            (false, false) => {}
        }
        ToolbarResponse::Activated(id)
    }

    /// Activates the button `id` chosen in the "more" menu.
    fn activate_id(&mut self, id: I) -> ToolbarResponse<I> {
        let slot = self
            .buttons()
            .find(|(_, button)| button.id == id)
            .map(|(slot, _)| slot);
        match slot {
            Some(slot) => self.activate(Stop::Button(slot)),
            None => ToolbarResponse::Activated(id),
        }
    }

    /// Opens the "more" menu of the items that did not fit, under the
    /// "more" button.
    fn open_menu(&mut self) -> ToolbarResponse<I> {
        let Some(layout) = &self.layout else {
            return ToolbarResponse::Ignored;
        };
        let Some(anchor) = layout.more else {
            return ToolbarResponse::Ignored;
        };
        let entries = self.menu_entries(layout.hidden_from);
        self.menu = Menu::new(entries);
        self.menu.open(anchor, PopupSide::Below);
        self.focus = Some(Stop::More);
        ToolbarResponse::Opened
    }

    /// The "more" menu's entries: the items from index `from` on, as menu
    /// items with their keys, a check mark for a toggle, and a radio dot
    /// for a group's button.
    fn menu_entries(&self, from: usize) -> Vec<MenuEntry<I>> {
        let mut entries = Vec::new();
        for item in self.items.iter().skip(from) {
            match item {
                ToolbarItem::Button(button) => entries.push(button.menu_item(false, &self.strings)),
                ToolbarItem::Group(buttons) => {
                    entries.extend(
                        buttons
                            .iter()
                            .map(|button| button.menu_item(true, &self.strings)),
                    );
                }
                ToolbarItem::Separator if !entries.is_empty() => {
                    entries.push(MenuEntry::Separator);
                }
                ToolbarItem::Separator | ToolbarItem::Spacer => {}
            }
        }
        entries
    }

    /// What the "more" menu's `response` means for the toolbar.
    fn settle_menu(&mut self, response: MenuResponse<I>) -> ToolbarResponse<I> {
        match response {
            MenuResponse::Activated(id) => self.activate_id(id),
            MenuResponse::Closed => ToolbarResponse::Closed,
            MenuResponse::Handled => ToolbarResponse::Handled,
            MenuResponse::Ignored => ToolbarResponse::Ignored,
        }
    }

    /// Moves the keyboard focus `delta` stops, wrapping.
    fn move_focus(&mut self, delta: isize) -> ToolbarResponse<I> {
        let stops = self.stops();
        if stops.is_empty() {
            return ToolbarResponse::Ignored;
        }
        let count = stops.len() as isize;
        let position = match self
            .focus
            .and_then(|focus| stops.iter().position(|stop| *stop == focus))
        {
            Some(position) => (position as isize + delta).rem_euclid(count),
            None => 0,
        };
        self.focus = Some(stops[position as usize]);
        ToolbarResponse::Handled
    }

    /// Moves the keyboard focus to the first stop, or the last when
    /// `last`.
    fn focus_end(&mut self, last: bool) -> ToolbarResponse<I> {
        let stops = self.stops();
        let stop = match last {
            true => stops.last(),
            false => stops.first(),
        };
        match stop {
            Some(stop) => {
                self.focus = Some(*stop);
                ToolbarResponse::Handled
            }
            None => ToolbarResponse::Ignored,
        }
    }

    /// Reacts to `key`, as the module documentation lists, while focused
    /// and the menu is closed.
    fn handle_key(&mut self, key: Key) -> ToolbarResponse<I> {
        if !self.focused || key.mods != Mods::NONE {
            return ToolbarResponse::Ignored;
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => self.move_focus(-1),
            KeyCode::Right | KeyCode::Char('l') => self.move_focus(1),
            KeyCode::Home => self.focus_end(false),
            KeyCode::End => self.focus_end(true),
            KeyCode::Char(' ') | KeyCode::Enter => match self.focus {
                Some(stop) => self.activate(stop),
                None => ToolbarResponse::Ignored,
            },
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Insert
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Tab
            | KeyCode::Up => ToolbarResponse::Ignored,
        }
    }

    /// Reacts to a left press or release at `point`: a press on a button
    /// holds it (and opens the "more" menu at once), and the release over
    /// the button pressed activates it.
    fn press(&mut self, pressed: bool, point: Point) -> ToolbarResponse<I> {
        let under = self.stop_at(point);
        match (pressed, under, self.pressed.take()) {
            (true, Some(Stop::More), _) => self.open_menu(),
            (true, Some(stop), _) => {
                self.pressed = Some(stop);
                self.focus = Some(stop);
                ToolbarResponse::Handled
            }
            (false, Some(stop), Some(held)) if stop == held => self.activate(stop),
            (false, _, Some(_)) => ToolbarResponse::Handled,
            (true, None, _) | (false, _, None) => ToolbarResponse::Ignored,
        }
    }

    /// Follows the pointer to `point` (`None` outside the window), asking a
    /// redraw when it moved onto another button; says whether it is over
    /// one.
    fn follow_pointer(&mut self, point: Option<Point>, cx: &mut EventCx<'_>) -> bool {
        let under = point.and_then(|point| self.stop_at(point));
        if under != self.hovered {
            self.hovered = under;
            cx.request_redraw();
        }
        under.is_some()
    }

    /// Each item's width with buttons as wide as `width` says and
    /// separators `separator` wide (a spacer's 0), and the width of them
    /// all `gap` apart.
    fn item_widths(
        &self,
        separator: f32,
        gap: f32,
        width: impl Fn(&ToolButton<I>) -> f32,
    ) -> (Vec<f32>, f32) {
        let item_width = |item: &ToolbarItem<I>| match item {
            ToolbarItem::Button(button) => width(button),
            ToolbarItem::Group(buttons) => buttons.iter().map(&width).sum(),
            ToolbarItem::Separator => separator,
            ToolbarItem::Spacer => 0.0,
        };
        let widths: Vec<f32> = self.items.iter().map(item_width).collect();
        let gaps = gap * self.items.len().saturating_sub(1) as f32;
        let total = widths.iter().sum::<f32>() + gaps;
        (widths, total)
    }

    /// Lays the items out in `area` with buttons `height` tall and as wide
    /// as `width` says, separators `separator` wide, `gap` apart, hiding
    /// the items that do not fit behind a square "more" button.
    fn lay_out(
        &self,
        area: Rect,
        height: f32,
        separator: f32,
        gap: f32,
        width: impl Fn(&ToolButton<I>) -> f32,
    ) -> ToolbarLayout {
        let (widths, total) = self.item_widths(separator, gap, &width);
        let (hidden_from, more) = match total <= area.width {
            true => (self.items.len(), None),
            false => {
                let more = Rect::new(area.x + area.width - height, area.y, height, height);
                let budget = area.width - height - gap;
                let mut used = 0.0;
                let fits = widths
                    .iter()
                    .take_while(|item| {
                        used += *item + gap;
                        used - gap <= budget
                    })
                    .count();
                (fits, Some(more))
            }
        };
        // Separators and spacers at the end of what is shown only waste room.
        let shown = self.items[..hidden_from]
            .iter()
            .rposition(|item| matches!(item, ToolbarItem::Button(_) | ToolbarItem::Group(_)))
            .map_or(0, |last| last + 1);
        let shown_width: f32 =
            widths[..shown].iter().sum::<f32>() + gap * shown.saturating_sub(1) as f32;
        let spacers = self.items[..shown]
            .iter()
            .filter(|item| matches!(item, ToolbarItem::Spacer))
            .count();
        let spare = match (spacers, more) {
            (0, _) | (_, Some(_)) => 0.0,
            (count, None) => (area.width - shown_width).max(0.0) / count as f32,
        };
        let mut layout = ToolbarLayout {
            area,
            buttons: Vec::new(),
            hidden_from,
            more,
            separators: Vec::new(),
        };
        let mut x = area.x;
        for (item, entry) in self.items[..shown].iter().enumerate() {
            match entry {
                ToolbarItem::Button(button) => {
                    let rect = Rect::new(x, area.y, width(button), height);
                    layout.buttons.push((Slot { item, member: 0 }, rect));
                }
                ToolbarItem::Group(buttons) => {
                    let mut left = x;
                    for (member, button) in buttons.iter().enumerate() {
                        let rect = Rect::new(left, area.y, width(button), height);
                        layout.buttons.push((Slot { item, member }, rect));
                        left += rect.width;
                    }
                }
                ToolbarItem::Separator => {
                    layout
                        .separators
                        .push(Rect::new(x, area.y, separator, height));
                }
                ToolbarItem::Spacer => x += spare,
            }
            x += widths[item] + gap;
        }
        layout
    }
}

impl<I: Copy + PartialEq> Widget for Toolbar<I> {
    type Output = Option<Rect>;
    type Response = ToolbarResponse<I>;

    /// Reacts to `input`: while the "more" menu is open, every input is
    /// the menu's; otherwise keys as the module documentation lists while
    /// focused, a left press and release over a button activating it, and
    /// the pointer lighting the button under it. Asks for a redraw
    /// whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ToolbarResponse<I> {
        if self.menu.is_open() {
            let response = self.menu.handle(input, cx);
            return self.settle_menu(response);
        }
        let response = match input {
            Input::Key(key) => self.handle_key(*key),
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.follow_pointer(Some(button.position), cx);
                self.press(button.pressed, button.position)
            }
            Input::PointerMoved(position) => match self.follow_pointer(Some(*position), cx) {
                true => ToolbarResponse::Handled,
                false => ToolbarResponse::Ignored,
            },
            Input::PointerLeft => {
                self.follow_pointer(None, cx);
                ToolbarResponse::Ignored
            }
            Input::FocusLost if self.pressed.is_some() => {
                self.pressed = None;
                ToolbarResponse::Handled
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
            | Input::PointerButton(_)
            | Input::Wheel(_) => ToolbarResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the toolbar across the top of `area`, as tall as a button:
    /// its items from the left, spacers sharing what room is left, and the
    /// "more" button at the right when items did not fit; then the open
    /// "more" menu in the overlay. Returns where the menu was drawn (for a
    /// [`FocusTree`](crate::FocusTree) trap), or `None` while it is closed.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Option<Rect> {
        if self.strings != theme.strings {
            self.strings = theme.strings.clone();
        }
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let height = face_height(metrics).min(area.height);
        let bar = Rect::new(area.x, area.y, area.width, height);
        let (separator, gap) = spacing(frame);
        let layout = self.lay_out(bar, height, separator, gap, |button| {
            button.face_width(frame, theme)
        });
        let stroke = logical(SEPARATOR_STROKE, scale_factor);
        for rect in &layout.separators {
            let tall = (height * SEPARATOR_SHARE).round();
            let x = (rect.x + (rect.width - stroke) / 2.0).round();
            let y = (rect.y + (rect.height - tall) / 2.0).round();
            frame.fill_rect_over_text(Rect::new(x, y, stroke, tall), theme.border);
        }
        self.layout = Some(layout);
        let stops = self.stops();
        if self.focus.is_none_or(|focus| !stops.contains(&focus)) {
            self.focus = stops.first().copied();
        }
        let ring = |stop: Stop| self.focused && self.focus == Some(stop);
        let lit = |stop: Stop| self.hovered == Some(stop);
        let held = |stop: Stop| self.pressed == Some(stop) && self.hovered == Some(stop);
        let Some(layout) = &self.layout else {
            return None;
        };
        frame.clipped(bar, |frame| {
            for (slot, rect) in &layout.buttons {
                let Some(button) = self.button(*slot) else {
                    continue;
                };
                let stop = Stop::Button(*slot);
                let face = button.face(ring(stop), lit(stop), held(stop));
                face.draw(frame, *rect, theme);
            }
            if let Some(rect) = layout.more {
                let face = ButtonFace {
                    active: self.menu.is_open(),
                    enabled: true,
                    focus_ring: ring(Stop::More),
                    hover: hover_light(lit(Stop::More)),
                    icon: None,
                    label: MORE_GLYPH,
                    neutral_icon: false,
                    pressed: false,
                    shortcut: None,
                    variant: ButtonVariant::Ghost,
                };
                // The square button is narrower than the glyph with padding:
                // the face centers it.
                face.draw(frame, rect, theme);
            }
        });
        let window = frame.bounds();
        self.menu.draw(frame, window, theme)
    }

    /// Tells it whether it has the keyboard: it takes keys only while it
    /// does (a [`FocusTree`](crate::FocusTree) draws the focus ring around
    /// it; the toolbar marks its focused button).
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl<I> Outcome for ToolbarResponse<I> {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// ToolButton Implementations
// ========================================================================

impl<I> ToolButton<I> {
    /// An enabled push button handing back `id`, showing nothing yet: give
    /// it a label, an icon, or a command.
    pub fn new(id: I) -> Self {
        Self {
            active: false,
            command: None,
            enabled: true,
            icon: None,
            id,
            keys: None,
            label: String::new(),
            title: None,
            toggle: false,
            tooltip: None,
        }
    }

    /// This button saying `label`.
    pub fn with_label(self, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..self
        }
    }

    /// This button showing `icon`, a one-color SVG, left of its label (or
    /// alone without one).
    pub fn with_icon(self, icon: Svg) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// This button with `tooltip` as its help, in place of its command's
    /// title.
    pub fn with_tooltip(self, tooltip: impl Into<String>) -> Self {
        Self {
            tooltip: Some(tooltip.into()),
            ..self
        }
    }

    /// This button bound to the registry command `command` (such as
    /// `"view.grid"`): [`Toolbar::sync_commands`] labels and enables it
    /// from the command.
    pub fn with_command(self, command: &'static str) -> Self {
        Self {
            command: Some(command),
            ..self
        }
    }

    /// This button taking input (`true`), or disabled.
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// This button pressed in (`active`) or out: a toggle that flips each
    /// time it is activated, or, in a group, whether it is the active one.
    pub fn toggle(self, active: bool) -> Self {
        Self {
            active,
            toggle: true,
            ..self
        }
    }

    /// The label shown: the app's, or the command's title for a button
    /// without an icon.
    fn shown_label(&self) -> &str {
        match (&self.title, &self.icon) {
            (Some(title), None) if self.label.is_empty() => title,
            _ => &self.label,
        }
    }

    /// What the button is called in a menu and by screen readers: its
    /// label, else its tooltip, else its command's title.
    fn name(&self) -> &str {
        [
            Some(self.label.as_str()),
            self.tooltip.as_deref(),
            self.title.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty())
        .unwrap_or_default()
    }

    /// The tooltip: the app's, else the command's title, with the command's
    /// keys as `strings` write them; `None` without either.
    fn tooltip_text(&self, strings: &Strings) -> Option<String> {
        let text = self.tooltip.as_deref().or(self.title.as_deref())?;
        Some(match &self.keys {
            Some(keys) => fill(&strings.tooltip_keys, &[text, &key_label(keys, strings)]),
            None => text.to_owned(),
        })
    }

    /// How the button is painted: flat, ringed when `ring`, lit when
    /// `hovered`, pressed in when `pressed`.
    fn face(&self, ring: bool, hovered: bool, pressed: bool) -> ButtonFace<'_> {
        ButtonFace {
            active: self.active,
            enabled: self.enabled,
            focus_ring: ring,
            hover: hover_light(hovered),
            icon: self.icon.as_ref(),
            label: self.shown_label(),
            neutral_icon: false,
            pressed,
            shortcut: None,
            variant: ButtonVariant::Ghost,
        }
    }

    /// How wide the button is drawn in `frame` and `theme`, in physical
    /// pixels: its face's width at rest.
    fn face_width(&self, frame: &Frame, theme: &Theme) -> f32 {
        self.face(false, false, false).width(frame, theme)
    }
}

impl<I: Copy> ToolButton<I> {
    /// The button as an item of the "more" menu, its keys in `strings`'
    /// words: a radio item when `grouped`, a checked one for a toggle.
    fn menu_item(&self, grouped: bool, strings: &Strings) -> MenuEntry<I> {
        let item = MenuItem::new(self.id, self.name()).with_enabled(self.enabled);
        let item = match self.keys.as_deref() {
            Some(keys) => item.with_accelerator(key_label(keys, strings)),
            None => item,
        };
        let item = match (grouped, self.toggle) {
            (true, _) => item.with_radio(self.active),
            (false, true) => item.with_checked(self.active),
            (false, false) => item,
        };
        MenuEntry::Item(item)
    }
}

impl<I> From<ToolButton<I>> for ToolbarItem<I> {
    fn from(button: ToolButton<I>) -> Self {
        Self::Button(button)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The accessibility child index of the button at `slot`.
fn slot_node(slot: Slot) -> u64 {
    slot.item as u64 * MEMBER_NODES + slot.member as u64
}

/// A separator's width and the gap between items in `frame`, in physical
/// pixels.
fn spacing(frame: &Frame) -> (f32, f32) {
    let separator = (SEPARATOR_CELLS * frame.cell_metrics().width).round();
    (separator, logical(ITEM_GAP, frame.scale_factor()))
}

/// The accessibility id of `stop` in the toolbar described as `id`.
fn stop_node(id: FocusId, stop: Stop) -> FocusId {
    match stop {
        Stop::Button(slot) => id.child(slot_node(slot)),
        Stop::More => id.child(MORE_NODE),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};
    use crate::widgets::command::Command;

    /// The commands the tests bind buttons to.
    const COMMANDS: &[Command] = &[
        Command::new("file.open", "Open", "File").with_keys(&["C-o"]),
        Command::new("file.save", "Save", "File").with_keys(&["C-s"]),
    ];

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

    fn center(rect: Rect) -> Point {
        Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    /// Open and Save (bound to commands), a separator, a Grid and List
    /// group (Grid active), a spacer, and a Wrap toggle.
    fn toolbar() -> Toolbar<&'static str> {
        let mut toolbar = Toolbar::new(vec![
            ToolButton::new("open").with_command("file.open").into(),
            ToolButton::new("save").with_command("file.save").into(),
            ToolbarItem::Separator,
            ToolbarItem::Group(vec![
                ToolButton::new("grid").with_label("Grid").toggle(true),
                ToolButton::new("list").with_label("List"),
            ]),
            ToolbarItem::Spacer,
            ToolButton::new("wrap")
                .with_label("Wrap")
                .toggle(false)
                .into(),
        ]);
        let registry = CommandRegistry::new(COMMANDS).expect("a valid table");
        toolbar.sync_commands(&registry);
        toolbar
    }

    /// `toolbar` drawn `width` pixels wide in `window`.
    fn draw(window: &TestWindow, toolbar: &mut Toolbar<&'static str>, width: f32) -> Frame {
        let mut frame = window.frame();
        toolbar.draw(&mut frame, Rect::new(0.0, 0.0, width, 40.0), &Theme::dark());
        frame
    }

    /// Where the button `id` was drawn.
    fn rect_of(toolbar: &Toolbar<&'static str>, id: &str) -> Rect {
        let layout = toolbar.layout.as_ref().expect("drawn");
        layout
            .buttons
            .iter()
            .find(|(slot, _)| toolbar.button(*slot).is_some_and(|button| button.id == id))
            .map(|(_, rect)| *rect)
            .expect("shown")
    }

    #[test]
    fn test_commands_label_buttons_and_fill_tooltips_with_their_keys() {
        let mut window = TestWindow::new(800, 60);
        let mut toolbar = toolbar();
        let mut registry = CommandRegistry::new(COMMANDS).expect("a valid table");
        registry.set_enabled("file.save", false);
        toolbar.sync_commands(&registry);
        draw(&window, &mut toolbar, 800.0);
        let open = toolbar
            .button(Slot { item: 0, member: 0 })
            .expect("a button");
        assert_eq!(open.shown_label(), "Open");
        assert_eq!(
            open.tooltip_text(&Strings::english()).as_deref(),
            Some("Open (Ctrl+O)")
        );
        let save = rect_of(&toolbar, "save");
        window.feed(&mut toolbar, &left(true, center(save)));
        assert_eq!(
            window.feed(&mut toolbar, &left(false, center(save))),
            ToolbarResponse::Ignored,
            "a disabled command's button"
        );
        let open = rect_of(&toolbar, "open");
        window.feed(&mut toolbar, &Input::PointerMoved(center(open)));
        assert_eq!(
            toolbar.tooltip_hover(),
            Some((open, String::from("Open (Ctrl+O)")))
        );
    }

    #[test]
    fn test_a_spacer_pushes_the_items_after_it_to_the_right_edge() {
        let window = TestWindow::new(800, 60);
        let mut toolbar = toolbar();
        draw(&window, &mut toolbar, 800.0);
        let wrap = rect_of(&toolbar, "wrap");
        assert_eq!(wrap.x + wrap.width, 800.0);
        assert!(toolbar.layout.as_ref().expect("drawn").more.is_none());
    }

    #[test]
    fn test_left_and_right_move_between_buttons_skipping_disabled_ones_and_enter_activates() {
        let mut window = TestWindow::new(800, 60);
        let mut toolbar = toolbar();
        toolbar.set_enabled("save", false);
        toolbar.set_focused(true);
        draw(&window, &mut toolbar, 800.0);
        let mut press =
            |spec: &str, toolbar: &mut Toolbar<&'static str>| window.feed(toolbar, &key(spec));
        assert_eq!(
            press("<cr>", &mut toolbar),
            ToolbarResponse::Activated("open")
        );
        press("<right>", &mut toolbar);
        assert_eq!(
            press("<space>", &mut toolbar),
            ToolbarResponse::Activated("grid"),
            "Save is disabled"
        );
        press("l", &mut toolbar);
        assert_eq!(
            press("<cr>", &mut toolbar),
            ToolbarResponse::Activated("list")
        );
        assert!(
            toolbar.is_active("list") && !toolbar.is_active("grid"),
            "one active"
        );
        press("<right>", &mut toolbar);
        assert_eq!(
            press("<cr>", &mut toolbar),
            ToolbarResponse::Activated("wrap")
        );
        assert!(toolbar.is_active("wrap"), "the toggle flipped on");
        press("<right>", &mut toolbar);
        assert_eq!(
            press("<cr>", &mut toolbar),
            ToolbarResponse::Activated("open"),
            "wrapped around"
        );
        toolbar.set_focused(false);
        assert_eq!(press("<cr>", &mut toolbar), ToolbarResponse::Ignored);
    }

    #[test]
    fn test_a_click_activates_on_release_over_the_button_pressed() {
        let mut window = TestWindow::new(800, 60);
        let mut toolbar = toolbar();
        draw(&window, &mut toolbar, 800.0);
        let (open, list) = (rect_of(&toolbar, "open"), rect_of(&toolbar, "list"));
        window.feed(&mut toolbar, &left(true, center(open)));
        assert_eq!(
            window.feed(&mut toolbar, &left(false, center(list))),
            ToolbarResponse::Handled,
            "released over another button"
        );
        window.feed(&mut toolbar, &left(true, center(list)));
        assert_eq!(
            window.feed(&mut toolbar, &left(false, center(list))),
            ToolbarResponse::Activated("list")
        );
    }

    #[test]
    fn test_items_that_do_not_fit_go_into_the_more_menu_with_their_marks() {
        let mut window = TestWindow::new(800, 400);
        let mut toolbar = toolbar();
        draw(&window, &mut toolbar, 800.0);
        let save = rect_of(&toolbar, "save");
        draw(&window, &mut toolbar, save.x + save.width + 60.0);
        let layout = toolbar.layout.as_ref().expect("drawn");
        let more = layout.more.expect("a more button");
        assert_eq!(layout.buttons.len(), 2, "Open and Save fit");
        assert!(layout.separators.is_empty(), "no separator left at the end");
        assert_eq!(
            window.feed(&mut toolbar, &left(true, center(more))),
            ToolbarResponse::Opened
        );
        let mut frame = draw(&window, &mut toolbar, save.x + save.width + 60.0);
        let id = FocusId::new("toolbar");
        toolbar.access(&mut frame, id);
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n\
             \x20 Toolbar\n\
             \x20   Button \"Open\"\n\
             \x20   Button \"Save\"\n\
             \x20   Button \"More\" expanded\n\
             \x20     Menu\n\
             \x20       MenuItemRadio \"Grid\" checked\n\
             \x20       MenuItemRadio \"List\" unchecked\n\
             \x20       MenuItemCheckBox \"Wrap\" unchecked\n"
        );
        for _ in 0..3 {
            window.feed(&mut toolbar, &key("<down>"));
        }
        assert_eq!(
            window.feed(&mut toolbar, &key("<cr>")),
            ToolbarResponse::Activated("wrap")
        );
        assert!(
            toolbar.is_active("wrap"),
            "chosen from the menu, it flipped"
        );
        assert!(!toolbar.is_menu_open());
    }

    #[test]
    fn test_a_screen_reader_clicks_buttons_and_the_more_button() {
        let window = TestWindow::new(800, 400);
        let mut toolbar = toolbar();
        draw(&window, &mut toolbar, 800.0);
        let id = FocusId::new("toolbar");
        let list = id.child(3 * MEMBER_NODES + 1);
        assert_eq!(
            toolbar.access_action(id, AccessAction::Click(list)),
            ToolbarResponse::Activated("list")
        );
        assert_eq!(
            toolbar.access_action(id, AccessAction::Click(id.child(MORE_NODE))),
            ToolbarResponse::Ignored,
            "no more button while everything fits"
        );
    }

    #[test]
    fn test_a_toolbar_drawn_its_natural_width_shows_every_button_and_a_pixel_less_shows_more() {
        let window = TestWindow::new(800, 60);
        let mut toolbar = toolbar();
        let natural = toolbar.natural_width(&window.frame(), &Theme::dark());
        draw(&window, &mut toolbar, natural);
        let layout = toolbar.layout.as_ref().expect("drawn");
        assert_eq!(layout.buttons.len(), 5, "Open, Save, Grid, List, and Wrap");
        assert!(layout.more.is_none());
        draw(&window, &mut toolbar, natural - 1.0);
        assert!(toolbar.layout.as_ref().expect("drawn").more.is_some());
    }
}

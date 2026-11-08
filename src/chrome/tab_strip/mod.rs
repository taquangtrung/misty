//! [`TabStrip`]: a row of tabs with titles, the active one marked, a close
//! button on the active and the hovered tab, and a new-tab button after the
//! last tab.
//!
//! The strip keeps the list of tabs (an id and a title each) and which one
//! is active; what a tab shows is the app's, looked up by its [`TabId`].
//! The app offers the strip each window event with
//! [`TabStrip::handle`](crate::Widget::handle) and acts on the [`TabStripResponse`]: a click
//! activates a tab at once, while closing and opening tabs are only asked
//! for, so the app can confirm, keep at least one tab, or fill a new one.
//!
//! A tab dragged along the strip moves to where it is dropped: a marker
//! shows the gap it would land in, and the strip answers
//! [`TabStripResponse::Moved`]; [`TabStrip::move_tab`] and
//! [`TabStrip::move_active`] move one from the keyboard.
//!
//! A tab dragged off the strip is the app's to place, as editor groups
//! do: while it moves the strip answers [`TabStripResponse::Dragging`],
//! and dropped elsewhere [`TabStripResponse::DroppedOutside`], each with a
//! [`TabDrag`] saying which tab and where the pointer is. Over another
//! strip, show where it would land with [`TabStrip::show_drop_marker`] and
//! move it there with [`TabStrip::drop_index`] and [`TabStrip::insert`];
//! over a pane, [`DropZone::at`](crate::DropZone::at) says which side it
//! would split (or the center, to join that pane's tabs) and
//! [`draw_drop_target`](crate::draw_drop_target) over its
//! [`DropZone::rect`](crate::DropZone::rect) shows it.
//!
//! A tab may show an icon from the app's [`IconSet`] before its title
//! ([`TabStrip::set_icon`]), a spinner in the icon's place while its work
//! is under way ([`TabStrip::set_busy`]; call [`TabStrip::about_to_wait`]
//! so it turns), and a dot in place of its close button while it holds
//! unsaved changes ([`TabStrip::set_modified`]); the pointer over the tab
//! brings the close button back.
//!
//! A strip in a pane that does not have the focus, as one of several
//! editor groups, says so with [`TabStrip::set_focused`]: its active tab
//! loses the accent line, so the focused pane's tab is the one marked.
//!
//! Tabs never shrink below a legible width. When they do not all fit, the
//! strip scrolls by whole tabs (the wheel scrolls it, and activating a tab
//! brings it into view); a tab that would be cut by the edge is not drawn.

mod layout;

use std::time::Instant;

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::BoxStyle;
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::spinner;
use super::super::base::strings::Strings;
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton, WheelDelta};
use super::super::render::icon::{Icon, logical};
use super::super::render::image::IconSet;
use super::super::widgets::drag::{Drag, DragResponse, Dropped};
use super::super::widgets::label::{draw_styled_label, styled_label_width};

use super::super::render::font::CellMetrics;
use layout::{TabSlot, TabStripLayout, strip_height};

// ========================================================================
// Constants
// ========================================================================

/// The side of the close and new-tab icons, in logical pixels.
const ICON_SIZE: f32 = 8.0;

/// The thickness of icon strokes and of the strip's bottom border, in
/// logical pixels.
const STROKE: f32 = 1.0;

/// The thickness of the accent line along the active tab's top, in logical
/// pixels.
const ACCENT_THICKNESS: f32 = 2.0;

/// The width of the marker showing where a dragged tab would land, in
/// logical pixels.
const GAP_MARKER: f32 = 2.0;

/// The modified dot's side as a share of the close button's.
const MODIFIED_DOT_SHARE: f32 = 0.4;

/// The child index of the new-tab button's accessibility node; each tab's
/// is its id's number, which never reaches it.
const NEW_TAB_NODE: u64 = u64::MAX;

// ========================================================================
// Data Structures
// ========================================================================

/// A row of tabs the app owns. It keeps the tabs, the active one, the hover
/// and press state between events, and the layout of the last frame it
/// drew, which its hit testing uses.
#[derive(Clone, Debug, Default)]
pub struct TabStrip {
    /// The index of the active tab, or `None` when there are no tabs.
    active: Option<usize>,
    /// A tab being dragged to a new place.
    drag: Drag<TabId>,
    /// Where another strip's tab is dragged over this one, to mark the gap
    /// it would land in.
    drop_marker: Option<Point>,
    /// The index of the first tab shown.
    first: usize,
    /// What the pointer is over.
    hovered: Option<TabStripHit>,
    /// The icons tabs name.
    icons: IconSet,
    /// Where the parts were in the last frame drawn, or `None` before it.
    layout: Option<TabStripLayout>,
    /// The id the next tab gets.
    next_id: u64,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// The button press not yet released, and what it was on.
    pressed: Option<Press>,
    /// Whether the next frame scrolls the active tab into view.
    reveal_active: bool,
    /// Wheel movement in pixels not yet turned into a whole tab of scroll.
    scroll_rest: f32,
    /// When the busy tabs' spinners started turning, or `None` while no
    /// tab is busy.
    spinner_epoch: Option<Instant>,
    /// The words of the theme last drawn in, for the new-tab button's name.
    strings: Strings,
    /// The tabs, left to right.
    tabs: Vec<Tab>,
    /// Whether the strip's pane does not have the focus, which takes the
    /// accent off its active tab.
    unfocused: bool,
}

/// One tab: its id, the title it shows, and its marks.
#[derive(Clone, Debug, PartialEq)]
struct Tab {
    /// Whether its work is under way, which shows a spinner.
    busy: bool,
    /// The name of its icon in the strip's [`IconSet`], or `None`.
    icon: Option<String>,
    /// What the app knows the tab by.
    id: TabId,
    /// Whether it holds unsaved changes, which shows a dot.
    modified: bool,
    /// The text shown on the tab.
    title: String,
}

/// Names one tab of a [`TabStrip`] for as long as it is open. Ids are never
/// reused within a strip.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TabId(u64);

/// A press that acts when released over the same part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Press {
    /// The left button on a tab's close button.
    Close(TabId),
    /// The middle button on a tab, which closes it.
    Middle(TabId),
    /// The left button on the new-tab button.
    NewTab,
}

/// A tab dragged off its strip: which, and where the pointer is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabDrag {
    /// Where the pointer is, in physical pixels.
    pub position: Point,
    /// The tab dragged.
    pub tab: TabId,
}

/// What a point in the window means to a tab strip.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum TabStripHit {
    /// A tab's close button.
    Close(TabId),
    /// The new-tab button.
    NewTab,
    /// A tab, outside its close button.
    Tab(TabId),
}

/// What became of an input offered to a [`TabStrip`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum TabStripResponse {
    /// This tab was clicked and is now active: show its content.
    Activated(TabId),
    /// The user asked to close this tab, with its close button or a middle
    /// click. It is still open: call [`TabStrip::remove`] to close it.
    Close(TabId),
    /// A tab is being dragged and the pointer moved: show where it would
    /// land if it is off the strip.
    Dragging(TabDrag),
    /// A tab was dropped off the strip: move it to the strip or pane under
    /// the pointer, or leave it where it is.
    DroppedOutside(TabDrag),
    /// The strip used the event; the app should not act on it again.
    Handled,
    /// The event is not the strip's; the app handles it as usual.
    Ignored,
    /// This tab was dragged to a new place among the tabs.
    Moved(TabId),
    /// The new-tab button was clicked: add a tab with [`TabStrip::push`].
    NewTab,
}

// ========================================================================
// TabStrip Implementations
// ========================================================================

impl TabStrip {
    // --------------------------------------------------------------------
    // Tabs
    // --------------------------------------------------------------------

    /// A strip with no tabs.
    pub fn new() -> Self {
        Self::default()
    }

    /// How tall a strip is for cells of `metrics`, for laying out what sits
    /// below it before it is drawn. The scale factor is unused here; every
    /// bar takes it, so an app asks each bar's height the same way.
    pub fn height(metrics: CellMetrics, _scale_factor: f64) -> f32 {
        strip_height(metrics)
    }

    /// Where tab `id` was last drawn, for a tooltip to hang under it;
    /// `None` when it is scrolled out of sight, closed, or before the first
    /// frame.
    pub fn tab_rect(&self, id: TabId) -> Option<Rect> {
        let index = self.index_of(id)?;
        let layout = self.layout.as_ref()?;
        layout
            .slots
            .iter()
            .find(|slot| slot.index == index)
            .map(|slot| slot.body)
    }

    /// This strip drawing the icons tabs name from `icons`.
    pub fn with_icons(self, icons: IconSet) -> Self {
        Self { icons, ..self }
    }

    /// The icons tabs name, to add the ones new tabs bring as they come.
    pub fn icons_mut(&mut self) -> &mut IconSet {
        &mut self.icons
    }

    /// Adds a tab titled `title` at `index` (held to the end) and makes it
    /// active: a tab dragged in from another strip.
    pub fn insert(&mut self, index: usize, title: impl Into<String>) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        let index = index.min(self.tabs.len());
        self.tabs.insert(index, Tab::new(id, title.into()));
        self.set_active(index);
        id
    }

    /// The title of tab `id`, if it is open.
    pub fn title(&self, id: TabId) -> Option<&str> {
        self.index_of(id)
            .map(|index| self.tabs[index].title.as_str())
    }

    /// Moves the active tab `delta` places right (left when negative), held
    /// to the ends; whether it moved.
    pub fn move_active(&mut self, delta: isize) -> bool {
        let Some(active) = self.active else {
            return false;
        };
        let to = active
            .saturating_add_signed(delta)
            .min(self.tabs.len().saturating_sub(1));
        let id = self.tabs[active].id;
        to != active && self.move_tab(id, to)
    }

    /// The place a tab dropped at `point` would take, as the strip was last
    /// drawn: `None` when `point` is not over the strip.
    pub fn drop_index(&self, point: Point) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        match layout.gap_at(point) {
            Some((gap, _)) => Some(gap),
            None => layout.bar.contains(point).then_some(self.tabs.len()),
        }
    }

    /// Marks, on the next frames, the gap a tab dragged in from another
    /// strip would land in at `point`; `None` to stop.
    pub fn show_drop_marker(&mut self, point: Option<Point>) {
        self.drop_marker = point;
    }

    /// Adds a tab titled `title` after the last one and makes it active, so
    /// the next keystroke lands in it.
    pub fn push(&mut self, title: impl Into<String>) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        self.tabs.push(Tab::new(id, title.into()));
        self.set_active(self.tabs.len() - 1);
        id
    }

    /// Closes the tab `id`; `false` when no tab has that id. The active tab
    /// stays active; when it is the one closed, the tab that took its place
    /// becomes active, or the new last tab when it was last. Closing the
    /// only tab leaves the strip empty: an app that always shows a tab
    /// pushes a new one.
    pub fn remove(&mut self, id: TabId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        self.tabs.remove(index);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(active.min(self.tabs.len() - 1)),
            other => other,
        };
        self.reveal_active = true;
        true
    }

    /// Changes the title of the tab `id`; `false` when no tab has that id.
    pub fn set_title(&mut self, id: TabId, title: impl Into<String>) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        self.tabs[index].title = title.into();
        true
    }

    /// Shows the icon named `icon` in the strip's [`IconSet`] before the
    /// title of tab `id`, or none; `false` when no tab has that id.
    pub fn set_icon(&mut self, id: TabId, icon: Option<String>) -> bool {
        self.update_tab(id, |tab| tab.icon = icon)
    }

    /// Marks tab `id` as holding unsaved changes or not; `false` when no
    /// tab has that id.
    pub fn set_modified(&mut self, id: TabId, modified: bool) -> bool {
        self.update_tab(id, |tab| tab.modified = modified)
    }

    /// Marks tab `id` as busy, with a spinner in its icon's place, or not;
    /// `false` when no tab has that id. Ask for a redraw after.
    pub fn set_busy(&mut self, id: TabId, busy: bool) -> bool {
        let found = self.update_tab(id, |tab| tab.busy = busy);
        if self.tabs.iter().any(|tab| tab.busy) {
            self.spinner_epoch.get_or_insert_with(Instant::now);
        } else {
            self.spinner_epoch = None;
        }
        found
    }

    /// Makes the tab `id` active and scrolls it into view; `false` when no
    /// tab has that id.
    pub fn activate(&mut self, id: TabId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        self.set_active(index);
        true
    }

    /// Activates the tab `delta` places right of the active one (left when
    /// negative), wrapping around both ends, and returns it; `None` when
    /// there are no tabs.
    pub fn cycle(&mut self, delta: isize) -> Option<TabId> {
        let count = self.tabs.len();
        let active = self.active?;
        // `rem_euclid` keeps the step in range however large `delta` is.
        let step = delta.rem_euclid(count as isize) as usize;
        self.set_active((active + step) % count);
        self.active()
    }

    /// The active tab, or `None` when there are no tabs.
    pub fn active(&self) -> Option<TabId> {
        self.active.map(|index| self.tabs[index].id)
    }

    /// The tabs, left to right.
    pub fn ids(&self) -> impl Iterator<Item = TabId> + '_ {
        self.tabs.iter().map(|tab| tab.id)
    }

    /// How many tabs are open.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether no tab is open.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Moves tab `id` to `index` among the tabs (the last place when past
    /// the end), the others keeping their order and the same tab staying
    /// active. Whether the tab moved.
    pub fn move_tab(&mut self, id: TabId, index: usize) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let to = index.min(self.tabs.len() - 1);
        if from == to {
            return false;
        }
        let active = self.active();
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = active.and_then(|active| self.index_of(active));
        self.reveal_active = true;
        true
    }

    /// What `point` hits in the strip as last drawn, or `None` outside it
    /// or before the first frame.
    pub fn hit_test(&self, point: Point) -> Option<TabStripHit> {
        self.layout.as_ref()?.hit(point, &self.tabs)
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws one tab: its background, accent, title, and close button.
    fn draw_tab(&self, frame: &mut Frame, theme: &Theme, slot: &TabSlot, scale_factor: f64) {
        let tab = &self.tabs[slot.index];
        let is_active = self.active == Some(slot.index);
        let is_hovered = matches!(
            self.hovered,
            Some(TabStripHit::Tab(id) | TabStripHit::Close(id)) if id == tab.id
        );
        if is_active {
            frame.fill_rect(slot.body, theme.background);
            if !self.unfocused {
                let accent = logical(ACCENT_THICKNESS, scale_factor);
                let line = Rect::new(slot.body.x, slot.body.y, slot.body.width, accent);
                frame.fill_rect_over_text(line, theme.accent);
            }
        } else if is_hovered {
            frame.fill_rect(slot.body, theme.hover);
        }
        let text_color = match is_active {
            true => theme.foreground,
            false => theme.muted,
        };
        if let Some(lead) = slot.lead {
            self.draw_lead(frame, tab, lead, slot.title_origin.y, text_color);
        }
        // The layout sized the tab for its title as measured below; the
        // label is cut only when that room ran out.
        let clip = Rect::new(
            slot.title_origin.x,
            slot.body.y,
            slot.title_room,
            slot.body.height,
        );
        let title = title_label(tab, text_color);
        draw_styled_label(frame, theme, &title, slot.title_origin, clip);
        if tab.modified && !is_hovered {
            let side = (slot.close.width * MODIFIED_DOT_SHARE).round().max(1.0);
            let dot = Rect::new(
                (slot.close.x + (slot.close.width - side) / 2.0).round(),
                (slot.close.y + (slot.close.height - side) / 2.0).round(),
                side,
                side,
            );
            let style = BoxStyle {
                border: None,
                corner_radius: side / 2.0,
                fill: Rgba::from(text_color),
                shadow: None,
            };
            frame.draw_box(dot, &style);
            return;
        }
        if !(is_active || is_hovered) {
            return;
        }
        let close_lit = self.hovered == Some(TabStripHit::Close(tab.id))
            || self.pressed == Some(Press::Close(tab.id));
        if close_lit {
            frame.fill_rect(slot.close, theme.hover);
        }
        for stroke in Icon::Close.strokes(slot.close, ICON_SIZE, STROKE, scale_factor) {
            frame.fill_rect_over_text(stroke, text_color);
        }
    }

    /// Draws `tab`'s spinner while it is busy, else its icon, in `lead`;
    /// the spinner's glyph sits on the title's line, which starts at `top`.
    fn draw_lead(&self, frame: &mut Frame, tab: &Tab, lead: Rect, top: f32, color: Rgb) {
        if let (true, Some(epoch)) = (tab.busy, self.spinner_epoch) {
            let now = match frame.reduced_motion() {
                true => epoch,
                false => Instant::now(),
            };
            let glyph = spinner::glyph_at(epoch, now).to_string();
            frame.draw_plain_text(&glyph, Point::new(lead.x, top), color);
            return;
        }
        if let Some(name) = tab.icon.as_deref() {
            frame.draw_named_icon(&self.icons, name, lead, color);
        }
    }

    /// Draws the new-tab button.
    fn draw_new_tab(&self, frame: &mut Frame, theme: &Theme, layout: &TabStripLayout) {
        let lit = self.hovered == Some(TabStripHit::NewTab) || self.pressed == Some(Press::NewTab);
        let color = match lit {
            true => {
                frame.fill_rect(layout.new_tab, theme.hover);
                theme.foreground
            }
            false => theme.muted,
        };
        let strokes = Icon::Plus.strokes(layout.new_tab, ICON_SIZE, STROKE, layout.scale_factor);
        for stroke in strokes {
            frame.fill_rect_over_text(stroke, color);
        }
    }

    /// Draws, while a tab is dragged, the marker of the gap it would land
    /// in and its title beside the pointer.
    fn draw_drag(&self, frame: &mut Frame, theme: &Theme, layout: &TabStripLayout) {
        let dragged = self.drag.payload().zip(self.drag.position());
        let Some(pointer) = dragged.map(|(_, pointer)| pointer).or(self.drop_marker) else {
            return;
        };
        if let Some((_, x)) = layout.gap_at(pointer) {
            let width = logical(GAP_MARKER, layout.scale_factor);
            let marker = Rect::new(
                (x - width / 2.0).round(),
                layout.bar.y,
                width,
                layout.bar.height,
            );
            frame.overlay(|frame| frame.fill_rect(marker, theme.accent));
        }
        if let Some(index) = dragged.and_then(|(id, _)| self.index_of(*id)) {
            self.drag.draw(frame, &self.tabs[index].title, theme);
        }
    }

    /// Draws the line along the strip's bottom, open under the active tab
    /// so it joins the content below.
    fn draw_border(&self, frame: &mut Frame, theme: &Theme, layout: &TabStripLayout) {
        let bar = layout.bar;
        let stroke = logical(STROKE, layout.scale_factor);
        let y = bar.y + bar.height - stroke;
        let gap = layout
            .slots
            .iter()
            .find(|slot| self.active == Some(slot.index))
            .map(|slot| (slot.body.x, slot.body.x + slot.body.width));
        let (gap_start, gap_end) = gap.unwrap_or((bar.x, bar.x));
        let right = bar.x + bar.width;
        frame.fill_rect_over_text(Rect::new(bar.x, y, gap_start - bar.x, stroke), theme.border);
        frame.fill_rect_over_text(Rect::new(gap_end, y, right - gap_end, stroke), theme.border);
    }

    // --------------------------------------------------------------------
    // Accessibility
    // --------------------------------------------------------------------

    /// Describes the strip as last drawn to screen readers as `id`: a tab
    /// list holding every tab (named by its title, the active one
    /// selected, the hidden ones without bounds) and the new-tab button,
    /// all clickable. A tab's node is the child of `id` numbered by its
    /// [`TabId`]. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        frame.enter_access(id, layout.bar, Node::new(Role::TabList));
        for (index, tab) in self.tabs.iter().enumerate() {
            let rect = layout
                .slots
                .iter()
                .find(|slot| slot.index == index)
                .map_or_else(Rect::default, |slot| slot.body);
            let mut node = Node::new(Role::Tab);
            node.set_label(tab.title.as_str());
            node.set_selected(self.active == Some(index));
            node.add_action(Action::Click);
            frame.access(id.child(tab.id.0), rect, node);
        }
        let mut new_tab = Node::new(Role::Button);
        new_tab.set_label(self.strings.new_tab.as_ref());
        new_tab.add_action(Action::Click);
        frame.access(id.child(NEW_TAB_NODE), layout.new_tab, new_tab);
        frame.leave_access();
    }

    /// Does what a screen reader's `action` on the strip described as `id`
    /// asks: a click on a tab activates it, a click on the new-tab button
    /// asks for a tab. Ignored for other actions and nodes.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> TabStripResponse {
        let AccessAction::Click(target) = action else {
            return TabStripResponse::Ignored;
        };
        if target == id.child(NEW_TAB_NODE) {
            return TabStripResponse::NewTab;
        }
        let tab = self
            .tabs
            .iter()
            .map(|tab| tab.id)
            .find(|tab| id.child(tab.0) == target);
        match tab {
            Some(tab) => {
                self.activate(tab);
                TabStripResponse::Activated(tab)
            }
            None => TabStripResponse::Ignored,
        }
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// Moves the dropped tab to the gap it was dropped in; a drop off the
    /// strip is the app's.
    fn drop_tab(&mut self, dropped: Dropped<TabId>) -> TabStripResponse {
        let Some(layout) = &self.layout else {
            return TabStripResponse::Handled;
        };
        let Some((gap, _)) = layout.gap_at(dropped.position) else {
            return TabStripResponse::DroppedOutside(TabDrag {
                position: dropped.position,
                tab: dropped.payload,
            });
        };
        let Some(from) = self.index_of(dropped.payload) else {
            return TabStripResponse::Handled;
        };
        match self.move_tab(dropped.payload, index_for_gap(from, gap)) {
            true => TabStripResponse::Moved(dropped.payload),
            false => TabStripResponse::Handled,
        }
    }

    /// The index of the tab `id`.
    fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// Runs `change` on tab `id`; whether it is open.
    fn update_tab(&mut self, id: TabId, change: impl FnOnce(&mut Tab)) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        change(&mut self.tabs[index]);
        true
    }

    /// Makes the tab at `index` active and asks for it to be shown.
    fn set_active(&mut self, index: usize) {
        self.active = Some(index);
        self.reveal_active = true;
    }

    /// Whether a press on the strip is not yet released, or a tab is
    /// armed to drag or dragged: what the pointer does next is the
    /// strip's wherever it goes.
    pub(crate) fn is_engaged(&self) -> bool {
        self.pressed.is_some() || self.drag.payload().is_some()
    }

    /// Whether the pointer is over the strip as last drawn.
    fn is_pointer_inside(&self) -> bool {
        match (self.pointer, &self.layout) {
            (Some(point), Some(layout)) => layout.bar.contains(point),
            (None, _) | (_, None) => false,
        }
    }

    /// Moves the hover to `hit`, redrawing when it changed.
    fn set_hovered(&mut self, cx: &mut EventCx<'_>, hit: Option<TabStripHit>) {
        if self.hovered != hit {
            self.hovered = hit;
            cx.request_redraw();
        }
    }

    /// A press of `button` over `hit`.
    fn press(
        &mut self,
        cx: &mut EventCx<'_>,
        button: PointerButton,
        hit: Option<TabStripHit>,
    ) -> TabStripResponse {
        let Some(hit) = hit else {
            return match self.is_pointer_inside() {
                true => TabStripResponse::Handled,
                false => TabStripResponse::Ignored,
            };
        };
        cx.request_redraw();
        match (button, hit) {
            (PointerButton::Left, TabStripHit::Tab(id)) => {
                let was_active = self.active() == Some(id);
                self.activate(id);
                if let (Some(point), Some(layout)) = (self.pointer, &self.layout) {
                    self.drag.arm(id, point, layout.scale_factor);
                }
                match was_active {
                    true => TabStripResponse::Handled,
                    false => TabStripResponse::Activated(id),
                }
            }
            (PointerButton::Left, TabStripHit::Close(id)) => {
                self.pressed = Some(Press::Close(id));
                TabStripResponse::Handled
            }
            (PointerButton::Left, TabStripHit::NewTab) => {
                self.pressed = Some(Press::NewTab);
                TabStripResponse::Handled
            }
            (PointerButton::Middle, TabStripHit::Tab(id) | TabStripHit::Close(id)) => {
                self.pressed = Some(Press::Middle(id));
                TabStripResponse::Handled
            }
            (
                PointerButton::Back
                | PointerButton::Forward
                | PointerButton::Middle
                | PointerButton::Other(_)
                | PointerButton::Right,
                _,
            ) => TabStripResponse::Handled,
        }
    }

    /// A release over `hit`: completes the pending press if it is still
    /// over the part it began on.
    fn release(&mut self, cx: &mut EventCx<'_>, hit: Option<TabStripHit>) -> TabStripResponse {
        let Some(press) = self.pressed.take() else {
            return TabStripResponse::Ignored;
        };
        cx.request_redraw();
        match (press, hit) {
            (Press::Close(id), Some(TabStripHit::Close(over))) if id == over => {
                TabStripResponse::Close(id)
            }
            (Press::Middle(id), Some(TabStripHit::Tab(over) | TabStripHit::Close(over)))
                if id == over =>
            {
                TabStripResponse::Close(id)
            }
            (Press::NewTab, Some(TabStripHit::NewTab)) => TabStripResponse::NewTab,
            (Press::Close(_) | Press::Middle(_) | Press::NewTab, _) => TabStripResponse::Handled,
        }
    }

    /// Scrolls the tabs by the wheel's `delta`: a tab per line, or per strip
    /// height of pixels.
    fn scroll(&mut self, cx: &mut EventCx<'_>, delta: WheelDelta) {
        let Some(layout) = &self.layout else {
            return;
        };
        // Down and right both scroll toward the later tabs.
        let steps = match delta {
            WheelDelta::Lines(lines) => {
                let lines = if lines.x != 0.0 { lines.x } else { -lines.y };
                lines.signum() as isize * lines.abs().ceil() as isize
            }
            WheelDelta::Pixels(position) => {
                let pixels = if position.x != 0.0 {
                    position.x
                } else {
                    -position.y
                };
                self.scroll_rest += pixels;
                let whole = (self.scroll_rest / layout.bar.height.max(1.0)).trunc();
                self.scroll_rest -= whole * layout.bar.height;
                whole as isize
            }
        };
        let first = self
            .first
            .saturating_add_signed(steps)
            .min(layout.last_first);
        if first != self.first {
            self.first = first;
            cx.request_redraw();
        }
    }
}

impl Widget for TabStrip {
    type Output = Rect;
    type Response = TabStripResponse;

    /// Reacts to `input`: hover, clicks on tabs and buttons,
    /// middle-click to close, and the wheel to scroll tabs that do not fit.
    /// Offer it every input before acting on it, and leave alone what it
    /// handled.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TabStripResponse {
        match self.drag.handle_event(input, cx) {
            DragResponse::Cancelled => return TabStripResponse::Handled,
            DragResponse::Dropped(dropped) => return self.drop_tab(dropped),
            DragResponse::Ignored => {}
            DragResponse::Moved | DragResponse::Started => {
                self.pointer = self.drag.position();
                let (Some(&tab), Some(position)) = (self.drag.payload(), self.drag.position())
                else {
                    return TabStripResponse::Handled;
                };
                return TabStripResponse::Dragging(TabDrag { position, tab });
            }
        }
        match input {
            Input::PointerButton(button) => {
                self.pointer = Some(button.position);
                let hit = self.hit_test(button.position);
                match button.pressed {
                    true => self.press(cx, button.button, hit),
                    false => self.release(cx, hit),
                }
            }
            Input::PointerLeft => {
                self.pointer = None;
                self.set_hovered(cx, None);
                TabStripResponse::Ignored
            }
            Input::PointerMoved(point) => {
                self.pointer = Some(*point);
                self.set_hovered(cx, self.hit_test(*point));
                match self.is_pointer_inside() {
                    true => TabStripResponse::Handled,
                    false => TabStripResponse::Ignored,
                }
            }
            Input::Wheel(wheel) if self.is_pointer_inside() => {
                self.scroll(cx, wheel.delta);
                TabStripResponse::Handled
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
            | Input::Wheel(_) => TabStripResponse::Ignored,
        }
    }

    /// Draws the strip along the top of `area` in `theme`'s colors and
    /// returns the part of `area` below it, which is the app's to draw the
    /// active tab's content in.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        if self.strings != theme.strings {
            self.strings = theme.strings.clone();
        }
        let reveal = self.active.filter(|_| self.reveal_active);
        // Measured as they are drawn, so each tab is as wide as its title.
        let title_widths: Vec<f32> = self
            .tabs
            .iter()
            .map(|tab| styled_label_width(frame, theme, &title_label(tab, theme.foreground)))
            .collect();
        let layout = TabStripLayout::new(
            area,
            frame.cell_metrics(),
            frame.scale_factor(),
            &self.tabs,
            &title_widths,
            self.first,
            reveal,
        );
        self.first = layout.first;
        self.reveal_active = false;
        // Tabs may have moved under a pointer that has not: a closed tab's
        // neighbor slides under it.
        self.hovered = self.pointer.and_then(|point| layout.hit(point, &self.tabs));
        let bar = layout.bar;
        frame.fill_rect(bar, theme.surface);
        for slot in &layout.slots {
            self.draw_tab(frame, theme, slot, layout.scale_factor);
        }
        self.draw_new_tab(frame, theme, &layout);
        self.draw_border(frame, theme, &layout);
        self.draw_drag(frame, theme, &layout);
        self.layout = Some(layout);
        Rect::new(
            area.x,
            bar.y + bar.height,
            area.width,
            (area.height - bar.height).max(0.0),
        )
    }

    /// Says whether the strip's pane has the focus: an unfocused strip's
    /// active tab has no accent line. Ask for a redraw after.
    fn set_focused(&mut self, focused: bool) {
        self.unfocused = !focused;
    }

    /// Whether the app last said the strip's pane has the focus.
    fn is_focused(&self) -> bool {
        !self.unfocused
    }

    /// Asks `cx` for a redraw of its window while a busy tab's spinner
    /// turns, and
    /// returns when it next turns, for the app's
    /// [`Application::about_to_wait`](crate::Application::about_to_wait)
    /// to hand back; `None` while no tab is busy or motion is reduced.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let epoch = self.spinner_epoch.filter(|_| !cx.reduced_motion())?;
        let now = Instant::now();
        cx.request_redraw();
        Some(spinner::next_turn(epoch, now))
    }
}

impl Outcome for TabStripResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Tab Implementations
// ========================================================================

impl Tab {
    /// Tab `id` titled `title`, with no marks.
    fn new(id: TabId, title: String) -> Self {
        Self {
            busy: false,
            icon: None,
            id,
            modified: false,
            title,
        }
    }

    /// Whether the tab shows an icon or a spinner before its title.
    fn has_lead(&self) -> bool {
        self.busy || self.icon.is_some()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The index a tab now at `from` takes when dropped in gap `gap` (the gap
/// before the tab at that index, before the tab is taken out).
fn index_for_gap(from: usize, gap: usize) -> usize {
    match gap > from {
        true => gap - 1,
        false => gap,
    }
}

/// `tab`'s title as its label in `color`, ending in an ellipsis when cut.
fn title_label(tab: &Tab, color: Rgb) -> StyledText {
    StyledText::plain(tab.title.as_str(), TextColor::Rgba(color.into())).with_ellipsis()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::find_node;
    use crate::app::offscreen;
    use crate::base::theme::{ChromeText, Theme};

    /// A strip with tabs titled `a`, `b`, `c`; `c` is active.
    fn strip() -> (TabStrip, [TabId; 3]) {
        let mut strip = TabStrip::new();
        let ids = ["a", "b", "c"].map(|title| strip.push(title));
        (strip, ids)
    }

    #[test]
    fn test_a_wide_title_in_the_proportional_font_is_given_the_room_it_takes() {
        let theme = Theme {
            chrome_text: ChromeText::Proportional,
            ..Theme::dark()
        };
        let mut strip = TabStrip::new();
        strip.push("WWWWWWWW");
        let mut frame = offscreen::plain_frame(800, 100);
        strip.draw(&mut frame, Rect::new(0.0, 0.0, 800.0, 100.0), &theme);
        let title = title_label(&strip.tabs[0], theme.foreground);
        let width = styled_label_width(&frame, &theme, &title);
        let cells = title.width() as f32 * frame.cell_metrics().width;
        assert!(
            width > cells,
            "wider than its cells: {width} against {cells}"
        );
        let slot = &strip.layout.as_ref().expect("drawn").slots[0];
        assert!(slot.title_room >= width, "{slot:?} holds {width}");
    }

    #[test]
    fn test_a_pushed_tab_is_appended_and_active() {
        let (strip, [_, _, c]) = strip();
        assert_eq!(strip.active(), Some(c));
        assert_eq!(strip.ids().last(), Some(c));
    }

    #[test]
    fn test_closing_a_tab_before_the_active_one_keeps_the_same_tab_active() {
        let (mut strip, [a, b, _]) = strip();
        strip.activate(b);
        assert!(strip.remove(a));
        assert_eq!(strip.active(), Some(b));
    }

    #[test]
    fn test_closing_the_active_tab_activates_its_right_neighbor_or_the_last() {
        let (mut strip, [a, b, c]) = strip();
        strip.activate(b);
        strip.remove(b);
        assert_eq!(strip.active(), Some(c), "the tab that took its place");
        strip.remove(c);
        assert_eq!(strip.active(), Some(a), "it was last: the new last tab");
        strip.remove(a);
        assert_eq!(strip.active(), None);
        assert!(strip.is_empty());
        assert!(!strip.remove(a), "already closed");
    }

    #[test]
    fn test_cycling_wraps_around_both_ends() {
        let (mut strip, [a, b, c]) = strip();
        assert_eq!(strip.cycle(1), Some(a));
        assert_eq!(strip.cycle(-1), Some(c));
        assert_eq!(strip.cycle(-4), Some(b));
        assert_eq!(TabStrip::new().cycle(1), None);
    }

    #[test]
    fn test_moving_a_tab_keeps_the_others_in_order_and_the_active_tab_active() {
        let (mut strip, [a, b, c]) = strip();
        strip.activate(b);
        assert!(strip.move_tab(a, 9), "past the end moves it last");
        assert_eq!(strip.ids().collect::<Vec<_>>(), [b, c, a]);
        assert_eq!(strip.active(), Some(b));
        assert!(strip.move_tab(b, 2));
        assert_eq!(strip.ids().collect::<Vec<_>>(), [c, a, b]);
        assert_eq!(strip.active(), Some(b), "the active tab moved itself");
        assert!(!strip.move_tab(b, 2), "already there");
    }

    #[test]
    fn test_a_gap_after_the_dragged_tab_counts_it_out() {
        assert_eq!(index_for_gap(0, 2), 1, "a dragged right past its neighbor");
        assert_eq!(index_for_gap(0, 1), 0, "the gap right after it: no move");
        assert_eq!(index_for_gap(2, 0), 0);
        assert_eq!(index_for_gap(2, 3), 2, "the end, where it already is");
    }

    #[test]
    fn test_ids_are_not_reused_after_a_close() {
        let (mut strip, [_, _, c]) = strip();
        strip.remove(c);
        let d = strip.push("d");
        assert_ne!(d, c);
    }

    #[test]
    fn test_the_active_tab_joins_the_content_and_others_sit_on_the_surface() {
        let theme = Theme::dark();
        let (mut strip, [a, ..]) = strip();
        strip.activate(a);
        let area = Rect::new(0.0, 0.0, 480.0, 80.0);
        let Some(snapshot) = offscreen::render(480, 80, |frame| {
            strip.draw(frame, area, &theme);
        }) else {
            return;
        };
        let layout = strip.layout.as_ref().expect("drawn");
        let (active, inactive) = (layout.slots[0].body, layout.slots[1].body);
        let bottom = layout.bar.y + layout.bar.height - 1.0;
        snapshot.assert_pixel(active.x + 1.0, active.y, theme.accent, "the accent line");
        snapshot.assert_pixel(
            active.x + 1.0,
            active.y + active.height / 2.0,
            theme.background,
            "the active tab",
        );
        snapshot.assert_pixel(active.x + 1.0, bottom, theme.background, "the gap");
        snapshot.assert_pixel(inactive.x + 1.0, active.y + 1.0, theme.surface, "a tab");
        snapshot.assert_pixel(inactive.x + 1.0, bottom, theme.border, "the border");
    }

    #[test]
    fn test_an_unfocused_strip_s_active_tab_has_no_accent() {
        let theme = Theme::dark();
        let (mut strip, [a, ..]) = strip();
        strip.activate(a);
        strip.set_focused(false);
        let area = Rect::new(0.0, 0.0, 480.0, 80.0);
        let Some(snapshot) = offscreen::render(480, 80, |frame| {
            strip.draw(frame, area, &theme);
        }) else {
            return;
        };
        let active = strip.tab_rect(a).expect("drawn");
        snapshot.assert_pixel(active.x + 1.0, active.y, theme.background, "no accent");
    }

    #[test]
    fn test_a_tab_s_rect_is_where_it_was_drawn_and_the_height_is_the_strip_s() {
        let (mut strip, [a, b, _]) = strip();
        let mut frame = offscreen::plain_frame(600, 100);
        strip.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 600.0, 100.0),
            &Theme::dark(),
        );
        let layout = strip.layout.clone().expect("drawn");
        assert_eq!(strip.tab_rect(a), Some(layout.slots[0].body));
        assert_eq!(strip.tab_rect(b), Some(layout.slots[1].body));
        assert_eq!(
            TabStrip::height(frame.cell_metrics(), frame.scale_factor()),
            layout.bar.height
        );
        strip.remove(a);
        assert_eq!(strip.tab_rect(a), None);
    }

    #[test]
    fn test_a_modified_tab_shows_a_dot_until_the_pointer_comes_over_it() {
        let theme = Theme::dark();
        let (mut strip, [a, b, _]) = strip();
        strip.set_modified(a, true);
        let area = Rect::new(0.0, 0.0, 480.0, 80.0);
        let render = |strip: &mut TabStrip| {
            offscreen::render(480, 80, |frame| {
                strip.draw(frame, area, &theme);
            })
        };
        let Some(dotted) = render(&mut strip) else {
            return;
        };
        let close = strip.layout.as_ref().expect("drawn").slots[0].close;
        let middle = |close: Rect| (close.x + close.width / 2.0, close.y + close.height / 2.0);
        // Beside the middle: inside the dot, off the close cross's strokes.
        let (x, y) = middle(close);
        let (x, y) = (x + 2.0, y);
        dotted.assert_pixel(x, y, theme.muted, "the dot on an inactive tab");
        strip.hovered = Some(TabStripHit::Tab(a));
        strip.pointer = Some(Point::new(close.x - 4.0, y));
        let Some(hovered) = render(&mut strip) else {
            return;
        };
        hovered.assert_pixel(x, y, theme.hover, "the close button in the dot's place");
        assert!(strip.set_modified(b, false));
        assert!(!strip.set_modified(TabId(99), true), "no such tab");
    }

    #[test]
    fn test_a_busy_tab_keeps_the_loop_turning_until_its_work_ends() {
        let mut window = crate::testing::TestWindow::new(600, 300);
        let (mut strip, [a, b, _]) = strip();
        let mut wait = |strip: &mut TabStrip, reduced_motion: bool| {
            window.set_reduced_motion(reduced_motion);
            strip.about_to_wait(&mut window.cx())
        };
        assert_eq!(wait(&mut strip, false), None, "nothing busy");
        strip.set_busy(a, true);
        strip.set_busy(b, true);
        let turn = wait(&mut strip, false).expect("a spinner turns");
        assert!(turn <= Instant::now() + spinner::FRAME);
        strip.set_busy(a, false);
        assert!(wait(&mut strip, false).is_some(), "one still busy");
        assert_eq!(
            wait(&mut strip, true),
            None,
            "a still spinner needs no frames"
        );
        strip.set_busy(b, false);
        assert_eq!(wait(&mut strip, false), None);
    }

    #[test]
    fn test_the_new_tab_button_takes_its_name_from_the_theme_last_drawn_in() {
        let id = FocusId::new("tabs");
        let (mut strip, _) = strip();
        let theme = Theme {
            strings: Strings {
                new_tab: "Neuer Tab".into(),
                ..Strings::english()
            },
            ..Theme::dark()
        };
        let mut frame = offscreen::plain_frame(600, 100);
        strip.draw(&mut frame, Rect::new(0.0, 0.0, 600.0, 100.0), &theme);
        strip.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let button = find_node(&update, id.child(NEW_TAB_NODE)).expect("described");
        assert_eq!(button.label(), Some("Neuer Tab"));
    }

    #[test]
    fn test_the_strip_describes_its_tabs_and_a_click_activates_one() {
        let id = FocusId::new("tabs");
        let (mut strip, [a, b, c]) = strip();
        let mut frame = offscreen::plain_frame(600, 100);
        strip.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 600.0, 100.0),
            &Theme::dark(),
        );
        strip.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let node = |id| find_node(&update, id).expect("described");
        assert_eq!(node(id).role(), Role::TabList);
        let tab_b = node(id.child(b.0));
        assert_eq!(
            (tab_b.role(), tab_b.label(), tab_b.is_selected()),
            (Role::Tab, Some("b"), Some(false))
        );
        assert_eq!(node(id.child(c.0)).is_selected(), Some(true));
        assert_eq!(
            strip.access_action(id, AccessAction::Click(id.child(a.0))),
            TabStripResponse::Activated(a)
        );
        assert_eq!(strip.active(), Some(a));
        assert_eq!(
            strip.access_action(id, AccessAction::Click(id.child(NEW_TAB_NODE))),
            TabStripResponse::NewTab
        );
        assert_eq!(
            strip.access_action(id, AccessAction::Focus(id)),
            TabStripResponse::Ignored
        );
    }

    /// `strip` drawn across the top of a 600 by 300 window, returning its
    /// bar.
    fn drawn(strip: &mut TabStrip, window: &crate::testing::TestWindow) -> Rect {
        let mut frame = window.frame();
        let area = Rect::new(0.0, 0.0, 600.0, 300.0);
        strip.draw(&mut frame, area, &Theme::dark());
        strip.layout.as_ref().expect("drawn").bar
    }

    #[test]
    fn test_a_tab_dragged_off_the_strip_is_reported_with_where_it_was_dropped() {
        use crate::input::event::ButtonInput;
        let mut window = crate::testing::TestWindow::new(600, 300);
        let (mut strip, [a, _, _]) = strip();
        let bar = drawn(&mut strip, &window);
        let on_a = Point::new(bar.x + 10.0, bar.y + bar.height / 2.0);
        let below = Point::new(300.0, 200.0);
        window.feed(
            &mut strip,
            &Input::PointerButton(ButtonInput::press(PointerButton::Left, on_a)),
        );
        let moving = window.feed(&mut strip, &Input::PointerMoved(below));
        assert_eq!(
            moving,
            TabStripResponse::Dragging(TabDrag {
                position: below,
                tab: a
            })
        );
        let dropped = window.feed(
            &mut strip,
            &Input::PointerButton(ButtonInput::release(PointerButton::Left, below)),
        );
        assert_eq!(
            dropped,
            TabStripResponse::DroppedOutside(TabDrag {
                position: below,
                tab: a
            })
        );
        assert_eq!(
            strip.ids().next(),
            Some(a),
            "the app moves it, not the strip"
        );
    }

    #[test]
    fn test_another_strip_takes_a_dropped_tab_at_the_gap_under_the_pointer() {
        let window = crate::testing::TestWindow::new(600, 300);
        let (mut strip, [a, b, _]) = strip();
        let bar = drawn(&mut strip, &window);
        let middle = Point::new(bar.x + 1.0, bar.y + bar.height / 2.0);
        assert_eq!(strip.drop_index(middle), Some(0));
        assert_eq!(
            strip.drop_index(Point::new(10.0, 250.0)),
            None,
            "off the strip"
        );
        let moved = strip.insert(1, "moved");
        assert_eq!(strip.ids().collect::<Vec<_>>()[..2], [a, moved]);
        assert_eq!(strip.active(), Some(moved));
        assert_eq!(strip.title(moved), Some("moved"));
        let mut empty = TabStrip::new();
        drawn(&mut empty, &window);
        assert_eq!(
            empty.drop_index(middle),
            Some(0),
            "an empty strip takes it first"
        );
        strip.activate(b);
        assert!(strip.move_active(-5));
        assert_eq!(strip.ids().next(), Some(b), "held to the start");
        assert!(!strip.move_active(-1));
    }
}

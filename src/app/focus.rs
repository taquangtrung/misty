//! [`FocusTree`]: which of the widgets composed in one window has the
//! keyboard and which is under the pointer, for an app that draws several
//! (a tab strip, a list, an editor, a dialog over them), and the
//! dispatcher that hands each [`Input`] to the widget it is for.
//!
//! Misty's widgets are drawn by the app, frame by frame, so the tree is
//! rebuilt the same way: while rendering, the app registers each widget
//! under a [`FocusId`] of its choosing, with the rectangle it was drawn
//! in, between [`FocusTree::begin_frame`] and [`FocusTree::end_frame`]:
//! [`FocusTree::register`] for a widget that takes the keyboard,
//! [`FocusTree::register_hit`] for one that only takes the pointer. [`FocusTree::enter`] and
//! [`FocusTree::leave`] nest widgets in a container (a panel, a dialog),
//! so keys a widget leaves bubble to the containers around it.
//!
//! - **Tab order** is the order widgets were registered in, siblings
//!   sorted by an explicit order where the app gave one with
//!   [`FocusTree::set_order`]. Tab and Shift+Tab move along it, wrapping,
//!   when the focused widget does not use them itself. A container with
//!   widgets in it is not a stop of its own; an empty one is.
//! - **Keys** go to the focused widget first, then to each container
//!   around it, innermost first, through [`FocusTree::dispatch`]; what
//!   none takes comes back for the app's keymap.
//! - **Pointer input** goes to the topmost widget under the pointer and
//!   down through those under it until one takes it, through the same
//!   [`FocusTree::dispatch`]: widgets registered later are above those
//!   registered before (register in paint order), and the open traps are
//!   above everything else, so overlap is settled by the framework rather
//!   than by the order the app happens to offer inputs in.
//! - **Traps** keep the focus inside a dialog or popup while it is open:
//!   [`FocusTree::open_trap`] moves the focus into it, Tab cycles inside
//!   it, a click outside it focuses nothing, and keys never bubble past
//!   it. [`FocusTree::close_trap`] (or simply no longer drawing the
//!   trapped widget) gives the focus back to the widget that had it
//!   before.
//! - **Each widget's focus** follows the tree through one call a frame,
//!   before the widget is drawn: [`FocusTree::sync`] tells a widget
//!   whether it has the keyboard, [`FocusTree::sync_within`] tells a
//!   container whether anything inside it does (both through
//!   [`Widget::set_focused`]); only a widget that answers to several ids
//!   of its own (a select and its open list) is told by hand, focused when
//!   any of them is. Widgets never query the tree themselves.
//! - **The focus ring** ([`FocusTree::draw`]) outlines the focused
//!   widget in the theme's accent color after the focus moved by the
//!   keyboard, and hides after a click, as browsers show `:focus-visible`.

use std::collections::HashMap;

use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::icon::{logical, outline};
use super::access::Focusable;
use super::frame::Frame;
use super::widget::Widget;

// ========================================================================
// Constants
// ========================================================================

/// The FNV-1a offset basis, where a [`FocusId`] hash starts.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// The FNV-1a prime, which mixes each byte into a [`FocusId`] hash.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The thickness of the focus ring, in logical pixels.
const RING_STROKE: f32 = 2.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The focusable widgets of one window and which has the keyboard.
#[derive(Clone, Debug, Default)]
pub struct FocusTree {
    /// The widgets being registered for the frame being drawn.
    building: Vec<FocusNode>,
    /// The widget that took the pointer press still held, which keeps
    /// the pointer until the release.
    captured: Option<FocusId>,
    /// The widget with the keyboard, if any.
    focused: Option<FocusId>,
    /// The widgets the last pointer move reached, to tell when it leaves
    /// them.
    hovered: Vec<FocusId>,
    /// The widgets registered in the last frame drawn, in registration
    /// order.
    nodes: Vec<FocusNode>,
    /// The explicit tab order of the widgets the app gave one.
    orders: HashMap<FocusId, i32>,
    /// Whether the focus ring shows: after the keyboard moved the focus,
    /// not after a click.
    ring_visible: bool,
    /// The containers open while registering, innermost last.
    stack: Vec<FocusId>,
    /// The open traps, innermost last.
    traps: Vec<Trap>,
}

/// Names one focusable widget. An app makes its ids with
/// [`FocusId::new`], usually as constants, and [`FocusId::child`] for
/// widgets it has many of (one per tab, say).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FocusId(u64);

/// Where an input handed out by [`FocusTree::dispatch`] went.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Route {
    /// The widget or container with this id took it.
    Delivered(FocusId),
    /// No widget took it; it was Tab or Shift+Tab and moved the focus.
    Moved,
    /// No widget took the key, and a trap is open: it goes nowhere, since
    /// the app behind a dialog must not act on it.
    Trapped,
    /// No widget took it: the app's keymap, or the app, may.
    Unhandled,
}

/// One registered widget.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FocusNode {
    /// Whether it takes the keyboard, rather than only the pointer.
    focusable: bool,
    /// Whether widgets were registered inside it.
    has_children: bool,
    /// Its id.
    id: FocusId,
    /// The container it was registered in, if any.
    parent: Option<FocusId>,
    /// Where it was drawn, in physical pixels.
    rect: Rect,
}

/// An open trap: the focus stays inside one widget until it closes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Trap {
    /// The widget the focus is kept inside.
    id: FocusId,
    /// The widget focused when the trap opened, focused again when it
    /// closes.
    restore: Option<FocusId>,
    /// Whether the focus ring showed when the trap opened.
    restore_ring: bool,
}

// ========================================================================
// FocusId Implementations
// ========================================================================

impl FocusId {
    /// The id named `name`: the same name always makes the same id, so an
    /// id can be a constant.
    pub const fn new(name: &str) -> Self {
        Self(fnv(FNV_OFFSET, name.as_bytes()))
    }

    /// The id of the `index`th child of this one, for widgets an app has
    /// many of: the same parent and index always make the same id.
    pub const fn child(self, index: u64) -> Self {
        Self(fnv(self.0, &index.to_le_bytes()))
    }

    /// The id as a number, unique among the ids of one app as far as a
    /// 64-bit hash goes.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The id whose number is `raw`, as [`FocusId::get`] gave it.
    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

// ========================================================================
// FocusTree Implementations
// ========================================================================

impl FocusTree {
    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// A tree with nothing registered and nothing focused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts registering the widgets of a new frame. Call it at the top
    /// of [`Application::render`](crate::Application::render).
    pub fn begin_frame(&mut self) {
        self.building.clear();
        self.stack.clear();
    }

    /// Registers the widget `id`, drawn in `rect`, in the container open
    /// now (if any), after the widgets registered before it: a tab stop,
    /// and above them for the pointer.
    pub fn register(&mut self, id: FocusId, rect: Rect) {
        self.add(id, rect, true);
    }

    /// Registers the widget `id`, drawn in `rect`, for the pointer only:
    /// above the widgets registered before it, but never a tab stop nor
    /// focused by a click (a title bar, a status bar with buttons).
    pub fn register_hit(&mut self, id: FocusId, rect: Rect) {
        self.add(id, rect, false);
    }

    /// Registers `id` in `rect`, taking the keyboard when `focusable`.
    fn add(&mut self, id: FocusId, rect: Rect, focusable: bool) {
        let parent = self.stack.last().copied();
        if let Some(parent) = parent
            && let Some(node) = self
                .building
                .iter_mut()
                .rev()
                .find(|node| node.id == parent)
        {
            node.has_children = true;
        }
        self.building.push(FocusNode {
            focusable,
            has_children: false,
            id,
            parent,
            rect,
        });
    }

    /// Registers the container `id`, drawn in `rect`, and opens it: the
    /// widgets registered until [`FocusTree::leave`] are inside it, and
    /// show only within it: the pointer reaches them and the ring outlines
    /// them only where they lie inside `rect`, so a scrolled view's rows
    /// stop at its edges. An open trap inside a container (a popup opened
    /// from a row) is not cut to it.
    pub fn enter(&mut self, id: FocusId, rect: Rect) {
        self.register(id, rect);
        self.stack.push(id);
    }

    /// Closes the container [`FocusTree::enter`] opened last.
    pub fn leave(&mut self) {
        self.stack.pop();
    }

    /// Ends the frame's registrations: they replace the last frame's.
    /// Traps whose widget was not registered close, giving the focus
    /// back; a focus that is gone, outside the innermost trap, or on a
    /// container with widgets in it moves to the first stop it can.
    pub fn end_frame(&mut self) {
        self.nodes = std::mem::take(&mut self.building);
        self.stack.clear();
        while let Some(trap) = self.traps.last().copied() {
            if self.node(trap.id).is_some() {
                break;
            }
            self.close_trap(trap.id);
        }
        self.settle_focus();
    }

    /// Gives `id` the tab order `order` among its siblings, lowest first;
    /// a widget without one has order 0, and siblings of equal order keep
    /// their registration order. Kept across frames.
    pub fn set_order(&mut self, id: FocusId, order: i32) {
        self.orders.insert(id, order);
    }

    // --------------------------------------------------------------------
    // Focus
    // --------------------------------------------------------------------

    /// The widget with the keyboard, if any.
    pub fn focused(&self) -> Option<FocusId> {
        self.focused
    }

    /// Whether `id` has the keyboard.
    pub fn is_focused(&self, id: FocusId) -> bool {
        self.focused == Some(id)
    }

    /// Whether `id` or a widget inside it has the keyboard.
    pub fn contains_focus(&self, id: FocusId) -> bool {
        self.focused
            .is_some_and(|focused| self.is_inside(focused, id))
    }

    /// Tells `widget`, registered as `id`, whether it has the keyboard
    /// ([`FocusTree::is_focused`]), made each frame before drawing it.
    /// Which call keeps a widget's focus in step with the tree depends on
    /// how it is registered: this one for a widget with one id;
    /// [`FocusTree::sync_within`] for a container entered as `id` with
    /// widgets inside it; and [`Widget::set_focused`] by hand only for a
    /// widget that answers to several ids of its own (a select registered
    /// apart from its open list), told it has the keyboard when any of them
    /// does.
    pub fn sync(&self, id: FocusId, widget: &mut (impl Widget + ?Sized)) {
        widget.set_focused(self.is_focused(id));
    }

    /// Tells `widget`, the container entered as `id`, whether it or a
    /// widget inside it has the keyboard ([`FocusTree::contains_focus`]):
    /// a scrolling form takes the keys its controls leave while any of
    /// them is focused.
    pub fn sync_within(&self, id: FocusId, widget: &mut (impl Widget + ?Sized)) {
        widget.set_focused(self.contains_focus(id));
    }

    /// Gives `id` the keyboard, unless a trap is open that `id` is outside
    /// of. Whether it did. The ring shows, as for a move by the keyboard;
    /// a container with widgets in it passes the focus to its first stop
    /// when the frame ends.
    pub fn focus(&mut self, id: FocusId) -> bool {
        if self
            .trap_root()
            .is_some_and(|root| self.node(id).is_some() && !self.is_inside(id, root))
        {
            return false;
        }
        self.focused = Some(id);
        self.ring_visible = true;
        true
    }

    /// Moves the focus `delta` stops along the tab order of the innermost
    /// trap, or of the whole window when none is open, wrapping. The ring
    /// shows.
    pub fn move_focus(&mut self, delta: isize) {
        let stops = self.tab_stops();
        if stops.is_empty() {
            return;
        }
        let count = stops.len() as isize;
        let next = match self
            .focused
            .and_then(|focused| stops.iter().position(|stop| *stop == focused))
        {
            Some(index) => (index as isize + delta).rem_euclid(count) as usize,
            None if delta < 0 => stops.len() - 1,
            None => 0,
        };
        self.focused = Some(stops[next]);
        self.ring_visible = true;
    }

    /// The widgets Tab visits, in order, inside the innermost trap or the
    /// whole window.
    pub fn tab_stops(&self) -> Vec<FocusId> {
        let mut stops = Vec::new();
        match self.trap_root() {
            Some(root) => self.collect_stops(root, &mut stops),
            None => {
                for root in self.children_of(None) {
                    self.collect_stops(root, &mut stops);
                }
            }
        }
        stops
    }

    // --------------------------------------------------------------------
    // Traps
    // --------------------------------------------------------------------

    /// Keeps the focus inside the widget `id` (a dialog, a popup) until
    /// [`FocusTree::close_trap`], or until a frame no longer registers it,
    /// and gives it the focus now: its first stop once it is drawn. The
    /// ring hides, since a dialog or popup marks its own focus, until Tab
    /// moves the focus inside it.
    pub fn open_trap(&mut self, id: FocusId) {
        self.traps.push(Trap {
            id,
            restore: self.focused,
            restore_ring: self.ring_visible,
        });
        self.focused = Some(id);
        self.ring_visible = false;
        if self.node(id).is_some() {
            self.settle_focus();
        }
    }

    /// Closes the trap on `id`, and any opened inside it since, giving the
    /// focus back to the widget that had it when the trap opened. Nothing
    /// when no trap on `id` is open.
    pub fn close_trap(&mut self, id: FocusId) {
        let Some(index) = self.traps.iter().rposition(|trap| trap.id == id) else {
            return;
        };
        let trap = self.traps[index];
        self.traps.truncate(index);
        self.focused = trap.restore;
        self.ring_visible = trap.restore_ring;
    }

    /// Whether a trap is open.
    pub fn is_trapped(&self) -> bool {
        !self.traps.is_empty()
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Hands `input` to the widget it is for, through `deliver`, which
    /// gives the input to the widget with the id given and says whether
    /// it took it; returns where the input went.
    ///
    /// - **Keys and input method text** go to the focused widget, then to
    ///   each container around it up to the innermost trap. A key none
    ///   takes moves the focus when it is Tab or Shift+Tab, goes nowhere
    ///   inside a trap, and comes back unhandled otherwise.
    /// - **Pointer input** goes to the topmost widget under the pointer,
    ///   then down through the widgets under it until one takes it: inside
    ///   the innermost trap only while one is open, and to the trap itself
    ///   when the pointer is outside it, so a popup sees the click that
    ///   dismisses it. A widget registered later is above one registered
    ///   earlier, and a trap above everything outside it. A move reaches
    ///   every widget under the pointer, and a widget the pointer left gets
    ///   [`Input::PointerLeft`]. A press focuses the innermost focusable
    ///   widget under it (hiding the ring), and the widget that took it
    ///   keeps the pointer until the button is released, so a drag goes on
    ///   past its edges.
    /// - **The window gaining or losing focus**, or files dragged over it
    ///   being taken away, reach every registered widget, then come back
    ///   unhandled for the app as well; a close request and an appearance
    ///   change are the app's.
    pub fn dispatch(
        &mut self,
        input: &Input,
        mut deliver: impl FnMut(FocusId, &Input) -> bool,
    ) -> Route {
        match input {
            Input::Ime(_) | Input::Key(_) | Input::KeyReleased(_) => {
                self.route_focused(input, &mut deliver)
            }
            Input::FileHoverCancelled | Input::FocusGained | Input::FocusLost => {
                if *input == Input::FocusLost {
                    self.captured = None;
                }
                let ids: Vec<FocusId> = self.nodes.iter().map(|node| node.id).collect();
                for id in ids {
                    deliver(id, input);
                }
                Route::Unhandled
            }
            Input::PointerLeft => {
                for id in std::mem::take(&mut self.hovered) {
                    deliver(id, input);
                }
                Route::Unhandled
            }
            Input::PointerMoved(point) => self.route_move(*point, input, &mut deliver),
            Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => self.route_pointer(input, &mut deliver),
            Input::AppearanceChanged(_) | Input::CloseRequested => Route::Unhandled,
        }
    }

    /// Hands `input`, a key or input method text, to the focused widget
    /// and the containers around it, as [`FocusTree::dispatch`] says.
    fn route_focused(
        &mut self,
        input: &Input,
        deliver: &mut impl FnMut(FocusId, &Input) -> bool,
    ) -> Route {
        let root = self.trap_root();
        let mut current = self.focused;
        while let Some(id) = current {
            if deliver(id, input) {
                return Route::Delivered(id);
            }
            if Some(id) == root {
                break;
            }
            current = self.node(id).and_then(|node| node.parent);
        }
        let tab = input
            .key()
            .filter(|key| key.code == KeyCode::Tab && matches!(key.mods, Mods::NONE | Mods::SHIFT));
        if let Some(key) = tab {
            let delta = match key.mods == Mods::SHIFT {
                true => -1,
                false => 1,
            };
            self.move_focus(delta);
            return Route::Moved;
        }
        match root {
            Some(_) => Route::Trapped,
            None => Route::Unhandled,
        }
    }

    /// Hands the move to `point` to every widget under it (or only to the
    /// one holding the pointer), telling those it left.
    fn route_move(
        &mut self,
        point: Point,
        input: &Input,
        deliver: &mut impl FnMut(FocusId, &Input) -> bool,
    ) -> Route {
        let under = match self.captured {
            Some(id) => vec![id],
            None => self.under(point),
        };
        let left: Vec<FocusId> = self
            .hovered
            .iter()
            .copied()
            .filter(|id| !under.contains(id))
            .collect();
        for id in left {
            deliver(id, &Input::PointerLeft);
        }
        let mut taker = None;
        for id in &under {
            if deliver(*id, input) && taker.is_none() {
                taker = Some(*id);
            }
        }
        self.hovered = under;
        taker.map_or(Route::Unhandled, Route::Delivered)
    }

    /// Hands a press, release, wheel turn, or file to the widgets under
    /// the pointer, topmost first, until one takes it, as
    /// [`FocusTree::dispatch`] says.
    fn route_pointer(
        &mut self,
        input: &Input,
        deliver: &mut impl FnMut(FocusId, &Input) -> bool,
    ) -> Route {
        let release = matches!(input, Input::PointerButton(button) if !button.pressed);
        if let Some(id) = self.captured {
            if release {
                self.captured = None;
            }
            deliver(id, input);
            return Route::Delivered(id);
        }
        let Some(point) = input.position() else {
            return Route::Unhandled;
        };
        if let Some(press) = input.press(PointerButton::Left) {
            self.press(press.position);
        }
        let pressed = matches!(input, Input::PointerButton(button) if button.pressed);
        for id in self.under(point) {
            if deliver(id, input) {
                if pressed {
                    self.captured = Some(id);
                }
                return Route::Delivered(id);
            }
        }
        Route::Unhandled
    }

    /// The widgets under `point` that pointer input reaches, topmost
    /// first: inside the innermost trap while one is open (the trap itself
    /// when nothing inside it is under the point).
    fn under(&self, point: Point) -> Vec<FocusId> {
        let root = self.trap_root();
        let under: Vec<FocusId> = self
            .nodes
            .iter()
            .rev()
            .filter(|node| root.is_none_or(|root| self.is_inside(node.id, root)))
            .filter(|node| {
                self.visible_rect(node)
                    .is_some_and(|rect| rect.contains(point))
            })
            .map(|node| node.id)
            .collect();
        match (under.is_empty(), root) {
            (true, Some(root)) => vec![root],
            (true, None) | (false, _) => under,
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Outlines the focused widget in `theme`'s accent color just outside
    /// its rectangle (kept inside the window), over everything else, while
    /// the ring shows, and tells the frame's accessibility tree which
    /// widgets take the focus and which has it. Call it after
    /// [`FocusTree::end_frame`], so it outlines where the widget was drawn
    /// in this frame.
    pub fn draw(&self, frame: &mut Frame, theme: &Theme) {
        let focusables = self
            .nodes
            .iter()
            .filter(|node| node.focusable)
            .map(|node| Focusable {
                id: node.id,
                parent: node.parent,
                rect: node.rect,
            })
            .collect();
        frame.access.set_focus(focusables, self.focused);
        let Some(rect) = self.ring_rect() else {
            return;
        };
        let stroke = logical(RING_STROKE, frame.scale_factor());
        let around = Rect::new(
            rect.x - stroke,
            rect.y - stroke,
            rect.width + 2.0 * stroke,
            rect.height + 2.0 * stroke,
        );
        let Some(ring) = around.intersection(&frame.bounds()) else {
            return;
        };
        frame.overlay(|frame| {
            for edge in outline(ring, stroke) {
                frame.fill_rect_over_text(edge, theme.accent);
            }
        });
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// The rectangle the ring outlines now, if it shows.
    fn ring_rect(&self) -> Option<Rect> {
        if !self.ring_visible {
            return None;
        }
        let rect = self.visible_rect(self.node(self.focused?)?)?;
        (rect.width > 0.0 && rect.height > 0.0).then_some(rect)
    }

    /// Focuses the innermost widget under `point` inside the innermost
    /// trap. Whether the focus changed.
    fn press(&mut self, point: Point) -> bool {
        let root = self.trap_root();
        let hit = self
            .nodes
            .iter()
            .rev()
            .filter(|node| node.focusable)
            .filter(|node| root.is_none_or(|root| self.is_inside(node.id, root)))
            .find(|node| {
                self.visible_rect(node)
                    .is_some_and(|rect| rect.contains(point))
            })
            .map(|node| node.id);
        self.ring_visible = false;
        match hit {
            Some(id) if self.focused != Some(id) => {
                self.focused = Some(id);
                true
            }
            Some(_) | None => false,
        }
    }

    /// The widget the innermost open trap keeps the focus in.
    fn trap_root(&self) -> Option<FocusId> {
        self.traps.last().map(|trap| trap.id)
    }

    /// The node registered as `id` in the last frame.
    fn node(&self, id: FocusId) -> Option<&FocusNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Where `node` shows: its rectangle cut to each container around it,
    /// up to an open trap, which floats over the containers it opened
    /// from; `None` when it lies wholly outside one of them.
    fn visible_rect(&self, node: &FocusNode) -> Option<Rect> {
        let mut rect = node.rect;
        let mut current = node;
        while !self.traps.iter().any(|trap| trap.id == current.id) {
            let Some(parent) = current.parent.and_then(|id| self.node(id)) else {
                break;
            };
            rect = rect.intersection(&parent.rect)?;
            current = parent;
        }
        Some(rect)
    }

    /// Whether `id` is `ancestor` or inside it.
    fn is_inside(&self, id: FocusId, ancestor: FocusId) -> bool {
        let mut current = Some(id);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self.node(id).and_then(|node| node.parent);
        }
        false
    }

    /// The widgets registered directly in `parent` (or at the top when
    /// `None`), in tab order.
    fn children_of(&self, parent: Option<FocusId>) -> Vec<FocusId> {
        let mut children: Vec<(i32, FocusId)> = self
            .nodes
            .iter()
            .filter(|node| node.parent == parent)
            .map(|node| (self.orders.get(&node.id).copied().unwrap_or(0), node.id))
            .collect();
        // Stable: equal orders keep their registration order.
        children.sort_by_key(|(order, _)| *order);
        children.into_iter().map(|(_, id)| id).collect()
    }

    /// Adds the stops of `id` to `stops`: `id` itself when nothing was
    /// registered inside it, else the stops of what was, in tab order.
    fn collect_stops(&self, id: FocusId, stops: &mut Vec<FocusId>) {
        match self.node(id).is_some_and(|node| node.has_children) {
            true => {
                for child in self.children_of(Some(id)) {
                    self.collect_stops(child, stops);
                }
            }
            false => {
                if self.node(id).is_some_and(|node| node.focusable) {
                    stops.push(id);
                }
            }
        }
    }

    /// Moves a focus that is gone, outside the innermost trap, or on a
    /// container with widgets in it to the first stop it can: the
    /// container's own first stop, else the first stop of the trap or
    /// window. Nothing when nothing is registered.
    fn settle_focus(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        let root = self.trap_root();
        let placed = self.focused.filter(|focused| {
            self.node(*focused).is_some() && root.is_none_or(|root| self.is_inside(*focused, root))
        });
        let mut stops = Vec::new();
        match placed {
            Some(focused) => self.collect_stops(focused, &mut stops),
            None => stops = self.tab_stops(),
        }
        self.focused = stops.first().copied().or(placed);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `hash` with `bytes` mixed in, by FNV-1a.
const fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    let mut hash = hash;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        index += 1;
    }
    hash
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::input::event::ButtonInput;

    use super::super::offscreen;
    use super::*;
    use crate::base::key::parse_key;
    use crate::base::theme::Theme;

    const TABS: FocusId = FocusId::new("tabs");
    const LIST: FocusId = FocusId::new("list");
    const EDITOR: FocusId = FocusId::new("editor");
    const PANEL: FocusId = FocusId::new("panel");
    const DIALOG: FocusId = FocusId::new("dialog");
    const OK: FocusId = FocusId::new("ok");
    const CANCEL: FocusId = FocusId::new("cancel");

    /// A rectangle 10 pixels tall in row `row` of a window 100 wide.
    fn row(row: f32) -> Rect {
        Rect::new(0.0, row * 10.0, 100.0, 10.0)
    }

    /// Registers the tab strip, a panel holding the list and the editor,
    /// and, when `dialog` is set, a dialog holding two buttons over them.
    fn frame(tree: &mut FocusTree, dialog: bool) {
        tree.begin_frame();
        tree.register(TABS, row(0.0));
        tree.enter(PANEL, Rect::new(0.0, 10.0, 100.0, 40.0));
        tree.register(LIST, row(1.0));
        tree.register(EDITOR, row(2.0));
        tree.leave();
        if dialog {
            tree.enter(DIALOG, Rect::new(20.0, 20.0, 60.0, 20.0));
            tree.register(CANCEL, Rect::new(20.0, 30.0, 30.0, 10.0));
            tree.register(OK, Rect::new(50.0, 30.0, 30.0, 10.0));
            tree.leave();
        }
        tree.end_frame();
    }

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    /// A press, or a release when not `pressed`, of the left button at
    /// `x`, `y`.
    fn left(pressed: bool, x: f32, y: f32) -> Input {
        let press = ButtonInput::press(PointerButton::Left, Point::new(x, y));
        Input::PointerButton(ButtonInput { pressed, ..press })
    }

    /// Dispatches `input` through `tree`, recording each widget offered
    /// it with what it was offered, the widgets in `takers` taking it.
    fn offer(
        tree: &mut FocusTree,
        input: &Input,
        takers: &[FocusId],
    ) -> (Route, Vec<(FocusId, Input)>) {
        let mut offered = Vec::new();
        let route = tree.dispatch(input, |id, input| {
            offered.push((id, input.clone()));
            takers.contains(&id)
        });
        (route, offered)
    }

    #[test]
    fn test_ids_are_stable_by_name_and_children_differ_by_index() {
        assert_eq!(FocusId::new("tabs"), TABS);
        assert_ne!(TABS, LIST);
        assert_eq!(TABS.child(3), TABS.child(3));
        assert_ne!(TABS.child(3), TABS.child(4));
        assert_ne!(TABS.child(0), TABS);
    }

    #[test]
    fn test_tab_order_follows_registration_and_explicit_order_within_siblings() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        assert_eq!(
            tree.tab_stops(),
            [TABS, LIST, EDITOR],
            "a full panel is no stop"
        );
        assert_eq!(tree.focused(), Some(TABS), "the first stop takes the focus");
        tree.set_order(LIST, 1);
        tree.set_order(PANEL, -1);
        frame(&mut tree, false);
        assert_eq!(tree.tab_stops(), [EDITOR, LIST, TABS]);
    }

    #[test]
    fn test_tab_and_shift_tab_move_along_the_order_and_wrap() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        let declined = |_: FocusId, _: &Input| false;
        assert_eq!(tree.dispatch(&key("TAB"), declined), Route::Moved);
        assert_eq!(tree.focused(), Some(LIST));
        tree.dispatch(&key("TAB"), declined);
        tree.dispatch(&key("TAB"), declined);
        assert_eq!(tree.focused(), Some(TABS), "wrapped forward");
        tree.dispatch(&key("S-TAB"), declined);
        assert_eq!(tree.focused(), Some(EDITOR), "wrapped back");
    }

    #[test]
    fn test_keys_go_to_the_focused_widget_then_bubble_through_its_containers() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        tree.focus(EDITOR);
        let mut offered = Vec::new();
        let route = tree.dispatch(&key("x"), |id, _| {
            offered.push(id);
            id == PANEL
        });
        assert_eq!(route, Route::Delivered(PANEL));
        assert_eq!(offered, [EDITOR, PANEL]);
        let route = tree.dispatch(&key("TAB"), |id, _| id == EDITOR);
        assert_eq!(
            route,
            Route::Delivered(EDITOR),
            "a widget may keep Tab for itself"
        );
        assert_eq!(tree.dispatch(&key("y"), |_, _| false), Route::Unhandled);
    }

    #[test]
    fn test_a_release_goes_where_its_press_went_and_a_released_tab_moves_no_focus() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        tree.focus(EDITOR);
        let released = |spec: &str| {
            let Input::Key(key) = key(spec) else {
                unreachable!("a key")
            };
            Input::KeyReleased(key)
        };
        let route = tree.dispatch(&released("x"), |id, _| id == EDITOR);
        assert_eq!(route, Route::Delivered(EDITOR));
        assert_eq!(
            tree.dispatch(&released("TAB"), |_, _| false),
            Route::Unhandled
        );
        assert_eq!(tree.focused(), Some(EDITOR), "only a pressed Tab moves");
    }

    #[test]
    fn test_a_trap_keeps_tab_clicks_and_keys_inside_and_restores_focus_on_close() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        tree.focus(EDITOR);
        tree.open_trap(DIALOG);
        frame(&mut tree, true);
        assert_eq!(tree.focused(), Some(CANCEL), "the dialog's first stop");
        assert_eq!(tree.ring_rect(), None, "the dialog marks its own focus");
        assert_eq!(tree.tab_stops(), [CANCEL, OK]);
        tree.dispatch(&key("TAB"), |_, _| false);
        tree.dispatch(&key("TAB"), |_, _| false);
        assert_eq!(tree.focused(), Some(CANCEL), "Tab cycles inside the dialog");
        assert!(!tree.focus(LIST), "nothing outside the dialog takes focus");
        tree.press(Point::new(5.0, 25.0));
        assert_eq!(tree.focused(), Some(CANCEL), "nor a click outside it");
        let mut offered = Vec::new();
        let route = tree.dispatch(&key("C-t"), |id, _| {
            offered.push(id);
            false
        });
        assert_eq!(route, Route::Trapped);
        assert_eq!(offered, [CANCEL, DIALOG], "never past the dialog");
        tree.close_trap(DIALOG);
        assert_eq!(tree.focused(), Some(EDITOR), "back where it was");
        assert_eq!(tree.ring_rect(), Some(row(2.0)), "with its ring");
    }

    #[test]
    fn test_a_trap_no_longer_drawn_closes_and_restores_focus() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        tree.focus(LIST);
        tree.open_trap(DIALOG);
        frame(&mut tree, true);
        assert!(tree.contains_focus(DIALOG));
        frame(&mut tree, false);
        assert!(!tree.is_trapped());
        assert_eq!(tree.focused(), Some(LIST));
    }

    #[test]
    fn test_a_focused_widget_that_is_gone_passes_the_focus_to_the_first_stop() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        tree.focus(EDITOR);
        tree.begin_frame();
        tree.register(TABS, row(0.0));
        tree.end_frame();
        assert_eq!(tree.focused(), Some(TABS));
        tree.focus(PANEL);
        frame(&mut tree, false);
        assert_eq!(tree.focused(), Some(LIST), "a container passes it inside");
    }

    #[test]
    fn test_a_click_focuses_the_innermost_widget_under_it_and_hides_the_ring() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        tree.move_focus(1);
        assert!(tree.ring_rect().is_some(), "the keyboard shows the ring");
        assert!(tree.press(Point::new(50.0, 25.0)));
        assert_eq!(tree.focused(), Some(EDITOR), "not the panel around it");
        assert_eq!(tree.ring_rect(), None);
    }

    #[test]
    fn test_the_ring_outlines_the_focused_widget_in_the_accent_color() {
        let theme = Theme::dark();
        let mut tree = FocusTree::new();
        tree.begin_frame();
        tree.register(LIST, Rect::new(20.0, 20.0, 60.0, 30.0));
        tree.end_frame();
        tree.move_focus(1);
        let Some(snapshot) = offscreen::render(100, 80, |frame| {
            frame.clear(theme.background.into());
            tree.draw(frame, &theme);
        }) else {
            return;
        };
        snapshot.assert_pixel(19.0, 35.0, theme.accent, "the left edge");
        snapshot.assert_pixel(50.0, 51.0, theme.accent, "the bottom edge");
        snapshot.assert_pixel(20.5, 35.0, theme.background, "inside");
    }

    #[test]
    fn test_a_container_cuts_the_pointer_and_the_ring_to_its_rect_but_not_a_trap() {
        let mut tree = FocusTree::new();
        let scrolled = |tree: &mut FocusTree| {
            tree.begin_frame();
            tree.enter(PANEL, Rect::new(0.0, 0.0, 100.0, 20.0));
            tree.register(LIST, Rect::new(0.0, 10.0, 100.0, 20.0));
            tree.register(EDITOR, Rect::new(0.0, 40.0, 100.0, 10.0));
            tree.register(DIALOG, Rect::new(0.0, 15.0, 100.0, 30.0));
            tree.leave();
            tree.end_frame();
        };
        scrolled(&mut tree);
        assert_eq!(
            tree.under(Point::new(5.0, 45.0)),
            Vec::<FocusId>::new(),
            "scrolled away"
        );
        assert_eq!(
            tree.under(Point::new(5.0, 25.0)),
            Vec::<FocusId>::new(),
            "past the edge"
        );
        assert!(tree.focus(LIST));
        assert_eq!(tree.ring_rect(), Some(Rect::new(0.0, 10.0, 100.0, 10.0)));
        tree.focus(EDITOR);
        assert_eq!(tree.ring_rect(), None, "no ring for a row out of sight");
        tree.open_trap(DIALOG);
        scrolled(&mut tree);
        assert_eq!(
            tree.under(Point::new(5.0, 40.0)),
            vec![DIALOG],
            "a popup opened from a row floats past the container"
        );
    }

    const BAR: FocusId = FocusId::new("bar");

    #[test]
    fn test_a_press_goes_to_the_topmost_widget_under_it_and_falls_through_what_declines() {
        let mut tree = FocusTree::new();
        tree.begin_frame();
        tree.register(LIST, row(1.0));
        tree.register_hit(BAR, Rect::new(0.0, 0.0, 100.0, 100.0));
        tree.end_frame();
        let (route, offered) = offer(&mut tree, &left(true, 5.0, 15.0), &[BAR]);
        assert_eq!(route, Route::Delivered(BAR), "registered later, so on top");
        assert_eq!(offered.len(), 1);
        offer(&mut tree, &left(false, 5.0, 15.0), &[BAR]);
        let (route, offered) = offer(&mut tree, &left(true, 5.0, 15.0), &[LIST]);
        assert_eq!(route, Route::Delivered(LIST));
        let ids: Vec<FocusId> = offered.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            [BAR, LIST],
            "the bar declined, so the list under it got it"
        );
        assert_eq!(
            tree.focused(),
            Some(LIST),
            "a press focuses what takes the keyboard"
        );
        assert_eq!(tree.tab_stops(), [LIST], "a pointer-only widget is no stop");
    }

    #[test]
    fn test_inside_a_trap_the_pointer_never_reaches_what_is_behind_it() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        tree.open_trap(DIALOG);
        frame(&mut tree, true);
        let (route, offered) = offer(&mut tree, &left(true, 5.0, 15.0), &[DIALOG]);
        assert_eq!(
            route,
            Route::Delivered(DIALOG),
            "a click outside is the dialog's"
        );
        assert_eq!(offered.len(), 1, "the list under the click never sees it");
        offer(&mut tree, &left(false, 5.0, 15.0), &[]);
        let (_, offered) = offer(&mut tree, &left(true, 55.0, 35.0), &[]);
        let ids: Vec<FocusId> = offered.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [OK, DIALOG], "the button, then the dialog around it");
    }

    #[test]
    fn test_the_widget_that_took_a_press_keeps_the_pointer_until_the_release() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        offer(&mut tree, &left(true, 5.0, 15.0), &[LIST]);
        let (route, offered) = offer(&mut tree, &Input::PointerMoved(Point::new(5.0, 25.0)), &[]);
        assert_eq!(route, Route::Unhandled);
        assert_eq!(
            offered,
            [(LIST, Input::PointerMoved(Point::new(5.0, 25.0)))],
            "not the editor"
        );
        let (route, _) = offer(&mut tree, &left(false, 5.0, 25.0), &[]);
        assert_eq!(route, Route::Delivered(LIST), "the release too");
        let (_, offered) = offer(&mut tree, &Input::PointerMoved(Point::new(5.0, 26.0)), &[]);
        assert!(offered.iter().any(|(id, _)| *id == EDITOR), "free again");
    }

    #[test]
    fn test_a_move_reaches_every_widget_under_the_pointer_and_tells_those_it_left() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        let (_, offered) = offer(&mut tree, &Input::PointerMoved(Point::new(5.0, 15.0)), &[]);
        let ids: Vec<FocusId> = offered.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [LIST, PANEL], "the list and the panel around it");
        let (_, offered) = offer(&mut tree, &Input::PointerMoved(Point::new(5.0, 25.0)), &[]);
        assert_eq!(
            offered[0],
            (LIST, Input::PointerLeft),
            "the list hears it was left"
        );
        let (_, offered) = offer(&mut tree, &Input::PointerLeft, &[]);
        let ids: Vec<FocusId> = offered.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [EDITOR, PANEL], "leaving the window leaves them all");
    }

    #[test]
    fn test_losing_window_focus_reaches_every_widget_and_ends_a_held_press() {
        let mut tree = FocusTree::new();
        frame(&mut tree, false);
        offer(&mut tree, &left(true, 5.0, 15.0), &[LIST]);
        let (route, offered) = offer(&mut tree, &Input::FocusLost, &[]);
        assert_eq!(route, Route::Unhandled, "the app hears it too");
        assert_eq!(offered.len(), 4);
        let (_, offered) = offer(&mut tree, &Input::PointerMoved(Point::new(5.0, 25.0)), &[]);
        assert!(
            offered.iter().any(|(id, _)| *id == EDITOR),
            "the press no longer holds"
        );
    }
}

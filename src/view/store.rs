//! [`Store`]: what lasts of one window's tree between rebuilds, and the
//! steps that rebuild it: one per frame, one per input, one per screen
//! reader action.
//!
//! The tree an app's [`App::view`] returns borrows the app, so it lives for
//! one step. Each step flattens it into a list in reading order, giving
//! every element its id (its explicit id, or its parent's id with its
//! place among its siblings mixed in), then syncs the store: each control's
//! widget (made the first time its id is seen, dropped the first step its
//! id is not) takes the element's props and its focus, and each scrolling
//! container keeps a [`ScrollView`].
//!
//! - **A frame** lays the list out with `misty::layout` (a scrolling
//!   container's children a second time, inside its viewport), then draws
//!   it in order: each control drawn, registered in the [`FocusTree`]
//!   (tab order is reading order), and described to screen readers; each
//!   form label drawn beside its control and naming it. The focus ring is
//!   drawn last.
//! - **An input** goes through [`FocusTree::dispatch`] to the widget of
//!   the id it is for, against the rectangles of the last frame. A change
//!   a widget reports queues its element's callback; the tree is dropped,
//!   the queued callbacks run on the app, and a key no element took goes to
//!   [`App::key`].
//! - **A screen reader action** focuses the widget it names, or is offered
//!   to each control in turn until one takes it (only those of the
//!   innermost modal layer while one shows), its change run as an input's
//!   is.
//!
//! Layers float over the base tree: each [`Overlay`](super::element::Overlay)
//! an element was given is flattened after the base tree, laid out over
//! the whole window, and drawn in the frame's overlay after the base
//! layer, in the order given; a control's own popup (a select's open list,
//! a menu) is drawn by its widget and registered after both. A modal
//! layer, and a control's popup while it shows, is a focus trap: each
//! rebuild opens a trap for each one that appeared since the last and
//! closes those that went, which gives the focus back to the element that
//! had it. A tooltip shows over everything for the element the pointer
//! rests on.
//!
//! A split lays its two children out in the sides its kept
//! [`SplitBox`] divides, each as a root of its own, and takes the pointer
//! over its area after them for its divider. A right click asks the
//! innermost element under the pointer that has a context menu callback
//! (one Shift+F10 asks the one around the focused control), once the
//! element under the pointer has taken the press.

use std::any::Any;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use accesskit::{Node as AccessNode, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::{FocusId, FocusTree, Route};
use super::super::app::frame::Frame;
use super::super::app::ime::ImeFocus;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::split::Axis;
use super::super::base::styled::StyledText;
use super::super::base::text;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::platform::window_id::WindowId;
use super::super::render::font::CellMetrics;
use super::super::render::layout::{self, Align, Edges, Length, Style};
use super::super::widgets::drag::{Drag, FileDrop, FileDropped, draw_drag_label, draw_drop_target};
use super::super::widgets::hints::{Hints, HintsResponse};
use super::super::widgets::label::{self, draw_proportional_label, styled_paragraph};
use super::super::widgets::popup::draw_backdrop;
use super::super::widgets::scroll_view::ScrollView;
use super::super::widgets::tooltip::Tooltip;
use super::app::{App, AppCx, Asks};
use super::chrome::SplitBox;
use super::controls::{Callback, call};
use super::element::{Direction, Element, Kind, Lazy, MenuRequest, Overlay, Text};
use super::embed::ScrollStep;
use super::host::{Host, WindowHost};
use super::node::{Call, ControlDrag, MovingDrag, Node, Stored, layer_id, over_id};
use super::props::{Corners, ElementSize, Extras, Payload, Sizing};
use super::region::{RegionBox, RegionCx, describe_button, region_id};

// ========================================================================
// Constants
// ========================================================================

/// How many rows a lazy column builds before it was ever drawn, when how
/// many show is not known yet.
const LAZY_FIRST_ROWS: usize = 64;

/// How tall a lazy column takes a row to be, in cell rows, before it has
/// measured any.
const LAZY_FIRST_ROW_CELLS: f32 = 1.5;

/// The id the root element's id is made from.
const ROOT: FocusId = FocusId::new("misty.view");

/// The id the shown tooltip is described to screen readers as.
const TOOLTIP: FocusId = FocusId::new("misty.view.tooltip");

/// The id the hint labels are described to screen readers as.
const HINTS: FocusId = FocusId::new("misty.view.hints");

/// The child index, under an element's id, its first layer's id is made
/// with; the others count up from it, apart from the ids of its children
/// and of a control's popup.
const OVERLAY_SLOT: u64 = 1 << 33;

/// The child index, under a container's id, of the group its tab stops
/// are ordered in as one block: apart from its children, its region, and
/// the other nodes misty makes under an element's id.
const GROUP_NODE: u64 = 1 << 36;

/// The cell columns between a form's widest label and its controls.
const LABEL_GAP: f32 = 3.0;

/// The pixels a text may overrun its room and still count as whole: what
/// rounding takes, never a cut character.
const CUT_SLACK: f32 = 1.0;

/// The rows between two rows of a form.
const FORM_ROW_GAP: f32 = 0.5;

/// The cell columns between two elements of a row.
const ROW_GAP: f32 = 1.0;

/// The function key that, with Shift, asks for the context menu of the
/// focused control, as Windows, GNOME, and KDE bind it.
const CONTEXT_MENU_KEY: u8 = 10;

// ========================================================================
// Data Structures
// ========================================================================

/// What lasts of one window's tree between rebuilds: each control's
/// widget by id, the focus, and the input method's place.
#[derive(Default)]
pub(crate) struct Store {
    /// Where the focused text control's caret was drawn last, if one is
    /// focused.
    caret: Option<Rect>,
    /// Where each element with a context menu callback was drawn last,
    /// outermost first, by id.
    contexts: Vec<(Rect, FocusId)>,
    /// How many of `contexts` lie under the topmost modal layer, out of
    /// the pointer's reach.
    contexts_covered: usize,
    /// How many children each container had at the last rebuild, to tell
    /// a container whose children come and go.
    counts: HashMap<FocusId, usize>,
    /// The drag between elements under way, if any.
    drag: Drag<Payload>,
    /// What the label following the drag says.
    drag_label: String,
    /// The drag between elements a control carries out itself (a tab
    /// dragged off its strip), while it runs.
    control_drag: Option<MovingDrag>,
    /// Where each drop target was drawn last, outermost first, by id.
    drops: Vec<(Rect, FocusId)>,
    /// How many of `drops` lie under the topmost modal layer.
    drops_covered: usize,
    /// The widget of each control and scrolling container, by id.
    entries: HashMap<FocusId, Entry>,
    /// Files dragged in from another app, while held over the window.
    file_drop: FileDrop,
    /// Where each element taking dropped files was drawn last, outermost
    /// first, by id.
    file_targets: Vec<(Rect, FocusId)>,
    /// Which control has the keyboard.
    focus: FocusTree,
    /// The hint labels over the controls, while they show
    /// ([`AppCx::show_hints`]).
    hints: Hints,
    /// Whether the app asked for hint labels, which the next frame puts
    /// over the controls it draws.
    hints_asked: bool,
    /// What each hint label goes to, in the order given to `hints`: its
    /// screen reader node's id and whether it clicks (else it takes the
    /// keyboard).
    hint_targets: Vec<(FocusId, bool)>,
    /// How many screen reader nodes the frame described under the
    /// topmost modal layer, out of the hint labels' reach.
    access_covered: usize,
    /// Where each control that takes the keyboard was drawn last, in
    /// reading order, for a menu asked for from the keyboard to open
    /// under the focused one.
    hits: Vec<(FocusId, Rect)>,
    /// The size each element with an `on_size_change` was laid out at in
    /// the last frame, by id.
    laid_out: HashMap<FocusId, ElementSize>,
    /// Whether the input method is on, and where.
    ime: ImeFocus,
    /// The regions controls took the pointer over above everything else
    /// in the last frame, by the control's id.
    overs: Vec<(FocusId, Rect)>,
    /// The id of the element the app asked to give the keyboard, until a
    /// frame finds where it goes.
    pending_focus: Option<String>,
    /// The size each element with an `on_size_change` was last reported
    /// at, by id.
    reported: HashMap<FocusId, ElementSize>,
    /// The popups the controls drew this frame, under their ids, to
    /// register over everything else.
    popups: Vec<(FocusId, Rect)>,
    /// The element kept in view ([`Element::keep_in_view`]) the last frame
    /// drew, the scrolling container it is in, and where in its content.
    kept: Option<(FocusId, FocusId, Rect)>,
    /// The element marked to be kept in view last scrolled into view.
    kept_revealed: Option<FocusId>,
    /// Whether a lazy column built other rows than it now shows, which
    /// [`Store::about_to_wait`] draws again for.
    lazy_again: bool,
    /// What each lazy column ([`lazy_column`](super::lazy_column)) keeps
    /// between frames, by id: the rows to build and how tall they are.
    lazies: HashMap<FocusId, LazyState>,
    /// The rows each lazy column with an `on_shown` showed in the last
    /// frame, and those last reported, by id.
    rows_shown: HashMap<FocusId, Range<usize>>,
    rows_reported: HashMap<FocusId, Range<usize>>,
    /// The cell size the tree was last drawn with.
    metrics: Option<CellMetrics>,
    /// Where each element the app named ([`Element::id`]) inside a
    /// scrolling container was drawn last: the container's id, and the
    /// element's place in its content.
    named: HashMap<FocusId, (FocusId, Rect)>,
    /// The control last scrolled into view after the focus moved to it.
    revealed: Option<FocusId>,
    /// The scale factor the window was last drawn at.
    scale: Option<f64>,
    /// The id the tree's ids are made under, for a tree sharing its
    /// window with others; `None` for the window's own.
    scope: Option<FocusId>,
    /// Whether an element changes the app's state as time passes, which
    /// [`Store::about_to_wait`] then rebuilds the tree for.
    ticking: bool,
    /// Where each element with a tooltip was drawn last, and its help.
    tips: Vec<(Rect, String)>,
    /// How many of `tips` lie under the topmost modal layer, out of the
    /// pointer's reach.
    tips_covered: usize,
    /// How wide each proportional text and each control with a natural
    /// width (a button) of the frame being drawn is, in pixels, by its
    /// index in the flattened tree: measured with the frame before
    /// layout, which has no frame.
    widths: HashMap<usize, f32>,
    /// The tooltip of the element the pointer rests on.
    tooltip: Tooltip,
    /// The focus traps open, outermost first: each modal layer's root id
    /// and each open popup's id.
    traps: Vec<FocusId>,
}

/// A window's tree flattened: its elements in reading order, the base
/// tree's first, then each layer's.
struct Tree<'a, A> {
    /// The elements.
    flat: Vec<Flat<'a, A>>,
    /// The layers over the base tree, in the order they are drawn.
    layers: Vec<Layer>,
}

/// A layer over the base tree, as flattened.
struct Layer {
    /// The index past its last element.
    end: usize,
    /// Whether the keys its elements leave go on to the app while it is
    /// the innermost trap.
    keys_to_app: bool,
    /// Whether it keeps the keyboard and the pointer while it shows.
    modal: bool,
    /// The index of its root element.
    root: usize,
}

/// A layer met while flattening, flattened after the elements before it.
struct Pending<'a, A> {
    /// The index of the element it was given to.
    anchor: usize,
    /// The layer.
    layer: Overlay<'a, A>,
    /// Its place among the element's layers.
    slot: usize,
}

/// One kept widget.
struct Entry {
    /// Whether the rebuild under way saw its id.
    seen: bool,
    /// The widget.
    widget: Box<dyn Stored>,
}

/// One element of a flattened tree.
struct Flat<'a, A> {
    /// The indices of its children in the list.
    children: Vec<usize>,
    /// What asking for its context menu runs, if anything.
    context: Option<Callback<A, MenuRequest>>,
    /// The name the app gave its id, if it gave one.
    explicit: Option<Cow<'a, str>>,
    /// Its layout, surface, and pointer properties, if it has any.
    extras: Option<Box<Extras<'a, A>>>,
    /// Its id.
    id: FocusId,
    /// Its place among its parent's children.
    index: usize,
    /// What it is.
    kind: FlatKind<'a, A>,
    /// The form label naming it, if any.
    name: Option<&'a str>,
    /// Its tab order among its siblings, if the app gave one.
    order: Option<i32>,
    /// The blank space inside it, in cell columns and rows.
    padding: (f32, f32),
    /// The index of its parent in the list.
    parent: Option<usize>,
    /// The help its tooltip shows, if it has one.
    tooltip: Option<Cow<'a, str>>,
}

/// What a flattened element is.
enum FlatKind<'a, A> {
    /// A container of the elements its `children` name.
    Container(Shape<'a>),
    /// A control.
    Control(Box<dyn Node<A> + 'a>),
    /// Two elements, its `children`, sharing its area.
    Split(SplitShape<A>),
    /// A line of text.
    Text(Text<'a>),
}

/// How a flattened split shares its area.
struct SplitShape<A> {
    /// How its two children are placed.
    axis: Axis,
    /// What dragging its divider runs.
    on_resize: Option<Callback<A, f32>>,
    /// The share the first child takes.
    ratio: f32,
}

/// How a flattened container lays its children out.
struct Shape<'a> {
    /// Whether a form sets its labels in the theme's chrome font.
    chrome_labels: bool,
    /// The direction.
    direction: Direction,
    /// Each child's label, for a form.
    labels: Vec<&'a str>,
    /// The rows a lazy column holds of all it has, if it is one.
    lazy: Option<LazyShape>,
    /// Whether it scrolls its children.
    scroll: bool,
}

/// Which rows of all it has a flattened lazy column holds.
#[derive(Clone, Copy, Debug)]
struct LazyShape {
    /// How many rows it has.
    count: usize,
    /// The row the app's cursor is on.
    cursor: usize,
    /// The row its first child is.
    first: usize,
}

/// What a lazy column keeps between frames.
#[derive(Clone, Debug, Default)]
struct LazyState {
    /// How tall its rows were laid out on average, or zero before any was.
    average: f32,
    /// The cursor's row as last drawn, which is scrolled into view when it
    /// changes.
    cursor: Option<usize>,
    /// The rows to build next, those around what shows.
    rows: Range<usize>,
}

/// Where an element is laid out, which decides how it is sized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Place {
    /// In a column or a form row's control cell: as wide as the column.
    Column,
    /// As the content of its own scrolling viewport: as wide as the
    /// viewport, as tall as its children.
    Content,
    /// As the root of the window: the whole window.
    Root,
    /// In a row: sharing the row's width.
    Row,
}

/// Where a frame lays elements out and what it draws them with.
#[derive(Clone, Copy)]
struct Room {
    /// The frame's cell size.
    metrics: CellMetrics,
    /// The frame's scale factor.
    scale: f64,
}

/// Where each element of a flattened tree landed: at its index, and each
/// form row at its control's index past the elements.
struct Rects(Vec<Option<Rect>>);

/// The scrolling container a control is drawn in.
#[derive(Clone, Copy)]
struct ScrollContext {
    /// The container's id.
    id: FocusId,
    /// Where its content's top left is in the window.
    origin: Point,
    /// Where it shows its content.
    viewport: Rect,
}

// ========================================================================
// Store Implementations
// ========================================================================

impl Store {
    // --------------------------------------------------------------------
    // Steps
    // --------------------------------------------------------------------

    /// A store for a tree sharing its window with others, its ids made
    /// under `scope` so they never meet theirs.
    pub(crate) fn scoped(scope: FocusId) -> Self {
        Self {
            scope: Some(scope),
            ..Self::default()
        }
    }

    /// Draws `app`'s `window` into the whole of `frame`, as
    /// [`Store::render_view`] draws a tree.
    pub(crate) fn render<A: App>(&mut self, app: &A, window: WindowId, frame: &mut Frame) {
        let theme = app.theme();
        frame.clear(theme.background.into());
        let bounds = frame.bounds();
        self.render_view(app.view(window), theme, frame, bounds);
    }

    /// Draws `view` into `area` of `frame` in `theme`: rebuilds the tree,
    /// lays it out, draws and describes it with its layers over it, then
    /// the tooltip and the focus ring.
    pub(crate) fn render_view<A>(
        &mut self,
        view: Element<'_, A>,
        theme: &Theme,
        frame: &mut Frame,
        area: Rect,
    ) {
        let tree = flatten(view, self.scope, &self.lazies);
        self.sync(&tree);
        let flat = &tree.flat;
        let room = Room {
            metrics: frame.cell_metrics(),
            scale: frame.scale_factor(),
        };
        self.metrics = Some(room.metrics);
        self.named.clear();
        self.measure_widths(flat, frame, theme);
        let bounds = area;
        let mut rects = Rects(vec![None; 2 * flat.len()]);
        let roots = [0]
            .into_iter()
            .chain(tree.layers.iter().map(|layer| layer.root));
        for root in roots {
            let layout = self.layout_of(flat, root, Place::Root, room);
            rects.take(&layout.compute(bounds), flat.len(), None);
        }
        self.focus.begin_frame();
        self.caret = None;
        self.contexts.clear();
        self.contexts_covered = 0;
        self.drops.clear();
        self.drops_covered = 0;
        self.file_targets.clear();
        self.hits.clear();
        self.access_covered = 0;
        self.laid_out.clear();
        self.rows_shown.clear();
        self.overs.clear();
        self.popups.clear();
        self.tips.clear();
        self.tips_covered = 0;
        self.mark_control_drops(flat);
        self.kept = None;
        let mut reveal = None;
        self.paint(flat, 0, &mut rects, frame, theme, room, None, &mut reveal);
        for layer in &tree.layers {
            let root = &flat[layer.root];
            // A modal layer of elements is a trap of its own, over a
            // backdrop; a modal control (a dialog) draws its own backdrop
            // and is the trap itself. A layer that is a scrolling
            // container is entered as one, which is its trap.
            let backdrop = layer.modal && matches!(root.kind, FlatKind::Container(_));
            let scrolls = matches!(&root.kind, FlatKind::Container(shape) if shape.scroll);
            let entered = backdrop && !scrolls;
            if layer.modal {
                self.tips_covered = self.tips.len();
                self.access_covered = frame.access_len();
                self.contexts_covered = self.contexts.len();
                self.drops_covered = self.drops.len();
            }
            frame.overlay(|frame| {
                if backdrop {
                    draw_backdrop(frame, bounds, theme);
                }
                if entered {
                    self.focus.enter(root.id, bounds);
                }
                self.paint(
                    flat,
                    layer.root,
                    &mut rects,
                    frame,
                    theme,
                    room,
                    None,
                    &mut reveal,
                );
                if entered {
                    self.focus.leave();
                }
            });
        }
        for (id, rect) in &self.overs {
            self.focus.register_hit(over_id(*id), *rect);
        }
        for (id, rect) in std::mem::take(&mut self.popups) {
            self.focus.register(id, rect);
        }
        self.scale = Some(room.scale);
        self.draw_drops(flat, frame, theme);
        self.tooltip.draw(frame, bounds, theme);
        self.draw_hints(frame, bounds, theme);
        if let Some((help, rect)) = self.tooltip.shown() {
            let mut node = AccessNode::new(Role::Tooltip);
            node.set_label(help);
            frame.access(scoped(self.scope, TOOLTIP), rect, node);
        }
        self.focus.end_frame();
        // An element new in this frame takes a requested focus once it is
        // registered.
        if self.pending_focus.is_some() {
            self.settle_requested_focus(flat, true);
            self.sync_focus_states(flat);
        }
        // Scroll a control the keyboard just focused into view, once, so
        // the wheel can still scroll away from it.
        if self.focus.focused() != self.revealed {
            self.revealed = self.focus.focused();
            if let Some((id, rect)) = reveal
                && let Some(scroll) = self.scroll_view(id)
            {
                scroll.reveal(rect);
            }
        }
        // And an element marked to be kept in view, once another takes the
        // mark.
        let kept = self.kept.map(|(id, _, _)| id);
        if kept != self.kept_revealed {
            self.kept_revealed = kept;
            if let Some((_, scroll, rect)) = self.kept
                && let Some(scroll) = self.scroll_view(scroll)
            {
                scroll.reveal(rect);
            }
        }
        self.focus.draw(frame, theme);
    }

    /// Marks the drop target of `flat` the drag under way, or the files
    /// held over the window, would land on, and draws the drag's label at
    /// the pointer.
    fn draw_drops<A>(&self, flat: &[Flat<'_, A>], frame: &mut Frame, theme: &Theme) {
        if let (Some(point), Some(payload)) = (self.drag.position(), self.drag.payload()) {
            self.draw_target(flat, point, payload, frame, theme);
            self.drag.draw(frame, &self.drag_label, theme);
        }
        if let Some(moving) = &self.control_drag {
            self.draw_target(flat, moving.position, &moving.payload, frame, theme);
            draw_drag_label(frame, moving.position, &moving.label, theme);
        }
        if self.file_drop.is_hovering()
            && let Some(point) = self.file_drop.position()
            && let Some((rect, _)) = self
                .file_targets
                .iter()
                .rev()
                .find(|(rect, _)| rect.contains(point))
        {
            draw_drop_target(frame, *rect, true, theme);
        }
    }

    /// Tells each control taking drops itself where a drag running over it
    /// would land, and the others that none is over them, before the
    /// frame draws them.
    fn mark_control_drops<A>(&mut self, flat: &[Flat<'_, A>]) {
        let running = match (
            &self.control_drag,
            self.drag.position(),
            self.drag.payload(),
        ) {
            (Some(moving), _, _) => Some((moving.position, moving.payload.clone())),
            (None, Some(point), Some(payload)) => Some((point, payload.clone())),
            (None, _, _) => None,
        };
        let over = running.and_then(|(point, payload)| {
            let (_, index) = self.drop_target(flat, point, &payload)?;
            Some((flat[index].id, point))
        });
        for node in flat {
            let FlatKind::Control(control) = &node.kind else {
                continue;
            };
            if !control.takes_drops() {
                continue;
            }
            let Some(entry) = self.entries.get_mut(&node.id) else {
                continue;
            };
            let point = over
                .filter(|(id, _)| *id == node.id)
                .map(|(_, point)| point);
            control.mark_drop(&mut *entry.widget, point);
        }
    }

    /// Marks the drop target under `point` that takes `payload`: the part
    /// its marker picks ([`Element::drop_marker`]), else all of it.
    fn draw_target<A>(
        &self,
        flat: &[Flat<'_, A>],
        point: Point,
        payload: &Payload,
        frame: &mut Frame,
        theme: &Theme,
    ) {
        let Some((rect, index)) = self.drop_target(flat, point, payload) else {
            return;
        };
        // A control taking drops itself marks where they land.
        if matches!(flat[index].kind, FlatKind::Control(_)) {
            return;
        }
        let marker = flat[index]
            .extras
            .as_ref()
            .and_then(|extras| extras.drop.as_ref())
            .and_then(|target| target.marker.as_ref());
        let marked = marker.map_or(rect, |marker| {
            let local = Point::new(point.x - rect.x, point.y - rect.y);
            marker(rect, local)
        });
        draw_drop_target(frame, marked, true, theme);
    }

    /// Where the innermost drop target of `flat` under `point` that takes
    /// `payload` was drawn, outside any modal layer over it, and its
    /// index.
    fn drop_target<A>(
        &self,
        flat: &[Flat<'_, A>],
        point: Point,
        payload: &Payload,
    ) -> Option<(Rect, usize)> {
        self.drops[self.drops_covered.min(self.drops.len())..]
            .iter()
            .rev()
            .filter(|(rect, _)| rect.contains(point))
            .find_map(|(rect, id)| {
                let index = flat.iter().position(|node| node.id == *id)?;
                let node = &flat[index];
                let takes = match (
                    node.extras.as_ref().and_then(|e| e.drop.as_ref()),
                    &node.kind,
                ) {
                    (Some(target), _) => (target.accepts)(payload.as_ref()),
                    (None, FlatKind::Control(control)) => control.takes_drop(payload.as_ref()),
                    (None, _) => false,
                };
                takes.then_some((*rect, index))
            })
    }

    /// Hands `input`, which came to `app`'s `window`, to the tree as
    /// [`Store::host_input`] does. Returns what the app asked of the
    /// runtime.
    pub(crate) fn input<A: App>(
        &mut self,
        app: &mut A,
        window: WindowId,
        input: &Input,
        cx: EventCx<'_>,
    ) -> Asks {
        self.host_input(&mut WindowHost { app, window }, input, cx)
            .0
    }

    /// Hands `input` to the element of `host`'s tree it is for through
    /// `cx`, runs the callbacks it made on the host's state, and passes a
    /// key no element took to [`Host::key`]. Returns what the host asked
    /// of the runtime, and whether the tree or the host used `input`.
    pub(crate) fn host_input<H: Host>(
        &mut self,
        host: &mut H,
        input: &Input,
        mut cx: EventCx<'_>,
    ) -> (Asks, bool) {
        cx.request_redraw();
        if let Some(asks) = self.hints_input(host, input, &mut cx) {
            return (asks, true);
        }
        self.follow_tooltip(input);
        self.file_drop.handle_event(input);
        // Esc, and the window losing the keyboard, cancel a drag between
        // elements, whatever has the focus.
        let escape = input.key() == Some(Key::new(KeyCode::Esc, Mods::NONE));
        if self.drag.is_dragging() && (escape || *input == Input::FocusLost) {
            self.drag.cancel();
            if escape {
                return (Asks::default(), true);
            }
        }
        let (route, calls, close, took_key, trap_passes) = {
            let tree = flatten(host.view(), self.scope, &self.lazies);
            self.sync(&tree);
            let flat = &tree.flat;
            let trap_passes = self.traps.last().is_some_and(|trap| {
                tree.layers
                    .iter()
                    .any(|layer| layer.keys_to_app && flat[layer.root].id == *trap)
            });
            let mut places: HashMap<FocusId, usize> = flat
                .iter()
                .enumerate()
                .map(|(index, node)| (node.id, index))
                .collect();
            // A region a control takes the pointer over is the control's.
            for (id, _) in &self.overs {
                if let Some(&index) = places.get(id) {
                    places.insert(over_id(*id), index);
                }
            }
            // An element's region is the element's.
            for (index, node) in flat.iter().enumerate() {
                if has_region(node) {
                    places.insert(region_id(node.id), index);
                }
            }
            // An open popup's trap is its control's.
            if !self.traps.is_empty() {
                for (index, node) in flat.iter().enumerate() {
                    let layer = layer_id(node.id);
                    if matches!(node.kind, FlatKind::Control(_)) && self.traps.contains(&layer) {
                        places.insert(layer, index);
                    }
                }
            }
            let mut calls: Vec<Call<H::State>> = Vec::new();
            let mut close = false;
            let mut dropped = None;
            let entries = &mut self.entries;
            let drag = &mut self.drag;
            let control_drag = &mut self.control_drag;
            let drag_label = &mut self.drag_label;
            let scale = self.scale.unwrap_or(1.0);
            let route = self.focus.dispatch(input, |id, input| {
                let Some(node) = places.get(&id).map(|index| &flat[*index]) else {
                    return false;
                };
                if id == region_id(node.id) {
                    let region = entries
                        .get_mut(&id)
                        .and_then(|entry| downcast_region(&mut *entry.widget));
                    let (Some(region), Some(extras)) = (region, &node.extras) else {
                        return false;
                    };
                    let mut rcx = RegionCx {
                        cx: &mut cx,
                        drag: &mut *drag,
                        drag_label: &mut *drag_label,
                        scale,
                    };
                    let answer = region.handle(extras, input, &mut rcx);
                    calls.extend(answer.calls);
                    dropped = dropped.take().or(answer.dropped);
                    return answer.used;
                }
                let Some(entry) = entries.get_mut(&node.id) else {
                    return false;
                };
                match &node.kind {
                    FlatKind::Control(control) => {
                        let answer = control.handle(&mut *entry.widget, input, &mut cx);
                        calls.extend(answer.call);
                        close |= answer.close;
                        match answer.drag {
                            Some(ControlDrag::Moving(moving)) => *control_drag = Some(moving),
                            Some(ControlDrag::Dropped(dropped_here)) => {
                                *control_drag = None;
                                dropped = dropped.take().or(Some(dropped_here));
                            }
                            Some(ControlDrag::Ended) => *control_drag = None,
                            None => {}
                        }
                        answer.used
                    }
                    FlatKind::Container(_) => downcast_scroll(&mut *entry.widget)
                        .is_some_and(|scroll| !scroll.handle(input, &mut cx).is_ignored()),
                    FlatKind::Split(shape) => {
                        let Some(split) = downcast_split(&mut *entry.widget) else {
                            return false;
                        };
                        let (used, ratio) = split.follow(input, &mut cx);
                        if let Some(ratio) = ratio {
                            calls.extend(call(shape.on_resize.as_ref(), ratio));
                        }
                        used
                    }
                    FlatKind::Text(_) => false,
                }
            });
            if let Some(dropped) = dropped
                && let Some((rect, index)) =
                    self.drop_target(flat, dropped.position, &dropped.payload)
            {
                let node = &flat[index];
                if let Some(target) = node.extras.as_ref().and_then(|e| e.drop.as_ref()) {
                    let on_drop = Rc::clone(&target.on_drop);
                    let point =
                        Point::new(dropped.position.x - rect.x, dropped.position.y - rect.y);
                    let payload = dropped.payload;
                    calls.push(Box::new(move |state: &mut H::State| {
                        on_drop(state, payload.as_ref(), point);
                    }));
                } else if let FlatKind::Control(control) = &node.kind
                    && let Some(entry) = self.entries.get(&node.id)
                {
                    let payload = dropped.payload.as_ref();
                    calls.extend(control.drop_call(&*entry.widget, payload, dropped.position));
                }
            }
            let request = self.context_request(&tree, &places, input, route);
            let took_key = request.is_some_and(|(_, request)| request.by_key);
            if let Some((index, request)) = request {
                calls.extend(call(tree.flat[index].context.as_ref(), request));
            }
            (route, calls, close, took_key, trap_passes)
        };
        let ran = !calls.is_empty();
        for call in calls {
            call(host.state());
        }
        // A key moving the focus is used only where there is a focus to
        // move.
        let mut used = ran
            || took_key
            || match route {
                Route::Delivered(_) | Route::Trapped => true,
                Route::Moved => !self.focus.tab_stops().is_empty(),
                Route::Unhandled => false,
            };
        let mut app_cx = AppCx::new(cx);
        let asked_close = close || matches!(input, Input::CloseRequested);
        if asked_close && host.close_requested(&mut app_cx) {
            app_cx.close_window();
        }
        match input {
            Input::AppearanceChanged(appearance) => host.follow_appearance(*appearance),
            Input::FocusGained => host.focus_changed(true),
            Input::FocusLost => host.focus_changed(false),
            Input::FileDropped(file) if route == Route::Unhandled => {
                let file = FileDropped {
                    path: file.path.clone(),
                    position: file.position,
                };
                host.file_dropped(file, &mut app_cx);
            }
            // Tab moves the focus, but in a window with nothing to focus
            // it is the app's like any other key.
            // A modal layer that hands its keys on gives the app those its
            // elements left.
            Input::Key(key)
                if !took_key
                    && (route == Route::Unhandled
                        || (route == Route::Trapped && trap_passes)
                        || (route == Route::Moved && self.focus.tab_stops().is_empty())) =>
            {
                used |= host.key(*key, &mut app_cx);
            }
            Input::KeyReleased(key) if matches!(route, Route::Unhandled | Route::Trapped) => {
                host.key_released(*key, &mut app_cx);
            }
            _ => {}
        }
        if host.should_exit() {
            app_cx.exit();
        }
        (app_cx.asks(), used)
    }

    /// Does what a screen reader's `action` on `app`'s `window` asks:
    /// focuses the widget it names, or hands it to the control it is for
    /// (inside the innermost trap while one is open), running the callback
    /// the change makes. Returns what the app asked of the runtime.
    pub(crate) fn access_action<A: App>(
        &mut self,
        app: &mut A,
        window: WindowId,
        action: AccessAction,
    ) -> Asks {
        self.act(&mut WindowHost { app, window }, action).0
    }

    /// Does what `action` asks, as [`Store::access_action`] describes;
    /// returns what the app asked and whether a widget took the action.
    pub(super) fn act<H: Host>(&mut self, host: &mut H, action: AccessAction) -> (Asks, bool) {
        if let AccessAction::Focus(id) = action {
            self.focus.focus(id);
            return (Asks::default(), true);
        }
        let call = {
            let tree = flatten(host.view(), self.scope, &self.lazies);
            self.sync(&tree);
            let range = self.trapped_range(&tree);
            let entries = &mut self.entries;
            // A press on a region a screen reader clicked.
            let pressed = tree.flat[range.clone()].iter().find_map(|node| {
                let AccessAction::Click(target) = action else {
                    return None;
                };
                let extras = node
                    .extras
                    .as_ref()
                    .filter(|_| target == region_id(node.id))?;
                let region = downcast_region(&mut *entries.get_mut(&target)?.widget)?;
                call(extras.on_press.as_ref(), region.key_press())
            });
            let pressed = pressed.map(|pressed| (Some(pressed), false));
            pressed.or_else(|| {
                tree.flat[range].iter().find_map(|node| {
                    let FlatKind::Control(control) = &node.kind else {
                        return None;
                    };
                    let entry = entries.get_mut(&node.id)?;
                    let answer = control.act(&mut *entry.widget, node.id, action);
                    answer.used.then_some((answer.call, answer.close))
                })
            })
        };
        let close = call.as_ref().is_some_and(|(_, close)| *close);
        let used = call.is_some();
        if let Some((Some(call), _)) = call {
            call(host.state());
        }
        let asks = Asks {
            close,
            exit: host.should_exit(),
            focus: None,
            hints: false,
        };
        (asks, used)
    }

    /// Where the focused text element's caret was drawn in the last
    /// frame, which the input method follows, or `None` when no focused
    /// element takes text.
    pub(crate) fn ime_caret(&self) -> Option<Rect> {
        self.caret
    }

    /// Steps every kept widget's animation and the tooltip's delay through
    /// `cx`, keeps the input method at the focused text control's caret,
    /// and runs on `app` what its `window`'s elements say time changed (the
    /// toasts expiring); returns when the next step is due, or `None` at
    /// rest.
    pub(crate) fn about_to_wait<A: App>(
        &mut self,
        app: &mut A,
        window: WindowId,
        cx: &mut EventCx<'_>,
    ) -> Option<Instant> {
        self.host_about_to_wait(&mut WindowHost { app, window }, cx)
    }

    /// Steps the tree of `host` as [`Store::about_to_wait`] describes,
    /// running on its state what time changed.
    pub(crate) fn host_about_to_wait<H: Host>(
        &mut self,
        host: &mut H,
        cx: &mut EventCx<'_>,
    ) -> Option<Instant> {
        self.ime.follow(cx, self.caret);
        let due = match self.ticking {
            true => tick(host, self.scope, &self.lazies, cx),
            false => None,
        };
        self.report_sizes(host, cx);
        self.report_rows(host, cx);
        // A lazy column whose rows came short of what shows, or past it,
        // builds the right ones in another frame.
        if std::mem::take(&mut self.lazy_again) {
            cx.request_redraw();
        }
        let tooltip = Widget::about_to_wait(&mut self.tooltip, cx);
        self.entries
            .values_mut()
            .filter_map(|entry| entry.widget.about_to_wait(cx))
            .chain(due)
            .chain(tooltip)
            .min()
    }

    /// Runs on `app` the `on_size_change` of each element of `window` the
    /// last frame laid out at another size than last reported, asking
    /// `cx` for a frame drawn from what they changed; nothing, and no
    /// rebuild, when every size is as reported.
    fn report_sizes<H: Host>(&mut self, host: &mut H, cx: &mut EventCx<'_>) {
        if self.laid_out == self.reported {
            return;
        }
        let calls: Vec<Call<H::State>> = {
            let tree = flatten(host.view(), self.scope, &self.lazies);
            tree.flat
                .iter()
                .filter_map(|node| {
                    let apply = node.extras.as_ref()?.on_size_change.as_ref();
                    let size = *self.laid_out.get(&node.id)?;
                    match self.reported.get(&node.id) == Some(&size) {
                        true => None,
                        false => call(apply, size),
                    }
                })
                .collect()
        };
        self.reported.clone_from(&self.laid_out);
        if !calls.is_empty() {
            cx.request_redraw();
        }
        for call in calls {
            call(host.state());
        }
    }

    /// Runs on `app` the `on_shown` of each lazy column the last frame
    /// showed other rows of than last reported, asking `cx` for a frame
    /// drawn from what they changed, as [`Store::report_sizes`] does.
    fn report_rows<H: Host>(&mut self, host: &mut H, cx: &mut EventCx<'_>) {
        if self.rows_shown == self.rows_reported {
            return;
        }
        let calls: Vec<Call<H::State>> = {
            let tree = flatten(host.view(), self.scope, &self.lazies);
            tree.flat
                .iter()
                .filter_map(|node| {
                    let apply = node.extras.as_ref()?.on_rows_shown.as_ref();
                    let shown = self.rows_shown.get(&node.id)?.clone();
                    match self.rows_reported.get(&node.id) == Some(&shown) {
                        true => None,
                        false => call(apply, shown),
                    }
                })
                .collect()
        };
        self.rows_reported.clone_from(&self.rows_shown);
        if !calls.is_empty() {
            cx.request_redraw();
        }
        for call in calls {
            call(host.state());
        }
    }

    /// Shows the tooltip of the element under the pointer after `input`
    /// moved it, hides it when the pointer left, and puts it away for a
    /// press, a scroll, or a key until the pointer rests on something else.
    fn follow_tooltip(&mut self, input: &Input) {
        match input {
            Input::PointerMoved(point) => {
                let tip = self.tips[self.tips_covered..]
                    .iter()
                    .rev()
                    .find(|(rect, _)| rect.contains(*point));
                match tip {
                    Some((rect, help)) => self.tooltip.hover(*rect, help.as_str()),
                    None => {
                        self.tooltip.leave();
                    }
                }
            }
            Input::PointerLeft => {
                self.tooltip.leave();
            }
            // Put away, and kept away while the pointer rests where it was.
            Input::Key(_) | Input::PointerButton(_) | Input::Wheel(_) => {
                self.tooltip.dismiss();
            }
            Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::KeyReleased(_) => {}
        }
    }

    /// The element of `tree` whose context menu `input` asks for, and
    /// where it opens: a right press asks the innermost element under the
    /// pointer that has a callback, outside any modal layer over it, and
    /// Shift+F10 that no control took asks the innermost one around the
    /// focused control, within its layer, opening under the control.
    /// `places` finds an element of `tree` by id.
    fn context_request<A>(
        &self,
        tree: &Tree<'_, A>,
        places: &HashMap<FocusId, usize>,
        input: &Input,
        route: Route,
    ) -> Option<(usize, MenuRequest)> {
        if let Some(press) = input.press(PointerButton::Right) {
            let point = press.position;
            let (_, id) = self.contexts[self.contexts_covered.min(self.contexts.len())..]
                .iter()
                .rev()
                .find(|(rect, _)| rect.contains(point))?;
            return Some((*places.get(id)?, MenuRequest::at(point)));
        }
        let asks = input.key() == Some(Key::new(KeyCode::F(CONTEXT_MENU_KEY), Mods::SHIFT))
            && matches!(route, Route::Unhandled | Route::Trapped);
        if !asks {
            return None;
        }
        let focused = self.focus.focused()?;
        let mut index = *places.get(&focused)?;
        loop {
            if tree.flat[index].context.is_some() {
                let (_, anchor) = self.hits.iter().find(|(id, _)| *id == focused)?;
                let anchor = *anchor;
                return Some((
                    index,
                    MenuRequest {
                        anchor,
                        by_key: true,
                    },
                ));
            }
            let is_layer_root = tree.layers.iter().any(|layer| layer.root == index);
            index = tree.flat[index].parent.filter(|_| !is_layer_root)?;
        }
    }

    /// The elements of `tree` a screen reader's action may reach: those
    /// of the innermost modal layer, or the control whose popup is the
    /// innermost trap, while a trap is open; all of them otherwise.
    fn trapped_range<A>(&self, tree: &Tree<'_, A>) -> Range<usize> {
        let Some(&trap) = self.traps.last() else {
            return 0..tree.flat.len();
        };
        if let Some(layer) = tree
            .layers
            .iter()
            .find(|layer| tree.flat[layer.root].id == trap)
        {
            return layer.root..layer.end;
        }
        tree.flat
            .iter()
            .position(|node| layer_id(node.id) == trap)
            .map_or(0..0, |index| index..index + 1)
    }

    // --------------------------------------------------------------------
    // Syncing
    // --------------------------------------------------------------------

    /// Brings the kept widgets in step with `tree`: makes a widget for each
    /// new id (or for an id now holding another kind of control), hands
    /// each its props, drops the widgets of ids not seen, opens and closes
    /// the traps of the layers and popups that came and went, and hands
    /// each widget its focus.
    fn sync<A>(&mut self, tree: &Tree<'_, A>) {
        let flat = &tree.flat;
        if cfg!(debug_assertions) {
            check_unique_ids(flat);
            self.check_stable_ids(flat);
        }
        for entry in self.entries.values_mut() {
            entry.seen = false;
        }
        let mut traps = Vec::new();
        let mut ticking = false;
        for (index, node) in flat.iter().enumerate() {
            if tree
                .layers
                .iter()
                .any(|layer| layer.modal && layer.root == index)
            {
                traps.push(node.id);
            }
            match &node.kind {
                FlatKind::Control(control) => {
                    let entry = self.entries.entry(node.id).or_insert_with(|| Entry {
                        seen: false,
                        widget: control.create(),
                    });
                    if !control.owns(&*entry.widget) {
                        entry.widget = control.create();
                    }
                    entry.seen = true;
                    control.sync(&mut *entry.widget, node.name);
                    if control.has_layer(&*entry.widget) {
                        traps.push(layer_id(node.id));
                    }
                    ticking |= control.is_ticking();
                }
                FlatKind::Container(shape) if shape.scroll => {
                    let entry = self.entries.entry(node.id).or_insert_with(|| Entry {
                        seen: false,
                        widget: Box::new(ScrollView::new()),
                    });
                    if downcast_scroll(&mut *entry.widget).is_none() {
                        entry.widget = Box::new(ScrollView::new());
                    }
                    entry.seen = true;
                }
                FlatKind::Split(shape) => {
                    let entry = self.entries.entry(node.id).or_insert_with(|| Entry {
                        seen: false,
                        widget: Box::new(SplitBox::new()),
                    });
                    if downcast_split(&mut *entry.widget).is_none() {
                        entry.widget = Box::new(SplitBox::new());
                    }
                    entry.seen = true;
                    if let Some(split) = downcast_split(&mut *entry.widget) {
                        split.sync(shape.axis, shape.ratio);
                    }
                }
                FlatKind::Container(_) | FlatKind::Text(_) => {}
            }
            if has_region(node) {
                let entry = self
                    .entries
                    .entry(region_id(node.id))
                    .or_insert_with(|| Entry {
                        seen: false,
                        widget: Box::new(RegionBox::default()),
                    });
                entry.seen = true;
            }
        }
        self.entries.retain(|_, entry| entry.seen);
        self.ticking = ticking;
        self.follow_traps(traps);
        if self.pending_focus.is_some() {
            self.settle_requested_focus(flat, false);
        }
        self.sync_focus_states(flat);
    }

    /// The scrolling container the app named `name`, as the last frame
    /// drew it.
    fn named_scroll(&mut self, name: &str) -> Option<&mut ScrollView> {
        let id = scoped(self.scope, FocusId::new(name));
        self.scroll_view(id)
    }

    /// Scrolls the container the app named `name` by `step`; says whether
    /// that moved it.
    pub(crate) fn scroll_named(&mut self, name: &str, step: ScrollStep) -> bool {
        let row = self.metrics.map_or(0.0, |metrics| metrics.height);
        let Some(scroll) = self.named_scroll(name) else {
            return false;
        };
        let page = scroll.viewport().map_or(0.0, |viewport| viewport.height);
        let at = scroll.offset().y;
        let target = match step {
            ScrollStep::End => f32::MAX,
            ScrollStep::Pages(pages) => at + pages * page,
            ScrollStep::Pixels(pixels) => at + pixels,
            ScrollStep::Rows(rows) => at + rows * row,
            ScrollStep::Start => 0.0,
        };
        scroll.glide_to_y(target.max(0.0))
    }

    /// How far down the container the app named `name` is scrolled, as
    /// last drawn.
    pub(crate) fn offset_named(&mut self, name: &str) -> Option<f32> {
        Some(self.named_scroll(name)?.offset().y)
    }

    /// Where the element the app named `name` starts down the content of
    /// the scrolling container it is in, as last drawn.
    pub(crate) fn top_named(&self, name: &str) -> Option<f32> {
        let id = scoped(self.scope, FocusId::new(name));
        self.named.get(&id).map(|(_, rect)| rect.y)
    }

    /// Scrolls the container around the element the app named `name` so
    /// the element is at its top, as far as the content goes; says whether
    /// that moved it.
    pub(crate) fn reveal_top_named(&mut self, name: &str) -> bool {
        let id = scoped(self.scope, FocusId::new(name));
        let Some((scroll, rect)) = self.named.get(&id).copied() else {
            return false;
        };
        self.scroll_view(scroll)
            .is_some_and(|scroll| scroll.glide_to_y(rect.y))
    }

    /// Gives the keyboard where the app asked ([`Store::request_focus`]):
    /// to the element of `flat` with that id when it takes the keyboard,
    /// else to the first tab stop inside it, unless the keyboard is
    /// inside it already. Keeps the request for after the frame's drawing
    /// when the element's stops are not registered yet, unless `last`;
    /// drops it, warning, when no element has the id or nothing inside it
    /// takes the keyboard.
    fn settle_requested_focus<A>(&mut self, flat: &[Flat<'_, A>], last: bool) {
        let Some(name) = self.pending_focus.clone() else {
            return;
        };
        let target = scoped(self.scope, FocusId::new(&name));
        let Some(index) = flat.iter().position(|node| node.id == target) else {
            self.pending_focus = None;
            log::warn!(
                "AppCx::focus({name:?}): no element of the window has this id; give the element \
                 `.id({name:?})`"
            );
            return;
        };
        let inside = subtree_ids(flat, index);
        if self
            .focus
            .focused()
            .is_some_and(|id| self.focus.is_inside_any(id, &inside))
        {
            self.pending_focus = None;
            return;
        }
        let stop = self
            .focus
            .tab_stops()
            .into_iter()
            .find(|id| self.focus.is_inside_any(*id, &inside));
        match (stop, last) {
            (Some(stop), _) => {
                self.pending_focus = None;
                self.focus.focus(stop);
            }
            (None, false) => {}
            (None, true) => {
                self.pending_focus = None;
                log::warn!(
                    "AppCx::focus({name:?}): nothing in the element takes the keyboard (or an open \
                     trap keeps it out); give it a control or close the trap first"
                );
            }
        }
    }

    /// Moves the keyboard `delta` tab stops on, as Tab and Shift+Tab do.
    pub(crate) fn move_focus(&mut self, delta: isize) {
        self.focus.move_focus(delta);
    }

    /// Asks to give the keyboard to the element with the id `name` when
    /// the window is next drawn, as [`AppCx::focus`] describes.
    pub(crate) fn request_focus(&mut self, name: String) {
        self.pending_focus = Some(name);
    }

    /// Asks for hint labels over the controls the window next draws, as
    /// [`AppCx::show_hints`] describes.
    pub(crate) fn request_hints(&mut self) {
        self.hints_asked = true;
    }

    /// Puts the hint labels asked for over what this frame described to
    /// screen readers as clickable or taking text, within `bounds` and
    /// left in reach by the topmost modal layer, and draws the labels
    /// showing, with their screen reader node.
    fn draw_hints(&mut self, frame: &mut Frame, bounds: Rect, theme: &Theme) {
        if std::mem::take(&mut self.hints_asked) {
            let (targets, rects): (Vec<(FocusId, bool)>, Vec<Rect>) = frame
                .hint_targets(self.access_covered)
                .into_iter()
                .filter(|(_, rect, _)| rect.width > 0.0 && rect.height > 0.0)
                .filter(|(_, rect, _)| rect.intersection(&bounds).is_some())
                .map(|(id, rect, clicks)| ((id, clicks), rect))
                .unzip();
            self.hint_targets = targets;
            self.hints.show(rects);
        }
        self.hints.draw(frame, bounds, theme);
        self.hints.access(frame, scoped(self.scope, HINTS), theme);
    }

    /// Hands `input` to the hint labels while they show: a label typed
    /// presses its target (or gives it the keyboard, if it takes no
    /// click). Returns what the app asked, or `None` when the labels do
    /// not show, leaving `input` to the tree.
    fn hints_input<H: Host>(
        &mut self,
        host: &mut H,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> Option<Asks> {
        match self.hints.handle(input, cx) {
            HintsResponse::Ignored => None,
            HintsResponse::Activated(index) => {
                let (target, clicks) = *self.hint_targets.get(index)?;
                self.hint_targets.clear();
                let (asks, used) = match clicks {
                    true => self.act(host, AccessAction::Click(target)),
                    false => (Asks::default(), false),
                };
                if !used {
                    self.focus.focus(target);
                }
                Some(asks)
            }
            HintsResponse::Cancelled | HintsResponse::Handled => {
                if !self.hints.is_active() {
                    self.hint_targets.clear();
                }
                Some(Asks::default())
            }
        }
    }

    /// Tells every kept widget of `flat` whether it has the keyboard.
    fn sync_focus_states<A>(&mut self, flat: &[Flat<'_, A>]) {
        for node in flat {
            let region = region_id(node.id);
            if let Some(entry) = self.entries.get_mut(&region) {
                entry.widget.set_focused(self.focus.is_focused(region));
            }
            let Some(entry) = self.entries.get_mut(&node.id) else {
                continue;
            };
            let focused = match node.kind {
                // A control with a popup has the keyboard while its popup
                // does.
                FlatKind::Control(_) => {
                    self.focus.is_focused(node.id) || self.focus.is_focused(layer_id(node.id))
                }
                // Keys its controls leave bubble up and scroll it.
                FlatKind::Container(_) => self.focus.contains_focus(node.id),
                FlatKind::Split(_) | FlatKind::Text(_) => continue,
            };
            entry.widget.set_focused(focused);
        }
    }

    /// Closes the open traps not in `seen`, innermost first, which gives
    /// the focus back to the element that had it before each, and opens
    /// those new in `seen`, outermost first, which moves the focus in.
    fn follow_traps(&mut self, seen: Vec<FocusId>) {
        for id in self.traps.iter().rev() {
            if !seen.contains(id) {
                self.focus.close_trap(*id);
            }
        }
        for id in &seen {
            if !self.traps.contains(id) {
                self.focus.open_trap(*id);
            }
        }
        self.traps = seen;
    }

    /// Panics when a container of `flat` has another number of children
    /// than at the last rebuild while a control inside it has no id of its
    /// own: its id is its place, which may now be another element's.
    fn check_stable_ids<A>(&mut self, flat: &[Flat<'_, A>]) {
        let mut counts = HashMap::new();
        for (index, node) in flat.iter().enumerate() {
            if !matches!(node.kind, FlatKind::Container(_)) {
                continue;
            }
            let count = node.children.len();
            counts.insert(node.id, count);
            let Some(&before) = self.counts.get(&node.id) else {
                continue;
            };
            if before == count {
                continue;
            }
            if let Some(control) = implicit_control(flat, index) {
                panic!(
                    "misty: the {} at {} went from {before} to {count} elements, and the {} \
                     at {} inside it has no id of its own, so it may take the state of the \
                     element that was in its place.\nGive it an id with `.id(..)` naming \
                     what it shows.",
                    kind_name(&node.kind),
                    path(flat, index),
                    kind_name(&flat[control].kind),
                    path(flat, control),
                );
            }
        }
        self.counts = counts;
    }

    // --------------------------------------------------------------------
    // Layout
    // --------------------------------------------------------------------

    /// Measures in `frame` with `theme` how wide each proportional text
    /// and each control with a natural width of `flat` is, for its layout.
    fn measure_widths<A>(&mut self, flat: &[Flat<'_, A>], frame: &Frame, theme: &Theme) {
        self.widths.clear();
        for (index, node) in flat.iter().enumerate() {
            let width = match &node.kind {
                FlatKind::Text(text) if text.proportional => {
                    Some(frame.paragraph_width(&styled_paragraph(&text.styled(theme.emphasis))))
                }
                FlatKind::Control(control) => control.natural_width(frame, theme),
                FlatKind::Container(shape) if shape.chrome_labels => shape
                    .labels
                    .iter()
                    .map(|label| label::label_width(frame, theme, label))
                    .reduce(f32::max),
                FlatKind::Container(_) | FlatKind::Split(_) | FlatKind::Text(_) => None,
            };
            if let Some(width) = width {
                self.widths.insert(index, width);
            }
        }
    }

    /// How wide the label column of the form `shape`, element `index`, is
    /// with `metrics`: its widest label and the gap after it, the labels
    /// measured as chrome in [`Store::measure_widths`] when it sets them
    /// so, else on the cell grid.
    fn label_width(&self, index: usize, shape: &Shape<'_>, metrics: CellMetrics) -> f32 {
        match shape.chrome_labels {
            true => {
                let widest = self.widths.get(&index).copied().unwrap_or(0.0);
                (widest + LABEL_GAP * metrics.width).ceil()
            }
            false => cell_label_width(shape, metrics),
        }
    }

    /// The layout of element `index` of `flat` and the elements inside it,
    /// laid out at `place`, in `room`. A scrolling container is a box
    /// taking the room left, unless it is laid out as the content of its
    /// own viewport.
    fn layout_of<'t, A>(
        &'t self,
        flat: &'t [Flat<'_, A>],
        index: usize,
        place: Place,
        room: Room,
    ) -> layout::Element<'t, usize> {
        let node = &flat[index];
        let metrics = room.metrics;
        let sizing = node.extras.as_ref().map(|extras| &extras.sizing);
        let padded = node.padding != (0.0, 0.0) || sizing.is_some_and(|s| s.padding_px.is_some());
        // A control with a natural width (a button) packs to it across a
        // row rather than taking a share, unless the app sized or grew it;
        // a padded one's box hugs it.
        let free = sizing.is_none_or(|s| s.width.is_none() && s.grow.is_none());
        let natural = match (&node.kind, place) {
            (FlatKind::Control(_), Place::Row) if free => self.widths.get(&index).copied(),
            (_, Place::Column | Place::Content | Place::Root | Place::Row) => None,
        };
        let base = match natural {
            Some(_) => Style::new(),
            None => place.style(),
        };
        // What the app set wins over what the place gives; a padded
        // element is sized by the box around it.
        let style = match padded {
            true => base,
            false => sized(base, sizing, place, room),
        };
        let element = match &node.kind {
            // A list or a table takes the room left, as a scrolling
            // container does.
            FlatKind::Control(control) if control.grows() => {
                let style = match place {
                    Place::Column if padded => base.with_height(Length::Flex(1.0)),
                    Place::Column => {
                        sized(base.with_height(Length::Flex(1.0)), sizing, place, room)
                    }
                    Place::Content | Place::Root | Place::Row => style,
                };
                layout::Element::leaf(style)
            }
            FlatKind::Control(control) => match self.entries.get(&node.id) {
                Some(entry) => {
                    let widget = &*entry.widget;
                    let style = match natural {
                        Some(width) => style.with_width(Length::Fixed(width)),
                        None => style,
                    };
                    layout::Element::measured(style, move |width| {
                        control.measure(widget, width, metrics, room.scale)
                    })
                }
                None => layout::Element::leaf(style),
            },
            FlatKind::Text(text) => {
                let width = match self.widths.get(&index) {
                    // Whole pixels, so the text is never cut by a rounding.
                    Some(width) => width.ceil(),
                    None => {
                        let cells: usize = text.spans.iter().map(|(run, _)| text::width(run)).sum();
                        cells as f32 * metrics.width
                    }
                };
                let style = Style::new().with_height(Length::Fixed(metrics.height));
                let style = match place {
                    Place::Row => style.with_width(Length::Fixed(width)),
                    Place::Column | Place::Content | Place::Root => style,
                };
                let style = match padded {
                    true => style,
                    false => sized(style, sizing, place, room),
                };
                layout::Element::leaf(style)
            }
            // A split takes the room left, as a scrolling container does,
            // and lays its children out in its sides as it draws.
            FlatKind::Split(_) => {
                return layout::Element::leaf(rooted(base, sizing, place, room)).with_key(index);
            }
            // A lazy column takes the room left down a column, and the
            // whole height of a row beside what shares it.
            FlatKind::Container(shape)
                if shape.lazy.is_some()
                    && matches!(place, Place::Column | Place::Row)
                    && !padded =>
            {
                let style = sized(base.with_height(Length::Flex(1.0)), sizing, place, room);
                return layout::Element::leaf(style).with_key(index);
            }
            FlatKind::Container(shape) if shape.scroll && place != Place::Content => {
                return layout::Element::leaf(rooted(base, sizing, place, room)).with_key(index);
            }
            FlatKind::Container(shape) => {
                let style = sized(base, sizing, place, room).with_padding(padding(
                    node.padding,
                    sizing,
                    room,
                ));
                return self.container_layout(flat, index, shape, style, room);
            }
        };
        let element = element.with_key(index);
        match padded {
            true => {
                let style = sized(base, sizing, place, room).with_padding(padding(
                    node.padding,
                    sizing,
                    room,
                ));
                layout::Element::column(style, vec![element])
            }
            false => element,
        }
    }

    /// The layout of the container `index` of `flat`, shaped as `shape`, in
    /// `style`, its gap and alignment as the app set them.
    fn container_layout<'t, A>(
        &'t self,
        flat: &'t [Flat<'_, A>],
        index: usize,
        shape: &Shape<'_>,
        style: Style,
        room: Room,
    ) -> layout::Element<'t, usize> {
        let metrics = room.metrics;
        let children = &flat[index].children;
        let sizing = flat[index].extras.as_ref().map(|extras| &extras.sizing);
        let arranged = |style: Style, across: bool| {
            let Some(sizing) = sizing else {
                return style;
            };
            let style = match sizing.gap {
                Some(gap) => style.with_gap(gap.pixels(across, metrics, room.scale)),
                None => style,
            };
            let style = match sizing.align {
                Some(align) => style.with_align(align),
                None => style,
            };
            match sizing.justify {
                Some(justify) => style.with_justify(justify),
                None => style,
            }
        };
        let element = match shape.direction {
            Direction::Column => {
                let children = children
                    .iter()
                    .map(|child| self.layout_of(flat, *child, Place::Column, room))
                    .collect();
                layout::Element::column(arranged(style, false), children)
            }
            Direction::Row => {
                let children = children
                    .iter()
                    .map(|child| self.layout_of(flat, *child, Place::Row, room))
                    .collect();
                let style = style
                    .with_gap(ROW_GAP * metrics.width)
                    .with_align(Align::Center);
                layout::Element::row(arranged(style, true), children)
            }
            Direction::Form => {
                let width = self.label_width(index, shape, metrics);
                let label = Style::new().with_width(Length::Fixed(width));
                let rows = children
                    .iter()
                    .map(|child| {
                        let control = self.layout_of(flat, *child, Place::Row, room);
                        let cells = vec![layout::Element::leaf(label), control];
                        layout::Element::row(Style::new(), cells).with_key(flat.len() + child)
                    })
                    .collect();
                let style = style.with_gap((FORM_ROW_GAP * metrics.height).round());
                layout::Element::column(arranged(style, false), rows)
            }
        };
        element.with_key(index)
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws element `index` of `flat` and the elements inside it, at
    /// `rects`, into `frame`: registering each control in the focus tree
    /// and describing it, and noting in `reveal` where the focused control
    /// lies inside the scrolling container `scroll` it is drawn in. A
    /// container that orders its tab stops as one block
    /// ([`is_tab_block`]) registers them inside a focus group.
    #[allow(clippy::too_many_arguments)]
    fn paint<A>(
        &mut self,
        flat: &[Flat<'_, A>],
        index: usize,
        rects: &mut Rects,
        frame: &mut Frame,
        theme: &Theme,
        room: Room,
        scroll: Option<ScrollContext>,
        reveal: &mut Option<(FocusId, Rect)>,
    ) {
        let node = &flat[index];
        if !is_tab_block(flat, index) {
            self.paint_element(flat, index, rects, frame, theme, room, scroll, reveal);
            return;
        }
        let group = node.id.child(GROUP_NODE);
        self.focus.enter_group(group, rects.get(index));
        if let Some(order) = node.order {
            self.focus.set_order(group, order);
        }
        self.paint_element(flat, index, rects, frame, theme, room, scroll, reveal);
        self.focus.leave();
    }

    /// Draws element `index` of `flat` and the elements inside it, as
    /// [`Store::paint`] says, inside the focus group it may have opened.
    #[allow(clippy::too_many_arguments)]
    fn paint_element<A>(
        &mut self,
        flat: &[Flat<'_, A>],
        index: usize,
        rects: &mut Rects,
        frame: &mut Frame,
        theme: &Theme,
        room: Room,
        scroll: Option<ScrollContext>,
        reveal: &mut Option<(FocusId, Rect)>,
    ) {
        let node = &flat[index];
        let rect = rects.get(index);
        if node.explicit.is_some()
            && let Some(scroll) = scroll
        {
            let at = Rect::new(
                rect.x - scroll.origin.x,
                rect.y - scroll.origin.y,
                rect.width,
                rect.height,
            );
            self.named.insert(node.id, (scroll.id, at));
        }
        self.paint_extras(flat, index, rect, frame, theme, scroll);
        // An element that scrolls itself fills the height its parent
        // leaves, which a scrolling container leaves unbounded.
        let scrolls = match &node.kind {
            FlatKind::Control(control) => control.scrolls_itself(),
            FlatKind::Container(shape) => shape.lazy.is_some(),
            FlatKind::Split(_) | FlatKind::Text(_) => false,
        };
        if scroll.is_some() && scrolls {
            frame.note_nested_scroll(|| path(flat, index));
        }
        match &node.kind {
            FlatKind::Text(text) => {
                let origin = Point::new(rect.x, rect.y);
                let styled = text.styled(theme.emphasis);
                let needed = match text.proportional {
                    true => {
                        draw_proportional_label(frame, theme, &styled, origin, rect);
                        frame.paragraph_width(&styled_paragraph(&styled))
                    }
                    false => {
                        frame.draw_text(&styled, origin, rect, theme);
                        styled.width() as f32 * frame.cell_metrics().width
                    }
                };
                self.note_regions(flat, index, rect, scroll);
                // Text cut short says it whole under the pointer, unless
                // what holds it has a tooltip of its own.
                if needed > rect.width + CUT_SLACK && !has_tooltip_above(flat, index) {
                    self.note_tip(rect, scroll, text_of(text));
                }
            }
            FlatKind::Control(control) => {
                let Some(entry) = self.entries.get_mut(&node.id) else {
                    return;
                };
                let drawn = control.draw(&mut *entry.widget, frame, rect, theme);
                let hit = drawn.hit;
                if control.takes_drops() {
                    self.drops.push((hit, node.id));
                }
                if let Some(layer) = drawn.layer {
                    self.popups.push((layer_id(node.id), layer));
                }
                if let Some(over) = drawn.over {
                    self.overs.push((node.id, over));
                }
                if control.is_focusable() {
                    match control.draws_focus_ring() {
                        true => self.focus.register(node.id, hit),
                        false => self.focus.register_without_ring(node.id, hit),
                    }
                    self.hits.push((node.id, hit));
                    if let Some(order) = node.order {
                        self.focus.set_order(node.id, order);
                    }
                } else if control.takes_pointer() {
                    self.focus.register_hit(node.id, hit);
                }
                control.describe(&*entry.widget, frame, node.id);
                for (rect, help) in control.tips(&*entry.widget) {
                    let shown = match scroll {
                        Some(scroll) => rect.intersection(&scroll.viewport),
                        None => Some(rect),
                    };
                    if let Some(rect) = shown {
                        self.tips.push((rect, help));
                    }
                }
                if self.focus.is_focused(node.id) {
                    self.caret = control.caret(&*entry.widget);
                    *reveal = scroll.map(|scroll| {
                        let at = Rect::new(
                            hit.x - scroll.origin.x,
                            hit.y - scroll.origin.y,
                            hit.width,
                            hit.height,
                        );
                        (scroll.id, at)
                    });
                }
                self.note_regions(flat, index, hit, scroll);
            }
            FlatKind::Split(shape) => {
                self.note_regions(flat, index, rect, scroll);
                let Some(split) = self.split_box(node.id) else {
                    return;
                };
                let sides = split.draw(frame, rect, theme);
                for (child, side) in node.children.iter().zip(sides) {
                    {
                        let layout = self.layout_of(flat, *child, Place::Root, room);
                        rects.take(&layout.compute(side), flat.len(), Some(index));
                    }
                    self.paint(flat, *child, rects, frame, theme, room, scroll, reveal);
                }
                // Over its sides, for the divider's grab either side of it;
                // a press in a side falls through to the elements there.
                if shape.on_resize.is_some() {
                    self.focus.register_hit(node.id, rect);
                }
            }
            FlatKind::Container(Shape {
                lazy: Some(lazy), ..
            }) => {
                self.note_regions(flat, index, rect, scroll);
                self.paint_lazy(flat, index, *lazy, rects, frame, theme, room, reveal);
            }
            FlatKind::Container(shape) if shape.scroll => {
                self.note_regions(flat, index, rect, scroll);
                self.paint_scrolled(flat, index, rects, frame, theme, room, reveal);
            }
            FlatKind::Container(shape) => {
                self.note_regions(flat, index, rect, scroll);
                let width = self.label_width(index, shape, room.metrics);
                for (place, child) in node.children.iter().enumerate() {
                    if let Some(label) = shape.labels.get(place) {
                        let row = rects.get(flat.len() + child);
                        draw_label(frame, label, row, (width, shape.chrome_labels), theme);
                    }
                    self.paint(flat, *child, rects, frame, theme, room, scroll, reveal);
                }
            }
        }
    }

    /// Draws the scrolling container `index` of `flat` at `rects`, then its
    /// children laid out inside its viewport, shifted by its offset and cut
    /// to it, as [`Store::paint`] draws them.
    #[allow(clippy::too_many_arguments)]
    fn paint_scrolled<A>(
        &mut self,
        flat: &[Flat<'_, A>],
        index: usize,
        rects: &mut Rects,
        frame: &mut Frame,
        theme: &Theme,
        room: Room,
        reveal: &mut Option<(FocusId, Rect)>,
    ) {
        let node = &flat[index];
        let rect = rects.get(index);
        let height = {
            let content = self.layout_of(flat, index, Place::Content, room);
            content
                .compute(rect)
                .rect(&index)
                .map_or(0.0, |rect| rect.height)
        };
        let Some(scroll) = self.scroll_view(node.id) else {
            return;
        };
        scroll.set_content_size(0.0, height);
        let viewport = scroll.draw(frame, rect, theme);
        scroll.access(frame, node.id);
        let offset = scroll.offset();
        let origin = Point::new(viewport.x, viewport.y - offset.y);
        {
            let content = self.layout_of(flat, index, Place::Content, room);
            let area = Rect::new(origin.x, origin.y, viewport.width, viewport.height);
            rects.take(&content.compute(area), flat.len(), Some(index));
        }
        // Its controls register inside it, so they show only within it.
        self.focus.enter(node.id, viewport);
        if let Some(order) = node.order {
            self.focus.set_order(node.id, order);
        }
        let context = ScrollContext {
            id: node.id,
            origin,
            viewport,
        };
        let width = match &node.kind {
            FlatKind::Container(shape) => self.label_width(index, shape, room.metrics),
            FlatKind::Control(_) | FlatKind::Split(_) | FlatKind::Text(_) => 0.0,
        };
        frame.clipped(viewport, |frame| {
            let FlatKind::Container(shape) = &node.kind else {
                return;
            };
            for (place, child) in node.children.iter().enumerate() {
                if let Some(label) = shape.labels.get(place) {
                    let row = rects.get(flat.len() + child);
                    draw_label(frame, label, row, (width, shape.chrome_labels), theme);
                }
                self.paint(
                    flat,
                    *child,
                    rects,
                    frame,
                    theme,
                    room,
                    Some(context),
                    reveal,
                );
            }
        });
        self.focus.leave();
    }

    /// Draws the lazy column `index` of `flat`, holding the rows `lazy` says
    /// of all it has, at `rects`: scrolled as its view says, the cursor's row
    /// brought into view when it moved, its rows laid out from where the
    /// first sits in all of them and clipped to it. Notes how tall the rows
    /// were and which to build next, and asks for another frame when they
    /// are not the ones built.
    #[allow(clippy::too_many_arguments)]
    fn paint_lazy<A>(
        &mut self,
        flat: &[Flat<'_, A>],
        index: usize,
        lazy: LazyShape,
        rects: &mut Rects,
        frame: &mut Frame,
        theme: &Theme,
        room: Room,
        reveal: &mut Option<(FocusId, Rect)>,
    ) {
        let node = &flat[index];
        let rect = rects.get(index);
        let state = self.lazies.entry(node.id).or_default();
        let average = match state.average > 0.0 {
            true => state.average,
            false => (LAZY_FIRST_ROW_CELLS * room.metrics.height).round(),
        };
        let moved = state.cursor != Some(lazy.cursor);
        state.cursor = Some(lazy.cursor);
        let Some(scroll) = self.scroll_view(node.id) else {
            return;
        };
        scroll.set_content_size(0.0, lazy.count as f32 * average);
        let viewport = scroll.draw(frame, rect, theme);
        scroll.access(frame, node.id);
        // The cursor's row comes into view at once when it moved: a jump
        // need not build every row it passes.
        if moved && lazy.count > 0 {
            let top = lazy.cursor as f32 * average;
            let offset = scroll.offset().y;
            let to = if top < offset {
                Some(top)
            } else if top + average > offset + viewport.height {
                Some(top + average - viewport.height)
            } else {
                None
            };
            if let Some(to) = to {
                scroll.scroll_to(Point::new(0.0, to));
            }
        }
        let offset = scroll.offset();
        let origin = Point::new(viewport.x, viewport.y - offset.y);
        // The rows built sit where the first is among all of them.
        let top = origin.y + lazy.first as f32 * average;
        {
            let content = self.layout_of(flat, index, Place::Content, room);
            let area = Rect::new(origin.x, top, viewport.width, viewport.height);
            rects.take(&content.compute(area), flat.len(), Some(index));
        }
        self.focus.enter(node.id, viewport);
        if let Some(order) = node.order {
            self.focus.set_order(node.id, order);
        }
        let context = ScrollContext {
            id: node.id,
            origin,
            viewport,
        };
        frame.clipped(viewport, |frame| {
            for child in &node.children {
                self.paint(
                    flat,
                    *child,
                    rects,
                    frame,
                    theme,
                    room,
                    Some(context),
                    reveal,
                );
            }
        });
        self.focus.leave();
        // How tall the rows were, and which show now with a view's worth
        // more above and below, for a wheel's glide to find built.
        let heights: Vec<f32> = node
            .children
            .iter()
            .map(|child| rects.get(*child).height)
            .collect();
        let state = self.lazies.entry(node.id).or_default();
        if !heights.is_empty() {
            state.average = heights.iter().sum::<f32>() / heights.len() as f32;
        }
        let average = state.average.max(1.0);
        let above = (offset.y - viewport.height).max(0.0);
        let below = offset.y + 2.0 * viewport.height;
        let first = ((above / average).floor() as usize).min(lazy.count);
        let end = ((below / average).ceil() as usize).min(lazy.count);
        let built = lazy.first..lazy.first + node.children.len();
        // Another frame, where what shows is not all built.
        let wanted = ((offset.y / average).floor() as usize).min(lazy.count)
            ..(((offset.y + viewport.height) / average).ceil() as usize).min(lazy.count);
        if wanted.start < built.start || wanted.end > built.end {
            self.lazy_again = true;
        }
        state.rows = first..end;
        // The rows on screen, as laid out, for an app that goes by them.
        if node
            .extras
            .as_ref()
            .is_some_and(|extras| extras.on_rows_shown.is_some())
        {
            let bottom = viewport.y + viewport.height;
            let mut on_screen = node.children.iter().enumerate().filter(|(_, child)| {
                let row = rects.get(**child);
                row.y + row.height > viewport.y && row.y < bottom
            });
            let shown = match (on_screen.next(), on_screen.next_back()) {
                (Some((top, _)), Some((last, _))) => lazy.first + top..lazy.first + last + 1,
                (Some((top, _)), None) => lazy.first + top..lazy.first + top + 1,
                (None, _) => lazy.first..lazy.first,
            };
            self.rows_shown.insert(node.id, shown);
        }
    }

    /// Draws the surface `node` was given under it at `rect`, inside the
    /// scrolling container `scroll` (if any), and registers its region for
    /// the pointer (a tab stop, described as a button, when it takes
    /// presses and is not a control already), noting where it takes drops.
    fn paint_extras<A>(
        &mut self,
        flat: &[Flat<'_, A>],
        index: usize,
        rect: Rect,
        frame: &mut Frame,
        theme: &Theme,
        scroll: Option<ScrollContext>,
    ) {
        let node = &flat[index];
        let Some(extras) = &node.extras else {
            return;
        };
        if extras.keep_in_view
            && let Some(scroll) = scroll
        {
            let at = Rect::new(
                rect.x - scroll.origin.x,
                rect.y - scroll.origin.y,
                rect.width,
                rect.height,
            );
            self.kept = Some((node.id, scroll.id, at));
        }
        if extras.on_size_change.is_some() {
            let size = ElementSize {
                height: rect.height,
                scale_factor: frame.scale_factor(),
                width: rect.width,
            };
            self.laid_out.insert(node.id, size);
        }
        let region = region_id(node.id);
        let hovered = self
            .entries
            .get_mut(&region)
            .and_then(|entry| downcast_region(&mut *entry.widget))
            .is_some_and(|kept| {
                kept.place(rect);
                kept.is_hovered()
            });
        draw_surface(frame, rect, extras, hovered, theme);
        if !extras.has_region() {
            return;
        }
        let is_control = matches!(&node.kind, FlatKind::Control(control) if control.is_focusable());
        if (extras.on_press.is_some() || extras.on_pointer_press.is_some()) && !is_control {
            frame.note_pressable(rect, || access_name(flat, index), !extras.surface.unlit);
        }
        match extras.on_press.is_some() && !is_control {
            true => {
                self.focus.register(region, rect);
                self.hits.push((region, rect));
                // A container's order is its group's or its scrolling
                // view's.
                if let Some(order) = node.order
                    && !matches!(node.kind, FlatKind::Container(_) | FlatKind::Split(_))
                {
                    self.focus.set_order(region, order);
                }
                describe_button(frame, region, rect, &access_name(flat, index));
            }
            false => self.focus.register_hit(region, rect),
        }
        let shown = match scroll {
            Some(scroll) => rect.intersection(&scroll.viewport),
            None => Some(rect),
        };
        let Some(shown) = shown else {
            return;
        };
        if extras.drop.is_some() {
            self.drops.push((shown, node.id));
        }
        if extras.file_drop.is_some() {
            self.file_targets.push((shown, node.id));
        }
    }

    /// Notes where element `index` of `flat` was drawn, at `rect` inside
    /// the scrolling container `scroll` (if any), when it has a tooltip or
    /// a context menu callback, for the pointer to find.
    fn note_regions<A>(
        &mut self,
        flat: &[Flat<'_, A>],
        index: usize,
        rect: Rect,
        scroll: Option<ScrollContext>,
    ) {
        let node = &flat[index];
        // A tooltip saying only what its element shows says nothing.
        if let Some(help) = &node.tooltip
            && squeezed(help) != squeezed(&own_words(flat, index))
        {
            self.note_tip(rect, scroll, help.to_string());
        }
        if node.context.is_some()
            && let Some(rect) = shown_part(rect, scroll)
        {
            self.contexts.push((rect, node.id));
        }
    }

    /// Notes `help` as the tooltip of what was drawn at `rect` inside the
    /// scrolling container `scroll` (if any), for the pointer to find.
    fn note_tip(&mut self, rect: Rect, scroll: Option<ScrollContext>, help: String) {
        if let Some(rect) = shown_part(rect, scroll) {
            self.tips.push((rect, help));
        }
    }

    /// The split view kept for the split `id`.
    fn split_box(&mut self, id: FocusId) -> Option<&mut SplitBox> {
        downcast_split(&mut *self.entries.get_mut(&id)?.widget)
    }

    /// The first kept widget of type `W`, for misty's tests to look
    /// inside.
    #[cfg(test)]
    pub(crate) fn widget<W: 'static>(&mut self) -> Option<&mut W> {
        self.entries
            .values_mut()
            .find_map(|entry| (&mut *entry.widget as &mut dyn Any).downcast_mut())
    }

    /// The scroll view kept for the container `id`, if it scrolls.
    fn scroll_view(&mut self, id: FocusId) -> Option<&mut ScrollView> {
        downcast_scroll(&mut *self.entries.get_mut(&id)?.widget)
    }
}

// ========================================================================
// Place Implementations
// ========================================================================

impl Place {
    /// The size an element starts from at this place.
    fn style(self) -> Style {
        let whole = Length::Percent(1.0);
        match self {
            Self::Column => Style::new(),
            Self::Content => Style::new().with_width(whole),
            Self::Root => Style::new().with_width(whole).with_height(whole),
            Self::Row => Style::new().with_width(Length::Flex(1.0)),
        }
    }
}

// ========================================================================
// Rects Implementations
// ========================================================================

impl Rects {
    /// Where element (or form row) `key` landed; empty when it was not
    /// laid out.
    fn get(&self, key: usize) -> Rect {
        self.0
            .get(key)
            .copied()
            .flatten()
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
    }

    /// Takes where each element of a tree of `count` elements, and each
    /// form row, landed in `layout`, but for the element `skip`.
    fn take(&mut self, layout: &layout::Layout<usize>, count: usize, skip: Option<usize>) {
        for (key, slot) in self.0.iter_mut().enumerate().take(2 * count) {
            if Some(key) != skip
                && let Some(rect) = layout.rect(&key)
            {
                *slot = Some(rect);
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The elements of the tree `root`, in reading order, each with its id,
/// and then each layer's, a layer's own layers after it.
fn flatten<'a, A>(
    root: Element<'a, A>,
    scope: Option<FocusId>,
    lazies: &HashMap<FocusId, LazyState>,
) -> Tree<'a, A> {
    let mut flat = Vec::new();
    let mut pending = VecDeque::new();
    let root_id = scoped(scope, ROOT);
    push_flat(
        &mut flat,
        &mut pending,
        root,
        None,
        0,
        0,
        root_id,
        None,
        scope,
        lazies,
    );
    let mut layers = Vec::new();
    while let Some(Pending {
        anchor,
        layer,
        slot,
    }) = pending.pop_front()
    {
        let root = flat.len();
        let anchor_id = flat[anchor].id;
        let id_slot = OVERLAY_SLOT + slot as u64;
        push_flat(
            &mut flat,
            &mut pending,
            layer.content,
            Some(anchor),
            slot,
            id_slot,
            anchor_id,
            None,
            scope,
            lazies,
        );
        layers.push(Layer {
            end: flat.len(),
            keys_to_app: layer.keys_to_app && layer.modal,
            modal: layer.modal,
            root,
        });
    }
    Tree { flat, layers }
}

/// Adds `element`, the `index`th child of the element `parent` whose id is
/// `parent_id`, its id made with `slot` unless it has its own (made under
/// `scope`), named `name` by a form label, and the elements inside it to
/// `flat`, and the layers it was given to `pending`.
#[allow(clippy::too_many_arguments)]
fn push_flat<'a, A>(
    flat: &mut Vec<Flat<'a, A>>,
    pending: &mut VecDeque<Pending<'a, A>>,
    element: Element<'a, A>,
    parent: Option<usize>,
    index: usize,
    slot: u64,
    parent_id: FocusId,
    name: Option<&'a str>,
    scope: Option<FocusId>,
    lazies: &HashMap<FocusId, LazyState>,
) {
    let id = match &element.id {
        Some(explicit) => scoped(scope, FocusId::new(explicit)),
        None => parent_id.child(slot),
    };
    let (kind, children) = match element.kind {
        Kind::Container(container) => {
            let shape = Shape {
                chrome_labels: container.chrome_labels,
                direction: container.direction,
                labels: container.labels,
                lazy: None,
                scroll: container.scroll,
            };
            (FlatKind::Container(shape), container.children)
        }
        Kind::Lazy(lazy) => lazy_rows(lazy, lazies.get(&id)),
        Kind::Control(control) => (FlatKind::Control(control), Vec::new()),
        Kind::Split(parts) => {
            let shape = SplitShape {
                axis: parts.axis,
                on_resize: parts.on_resize,
                ratio: parts.ratio,
            };
            (FlatKind::Split(shape), parts.children)
        }
        Kind::Text(text) => (FlatKind::Text(text), Vec::new()),
    };
    let me = flat.len();
    flat.push(Flat {
        children: Vec::new(),
        context: element.context,
        explicit: element.id,
        extras: element.extras,
        id,
        index,
        kind,
        name,
        order: element.order,
        padding: element.padding,
        parent,
        tooltip: element.tooltip,
    });
    for (slot, layer) in element.overlays.into_iter().enumerate() {
        if let Some(layer) = layer {
            pending.push_back(Pending {
                anchor: me,
                layer,
                slot,
            });
        }
    }
    for (place, child) in children.into_iter().enumerate() {
        let label = match &flat[me].kind {
            FlatKind::Container(shape) => shape.labels.get(place).copied(),
            FlatKind::Control(_) | FlatKind::Split(_) | FlatKind::Text(_) => None,
        };
        let name = label.filter(|label| !label.is_empty());
        // A lazy column's row keeps its id by which row it is, whichever
        // rows are built around it.
        let slot = match &flat[me].kind {
            FlatKind::Container(Shape {
                lazy: Some(lazy), ..
            }) => lazy.first + place,
            FlatKind::Container(_)
            | FlatKind::Control(_)
            | FlatKind::Split(_)
            | FlatKind::Text(_) => place,
        };
        let child_index = flat.len();
        push_flat(
            flat,
            pending,
            child,
            Some(me),
            place,
            slot as u64,
            id,
            name,
            scope,
            lazies,
        );
        flat[me].children.push(child_index);
    }
}

/// The flattened shape of the lazy column `lazy` and the rows it builds:
/// those `state` says show, or its first rows before it was ever drawn.
fn lazy_rows<'a, A>(
    lazy: Lazy<'a, A>,
    state: Option<&LazyState>,
) -> (FlatKind<'a, A>, Vec<Element<'a, A>>) {
    let count = lazy.rows.count();
    let rows = match state {
        Some(state) if !state.rows.is_empty() || count == 0 => {
            state.rows.start.min(count)..state.rows.end.min(count)
        }
        _ => 0..count.min(LAZY_FIRST_ROWS),
    };
    let shape = Shape {
        chrome_labels: false,
        direction: Direction::Column,
        labels: Vec::new(),
        lazy: Some(LazyShape {
            count,
            cursor: lazy.cursor,
            first: rows.start,
        }),
        scroll: true,
    };
    let children = rows.map(|row| lazy.rows.row(row)).collect();
    (FlatKind::Container(shape), children)
}

/// `id` as a tree under `scope` has it: `id` itself in a window's own
/// tree, and a child of `scope` in a tree sharing its window.
fn scoped(scope: Option<FocusId>, id: FocusId) -> FocusId {
    match scope {
        Some(scope) => scope.child(id.get()),
        None => id,
    }
}

/// The ids of element `index` of `flat` and every element inside it,
/// with the ids their pressable regions and popups take the keyboard
/// under.
fn subtree_ids<A>(flat: &[Flat<'_, A>], index: usize) -> HashSet<FocusId> {
    let mut ids = HashSet::new();
    let mut todo = vec![index];
    while let Some(at) = todo.pop() {
        let id = flat[at].id;
        ids.extend([id, region_id(id), layer_id(id)]);
        todo.extend(&flat[at].children);
    }
    ids
}

/// Runs on `app` what the elements of its `window` say time changed (the
/// toasts expiring), asking `cx` for a redraw when anything did; returns
/// when the next change is due.
fn tick<H: Host>(
    host: &mut H,
    scope: Option<FocusId>,
    lazies: &HashMap<FocusId, LazyState>,
    cx: &mut EventCx<'_>,
) -> Option<Instant> {
    let now = clock::now();
    let (calls, due) = {
        let tree = flatten(host.view(), scope, lazies);
        let mut calls: Vec<Call<H::State>> = Vec::new();
        let mut due = None;
        for node in &tree.flat {
            if let FlatKind::Control(control) = &node.kind
                && control.is_ticking()
            {
                let (call, next) = control.tick(now);
                calls.extend(call);
                due = due.into_iter().chain(next).min();
            }
        }
        (calls, due)
    };
    if !calls.is_empty() {
        cx.request_redraw();
    }
    for call in calls {
        call(host.state());
    }
    due
}

/// Panics when two elements of `flat` have one id, naming both.
fn check_unique_ids<A>(flat: &[Flat<'_, A>]) {
    let mut seen: HashMap<FocusId, usize> = HashMap::with_capacity(flat.len());
    for (index, node) in flat.iter().enumerate() {
        if let Some(first) = seen.insert(node.id, index) {
            let name = node
                .explicit
                .as_deref()
                .map_or_else(|| path(flat, index), str::to_owned);
            panic!(
                "misty: two elements have the id {name:?} ({} and {}).\nGive one of them \
                 another id with `.id(..)`, or build it once.",
                path(flat, first),
                path(flat, index),
            );
        }
    }
}

/// The first control inside the container `index` of `flat` whose id is
/// its place, through containers that have no id of their own either.
fn implicit_control<A>(flat: &[Flat<'_, A>], index: usize) -> Option<usize> {
    flat[index]
        .children
        .iter()
        .filter(|child| flat[**child].explicit.is_none())
        .find_map(|child| match flat[*child].kind {
            FlatKind::Control(_) => Some(*child),
            FlatKind::Container(_) | FlatKind::Split(_) => implicit_control(flat, *child),
            FlatKind::Text(_) => None,
        })
}

/// Where element `index` of `flat` is in the tree, as messages name it:
/// each element's kind from the root down, with its place among its
/// siblings (`column/0/form/3/slider`).
fn path<A>(flat: &[Flat<'_, A>], index: usize) -> String {
    let mut chain = vec![index];
    while let Some(parent) = flat[*chain.last().unwrap_or(&index)].parent {
        chain.push(parent);
    }
    chain
        .iter()
        .rev()
        .enumerate()
        .map(|(depth, at)| match depth {
            0 => kind_name(&flat[*at].kind).to_owned(),
            _ => format!("{}/{}", flat[*at].index, kind_name(&flat[*at].kind)),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// What messages call an element of `kind`.
fn kind_name<A>(kind: &FlatKind<'_, A>) -> &'static str {
    match kind {
        FlatKind::Container(shape) => match shape.direction {
            Direction::Column => "column",
            Direction::Form => "form",
            Direction::Row => "row",
        },
        FlatKind::Control(control) => control.kind(),
        FlatKind::Split(_) => "split",
        FlatKind::Text(_) => "text",
    }
}

/// How wide the label column of the cell-grid form `shape` is with
/// `metrics`: its widest label and the gap after it.
fn cell_label_width(shape: &Shape<'_>, metrics: CellMetrics) -> f32 {
    let widest = shape
        .labels
        .iter()
        .map(|label| text::width(label))
        .max()
        .unwrap_or(0);
    (widest as f32 + LABEL_GAP) * metrics.width
}

/// Draws a form's `label`, muted, at the left of its `row`, `width` wide,
/// centered on the row's height: as chrome when `chrome`, else on the
/// cell grid.
fn draw_label(
    frame: &mut Frame,
    label: &str,
    row: Rect,
    (width, chrome): (f32, bool),
    theme: &Theme,
) {
    if label.is_empty() {
        return;
    }
    let metrics = frame.cell_metrics();
    let y = (row.y + (row.height - metrics.height) / 2.0).round();
    let clip = Rect::new(row.x, y, width, metrics.height);
    if chrome {
        frame.clipped(clip, |frame| {
            label::draw_label(frame, theme, label, Point::new(row.x, y), theme.muted);
        });
        return;
    }
    let text = StyledText::plain(label, ThemeToken::Muted.into());
    frame.draw_text(&text, Point::new(row.x, y), clip, theme);
}

/// The blank space inside an element: `cells` in cell columns and rows,
/// and the pixels `sizing` adds, in physical pixels in `room`.
fn padding((columns, rows): (f32, f32), sizing: Option<&Sizing>, room: Room) -> Edges {
    let metrics = room.metrics;
    let pixels = sizing.map_or(Edges::default(), |sizing| sizing.padding(room.scale));
    Edges {
        bottom: rows * metrics.height + pixels.bottom,
        left: columns * metrics.width + pixels.left,
        right: columns * metrics.width + pixels.right,
        top: rows * metrics.height + pixels.top,
    }
}

/// `style` sized as the app set in `sizing`, for an element laid out at
/// `place` in `room`: its width and height, their bounds, and its share
/// of the room along its container's direction. A root (the window's, a
/// layer's, a split side's) has no siblings to share with and fills its
/// area already, so growing leaves it filling it.
fn sized(style: Style, sizing: Option<&Sizing>, place: Place, room: Room) -> Style {
    let Some(sizing) = sizing else {
        return style;
    };
    let (metrics, scale) = (room.metrics, room.scale);
    let mut style = style;
    if let Some(grow) = sizing.grow {
        style = match place {
            Place::Row => style.with_width(Length::Flex(grow)),
            Place::Column | Place::Content => style.with_height(Length::Flex(grow)),
            Place::Root => style,
        };
    }
    if let Some(width) = sizing.width {
        style = style.with_width(width.length(true, metrics, scale));
    }
    if let Some(height) = sizing.height {
        style = style.with_height(height.length(false, metrics, scale));
    }
    if let Some(width) = sizing.min_width {
        style = style.with_min_width(width.length(true, metrics, scale));
    }
    if let Some(width) = sizing.max_width {
        style = style.with_max_width(width.length(true, metrics, scale));
    }
    if let Some(height) = sizing.min_height {
        style = style.with_min_height(height.length(false, metrics, scale));
    }
    if let Some(height) = sizing.max_height {
        style = style.with_max_height(height.length(false, metrics, scale));
    }
    match sizing.shrink {
        Some(shrink) => style.with_shrink(shrink),
        None => style,
    }
}

/// Whether element `index` of `flat` registers its tab stops in a focus
/// group of their own, Tab visiting them as one block: a container or a
/// split (but a scrolling container, which the focus tree enters anyway)
/// that the app gave an order, or whose children include such a block,
/// so that order is among its siblings in the tree.
fn is_tab_block<A>(flat: &[Flat<'_, A>], index: usize) -> bool {
    let is_block = |node: &Flat<'_, A>| match &node.kind {
        FlatKind::Container(shape) => !shape.scroll,
        FlatKind::Split(_) => true,
        FlatKind::Control(_) | FlatKind::Text(_) => false,
    };
    let node = &flat[index];
    is_block(node)
        && (node.order.is_some()
            || node.children.iter().any(|child| {
                let child = &flat[*child];
                child.order.is_some()
                    && matches!(child.kind, FlatKind::Container(_) | FlatKind::Split(_))
            }))
}

/// `style` for an element that takes the room its column leaves, as a
/// scrolling container and a split do, sized as the app set in `sizing`.
fn rooted(style: Style, sizing: Option<&Sizing>, place: Place, room: Room) -> Style {
    let style = match place {
        Place::Column => style.with_height(Length::Flex(1.0)),
        Place::Content | Place::Root | Place::Row => style,
    };
    sized(style, sizing, place, room)
}

/// Draws the surface `extras` asks for under an element at `rect`: its
/// background (the hover one while `hovered`) and border in `theme`'s
/// colors, its corners from the theme's shape.
fn draw_surface<A>(
    frame: &mut Frame,
    rect: Rect,
    extras: &Extras<'_, A>,
    hovered: bool,
    theme: &Theme,
) {
    let surface = extras.surface;
    let (fill, border) = match hovered {
        true => (
            extras.hover_fill().or(surface.background),
            surface.hover_border.or(surface.border),
        ),
        false => (surface.background, surface.border),
    };
    if fill.is_none() && border.is_none() {
        return;
    }
    let scale = frame.scale_factor();
    let radius = match surface.corners {
        Corners::Box => theme.shape.corner_radius,
        Corners::Control => theme.shape.control_radius,
        Corners::Square => 0,
    };
    // A border asked for shows even in a theme whose boxes have none.
    let width = theme.shape.border(scale).max((scale as f32).round());
    let style = BoxStyle {
        border: border.map(|token| Border {
            color: theme.color(token).into(),
            width,
        }),
        corner_radius: f32::from(radius) * scale as f32,
        fill: fill.map_or(theme.background.with_alpha(0), |token| {
            theme.color(token).into()
        }),
        shadow: None,
    };
    frame.draw_box(rect, &style);
}

/// The name screen readers call the pressable region of element `index`
/// of `flat` by: the name the app gave it, its text, its tooltip, or, for
/// a container, the text inside it (a row of a list, a sidebar entry).
fn access_name<A>(flat: &[Flat<'_, A>], index: usize) -> String {
    let node = &flat[index];
    if let Some(name) = node.extras.as_ref().and_then(|extras| extras.access_name) {
        return name.to_owned();
    }
    match &node.kind {
        FlatKind::Text(text) => text_of(text),
        FlatKind::Container(_) => {
            let mut words = Vec::new();
            let mut todo = vec![index];
            while let Some(at) = todo.pop() {
                if let FlatKind::Text(text) = &flat[at].kind {
                    words.push(text_of(text));
                }
                // Children in reading order, the first popped first.
                todo.extend(flat[at].children.iter().rev());
            }
            words.retain(|word| !word.is_empty());
            // A container with no words of its own, such as one around an
            // icon, is named by its tooltip.
            match words.is_empty() {
                true => node.tooltip.as_deref().unwrap_or_default().to_owned(),
                false => words.join(" "),
            }
        }
        FlatKind::Control(_) | FlatKind::Split(_) => {
            node.tooltip.as_deref().unwrap_or_default().to_owned()
        }
    }
}

/// What `text` says, its runs joined.
/// The part of `rect` in sight inside the scrolling container `scroll`, if
/// any, or `None` when it is scrolled out of sight.
fn shown_part(rect: Rect, scroll: Option<ScrollContext>) -> Option<Rect> {
    match scroll {
        Some(scroll) => rect.intersection(&scroll.viewport),
        None => Some(rect),
    }
}

/// The words element `index` of `flat` shows itself: a text's, or those of
/// the texts inside a container, in reading order; none for a control or a
/// split, whose words misty draws.
fn own_words<A>(flat: &[Flat<'_, A>], index: usize) -> String {
    let mut words = Vec::new();
    let mut todo = vec![index];
    while let Some(at) = todo.pop() {
        match &flat[at].kind {
            FlatKind::Text(text) => words.push(text_of(text)),
            FlatKind::Container(_) => todo.extend(flat[at].children.iter().rev()),
            FlatKind::Control(_) | FlatKind::Split(_) => {}
        }
    }
    words.join(" ")
}

/// `text` with its runs of white space made single spaces and its ends
/// trimmed, for telling whether two texts say the same.
fn squeezed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether element `index` of `flat`, or any element holding it, has a
/// tooltip, which then speaks for its text.
fn has_tooltip_above<A>(flat: &[Flat<'_, A>], index: usize) -> bool {
    let mut at = Some(index);
    while let Some(index) = at {
        if flat[index].tooltip.is_some() {
            return true;
        }
        at = flat[index].parent;
    }
    false
}

fn text_of(text: &Text<'_>) -> String {
    text.spans.iter().map(|(run, _)| run.as_ref()).collect()
}

/// Whether `node` takes the pointer, or the keyboard for a press, in a
/// region of its own.
fn has_region<A>(node: &Flat<'_, A>) -> bool {
    node.extras
        .as_ref()
        .is_some_and(|extras| extras.has_region())
}

/// `widget` as the region of an element, if it is one.
fn downcast_region(widget: &mut dyn Stored) -> Option<&mut RegionBox> {
    (widget as &mut dyn Any).downcast_mut()
}

/// `widget` as the split view of a split, if it is one.
fn downcast_split(widget: &mut dyn Stored) -> Option<&mut SplitBox> {
    (widget as &mut dyn Any).downcast_mut()
}

/// `widget` as the scroll view of a scrolling container, if it is one.
fn downcast_scroll(widget: &mut dyn Stored) -> Option<&mut ScrollView> {
    (widget as &mut dyn Any).downcast_mut()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::base::key::{Key, KeyCode, Mods};
    use crate::base::notice::{Notice, Notifier, Severity};
    use crate::chrome::title_bar::TitleBarButton;
    use crate::input::event::{ButtonInput, PointerButton};
    use crate::render::paragraph::Paragraph;
    use crate::testing::{TreeWindow, access_tree};
    use crate::view::chrome::{split, tab, tab_strip, title_bar};
    use crate::view::controls::{button, checkbox, icon_button, progress, slider, text_field};
    use crate::view::custom::canvas;
    use crate::view::data::list;
    use crate::view::element::{column, field, form, overlay, row, text};
    use crate::view::popups::{ToastCards, context_menu, dialog, menu, toasts, which_key};
    use crate::view::props::PointerDrag;
    use crate::widgets::list_view::ListSource;
    use crate::widgets::menu::{MenuEntry, MenuItem};
    use crate::widgets::number_range::NumberRange;
    use crate::widgets::which_key::WhichKeyEntry;

    /// The values the probe's slider takes.
    const RANGE: NumberRange = NumberRange::new(0.0, 10.0, 1.0);

    /// An app whose tree each test picks, recording what reaches it.
    struct Probe {
        /// The id of the menu item chosen last.
        chosen: Option<u8>,
        /// Whether the tree shows its optional element.
        extra: bool,
        /// The keys no element took.
        keys: Vec<Key>,
        /// Where the context menu the user asked for opens, while it does.
        menu: Option<MenuRequest>,
        /// A text the tree's field edits.
        name: String,
        /// The notices the tree's toasts show.
        notifier: Notifier,
        /// The toast clicked last.
        opened: bool,
        /// The keys the app's keymap waits on.
        prefix: Vec<Key>,
        /// The colors it is drawn in.
        theme: Theme,
        /// A number the tree's sliders move.
        value: f64,
        /// The test's tree.
        view: fn(&Probe) -> Element<'_, Probe>,
    }

    impl App for Probe {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            (self.view)(self)
        }

        /// Records `key`; Alt+H, Alt+L, and Alt+X move the focus to the
        /// elements `left`, `right`, and `nowhere`, as pane moves do, and
        /// Alt+F shows hint labels.
        fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
            self.keys.push(key);
            let pane = match (key.mods, key.code) {
                (Mods::ALT, KeyCode::Char('h')) => Some("left"),
                (Mods::ALT, KeyCode::Char('l')) => Some("right"),
                (Mods::ALT, KeyCode::Char('x')) => Some("nowhere"),
                _ => None,
            };
            if let Some(pane) = pane {
                cx.focus(pane);
            }
            if key == Key::new(KeyCode::Char('f'), Mods::ALT) {
                cx.show_hints();
            }
            true
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window 400 by `height` pixels showing the tree `view`.
    fn window(height: u32, view: fn(&Probe) -> Element<'_, Probe>) -> TreeWindow<Probe> {
        let probe = Probe {
            chosen: None,
            extra: false,
            keys: Vec::new(),
            menu: None,
            name: String::new(),
            notifier: Notifier::new(),
            opened: false,
            prefix: Vec::new(),
            theme: Theme::dark(),
            value: 0.0,
            view,
        };
        TreeWindow::new(probe, "t", 400, height)
    }

    /// Two buttons setting the probe's value to 1 and 2, a pressable text
    /// setting it to 3, and the probe's text field.
    fn hinted(probe: &Probe) -> Element<'_, Probe> {
        column([
            button("One")
                .on_click(|probe: &mut Probe| probe.value = 1.0)
                .into(),
            button("Two")
                .on_click(|probe: &mut Probe| probe.value = 2.0)
                .into(),
            Element::from(text("Three"))
                .on_press(|probe: &mut Probe, _| probe.value = 3.0)
                .access_name("Three"),
            Element::from(
                text_field(&probe.name).on_change(|probe: &mut Probe, name| probe.name = name),
            )
            .id("name"),
        ])
    }

    #[test]
    fn test_a_hint_label_typed_presses_its_target_and_esc_takes_them_away() {
        let mut window = window(300, hinted);
        window.press("M-f");
        assert!(window.store().hints.is_active(), "labels over the controls");
        assert_eq!(window.store().hint_targets.len(), 4, "every one in reach");
        window.press("s");
        assert_eq!(window.app().value, 2.0, "the second label presses Two");
        assert!(!window.store().hints.is_active(), "and the labels go");
        window.press("M-f");
        window.press("d");
        assert_eq!(window.app().value, 3.0, "a pressable element too");
        window.press("M-f");
        window.press("f");
        window.type_text("ok");
        assert_eq!(window.app().name, "ok", "a text field gets the keyboard");
        window.press("M-f");
        window.press("ESC");
        window.press("a");
        assert!(!window.store().hints.is_active());
        assert_ne!(window.app().value, 1.0, "Esc took the labels away first");
    }

    /// A tab strip of two tabs, the one activated stored as the value.
    fn hinted_tabs(_probe: &Probe) -> Element<'_, Probe> {
        let tabs = [tab(1, "one"), tab(2, "two")];
        column([tab_strip(tabs, Some(&1))
            .on_activate(|probe: &mut Probe, key: u64| probe.value = key as f64)
            .into()])
    }

    #[test]
    fn test_each_tab_of_a_strip_gets_a_hint_label_of_its_own() {
        let mut window = window(200, hinted_tabs);
        window.press("M-f");
        let labels = window.store().hint_targets.len();
        assert!(labels >= 3, "two tabs and the new tab button: {labels}");
        let activated = "asdfjkl".chars().take(labels).any(|letter| {
            window.press("M-f");
            window.press(&letter.to_string());
            window.app().value == 2.0
        });
        assert!(activated, "a label of its own presses the second tab");
    }

    /// A tab strip of two tabs, closing one storing 10 more than its key.
    fn closable_tabs(_probe: &Probe) -> Element<'_, Probe> {
        let tabs = [tab(1, "one"), tab(2, "two")];
        column([tab_strip(tabs, Some(&1))
            .on_close(|probe: &mut Probe, key: u64| probe.value = 10.0 + key as f64)
            .into()])
    }

    #[test]
    fn test_a_tabs_close_button_gets_a_hint_label_that_closes_it() {
        let mut window = window(200, closable_tabs);
        window.press("M-f");
        let labels = window.store().hint_targets.len();
        assert!(
            labels >= 5,
            "two tabs, two close buttons, new tab: {labels}"
        );
        let closed = "asdfjkl".chars().take(labels).any(|letter| {
            window.press("M-f");
            window.press(&letter.to_string());
            window.app().value == 12.0
        });
        assert!(closed, "a label of its own closes the second tab");
    }

    /// Three rows, the one activated stored as the value.
    struct Rows;

    impl ListSource for Rows {
        fn row_count(&self) -> usize {
            3
        }

        fn row_text(&self, row: usize) -> std::borrow::Cow<'_, str> {
            format!("row {row}").into()
        }
    }

    /// A list of [`Rows`], activating a row storing its index.
    fn hinted_rows(_probe: &Probe) -> Element<'_, Probe> {
        column([list(&Rows, 0)
            .on_activate(|probe: &mut Probe, row| probe.value = row as f64)
            .into()])
    }

    #[test]
    fn test_each_row_of_a_list_gets_a_hint_label_that_activates_it() {
        let mut window = window(200, hinted_rows);
        window.press("M-f");
        assert_eq!(window.store().hint_targets.len(), 3, "a label a row");
        window.press("d");
        assert_eq!(window.app().value, 2.0, "the third label activates row 2");
    }

    /// A button under a modal layer holding one button of its own.
    fn hinted_modal(_probe: &Probe) -> Element<'_, Probe> {
        column([button("Under")
            .on_click(|probe: &mut Probe| probe.value = 1.0)
            .into()])
        .overlay(
            overlay(column([button("Over")
                .on_click(|probe: &mut Probe| probe.value = 2.0)
                .into()]))
            .modal()
            .keys_to_app(),
        )
    }

    #[test]
    fn test_hint_labels_go_only_over_the_topmost_modal_layer() {
        let mut window = window(300, hinted_modal);
        window.press("M-f");
        assert_eq!(window.store().hint_targets.len(), 1);
        window.press("a");
        assert_eq!(window.app().value, 2.0, "the layer's button");
    }

    /// A source dragged only with Alt held, carrying 5, whose plain press
    /// sets the value to 1 and whose Alt+click sets it to 7, over a target
    /// setting it to ten times what is dropped.
    fn alt_drag(_probe: &Probe) -> Element<'_, Probe> {
        column([
            Element::from(text("source"))
                .drag_source_with(Mods::ALT, 5_u8, "five")
                .on_pointer_press(|probe: &mut Probe, _| probe.value = 1.0)
                .on_pointer_drag(|probe: &mut Probe, drag: PointerDrag| {
                    if drag.done && drag.mods == Mods::ALT && drag.point == drag.start {
                        probe.value = 7.0;
                    }
                }),
            Element::from(text("target")).grow(1.0).drop_target(
                |_: &u8| true,
                |probe: &mut Probe, carried: u8, _| probe.value = f64::from(carried) * 10.0,
            ),
        ])
    }

    /// Presses the left button at `from` holding `mods`, moves to `to`,
    /// and releases there.
    fn drag_holding(window: &mut TreeWindow<Probe>, from: Point, to: Point, mods: Mods) {
        window.send(&Input::PointerMoved(from));
        let mut press = ButtonInput::press(PointerButton::Left, from);
        press.mods = mods;
        window.send(&Input::PointerButton(press));
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            let at = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            window.send(&Input::PointerMoved(at));
        }
        window.send(&Input::PointerButton(ButtonInput::release(
            PointerButton::Left,
            to,
        )));
    }

    #[test]
    fn test_a_drag_source_asking_for_alt_starts_only_with_alt_and_takes_the_press() {
        let mut window = window(300, alt_drag);
        let (from, to) = (Point::new(20.0, 8.0), Point::new(20.0, 200.0));
        drag_holding(&mut window, from, to, Mods::NONE);
        assert_eq!(
            window.app().value,
            1.0,
            "a plain press is the element's, no drop"
        );
        let mut window = self::window(300, alt_drag);
        drag_holding(&mut window, from, to, Mods::ALT);
        assert_eq!(window.app().value, 50.0, "dropped, the press not run");
        let mut window = self::window(300, alt_drag);
        drag_holding(&mut window, from, from, Mods::ALT);
        assert_eq!(window.app().value, 7.0, "released where pressed, a click");
    }

    /// A slider storing its value in the probe's value.
    fn value_slider(probe: &Probe) -> Element<'_, Probe> {
        slider(RANGE, probe.value)
            .on_change(|probe: &mut Probe, value| probe.value = value)
            .into()
    }

    /// A menu button saying "File" storing the id chosen in the probe.
    fn file_menu(_probe: &Probe) -> Element<'_, Probe> {
        let entries = vec![
            MenuEntry::from(MenuItem::new(1, "New")),
            MenuEntry::from(MenuItem::new(2, "Open")),
        ];
        menu("File", entries)
            .on_choose(|probe: &mut Probe, id| probe.chosen = Some(id))
            .into()
    }

    /// A left press, then its release, at `point`.
    fn click_at(window: &mut TreeWindow<Probe>, point: Point) {
        window.send(&Input::PointerMoved(point));
        let press = ButtonInput::press(PointerButton::Left, point);
        window.send(&Input::PointerButton(press));
        let release = ButtonInput::release(PointerButton::Left, point);
        window.send(&Input::PointerButton(release));
    }

    /// The line of `window`'s description that has the focus.
    fn focused(window: &TreeWindow<Probe>) -> String {
        let text = window.ui_text();
        let line = text.lines().find(|line| line.ends_with(" focused"));
        line.map_or_else(
            || format!("nothing in {text}"),
            |line| line.trim().to_owned(),
        )
    }

    /// A text field storing its text in the probe's name.
    fn name_field(probe: &Probe) -> Element<'_, Probe> {
        text_field(&probe.name)
            .on_change(|probe: &mut Probe, name| probe.name = name)
            .into()
    }

    /// A field and a line of text, each asking for a context menu (the
    /// column around them notes in the probe's `extra` that it was the one
    /// asked), and the menu open while the probe keeps a request.
    fn noted(probe: &Probe) -> Element<'_, Probe> {
        let entries = vec![
            MenuEntry::from(MenuItem::new(1, "Cut")),
            MenuEntry::from(MenuItem::new(2, "Copy")),
        ];
        let at = probe.menu.unwrap_or(MenuRequest::at(Point::default()));
        let menu = context_menu(entries, at)
            .on_choose(|probe: &mut Probe, id| {
                probe.chosen = Some(id);
                probe.menu = None;
            })
            .on_close(|probe: &mut Probe| probe.menu = None);
        column([
            name_field(probe).on_context_menu(|probe: &mut Probe, at| probe.menu = Some(at)),
            text("plain").into(),
        ])
        .on_context_menu(|probe: &mut Probe, at| {
            probe.menu = Some(at);
            probe.extra = true;
        })
        .overlay_if(probe.menu.is_some(), menu)
    }

    /// A press and release of the right button at `point`.
    fn right_click_at(window: &mut TreeWindow<Probe>, point: Point) {
        window.click_at(point, PointerButton::Right);
    }

    #[test]
    fn test_a_right_click_opens_the_innermost_context_menu_where_it_was_asked() {
        let mut window = window(200, noted);
        let field = window.center(Role::TextInput, "");
        right_click_at(&mut window, field);
        assert_eq!(window.app().menu, Some(MenuRequest::at(field)));
        assert!(
            !window.app().extra,
            "the field, inside the column, was asked"
        );
        let text = window.ui_text();
        assert!(text.contains("MenuItem \"Cut\""), "{text}");
        assert!(!text.contains("MenuItem \"Cut\" focused"), "{text}");
        assert_eq!(window.store().traps.len(), 1, "the open menu traps");
        window.press("<down> <down> RET");
        assert_eq!(window.app().chosen, Some(2));
        assert_eq!(window.app().menu, None);
        assert!(window.store().traps.is_empty());
        assert!(
            focused(&window).starts_with("TextInput"),
            "{}",
            window.ui_text()
        );
    }

    #[test]
    fn test_a_right_click_outside_inner_elements_asks_the_one_around_them() {
        let mut window = window(200, noted);
        right_click_at(&mut window, Point::new(390.0, 190.0));
        assert!(window.app().extra);
        assert!(window.app().menu.is_some());
    }

    #[test]
    fn test_esc_and_a_click_outside_close_the_context_menu_and_give_the_focus_back() {
        let mut window = window(200, noted);
        let field = window.center(Role::TextInput, "");
        right_click_at(&mut window, field);
        window.press("<esc>");
        assert_eq!(window.app().menu, None);
        assert!(window.store().traps.is_empty());
        right_click_at(&mut window, field);
        click_at(&mut window, Point::new(390.0, 190.0));
        assert_eq!(window.app().menu, None);
        assert_eq!(window.app().chosen, None);
        window.type_text("x");
        assert_eq!(window.app().name, "x", "the field has the keyboard again");
    }

    #[test]
    fn test_shift_f10_opens_the_focused_controls_menu_its_first_item_highlighted() {
        let mut window = window(200, noted);
        window.press("S-<f10>");
        let request = window.app().menu.expect("the menu was asked for");
        assert!(request.by_key);
        assert!(!window.app().extra);
        let text = window.ui_text();
        assert!(text.contains("MenuItem \"Cut\" focused"), "{text}");
        assert!(window.app().keys.is_empty(), "the key was taken");
        window.press("RET");
        assert_eq!(window.app().chosen, Some(1));
    }

    #[test]
    fn test_no_context_menu_is_asked_for_under_a_modal_layer() {
        let mut window = window(200, |probe| {
            let layer = column([Element::from(checkbox("A", false)).id("a")]);
            column([text("under").into()])
                .on_context_menu(|probe: &mut Probe, at| probe.menu = Some(at))
                .overlay_if(probe.extra, overlay(layer).modal())
        });
        window.update(|probe| probe.extra = true);
        right_click_at(&mut window, Point::new(390.0, 190.0));
        assert_eq!(window.app().menu, None);
    }

    /// Hints of one entry for the probe's prefix, shown at once, the
    /// entry clicked noted as the probe's chosen id; or under a header
    /// when the probe says `extra`.
    fn hints(probe: &Probe) -> Element<'_, Probe> {
        let entries = vec![WhichKeyEntry::new(
            Key::new(KeyCode::Char('s'), Mods::NONE),
            "Save the file",
            false,
        )];
        let hints = which_key(&probe.prefix, entries)
            .delay(Duration::ZERO)
            .on_activate(|probe: &mut Probe, index| probe.chosen = Some(index as u8))
            .on_close(|probe: &mut Probe| probe.prefix.clear());
        let hints = match probe.extra {
            true => hints.header("File"),
            false => hints,
        };
        column([text("under").into()]).overlay(hints)
    }

    /// Whether `window` draws the hints' entry.
    fn shows_hints(window: &TreeWindow<Probe>) -> bool {
        window
            .drawn_text()
            .iter()
            .any(|line| line.contains("Save the file"))
    }

    #[test]
    fn test_which_key_hints_show_once_the_prefix_waits_and_hide_with_it() {
        let mut window = window(300, hints);
        assert!(!shows_hints(&window));
        window.update(|probe| probe.prefix = vec![Key::new(KeyCode::Char('g'), Mods::NONE)]);
        assert!(!shows_hints(&window), "not before the delay has passed");
        window.about_to_wait();
        assert!(shows_hints(&window), "{:?}", window.drawn_text());
        window.press("x");
        assert_eq!(window.app().keys.len(), 1, "keys go on to the app");
        window.update(|probe| probe.prefix.clear());
        assert!(!shows_hints(&window));
    }

    #[test]
    fn test_hints_under_a_header_show_at_once_and_a_click_outside_closes_them() {
        let mut window = window(300, hints);
        window.update(|probe| {
            probe.extra = true;
            probe.prefix = vec![Key::new(KeyCode::Char('g'), Mods::NONE)];
        });
        assert!(shows_hints(&window));
        window.update(|probe| probe.extra = false);
        window.about_to_wait();
        click_at(&mut window, Point::new(5.0, 5.0));
        assert!(window.app().prefix.is_empty(), "on_close ran");
        assert!(!shows_hints(&window));
    }

    #[test]
    fn test_an_update_sent_from_another_thread_runs_once_delivered_and_redraws() {
        let mut window = window(200, |probe| column([value_slider(probe)]));
        let waker = window.waker();
        std::thread::spawn(move || waker.send(Box::new(|probe: &mut Probe| probe.value = 7.0)))
            .join()
            .expect("the sending thread finished")
            .expect("the window was open");
        assert_eq!(
            window.app().value,
            0.0,
            "nothing runs before it is delivered"
        );
        assert_eq!(window.deliver(), 1);
        assert_eq!(window.app().value, 7.0);
        assert!(window.ui_text().contains("value=7"), "{}", window.ui_text());
    }

    #[test]
    #[should_panic(expected = "two elements have the id \"same\" (column/0/slider and \
                               column/1/slider)")]
    fn test_two_elements_with_one_id_panic_naming_both() {
        window(200, |probe| {
            column([
                Element::from(slider(RANGE, probe.value)).id("same"),
                Element::from(slider(RANGE, probe.value)).id("same"),
            ])
        });
    }

    #[test]
    #[should_panic(expected = "the column at column went from 1 to 2 elements, and the \
                               checkbox at column/0/checkbox inside it has no id of its own")]
    fn test_a_control_without_an_id_in_a_changing_container_panics() {
        let mut window = window(200, |probe| {
            let mut children = vec![checkbox("Wrap", false).into()];
            if probe.extra {
                children.push(text("more").into());
            }
            column(children)
        });
        window.update(|probe| probe.extra = true);
    }

    #[test]
    fn test_an_explicit_id_keeps_a_controls_state_when_elements_come_before_it() {
        let mut window = window(200, |probe| {
            let mut children = Vec::new();
            if probe.extra {
                children.push(Element::from(checkbox("Extra", false)).id("extra"));
            }
            children.push(name_field(probe).id("name"));
            column(children)
        });
        window.type_text("ab");
        window.press("<left>");
        window.update(|probe| probe.extra = true);
        assert!(window.ui_text().contains("CheckBox \"Extra\""));
        window.type_text("X");
        assert_eq!(window.app().name, "aXb", "the caret and focus stayed");
    }

    #[test]
    fn test_a_widget_is_dropped_once_its_element_is_not_built() {
        let mut window = window(200, |probe| match probe.extra {
            true => column([text("only text").into()]).id("root"),
            false => column([name_field(probe).id("name")]).id("root"),
        });
        assert_eq!(window.store().entries.len(), 1);
        window.update(|probe| probe.extra = true);
        assert!(window.store().entries.is_empty());
    }

    #[test]
    fn test_a_control_without_a_callback_is_read_only_and_says_so() {
        let mut window = window(200, |probe| {
            form([field("Level", slider(RANGE, probe.value))])
        });
        assert!(
            window
                .ui_text()
                .contains("Slider \"Level\" value=\"0\" value=0 disabled")
        );
        window.press("<right>");
        assert_eq!(window.app().value, 0.0);
    }

    #[test]
    fn test_keys_no_control_takes_reach_the_app_and_used_ones_do_not() {
        let mut window = window(200, |probe| {
            column([slider(RANGE, probe.value)
                .on_change(|probe: &mut Probe, value| probe.value = value)
                .into()])
        });
        window.press("<right> C-x");
        assert_eq!(window.app().value, 1.0);
        assert_eq!(
            window.app().keys,
            [Key::new(KeyCode::Char('x'), Mods::CTRL)]
        );
    }

    #[test]
    fn test_tab_scrolls_the_control_it_focuses_into_view() {
        let mut window = window(120, |probe| {
            let fields = (0..12).map(|_| {
                field(
                    "Value",
                    slider(RANGE, probe.value).on_change(|probe: &mut Probe, v| probe.value = v),
                )
            });
            column([form(fields).scroll()]).id("root")
        });
        window.window().set_reduced_motion(true);
        window.press("S-<tab>");
        window.press("<right>");
        let scroll =
            window.store().entries.values_mut().find_map(|entry| {
                downcast_scroll(&mut *entry.widget).map(|scroll| scroll.offset().y)
            });
        assert!(scroll.is_some_and(|offset| offset > 0.0), "{scroll:?}");
    }

    #[test]
    fn test_an_element_kept_in_view_is_scrolled_to_each_time_another_takes_the_mark() {
        let mut window = window(120, |probe| {
            let at = probe.value as usize;
            let rows = (0..40).map(move |row| {
                let line = text(format!("row {row}")).into();
                match row == at {
                    true => column([line]).keep_in_view(),
                    false => column([line]),
                }
            });
            column([column(rows).scroll()]).id("root")
        });
        window.window().set_reduced_motion(true);
        window.update(|probe| probe.value = 30.0);
        // The frame that moved the mark scrolls; the next one shows it.
        window.update(|_| {});
        let down = scroll_offsets(&mut window);
        assert!(down.iter().any(|offset| *offset > 0.0), "{down:?}");
        window.update(|probe| probe.value = 0.0);
        window.update(|_| {});
        assert_eq!(scroll_offsets(&mut window), [0.0], "back to the top");
    }

    /// The scroll offsets of every scrolling container `window` keeps.
    fn scroll_offsets(window: &mut TreeWindow<Probe>) -> Vec<f32> {
        window
            .store()
            .entries
            .values_mut()
            .filter_map(|entry| downcast_scroll(&mut *entry.widget).map(|scroll| scroll.offset().y))
            .collect()
    }

    #[test]
    fn test_a_modal_layer_scrolls_a_form_taller_than_the_window() {
        let mut window = window(120, |probe| {
            let fields = (0..12).map(|_| {
                field(
                    "Value",
                    slider(RANGE, probe.value).on_change(|probe: &mut Probe, v| probe.value = v),
                )
            });
            column([text("under").into()])
                .overlay(overlay(column([form(fields).scroll()]).padding(2.0, 1.0)).modal())
        });
        window.window().set_reduced_motion(true);
        assert!(
            focused(&window).starts_with("Slider \"Value\""),
            "{}",
            window.ui_text()
        );
        window.press("S-<tab> <right>");
        assert_eq!(
            window.app().value,
            1.0,
            "the last slider in the layer took the key"
        );
        let offsets = scroll_offsets(&mut window);
        assert!(offsets.iter().any(|offset| *offset > 0.0), "{offsets:?}");
        let text = window.ui_text();
        assert!(text.contains("ScrollView"), "{text}");
    }

    #[test]
    fn test_a_layer_that_is_itself_a_scrolling_container_scrolls_and_traps() {
        let mut window = window(120, |probe| {
            let fields = (0..12).map(|_| {
                field(
                    "Value",
                    slider(RANGE, probe.value).on_change(|probe: &mut Probe, v| probe.value = v),
                )
            });
            column([name_field(probe)]).overlay(overlay(form(fields).scroll()).modal())
        });
        window.window().set_reduced_motion(true);
        window.press("S-<tab> <right>");
        assert_eq!(window.app().value, 1.0);
        let offsets = scroll_offsets(&mut window);
        assert!(offsets.iter().any(|offset| *offset > 0.0), "{offsets:?}");
        window.type_text("x");
        assert_eq!(
            window.app().name,
            "",
            "keys never reach the field under the layer"
        );
    }

    #[test]
    fn test_a_focused_caret_and_a_sweep_animate_and_still_text_rests() {
        let mut window = window(200, |probe| column([name_field(probe)]));
        window.press("<tab>");
        assert!(window.about_to_wait().is_some(), "the caret blinks");
        let mut idle = self::window(200, |_| column([text("still").into()]));
        assert_eq!(idle.about_to_wait(), None);
        let mut sweeping = self::window(200, |_| column([progress(None).into()]));
        assert!(sweeping.about_to_wait().is_some(), "a sweep animates");
    }

    #[test]
    fn test_a_modal_overlay_opens_a_trap_as_it_appears_and_closes_it_as_it_goes() {
        let mut window = window(200, |probe| {
            let layer = column([
                Element::from(checkbox("A", false)).id("a"),
                Element::from(checkbox("B", false)).id("b"),
            ]);
            column([value_slider(probe)]).overlay_if(probe.extra, overlay(layer).modal())
        });
        assert!(focused(&window).starts_with("Slider"));
        window.update(|probe| probe.extra = true);
        assert_eq!(window.store().traps.len(), 1);
        assert_eq!(
            focused(&window),
            "CheckBox \"A\" unchecked disabled focused"
        );
        window.press("<tab>");
        assert_eq!(
            focused(&window),
            "CheckBox \"B\" unchecked disabled focused"
        );
        window.press("<tab> <right> C-x");
        assert_eq!(
            focused(&window),
            "CheckBox \"A\" unchecked disabled focused",
            "Tab cycles"
        );
        assert_eq!(window.app().value, 0.0, "no key reaches the slider");
        assert!(window.app().keys.is_empty(), "nor the app");
        window.update(|probe| probe.extra = false);
        assert!(window.store().traps.is_empty());
        assert!(focused(&window).starts_with("Slider"), "the focus is back");
    }

    #[test]
    fn test_a_modal_overlay_handing_keys_on_gives_the_app_those_its_elements_leave() {
        let mut window = window(200, |probe| {
            let layer = column([
                Element::from(checkbox("A", false)).id("a"),
                Element::from(checkbox("B", false)).id("b"),
            ]);
            column([value_slider(probe)])
                .overlay_if(probe.extra, overlay(layer).modal().keys_to_app())
        });
        window.update(|probe| probe.extra = true);
        window.press("<tab> <esc> C-x <right>");
        assert_eq!(
            focused(&window),
            "CheckBox \"B\" unchecked disabled focused",
            "Tab still cycles inside"
        );
        let keys: Vec<String> = window.app().keys.iter().map(ToString::to_string).collect();
        assert_eq!(
            keys,
            ["<esc>", "C-x", "<right>"],
            "the app gets what is left"
        );
        assert_eq!(window.app().value, 0.0, "the slider under it gets nothing");
        assert_eq!(window.store().traps.len(), 1, "the trap holds");
    }

    /// A lookup dialog asking for a word in the probe's name, while the
    /// probe shows its extra element.
    fn lookup(probe: &Probe) -> Element<'_, Probe> {
        let body = vec![Paragraph::plain(
            "Which word?",
            ThemeToken::Foreground.into(),
        )];
        let asking = dialog("Look up", body)
            .field(&probe.name, |probe: &mut Probe, name| probe.name = name)
            .field_placeholder("A word")
            .button("Cancel", |probe: &mut Probe| probe.extra = false)
            .primary("Look up", |probe: &mut Probe| probe.chosen = Some(1))
            .on_close(|probe: &mut Probe| probe.extra = false);
        column([value_slider(probe)]).overlay_if(probe.extra, asking)
    }

    #[test]
    fn test_a_dialogs_text_field_takes_a_word_and_enter_chooses_the_primary_button() {
        let mut window = window(200, lookup);
        window.update(|probe| probe.extra = true);
        window.type_text("word");
        assert_eq!(window.app().name, "word");
        assert!(
            window.ime_area().is_some(),
            "the input method follows the field"
        );
        assert!(
            window.ui_text().contains("\"word\""),
            "{}",
            window.ui_text()
        );
        window.press("RET");
        assert_eq!(window.app().chosen, Some(1), "Enter chose Look up");
        window.press("<tab> x");
        assert_eq!(window.app().name, "word", "the keyboard is on a button now");
        window.press("<esc>");
        assert!(!window.app().extra, "Esc closes it");
        assert!(window.app().keys.is_empty(), "no key reached the app");
    }

    #[test]
    fn test_a_dialogs_button_runs_on_its_letter_and_its_severity_is_drawn() {
        let mut window = window(200, |probe| {
            let body = vec![Paragraph::plain("Discard?", ThemeToken::Foreground.into())];
            let asking = dialog("Unsaved changes", body)
                .severity(Severity::Warning)
                .button("Keep", |probe: &mut Probe| probe.chosen = Some(0))
                .key('k')
                .primary("Discard", |probe: &mut Probe| probe.chosen = Some(1))
                .key('d');
            column([value_slider(probe)]).overlay_if(probe.extra, asking)
        });
        window.update(|probe| probe.extra = true);
        assert!(
            window
                .drawn_text()
                .iter()
                .any(|text| text == Severity::Warning.glyph()),
            "the warning sign before the title: {:?}",
            window.drawn_text()
        );
        window.press("k");
        assert_eq!(window.app().chosen, Some(0), "k chose Keep");
        window.press("d");
        assert_eq!(window.app().chosen, Some(1), "d chose Discard");
    }

    #[test]
    fn test_a_masked_text_field_hands_the_app_its_text_but_shows_and_names_only_dots() {
        let mut window = window(200, |probe| {
            column([text_field(&probe.name)
                .masked()
                .on_change(|probe: &mut Probe, name| probe.name = name)
                .into()])
        });
        window.type_text("hunter2");
        assert_eq!(window.app().name, "hunter2");
        let text = window.ui_text();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("PasswordInput"), "{text}");
        assert!(
            window
                .drawn_text()
                .iter()
                .all(|shown| !shown.contains("hunter")),
            "{:?}",
            window.drawn_text()
        );
    }

    #[test]
    fn test_a_form_sets_its_labels_as_chrome_only_when_asked_and_sizes_the_column_to_them() {
        let plain = window(200, |probe| {
            column([form([field("Name", name_field(probe))])])
        });
        let chrome = window(200, |probe| {
            column([form([field("Name", name_field(probe))]).chrome_labels()])
        });
        assert!(
            !shaped(&plain).iter().any(|text| text == "Name"),
            "on the grid"
        );
        assert!(
            shaped(&chrome).iter().any(|text| text == "Name"),
            "as chrome"
        );
        let field_x = |window: &TreeWindow<Probe>| {
            let tree = access_tree(window.frame(), "t");
            let (_, input) = tree
                .nodes
                .iter()
                .find(|(_, node)| node.role() == Role::TextInput)
                .expect("the field is described");
            input.bounds().expect("bounds").x0
        };
        let metrics = plain.frame().cell_metrics();
        assert_eq!(
            field_x(&plain),
            f64::from((4.0 + LABEL_GAP) * metrics.width),
            "four cells and the gap"
        );
        assert_ne!(field_x(&chrome), field_x(&plain), "measured in the UI font");
    }

    #[test]
    fn test_a_menu_opens_under_its_button_chooses_and_gives_the_focus_back() {
        let mut window = window(200, file_menu);
        window.press("RET");
        let text = window.ui_text();
        assert!(text.contains("Menu"), "{text}");
        assert!(text.contains("MenuItem \"New\" focused"), "{text}");
        assert_eq!(window.store().traps.len(), 1);
        window.press("<tab> <down> RET");
        assert_eq!(window.app().chosen, Some(2));
        assert!(window.store().traps.is_empty());
        assert_eq!(focused(&window), "Button \"File\" focused");
    }

    #[test]
    fn test_esc_and_a_click_outside_close_the_menu_choosing_nothing() {
        let mut window = window(200, file_menu);
        window.press("<space> <esc>");
        assert!(!window.ui_text().contains("Menu"));
        assert_eq!(focused(&window), "Button \"File\" focused");
        window.press("<space>");
        click_at(&mut window, Point::new(390.0, 190.0));
        assert!(!window.ui_text().contains("Menu"), "{}", window.ui_text());
        assert_eq!(window.app().chosen, None);
        assert!(window.store().traps.is_empty());
    }

    #[test]
    fn test_a_screen_reader_opens_the_menu_and_clicks_an_item() {
        let mut window = window(200, file_menu);
        let file = window.find(Role::Button, "File");
        window.act(AccessAction::Click(file));
        let open = window.find(Role::MenuItem, "Open");
        window.act(AccessAction::Click(open));
        assert_eq!(window.app().chosen, Some(2));
    }

    /// Two panes side by side: `left` holding checkboxes A and B, `right`
    /// holding C.
    fn panes(_probe: &Probe) -> Element<'_, Probe> {
        row([
            column([checkbox("A", false).into(), checkbox("B", false).into()]).id("left"),
            column([checkbox("C", false).into()]).id("right"),
        ])
    }

    #[test]
    fn test_the_app_moves_the_focus_to_a_pane_by_its_id() {
        let mut window = window(200, panes);
        assert!(focused(&window).contains("\"A\""), "{}", focused(&window));
        window.press("M-l");
        assert!(focused(&window).contains("\"C\""), "{}", focused(&window));
        window.press("M-h");
        assert!(
            focused(&window).contains("\"A\""),
            "the first stop inside the pane: {}",
            focused(&window)
        );
        window.press("<tab> M-h");
        assert!(
            focused(&window).contains("\"B\""),
            "a pane that has the keyboard keeps it where it is: {}",
            focused(&window)
        );
        window.press("M-x");
        assert!(focused(&window).contains("\"B\""), "no such element");
    }

    /// A split whose sides grow, as a pane in a column would: a canvas
    /// over a button on the left, a canvas alone on the right.
    fn growing_sides(_probe: &Probe) -> Element<'_, Probe> {
        let left = column([
            Element::from(canvas(|_, _, _| {}).name("Left")).grow(1.0),
            button("Below").on_click(|_: &mut Probe| {}).into(),
        ])
        .grow(1.0);
        let right = Element::from(canvas(|_, _, _| {}).name("Right")).grow(1.0);
        split(Axis::Horizontal, 0.5, left, right).into()
    }

    #[test]
    fn test_a_growing_split_side_fills_the_side_and_its_canvas_the_room_left() {
        let window = window(200, growing_sides);
        let height = |name: &str| {
            let tree = access_tree(window.frame(), "t");
            let (_, node) = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(name))
                .unwrap_or_else(|| panic!("no {name}"));
            let rect = node.bounds().expect("described with bounds");
            rect.y1 - rect.y0
        };
        assert_eq!(height("Right"), 200.0);
        let below = height("Below");
        assert!(below > 0.0);
        assert_eq!(height("Left"), 200.0 - below);
    }

    /// Two panes split side by side: the left one's toolbar drawn above
    /// its body but ordered after it, the right one a button.
    fn ordered_panes(probe: &Probe) -> Element<'_, Probe> {
        let left = column([
            row([
                button("Zoom")
                    .on_click(|probe: &mut Probe| probe.chosen = Some(1))
                    .into(),
                button("Fit").on_click(|_: &mut Probe| {}).into(),
            ])
            .order(1),
            button("Body").on_click(|_: &mut Probe| {}).into(),
        ]);
        let right = column([button("Side").on_click(|_: &mut Probe| {}).into()]);
        column([
            split(Axis::Horizontal, 0.5, left, right).into(),
            value_slider(probe),
        ])
    }

    /// The names of the controls Tab visits from the first, in order,
    /// `count` of them.
    fn tab_order(window: &mut TreeWindow<Probe>, count: usize) -> Vec<String> {
        (0..count)
            .map(|_| {
                let line = focused(window);
                window.press("<tab>");
                line
            })
            .collect()
    }

    #[test]
    fn test_an_ordered_container_is_one_block_in_the_tab_order_among_its_siblings() {
        let mut window = window(200, ordered_panes);
        let order = tab_order(&mut window, 6);
        let names: Vec<&str> = ["Body", "Zoom", "Fit", "Side", "Slider", "Body"].to_vec();
        for (line, name) in order.iter().zip(names) {
            assert!(line.contains(name), "{name} expected: {order:?}");
        }
        // The pointer reaches the controls of a block as before.
        window.click(Role::Button, "Zoom");
        assert_eq!(window.app().chosen, Some(1));
    }

    /// Two panes side by side, each taking the keyboard only through
    /// pressable regions: `left` is itself one, `right` holds two.
    fn pressable_panes(_probe: &Probe) -> Element<'_, Probe> {
        row([
            Element::from(checkbox("A", false)),
            column([text("L").into()])
                .on_press(|_: &mut Probe, _| {})
                .access_name("L")
                .id("left"),
            column([
                column([text("R1").into()])
                    .on_press(|_: &mut Probe, _| {})
                    .access_name("R1"),
                column([text("R2").into()])
                    .on_press(|_: &mut Probe, _| {})
                    .access_name("R2"),
            ])
            .id("right"),
        ])
    }

    #[test]
    fn test_the_app_moves_the_focus_to_an_element_whose_only_stop_is_its_press_region() {
        let mut window = window(200, pressable_panes);
        window.press("M-h");
        assert!(focused(&window).contains("\"L\""), "{}", focused(&window));
    }

    #[test]
    fn test_the_app_moves_the_focus_to_the_first_press_region_inside_a_container() {
        let mut window = window(200, pressable_panes);
        window.press("M-l");
        assert!(focused(&window).contains("\"R1\""), "{}", focused(&window));
        window.press("<tab> M-l");
        assert!(
            focused(&window).contains("\"R2\""),
            "a pane that has the keyboard in a region keeps it there: {}",
            focused(&window)
        );
    }

    #[test]
    fn test_the_app_moves_the_focus_to_a_tab_strip_by_its_id() {
        let mut window = window(200, |_| {
            let tabs = [tab(1, "one"), tab(2, "two")];
            column([
                Element::from(checkbox("A", false)).id("right"),
                Element::from(tab_strip(tabs, Some(&1))).id("left"),
            ])
        });
        window.press("M-h");
        assert!(focused(&window).contains("TabList"), "{}", focused(&window));
    }

    /// A wide proportional line, a narrow one, and a button after each.
    fn prose(_probe: &Probe) -> Element<'_, Probe> {
        column([
            row([text("WWW mmm").proportional().into(), button("A").into()]),
            row([text("iiiiiiii").proportional().into(), button("B").into()]),
            row([text("iiiiiiii").into(), button("C").into()]),
        ])
    }

    /// The lines `window` drew in the proportional font.
    fn shaped(window: &TreeWindow<Probe>) -> Vec<String> {
        let frame = window.frame();
        frame
            .draw
            .text()
            .iter()
            .filter(|segment| segment.wrap.is_some())
            .map(|segment| segment.runs.iter().map(|run| run.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn test_a_proportional_text_is_shaped_whole_in_the_width_its_glyphs_take() {
        let window = window(200, prose);
        let lines = shaped(&window);
        for line in ["WWW mmm", "iiiiiiii"] {
            assert!(
                lines.iter().any(|shown| shown == line),
                "{line} whole in {lines:?}"
            );
        }
        let (a, b, c) = (
            window.center(Role::Button, "A"),
            window.center(Role::Button, "B"),
            window.center(Role::Button, "C"),
        );
        assert!(
            b.x < c.x,
            "narrow glyphs take less than their cells: {b:?} {c:?}"
        );
        assert!(a.x > b.x, "wide glyphs take more: {a:?} {b:?}");
    }

    #[test]
    fn test_a_proportional_text_too_long_for_its_column_ends_in_an_ellipsis() {
        let window = window(200, |_| {
            column([text("word ".repeat(80)).proportional().into()])
        });
        let lines = shaped(&window);
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("word") && line.ends_with('\u{2026}')),
            "{lines:?}"
        );
    }

    #[test]
    fn test_a_tooltip_repeating_what_its_element_shows_is_left_out() {
        let mut window = window(200, |_| {
            column([
                Element::from(text("Save")).tooltip("Save"),
                row([text("Open").into(), text("a file").into()]).tooltip(" Open  a file "),
                Element::from(text("Save")).tooltip("Save the file"),
            ])
        });
        let tips: Vec<String> = window
            .store()
            .tips
            .iter()
            .map(|(_, help)| help.clone())
            .collect();
        assert_eq!(tips, ["Save the file"], "only the one that adds something");
    }

    #[test]
    fn test_text_cut_short_says_it_whole_unless_what_holds_it_has_a_tooltip() {
        let long = "word ".repeat(80);
        let mut window = window(200, |_| {
            column([
                text("short").proportional().into(),
                text("word ".repeat(80)).proportional().into(),
                column([text("row ".repeat(80)).proportional().into()]).tooltip("the row's own"),
            ])
        });
        let tips: Vec<String> = window
            .store()
            .tips
            .iter()
            .map(|(_, help)| help.clone())
            .collect();
        assert!(tips.contains(&long), "the cut text whole: {tips:?}");
        assert!(
            !tips.iter().any(|help| help == "short"),
            "nothing cut, no tooltip"
        );
        assert!(
            !tips.iter().any(|help| help.starts_with("row ")),
            "the row's tooltip speaks for it: {tips:?}"
        );
    }

    #[test]
    fn test_a_tooltip_shows_for_the_element_the_pointer_rests_on() {
        let mut window = window(200, |_| {
            column([Element::from(button("Save")).tooltip("Save the file")])
        });
        window.store().tooltip = Tooltip::new().with_delay(Duration::ZERO);
        let (anchor, _) = window.store().tips[0].clone();
        let center = Point::new(
            anchor.x + anchor.width / 2.0,
            anchor.y + anchor.height / 2.0,
        );
        window.send(&Input::PointerMoved(center));
        let text = window.ui_text();
        assert!(text.contains("Tooltip \"Save the file\""), "{text}");
        window.send(&Input::PointerMoved(Point::new(390.0, 190.0)));
        assert!(!window.ui_text().contains("Tooltip"));
    }

    #[test]
    fn test_an_icon_button_is_named_and_tipped_by_its_name_and_a_button_takes_an_icon() {
        use std::sync::LazyLock;

        use crate::render::image::Svg;

        static ICON: LazyLock<Svg> = LazyLock::new(|| {
            Svg::parse(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>"#)
                .expect("a valid svg")
        });
        let mut window = window(200, |_| {
            row([
                Element::from(
                    icon_button(&ICON, "Back").on_click(|probe: &mut Probe| probe.opened = true),
                )
                .id("back"),
                button("Open").icon(&ICON).into(),
                button("Open").into(),
            ])
        });
        window.store().tooltip = Tooltip::new().with_delay(Duration::ZERO);
        let text = window.ui_text();
        assert!(text.contains("Button \"Back\""), "{text}");
        assert!(text.contains("Button \"Open\""), "{text}");
        let (back, help) = window.store().tips[0].clone();
        assert_eq!(help, "Back");
        assert_eq!(back.width, back.height, "an icon alone is square");
        window.send(&Input::PointerMoved(Point::new(
            back.x + back.width / 2.0,
            back.y + back.height / 2.0,
        )));
        assert!(window.ui_text().contains("Tooltip \"Back\""));
        window.click(Role::Button, "Back");
        assert!(window.app().opened);
        // The controls' rectangles, in reading order: Back, then Open with
        // its icon and without.
        let hits: Vec<f32> = window
            .store()
            .hits
            .iter()
            .map(|(_, rect)| rect.width)
            .collect();
        let width = |index: usize| hits[index];
        let metrics = window.frame().cell_metrics();
        let icon = metrics.height.round() + (0.5 * metrics.width).round();
        assert!(
            (width(1) - width(2) - icon).abs() <= 1.0,
            "an icon widens a labelled button by its square and gap"
        );
    }

    #[test]
    fn test_a_ghost_button_is_flat_at_rest_and_filled_under_the_pointer() {
        use std::sync::LazyLock;

        use crate::render::image::Svg;
        use crate::widgets::button::ButtonVariant;

        static ICON: LazyLock<Svg> = LazyLock::new(|| {
            Svg::parse(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>"#)
                .expect("a valid svg")
        });
        let mut window = window(200, |_| {
            let ghost = ButtonVariant::Ghost;
            let click = |_: &mut Probe| {};
            row([
                button("Find").variant(ghost).on_click(click).into(),
                icon_button(&ICON, "Back")
                    .variant(ghost)
                    .on_click(click)
                    .into(),
                button("Open").on_click(click).into(),
            ])
        });
        let hits: Vec<Rect> = window.store().hits.iter().map(|(_, rect)| *rect).collect();
        let corner = |index: usize| (hits[index].x + 2.0, hits[index].y + 2.0);
        let background = window.app().theme.background;
        let Some(snapshot) = window.render() else {
            return;
        };
        let (x, y) = corner(0);
        snapshot.assert_pixel(x, y, background, "a labelled ghost at rest");
        let (x, y) = corner(1);
        snapshot.assert_pixel(x, y, background, "an icon-only ghost at rest");
        let (x, y) = corner(2);
        assert_ne!(
            snapshot.pixel(x, y),
            background,
            "the default look is filled"
        );
        let rect = hits[0];
        window.send(&Input::PointerMoved(Point::new(
            rect.x + rect.width / 2.0,
            rect.y + rect.height / 2.0,
        )));
        // Past the hover's fade in.
        window.advance(Duration::from_secs(1));
        let snapshot = window.render().expect("drawn once already");
        let (x, y) = corner(0);
        assert_ne!(snapshot.pixel(x, y), background, "filled under the pointer");
    }

    #[test]
    fn test_buttons_in_a_row_pack_to_their_widths_and_one_that_grows_takes_the_rest() {
        let mut window = window(200, |_| {
            let click = |_: &mut Probe| {};
            row([
                button("Zoom in").on_click(click).into(),
                button("Fit").on_click(click).into(),
                Element::from(button("Wide").on_click(click)).grow(1.0),
                button("Close").on_click(click).into(),
            ])
        });
        let hits: Vec<Rect> = window.store().hits.iter().map(|(_, rect)| *rect).collect();
        let metrics = window.frame().cell_metrics();
        let gap = ROW_GAP * metrics.width;
        assert_eq!(hits[0].x, 0.0);
        assert_eq!(
            hits[1].x,
            hits[0].x + hits[0].width + gap,
            "packed against the first: {hits:?}"
        );
        assert_eq!(hits[2].x, hits[1].x + hits[1].width + gap, "{hits:?}");
        assert_eq!(
            hits[3].x + hits[3].width,
            400.0,
            "the growing one takes the room left, the last ending the row: {hits:?}"
        );
    }

    #[test]
    fn test_an_element_reports_the_size_it_was_laid_out_at_once_per_change() {
        use std::cell::Cell;

        thread_local! {
            /// How many times the tree was built.
            static BUILDS: Cell<usize> = const { Cell::new(0) };
        }
        let mut window = window(200, |probe| {
            BUILDS.with(|builds| builds.set(builds.get() + 1));
            column([
                text(format!("{}", probe.value)).into(),
                column([button("Fit").into()])
                    .on_size_change(|probe: &mut Probe, size| probe.value = f64::from(size.width)),
            ])
        });
        let builds = || BUILDS.with(Cell::get);
        assert_eq!(window.app().value, 0.0, "not before the frame is done");
        window.about_to_wait();
        assert_eq!(window.app().value, 400.0, "the column's width");
        assert!(
            window.drawn_text().iter().any(|line| line.contains("400")),
            "the next frame was built from it"
        );
        window.update(|probe| probe.value = -1.0);
        let before = builds();
        window.about_to_wait();
        assert_eq!(window.app().value, -1.0, "not reported again");
        assert_eq!(builds() - before, 1, "no rebuild to report nothing");
        window.resize(300, 200);
        let before = builds();
        window.about_to_wait();
        assert_eq!(window.app().value, 300.0, "reported once it changed");
        assert_eq!(builds() - before, 2, "one rebuild to report, one to draw");
        let size = ElementSize {
            height: 30.0,
            scale_factor: 2.0,
            width: 300.0,
        };
        assert_eq!((size.logical_width(), size.logical_height()), (150.0, 15.0));
    }

    #[test]
    fn test_a_title_bar_button_shows_its_own_tooltip() {
        let mut window = window(200, |_| {
            let button = TitleBarButton::label("Left").with_tooltip("Show the left sidebar");
            column([title_bar("t").app_button(button, |_: &mut Probe| {}).into()])
        });
        window.store().tooltip = Tooltip::new().with_delay(Duration::ZERO);
        let center = window.center(Role::Button, "Left");
        window.send(&Input::PointerMoved(center));
        let text = window.ui_text();
        assert!(text.contains("Tooltip \"Show the left sidebar\""), "{text}");
    }

    /// A tab whose title the narrow strip cuts short, beside a short one.
    const LONG_TAB: &str = "A tab title far too long for the strip it sits in";

    fn long_tab(_probe: &Probe) -> Element<'_, Probe> {
        let tabs = [tab(1_u8, LONG_TAB), tab(2, "Two")];
        column([tab_strip::<Probe, u8>(tabs, Some(&1)).into()])
    }

    #[test]
    fn test_a_tab_whose_title_is_cut_says_it_whole_in_a_tooltip() {
        let mut window = window(160, long_tab);
        window.store().tooltip = Tooltip::new().with_delay(Duration::ZERO);
        let center = window.center(Role::Tab, LONG_TAB);
        window.send(&Input::PointerMoved(center));
        let text = window.ui_text();
        assert!(text.contains(&format!("Tooltip \"{LONG_TAB}\"")), "{text}");
    }

    #[test]
    fn test_no_tooltip_shows_for_an_element_under_a_modal_layer() {
        let mut window = window(200, |probe| {
            let layer = column([Element::from(checkbox("A", false)).id("a")]);
            column([Element::from(button("Save")).tooltip("Save the file")])
                .overlay_if(probe.extra, overlay(layer).modal())
        });
        window.store().tooltip = Tooltip::new().with_delay(Duration::ZERO);
        let (anchor, _) = window.store().tips[0].clone();
        window.update(|probe| probe.extra = true);
        window.send(&Input::PointerMoved(Point::new(
            anchor.x + 1.0,
            anchor.y + 1.0,
        )));
        assert!(
            !window.ui_text().contains("Tooltip"),
            "{}",
            window.ui_text()
        );
    }

    #[test]
    fn test_the_toasts_hold_open_and_expire_on_the_apps_notifier() {
        let mut window = window(300, |probe| {
            column([text("under").into()]).overlay(
                toasts(&probe.notifier, |probe: &mut Probe| &mut probe.notifier)
                    .on_open(|probe: &mut Probe, _| probe.opened = true),
            )
        });
        window.update(|probe| probe.notifier.notify(Notice::error("disk full")));
        let card = window.store().entries.values_mut().find_map(|entry| {
            let widget: &mut dyn Any = &mut *entry.widget;
            widget
                .downcast_mut::<ToastCards>()
                .and_then(|cards| cards.cards.cards_rect())
        });
        let card = card.expect("the card was drawn");
        let at = Point::new(card.x + 4.0, card.y + card.height / 2.0);
        window.send(&Input::PointerMoved(at));
        assert_eq!(
            window.app().notifier.toasts().next_expiry(),
            None,
            "held under the pointer"
        );
        click_at(&mut window, at);
        assert!(window.app().opened);
        window.send(&Input::PointerLeft);
        let past = Instant::now() - Duration::from_secs(3600);
        window.update(|probe| {
            probe.notifier = Notifier::new();
            probe.notifier.post(&Notice::error("old"), past, 0);
        });
        window.about_to_wait();
        assert_eq!(window.app().notifier.toasts().iter().count(), 0, "expired");
    }
}

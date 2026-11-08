//! [`TreeView`]: a tree of one-line rows whose children the app loads
//! lazily, often on another thread, such as the folders of a file system.
//!
//! The app gives the top-level items with [`TreeView::set_roots`]. Each
//! item has an id of the app's choosing, stable across reloads, so the
//! expanded nodes and the cursor survive a refresh. Expanding a node
//! whose children the tree does not hold answers [`TreeViewResponse::Load`]
//! and shows a loading row under it; the app loads the children (a worker
//! thread sends them back through its [`Waker`](crate::Waker)) and hands
//! them over with [`TreeView::set_children`], or reports a failure with
//! [`TreeView::set_load_failed`]. [`TreeView::reload`] forgets a node's
//! children to load them again.
//!
//! Keys, through [`TreeView::handle`](crate::Widget::handle), are the ones every list-like
//! widget shares, as Vim moves over lines (see [`List`] for the whole
//! set): Up and Down (`k`, `j`, `C-p`, `C-n`) move, `C-f` and `C-b` page,
//! `C-d` and `C-u` move half a page, `C-e` and `C-y` scroll, `H`, `M`,
//! `L` go to the screen's rows, `g g` and `G` to the ends, `z z` and its
//! kin scroll the cursor's row, a count repeats a motion or names a row;
//! Right (`l`) expands a node or moves to its first child, Left (`h`)
//! collapses it or moves to its parent, Space toggles the row's
//! selection, Shift with Down, Up, Page Down, Page Up, Home, or End
//! extends it from its anchor, and Enter activates it. [`TreeView::keys`] tells what count
//! or prefix is pending. The pointer, through
//! [`TreeView::handle`](crate::Widget::handle): a click on a node's arrow expands or
//! collapses it, a click elsewhere moves the cursor (Ctrl+click toggles
//! the selection, Shift+click selects from the anchor to the row), a
//! double click activates, a drag from empty space below the rows (or
//! with Alt) draws a rubber band, and the wheel scrolls; rows glide to a
//! new scroll as a [`List`]'s do. As in Vim, a press of any button or the
//! wheel over the tree drops a pending count or prefix, as does the window
//! losing focus.
//!
//! An item shows its label unless made [`TreeItem::with_content`], with an
//! icon, styled text, and badges. [`TreeView::row_rect`],
//! [`TreeView::visible_range`], and [`TreeView::row_id`] tell where rows
//! were drawn and which item each shows.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::mem;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::Key;
use super::super::base::list::ListState;
use super::super::base::strings::{self, Strings};
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::Input;
use super::super::render::paragraph::TextAlign;
use super::list_keys::{CountedKey, ListKey, ListKeys, Read};
use super::list_view::{List, ListResponse, RowsToDraw};
use super::row_content::{RowContent, draw_content};

// ========================================================================
// Constants
// ========================================================================

/// The cells each level of depth indents a row.
pub(crate) const INDENT_CELLS: usize = 2;

/// The cells before the first level, past the cursor bar.
pub(crate) const PADDING_CELLS: usize = 1;

/// The cells the arrow and the space after it take.
pub(crate) const ARROW_CELLS: usize = 2;

/// The arrow of a collapsed node.
pub(crate) const COLLAPSED_ARROW: &str = "▸";

/// The arrow of an expanded node.
pub(crate) const EXPANDED_ARROW: &str = "▾";

/// The color of items' labels.
const LABEL_COLOR: TextColor = TextColor::Token(ThemeToken::Foreground);

/// The color of the arrows and the loading row.
const QUIET_COLOR: TextColor = TextColor::Token(ThemeToken::Muted);

/// The color of a failure row.
const FAILED_COLOR: TextColor = TextColor::Token(ThemeToken::Danger);

// ========================================================================
// Data Structures
// ========================================================================

/// A lazily loaded tree of items with ids of type `K`. It keeps the items
/// it was given, which nodes are expanded, the rows those make, and the
/// cursor, selection, and scroll over the rows.
#[derive(Clone, Debug)]
pub struct TreeView<K> {
    /// The width of one cell in the last frame, for the arrows' places.
    cell_width: f32,
    /// The children of each node the app handed over or is loading.
    children: HashMap<K, Children<K>>,
    /// The nodes expanded, whether their children are loaded or not.
    expanded: HashSet<K>,
    /// The rows: cursor, selection, scroll, glide, and clicks.
    list: List,
    /// An item the cursor was on before a reload took it away, and the row
    /// the cursor went to instead: while the cursor stays there, it goes
    /// back to the item once a reload brings it back.
    lost_cursor: Option<(K, usize)>,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// The top-level items.
    roots: Vec<TreeItem<K>>,
    /// The rows shown: the roots and the children of expanded nodes, depth
    /// first.
    rows: Vec<TreeRow<K>>,
    /// The words of the theme last drawn in, for loading and failed rows.
    strings: Strings,
}

/// One item of a [`TreeView`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeItem<K> {
    /// What its row shows in place of the label, when the app said.
    content: Option<RowContent>,
    /// Whether it may have children, and so can be expanded.
    has_children: bool,
    /// The app's id for it, stable across reloads.
    id: K,
    /// What its row says.
    label: String,
}

/// What became of a key or event offered to a [`TreeView`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TreeViewResponse<K> {
    /// This item was activated (Enter, a double click): open it.
    Activated(K),
    /// This node was collapsed.
    Collapsed(K),
    /// This node was expanded and its children are shown.
    Expanded(K),
    /// The tree used the key or event.
    Handled,
    /// Not the tree's.
    Ignored,
    /// This node was expanded and needs its children: load them and hand
    /// them over with [`TreeView::set_children`].
    Load(K),
}

/// The children of a node.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Children<K> {
    /// Loading them failed, for this reason.
    Failed(String),
    /// These, in order.
    Loaded(Vec<TreeItem<K>>),
    /// Being loaded by the app.
    Loading,
}

/// One row shown.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TreeRow<K> {
    /// How deep it is: 0 for a root.
    depth: usize,
    /// What it shows.
    kind: RowKind<K>,
    /// The node it is under, `None` for a root.
    parent: Option<K>,
}

/// What identifies a row across rebuilds: the item it shows, or the node
/// whose loading or failure row it is.
type RowKey<K> = (Option<K>, Option<K>);

/// What a row shows.
#[derive(Clone, Debug, Eq, PartialEq)]
enum RowKind<K> {
    /// That its parent's children failed to load, and why.
    Failed(String),
    /// An item.
    Item(TreeItem<K>),
    /// That its parent's children are being loaded.
    Loading,
}

// ========================================================================
// TreeItem Implementations
// ========================================================================

impl<K> TreeItem<K> {
    /// An item that has no children, such as a file.
    pub fn leaf(id: K, label: impl Into<String>) -> Self {
        Self {
            content: None,
            has_children: false,
            id,
            label: label.into(),
        }
    }

    /// An item that may have children, loaded when it is expanded, such
    /// as a folder.
    pub fn branch(id: K, label: impl Into<String>) -> Self {
        Self {
            content: None,
            has_children: true,
            id,
            label: label.into(),
        }
    }

    /// The app's id for it.
    pub fn id(&self) -> &K {
        &self.id
    }

    /// What its row says.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// This item showing `content` (an icon, styled text, badges) in its
    /// row instead of the plain label, which screen readers still read.
    pub fn with_content(self, content: RowContent) -> Self {
        Self {
            content: Some(content),
            ..self
        }
    }
}

// ========================================================================
// TreeView Implementations
// ========================================================================

impl<K: Clone + Eq + Hash> TreeView<K> {
    // --------------------------------------------------------------------
    // Items
    // --------------------------------------------------------------------

    /// An empty tree.
    pub fn new() -> Self {
        let mut list = List::new();
        list.turn_drag_off();
        Self {
            cell_width: 0.0,
            children: HashMap::new(),
            expanded: HashSet::new(),
            list,
            lost_cursor: None,
            pointer: None,
            roots: Vec::new(),
            rows: Vec::new(),
            strings: Strings::english(),
        }
    }

    /// Shows `roots` as the top-level items, keeping the nodes expanded,
    /// the children loaded, and the cursor on its item where the ids are
    /// still there.
    pub fn set_roots(&mut self, roots: Vec<TreeItem<K>>) {
        self.roots = roots;
        self.rebuild();
    }

    /// Hands over the children of node `id`, as loaded, keeping the nodes
    /// below expanded and the cursor on its item where the ids are still
    /// there. Children of a node not expanded are kept for when it is.
    pub fn set_children(&mut self, id: &K, children: Vec<TreeItem<K>>) {
        self.children.insert(id.clone(), Children::Loaded(children));
        self.rebuild();
    }

    /// Reports that loading the children of node `id` failed for `reason`,
    /// shown in a row under it until it is reloaded.
    pub fn set_load_failed(&mut self, id: &K, reason: impl Into<String>) {
        self.children
            .insert(id.clone(), Children::Failed(reason.into()));
        self.rebuild();
    }

    /// Forgets the children of node `id` (after the app saw them change).
    /// Whether they must be loaded again, since the node is expanded: then
    /// a loading row shows until [`TreeView::set_children`].
    pub fn reload(&mut self, id: &K) -> bool {
        if !self.expanded.contains(id) {
            self.children.remove(id);
            return false;
        }
        self.children.insert(id.clone(), Children::Loading);
        self.rebuild();
        true
    }

    /// Whether node `id` is expanded.
    pub fn is_expanded(&self, id: &K) -> bool {
        self.expanded.contains(id)
    }

    /// The id of the item at the cursor, or `None` on a loading row or in
    /// an empty tree.
    pub fn cursor(&self) -> Option<&K> {
        self.item_at(self.list.state().cursor()).map(TreeItem::id)
    }

    /// The ids of the selected items.
    pub fn selected(&self) -> Vec<&K> {
        self.list
            .state()
            .selected()
            .filter_map(|row| self.item_at(row).map(TreeItem::id))
            .collect()
    }

    /// The cursor, selection, and scroll over the rows shown.
    pub fn state(&self) -> &ListState {
        self.list.state()
    }

    /// The cursor, selection, and scroll over the rows shown, to move and
    /// change. Ask for a redraw after changing them. The tree keeps their
    /// count to the rows shown, so a count set here is replaced.
    pub fn state_mut(&mut self) -> &mut ListState {
        self.list.state_mut()
    }

    /// The count and prefix typed toward a motion, to show in a status bar
    /// or a [`WhichKey`](crate::WhichKey) popup.
    pub fn keys(&self) -> &ListKeys {
        self.list.keys()
    }

    /// The count and prefix typed toward a motion, to drop them with
    /// [`ListKeys::reset`] when the tree loses the keyboard.
    pub fn keys_mut(&mut self) -> &mut ListKeys {
        self.list.keys_mut()
    }

    // --------------------------------------------------------------------
    // Expanding
    // --------------------------------------------------------------------

    /// Expands node `id`: [`TreeViewResponse::Expanded`] when its children are
    /// held, [`TreeViewResponse::Load`] with a loading row when they must be
    /// loaded, [`TreeViewResponse::Handled`] when it was expanded already or is
    /// no node.
    pub fn expand(&mut self, id: &K) -> TreeViewResponse<K> {
        if self.expanded.contains(id) {
            return TreeViewResponse::Handled;
        }
        self.expanded.insert(id.clone());
        let response = match self.children.get(id) {
            Some(Children::Loaded(_) | Children::Failed(_)) => {
                TreeViewResponse::Expanded(id.clone())
            }
            Some(Children::Loading) => TreeViewResponse::Handled,
            None => {
                self.children.insert(id.clone(), Children::Loading);
                TreeViewResponse::Load(id.clone())
            }
        };
        self.rebuild();
        response
    }

    /// Collapses node `id`, moving the cursor to it when it was below it.
    pub fn collapse(&mut self, id: &K) -> TreeViewResponse<K> {
        if !self.expanded.contains(id) {
            return TreeViewResponse::Handled;
        }
        let cursor = self.list.state().cursor();
        let node = self.row_of(id);
        let inside = node.is_some_and(|node| cursor > node && self.is_below(cursor, node));
        self.expanded.remove(id);
        // A failure is shown once; collapsing lets the next expand retry.
        if matches!(self.children.get(id), Some(Children::Failed(_))) {
            self.children.remove(id);
        }
        if let (true, Some(node)) = (inside, node) {
            self.list.state_mut().set_cursor(node);
        }
        self.rebuild();
        TreeViewResponse::Collapsed(id.clone())
    }

    /// Expands a collapsed node and collapses an expanded one.
    pub fn toggle(&mut self, id: &K) -> TreeViewResponse<K> {
        match self.expanded.contains(id) {
            true => self.collapse(id),
            false => self.expand(id),
        }
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Reacts to `key`, as the module documentation lists. Hand it the
    /// keys of the window while the tree has the focus.
    pub(crate) fn handle_key(&mut self, key: Key) -> TreeViewResponse<K> {
        let CountedKey { count, key } = match self.list.read_key(key) {
            Read::Consumed => return TreeViewResponse::Handled,
            Read::Ignored => return TreeViewResponse::Ignored,
            Read::Key(counted) => counted,
        };
        match key {
            ListKey::Activate => match self.cursor() {
                Some(id) => TreeViewResponse::Activated(id.clone()),
                None => TreeViewResponse::Handled,
            },
            ListKey::Left => self.step_out(),
            ListKey::LineEnd | ListKey::LineStart => TreeViewResponse::Ignored,
            ListKey::Extend(_) | ListKey::Move(_) | ListKey::Toggle => {
                self.list.run_key(key, count);
                TreeViewResponse::Handled
            }
            ListKey::Right => self.step_in(),
        }
    }

    /// Reacts to pointer `input`, as the module documentation lists.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TreeViewResponse<K> {
        if let Some(position) = input.position() {
            self.pointer = Some(position);
        }
        if *input == Input::PointerLeft {
            self.pointer = None;
        }
        match self.list.handle_event(input, cx) {
            ListResponse::Activated(row) => match self.item_at(row) {
                Some(item) => TreeViewResponse::Activated(item.id.clone()),
                None => TreeViewResponse::Handled,
            },
            ListResponse::CursorMoved(row) => {
                let on_arrow = self
                    .pointer
                    .is_some_and(|point| self.is_on_arrow(row, point.x));
                match self
                    .item_at(row)
                    .filter(|item| item.has_children && on_arrow)
                {
                    Some(item) => {
                        let id = item.id.clone();
                        self.toggle(&id)
                    }
                    None => TreeViewResponse::Handled,
                }
            }
            ListResponse::Dropped(_) | ListResponse::Handled => TreeViewResponse::Handled,
            ListResponse::Ignored => TreeViewResponse::Ignored,
        }
    }

    /// Right: expands the node at the cursor, or moves to its first child
    /// when it is expanded.
    fn step_in(&mut self) -> TreeViewResponse<K> {
        let cursor = self.list.state().cursor();
        let Some(item) = self.item_at(cursor).filter(|item| item.has_children) else {
            return TreeViewResponse::Handled;
        };
        let id = item.id.clone();
        if !self.expanded.contains(&id) {
            return self.expand(&id);
        }
        if self.is_below(cursor + 1, cursor) {
            self.list.state_mut().set_cursor(cursor + 1);
        }
        TreeViewResponse::Handled
    }

    /// Left: collapses the node at the cursor when it is expanded, or moves
    /// to its parent.
    fn step_out(&mut self) -> TreeViewResponse<K> {
        let cursor = self.list.state().cursor();
        let Some(row) = self.rows.get(cursor) else {
            return TreeViewResponse::Handled;
        };
        if let RowKind::Item(item) = &row.kind
            && self.expanded.contains(&item.id)
        {
            let id = item.id.clone();
            return self.collapse(&id);
        }
        if let Some(parent) = row.parent.clone().and_then(|parent| self.row_of(&parent)) {
            self.list.state_mut().set_cursor(parent);
        }
        TreeViewResponse::Handled
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Where row `index` was drawn in the last frame, in physical pixels,
    /// whether in sight or not; `None` before the first frame.
    pub fn row_rect(&self, index: usize) -> Option<Rect> {
        self.list.row_rect(index)
    }

    /// The rows drawn in the last frame, wholly or in part. Empty before
    /// the first frame.
    pub fn visible_range(&self) -> Range<usize> {
        self.list.visible_range()
    }

    /// The id of the item row `index` shows, or `None` for a loading or
    /// failure row or past the last row.
    pub fn row_id(&self, index: usize) -> Option<&K> {
        self.item_at(index).map(TreeItem::id)
    }

    /// Describes the tree as last drawn to screen readers as `id`: a tree
    /// holding the rows in sight as tree items (row `index` is the
    /// `index`th child of `id`) named by their labels, with their level,
    /// nodes marked expanded or collapsed, selected rows marked; the
    /// cursor's row has the focus while the tree does. Nothing before the
    /// first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let state = self.list.state();
        let Some(top) = self.list.row_rect(state.scroll()) else {
            return;
        };
        let height = state.visible_rows().len() as f32 * top.height;
        frame.enter_access(
            id,
            Rect::new(top.x, top.y, top.width, height),
            Node::new(Role::Tree),
        );
        for index in state.visible_rows() {
            let (Some(row), Some(rect)) = (self.rows.get(index), self.list.row_rect(index)) else {
                continue;
            };
            let mut node = Node::new(Role::TreeItem);
            node.set_level(row.depth + 1);
            node.set_selected(state.is_selected(index));
            match &row.kind {
                RowKind::Failed(reason) => {
                    node.set_label(strings::fill(&self.strings.load_failed, &[reason]));
                }
                RowKind::Item(item) => {
                    node.set_label(item.label.as_str());
                    if item.has_children {
                        node.set_expanded(self.expanded.contains(&item.id));
                    }
                }
                RowKind::Loading => {
                    node.set_label(self.strings.loading.as_ref());
                    node.set_busy();
                }
            }
            frame.access(id.child(index as u64), rect, node);
        }
        frame.leave_access();
        if state.visible_rows().contains(&state.cursor()) {
            frame
                .access
                .set_inner_focus(id, id.child(state.cursor() as u64));
        }
    }

    // --------------------------------------------------------------------
    // Rows
    // --------------------------------------------------------------------

    /// Lays the rows out again from the roots, the expanded nodes, and the
    /// children held, keeping the cursor on the row it was on: the same
    /// item, or the same node's loading row.
    fn rebuild(&mut self) {
        let cursor = self.list.state().cursor();
        let mut wanted = self.cursor_path(cursor);
        if let Some((lost, at)) = self.lost_cursor.take()
            && at == cursor
        {
            wanted.insert(0, (Some(lost), None));
        }
        let mut rows = Vec::with_capacity(self.rows.len());
        let roots = mem::take(&mut self.roots);
        self.push_rows(&roots, 0, None, &mut rows);
        self.roots = roots;
        self.rows = rows;
        self.list.state_mut().set_count(self.rows.len());
        let found = wanted.iter().enumerate().find_map(|(rank, key)| {
            let index = self.rows.iter().position(|row| row_key(row) == *key)?;
            Some((rank, index))
        });
        let Some((rank, index)) = found else {
            return;
        };
        self.list.state_mut().set_cursor(index);
        if let (true, Some((Some(lost), _))) = (rank > 0, wanted.first()) {
            self.lost_cursor = Some((lost.clone(), index));
        }
    }

    /// The keys of the row at `index` and of the nodes above it, nearest
    /// first: where the cursor goes when its row is gone.
    fn cursor_path(&self, index: usize) -> Vec<RowKey<K>> {
        let mut path = Vec::new();
        let mut row = self.rows.get(index);
        while let Some(current) = row {
            path.push(row_key(current));
            row = current
                .parent
                .as_ref()
                .and_then(|parent| self.row_of(parent))
                .and_then(|parent| self.rows.get(parent));
        }
        path
    }

    /// Adds `items` at `depth` under `parent` to `rows`, each followed by
    /// its children when it is expanded.
    fn push_rows(
        &self,
        items: &[TreeItem<K>],
        depth: usize,
        parent: Option<&K>,
        rows: &mut Vec<TreeRow<K>>,
    ) {
        for item in items {
            rows.push(TreeRow {
                depth,
                kind: RowKind::Item(item.clone()),
                parent: parent.cloned(),
            });
            if !item.has_children || !self.expanded.contains(&item.id) {
                continue;
            }
            let under = |kind| TreeRow {
                depth: depth + 1,
                kind,
                parent: Some(item.id.clone()),
            };
            match self.children.get(&item.id) {
                Some(Children::Loaded(children)) => {
                    self.push_rows(children, depth + 1, Some(&item.id), rows)
                }
                Some(Children::Failed(reason)) => rows.push(under(RowKind::Failed(reason.clone()))),
                Some(Children::Loading) | None => rows.push(under(RowKind::Loading)),
            }
        }
    }

    /// The item shown at row `index`, or `None` for a loading or failure
    /// row or past the last row.
    fn item_at(&self, index: usize) -> Option<&TreeItem<K>> {
        match &self.rows.get(index)?.kind {
            RowKind::Item(item) => Some(item),
            RowKind::Failed(_) | RowKind::Loading => None,
        }
    }

    /// The row showing item `id`.
    fn row_of(&self, id: &K) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(&row.kind, RowKind::Item(item) if item.id == *id))
    }

    /// Whether row `index` is in the subtree of the node at row `node`:
    /// deeper than it, with no row as shallow between them.
    fn is_below(&self, index: usize, node: usize) -> bool {
        let Some(depth) = self.rows.get(node).map(|row| row.depth) else {
            return false;
        };
        index > node
            && self
                .rows
                .get(node + 1..=index)
                .is_some_and(|between| between.iter().all(|row| row.depth > depth))
    }

    /// Whether `x`, in physical pixels, falls on the arrow of row `index`
    /// as last drawn.
    fn is_on_arrow(&self, index: usize, x: f32) -> bool {
        let (Some(row), Some(rect)) = (self.rows.get(index), self.list.row_rect(index)) else {
            return false;
        };
        let start = rect.x + indent_cells(row.depth) as f32 * self.cell_width;
        x >= start && x < start + ARROW_CELLS as f32 * self.cell_width
    }
}

impl<K: Clone + Eq + Hash> Widget for TreeView<K> {
    type Output = ();
    type Response = TreeViewResponse<K>;

    /// Reacts to `input`, as the module documentation lists: keys while
    /// the tree has the keyboard, and the pointer. Asks for a redraw
    /// whenever what is drawn changed.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TreeViewResponse<K> {
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
        if !matches!(response, TreeViewResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the rows that fit in `area` in `theme`'s colors: each indented
    /// by its depth, nodes with an arrow pointing right when collapsed and
    /// down when expanded, a loading row muted and a failure in the danger
    /// color, cursor and selection as a [`List`] draws them.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if self.strings != theme.strings {
            self.strings = theme.strings.clone();
        }
        let words = &theme.strings;
        let metrics = frame.cell_metrics();
        self.cell_width = metrics.width;
        let rows = RowsToDraw {
            area,
            count: self.rows.len(),
            now: Instant::now(),
        };
        let (shown, expanded) = (&self.rows, &self.expanded);
        let draw_row = |frame: &mut Frame, index: usize, rect: Rect| {
            let row = &shown[index];
            let x = rect.x + (indent_cells(row.depth) as f32 * metrics.width);
            let label_x = x + (ARROW_CELLS as f32 * metrics.width);
            let clip = Rect::new(
                label_x,
                rect.y,
                (rect.x + rect.width - label_x).max(0.0),
                rect.height,
            );
            let text = match &row.kind {
                RowKind::Failed(reason) => {
                    StyledText::plain(strings::fill(&words.load_failed, &[reason]), FAILED_COLOR)
                }
                RowKind::Item(item) => {
                    if item.has_children {
                        let arrow = match expanded.contains(&item.id) {
                            true => EXPANDED_ARROW,
                            false => COLLAPSED_ARROW,
                        };
                        let arrow = StyledText::plain(arrow, QUIET_COLOR);
                        frame.draw_text(&arrow, Point::new(x, rect.y), rect, theme);
                    }
                    if let Some(content) = &item.content {
                        draw_content(frame, content, clip, TextAlign::Start, theme);
                        return;
                    }
                    StyledText::plain(item.label.as_str(), LABEL_COLOR)
                }
                RowKind::Loading => StyledText::plain(words.loading.as_ref(), QUIET_COLOR),
            };
            frame.draw_text(
                &text.with_ellipsis(),
                Point::new(label_x, rect.y),
                clip,
                theme,
            );
        };
        self.list.draw_rows(frame, rows, theme, draw_row, |_| {
            words.loading.as_ref().into()
        });
    }

    /// Asks for the next frame of a glide still under way, returning when
    /// it is due; `None` at rest. Call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait).
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.list.about_to_wait(cx)
    }
}

impl<K> Outcome for TreeViewResponse<K> {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

impl<K: Clone + Eq + Hash> Default for TreeView<K> {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The cells before the arrow of a row at `depth`.
fn indent_cells(depth: usize) -> usize {
    PADDING_CELLS + depth * INDENT_CELLS
}

/// What identifies `row` across rebuilds: its item, or the node whose
/// loading or failure row it is.
fn row_key<K: Clone>(row: &TreeRow<K>) -> RowKey<K> {
    match &row.kind {
        RowKind::Item(item) => (Some(item.id.clone()), None),
        RowKind::Failed(_) | RowKind::Loading => (None, row.parent.clone()),
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
    use crate::base::key::parse_key;
    use crate::testing::drawn_text;

    /// A tree of two folders and a file at the top.
    fn tree() -> TreeView<&'static str> {
        let mut tree = TreeView::new();
        tree.set_roots(vec![
            TreeItem::branch("docs", "docs"),
            TreeItem::branch("src", "src"),
            TreeItem::leaf("readme", "README.md"),
        ]);
        tree
    }

    /// What the rows show, indented two spaces a level, a loading row as
    /// `...`.
    fn shown(tree: &TreeView<&'static str>) -> Vec<String> {
        tree.rows
            .iter()
            .map(|row| {
                let text = match &row.kind {
                    RowKind::Failed(_) => "!".to_owned(),
                    RowKind::Item(item) => item.label.clone(),
                    RowKind::Loading => "...".to_owned(),
                };
                format!("{}{text}", "  ".repeat(row.depth))
            })
            .collect()
    }

    /// Hands `tree` each key of `specs`, returning the last response.
    fn keys(tree: &mut TreeView<&'static str>, specs: &[&str]) -> TreeViewResponse<&'static str> {
        let mut last = TreeViewResponse::Ignored;
        for spec in specs {
            last = tree.handle_key(parse_key(spec).expect("a valid key"));
        }
        last
    }

    #[test]
    fn test_expanding_an_unloaded_node_asks_for_its_children_under_a_loading_row() {
        let mut tree = tree();
        assert_eq!(keys(&mut tree, &["j", "l"]), TreeViewResponse::Load("src"));
        assert_eq!(shown(&tree), ["docs", "src", "  ...", "README.md"]);
        tree.set_children(&"src", vec![TreeItem::leaf("main", "main.rs")]);
        assert_eq!(shown(&tree), ["docs", "src", "  main.rs", "README.md"]);
        assert_eq!(tree.cursor(), Some(&"src"), "the cursor stayed on its item");
        keys(&mut tree, &["h"]);
        assert_eq!(
            keys(&mut tree, &["l"]),
            TreeViewResponse::Expanded("src"),
            "kept"
        );
    }

    #[test]
    fn test_right_steps_into_a_node_and_left_collapses_or_steps_to_the_parent() {
        let mut tree = tree();
        tree.expand(&"docs");
        tree.set_children(&"docs", vec![TreeItem::branch("guide", "guide")]);
        keys(&mut tree, &["l"]);
        assert_eq!(
            tree.cursor(),
            Some(&"guide"),
            "Right on an open node: its first child"
        );
        keys(&mut tree, &["h"]);
        assert_eq!(
            tree.cursor(),
            Some(&"docs"),
            "Left on a closed child: its parent"
        );
        assert_eq!(keys(&mut tree, &["h"]), TreeViewResponse::Collapsed("docs"));
        assert_eq!(shown(&tree), ["docs", "src", "README.md"]);
        assert_eq!(
            keys(&mut tree, &["G", "l"]),
            TreeViewResponse::Handled,
            "a leaf"
        );
        assert_eq!(
            keys(&mut tree, &["<cr>"]),
            TreeViewResponse::Activated("readme")
        );
    }

    #[test]
    fn test_a_reload_keeps_nested_expansion_and_the_cursor_by_id() {
        let mut tree = tree();
        tree.expand(&"src");
        tree.set_children(
            &"src",
            vec![
                TreeItem::branch("ui", "ui"),
                TreeItem::leaf("lib", "lib.rs"),
            ],
        );
        tree.expand(&"ui");
        tree.set_children(&"ui", vec![TreeItem::leaf("view", "view.rs")]);
        keys(&mut tree, &["G", "k"]);
        assert_eq!(tree.cursor(), Some(&"lib"));
        assert!(tree.reload(&"src"));
        assert_eq!(shown(&tree), ["docs", "src", "  ...", "README.md"]);
        assert_eq!(
            tree.cursor(),
            Some(&"src"),
            "on the nearest node still shown"
        );
        tree.set_children(
            &"src",
            vec![
                TreeItem::leaf("a", "a.rs"),
                TreeItem::leaf("lib", "lib.rs"),
                TreeItem::branch("ui", "ui"),
            ],
        );
        assert_eq!(
            shown(&tree),
            [
                "docs",
                "src",
                "  a.rs",
                "  lib.rs",
                "  ui",
                "    view.rs",
                "README.md"
            ],
            "ui is still expanded with its children"
        );
        assert_eq!(
            tree.cursor(),
            Some(&"lib"),
            "back on its item once reloaded"
        );
    }

    #[test]
    fn test_collapsing_a_node_brings_a_cursor_inside_it_up_to_the_node() {
        let mut tree = tree();
        tree.expand(&"docs");
        tree.set_children(
            &"docs",
            vec![TreeItem::leaf("a", "a.md"), TreeItem::leaf("b", "b.md")],
        );
        keys(&mut tree, &["j", "j"]);
        assert_eq!(tree.cursor(), Some(&"b"));
        tree.collapse(&"docs");
        assert_eq!(tree.cursor(), Some(&"docs"));
    }

    #[test]
    fn test_a_failed_load_shows_its_reason_and_retries_after_a_collapse() {
        let mut tree = tree();
        tree.expand(&"docs");
        tree.set_load_failed(&"docs", "permission denied");
        assert_eq!(shown(&tree), ["docs", "  !", "src", "README.md"]);
        tree.collapse(&"docs");
        assert_eq!(tree.expand(&"docs"), TreeViewResponse::Load("docs"));
    }

    #[test]
    fn test_loading_and_failed_rows_are_worded_by_the_theme() {
        let theme = Theme {
            strings: Strings {
                load_failed: "Fehler ({})".into(),
                loading: "Lädt…".into(),
                ..Strings::english()
            },
            ..Theme::dark()
        };
        let mut tree = tree();
        tree.expand(&"docs");
        tree.set_load_failed(&"docs", "keine Rechte");
        tree.expand(&"src");
        let mut frame = offscreen::plain_frame(400, 300);
        tree.draw(&mut frame, Rect::new(0.0, 0.0, 400.0, 300.0), &theme);
        let drawn = drawn_text(&frame).concat();
        assert!(drawn.contains("Fehler (keine Rechte)"), "{drawn}");
        assert!(drawn.contains("Lädt…"), "{drawn}");
        let id = FocusId::new("tree");
        tree.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let label = |row| find_node(&update, id.child(row)).and_then(Node::label);
        assert_eq!(label(1), Some("Fehler (keine Rechte)"));
        assert_eq!(label(3), Some("Lädt…"));
    }

    #[test]
    fn test_a_press_on_an_arrow_falls_on_the_arrow_of_its_depth() {
        let mut frame = offscreen::plain_frame(400, 300);
        let mut tree = tree();
        tree.expand(&"docs");
        tree.set_children(&"docs", vec![TreeItem::branch("guide", "guide")]);
        tree.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 400.0, 300.0),
            &Theme::dark(),
        );
        let cell = frame.cell_metrics().width;
        assert!(
            tree.is_on_arrow(0, cell * 1.5),
            "a root's arrow after the padding"
        );
        assert!(!tree.is_on_arrow(0, cell * 3.5), "a root's label");
        assert!(
            tree.is_on_arrow(1, cell * 3.5),
            "a child's arrow two cells in"
        );
    }

    #[test]
    fn test_the_tree_describes_its_items_with_level_and_expanded_state() {
        let id = FocusId::new("folders");
        let mut frame = offscreen::plain_frame(400, 300);
        let mut tree = tree();
        tree.expand(&"docs");
        tree.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 400.0, 300.0),
            &Theme::dark(),
        );
        tree.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let node = |id| find_node(&update, id).expect("described");
        assert_eq!(node(id).role(), Role::Tree);
        let docs = node(id.child(0));
        assert_eq!(
            (docs.role(), docs.label(), docs.level(), docs.is_expanded()),
            (Role::TreeItem, Some("docs"), Some(1), Some(true))
        );
        let loading = node(id.child(1));
        assert_eq!((loading.level(), loading.is_busy()), (Some(2), true));
        assert_eq!(
            node(id.child(2)).is_expanded(),
            Some(false),
            "src is collapsed"
        );
        assert_eq!(node(id.child(3)).is_expanded(), None, "a leaf has no state");
    }

    #[test]
    fn test_an_item_with_content_draws_it_and_screen_readers_keep_the_label() {
        use crate::base::styled::{Span, StyledText};
        use crate::widgets::row_content::RowContent;
        let mut tree = TreeView::new();
        let styled = StyledText::new()
            .with(Span::new("READ", ThemeToken::Accent.into()))
            .with(Span::new("ME", ThemeToken::Muted.into()));
        tree.set_roots(vec![
            TreeItem::leaf("readme", "README.md").with_content(RowContent::styled(styled)),
        ]);
        let mut frame = offscreen::plain_frame(400, 300);
        tree.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 400.0, 300.0),
            &Theme::dark(),
        );
        assert_eq!(drawn_text(&frame).concat(), "README");
        assert_eq!(tree.row_id(0), Some(&"readme"));
        assert_eq!(tree.visible_range(), 0..1);
        let id = FocusId::new("tree");
        tree.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let label = find_node(&update, id.child(0)).and_then(Node::label);
        assert_eq!(label, Some("README.md"));
    }

    #[test]
    fn test_shift_arrows_extend_the_selection_over_the_rows() {
        let mut tree = tree();
        tree.handle_key(parse_key("S-<down>").expect("a key"));
        tree.handle_key(parse_key("S-<down>").expect("a key"));
        assert_eq!(tree.selected(), [&"docs", &"src", &"readme"]);
    }
}

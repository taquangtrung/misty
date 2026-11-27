//! The elements that show the app's data, lent for one rebuild rather
//! than copied: [`list`] over a [`ListSource`], [`table`] over a
//! [`TableSource`], [`tree_view`] over the app's [`TreeItems`],
//! [`icon_grid`] over a [`GridSource`], and [`cells`] over a
//! [`CellSource`].
//!
//! Each list-like element is *controlled* as the form controls are: its
//! cursor and its selection come from the app's state, and the user moves
//! them only through the callbacks the element was given (`on_cursor_move`,
//! `on_select`), which misty runs on the app's state once the tree is
//! dropped. A list without `on_cursor_move` keeps its cursor where the app
//! put it, and one without `on_select` selects nothing. What only the view
//! knows (the scroll and its glide, a count or prefix typed toward a motion,
//! a drag, a rubber band, the nodes a tree expanded, the column widths a
//! drag set) misty keeps under the element's id, and reports what the app
//! may want to show or keep through more callbacks (`on_keys`, `on_expand`,
//! `on_resize`).
//!
//! Each list-like element takes the height its column leaves, as a
//! scrolling container does, so it goes in a [`column`](crate::view::column)
//! beside the elements that keep their own height (a status line).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::hash::Hash;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::cell::{Cell, CellHighlight, CellSource, Cursor};
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::Key;
use super::super::base::list::ListState;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::super::widgets::icon_grid::{GridSource, IconGrid, IconGridResponse};
use super::super::widgets::list_keys::ListKeys;
use super::super::widgets::list_view::{self, ListResponse, ListSource, RowDrop};
use super::super::widgets::table::{
    self, HeaderMenu, SortKey, TableColumn, TableResponse, TableSource,
};
use super::super::widgets::thumbnails::Thumbnails;
use super::super::widgets::tree_view::{TreeDrop, TreeItems, TreeView, TreeViewResponse};
use super::super::widgets::which_key::WhichKey;
use super::controls::{Callback, call};
use super::custom::Accepts;
use super::element::{Element, Kind};
use super::node::{Call, Drawn, Spec};

// ========================================================================
// Constants
// ========================================================================

/// The selection of a list element the app gave none.
static NO_ROWS: BTreeSet<usize> = BTreeSet::new();

// ========================================================================
// Data Structures
// ========================================================================

/// A virtualized list of one-line rows the app lends, as an element. Made
/// by [`list`].
pub struct List<'a, A, S: ?Sized> {
    /// The row the app's cursor is on.
    cursor: usize,
    /// What the cursor, the selection, and the keys typed run.
    moves: Moves<A, usize, BTreeSet<usize>>,
    /// What activating a row runs.
    on_activate: Option<Callback<A, usize>>,
    /// What dropping a dragged row runs.
    on_drop: Option<Callback<A, RowDrop>>,
    /// What a key the list left unused on the cursor's row runs.
    on_row_key: Option<RowKeys<'a, A, usize>>,
    /// Whether the rows are set in the proportional font.
    proportional: bool,
    /// The rows.
    rows: &'a S,
    /// The rows the app has selected.
    selected: &'a BTreeSet<usize>,
    /// Whether a prefix left waiting opens a popup of what may follow.
    which_key: bool,
}

/// A virtualized table of rows in columns the app lends, as an element.
/// Made by [`table`].
pub struct Table<'a, A, S: ?Sized> {
    /// The column of the app's cell cursor, or `None` for none.
    cell_cursor: Option<usize>,
    /// The columns, left to right.
    columns: &'a [TableColumn],
    /// The row the app's cursor is on.
    cursor: usize,
    /// How many leading rows and columns stay in place.
    frozen: (usize, usize),
    /// What the cursor, the selection, and the keys typed run.
    moves: Moves<A, usize, BTreeSet<usize>>,
    /// What activating a row runs.
    on_activate: Option<Callback<A, usize>>,
    /// What dropping a dragged row runs.
    on_drop: Option<Callback<A, RowDrop>>,
    /// What moving the cell cursor runs.
    on_cell_cursor_move: Option<Callback<A, usize>>,
    /// What a right click on the header runs.
    on_header_menu: Option<Callback<A, HeaderMenu>>,
    /// What ending a drag of a column's border runs.
    on_resize: Option<Callback<A, (usize, Vec<f32>)>>,
    /// What a key the table left unused on the cursor's row runs.
    on_row_key: Option<RowKeys<'a, A, usize>>,
    /// What a click on a sortable column's title runs.
    on_sort: Option<Callback<A, SortKey>>,
    /// Whether the cells are set in the proportional font.
    proportional: bool,
    /// The rows.
    rows: &'a S,
    /// The rows the app has selected.
    selected: &'a BTreeSet<usize>,
    /// The column the app sorted its rows by.
    sort: Option<SortKey>,
    /// How many times their size the cells are drawn.
    zoom: f32,
}

/// A tree of rows over items the app keeps and loads, as an element. Made
/// by [`tree_view`].
pub struct Tree<'a, A, K> {
    /// The item the app's cursor is on, or `None` to leave it where it is.
    cursor: Option<&'a K>,
    /// The items.
    items: &'a TreeItems<K>,
    /// What the cursor, the selection, and the keys typed run.
    moves: Moves<A, Option<K>, Vec<K>>,
    /// What activating an item runs.
    on_activate: Option<Callback<A, K>>,
    /// What collapsing a node runs.
    on_collapse: Option<Callback<A, K>>,
    /// What expanding a node whose children are held runs.
    on_expand: Option<Callback<A, K>>,
    /// What dropping an item dragged onto another runs, which lets items
    /// be dragged.
    on_drop: Option<Callback<A, TreeDrop<K>>>,
    /// What expanding a node whose children must be loaded runs.
    on_load: Option<Callback<A, K>>,
    /// What a key the tree left unused on the cursor's item runs.
    on_row_key: Option<RowKeys<'a, A, K>>,
    /// Whether the labels are set in the proportional font.
    proportional: bool,
    /// The items the app has selected.
    selected: &'a [K],
}

/// A virtualized grid of thumbnails with labels the app lends, as an
/// element. Made by [`icon_grid`].
pub struct Grid<'a, A, S: GridSource + ?Sized> {
    /// The item the app's cursor is on.
    cursor: usize,
    /// The items.
    items: &'a S,
    /// Makes the cache the thumbnails load in, the first time the element
    /// is built.
    make_thumbnails: Box<dyn Fn() -> Thumbnails<S::Key> + 'a>,
    /// What the cursor, the selection, and the keys typed run.
    moves: Moves<A, usize, BTreeSet<usize>>,
    /// What activating an item runs.
    on_activate: Option<Callback<A, usize>>,
    /// The items the app has selected.
    selected: &'a BTreeSet<usize>,
}

/// A grid of styled cells the app lends, as a terminal's screen is drawn:
/// its cursor blinking. Made by [`cells`].
pub struct Cells<'a, S: ?Sized> {
    /// Whether the cursor blinks.
    blinks: bool,
    /// The cells.
    source: &'a S,
}

/// The callbacks every list-like element shares: what moving its cursor,
/// changing its selection, and typing toward a motion run.
struct Moves<A, C, S> {
    /// What moving the cursor runs.
    on_cursor_move: Option<Callback<A, C>>,
    /// What a count or prefix typed or dropped runs.
    on_keys: Option<Callback<A, ListKeys>>,
    /// What changing the selection runs.
    on_select: Option<Callback<A, S>>,
}

/// The keys of the app's own a list-like element hands it with the row
/// of type `R` the cursor is on: which it takes, and what they run.
struct RowKeys<'a, A, R> {
    /// Whether a key the widget left unused is the app's.
    accepts: Accepts<'a>,
    /// What runs with the cursor's row and the key.
    apply: Callback<A, (R, Key)>,
}

/// What the user changed of a list-like element's cursor, selection, and
/// keys typed in one input, with the cursor of type `C` and the selection
/// of type `S`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Moved<C, S> {
    /// The cursor, if it moved.
    cursor: Option<C>,
    /// The count and prefix typed, if they changed.
    keys: Option<ListKeys>,
    /// The selection, if it changed.
    selected: Option<S>,
}

/// What a list-like element's widget answered an input with: its own
/// response, and what moved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Answered<R, C, S> {
    /// What moved.
    moved: Moved<C, S>,
    /// The widget's response.
    response: R,
    /// A key the widget left unused that the element hands the app, with
    /// the cursor's row.
    row_key: Option<(C, Key)>,
}

/// The props every list-like element gives its widget: how many rows the
/// app lends, and the app's cursor and selection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowsProps<'a> {
    /// How many rows there are.
    count: usize,
    /// The row the app's cursor is on.
    cursor: usize,
    /// The rows the app has selected.
    selected: &'a BTreeSet<usize>,
}

/// The widget kept for a [`List`]: the list and its which-key popup.
pub(crate) struct ListBox {
    /// The list: its cursor, selection, scroll, and the keys typed.
    list: list_view::List,
    /// The keys the which-key popup was last told of.
    shown_keys: ListKeys,
    /// What may follow a prefix left waiting.
    which_key: WhichKey,
}

/// The props a [`Table`] gives its widget.
pub(crate) struct TableProps<'a> {
    /// The column of the app's cell cursor.
    cell_cursor: Option<usize>,
    /// The app's columns.
    columns: &'a [TableColumn],
    /// How many leading rows and columns stay in place.
    frozen: (usize, usize),
    /// The rows' count, and the app's cursor and selection.
    rows: RowsProps<'a>,
    /// The column the app sorted its rows by.
    sort: Option<SortKey>,
    /// How many times their size the cells are drawn.
    zoom: f32,
}

/// The widget kept for a [`Table`]: the table and the app's columns as
/// it last took them.
pub(crate) struct TableBox {
    /// The app's columns as last taken.
    columns: Vec<TableColumn>,
    /// The table.
    table: table::Table,
}

/// What a table answered, and every column's width after a resize.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TableReply {
    /// The column the cell cursor moved to, if it moved.
    column: Option<usize>,
    /// The table's response.
    response: TableResponse,
    /// The columns' widths, when a resize ended.
    widths: Vec<f32>,
}

/// The props a [`Tree`] gives its widget.
pub(crate) struct TreeProps<'a, K> {
    /// The item the app's cursor is on.
    cursor: Option<&'a K>,
    /// The items.
    items: &'a TreeItems<K>,
    /// The items the app has selected.
    selected: &'a [K],
}

/// The widget kept for a [`Tree`]: the tree view and the version of the
/// items it last copied.
pub(crate) struct TreeBox<K> {
    /// The tree view.
    tree: TreeView<K>,
    /// The version of the items last copied, or `None` before the first.
    version: Option<u64>,
}

/// The widget kept for a [`Grid`]: the grid and its thumbnails' cache.
pub(crate) struct GridBox<K> {
    /// The grid.
    grid: IconGrid,
    /// The thumbnails, made the first time the element was built.
    thumbnails: Thumbnails<K>,
}

/// The props a [`Cells`] element gives its widget.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CellsProps {
    /// Whether the cursor blinks.
    blinks: bool,
    /// The cursor the source has.
    cursor: Option<Cursor>,
    /// How many rows the source has.
    rows: usize,
}

/// The widget kept for a [`Cells`] element: its cursor's blink.
pub(crate) struct CellsBox {
    /// Whether the cursor blinks.
    blinks: bool,
    /// The cursor as last seen, to start its blink over when it moves.
    cursor: Option<Cursor>,
    /// How long the cursor shows, then hides, as last drawn.
    interval: Duration,
    /// How many rows the source has.
    rows: usize,
    /// When the cursor last started to show.
    since: Instant,
}

/// A grid of cells with its cursor hidden while it blinks off.
struct Blinked<'s, S: ?Sized> {
    /// The cursor to draw.
    cursor: Option<Cursor>,
    /// The cells.
    source: &'s S,
}

/// No rows, for stepping a table's glide without the app's rows.
struct NoRows;

/// No items, for stepping a grid's glide without the app's items.
struct NoItems<K>(PhantomData<fn() -> K>);

/// A list's answer.
type ListAnswer = Answered<ListResponse, usize, BTreeSet<usize>>;

/// A table's answer.
type TableAnswer = Answered<TableReply, usize, BTreeSet<usize>>;

/// A tree's answer.
type TreeAnswer<K> = Answered<TreeViewResponse<K>, Option<K>, Vec<K>>;

/// A grid's answer.
type GridAnswer = Answered<IconGridResponse, usize, BTreeSet<usize>>;

// ========================================================================
// List Implementations
// ========================================================================

impl<'a, A, S: ?Sized> List<'a, A, S> {
    /// This list with the rows `selected` selected, the app's selection;
    /// none without it.
    pub fn selected(self, selected: &'a BTreeSet<usize>) -> Self {
        Self { selected, ..self }
    }

    /// What runs on the app's state with the row the cursor moved to (a
    /// key, a click, a scroll that took the cursor along). Without it the
    /// cursor stays on the row the app gave.
    pub fn on_cursor_move(mut self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        self.moves.on_cursor_move = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the rows selected after the user
    /// changed the selection (Space, Shift with a motion, Ctrl+click, a
    /// rubber band). Without it nothing can be selected.
    pub fn on_select(mut self, apply: impl Fn(&mut A, BTreeSet<usize>) + 'static) -> Self {
        self.moves.on_select = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the count and prefix typed toward
    /// a motion whenever they change, for a status line to show them.
    pub fn on_keys(mut self, apply: impl Fn(&mut A, ListKeys) + 'static) -> Self {
        self.moves.on_keys = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the row the user activated
    /// (Enter, a double click): open it.
    pub fn on_activate(self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        Self {
            on_activate: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with a dragged row dropped: move the
    /// row (and the cursor with it), or hand it to another drop target.
    /// Without it a row cannot be dragged; fixed when the element is first
    /// built.
    pub fn on_drop(self, apply: impl Fn(&mut A, RowDrop) + 'static) -> Self {
        Self {
            on_drop: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with the row the cursor is on and a
    /// key the list left unused, while `accepts` says yes to the key: a
    /// key of the app's own on the row (`x` to mark it read). The list's
    /// own keys (its motions and counts, a prefix and the key completing
    /// or dropping it, Space, Enter) never reach `accepts`; a key it
    /// declines goes on to the containers around the list and the app, as
    /// without it. An empty list hands none.
    pub fn on_row_key(
        self,
        accepts: impl Fn(Key) -> bool + 'a,
        apply: impl Fn(&mut A, usize, Key) + 'static,
    ) -> Self {
        Self {
            on_row_key: Some(RowKeys::new(accepts, apply)),
            ..self
        }
    }

    /// This list setting its rows on the monospace cell grid, for rows
    /// whose columns line up by character, rather than in the proportional
    /// font rows are set in; fixed when the element is first built.
    pub fn monospace(self) -> Self {
        Self {
            proportional: false,
            ..self
        }
    }

    /// This list in the proportional font, as every list is unless
    /// [`List::monospace`] puts it on the cell grid.
    #[deprecated(
        note = "rows are proportional by default; drop the call, or use `monospace` for the cell grid"
    )]
    pub fn proportional(self) -> Self {
        Self {
            proportional: true,
            ..self
        }
    }

    /// This list showing a popup of the keys that may follow a prefix left
    /// waiting (`g`, `z`), along its bottom, after a moment.
    pub fn which_key(self) -> Self {
        Self {
            which_key: true,
            ..self
        }
    }
}

impl<A: 'static, S: ListSource + ?Sized> Spec<A> for List<'_, A, S> {
    type Widget = ListBox;

    const KIND: &'static str = "list";

    fn create(&self) -> ListBox {
        let mut list = list_view::List::new();
        if self.proportional {
            list = list.with_proportional_font();
        }
        if self.on_drop.is_none() {
            list.turn_drag_off();
        }
        if self.on_activate.is_some() {
            list.make_rows_clickable();
        }
        ListBox {
            list,
            shown_keys: ListKeys::default(),
            which_key: WhichKey::new(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> RowsProps<'b> {
        RowsProps {
            count: self.rows.row_count(),
            cursor: self.cursor,
            selected: self.selected,
        }
    }

    fn call(&self, answer: ListAnswer) -> Option<Call<A>> {
        let event = match answer.response {
            ListResponse::Activated(row) => call(self.on_activate.as_ref(), row),
            ListResponse::Dropped(drop) => call(self.on_drop.as_ref(), drop),
            _ => None,
        };
        let row_key = RowKeys::call(self.on_row_key.as_ref(), answer.row_key);
        self.moves.calls(answer.moved, event.or(row_key))
    }

    fn draw(&self, widget: &mut ListBox, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.list.lend(self.rows).draw(frame, rect, theme);
        if self.which_key {
            let keys = widget.list.keys();
            if *keys != widget.shown_keys {
                widget
                    .which_key
                    .set_pending(keys.pending_keys(), keys.hints(&theme.strings));
                widget.shown_keys = keys.clone();
            }
            widget.which_key.draw(frame, rect, theme);
        }
        rect.into()
    }

    fn describe(&self, widget: &ListBox, frame: &mut Frame, id: FocusId) {
        widget.list.access(frame, id, self.rows);
    }

    /// A click on a row's node (a screen reader's, a hint label's) moves
    /// the cursor there and activates the row.
    fn act(&self, widget: &mut ListBox, id: FocusId, action: AccessAction) -> Option<ListAnswer> {
        let AccessAction::Click(target) = action else {
            return None;
        };
        self.on_activate.as_ref()?;
        let row = widget.list.access_row(id, target)?;
        let before = Watch::of(widget.list.state(), widget.list.keys());
        widget.list.state_mut().set_cursor(row);
        Some(Answered {
            moved: before.moved(widget.list.state(), widget.list.keys()),
            response: ListResponse::Activated(row),
            row_key: None,
        })
    }

    fn handle(&self, widget: &mut ListBox, input: &Input, cx: &mut EventCx<'_>) -> ListAnswer {
        let before = Watch::of(widget.list.state(), widget.list.keys());
        let response = widget.list.lend(self.rows).handle(input, cx);
        let row = cursor_row(widget.list.state());
        Answered {
            moved: before.moved(widget.list.state(), widget.list.keys()),
            row_key: RowKeys::take(self.on_row_key.as_ref(), &response, input, row),
            response,
        }
    }

    fn grows(&self) -> bool {
        true
    }

    fn scrolls_itself(&self) -> bool {
        true
    }
}

impl<'a, A: 'static, S: ListSource + ?Sized> From<List<'a, A, S>> for Element<'a, A> {
    fn from(list: List<'a, A, S>) -> Self {
        Element::new(Kind::Control(Box::new(list)))
    }
}

// ========================================================================
// ListBox Implementations
// ========================================================================

impl Widget for ListBox {
    type Output = ();
    type Response = ListAnswer;

    /// Nothing: the [`List`] element hands its widget inputs lent the
    /// app's rows.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) -> ListAnswer {
        Answered::ignored(ListResponse::Ignored)
    }

    /// Nothing: the [`List`] element draws its widget lent the app's rows.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let glide = self.list.about_to_wait(cx);
        let which_key = self.which_key.about_to_wait(cx);
        glide.into_iter().chain(which_key).min()
    }
}

impl Control for ListBox {
    type Props<'a> = RowsProps<'a>;
    type Value = ListAnswer;

    fn sync(&mut self, props: RowsProps<'_>) {
        props.sync(self.list.state_mut());
    }

    /// Nothing: a list takes the height its column leaves.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, answer: ListAnswer) -> Option<ListAnswer> {
        answer.reports(|response| {
            matches!(
                response,
                ListResponse::Activated(_) | ListResponse::Dropped(_)
            )
        })
    }
}

// ========================================================================
// Table Implementations
// ========================================================================

impl<'a, A, S: ?Sized> Table<'a, A, S> {
    /// This table with the rows `selected` selected, the app's selection;
    /// none without it.
    pub fn selected(self, selected: &'a BTreeSet<usize>) -> Self {
        Self { selected, ..self }
    }

    /// This table marking its rows as sorted by `sort` (`None`: in no
    /// column's order), the order the app put them in.
    pub fn sort(self, sort: Option<SortKey>) -> Self {
        Self { sort, ..self }
    }

    /// What runs on the app's state with the row the cursor moved to.
    /// Without it the cursor stays on the row the app gave.
    pub fn on_cursor_move(mut self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        self.moves.on_cursor_move = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the rows selected after the user
    /// changed the selection. Without it nothing can be selected.
    pub fn on_select(mut self, apply: impl Fn(&mut A, BTreeSet<usize>) + 'static) -> Self {
        self.moves.on_select = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the count and prefix typed toward
    /// a motion whenever they change, for a status line to show them.
    pub fn on_keys(mut self, apply: impl Fn(&mut A, ListKeys) + 'static) -> Self {
        self.moves.on_keys = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the row the user activated
    /// (Enter, a double click): open it.
    pub fn on_activate(self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        Self {
            on_activate: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with a dragged row dropped: move the
    /// row, or hand it to another drop target. Without it a dropped row
    /// stays where it was.
    pub fn on_drop(self, apply: impl Fn(&mut A, RowDrop) + 'static) -> Self {
        Self {
            on_drop: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with the row the cursor is on and a
    /// key the table left unused, while `accepts` says yes to the key: a
    /// key of the app's own on the row (`x` to mark it read). The table's
    /// own keys (its motions and counts, a prefix and the key completing
    /// or dropping it, Space, Enter) never reach `accepts`; a key it
    /// declines goes on to the containers around the table and the app, as
    /// without it. An empty table hands none.
    pub fn on_row_key(
        self,
        accepts: impl Fn(Key) -> bool + 'a,
        apply: impl Fn(&mut A, usize, Key) + 'static,
    ) -> Self {
        Self {
            on_row_key: Some(RowKeys::new(accepts, apply)),
            ..self
        }
    }

    /// What runs on the app's state with the sort a click on a sortable
    /// column's title asks for: reorder the rows, and give the table the
    /// sort with [`Table::sort`]. The table sorts nothing itself.
    pub fn on_sort(self, apply: impl Fn(&mut A, SortKey) + 'static) -> Self {
        Self {
            on_sort: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with the column whose border a drag
    /// resized and every column's width after it, left to right: save the
    /// widths. The table keeps the widths the user dragged to; the app's
    /// columns change them only when the app changes a column's width.
    pub fn on_resize(self, apply: impl Fn(&mut A, (usize, Vec<f32>)) + 'static) -> Self {
        Self {
            on_resize: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with where a right click on the header
    /// landed: open a menu of the columns there, hiding one by giving the
    /// table its column made [`TableColumn::with_hidden`].
    pub fn on_header_menu(self, apply: impl Fn(&mut A, HeaderMenu) + 'static) -> Self {
        Self {
            on_header_menu: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This table setting its cells on the monospace cell grid, as a
    /// terminal's listing lines up by character, rather than in the
    /// proportional font cells are set in, the header following the
    /// theme's chrome font either way; fixed when the element is first
    /// built.
    pub fn monospace(self) -> Self {
        Self {
            proportional: false,
            ..self
        }
    }

    /// This table in the proportional font, as every table is unless
    /// [`Table::monospace`] puts it on the cell grid.
    #[deprecated(
        note = "cells are proportional by default; drop the call, or use `monospace` for the cell grid"
    )]
    pub fn proportional(self) -> Self {
        Self {
            proportional: true,
            ..self
        }
    }

    /// This table keeping its first `rows` rows and `columns` columns in
    /// place while the others scroll under them, as
    /// [`Table::with_frozen`](crate::Table::with_frozen) says; none
    /// without it.
    pub fn frozen(self, rows: usize, columns: usize) -> Self {
        Self {
            frozen: (rows, columns),
            ..self
        }
    }

    /// This table drawing its cells (their text, the rows' height, and the
    /// columns' widths) `zoom` times their size, as
    /// [`Table::set_zoom`](crate::Table::set_zoom) says; 1 without it.
    pub fn zoom(self, zoom: f32) -> Self {
        Self { zoom, ..self }
    }

    /// This table with a cell cursor on `column` of the cursor's row, as
    /// [`Table::set_cell_cursor`](crate::Table::set_cell_cursor) draws and
    /// moves it; none without it.
    pub fn cell_cursor(self, column: usize) -> Self {
        Self {
            cell_cursor: Some(column),
            ..self
        }
    }

    /// What runs on the app's state with the column a key or a click moved
    /// the cell cursor to. Without it the cell cursor stays on the column
    /// the app gave.
    pub fn on_cell_cursor_move(self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        Self {
            on_cell_cursor_move: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static, S: TableSource + ?Sized> Spec<A> for Table<'_, A, S> {
    type Widget = TableBox;

    const KIND: &'static str = "table";

    fn create(&self) -> TableBox {
        let mut table = table::Table::new(self.columns.to_vec());
        if self.proportional {
            table = table.with_proportional_font();
        }
        TableBox {
            columns: self.columns.to_vec(),
            table,
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> TableProps<'b> {
        TableProps {
            cell_cursor: self.cell_cursor,
            columns: self.columns,
            frozen: self.frozen,
            rows: RowsProps {
                count: self.rows.row_count(),
                cursor: self.cursor,
                selected: self.selected,
            },
            sort: self.sort,
            zoom: self.zoom,
        }
    }

    fn call(&self, answer: TableAnswer) -> Option<Call<A>> {
        let TableReply {
            column,
            response,
            widths,
        } = answer.response;
        let event = match response {
            TableResponse::Activated(row) => call(self.on_activate.as_ref(), row),
            TableResponse::ColumnResized(column) => call(self.on_resize.as_ref(), (column, widths)),
            TableResponse::Dropped(drop) => call(self.on_drop.as_ref(), drop),
            TableResponse::HeaderMenu(menu) => call(self.on_header_menu.as_ref(), menu),
            TableResponse::Sorted(sort) => call(self.on_sort.as_ref(), sort),
            _ => None,
        };
        let row_key = RowKeys::call(self.on_row_key.as_ref(), answer.row_key);
        let column = column.and_then(|column| call(self.on_cell_cursor_move.as_ref(), column));
        let event = match (event.or(row_key), column) {
            (Some(event), Some(column)) => Some(Box::new(move |app: &mut A| {
                column(app);
                event(app);
            }) as Call<A>),
            (event, column) => event.or(column),
        };
        self.moves.calls(answer.moved, event)
    }

    fn draw(&self, widget: &mut TableBox, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.table.lend(self.rows).draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &TableBox, frame: &mut Frame, id: FocusId) {
        widget.table.access(frame, id, self.rows);
    }

    fn act(
        &self,
        _widget: &mut TableBox,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<TableAnswer> {
        None
    }

    fn handle(&self, widget: &mut TableBox, input: &Input, cx: &mut EventCx<'_>) -> TableAnswer {
        let before = Watch::of(widget.table.state(), widget.table.keys());
        let column_before = widget.table.cell_cursor();
        let response = widget.table.lend(self.rows).handle(input, cx);
        let widths = match response {
            TableResponse::ColumnResized(_) => widget.table.column_widths(),
            _ => Vec::new(),
        };
        let row = cursor_row(widget.table.state());
        let column = widget
            .table
            .cell_cursor()
            .filter(|column| Some(*column) != column_before);
        Answered {
            moved: before.moved(widget.table.state(), widget.table.keys()),
            row_key: RowKeys::take(self.on_row_key.as_ref(), &response, input, row),
            response: TableReply {
                column,
                response,
                widths,
            },
        }
    }

    fn grows(&self) -> bool {
        true
    }

    fn scrolls_itself(&self) -> bool {
        true
    }
}

impl<'a, A: 'static, S: TableSource + ?Sized> From<Table<'a, A, S>> for Element<'a, A> {
    fn from(table: Table<'a, A, S>) -> Self {
        Element::new(Kind::Control(Box::new(table)))
    }
}

// ========================================================================
// TableBox Implementations
// ========================================================================

impl Widget for TableBox {
    type Output = ();
    type Response = TableAnswer;

    /// Nothing: the [`Table`] element hands its widget inputs lent the
    /// app's rows.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) -> TableAnswer {
        Answered::ignored(TableReply {
            column: None,
            response: TableResponse::Ignored,
            widths: Vec::new(),
        })
    }

    /// Nothing: the [`Table`] element draws its widget lent the app's rows.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        // The glide needs no rows.
        self.table.lend(&NoRows).about_to_wait(cx)
    }
}

impl Control for TableBox {
    type Props<'a> = TableProps<'a>;
    type Value = TableAnswer;

    /// Takes the app's columns where they changed since last taken, its
    /// sort, then its cursor and selection.
    fn sync(&mut self, props: TableProps<'_>) {
        if props.columns != self.columns.as_slice() {
            self.table.take_columns(props.columns, &self.columns);
            self.columns = props.columns.to_vec();
        }
        if self.table.sort() != props.sort {
            self.table.set_sort(props.sort);
        }
        let (rows, columns) = props.frozen;
        self.table.set_frozen(rows, columns);
        self.table.set_zoom(props.zoom);
        self.table.set_cell_cursor(props.cell_cursor);
        props.rows.sync(self.table.state_mut());
    }

    /// Nothing: a table takes the height its column leaves.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, answer: TableAnswer) -> Option<TableAnswer> {
        answer.reports(|reply| {
            reply.column.is_some()
                || matches!(
                    reply.response,
                    TableResponse::Activated(_)
                        | TableResponse::ColumnResized(_)
                        | TableResponse::Dropped(_)
                        | TableResponse::HeaderMenu(_)
                        | TableResponse::Sorted(_)
                )
        })
    }
}

impl Outcome for TableReply {
    fn is_ignored(&self) -> bool {
        self.response.is_ignored()
    }
}

// ========================================================================
// Tree Implementations
// ========================================================================

impl<'a, A, K> Tree<'a, A, K> {
    /// This tree with the items `selected` selected, the app's selection;
    /// none without it.
    pub fn selected(self, selected: &'a [K]) -> Self {
        Self { selected, ..self }
    }

    /// What runs on the app's state with the item the cursor moved to
    /// (`None` on a loading or failure row). Without it the cursor stays on
    /// the item the app gave.
    pub fn on_cursor_move(mut self, apply: impl Fn(&mut A, Option<K>) + 'static) -> Self {
        self.moves.on_cursor_move = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the items selected, top to
    /// bottom, after the user changed the selection. Without it nothing can
    /// be selected.
    pub fn on_select(mut self, apply: impl Fn(&mut A, Vec<K>) + 'static) -> Self {
        self.moves.on_select = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the count and prefix typed toward
    /// a motion whenever they change, for a status line to show them.
    pub fn on_keys(mut self, apply: impl Fn(&mut A, ListKeys) + 'static) -> Self {
        self.moves.on_keys = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the item the user activated
    /// (Enter, a double click): open it.
    pub fn on_activate(self, apply: impl Fn(&mut A, K) + 'static) -> Self {
        Self {
            on_activate: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with a node the user expanded whose
    /// children the items hold.
    pub fn on_expand(self, apply: impl Fn(&mut A, K) + 'static) -> Self {
        Self {
            on_expand: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with a node the user collapsed.
    pub fn on_collapse(self, apply: impl Fn(&mut A, K) + 'static) -> Self {
        Self {
            on_collapse: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with a node the user expanded whose
    /// children the items lack: load them (often on another thread) and
    /// hand them over with [`TreeItems::set_children`]. A loading row shows
    /// under the node until then.
    pub fn on_load(self, apply: impl Fn(&mut A, K) + 'static) -> Self {
        Self {
            on_load: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with an item the user dragged and
    /// dropped, and the item it was dropped onto (`None` off the items):
    /// file or move it. Without it items cannot be dragged. While one is
    /// dragged the item under the pointer is marked as the drop target;
    /// fixed when the element is first built.
    pub fn on_drop(self, apply: impl Fn(&mut A, TreeDrop<K>) + 'static) -> Self {
        Self {
            on_drop: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state with the item the cursor is on and a
    /// key the tree left unused, while `accepts` says yes to the key: a
    /// key of the app's own on the item (`x` to mark it read). The tree's
    /// own keys (its motions and counts, a prefix and the key completing
    /// or dropping it, Space, Enter) never reach `accepts`; a key it
    /// declines goes on to the containers around the tree and the app, as
    /// without it. On a loading or failure row it hands none.
    pub fn on_row_key(
        self,
        accepts: impl Fn(Key) -> bool + 'a,
        apply: impl Fn(&mut A, K, Key) + 'static,
    ) -> Self {
        Self {
            on_row_key: Some(RowKeys::new(accepts, apply)),
            ..self
        }
    }

    /// This tree setting its labels on the monospace cell grid rather than
    /// in the proportional font labels are set in, the arrows and indents
    /// on the grid either way so they line up by depth; fixed when the
    /// element is first built.
    pub fn monospace(self) -> Self {
        Self {
            proportional: false,
            ..self
        }
    }

    /// This tree in the proportional font, as every tree is unless
    /// [`Tree::monospace`] puts it on the cell grid.
    #[deprecated(
        note = "labels are proportional by default; drop the call, or use `monospace` for the cell grid"
    )]
    pub fn proportional(self) -> Self {
        Self {
            proportional: true,
            ..self
        }
    }
}

impl<A: 'static, K: Clone + Eq + Hash + 'static> Spec<A> for Tree<'_, A, K> {
    type Widget = TreeBox<K>;

    const KIND: &'static str = "tree";

    fn create(&self) -> TreeBox<K> {
        let mut tree = TreeView::new();
        if self.proportional {
            tree = tree.with_proportional_font();
        }
        if self.on_drop.is_some() {
            tree = tree.with_drops_onto();
        }
        TreeBox {
            tree,
            version: None,
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> TreeProps<'b, K> {
        TreeProps {
            cursor: self.cursor,
            items: self.items,
            selected: self.selected,
        }
    }

    fn call(&self, answer: TreeAnswer<K>) -> Option<Call<A>> {
        let event = match answer.response {
            TreeViewResponse::Activated(id) => call(self.on_activate.as_ref(), id),
            TreeViewResponse::Collapsed(id) => call(self.on_collapse.as_ref(), id),
            TreeViewResponse::Expanded(id) => call(self.on_expand.as_ref(), id),
            TreeViewResponse::Load(id) => call(self.on_load.as_ref(), id),
            TreeViewResponse::Dropped(drop) => call(self.on_drop.as_ref(), drop),
            _ => None,
        };
        let row_key = answer.row_key.and_then(|(item, key)| Some((item?, key)));
        let row_key = RowKeys::call(self.on_row_key.as_ref(), row_key);
        self.moves.calls(answer.moved, event.or(row_key))
    }

    fn draw(&self, widget: &mut TreeBox<K>, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.tree.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &TreeBox<K>, frame: &mut Frame, id: FocusId) {
        widget.tree.access(frame, id);
    }

    fn act(
        &self,
        _widget: &mut TreeBox<K>,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<TreeAnswer<K>> {
        None
    }

    fn handle(
        &self,
        widget: &mut TreeBox<K>,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> TreeAnswer<K> {
        let tree = &mut widget.tree;
        let cursor = tree.cursor().cloned();
        let selected = owned(tree.selected());
        let keys = tree.keys().clone();
        let response = tree.handle(input, cx);
        let after = tree.cursor().cloned();
        let now = owned(tree.selected());
        let row_key = RowKeys::take(self.on_row_key.as_ref(), &response, input, after.clone())
            .map(|(item, key)| (Some(item), key));
        let moved = Moved {
            cursor: (after != cursor).then_some(after),
            keys: (*tree.keys() != keys).then(|| tree.keys().clone()),
            selected: (now != selected).then_some(now),
        };
        Answered {
            moved,
            response,
            row_key,
        }
    }

    fn grows(&self) -> bool {
        true
    }

    fn scrolls_itself(&self) -> bool {
        true
    }
}

impl<'a, A: 'static, K: Clone + Eq + Hash + 'static> From<Tree<'a, A, K>> for Element<'a, A> {
    fn from(tree: Tree<'a, A, K>) -> Self {
        Element::new(Kind::Control(Box::new(tree)))
    }
}

// ========================================================================
// TreeBox Implementations
// ========================================================================

impl<K: Clone + Eq + Hash + 'static> Widget for TreeBox<K> {
    type Output = ();
    type Response = TreeAnswer<K>;

    /// Hands `input` to the tree view, which keeps the items it was given.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TreeAnswer<K> {
        Answered::ignored(self.tree.handle(input, cx))
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.tree.draw(frame, area, theme);
    }

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.tree.about_to_wait(cx)
    }
}

impl<K: Clone + Eq + Hash + 'static> Control for TreeBox<K> {
    type Props<'a> = TreeProps<'a, K>;
    type Value = TreeAnswer<K>;

    /// Copies the items when their version moved, then takes the app's
    /// cursor and selection where they differ from the tree's.
    fn sync(&mut self, props: TreeProps<'_, K>) {
        let version = props.items.version();
        if self.version != Some(version) {
            self.tree.set_items(props.items);
            self.version = Some(version);
        }
        if let Some(cursor) = props.cursor
            && self.tree.cursor() != Some(cursor)
        {
            self.tree.set_cursor_on(cursor);
        }
        let selected = self.tree.selected();
        if selected.len() != props.selected.len()
            || !selected.iter().all(|id| props.selected.contains(id))
        {
            self.tree.set_selected_ids(props.selected);
        }
    }

    /// Nothing: a tree takes the height its column leaves.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, answer: TreeAnswer<K>) -> Option<TreeAnswer<K>> {
        answer.reports(|response| {
            matches!(
                response,
                TreeViewResponse::Activated(_)
                    | TreeViewResponse::Collapsed(_)
                    | TreeViewResponse::Expanded(_)
                    | TreeViewResponse::Load(_)
                    | TreeViewResponse::Dropped(_)
            )
        })
    }
}

// ========================================================================
// Grid Implementations
// ========================================================================

impl<'a, A, S: GridSource + ?Sized> Grid<'a, A, S> {
    /// This grid with the items `selected` selected, the app's selection;
    /// none without it.
    pub fn selected(self, selected: &'a BTreeSet<usize>) -> Self {
        Self { selected, ..self }
    }

    /// What runs on the app's state with the item the cursor moved to.
    /// Without it the cursor stays on the item the app gave.
    pub fn on_cursor_move(mut self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        self.moves.on_cursor_move = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the items selected after the user
    /// changed the selection. Without it nothing can be selected.
    pub fn on_select(mut self, apply: impl Fn(&mut A, BTreeSet<usize>) + 'static) -> Self {
        self.moves.on_select = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the count and prefix typed toward
    /// a motion whenever they change, for a status line to show them.
    pub fn on_keys(mut self, apply: impl Fn(&mut A, ListKeys) + 'static) -> Self {
        self.moves.on_keys = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state with the item the user activated
    /// (Enter, a double click): open it.
    pub fn on_activate(self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        Self {
            on_activate: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static, S: GridSource + ?Sized> Spec<A> for Grid<'_, A, S> {
    type Widget = GridBox<S::Key>;

    const KIND: &'static str = "icon grid";

    fn create(&self) -> GridBox<S::Key> {
        GridBox {
            grid: IconGrid::new(),
            thumbnails: (self.make_thumbnails)(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> RowsProps<'b> {
        RowsProps {
            count: self.items.item_count(),
            cursor: self.cursor,
            selected: self.selected,
        }
    }

    fn call(&self, answer: GridAnswer) -> Option<Call<A>> {
        let event = match answer.response {
            IconGridResponse::Activated(index) => call(self.on_activate.as_ref(), index),
            _ => None,
        };
        self.moves.calls(answer.moved, event)
    }

    fn draw(
        &self,
        widget: &mut GridBox<S::Key>,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        let GridBox { grid, thumbnails } = widget;
        grid.lend(self.items, thumbnails).draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &GridBox<S::Key>, frame: &mut Frame, id: FocusId) {
        widget.grid.access(frame, id, self.items);
    }

    fn act(
        &self,
        _widget: &mut GridBox<S::Key>,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<GridAnswer> {
        None
    }

    fn handle(
        &self,
        widget: &mut GridBox<S::Key>,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> GridAnswer {
        let GridBox { grid, thumbnails } = widget;
        let cursor = grid.cursor();
        let selected: BTreeSet<usize> = grid.selected().collect();
        let keys = grid.keys().clone();
        let response = grid.lend(self.items, thumbnails).handle(input, cx);
        let now: BTreeSet<usize> = grid.selected().collect();
        let moved = Moved {
            cursor: (grid.cursor() != cursor).then(|| grid.cursor()),
            keys: (*grid.keys() != keys).then(|| grid.keys().clone()),
            selected: (now != selected).then_some(now),
        };
        Answered {
            moved,
            response,
            row_key: None,
        }
    }

    fn grows(&self) -> bool {
        true
    }

    fn scrolls_itself(&self) -> bool {
        true
    }
}

impl<'a, A: 'static, S: GridSource + ?Sized> From<Grid<'a, A, S>> for Element<'a, A> {
    fn from(grid: Grid<'a, A, S>) -> Self {
        Element::new(Kind::Control(Box::new(grid)))
    }
}

// ========================================================================
// GridBox Implementations
// ========================================================================

impl<K: Clone + Eq + Hash + Send + 'static> Widget for GridBox<K> {
    type Output = ();
    type Response = GridAnswer;

    /// Nothing: the [`Grid`] element hands its widget inputs lent the
    /// app's items.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) -> GridAnswer {
        Answered::ignored(IconGridResponse::Ignored)
    }

    /// Nothing: the [`Grid`] element draws its widget lent the app's items.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    /// Takes in the thumbnails the workers loaded, asking for a redraw when
    /// any came, and steps the glide.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        if self.thumbnails.poll() {
            cx.request_redraw();
        }
        let items = NoItems(PhantomData);
        self.grid
            .lend(&items, &mut self.thumbnails)
            .about_to_wait(cx)
    }
}

impl<K: Clone + Eq + Hash + Send + 'static> Control for GridBox<K> {
    type Props<'a> = RowsProps<'a>;
    type Value = GridAnswer;

    fn sync(&mut self, props: RowsProps<'_>) {
        let cursor = props.cursor.min(props.count.saturating_sub(1));
        if self.grid.cursor() != cursor && props.count > 0 {
            self.grid.set_cursor(cursor);
        }
        if !self.grid.selected().eq(props.selected.iter().copied()) {
            self.grid.set_selected(props.selected.iter().copied());
        }
    }

    /// Nothing: a grid takes the height its column leaves.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, answer: GridAnswer) -> Option<GridAnswer> {
        answer.reports(|response| matches!(response, IconGridResponse::Activated(_)))
    }
}

// ========================================================================
// Cells Implementations
// ========================================================================

impl<S: ?Sized> Cells<'_, S> {
    /// These cells with their cursor drawn steady rather than blinking.
    pub fn steady(self) -> Self {
        Self {
            blinks: false,
            ..self
        }
    }
}

impl<A, S: CellSource + ?Sized> Spec<A> for Cells<'_, S> {
    type Widget = CellsBox;

    const KIND: &'static str = "cells";

    fn create(&self) -> CellsBox {
        CellsBox {
            blinks: self.blinks,
            cursor: self.source.cursor(),
            interval: Theme::default().motion.caret_blink,
            rows: self.source.rows(),
            since: clock::now(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> CellsProps {
        CellsProps {
            blinks: self.blinks,
            cursor: self.source.cursor(),
            rows: self.source.rows(),
        }
    }

    fn call(&self, _value: ()) -> Option<Call<A>> {
        None
    }

    /// Draws the cells from `rect`'s top left, cut to it, the cursor
    /// hidden while it blinks off.
    fn draw(&self, widget: &mut CellsBox, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        widget.interval = theme.motion.caret_blink;
        let source = Blinked {
            cursor: widget
                .shown(clock::now())
                .then_some(widget.cursor)
                .flatten(),
            source: self.source,
        };
        frame.clipped(rect, |frame| {
            frame.draw_cells(&source, Point::new(rect.x, rect.y), theme);
        });
        rect.into()
    }

    /// Nothing: a grid of cells is described by the app that knows what
    /// it shows.
    fn describe(&self, _widget: &CellsBox, _frame: &mut Frame, _id: FocusId) {}

    fn act(&self, _widget: &mut CellsBox, _id: FocusId, _action: AccessAction) -> Option<()> {
        None
    }

    fn is_focusable(&self) -> bool {
        false
    }
}

impl<'a, A, S: CellSource + ?Sized> From<Cells<'a, S>> for Element<'a, A> {
    fn from(cells: Cells<'a, S>) -> Self {
        Element::new(Kind::Control(Box::new(cells)))
    }
}

// ========================================================================
// CellsBox Implementations
// ========================================================================

impl CellsBox {
    /// Whether the cursor shows at `now`: always when steady, else in
    /// every other interval since it last started to show.
    fn shown(&self, now: Instant) -> bool {
        if !self.blinks || self.interval.is_zero() {
            return true;
        }
        let flips = now.saturating_duration_since(self.since).as_millis()
            / self.interval.as_millis().max(1);
        flips.is_multiple_of(2)
    }

    /// When the cursor next shows or hides after `now`.
    fn next_flip(&self, now: Instant) -> Instant {
        let interval = self.interval.as_millis().max(1);
        let flips = now.saturating_duration_since(self.since).as_millis() / interval + 1;
        let flips = u32::try_from(flips).unwrap_or(u32::MAX);
        self.since + self.interval * flips
    }
}

impl Widget for CellsBox {
    type Output = ();
    type Response = ();

    /// Nothing: the cells take no input.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) {}

    /// Nothing: the [`Cells`] element draws the app's cells.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    /// Asks for a redraw when the blinking cursor next shows or hides.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        if !self.blinks || self.cursor.is_none() || self.interval.is_zero() {
            return None;
        }
        let next = self.next_flip(clock::now());
        cx.request_redraw_at(next);
        Some(next)
    }
}

impl Control for CellsBox {
    type Props<'a> = CellsProps;
    type Value = ();

    /// Starts the blink over, showing, when the cursor moved or changed
    /// shape.
    fn sync(&mut self, props: CellsProps) {
        if props.cursor != self.cursor {
            self.cursor = props.cursor;
            self.since = clock::now();
        }
        self.blinks = props.blinks;
        self.rows = props.rows;
    }

    /// As tall as the source's rows.
    fn measure(&self, _width: f32, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        self.rows as f32 * metrics.height
    }

    fn changed(&self, (): ()) -> Option<()> {
        None
    }
}

// ========================================================================
// Blinked Implementations
// ========================================================================

impl<S: CellSource + ?Sized> CellSource for Blinked<'_, S> {
    fn cols(&self) -> usize {
        self.source.cols()
    }

    fn rows(&self) -> usize {
        self.source.rows()
    }

    fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.source.cell(row, col)
    }

    fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }

    fn blink_visible(&self) -> bool {
        self.source.blink_visible()
    }

    fn highlights(&self) -> &[CellHighlight] {
        self.source.highlights()
    }
}

// ========================================================================
// Moves Implementations
// ========================================================================

impl<A, C, S> Default for Moves<A, C, S> {
    fn default() -> Self {
        Self {
            on_cursor_move: None,
            on_keys: None,
            on_select: None,
        }
    }
}

impl<A: 'static, C: 'static, S: 'static> Moves<A, C, S> {
    /// The call running the callbacks of what `moved`, then `event`'s.
    fn calls(&self, moved: Moved<C, S>, event: Option<Call<A>>) -> Option<Call<A>> {
        let calls: Vec<Call<A>> = [
            moved
                .cursor
                .and_then(|cursor| call(self.on_cursor_move.as_ref(), cursor)),
            moved
                .selected
                .and_then(|selected| call(self.on_select.as_ref(), selected)),
            moved
                .keys
                .and_then(|keys| call(self.on_keys.as_ref(), keys)),
            event,
        ]
        .into_iter()
        .flatten()
        .collect();
        if calls.is_empty() {
            return None;
        }
        Some(Box::new(move |app: &mut A| {
            for call in calls {
                call(app);
            }
        }))
    }
}

// ========================================================================
// Answered Implementations
// ========================================================================

impl<R, C, S> Answered<R, C, S> {
    /// `response`, with nothing moved.
    fn ignored(response: R) -> Self {
        Self {
            moved: Moved {
                cursor: None,
                keys: None,
                selected: None,
            },
            response,
            row_key: None,
        }
    }

    /// This answer, when something moved or `reports` says the response is
    /// one the element has a callback for.
    fn reports(self, reports: impl FnOnce(&R) -> bool) -> Option<Self> {
        let moved = &self.moved;
        let any = moved.cursor.is_some() || moved.keys.is_some() || moved.selected.is_some();
        (any || self.row_key.is_some() || reports(&self.response)).then_some(self)
    }
}

impl<R: Outcome, C, S> Outcome for Answered<R, C, S> {
    /// Whether the widget left the input unused and the element handed
    /// the app no key of its own either.
    fn is_ignored(&self) -> bool {
        self.response.is_ignored() && self.row_key.is_none()
    }
}

// ========================================================================
// RowKeys Implementations
// ========================================================================

impl<'a, A, R> RowKeys<'a, A, R> {
    /// The keys `accepts` takes, running `apply` with the row and the key.
    fn new(accepts: impl Fn(Key) -> bool + 'a, apply: impl Fn(&mut A, R, Key) + 'static) -> Self {
        Self {
            accepts: Box::new(accepts),
            apply: Rc::new(move |app: &mut A, (row, key)| apply(app, row, key)),
        }
    }

    /// The cursor's `row` and the key of `input` that `keys` hand the app:
    /// a key the widget left unused (its `response` ignored) that
    /// `accepts` takes, on a row the cursor is on.
    fn take(
        keys: Option<&Self>,
        response: &impl Outcome,
        input: &Input,
        row: Option<R>,
    ) -> Option<(R, Key)> {
        let (Some(keys), Input::Key(key)) = (keys, input) else {
            return None;
        };
        if !response.is_ignored() || !(keys.accepts)(*key) {
            return None;
        }
        Some((row?, *key))
    }
}

impl<A: 'static, R: 'static> RowKeys<'_, A, R> {
    /// The call running `keys`' `apply` with the row and key handed over.
    fn call(keys: Option<&Self>, row_key: Option<(R, Key)>) -> Option<Call<A>> {
        call(Some(&keys?.apply), row_key?)
    }
}

// ========================================================================
// RowsProps Implementations
// ========================================================================

impl RowsProps<'_> {
    /// Brings `state` in step with the app's rows: their count when it
    /// changed, then the app's cursor and selection where they differ.
    fn sync(self, state: &mut ListState) {
        if state.count() != self.count {
            state.set_count(self.count);
        }
        let cursor = self.cursor.min(self.count.saturating_sub(1));
        if state.cursor() != cursor {
            state.set_cursor(cursor);
        }
        if !state.selected().eq(self.selected.iter().copied()) {
            state.set_selected(self.selected.iter().copied());
        }
    }
}

// ========================================================================
// Watch Implementations
// ========================================================================

/// A list-like widget's cursor, selection, and keys before an input.
struct Watch {
    /// The cursor.
    cursor: usize,
    /// The keys typed.
    keys: ListKeys,
    /// The selection.
    selected: BTreeSet<usize>,
}

impl Watch {
    /// What `state` and `keys` are now.
    fn of(state: &ListState, keys: &ListKeys) -> Self {
        Self {
            cursor: state.cursor(),
            keys: keys.clone(),
            selected: state.selected().collect(),
        }
    }

    /// What moved from this to `state` and `keys`.
    fn moved(self, state: &ListState, keys: &ListKeys) -> Moved<usize, BTreeSet<usize>> {
        let selected = !state.selected().eq(self.selected.iter().copied());
        Moved {
            cursor: (state.cursor() != self.cursor).then(|| state.cursor()),
            keys: (*keys != self.keys).then(|| keys.clone()),
            selected: selected.then(|| state.selected().collect()),
        }
    }
}

// ========================================================================
// NoRows and NoItems Implementations
// ========================================================================

impl TableSource for NoRows {
    fn row_count(&self) -> usize {
        0
    }

    fn cell_text(&self, _row: usize, _column: usize) -> Cow<'_, str> {
        Cow::Borrowed("")
    }
}

impl<K: Clone + Eq + Hash + Send + 'static> GridSource for NoItems<K> {
    type Key = K;

    fn item_count(&self) -> usize {
        0
    }

    fn item_key(&self, index: usize) -> K {
        unreachable!("a grid of no items asked for the key of item {index}")
    }

    fn item_label(&self, _index: usize) -> Cow<'_, str> {
        Cow::Borrowed("")
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A list of the app's `rows`, its cursor on row `cursor`, as the
/// [`List`](crate::List) widget draws it: only the rows in sight are asked
/// for, so a list of a million rows costs what one of twenty does. Its
/// keys are the Vim list keys the widget documents (`j`, `k`, `C-f`, `g g`,
/// `5j`, Space to select, Enter to activate); the pointer clicks, drags
/// rows, draws rubber bands, and scrolls.
///
/// ```
/// use std::borrow::Cow;
/// use std::collections::BTreeSet;
///
/// use misty::view::{column, list, text};
/// use misty::{App, Element, ListSource, Theme, WindowId};
///
/// struct Rows(Vec<String>);
///
/// impl ListSource for Rows {
///     fn row_count(&self) -> usize {
///         self.0.len()
///     }
///
///     fn row_text(&self, row: usize) -> Cow<'_, str> {
///         Cow::Borrowed(&self.0[row])
///     }
/// }
///
/// struct Names {
///     cursor: usize,
///     names: Rows,
///     opened: Option<usize>,
///     selected: BTreeSet<usize>,
///     theme: Theme,
/// }
///
/// impl App for Names {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         column([
///             list(&self.names, self.cursor)
///                 .selected(&self.selected)
///                 .on_cursor_move(|app: &mut Self, row| app.cursor = row)
///                 .on_select(|app: &mut Self, rows| app.selected = rows)
///                 .on_activate(|app: &mut Self, row| app.opened = Some(row))
///                 .into(),
///             text(format!("{} selected", self.selected.len())).muted().into(),
///         ])
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
/// ```
pub fn list<'a, A, S: ListSource + ?Sized>(rows: &'a S, cursor: usize) -> List<'a, A, S> {
    List {
        cursor,
        moves: Moves::default(),
        on_activate: None,
        on_drop: None,
        on_row_key: None,
        proportional: true,
        rows,
        selected: &NO_ROWS,
        which_key: false,
    }
}

/// A table of the app's `rows` in `columns`, under a header naming each,
/// its cursor on row `cursor`, as the [`Table`](crate::Table) widget draws
/// it. Its rows take a list's keys and pointer; dragging a border between
/// titles resizes a column, a click on a title asks to sort by it
/// ([`Table::on_sort`]), and columns wider than the table scroll sideways.
pub fn table<'a, A, S: TableSource + ?Sized>(
    rows: &'a S,
    columns: &'a [TableColumn],
    cursor: usize,
) -> Table<'a, A, S> {
    Table {
        cell_cursor: None,
        columns,
        cursor,
        frozen: (0, 0),
        moves: Moves::default(),
        on_activate: None,
        on_cell_cursor_move: None,
        on_drop: None,
        on_header_menu: None,
        on_resize: None,
        on_row_key: None,
        on_sort: None,
        proportional: true,
        rows,
        selected: &NO_ROWS,
        sort: None,
        zoom: 1.0,
    }
}

/// A tree of the app's `items`, its cursor on the item `cursor` (`None`
/// leaves it where it is, on the first row at first), as the
/// [`TreeView`](crate::TreeView) widget draws it. Right (`l`) expands a
/// node, Left (`h`) collapses it, and the rest are a list's keys; a click
/// on a node's arrow expands or collapses it. A node whose children the
/// items lack asks for them through [`Tree::on_load`]. The items are copied
/// into the tree only when they changed, which their version tells.
pub fn tree_view<'a, A, K>(items: &'a TreeItems<K>, cursor: Option<&'a K>) -> Tree<'a, A, K> {
    Tree {
        cursor,
        items,
        moves: Moves::default(),
        on_activate: None,
        on_collapse: None,
        on_expand: None,
        on_drop: None,
        on_load: None,
        on_row_key: None,
        proportional: true,
        selected: &[],
    }
}

/// A grid of the app's `items` as thumbnails with labels, as many columns
/// as the width holds, its cursor on item `cursor`, as the
/// [`IconGrid`](crate::IconGrid) widget draws it. The thumbnails load in
/// the cache `thumbnails` makes, called once, the first time the element
/// is built (it takes the [`Waker`](crate::Waker) the app was started
/// with); misty takes in what its workers loaded. The arrows move over the
/// cells, `+` and `-` zoom, and the rest are a list's keys.
pub fn icon_grid<'a, A, S: GridSource + ?Sized>(
    items: &'a S,
    cursor: usize,
    thumbnails: impl Fn() -> Thumbnails<S::Key> + 'a,
) -> Grid<'a, A, S> {
    Grid {
        cursor,
        items,
        make_thumbnails: Box::new(thumbnails),
        moves: Moves::default(),
        on_activate: None,
        selected: &NO_ROWS,
    }
}

/// The app's grid of styled cells in `source`, drawn on the cell grid
/// from the element's top left as a terminal's screen is, as tall as its
/// rows; its cursor blinks at the theme's caret rate, starting over each
/// time it moves, unless made [`Cells::steady`]. It takes no input and is
/// not described to screen readers.
pub fn cells<S: CellSource + ?Sized>(source: &S) -> Cells<'_, S> {
    Cells {
        blinks: true,
        source,
    }
}

/// The row the cursor of `state` is on, or `None` with no rows.
fn cursor_row(state: &ListState) -> Option<usize> {
    (state.count() > 0).then(|| state.cursor())
}

/// The ids in `ids`, owned.
fn owned<K: Clone>(ids: Vec<&K>) -> Vec<K> {
    ids.into_iter().cloned().collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use accesskit::Role;

    use super::*;
    use crate::base::cell::{CellBuffer, CursorShape};
    use crate::base::key::{KeyCode, KeyScheme};
    use crate::platform::window_id::WindowId;
    use crate::testing::{TreeWindow, access_tree};
    use crate::view::app::{App, AppCx};
    use crate::view::element::{column, text};
    use crate::widgets::tree_view::TreeItem;

    /// Rows named by their number.
    struct Rows(usize);

    impl ListSource for Rows {
        fn row_count(&self) -> usize {
            self.0
        }

        fn row_text(&self, row: usize) -> Cow<'_, str> {
            Cow::Owned(format!("row {row}"))
        }
    }

    impl TableSource for Rows {
        fn row_count(&self) -> usize {
            self.0
        }

        fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str> {
            Cow::Owned(format!("r{row}c{column}"))
        }
    }

    /// An app whose tree each test picks, recording what reaches it.
    struct Probe {
        /// The row activated last.
        activated: Option<usize>,
        /// The keys no element took, as `App::key` saw them.
        app_keys: Vec<KeyCode>,
        /// The column of the table's cell cursor.
        cell_column: usize,
        /// The table's columns.
        columns: Vec<TableColumn>,
        /// The cursor of the list or table.
        cursor: usize,
        /// The keys typed toward a motion, as last reported.
        keys: ListKeys,
        /// The nodes the tree asked to load.
        loads: Vec<PathBuf>,
        /// The rows and keys a list or table handed over.
        row_keys: Vec<(usize, KeyCode)>,
        /// The rows.
        rows: Rows,
        /// The selection of the list or table.
        selected: BTreeSet<usize>,
        /// The sort the table asked for last.
        sort: Option<SortKey>,
        /// The colors it is drawn in.
        theme: Theme,
        /// The items the tree dropped onto others.
        tree_drops: Vec<TreeDrop<PathBuf>>,
        /// The tree's cursor.
        tree_cursor: Option<PathBuf>,
        /// The tree's items.
        tree_items: TreeItems<PathBuf>,
        /// The items and keys the tree handed over.
        tree_keys: Vec<(PathBuf, KeyCode)>,
        /// The tree's selection.
        tree_selected: Vec<PathBuf>,
        /// The test's tree.
        view: fn(&Probe) -> Element<'_, Probe>,
        /// The widths a resize reported.
        widths: Option<(usize, Vec<f32>)>,
    }

    impl App for Probe {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            (self.view)(self)
        }

        fn key(&mut self, key: Key, _cx: &mut AppCx<'_>) -> bool {
            self.app_keys.push(key.code);
            true
        }

        fn key_scheme(&self) -> KeyScheme {
            KeyScheme::VIM_AND_EMACS
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window 400 by 300 pixels showing `view` over 1000 rows.
    fn window(view: fn(&Probe) -> Element<'_, Probe>) -> TreeWindow<Probe> {
        let root = PathBuf::from("/root");
        let probe = Probe {
            activated: None,
            app_keys: Vec::new(),
            cell_column: 0,
            columns: vec![
                TableColumn::new("Name", 120.0),
                TableColumn::new("Size", 80.0),
            ],
            cursor: 0,
            keys: ListKeys::default(),
            loads: Vec::new(),
            row_keys: Vec::new(),
            rows: Rows(1000),
            selected: BTreeSet::new(),
            sort: None,
            theme: Theme::dark(),
            tree_cursor: Some(root.clone()),
            tree_drops: Vec::new(),
            tree_items: TreeItems::new(vec![TreeItem::branch(root, "root")]),
            tree_keys: Vec::new(),
            tree_selected: Vec::new(),
            view,
            widths: None,
        };
        TreeWindow::new(probe, "t", 400, 300)
    }

    /// A list over the probe's rows, every callback kept in the probe.
    fn full_list(probe: &Probe) -> Element<'_, Probe> {
        column([
            list(&probe.rows, probe.cursor)
                .monospace()
                .selected(&probe.selected)
                .on_cursor_move(|probe: &mut Probe, row| probe.cursor = row)
                .on_select(|probe: &mut Probe, rows| probe.selected = rows)
                .on_keys(|probe: &mut Probe, keys| probe.keys = keys)
                .on_activate(|probe: &mut Probe, row| probe.activated = Some(row))
                .into(),
            text("status").monospace().into(),
        ])
    }

    /// A table over the probe's rows in its columns, sorted as it says.
    fn full_table(probe: &Probe) -> Element<'_, Probe> {
        column([table(&probe.rows, &probe.columns, probe.cursor)
            .monospace()
            .selected(&probe.selected)
            .sort(probe.sort)
            .on_cursor_move(|probe: &mut Probe, row| probe.cursor = row)
            .on_sort(|probe: &mut Probe, sort| probe.sort = Some(sort))
            .on_resize(|probe: &mut Probe, widths| probe.widths = Some(widths))
            .into()])
    }

    /// A tree over the probe's items.
    fn full_tree(probe: &Probe) -> Element<'_, Probe> {
        column([tree_view(&probe.tree_items, probe.tree_cursor.as_ref())
            .monospace()
            .selected(&probe.tree_selected)
            .on_cursor_move(|probe: &mut Probe, id| probe.tree_cursor = id)
            .on_select(|probe: &mut Probe, ids| probe.tree_selected = ids)
            .on_load(|probe: &mut Probe, id| probe.loads.push(id))
            .into()])
    }

    /// The probe's tree, its items dragged onto others.
    fn dropping_tree(probe: &Probe) -> Element<'_, Probe> {
        column([tree_view(&probe.tree_items, probe.tree_cursor.as_ref())
            .monospace()
            .on_cursor_move(|probe: &mut Probe, id| probe.tree_cursor = id)
            .on_drop(|probe: &mut Probe, drop| probe.tree_drops.push(drop))
            .into()])
    }

    /// Whether `key` is `x`, the probe's key of its own on a row.
    fn is_x(key: Key) -> bool {
        key.code == KeyCode::Char('x')
    }

    /// A list over the probe's rows handing it `x` on a row.
    fn keyed_list(probe: &Probe) -> Element<'_, Probe> {
        column([list(&probe.rows, probe.cursor)
            .monospace()
            .on_cursor_move(|probe: &mut Probe, row| probe.cursor = row)
            .on_keys(|probe: &mut Probe, keys| probe.keys = keys)
            .on_row_key(
                |key| is_x(key) || key.code == KeyCode::Char('j'),
                |probe: &mut Probe, row, key| probe.row_keys.push((row, key.code)),
            )
            .into()])
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

    #[test]
    fn test_a_list_moves_selects_and_activates_through_the_apps_state() {
        let mut window = window(full_list);
        window.press("j j <space> j");
        assert_eq!(window.app().cursor, 3);
        assert_eq!(window.app().selected, BTreeSet::from([2]));
        assert_eq!(focused(&window), "ListItem \"row 3\" focused");
        window.press("RET");
        assert_eq!(window.app().activated, Some(3));
        window.press("1 0");
        assert_eq!(
            window.app().keys.count(),
            Some(10),
            "a pending count is reported"
        );
        window.press("G");
        assert_eq!(window.app().cursor, 9, "the count named the row");
        assert_eq!(window.app().keys.count(), None);
    }

    #[test]
    fn test_a_list_hands_the_app_its_keys_on_the_row_and_keeps_its_own() {
        let mut window = window(keyed_list);
        window.press("j x");
        assert_eq!(window.app().cursor, 1, "j accepted is still the list's");
        assert_eq!(window.app().row_keys, [(1, KeyCode::Char('x'))]);
        window.press("g x y");
        assert_eq!(
            window.app().row_keys.len(),
            1,
            "a dropped prefix swallows x"
        );
        assert_eq!(window.app().app_keys, [KeyCode::Char('y')], "declined");
        window.press("5 x");
        assert_eq!(window.app().row_keys[1], (1, KeyCode::Char('x')));
        assert_eq!(
            window.app().keys.count(),
            None,
            "the dropped count is reported"
        );
        window.update(|probe| probe.rows = Rows(0));
        window.press("x");
        assert_eq!(window.app().row_keys.len(), 2, "no row, no key handed");
        assert_eq!(window.app().app_keys.last(), Some(&KeyCode::Char('x')));
    }

    #[test]
    fn test_a_table_and_a_tree_hand_the_app_its_keys_on_the_cursors_row() {
        let mut tabled = window(|probe| {
            column([table(&probe.rows, &probe.columns, probe.cursor)
                .monospace()
                .on_cursor_move(|probe: &mut Probe, row| probe.cursor = row)
                .on_row_key(is_x, |probe: &mut Probe, row, key| {
                    probe.row_keys.push((row, key.code));
                })
                .into()])
        });
        tabled.press("j j x");
        assert_eq!(tabled.app().row_keys, [(2, KeyCode::Char('x'))]);
        assert!(tabled.app().app_keys.is_empty());
        let mut window = window(|probe| {
            column([tree_view(&probe.tree_items, probe.tree_cursor.as_ref())
                .monospace()
                .on_cursor_move(|probe: &mut Probe, id| probe.tree_cursor = id)
                .on_row_key(is_x, |probe: &mut Probe, item, key| {
                    probe.tree_keys.push((item, key.code));
                })
                .into()])
        });
        window.press("x l");
        assert_eq!(
            window.app().tree_keys,
            [(PathBuf::from("/root"), KeyCode::Char('x'))]
        );
        window.press("j x");
        assert_eq!(window.app().tree_keys.len(), 1, "a loading row hands none");
        assert_eq!(window.app().app_keys, [KeyCode::Char('x')]);
    }

    #[test]
    fn test_the_app_moves_a_lists_cursor_and_selection_by_changing_its_state() {
        let mut window = window(full_list);
        window.update(|probe| {
            probe.cursor = 500;
            probe.selected = BTreeSet::from([500, 501]);
        });
        let text = window.ui_text();
        assert!(
            text.contains("ListItem \"row 500\" selected focused"),
            "{text}"
        );
        assert!(text.contains("ListItem \"row 501\" selected"), "{text}");
        window.press("k");
        assert_eq!(
            window.app().cursor,
            499,
            "the user moves on from where the app put it"
        );
    }

    #[test]
    fn test_a_list_without_callbacks_keeps_the_apps_cursor_and_selects_nothing() {
        let mut window =
            window(|probe| column([list(&probe.rows, probe.cursor).monospace().into()]));
        window.press("j j <space>");
        assert_eq!(focused(&window), "ListItem \"row 0\" focused");
        assert!(
            !window.ui_text().contains("selected"),
            "{}",
            window.ui_text()
        );
    }

    #[test]
    fn test_a_list_takes_the_height_its_column_leaves_above_the_status_line() {
        let window = window(full_list);
        assert_eq!(
            window.drawn_text().last().map(String::as_str),
            Some("status")
        );
        let tree = access_tree(window.frame(), "t");
        let (_, list) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::List)
            .expect("the list is described");
        let bounds = list.bounds().expect("the list has bounds");
        let metrics = window.frame().cell_metrics();
        assert_eq!(
            (bounds.y0, bounds.y1),
            (0.0, f64::from(300.0 - metrics.height))
        );
    }

    #[test]
    fn test_a_table_asks_the_app_to_sort_and_marks_the_sort_it_is_given() {
        let mut window = window(full_table);
        window.click(Role::ColumnHeader, "Size");
        let sort = window.app().sort.expect("the click asked to sort");
        assert_eq!(sort.column, 1);
        window.click(Role::ColumnHeader, "Size");
        assert_ne!(
            window.app().sort,
            Some(sort),
            "a second click flips the direction"
        );
        window.press("j");
        assert_eq!(window.app().cursor, 1);
    }

    #[test]
    fn test_a_tables_cell_cursor_moves_with_its_keys_and_tells_the_app() {
        let mut window = window(|probe| {
            column([table(&probe.rows, &probe.columns, probe.cursor)
                .monospace()
                .cell_cursor(probe.cell_column)
                .frozen(1, 0)
                .on_cell_cursor_move(|probe: &mut Probe, column| probe.cell_column = column)
                .on_cursor_move(|probe: &mut Probe, row| probe.cursor = row)
                .into()])
        });
        window.press("l j");
        assert_eq!((window.app().cell_column, window.app().cursor), (1, 1));
        window.press("h");
        assert_eq!(window.app().cell_column, 0);
    }

    #[test]
    fn test_a_table_hides_a_column_the_app_hides_and_keeps_the_others() {
        let mut window = window(full_table);
        assert!(window.ui_text().contains("ColumnHeader \"Size\""));
        window.update(|probe| probe.columns[1] = probe.columns[1].clone().with_hidden(true));
        let text = window.ui_text();
        assert!(!text.contains("ColumnHeader \"Size\""), "{text}");
        assert!(text.contains("ColumnHeader \"Name\""), "{text}");
    }

    /// The texts `window` drew shaped in the proportional font.
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
    fn test_a_proportional_table_and_tree_shape_their_rows_and_others_keep_cells() {
        let shapes = |view: fn(&Probe) -> Element<'_, Probe>, text: &str| {
            shaped(&window(view)).iter().any(|shown| shown == text)
        };
        let proportional_table: fn(&Probe) -> Element<'_, Probe> =
            |probe| column([table(&probe.rows, &probe.columns, 0).into()]);
        let proportional_tree: fn(&Probe) -> Element<'_, Probe> =
            |probe| column([tree_view(&probe.tree_items, None).into()]);
        assert!(shapes(proportional_table, "r0c0"));
        assert!(!shapes(full_table, "r0c0"), "a table keeps the cell grid");
        assert!(shapes(proportional_tree, "root"));
        assert!(!shapes(full_tree, "root"), "a tree keeps the cell grid");
    }

    #[test]
    fn test_a_tree_asks_the_app_to_load_and_shows_what_it_hands_over() {
        let mut window = window(full_tree);
        window.press("l");
        assert_eq!(window.app().loads, [PathBuf::from("/root")]);
        assert!(
            window.ui_text().contains("TreeItem \"Loading…\""),
            "{}",
            window.ui_text()
        );
        window.update(|probe| {
            let children = vec![
                TreeItem::leaf(PathBuf::from("/root/a"), "a"),
                TreeItem::leaf(PathBuf::from("/root/b"), "b"),
            ];
            probe
                .tree_items
                .set_children(&PathBuf::from("/root"), children);
        });
        window.press("j j <space>");
        assert_eq!(window.app().tree_cursor, Some(PathBuf::from("/root/b")));
        assert_eq!(window.app().tree_selected, [PathBuf::from("/root/b")]);
        let text = window.ui_text();
        assert!(text.contains("TreeItem \"b\" selected focused"), "{text}");
        window.update(|probe| probe.tree_cursor = Some(PathBuf::from("/root/a")));
        assert!(window.ui_text().contains("TreeItem \"a\" focused"));
    }

    #[test]
    fn test_a_tree_item_dragged_onto_another_is_dropped_onto_it() {
        let mut window = window(dropping_tree);
        window.update(|probe| {
            let children = vec![
                TreeItem::leaf(PathBuf::from("/root/a"), "a"),
                TreeItem::leaf(PathBuf::from("/root/b"), "b"),
                TreeItem::leaf(PathBuf::from("/root/c"), "c"),
            ];
            let root = PathBuf::from("/root");
            probe.tree_items.set_children(&root, children);
        });
        window.press("l");
        let (from, to) = (
            window.center(Role::TreeItem, "a"),
            window.center(Role::TreeItem, "c"),
        );
        window.drag(from, to);
        assert_eq!(
            window.app().tree_drops,
            [TreeDrop {
                dragged: PathBuf::from("/root/a"),
                onto: Some(PathBuf::from("/root/c")),
            }]
        );
    }

    #[test]
    fn test_a_tree_reloads_an_expanded_node_keeping_it_expanded() {
        let mut window = window(full_tree);
        window.update(|probe| {
            let children = vec![TreeItem::leaf(PathBuf::from("/root/a"), "a")];
            probe
                .tree_items
                .set_children(&PathBuf::from("/root"), children);
        });
        window.press("l");
        assert!(window.app().loads.is_empty(), "the children were held");
        assert!(window.ui_text().contains("TreeItem \"a\""));
        window.update(|probe| {
            probe.tree_items.reload(&PathBuf::from("/root"));
        });
        let text = window.ui_text();
        assert!(text.contains("TreeItem \"root\" expanded"), "{text}");
        assert!(text.contains("TreeItem \"Loading…\""), "{text}");
    }

    #[test]
    fn test_a_blinking_cursor_hides_every_other_interval_and_starts_over_when_it_moves() {
        let cursor = Cursor::new(0, 0, CursorShape::Block);
        let start = Instant::now();
        let interval = Duration::from_millis(500);
        let mut cells = CellsBox {
            blinks: true,
            cursor: Some(cursor),
            interval,
            rows: 1,
            since: start,
        };
        assert!(cells.shown(start));
        assert!(!cells.shown(start + interval));
        assert!(cells.shown(start + interval * 2));
        assert_eq!(cells.next_flip(start + interval / 2), start + interval);
        cells.sync(CellsProps {
            blinks: true,
            cursor: Some(Cursor::new(0, 1, CursorShape::Bar)),
            rows: 1,
        });
        assert!(cells.since > start, "a moved cursor starts showing again");
    }

    #[test]
    fn test_cells_draw_the_apps_grid_as_tall_as_its_rows_above_the_next_element() {
        struct Screen {
            buffer: CellBuffer,
            theme: Theme,
        }

        impl App for Screen {
            fn view(&self, _window: WindowId) -> Element<'_, Self> {
                column([cells(&self.buffer).into(), text("below").monospace().into()])
            }

            fn theme(&self) -> &Theme {
                &self.theme
            }
        }

        let mut buffer = CellBuffer::new(4, 2);
        for (col, ch) in "hey".chars().enumerate() {
            buffer.set(1, col, Cell::new(ch, Default::default()));
        }
        let screen = Screen {
            buffer,
            theme: Theme::dark(),
        };
        let window = TreeWindow::new(screen, "t", 200, 100);
        assert_eq!(window.drawn_text(), ["hey", "below"]);
    }
}

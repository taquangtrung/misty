//! [`Element`]: one node of a window's tree, and the containers and text
//! built from free functions: [`column()`], [`row`], [`form`] with its
//! [`field`]s, and [`text`]; and [`Overlay`], a layer drawn over the
//! window, made by [`overlay()`]; and [`MenuRequest`], where a menu the
//! user asked for opens. The controls are built in
//! [`misty::view`](crate::view).

use std::any::Any;
use std::borrow::Cow;
use std::rc::Rc;

use super::super::base::geometry::{Point, Rect};
use super::super::base::key::Mods;
use super::super::base::split::Axis;
use super::super::base::styled::{Span, StyledText, TextColor, TextSize};
use super::super::base::theme::{Emphasis, ThemeToken};
use super::super::input::event::WheelInput;
use super::super::render::layout::{Align, Justify};
use super::super::widgets::drag::FileDropped;
use super::controls::Callback;
use super::lazy::LazySource;
use super::node::Node;
use super::props::{
    Corners, DragSource, DropTarget, ElementSize, Extent, Extras, PointerDrag, Press, Space,
};

// ========================================================================
// Data Structures
// ========================================================================

/// One node of a window's tree: a control, a container of other elements,
/// or a line of text, borrowing the app's state for the lifetime `'a`, in
/// an app of type `A`.
///
/// An app builds its window's tree in [`App::view`](crate::App::view)
/// from free functions: the containers [`column()`], [`row`], and [`form`],
/// [`text`], and controls such as [`checkbox`](crate::view::checkbox).
/// Misty rebuilds it on every input and every frame, lays it out, draws
/// it, routes input through it, and runs the callbacks its controls were
/// given. A control's state that only it knows (a caret, a scroll offset)
/// lives in misty under the element's id, which by default is its place in
/// the tree; [`Element::id`] names an element whose place may change.
pub struct Element<'a, A> {
    /// What a right click on it, or Shift+F10 on a control inside it,
    /// runs.
    pub(crate) context: Option<Callback<A, MenuRequest>>,
    /// Its layout, surface, and pointer properties, if it was given any.
    pub(crate) extras: Option<Box<Extras<'a, A>>>,
    /// The name the app gave the element's id, if it gave one.
    pub(crate) id: Option<Cow<'a, str>>,
    /// What the element is.
    pub(crate) kind: Kind<'a, A>,
    /// Its tab order among its siblings, if the app gave one.
    pub(crate) order: Option<i32>,
    /// The layers drawn over the window while it shows, each in its slot:
    /// `None` for an [`Element::overlay_if`] not showing now, so the layers
    /// after it keep their ids.
    pub(crate) overlays: Vec<Option<Overlay<'a, A>>>,
    /// The blank space inside it, in cell columns and rows.
    pub(crate) padding: (f32, f32),
    /// The help a tooltip shows when the pointer rests on it.
    pub(crate) tooltip: Option<Cow<'a, str>>,
}

/// A layer drawn over the whole window, after everything under it, as part
/// of the element it is given to with [`Element::overlay`]: the element's
/// id names its state, and it shows while the element does. Made by
/// [`overlay()`], or from a [`dialog`](crate::view::dialog) or
/// [`toasts`](crate::view::toasts).
pub struct Overlay<'a, A> {
    /// What the layer shows, laid out over the whole window.
    pub(crate) content: Element<'a, A>,
    /// Whether the keys its elements leave go on to [`App::key`] while it
    /// is the innermost trap.
    ///
    /// [`App::key`]: crate::App::key
    pub(crate) keys_to_app: bool,
    /// Whether it keeps the keyboard and the pointer while it shows.
    pub(crate) modal: bool,
}

/// Where a menu the user asked for opens, and how: what a right click
/// or a menu button reports to the app (through
/// [`Element::on_context_menu`] or a title bar's `on_menu`), and what
/// [`context_menu`](crate::view::context_menu) opens the menu at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuRequest {
    /// What the menu opens beside: under it, or above it when the window
    /// has no room below. A right click's is the point clicked, as an
    /// empty rectangle; a menu button's is the button.
    pub anchor: Rect,
    /// Whether a key asked for the menu, which opens it with its first
    /// item highlighted, so the keyboard can go on from there.
    pub by_key: bool,
}

/// What an element is.
pub(crate) enum Kind<'a, A> {
    /// Elements laid out together.
    Container(Container<'a, A>),
    /// A control, its widget kept in the store.
    Control(Box<dyn Node<A> + 'a>),
    /// Two elements side by side or one above the other, a divider
    /// between them.
    Split(SplitParts<'a, A>),
    /// Rows built as they come into view: only those that show are
    /// made, from the app's builder.
    Lazy(Lazy<'a, A>),
    /// A line of text.
    Text(Text<'a>),
}

/// Rows a [`lazy_column`](super::lazy_column) builds as they come into
/// view.
pub(crate) struct Lazy<'a, A> {
    /// The row the app's cursor is on, scrolled into view when it moves.
    pub(crate) cursor: usize,
    /// What builds the rows, asked only for those that show.
    pub(crate) rows: &'a dyn LazySource<A>,
}

/// Two elements sharing an area, as a split element holds them.
pub(crate) struct SplitParts<'a, A> {
    /// How the two are placed.
    pub(crate) axis: Axis,
    /// The left or top element, then the right or bottom one.
    pub(crate) children: Vec<Element<'a, A>>,
    /// What dragging the divider runs, with the new ratio.
    pub(crate) on_resize: Option<Callback<A, f32>>,
    /// The share of the area, less the divider, the first element takes.
    pub(crate) ratio: f32,
}

/// Elements laid out together.
pub(crate) struct Container<'a, A> {
    /// Whether a scrollbar along its content's right edge runs in its
    /// right padding, at its own right edge.
    pub(crate) bars_in_padding: bool,
    /// Whether a form sets its labels in the theme's label font.
    pub(crate) label_font: bool,
    /// The elements inside, in order.
    pub(crate) children: Vec<Element<'a, A>>,
    /// How they are laid out.
    pub(crate) direction: Direction,
    /// Each child's label, for a form; empty otherwise.
    pub(crate) labels: Vec<&'a str>,
    /// Whether the container scrolls its children when they do not fit.
    pub(crate) scroll: bool,
}

/// How a container lays its children out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    /// Top to bottom.
    Column,
    /// Top to bottom, each child beside its label.
    Form,
    /// Left to right.
    Row,
}

/// One row of a [`form`]: a label and the control it names. Made by
/// [`field`].
pub struct Field<'a, A> {
    /// The control.
    control: Element<'a, A>,
    /// What the label column says beside it.
    label: &'a str,
}

/// A line of text in one or more colors, set in the proportional (UI) font
/// (or on the monospace cell grid, [`Text::monospace`]) and cut with an
/// ellipsis when it does not fit. Made by [`text`], it becomes an [`Element`] with
/// `.into()`, or where a function takes one.
#[derive(Clone, Debug)]
pub struct Text<'a> {
    /// Whether it is stressed the theme's way ([`Theme::emphasis`](crate::Theme::emphasis)), as a
    /// heading is.
    pub(crate) emphasized: bool,
    /// Whether it is set in the proportional font, as it is unless
    /// [`Text::monospace`] puts it on the cell grid.
    pub(crate) proportional: bool,
    /// Optional font-size multiplier relative to the theme's UI font.
    pub(crate) scale: Option<f32>,
    /// The runs of text, each in its color, in order.
    pub(crate) spans: Vec<(Cow<'a, str>, TextColor)>,
}

// ========================================================================
// Element Implementations
// ========================================================================

impl<'a, A> Element<'a, A> {
    /// An element of `kind`, its id its place in the tree.
    pub(crate) fn new(kind: Kind<'a, A>) -> Self {
        Self {
            context: None,
            extras: None,
            id: None,
            kind,
            order: None,
            overlays: Vec::new(),
            padding: (0.0, 0.0),
            tooltip: None,
        }
    }

    /// This element known by the id `name` rather than by its place in the
    /// tree, so it keeps its state (a caret, a scroll offset) when elements
    /// before it come and go: a row of a filtered list, a tab. Two
    /// elements of one window must not share an id; debug builds panic
    /// when they do, and when a control whose container changes its number
    /// of children has no id of its own.
    pub fn id(self, name: impl Into<Cow<'a, str>>) -> Self {
        Self {
            id: Some(name.into()),
            ..self
        }
    }

    /// This element visited by Tab in the order `order` among its
    /// siblings, lowest first; elements without one have order 0 and keep
    /// the order they are built in, which is the usual tab order.
    ///
    /// On a control, its siblings are the other tab stops of its
    /// scrolling container (or layer, or window). On a container (a
    /// column, a row, a form, a split, a scrolling container), the tab
    /// stops inside it are visited as one block, and the block takes
    /// `order` among the elements beside it in the tree: so a toolbar
    /// drawn above a canvas can come after it, the rest of the window
    /// keeping its place.
    ///
    /// ```
    /// use misty::view::{button, canvas, column, row};
    /// use misty::{Element, Theme};
    ///
    /// struct Viewer;
    ///
    /// fn view(_viewer: &Viewer) -> Element<'_, Viewer> {
    ///     column([
    ///         // Drawn first, visited after the pages.
    ///         row([button("Zoom in").on_click(|_: &mut Viewer| {}).into()]).order(1),
    ///         Element::from(canvas(|_, _, _| {}).focusable()).grow(1.0),
    ///     ])
    /// }
    /// # let _ = (view, Theme::dark());
    /// ```
    pub fn order(self, order: i32) -> Self {
        Self {
            order: Some(order),
            ..self
        }
    }

    /// This element with `columns` blank cell columns at its left and right
    /// and `rows` blank rows above and below it, inside its area.
    pub fn padding(self, columns: f32, rows: f32) -> Self {
        Self {
            padding: (columns, rows),
            ..self
        }
    }

    /// This element with `layer` drawn over the whole window while it
    /// shows, after the window's other elements: a dialog, the toasts, or
    /// a layer of elements made with [`overlay()`]. A modal layer keeps
    /// the keyboard and the pointer while it shows (Tab cycles inside it,
    /// keys never reach the elements under it), and gives the focus back
    /// to the element that had it when it goes. The layers of one element
    /// are drawn in the order they were given.
    pub fn overlay(mut self, layer: impl Into<Overlay<'a, A>>) -> Self {
        self.overlays.push(Some(layer.into()));
        self
    }

    /// [`Element::overlay`] while `show` is true: a dialog the app opens
    /// from its own state. A layer not shown keeps its place, so the
    /// layers after it keep their state.
    pub fn overlay_if(mut self, show: bool, layer: impl Into<Overlay<'a, A>>) -> Self {
        self.overlays.push(show.then(|| layer.into()));
        self
    }

    /// This element showing `help` in a tooltip under it once the pointer
    /// has rested on it for a moment; screen readers read the tooltip while
    /// it shows.
    pub fn tooltip(self, help: impl Into<Cow<'a, str>>) -> Self {
        Self {
            tooltip: Some(help.into()),
            ..self
        }
    }

    /// What runs on the app's state when the user asks for a context menu
    /// over this element: a right click on it (once the element under the
    /// pointer has had the press), or Shift+F10 while a control inside it
    /// has the keyboard.
    /// The [`MenuRequest`] says where; the app keeps it and shows a
    /// [`context_menu`](crate::view::context_menu) there until the menu
    /// closes. The innermost element with one under the pointer, or
    /// around the focused control, is asked.
    pub fn on_context_menu(self, apply: impl Fn(&mut A, MenuRequest) + 'static) -> Self {
        Self {
            context: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This container taking the room its parent has left (the window's
    /// height, less its siblings) and scrolling its children when they do
    /// not fit, behind a thin scrollbar. The wheel scrolls it, so do the
    /// keys its focused control leaves (PageDown, `j`), and Tab scrolls the
    /// control it focuses into view. Nothing for a control or text.
    pub fn scroll(mut self) -> Self {
        if let Kind::Container(container) = &mut self.kind {
            container.scroll = true;
        }
        self
    }

    /// This container letting the scrollbar of an element inside it that
    /// scrolls itself, such as a [`lazy_column`](super::lazy_column), run
    /// in its right padding, along its own right edge, when that element
    /// reaches the padding: so a padded list's scrollbar sits at the edge
    /// of the pane, as a scrolling container's does, and its rows keep the
    /// padding. A [`Page::filled`](super::Page::filled) page's content is
    /// so. Nothing for a control or text.
    pub fn bars_in_padding(mut self) -> Self {
        if let Kind::Container(container) = &mut self.kind {
            container.bars_in_padding = true;
        }
        self
    }

    /// This form setting its labels in the theme's
    /// [`LabelFont`](crate::LabelFont) font as buttons and headers are
    /// (the proportional font in the built-in themes, the cell grid in the
    /// grid look), the label column as wide as the widest label so set.
    /// Without it a form's labels sit on the cell grid, as they always
    /// have. Nothing for other elements.
    pub fn label_font(mut self) -> Self {
        if let Kind::Container(container) = &mut self.kind
            && container.direction == Direction::Form
        {
            container.label_font = true;
        }
        self
    }

    // --------------------------------------------------------------------
    // Layout
    // --------------------------------------------------------------------

    /// This element `width` wide, rather than as wide as its place makes
    /// it (a column's width, a share of a row's).
    pub fn width(mut self, width: Extent) -> Self {
        self.extras().sizing.width = Some(width);
        self
    }

    /// This element `height` tall, rather than as tall as its content (or
    /// the room left, for a list or a scrolling container).
    pub fn height(mut self, height: Extent) -> Self {
        self.extras().sizing.height = Some(height);
        self
    }

    /// This element never narrower than `width`.
    pub fn min_width(mut self, width: Extent) -> Self {
        self.extras().sizing.min_width = Some(width);
        self
    }

    /// This element never wider than `width`.
    pub fn max_width(mut self, width: Extent) -> Self {
        self.extras().sizing.max_width = Some(width);
        self
    }

    /// This element never shorter than `height`.
    pub fn min_height(mut self, height: Extent) -> Self {
        self.extras().sizing.min_height = Some(height);
        self
    }

    /// This element never taller than `height`.
    pub fn max_height(mut self, height: Extent) -> Self {
        self.extras().sizing.max_height = Some(height);
        self
    }

    /// This element taking a share of the room its container leaves along
    /// the container's direction, by `weight` against its siblings that
    /// grow: across a row, down a column.
    pub fn grow(mut self, weight: f32) -> Self {
        self.extras().sizing.grow = Some(weight.max(0.0));
        self
    }

    /// This element giving up room by `weight` against its siblings when
    /// they do not fit along their container's direction: 0 never shrinks;
    /// every element starts at 1.
    pub fn shrink(mut self, weight: f32) -> Self {
        self.extras().sizing.shrink = Some(weight.max(0.0));
        self
    }

    /// This container with `gap` between its children, rather than a cell
    /// column between a row's and none between a column's.
    pub fn gap(mut self, gap: Space) -> Self {
        self.extras().sizing.gap = Some(gap);
        self
    }

    /// This container lining its children up across its direction as
    /// `align` says: down a row (centered by default), across a column
    /// (stretched by default).
    pub fn align(mut self, align: Align) -> Self {
        self.extras().sizing.align = Some(align);
        self
    }

    /// This container spreading its children along its direction as
    /// `justify` says (packed at the start by default).
    pub fn justify(mut self, justify: Justify) -> Self {
        self.extras().sizing.justify = Some(justify);
        self
    }

    /// This element with `across` logical pixels of blank space at its left
    /// and right and `down` above and below, inside its area and on top of
    /// any [`Element::padding`] in cells: for what is drawn rather than
    /// written.
    pub fn padding_px(mut self, across: f32, down: f32) -> Self {
        self.extras().sizing.padding_px = Some((across, down));
        self
    }

    /// This element with the theme's page inset
    /// ([`Spacing::page_inset`](crate::Spacing::page_inset)) at its left
    /// and right, on top of its other padding: a page's bands and content.
    pub(crate) fn page_inset(mut self) -> Self {
        self.extras().sizing.page_inset = true;
        self
    }

    /// This element with the theme's row padding
    /// ([`Shape::row_padding`](crate::Shape::row_padding)) above and below,
    /// on top of its other padding, as the rows of misty's lists and tables
    /// and their headers have: for a row the app builds of elements, such
    /// as a table's column headings, to stand as tall as theirs.
    pub fn row_padding(mut self) -> Self {
        self.extras().sizing.row_padding = true;
        self
    }

    /// What runs on the app's state when this element is laid out at
    /// another size than it was last reported at (and once when it is
    /// first laid out, or again after it was gone): its [`ElementSize`], so
    /// a toolbar can leave out the buttons that do not fit the width its
    /// group gave it. Misty reports it once the frame that laid the element
    /// out is drawn, and draws again, so the value is the app's for the
    /// next rebuild: the frame that found the new size still shows what
    /// the app built for the old one. A size reported once is not
    /// reported again until it changes. Size the element by its place
    /// ([`Element::grow`], an [`Extent::Fraction`], the room its column
    /// leaves) rather than by what the callback changes inside it: an
    /// element whose size follows its own report may never settle, a
    /// size each frame. In tests, [`TreeWindow::about_to_wait`](crate::testing::TreeWindow::about_to_wait)
    /// reports it.
    pub fn on_size_change(mut self, apply: impl Fn(&mut A, ElementSize) + 'static) -> Self {
        self.extras().on_size_change = Some(Rc::new(apply));
        self
    }

    // --------------------------------------------------------------------
    // Surface
    // --------------------------------------------------------------------

    /// This element drawn over a fill of the theme's `token` color, its
    /// corners as [`Element::corners`] says (a box's by default).
    pub fn background(mut self, token: ThemeToken) -> Self {
        self.extras().surface.background = Some(token);
        self
    }

    /// This element lined with the theme's `token` color, as thick as the
    /// theme's box borders ([`Shape::border_width`](crate::Shape)).
    pub fn border(mut self, token: ThemeToken) -> Self {
        self.extras().surface.border = Some(token);
        self
    }

    /// This element's background and border drawn with `corners` from the
    /// theme's shape.
    pub fn corners(mut self, corners: Corners) -> Self {
        self.extras().surface.corners = corners;
        self
    }

    /// This element's background the theme's `token` color while the
    /// pointer is over it: the flat hover of a card or a row.
    pub fn hover_background(mut self, token: ThemeToken) -> Self {
        self.extras().surface.hover_background = Some(token);
        self
    }

    /// A line around this element in the theme's `token` color while the
    /// pointer is over it, in place of a hover fill, so its own fill (a
    /// selected row's) still shows: the hover of a tree's rows.
    pub fn hover_border(mut self, token: ThemeToken) -> Self {
        self.extras().surface.hover_border = Some(token);
        self
    }

    /// Keep this element unlit under the pointer, though it can be pressed:
    /// for one that shows its own answer to the pointer, as a field its
    /// caret. Anything else pressable takes the theme's hover light.
    pub fn unlit(mut self) -> Self {
        self.extras().surface.unlit = true;
        self
    }

    // --------------------------------------------------------------------
    // Pointer
    // --------------------------------------------------------------------

    /// What runs on the app's state when the user presses this element:
    /// any pointer button over it that the elements inside it leave (the
    /// [`Press`] saying which, how many in a row, the modifiers, and where
    /// from its top left). Unless it is a control already, the element
    /// also takes the keyboard in reading order, with a focus ring, and
    /// Enter, Space, or a screen reader's click press it too, so whatever
    /// the pointer can do the keyboard can; name it for screen readers
    /// with [`Element::access_name`].
    pub fn on_press(mut self, apply: impl Fn(&mut A, Press) + 'static) -> Self {
        self.extras().on_press = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state when a pointer button is pressed over
    /// this element and the elements inside it leave the press (the
    /// [`Press`] saying which button, how many in a row, the modifiers, and
    /// where from its top left), and only then: unlike
    /// [`Element::on_press`], the element does not become a tab stop, and
    /// Enter, Space, and screen readers do not press it. For content that
    /// already has its own keyboard path (a document's pages driven by the
    /// app's keymap, a canvas with keys of its own), where a tab stop
    /// taking Enter and Space would get in the way. With
    /// [`Element::on_pointer_drag`] too, it runs first, then the drag
    /// reports the moves.
    pub fn on_pointer_press(mut self, apply: impl Fn(&mut A, Press) + 'static) -> Self {
        self.extras().on_pointer_press = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state when the pointer comes over this
    /// element (`true`) and when it leaves (`false`), whatever is inside
    /// it; for a highlight alone, [`Element::hover_background`] needs no
    /// state.
    pub fn on_hover(mut self, apply: impl Fn(&mut A, bool) + 'static) -> Self {
        self.extras().on_hover = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state as the pointer moves over this element,
    /// whatever is inside it, with where it is from the element's top left
    /// in physical pixels (`Some`), and once when it leaves (`None`): to
    /// follow the mark or link under the pointer on a
    /// [`canvas`](crate::view::canvas). While a press on the element is
    /// held ([`Element::on_pointer_drag`]) it keeps reporting the pointer
    /// over it. Each move already rebuilds the tree and redraws the window
    /// once, so the callback costs no rebuild of its own.
    pub fn on_pointer_move(mut self, apply: impl Fn(&mut A, Option<Point>) + 'static) -> Self {
        self.extras().on_pointer_move = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state when the wheel turns over this element
    /// and nothing inside it scrolled, its position from the element's top
    /// left.
    pub fn on_scroll(mut self, apply: impl Fn(&mut A, WheelInput) + 'static) -> Self {
        self.extras().on_scroll = Some(Rc::new(apply));
        self
    }

    /// What runs on the app's state as the pointer moves with the left
    /// or middle button held after a press on this element, and once more
    /// when that button is released ([`PointerDrag::done`]), the points
    /// from the element's top left: for drawing on a
    /// [`canvas`](crate::view::canvas) or moving a handle with the left
    /// button, panning with the middle one ([`PointerDrag::button`]). The element keeps the pointer until the release,
    /// so the drag goes on past its edges. The press itself is
    /// [`Element::on_press`]'s; to carry a value to another element, use
    /// [`Element::drag_source`] instead.
    pub fn on_pointer_drag(mut self, apply: impl Fn(&mut A, PointerDrag) + 'static) -> Self {
        self.extras().on_pointer_drag = Some(Rc::new(apply));
        self
    }

    /// This element kept in view by the scrolling container around it:
    /// scrolled into view each time another element takes this mark, as
    /// the row an app's own cursor is on (moved by its keymap rather than
    /// misty's focus) when the cursor moves. Once there, the wheel may
    /// scroll it away again.
    pub fn keep_in_view(mut self) -> Self {
        self.extras().keep_in_view = true;
        self
    }

    /// The name screen readers call this element by when it takes presses
    /// ([`Element::on_press`]), as a button.
    pub fn access_name(mut self, name: &'a str) -> Self {
        self.extras().access_name = Some(name);
        self
    }

    // --------------------------------------------------------------------
    // Drag and drop
    // --------------------------------------------------------------------

    /// This element dragged with the left button, carrying `payload` to an
    /// element whose [`Element::drop_target`] takes a `P`, while a label
    /// saying `label` follows the pointer. The drag starts once the
    /// pointer has moved a few pixels, so a click stays a click; Esc
    /// cancels it.
    pub fn drag_source<P: 'static>(mut self, payload: P, label: impl Into<Cow<'a, str>>) -> Self {
        self.extras().drag = Some(DragSource {
            label: label.into(),
            mods: None,
            payload: Rc::new(payload),
        });
        self
    }

    /// This element dragged as [`Element::drag_source`] says, but only by
    /// a left press holding exactly `mods` (Alt, to carry a document's
    /// view where a plain drag selects): such a press goes to none of the
    /// element's other pointer callbacks, and a press without them is the
    /// element's as before. Released before it dragged, the press is a
    /// click after all: [`Element::on_pointer_drag`] hears it once, done,
    /// where it was pressed and with `mods` (an Alt+click on a link).
    pub fn drag_source_with<P: 'static>(
        mut self,
        mods: Mods,
        payload: P,
        label: impl Into<Cow<'a, str>>,
    ) -> Self {
        self.extras().drag = Some(DragSource {
            label: label.into(),
            mods: Some(mods),
            payload: Rc::new(payload),
        });
        self
    }

    /// This element taking drags of [`Element::drag_source`]s carrying a
    /// `P` for which `accepts` says yes, marked while one is over it;
    /// dropping one runs `on_drop` on the app's state with the payload and
    /// where it was dropped, from this element's top left. The innermost
    /// target under the pointer that accepts takes it.
    pub fn drop_target<P: Clone + 'static>(
        mut self,
        accepts: impl Fn(&P) -> bool + 'a,
        on_drop: impl Fn(&mut A, P, Point) + 'static,
    ) -> Self {
        self.extras().drop = Some(DropTarget {
            accepts: Box::new(move |payload: &dyn Any| {
                payload.downcast_ref::<P>().is_some_and(&accepts)
            }),
            marker: None,
            on_drop: Rc::new(move |app: &mut A, payload: &dyn Any, point| {
                if let Some(payload) = payload.downcast_ref::<P>() {
                    on_drop(app, payload.clone(), point);
                }
            }),
        });
        self
    }

    /// The part of this drop target ([`Element::drop_target`]) a drag over
    /// it marks: `marker` gets where the target was drawn and the pointer
    /// from its top left, and returns the rectangle to mark, as a pane
    /// marks the side a tab dropped there would split it on
    /// ([`DropZone::at`](crate::widgets::DropZone::at) and
    /// [`DropZone::rect`](crate::widgets::DropZone::rect)). Without one the whole
    /// target is marked; on an element that is no drop target it does
    /// nothing.
    pub fn drop_marker(mut self, marker: impl Fn(Rect, Point) -> Rect + 'static) -> Self {
        if let Some(target) = self.extras().drop.as_mut() {
            target.marker = Some(Rc::new(marker));
        }
        self
    }

    /// What runs on the app's state when a file dragged in from another
    /// app is dropped on this element, its position from the element's
    /// top left; the element is marked while one is held over it. The
    /// innermost such element under the pointer takes it, and a file no
    /// element takes goes to [`App::file_dropped`](crate::App::file_dropped).
    pub fn on_file_drop(mut self, apply: impl Fn(&mut A, FileDropped) + 'static) -> Self {
        self.extras().file_drop = Some(Rc::new(apply));
        self
    }

    /// The element's extra properties, made empty the first time.
    pub(crate) fn extras(&mut self) -> &mut Extras<'a, A> {
        self.extras.get_or_insert_with(Box::default)
    }

    /// A container laying `children` out in `direction`, beside `labels`
    /// for a form.
    fn container(
        direction: Direction,
        children: Vec<Element<'a, A>>,
        labels: Vec<&'a str>,
    ) -> Self {
        Self::new(Kind::Container(Container {
            bars_in_padding: false,
            label_font: false,
            children,
            direction,
            labels,
            scroll: false,
        }))
    }
}

impl<'a, A> From<Text<'a>> for Element<'a, A> {
    fn from(text: Text<'a>) -> Self {
        Self::new(Kind::Text(text))
    }
}

// ========================================================================
// MenuRequest Implementations
// ========================================================================

impl MenuRequest {
    /// A menu asked for with the pointer at `point`, opening there.
    pub fn at(point: Point) -> Self {
        Self {
            anchor: Rect::new(point.x, point.y, 0.0, 0.0),
            by_key: false,
        }
    }
}

// ========================================================================
// Overlay Implementations
// ========================================================================

impl<'a, A> Overlay<'a, A> {
    /// This layer modal: it keeps the keyboard and the pointer while it
    /// shows, over a dimmed backdrop, and the focus goes back to the
    /// element that had it when it goes.
    pub fn modal(self) -> Self {
        Self {
            modal: true,
            ..self
        }
    }

    /// This modal layer handing the keys its elements leave to
    /// [`App::key`](crate::App::key), as the window does without a
    /// modal layer, so the app's Esc closes it and its chords still work:
    /// a key no element inside took (Tab still cycles inside it), while no
    /// menu or popup opened over it keeps the keyboard. The elements under
    /// it still get no key and no pointer. Without it, such keys go
    /// nowhere.
    pub fn keys_to_app(self) -> Self {
        Self {
            keys_to_app: true,
            ..self
        }
    }
}

// ========================================================================
// Text Implementations
// ========================================================================

impl<'a> Text<'a> {
    /// This text in `color` from its last run on: the run [`text`] or
    /// [`Text::then`] started.
    pub fn color(mut self, color: impl Into<TextColor>) -> Self {
        if let Some((_, last)) = self.spans.last_mut() {
            *last = color.into();
        }
        self
    }

    /// This text's last run in the theme's muted color, as for a hint or a
    /// caption.
    pub fn muted(self) -> Self {
        self.color(ThemeToken::Muted)
    }

    /// This text followed by `text` in the foreground color, recolored by
    /// the [`Text::color`] or [`Text::muted`] after it.
    pub fn then(mut self, text: impl Into<Cow<'a, str>>) -> Self {
        self.spans
            .push((text.into(), ThemeToken::Foreground.into()));
        self
    }

    /// This text stressed as the theme stresses words
    /// ([`Theme::emphasis`](crate::Theme::emphasis)): bold in the built-in themes, as a heading or
    /// a title.
    pub fn emphasized(self) -> Self {
        Self {
            emphasized: true,
            ..self
        }
    }

    /// This text on the monospace cell grid, each character a cell wide
    /// (two for a wide one), as code, keys, and columns aligned by
    /// character want, rather than in the proportional font text is set
    /// in.
    pub fn monospace(self) -> Self {
        Self {
            proportional: false,
            ..self
        }
    }

    /// This text in the proportional font, as all text is unless
    /// [`Text::monospace`] puts it on the cell grid.
    #[deprecated(
        note = "text is proportional by default; drop the call, or use `monospace` for the cell grid"
    )]
    pub fn proportional(self) -> Self {
        Self {
            proportional: true,
            ..self
        }
    }

    /// Scale this text's font size by `factor` relative to the theme's UI font.
    pub fn scale(self, factor: f32) -> Self {
        Self {
            scale: Some(factor),
            ..self
        }
    }

    /// The text as misty draws it, cut with an ellipsis.
    pub(crate) fn styled(&self, emphasis: Emphasis) -> StyledText {
        self.spans
            .iter()
            .fold(StyledText::new(), |styled, (text, color)| {
                let mut span = Span::new(text.as_ref(), *color);
                if self.emphasized {
                    span = span.emphasized(emphasis);
                }
                if let Some(factor) = self.scale {
                    span = span.size(TextSize::Scale(factor));
                }
                styled.with(span)
            })
            .with_ellipsis()
    }
}

// ========================================================================
// Field Implementations
// ========================================================================

impl<'a, A> Field<'a, A> {
    /// The label and the control.
    fn into_parts(self) -> (&'a str, Element<'a, A>) {
        (self.label, self.control)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A container laying `children` out top to bottom, each as wide as the
/// column.
pub fn column<'a, A>(children: impl IntoIterator<Item = Element<'a, A>>) -> Element<'a, A> {
    Element::container(
        Direction::Column,
        children.into_iter().collect(),
        Vec::new(),
    )
}

/// A container laying `children` out left to right, a cell apart, sharing
/// its width between its controls.
pub fn row<'a, A>(children: impl IntoIterator<Item = Element<'a, A>>) -> Element<'a, A> {
    Element::container(Direction::Row, children.into_iter().collect(), Vec::new())
}

/// A container laying `fields` out as a form: top to bottom, each control
/// beside its label in a label column as wide as the widest label. The
/// label is drawn by the form, muted, and also names the control for
/// screen readers (a checkbox or a button keeps the name it shows).
pub fn form<'a, A>(fields: impl IntoIterator<Item = Field<'a, A>>) -> Element<'a, A> {
    let (labels, children) = fields.into_iter().map(Field::into_parts).unzip();
    Element::container(Direction::Form, children, labels)
}

/// A row of a [`form`]: `control` beside `label`, which names it for
/// screen readers too. An empty label leaves the label column blank, for
/// a control that belongs with the one above it.
pub fn field<'a, A>(label: &'a str, control: impl Into<Element<'a, A>>) -> Field<'a, A> {
    Field {
        control: control.into(),
        label,
    }
}

/// A layer showing `content` over the whole window, laid out as a window's
/// root element is; [`Overlay::modal`] makes it keep the keyboard. Give it
/// to an element with [`Element::overlay`].
pub fn overlay<'a, A>(content: impl Into<Element<'a, A>>) -> Overlay<'a, A> {
    Overlay {
        content: content.into(),
        keys_to_app: false,
        modal: false,
    }
}

/// A line of `text` in the foreground color, in the proportional (UI)
/// font; [`Text::muted`], [`Text::color`], and [`Text::then`] change its
/// colors, and [`Text::monospace`] puts it on the cell grid.
pub fn text<'a>(text: impl Into<Cow<'a, str>>) -> Text<'a> {
    Text {
        emphasized: false,
        proportional: true,
        scale: None,
        spans: vec![(text.into(), ThemeToken::Foreground.into())],
    }
}

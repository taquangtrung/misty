//! What the store needs of a control element, whatever its widget:
//! [`Spec`], which each control builder implements for its own widget
//! type, and [`Node`], the same operations on a widget kept as a
//! [`Stored`] box, which [`Spec`] gives every builder for free. The store
//! keeps widgets of many types in one map and reaches each through the
//! element that names it, so neither the store nor the builders downcast
//! by hand.
//!
//! A control may float a popup of its own over the window (a select's
//! open list, a menu): [`Spec::has_layer`] says when it shows, and the
//! store keeps the focus inside it, under [`layer_id`], while it does.
//! A control may also take the pointer over a region above everything
//! else ([`Drawn::over`]), as a title bar takes the window's resize
//! border, under [`over_id`].

use std::any::Any;
use std::time::Instant;

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::super::widgets::drag::Dropped;
use super::super::widgets::select::LIST_NODE;
use super::props::Payload;

// ========================================================================
// Constants
// ========================================================================

/// The child index, under a control's id, of the region it takes the
/// pointer over above everything else: apart from its children, its
/// popup ([`LIST_NODE`]), and its layers.
const OVER_NODE: u64 = 1 << 34;

// ========================================================================
// Data Structures
// ========================================================================

/// A change to the app's state an element's callback makes, queued while
/// the tree borrows the app and run once the tree is dropped.
pub(crate) type Call<A> = Box<dyn FnOnce(&mut A)>;

/// Where a control was drawn: the rectangle the focus tree registers it
/// in, and the popup it floats over the window, if one shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Drawn {
    /// Where the control takes the pointer and the focus ring.
    pub(crate) hit: Rect,
    /// Where its popup was drawn, registered under [`layer_id`].
    pub(crate) layer: Option<Rect>,
    /// Where it takes the pointer above every other element (a title
    /// bar's resize border runs along the window's edges, over the
    /// content), registered under [`over_id`] for the pointer only; the
    /// pointer input it declines falls through to what is under it.
    pub(crate) over: Option<Rect>,
}

/// A drag between elements a control carries out itself (a tab dragged
/// off its strip): the store draws its label and the drop target under
/// it, and hands the drop to that target, as for a region's drag source.
pub(crate) enum ControlDrag {
    /// The drag ended without a drop (cancelled, or back on the control).
    Ended,
    /// It is running: what it carries, where the pointer is, and what the
    /// label following the pointer says.
    Moving(MovingDrag),
    /// It was dropped here.
    Dropped(Dropped<Payload>),
}

/// A control's drag while it runs.
pub(crate) struct MovingDrag {
    pub(crate) label: String,
    pub(crate) payload: Payload,
    pub(crate) position: Point,
}

/// What a control answered an input or a screen reader's action with.
pub(crate) struct Answer<A> {
    /// The callback call the change it reported makes, if it reported one
    /// and the element has a callback.
    pub(crate) call: Option<Call<A>>,
    /// Whether the change asks to close the window, as a title bar's close
    /// button does when the app gave it no callback.
    pub(crate) close: bool,
    /// The drag between elements it carries out, if its answer was about
    /// one.
    pub(crate) drag: Option<ControlDrag>,
    /// Whether the control used the input.
    pub(crate) used: bool,
}

// ========================================================================
// Traits
// ========================================================================

/// A widget the store keeps across rebuilds, whatever its type: what the
/// store does to every widget alike (its focus, its animation), and the
/// [`Any`] it is told apart by.
pub(crate) trait Stored: Any {
    /// Tells the widget whether it has the keyboard.
    fn set_focused(&mut self, focused: bool);

    /// Steps the widget's animation, returning when it next needs this.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant>;
}

/// A control builder: the widget it keeps in the store, and how its props,
/// drawing, description, and callback reach that widget. Implemented by
/// each builder of `misty::view`.
pub(crate) trait Spec<A> {
    /// The widget the store keeps for the element.
    type Widget: Control + 'static;

    /// The element's kind, as messages and tree paths name it.
    const KIND: &'static str;

    /// A new widget for an element seen for the first time.
    fn create(&self) -> Self::Widget;

    /// The props the element gives its widget, `name` the field label the
    /// widget is named by for screen readers (a widget that draws its own
    /// label keeps that).
    fn props<'b>(&'b self, name: Option<&'b str>) -> <Self::Widget as Control>::Props<'b>;

    /// The callback call storing `value`, or `None` without a callback.
    fn call(&self, value: <Self::Widget as Control>::Value) -> Option<Call<A>>;

    /// Whether `value` asks misty to close the window, as a title bar's
    /// close button without a callback does; by default nothing does.
    fn asks_close(&self, _value: &<Self::Widget as Control>::Value) -> bool {
        false
    }

    /// Draws `widget` in `rect`, returning where the focus tree registers
    /// it and its popup.
    fn draw(
        &self,
        widget: &mut Self::Widget,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn;

    /// Describes `widget` to screen readers as `id`.
    fn describe(&self, widget: &Self::Widget, frame: &mut Frame, id: FocusId);

    /// Where parts of `widget` as last drawn show a tooltip of their own
    /// (a title bar's buttons), with its help; none by default.
    fn tips(&self, _widget: &Self::Widget) -> Vec<(Rect, String)> {
        Vec::new()
    }

    /// Does what a screen reader's `action` on `widget`, described as
    /// `id`, asks, or `None` for a widget that takes no actions.
    fn act(
        &self,
        widget: &mut Self::Widget,
        id: FocusId,
        action: AccessAction,
    ) -> Option<<Self::Widget as Widget>::Response>;

    /// Hands `input` to `widget`: by default its own [`Widget::handle`],
    /// and for an element lending the app's data (a list's rows), the
    /// widget lent that data.
    fn handle(
        &self,
        widget: &mut Self::Widget,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> <Self::Widget as Widget>::Response {
        widget.handle(input, cx)
    }

    /// Whether the element takes drags between elements dropped on it
    /// itself, working out where they land from its own widget (a tab
    /// strip, the gap a tab would go in); no by default.
    fn takes_drops(&self) -> bool {
        false
    }

    /// Whether it takes a drag carrying `payload`.
    fn takes_drop(&self, _payload: &dyn Any) -> bool {
        false
    }

    /// Shows where a drag over it at `point` (in the window) would land,
    /// or stops showing it for `None`.
    fn mark_drop(&self, _widget: &mut Self::Widget, _point: Option<Point>) {}

    /// What a drag carrying `payload` dropped on it at `point` (in the
    /// window) runs on the app's state.
    fn drop_call(
        &self,
        _widget: &Self::Widget,
        _payload: &dyn Any,
        _point: Point,
    ) -> Option<Call<A>> {
        None
    }

    /// The drag between elements `response` is about, for a control that
    /// carries one out itself; none by default.
    fn dragged(
        &self,
        _widget: &Self::Widget,
        _response: &<Self::Widget as Widget>::Response,
    ) -> Option<ControlDrag> {
        None
    }

    /// What `input`, which `widget` left unused, runs on the app's state
    /// when the element takes it itself (a text field's Enter with
    /// `on_submit`), or `None` to let it go on; none by default.
    fn unused(&self, _widget: &Self::Widget, _input: &Input) -> Option<Call<A>> {
        None
    }

    /// Whether the element takes the height its parent leaves, as a list
    /// or a table does, rather than the height its widget measures.
    fn grows(&self) -> bool {
        false
    }

    /// Whether the element takes the keyboard; a progress bar does not.
    fn is_focusable(&self) -> bool {
        true
    }

    /// Whether an element that does not take the keyboard still takes the
    /// pointer, as the toasts' cards do.
    fn takes_pointer(&self) -> bool {
        false
    }

    /// Whether `widget` floats a popup over the window now (a select's
    /// open list, a menu), which keeps the focus inside it until it closes.
    fn has_layer(_widget: &Self::Widget) -> bool {
        false
    }

    /// Whether the element changes the app's state as time passes, through
    /// [`Spec::tick`], as the toasts expire.
    fn is_ticking(&self) -> bool {
        false
    }

    /// What is due at `now`: the call that changes the app's state as time
    /// passed, and when the next change is due.
    fn tick(&self, _now: Instant) -> (Option<Call<A>>, Option<Instant>) {
        (None, None)
    }

    /// The height the element is held to with `metrics`, or `None` for the
    /// height its widget measures.
    fn fixed_height(&self, _metrics: CellMetrics) -> Option<f32> {
        None
    }

    /// Where `widget`'s caret is, for the input method, or `None` for a
    /// widget that takes no text.
    fn caret(&self, _widget: &Self::Widget) -> Option<Rect> {
        None
    }

    /// How wide the element is drawn in `frame` with `theme` when it has
    /// the room (a button's label, icon, and padding), which a row packs
    /// it to unless the app sized or grew it; `None`, the default, for an
    /// element that takes a share of its row.
    fn natural_width(&self, _frame: &Frame, _theme: &Theme) -> Option<f32> {
        None
    }

    /// How tall the element measures `widget` to be `width` pixels wide,
    /// when the element rather than its widget knows (a canvas sized from
    /// the app's state), or `None` to ask the widget.
    fn measure_widget(
        &self,
        _widget: &Self::Widget,
        _width: f32,
        _metrics: CellMetrics,
        _scale: f64,
    ) -> Option<f32> {
        None
    }
}

/// A control element as the store sees it: the operations of [`Spec`] on
/// a [`Stored`] widget, which the store checks with [`Node::owns`] before
/// handing it over.
pub(crate) trait Node<A> {
    /// The element's kind, as messages and tree paths name it.
    fn kind(&self) -> &'static str;

    /// Whether the element takes the keyboard.
    fn is_focusable(&self) -> bool;

    /// Whether an element that does not take the keyboard takes the
    /// pointer.
    fn takes_pointer(&self) -> bool;

    /// Whether `widget` floats a popup over the window now.
    fn has_layer(&self, widget: &dyn Stored) -> bool;

    /// Whether the element changes the app's state as time passes.
    fn is_ticking(&self) -> bool;

    /// What is due at `now`, and when the next change is.
    fn tick(&self, now: Instant) -> (Option<Call<A>>, Option<Instant>);

    /// Whether the element takes the height its parent leaves.
    fn grows(&self) -> bool;

    /// Whether `widget` is of the type this element keeps.
    fn owns(&self, widget: &dyn Stored) -> bool;

    /// A new widget for the element.
    fn create(&self) -> Box<dyn Stored>;

    /// Copies the element's props, named `name`, into `widget`.
    fn sync(&self, widget: &mut dyn Stored, name: Option<&str>);

    /// How wide the element is drawn when it has the room, if it packs to
    /// its own width in a row.
    fn natural_width(&self, frame: &Frame, theme: &Theme) -> Option<f32>;

    /// How tall `widget` is `width` pixels wide.
    fn measure(&self, widget: &dyn Stored, width: f32, metrics: CellMetrics, scale: f64) -> f32;

    /// Draws `widget` in `rect`, returning where the focus tree registers
    /// it and its popup.
    fn draw(&self, widget: &mut dyn Stored, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn;

    /// Describes `widget` to screen readers as `id`.
    fn describe(&self, widget: &dyn Stored, frame: &mut Frame, id: FocusId);

    /// Where parts of `widget` show a tooltip of their own, with its help.
    fn tips(&self, widget: &dyn Stored) -> Vec<(Rect, String)>;

    /// Hands `input` to `widget`.
    fn handle(&self, widget: &mut dyn Stored, input: &Input, cx: &mut EventCx<'_>) -> Answer<A>;

    /// Hands a screen reader's `action` to `widget`, described as `id`.
    fn act(&self, widget: &mut dyn Stored, id: FocusId, action: AccessAction) -> Answer<A>;

    /// Where `widget`'s caret is, if it takes text.
    fn caret(&self, widget: &dyn Stored) -> Option<Rect>;

    /// Whether the element takes drops itself ([`Spec::takes_drops`]).
    fn takes_drops(&self) -> bool;

    /// Whether it takes a drag carrying `payload`.
    fn takes_drop(&self, payload: &dyn Any) -> bool;

    /// Shows or hides where a drag over `widget` would land.
    fn mark_drop(&self, widget: &mut dyn Stored, point: Option<Point>);

    /// What dropping `payload` on `widget` at `point` runs.
    fn drop_call(&self, widget: &dyn Stored, payload: &dyn Any, point: Point) -> Option<Call<A>>;
}

// ========================================================================
// Drawn Implementations
// ========================================================================

impl From<Rect> for Drawn {
    /// A control drawn at `hit` with no popup.
    fn from(hit: Rect) -> Self {
        Self {
            hit,
            layer: None,
            over: None,
        }
    }
}

// ========================================================================
// Stored Implementations
// ========================================================================

impl<W: Widget + 'static> Stored for W {
    fn set_focused(&mut self, focused: bool) {
        Widget::set_focused(self, focused);
    }

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        Widget::about_to_wait(self, cx)
    }
}

// ========================================================================
// Node Implementations
// ========================================================================

impl<A, S: Spec<A>> Node<A> for S {
    fn kind(&self) -> &'static str {
        S::KIND
    }

    fn is_focusable(&self) -> bool {
        Spec::is_focusable(self)
    }

    fn takes_pointer(&self) -> bool {
        Spec::takes_pointer(self)
    }

    fn has_layer(&self, widget: &dyn Stored) -> bool {
        S::has_layer(downcast(widget))
    }

    fn is_ticking(&self) -> bool {
        Spec::is_ticking(self)
    }

    fn tick(&self, now: Instant) -> (Option<Call<A>>, Option<Instant>) {
        Spec::tick(self, now)
    }

    fn grows(&self) -> bool {
        Spec::grows(self)
    }

    fn owns(&self, widget: &dyn Stored) -> bool {
        (widget as &dyn Any).is::<S::Widget>()
    }

    fn create(&self) -> Box<dyn Stored> {
        Box::new(Spec::create(self))
    }

    fn sync(&self, widget: &mut dyn Stored, name: Option<&str>) {
        downcast_mut::<S::Widget>(widget).sync(self.props(name));
    }

    fn natural_width(&self, frame: &Frame, theme: &Theme) -> Option<f32> {
        Spec::natural_width(self, frame, theme)
    }

    fn measure(&self, widget: &dyn Stored, width: f32, metrics: CellMetrics, scale: f64) -> f32 {
        let widget = downcast::<S::Widget>(widget);
        self.fixed_height(metrics)
            .or_else(|| self.measure_widget(widget, width, metrics, scale))
            .unwrap_or_else(|| widget.measure(width, metrics, scale))
    }

    fn draw(&self, widget: &mut dyn Stored, frame: &mut Frame, rect: Rect, theme: &Theme) -> Drawn {
        Spec::draw(self, downcast_mut(widget), frame, rect, theme)
    }

    fn describe(&self, widget: &dyn Stored, frame: &mut Frame, id: FocusId) {
        Spec::describe(self, downcast(widget), frame, id);
    }

    fn tips(&self, widget: &dyn Stored) -> Vec<(Rect, String)> {
        Spec::tips(self, downcast(widget))
    }

    fn handle(&self, widget: &mut dyn Stored, input: &Input, cx: &mut EventCx<'_>) -> Answer<A> {
        let widget = downcast_mut::<S::Widget>(widget);
        let response = Spec::handle(self, widget, input, cx);
        if response.is_ignored()
            && let Some(call) = self.unused(widget, input)
        {
            return Answer {
                call: Some(call),
                close: false,
                drag: None,
                used: true,
            };
        }
        answer(self, widget, response)
    }

    fn act(&self, widget: &mut dyn Stored, id: FocusId, action: AccessAction) -> Answer<A> {
        let widget = downcast_mut::<S::Widget>(widget);
        match Spec::act(self, widget, id, action) {
            Some(response) => answer(self, widget, response),
            None => Answer {
                call: None,
                close: false,
                drag: None,
                used: false,
            },
        }
    }

    fn caret(&self, widget: &dyn Stored) -> Option<Rect> {
        Spec::caret(self, downcast(widget))
    }

    fn takes_drops(&self) -> bool {
        Spec::takes_drops(self)
    }

    fn takes_drop(&self, payload: &dyn Any) -> bool {
        Spec::takes_drop(self, payload)
    }

    fn mark_drop(&self, widget: &mut dyn Stored, point: Option<Point>) {
        Spec::mark_drop(self, downcast_mut::<S::Widget>(widget), point);
    }

    fn drop_call(&self, widget: &dyn Stored, payload: &dyn Any, point: Point) -> Option<Call<A>> {
        Spec::drop_call(self, downcast::<S::Widget>(widget), payload, point)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The id the store registers the popup of the control `id` under, and
/// keeps the focus inside while it shows: the node a select describes its
/// open list as, so the trap and the list are one node for screen readers.
pub(crate) fn layer_id(id: FocusId) -> FocusId {
    id.child(LIST_NODE)
}

/// The id the store registers the region `id`'s control takes the
/// pointer over above everything else under ([`Drawn::over`]).
pub(crate) fn over_id(id: FocusId) -> FocusId {
    id.child(OVER_NODE)
}

/// What `widget`'s `response` answers for the element `spec`: whether it
/// was used, and the callback call of the value it changed to.
fn answer<A, S: Spec<A>>(
    spec: &S,
    widget: &S::Widget,
    response: <S::Widget as Widget>::Response,
) -> Answer<A> {
    let used = !response.is_ignored();
    let drag = spec.dragged(widget, &response);
    let value = widget.changed(response);
    let close = value.as_ref().is_some_and(|value| spec.asks_close(value));
    let call = value.and_then(|value| spec.call(value));
    Answer {
        call,
        close,
        drag,
        used,
    }
}

/// `widget` as the `W` the store keeps for its element, which the store
/// checked with [`Node::owns`] when it made or kept the widget.
fn downcast<W: 'static>(widget: &dyn Stored) -> &W {
    (widget as &dyn Any)
        .downcast_ref()
        .expect("the store keeps each element's own widget type")
}

/// `widget` as the `W` the store keeps for its element, mutably.
fn downcast_mut<W: 'static>(widget: &mut dyn Stored) -> &mut W {
    (widget as &mut dyn Any)
        .downcast_mut()
        .expect("the store keeps each element's own widget type")
}

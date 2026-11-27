//! The two elements that let an app draw and drive what misty's controls
//! do not: [`custom`], which embeds any hand-driven [`Widget`] in a tree,
//! the store keeping the widget across rebuilds as it keeps a control's;
//! and [`canvas`], which paints with a closure and, given keys, takes the
//! keyboard. The pointer over either comes through the properties every
//! element takes ([`Element::on_press`], [`Element::on_pointer_drag`],
//! [`Element::on_hover`], [`Element::on_scroll`]), which report points
//! from the element's top left.
//!
//! Both are controls to the store ([`Spec`]). What a widget's response
//! runs on the app's state is worked out while the element, which holds
//! the app's closures, still has the widget at hand, and kept in the
//! widget's box until the store asks for it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use accesskit::{Action, Node as AccessNode, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::Rect;
use super::super::base::key::Key;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::controls::Callback;
use super::element::{Element, Kind};
use super::node::{Call, Drawn, Spec};

// ========================================================================
// Data Structures
// ========================================================================

/// A hand-driven widget embedded in a tree, as an element. Made by
/// [`custom`].
pub struct Custom<'a, A, W: Widget> {
    /// How it is described to screen readers, if not as a group.
    access: Option<Describe<'a, W>>,
    /// What makes the widget the first time the element is seen.
    create: Box<dyn Fn() -> W + 'a>,
    /// Whether it takes the keyboard.
    focusable: bool,
    /// Where the widget's text caret is for the input method, if it takes
    /// text.
    ime_caret: Option<ImeCaret<'a, W>>,
    /// How tall it is for a width, or `None` to take the room left.
    measure: Option<Measure<'a, W>>,
    /// The name screen readers call it by.
    name: Option<&'a str>,
    /// What the widget's responses run on the app's state.
    respond: Option<Respond<'a, A, W>>,
    /// Whether the focus ring leaves it unmarked.
    ringless: bool,
    /// What copies the app's state into the widget at each rebuild.
    sync: Option<SyncWidget<'a, W>>,
}

/// A painting element. Made by [`canvas`].
pub struct Canvas<'a, A> {
    /// What paints it.
    draw: Paint<'a>,
    /// Whether it takes the keyboard even without keys of its own.
    focusable: bool,
    /// The keys it takes while it has the keyboard, and what they run;
    /// `None` for a canvas that does not take the keyboard.
    keys: Option<(Accepts<'a>, Callback<A, Key>)>,
    /// How tall it is for a width, or `None` to take the room left.
    measure: Option<Box<dyn Fn(f32) -> f32 + 'a>>,
    /// The name screen readers call it by.
    name: Option<&'a str>,
    /// Whether the focus ring leaves it unmarked.
    ringless: bool,
}

/// What the store keeps for a [`custom`] element: the widget, where it
/// was drawn last, and the call its last response makes.
pub(crate) struct Embedded<A, W> {
    /// The call the widget's last response makes, until the store takes
    /// it.
    pending: Cell<Option<Call<A>>>,
    /// Where it was drawn last.
    rect: Rect,
    /// The widget.
    widget: W,
}

/// What the store keeps for a [`canvas`]: whether it has the keyboard,
/// where it was drawn last, and the call the last key it took makes.
pub(crate) struct CanvasBox<A> {
    /// Whether it has the keyboard.
    focused: bool,
    /// The call the last key it took makes, until the store takes it.
    pending: Cell<Option<Call<A>>>,
    /// Where it was drawn last.
    rect: Rect,
}

/// Whether an embedded widget or a canvas used an input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Used(bool);

/// What a custom element copies the app's state into its widget with at
/// each rebuild.
type SyncWidget<'a, W> = Box<dyn Fn(&mut W) + 'a>;

/// What paints a canvas in its area.
type Paint<'a> = Box<dyn Fn(&mut Frame, Rect, &Theme) + 'a>;

/// Which keys a canvas, or a list-like element's row, takes.
pub(crate) type Accepts<'a> = Box<dyn Fn(Key) -> bool + 'a>;

/// How a custom element's widget describes itself to screen readers.
type Describe<'a, W> = Box<dyn Fn(&W, &mut Frame, FocusId) + 'a>;

/// Where a custom element's widget's text caret is in the window, for the
/// input method.
type ImeCaret<'a, W> = Box<dyn Fn(&W) -> Option<Rect> + 'a>;

/// How tall a custom element's widget is for a width, with the frame's
/// cell size and scale factor.
type Measure<'a, W> = Box<dyn Fn(&W, f32, CellMetrics, f64) -> f32 + 'a>;

/// What a custom element's widget's response runs on the app's state,
/// read with the widget as it is after the response.
type Respond<'a, A, W> = Box<dyn Fn(&W, <W as Widget>::Response) -> Option<Call<A>> + 'a>;

/// What a custom element copies the app's state into its widget with.
type SyncProps<'b, W> = Option<&'b (dyn Fn(&mut W) + 'b)>;

// ========================================================================
// Custom Implementations
// ========================================================================

impl<'a, A, W: Widget + 'static> Custom<'a, A, W> {
    /// This element copying the app's state into its widget with `sync`
    /// at every rebuild, before the widget is handed an input or drawn:
    /// the controlled half of embedding, as a control's props are. Set
    /// only what changed (compare first), so the state only the widget
    /// knows (a cursor, a scroll) lasts.
    pub fn sync(mut self, sync: impl Fn(&mut W) + 'a) -> Self {
        self.sync = Some(Box::new(sync));
        self
    }

    /// What runs on the app's state when the widget answers an input:
    /// `read` looks at the widget, as it is after the input, and its
    /// response, and returns the value to store (or `None` for nothing);
    /// `apply` stores it once the tree is dropped. A response the widget
    /// did not ignore counts as used either way.
    pub fn on_response<V: 'static>(
        mut self,
        read: impl Fn(&W, W::Response) -> Option<V> + 'a,
        apply: impl Fn(&mut A, V) + 'static,
    ) -> Self
    where
        A: 'static,
    {
        let apply = Rc::new(apply);
        self.respond = Some(Box::new(move |widget: &W, response| {
            let value = read(widget, response)?;
            let apply = Rc::clone(&apply);
            Some(Box::new(move |app: &mut A| apply(app, value)) as Call<A>)
        }));
        self
    }

    /// This element as tall as `measure` says its widget is for a width in
    /// physical pixels, with the frame's cell size and scale factor,
    /// rather than taking the room its column leaves; a fixed size is
    /// [`Element::height`].
    pub fn measure(mut self, measure: impl Fn(&W, f32, CellMetrics, f64) -> f32 + 'a) -> Self {
        self.measure = Some(Box::new(measure));
        self
    }

    /// This element taking text from the window's input method while it
    /// has the keyboard, as misty's text fields do: `caret` says where the
    /// widget's text caret was drawn in the window, in physical pixels (a
    /// rectangle as tall as its line, as
    /// [`TextField::cursor_area`](crate::TextField::cursor_area) gives), or
    /// `None` while it takes no text (a Vim Normal mode). While it gives
    /// one, the input method is enabled with its candidate window beside
    /// the caret, and the widget gets the composition as
    /// [`Input::Ime`](crate::Input::Ime): each [`ImeInput::Preedit`](crate::ImeInput::Preedit)
    /// to show at the caret (a [`Preedit`](crate::Preedit) keeps it), then
    /// the [`ImeInput::Commit`](crate::ImeInput::Commit) to type. With
    /// `None`, or without this, the input method is disabled, so keys
    /// reach it as keys.
    pub fn ime_caret(mut self, caret: impl Fn(&W) -> Option<Rect> + 'a) -> Self {
        self.ime_caret = Some(Box::new(caret));
        self
    }

    /// This element left out of the tab order and never focused, for a
    /// widget that only shows (it still takes the pointer).
    pub fn unfocusable(mut self) -> Self {
        self.focusable = false;
        self
    }

    /// This element keeping the keyboard without the focus ring around
    /// it, as [`Canvas::without_ring`] does for a canvas: for content
    /// whose pane shows the focus another way (an editor or a book under a
    /// tab marked with the accent).
    pub fn without_ring(mut self) -> Self {
        self.ringless = true;
        self
    }

    /// This element described to screen readers by its widget, through
    /// `describe` (most widgets have an `access(frame, id)` of their own),
    /// rather than as a group named [`Custom::name`].
    pub fn access(mut self, describe: impl Fn(&W, &mut Frame, FocusId) + 'a) -> Self {
        self.access = Some(Box::new(describe));
        self
    }

    /// The name screen readers call this element by when its widget does
    /// not describe itself.
    pub fn name(mut self, name: &'a str) -> Self {
        self.name = Some(name);
        self
    }
}

impl<A: 'static, W: Widget + 'static> Spec<A> for Custom<'_, A, W> {
    type Widget = Embedded<A, W>;

    const KIND: &'static str = "custom";

    fn create(&self) -> Embedded<A, W> {
        Embedded {
            pending: Cell::new(None),
            rect: Rect::default(),
            widget: (self.create)(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> SyncProps<'b, W> {
        match &self.sync {
            Some(sync) => Some(sync.as_ref()),
            None => None,
        }
    }

    fn call(&self, value: Call<A>) -> Option<Call<A>> {
        Some(value)
    }

    fn draw(
        &self,
        widget: &mut Embedded<A, W>,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.rect = rect;
        frame.clipped(rect, |frame| {
            widget.widget.draw(frame, rect, theme);
        });
        rect.into()
    }

    fn describe(&self, widget: &Embedded<A, W>, frame: &mut Frame, id: FocusId) {
        match &self.access {
            Some(describe) => describe(&widget.widget, frame, id),
            None => describe_as(frame, id, widget.rect, Role::Group, self.name),
        }
    }

    fn act(
        &self,
        _widget: &mut Embedded<A, W>,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<Used> {
        None
    }

    fn handle(&self, widget: &mut Embedded<A, W>, input: &Input, cx: &mut EventCx<'_>) -> Used {
        let response = widget.widget.handle(input, cx);
        let used = !response.is_ignored();
        if let Some(respond) = &self.respond {
            widget.pending.set(respond(&widget.widget, response));
        }
        Used(used)
    }

    fn grows(&self) -> bool {
        self.measure.is_none()
    }

    fn is_focusable(&self) -> bool {
        self.focusable
    }

    fn draws_focus_ring(&self) -> bool {
        !self.ringless
    }

    fn takes_pointer(&self) -> bool {
        true
    }

    fn caret(&self, widget: &Embedded<A, W>) -> Option<Rect> {
        self.ime_caret.as_ref()?(&widget.widget)
    }

    fn measure_widget(
        &self,
        widget: &Embedded<A, W>,
        width: f32,
        metrics: CellMetrics,
        scale: f64,
    ) -> Option<f32> {
        let measure = self.measure.as_ref()?;
        Some(measure(&widget.widget, width, metrics, scale))
    }
}

impl<'a, A: 'static, W: Widget + 'static> From<Custom<'a, A, W>> for Element<'a, A> {
    fn from(custom: Custom<'a, A, W>) -> Self {
        Element::new(Kind::Control(Box::new(custom)))
    }
}

// ========================================================================
// Embedded Implementations
// ========================================================================

impl<A, W: Widget> Widget for Embedded<A, W> {
    type Output = ();
    type Response = Used;

    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Used {
        Used(!self.widget.handle(input, cx).is_ignored())
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.widget.draw(frame, area, theme);
    }

    fn set_focused(&mut self, focused: bool) {
        self.widget.set_focused(focused);
    }

    fn is_focused(&self) -> bool {
        self.widget.is_focused()
    }

    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.widget.about_to_wait(cx)
    }
}

impl<A, W: Widget> Control for Embedded<A, W> {
    type Props<'b>
        = SyncProps<'b, W>
    where
        Self: 'b;
    type Value = Call<A>;

    fn sync(&mut self, sync: SyncProps<'_, W>) {
        if let Some(sync) = sync {
            sync(&mut self.widget);
        }
    }

    /// Never asked: a custom element is measured by its own closure, or
    /// takes the room left.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, _response: Used) -> Option<Call<A>> {
        self.pending.take()
    }
}

// ========================================================================
// Canvas Implementations
// ========================================================================

impl<'a, A> Canvas<'a, A> {
    /// This canvas taking the keyboard, in reading order with a focus
    /// ring, while `accepts` says yes to a key, `apply` running on the
    /// app's state with each key it takes; a key it does not take goes on
    /// to the containers around it and the app. Pressing it focuses it.
    pub fn on_key(
        mut self,
        accepts: impl Fn(Key) -> bool + 'a,
        apply: impl Fn(&mut A, Key) + 'static,
    ) -> Self {
        self.keys = Some((Box::new(accepts), Rc::new(apply)));
        self
    }

    /// This canvas taking the keyboard (in reading order with a focus
    /// ring, and when pressed, as a control is) though it takes no keys
    /// itself: every key goes on to the containers around it and the app.
    /// For content the app drives with its own keymap (a document's
    /// pages), so the keyboard rests on it rather than on a button that
    /// would take Enter or Space; [`Canvas::on_key`] makes it focusable
    /// too.
    pub fn focusable(mut self) -> Self {
        self.focusable = true;
        self
    }

    /// This canvas keeping the keyboard without the focus ring around it,
    /// for content whose pane shows the focus another way (a document's
    /// pages, under a tab marked with the accent). Buttons and fields keep
    /// their rings.
    pub fn without_ring(mut self) -> Self {
        self.ringless = true;
        self
    }

    /// This canvas as tall as `measure` says for a width in physical
    /// pixels, rather than taking the room its column leaves; a fixed size
    /// is [`Element::height`].
    pub fn measure(mut self, measure: impl Fn(f32) -> f32 + 'a) -> Self {
        self.measure = Some(Box::new(measure));
        self
    }

    /// The name screen readers call this canvas by.
    pub fn name(mut self, name: &'a str) -> Self {
        self.name = Some(name);
        self
    }
}

impl<A: 'static> Spec<A> for Canvas<'_, A> {
    type Widget = CanvasBox<A>;

    const KIND: &'static str = "canvas";

    fn create(&self) -> CanvasBox<A> {
        CanvasBox {
            focused: false,
            pending: Cell::new(None),
            rect: Rect::default(),
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) {}

    fn call(&self, value: Call<A>) -> Option<Call<A>> {
        Some(value)
    }

    fn draw(
        &self,
        widget: &mut CanvasBox<A>,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.rect = rect;
        frame.clipped(rect, |frame| (self.draw)(frame, rect, theme));
        rect.into()
    }

    fn describe(&self, widget: &CanvasBox<A>, frame: &mut Frame, id: FocusId) {
        describe_as(frame, id, widget.rect, Role::Canvas, self.name);
    }

    fn act(&self, _widget: &mut CanvasBox<A>, _id: FocusId, _action: AccessAction) -> Option<Used> {
        None
    }

    fn handle(&self, widget: &mut CanvasBox<A>, input: &Input, _cx: &mut EventCx<'_>) -> Used {
        let (Input::Key(key), Some((accepts, apply))) = (input, &self.keys) else {
            return Used(false);
        };
        if !widget.focused || !accepts(*key) {
            return Used(false);
        }
        let (apply, key) = (Rc::clone(apply), *key);
        widget
            .pending
            .set(Some(Box::new(move |app: &mut A| apply(app, key))));
        Used(true)
    }

    fn grows(&self) -> bool {
        self.measure.is_none()
    }

    fn is_focusable(&self) -> bool {
        self.focusable || self.keys.is_some()
    }

    fn draws_focus_ring(&self) -> bool {
        !self.ringless
    }

    fn measure_widget(
        &self,
        _widget: &CanvasBox<A>,
        width: f32,
        _metrics: CellMetrics,
        _scale: f64,
    ) -> Option<f32> {
        let measure = self.measure.as_ref()?;
        Some(measure(width))
    }
}

impl<'a, A: 'static> From<Canvas<'a, A>> for Element<'a, A> {
    fn from(canvas: Canvas<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(canvas)))
    }
}

// ========================================================================
// CanvasBox Implementations
// ========================================================================

impl<A> Widget for CanvasBox<A> {
    type Output = ();
    type Response = Used;

    /// Uses nothing: the canvas element hands its keys out itself.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) -> Used {
        Used(false)
    }

    /// Draws nothing: the canvas element's closure paints.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl<A> Control for CanvasBox<A> {
    type Props<'b>
        = ()
    where
        Self: 'b;
    type Value = Call<A>;

    fn sync(&mut self, _props: ()) {}

    /// Never asked: a canvas is measured by its own closure, or takes the
    /// room left.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, _response: Used) -> Option<Call<A>> {
        self.pending.take()
    }
}

// ========================================================================
// Used Implementations
// ========================================================================

impl Outcome for Used {
    fn is_ignored(&self) -> bool {
        !self.0
    }
}

// ========================================================================
// Functions
// ========================================================================

/// An element embedding the hand-driven widget `create` makes, the first
/// time the element is seen under its id; the store keeps it from then on,
/// hands it the inputs the element gets, steps its animation, and draws it
/// in the element's area (clipped to it). [`Custom::sync`] copies the
/// app's state into it, [`Custom::on_response`] brings its answers back to
/// the app, and [`Custom::access`] describes it. It takes the room its
/// column leaves unless [`Custom::measure`] or [`Element::height`] says
/// otherwise, and the keyboard unless [`Custom::unfocusable`].
pub fn custom<'a, A, W: Widget + 'static>(create: impl Fn() -> W + 'a) -> Custom<'a, A, W> {
    Custom {
        access: None,
        create: Box::new(create),
        focusable: true,
        ime_caret: None,
        measure: None,
        name: None,
        respond: None,
        ringless: false,
        sync: None,
    }
}

/// An element painted by `draw`, given the frame, the element's area in
/// physical pixels (drawing is clipped to it), and the theme; it reads
/// the app's state through what it captures from
/// [`App::view`](crate::App::view). It takes the room its column leaves
/// unless [`Canvas::measure`] or [`Element::height`] says otherwise, and
/// takes the keyboard only with [`Canvas::on_key`] or
/// [`Canvas::focusable`], focused then when pressed. The pointer over it
/// is [`Element::on_press`], [`Element::on_pointer_drag`],
/// [`Element::on_pointer_move`], [`Element::on_hover`], and
/// [`Element::on_scroll`], each with points from its top left.
///
/// The frame draws text three ways: [`Frame::draw_plain_text`] and
/// [`Frame::draw_text`] on the cell grid, [`draw_label`](crate::draw_label)
/// in the theme's label font, and [`Frame::draw_paragraph`] in the
/// proportional (UI) font whatever the theme. A label at a point, such as
/// a graph node's name, is a [`Paragraph`](crate::Paragraph) drawn in a
/// rectangle one line tall and as wide as
/// [`Frame::paragraph_width`] says, a pixel to spare so it never wraps:
///
/// ```
/// use misty::view::canvas;
/// use misty::{App, Element, Paragraph, Rect, TextColor, Theme, ThemeToken, WindowId};
///
/// struct Graph {
///     /// Each node's name and middle, from the canvas's top left.
///     nodes: Vec<(String, f32, f32)>,
///     theme: Theme,
/// }
///
/// impl App for Graph {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         canvas(|frame, area, theme| {
///             let color = TextColor::Token(ThemeToken::Foreground);
///             for (name, x, y) in &self.nodes {
///                 let label = Paragraph::plain(name.as_str(), color);
///                 let width = frame.paragraph_width(&label).ceil() + 1.0;
///                 let height = frame.cell_metrics().height;
///                 let left = area.x + x - width / 2.0;
///                 let at = Rect::new(left, area.y + y, width, height);
///                 frame.draw_paragraph(&label, at, theme);
///             }
///         })
///         .into()
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
/// ```
pub fn canvas<'a, A>(draw: impl Fn(&mut Frame, Rect, &Theme) + 'a) -> Canvas<'a, A> {
    Canvas {
        draw: Box::new(draw),
        focusable: false,
        keys: None,
        measure: None,
        name: None,
        ringless: false,
    }
}

/// Describes an element at `rect` to screen readers as `role`, called
/// `name`, under `id`.
fn describe_as(frame: &mut Frame, id: FocusId, rect: Rect, role: Role, name: Option<&str>) {
    let mut node = AccessNode::new(role);
    if let Some(name) = name {
        node.set_label(name);
    }
    node.add_action(Action::Focus);
    frame.access(id, rect, node);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use accesskit::Role;

    use super::*;
    use crate::app::context::WindowAction;
    use crate::app::ime::{Preedit, PreeditChange};
    use crate::base::geometry::Point;
    use crate::base::theme::ThemeToken;
    use crate::input::event::PointerButton;
    use crate::testing::{TreeWindow, access_tree};
    use crate::view::element::column;
    use crate::view::props::{Extent, PointerDrag, Press};
    use crate::widgets::code_view::{CodeView, CodeViewResponse};
    use crate::{App, AppCx, KeyCode, WindowId};

    /// An app showing a code view and a canvas, recording what reaches it.
    struct Probe {
        /// Where the code view's cursor is, as its responses said.
        cursor: Option<usize>,
        /// The canvas's pointer drags.
        drags: Vec<PointerDrag>,
        /// The keys no element took.
        keys: Vec<Key>,
        /// The keys the canvas took.
        moves: Vec<Key>,
        /// The canvas's presses.
        presses: Vec<Press>,
        /// The code view's text.
        source: String,
        /// The colors it is drawn in.
        theme: Theme,
    }

    impl App for Probe {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            column([
                custom(|| CodeView::new(""))
                    .sync(|view: &mut CodeView| {
                        if view.text() != self.source {
                            view.set_text(self.source.as_str());
                        }
                    })
                    .on_response(
                        |view: &CodeView, response| {
                            (response == CodeViewResponse::Moved).then(|| view.cursor())
                        },
                        |probe: &mut Probe, cursor| probe.cursor = Some(cursor),
                    )
                    .access(|view: &CodeView, frame, id| view.access(frame, id))
                    .into(),
                Element::from(
                    canvas(|frame, rect, theme| {
                        frame.fill_rect(rect, theme.color(ThemeToken::Accent));
                    })
                    .on_key(
                        |key| matches!(key.code, KeyCode::Char('j' | 'k')),
                        |probe: &mut Probe, key| probe.moves.push(key),
                    )
                    .name("Board"),
                )
                .height(Extent::Px(80.0))
                .on_press(|probe: &mut Probe, press| probe.presses.push(press))
                .on_pointer_drag(|probe: &mut Probe, drag| probe.drags.push(drag)),
                canvas(|_, _, _| {})
                    .measure(|width| width / 4.0)
                    .name("Strip")
                    .into(),
            ])
        }

        fn key(&mut self, key: Key, _cx: &mut AppCx<'_>) -> bool {
            self.keys.push(key);
            true
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window 400 by 400 pixels showing the probe.
    fn window() -> TreeWindow<Probe> {
        let probe = Probe {
            cursor: None,
            drags: Vec::new(),
            keys: Vec::new(),
            moves: Vec::new(),
            presses: Vec::new(),
            source: "first line\nsecond line\n".to_owned(),
            theme: Theme::dark(),
        };
        TreeWindow::new(probe, "t", 400, 400)
    }

    /// Where the node of `role` was described, the first if several.
    fn bounds(window: &TreeWindow<Probe>, role: Role) -> Rect {
        let tree = access_tree(window.frame(), "t");
        let (_, node) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == role)
            .unwrap_or_else(|| panic!("no {role:?}"));
        let rect = node.bounds().expect("described with bounds");
        Rect::new(
            rect.x0 as f32,
            rect.y0 as f32,
            (rect.x1 - rect.x0) as f32,
            (rect.y1 - rect.y0) as f32,
        )
    }

    #[test]
    fn test_an_embedded_widget_keeps_its_state_takes_the_apps_and_answers_back() {
        let mut window = window();
        assert!(bounds(&window, Role::MultilineTextInput).height > 0.0);
        // The view, first in reading order, has the keyboard; its own keys
        // move its cursor, which comes back to the app through the
        // response.
        window.press("S-<right> S-<right>");
        assert_eq!(window.app().cursor, Some(2));
        // A rebuild for another reason keeps the cursor the view knows.
        window.update(|probe| probe.keys.clear());
        window.press("S-<right>");
        assert_eq!(window.app().cursor, Some(3));
        // The app's text reaches the view at the next rebuild.
        window.update(|probe| probe.source = "other".to_owned());
        window.press("C-a");
        assert_eq!(window.app().cursor, Some("other".len()));
    }

    #[test]
    fn test_a_canvas_paints_its_area_and_takes_the_keys_it_accepts() {
        let mut window = window();
        let board = bounds(&window, Role::Canvas);
        assert_eq!(board.height, 80.0);
        let accent = window.app().theme.color(ThemeToken::Accent).into();
        let painted = window
            .frame()
            .draw
            .fills()
            .iter()
            .any(|quad| quad.rect == board && quad.color == accent);
        assert!(painted, "the canvas fills its own area");
        window.press("<tab> j k x");
        let moves: Vec<KeyCode> = window.app().moves.iter().map(|key| key.code).collect();
        assert_eq!(moves, [KeyCode::Char('j'), KeyCode::Char('k')]);
        assert_eq!(window.app().keys.len(), 1, "x goes on to the app");
    }

    #[test]
    fn test_a_canvas_reports_presses_and_drags_from_its_top_left() {
        let mut window = window();
        let board = bounds(&window, Role::Canvas);
        let from = Point::new(board.x + 10.0, board.y + 20.0);
        window.click_at(from, PointerButton::Left);
        assert_eq!(window.app().presses[0].point, Point::new(10.0, 20.0));
        // A click is a drag that ended where it began.
        let click = window.app().drags.clone();
        assert_eq!(click.len(), 1);
        assert!(click[0].done && click[0].point == click[0].start);
        window.update(|probe| probe.drags.clear());
        let to = Point::new(board.x + 50.0, board.y + board.height + 30.0);
        window.drag(from, to);
        let drags = &window.app().drags;
        let last = drags.last().expect("the drag reported");
        assert!(last.done && drags[..drags.len() - 1].iter().all(|drag| !drag.done));
        assert_eq!(last.start, Point::new(10.0, 20.0));
        // Past the canvas's bottom edge, the canvas still has the pointer.
        assert_eq!(last.point, Point::new(50.0, board.height + 30.0));
        assert_eq!(last.button, PointerButton::Left);
    }

    #[test]
    fn test_a_middle_drag_is_reported_as_one_and_a_right_drag_is_not() {
        let mut window = window();
        let board = bounds(&window, Role::Canvas);
        let from = Point::new(board.x + 10.0, board.y + 20.0);
        let to = Point::new(board.x + 40.0, board.y + 10.0);
        window.drag_with(PointerButton::Middle, from, to);
        let drags = window.app().drags.clone();
        let last = drags.last().expect("the middle drag reported");
        assert!(last.done, "its release ends it");
        assert!(
            drags
                .iter()
                .all(|drag| drag.button == PointerButton::Middle)
        );
        assert_eq!(last.point, Point::new(40.0, 10.0));
        window.update(|probe| probe.drags.clear());
        window.drag_with(PointerButton::Right, from, to);
        assert!(
            window.app().drags.is_empty(),
            "no drag with the right button"
        );
    }

    #[test]
    fn test_a_measured_canvas_is_as_tall_as_its_closure_says() {
        let window = window();
        let tree = access_tree(window.frame(), "t");
        let strip = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Strip"))
            .and_then(|(_, node)| node.bounds())
            .expect("the strip described");
        assert_eq!(strip.y1 - strip.y0, (strip.x1 - strip.x0) / 4.0);
    }

    /// Pages painted on a key-less canvas taking pointer drags, above a
    /// toolbar button.
    struct Pages {
        /// How many times the toolbar button was clicked.
        clicks: usize,
        /// The keys no element took.
        keys: Vec<Key>,
        /// The colors it is drawn in.
        theme: Theme,
    }

    impl App for Pages {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            column([
                Element::from(canvas(|_, _, _| {}).focusable().name("Pages"))
                    .grow(1.0)
                    .on_pointer_drag(|_: &mut Pages, _| {}),
                crate::view::controls::button("Zoom")
                    .on_click(|pages: &mut Pages| pages.clicks += 1)
                    .into(),
            ])
        }

        fn key(&mut self, key: Key, _cx: &mut AppCx<'_>) -> bool {
            self.keys.push(key);
            true
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_a_pressed_canvas_takes_the_keyboard_from_a_button_and_passes_its_keys_on() {
        let pages = Pages {
            clicks: 0,
            keys: Vec::new(),
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(pages, "t", 400, 300);
        // The canvas is the first tab stop; Tab moves on to the button.
        window.press("<tab>");
        let at = window.center(Role::Canvas, "Pages");
        window.click_at(at, PointerButton::Left);
        window.press("<cr> j");
        assert_eq!(window.app().clicks, 0, "the button lost the keyboard");
        let codes: Vec<KeyCode> = window.app().keys.iter().map(|key| key.code).collect();
        assert_eq!(codes, [KeyCode::Enter, KeyCode::Char('j')]);
    }

    /// A hand-driven widget taking text from the input method while
    /// `composing`, as an editor in Insert mode does.
    #[derive(Default)]
    struct Typist {
        /// Whether it takes text now.
        composing: bool,
        /// Where it was drawn.
        drawn: Rect,
        /// What the input method is composing.
        preedit: Option<Preedit>,
        /// What it was typed.
        typed: String,
    }

    impl Widget for Typist {
        type Output = ();
        type Response = Used;

        fn handle(&mut self, input: &Input, _cx: &mut EventCx<'_>) -> Used {
            let Input::Ime(ime) = input else {
                return Used(false);
            };
            if let PreeditChange::Commit(text) = Preedit::apply(&mut self.preedit, ime) {
                self.typed.push_str(&text);
            }
            Used(true)
        }

        fn draw(&mut self, _frame: &mut Frame, area: Rect, _theme: &Theme) {
            self.drawn = area;
        }
    }

    impl Typist {
        /// Its caret, a line tall at its top left, while it takes text.
        fn caret(&self) -> Option<Rect> {
            let drawn = self.drawn;
            self.composing
                .then(|| Rect::new(drawn.x + 3.0, drawn.y, 1.0, 16.0))
        }
    }

    /// An app embedding a [`Typist`], keeping what it typed.
    struct Editor {
        /// Whether the typist takes text.
        composing: bool,
        /// What it typed, as its responses said.
        preedit: Option<String>,
        /// The colors it is drawn in.
        theme: Theme,
        /// What it typed.
        typed: String,
    }

    impl App for Editor {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            column([custom(Typist::default)
                .sync(|typist: &mut Typist| typist.composing = self.composing)
                .ime_caret(Typist::caret)
                .on_response(
                    |typist: &Typist, _| {
                        let preedit = typist.preedit.as_ref().map(|p| p.text().to_owned());
                        Some((typist.typed.clone(), preedit))
                    },
                    |editor: &mut Editor, (typed, preedit)| {
                        editor.typed = typed;
                        editor.preedit = preedit;
                    },
                )
                .name("Editor")
                .into()])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_an_embedded_widget_s_caret_anchors_the_input_method_and_takes_its_text() {
        use crate::input::event::{Composition, ImeInput};
        let editor = Editor {
            composing: true,
            preedit: None,
            theme: Theme::dark(),
            typed: String::new(),
        };
        let mut window = TreeWindow::new(editor, "t", 400, 200);
        assert_eq!(window.ime_area(), None, "not focused yet");
        window.press("<tab>");
        let area = window.ime_area().expect("the input method anchored");
        assert_eq!((area.x, area.height), (3.0, 16.0));
        window.about_to_wait();
        let actions = window.window().take_actions();
        assert!(
            actions.contains(&WindowAction::SetImeAllowed(true)),
            "{actions:?}"
        );
        assert!(
            actions.contains(&WindowAction::SetImeArea(area)),
            "{actions:?}"
        );
        window.send(&Input::Ime(ImeInput::Preedit(Composition {
            cursor: Some((2, 2)),
            text: "ni".into(),
        })));
        assert_eq!(window.app().preedit.as_deref(), Some("ni"));
        window.send(&Input::Ime(ImeInput::Commit("你".into())));
        assert_eq!(window.app().typed, "你");
        assert_eq!(window.app().preedit, None);
        window.update(|editor| editor.composing = false);
        assert_eq!(window.ime_area(), None, "keys as keys again");
        window.about_to_wait();
        let actions = window.window().take_actions();
        assert!(
            actions.contains(&WindowAction::SetImeAllowed(false)),
            "{actions:?}"
        );
    }
}

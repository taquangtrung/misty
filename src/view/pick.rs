//! [`quick_pick`]: a type-to-filter picker over the window, as a layer an
//! app shows while its state says the picker is open. The session (the
//! query typed, the rows matched, the selection) is the view's, kept by
//! misty while the layer shows and dropped when it goes; what choosing a
//! row does reaches the app through [`QuickPick::on_run`], and closing it
//! through [`QuickPick::on_close`].

use std::rc::Rc;
use std::time::Instant;

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::super::render::image::IconSet;
use super::super::widgets::picker::{Picker, Reply};
use super::super::widgets::quick_pick;
use super::controls::{Callback, call};
use super::element::{Element, Kind, Overlay, overlay};
use super::node::{Call, Drawn, Spec};

// ========================================================================
// Data Structures
// ========================================================================

/// A picker over the window, filtering rows as the user types, as a
/// layer. Made by [`quick_pick`], shown with
/// [`Element::overlay_if`](crate::Element::overlay_if).
pub struct QuickPick<'a, A, O, P = ()> {
    /// The icons the rows name, when they name any.
    icons: Option<&'a IconSet>,
    /// What closing the picker without a choice runs.
    on_close: Option<Callback<A, ()>>,
    /// What choosing a row runs, with what the row does.
    on_run: Option<Callback<A, O>>,
    /// Opens the session the first time the layer shows.
    open: Box<dyn Fn() -> Picker<O, P> + 'a>,
}

/// The widget kept for a [`QuickPick`]: the picker's view and its
/// session.
pub(crate) struct PickBox<O, P> {
    /// The view of the session.
    pick: quick_pick::QuickPick<O, P>,
}

/// What the user did with a picker that the app hears of.
pub(crate) enum Picked<O> {
    /// Closed it without a choice.
    Closed,
    /// Chose a row doing this.
    Ran(O),
}

// ========================================================================
// QuickPick Implementations
// ========================================================================

impl<'a, A, O, P> QuickPick<'a, A, O, P> {
    /// This picker drawing the icons its rows name
    /// ([`Row::with_icon`](crate::picker::Row::with_icon)) from
    /// `icons`; taken when the picker first shows.
    pub fn icons(self, icons: &'a IconSet) -> Self {
        Self {
            icons: Some(icons),
            ..self
        }
    }

    /// What runs on the app's state with what the row the user chose does
    /// (Enter, a click): carry it out, and close the picker by no longer
    /// showing it.
    pub fn on_run(self, apply: impl Fn(&mut A, O) + 'static) -> Self {
        Self {
            on_run: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state when the user closes the picker
    /// without a choice (Esc, a click outside it): stop showing it.
    pub fn on_close(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_close: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }
}

impl<A: 'static, O: 'static, P: 'static> Spec<A> for QuickPick<'_, A, O, P> {
    type Widget = PickBox<O, P>;

    const KIND: &'static str = "quick pick";

    fn create(&self) -> PickBox<O, P> {
        let pick = quick_pick::QuickPick::new((self.open)());
        PickBox {
            pick: match self.icons {
                Some(icons) => pick.with_icons(icons.clone()),
                None => pick,
            },
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) {}

    fn call(&self, picked: Picked<O>) -> Option<Call<A>> {
        match picked {
            Picked::Closed => call(self.on_close.as_ref(), ()),
            Picked::Ran(outcome) => call(self.on_run.as_ref(), outcome),
        }
    }

    /// Draws the picker over the whole window, `rect`, which takes the
    /// pointer: a click outside the picker's box closes it.
    fn draw(
        &self,
        widget: &mut PickBox<O, P>,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.pick.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &PickBox<O, P>, frame: &mut Frame, id: FocusId) {
        widget.pick.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut PickBox<O, P>,
        id: FocusId,
        action: AccessAction,
    ) -> Option<Reply<O>> {
        match widget.pick.access_action(id, action) {
            Reply::Ignored => None,
            reply => Some(reply),
        }
    }

    fn caret(&self, widget: &PickBox<O, P>) -> Option<Rect> {
        widget.pick.cursor_area()
    }
}

impl<'a, A: 'static, O: 'static, P: 'static> From<QuickPick<'a, A, O, P>> for Overlay<'a, A> {
    /// The picker as a modal layer: it keeps the keyboard and the pointer
    /// while it shows, and gives the focus back when it goes.
    fn from(pick: QuickPick<'a, A, O, P>) -> Self {
        overlay(Element::new(Kind::Control(Box::new(pick)))).modal()
    }
}

// ========================================================================
// PickBox Implementations
// ========================================================================

impl<O, P> Widget for PickBox<O, P> {
    type Output = ();
    type Response = Reply<O>;

    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Reply<O> {
        self.pick.handle(input, cx)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.pick.draw(frame, area, theme);
    }

    fn set_focused(&mut self, focused: bool) {
        self.pick.set_focused(focused);
    }

    /// Takes in the rows the source and the matcher sent.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.pick.about_to_wait(cx)
    }
}

impl<O: 'static, P: 'static> Control for PickBox<O, P> {
    type Props<'a> = ();
    type Value = Picked<O>;

    /// Nothing: the session is the view's own.
    fn sync(&mut self, (): ()) {}

    /// Nothing: the picker places itself in the window.
    fn measure(&self, _width: f32, _metrics: CellMetrics, _scale_factor: f64) -> f32 {
        0.0
    }

    fn changed(&self, reply: Reply<O>) -> Option<Picked<O>> {
        match reply {
            Reply::Close => Some(Picked::Closed),
            Reply::Run(outcome) => Some(Picked::Ran(outcome)),
            _ => None,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A picker over the window that `open` opens a session of, the first
/// time the layer shows (and again each time it shows after it went):
/// a heading with the count, the query line, the rows filtered as the user
/// types, and a line naming the keys. Up and Down (`C-p`, `C-n`) move the
/// selection, Enter or a click runs the selected row, and Esc or a click
/// outside closes it. Show it with
/// [`Element::overlay_if`](crate::Element::overlay_if) while the app's
/// state says it is open:
///
/// ```
/// use misty::picker::{FixedSource, Picker, PickerMemory, Row, RowKey};
/// use misty::view::{column, quick_pick, text};
/// use misty::{App, Element, Theme, WindowId};
///
/// struct Demo {
///     opened: Option<u64>,
///     picking: bool,
///     theme: Theme,
/// }
///
/// impl App for Demo {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         let open = || {
///             let rows = vec![Row::new(RowKey(1), "notes.txt"), Row::new(RowKey(2), "todo.md")];
///             let source = FixedSource::new("files", "Open", rows, "Open", |key| Some(key.0));
///             Picker::<u64>::synchronous(Box::new(source), PickerMemory::default())
///         };
///         column([text("editing").into()]).overlay_if(
///             self.picking,
///             quick_pick(open)
///                 .on_run(|app: &mut Self, file| {
///                     app.opened = Some(file);
///                     app.picking = false;
///                 })
///                 .on_close(|app: &mut Self| app.picking = false),
///         )
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
/// ```
///
/// A picker matching on its own thread wakes the event loop through the
/// [`Waker`](crate::Waker) [`App::start`](crate::App::start) gave the app:
/// `Picker::open(source, memory, waker.clone())`.
pub fn quick_pick<'a, A, O, P>(open: impl Fn() -> Picker<O, P> + 'a) -> QuickPick<'a, A, O, P> {
    QuickPick {
        icons: None,
        on_close: None,
        on_run: None,
        open: Box::new(open),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use accesskit::Role;

    use super::*;
    use crate::platform::window_id::WindowId;
    use crate::testing::TreeWindow;
    use crate::view::app::App;
    use crate::view::controls::checkbox;
    use crate::view::element::column;
    use crate::widgets::picker::{FixedSource, PickerMemory, Row, RowKey};

    /// An app opening a picker of two files.
    struct Demo {
        /// Whether the checkbox under the picker is on.
        on: bool,
        /// The file chosen last.
        opened: Option<u64>,
        /// Whether the picker shows.
        picking: bool,
        /// The colors it is drawn in.
        theme: Theme,
    }

    impl App for Demo {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let open = || {
                let rows = vec![
                    Row::new(RowKey(1), "notes.txt"),
                    Row::new(RowKey(2), "todo.md"),
                ];
                let source = FixedSource::new("files", "Open", rows, "Open", |key| Some(key.0));
                Picker::<u64>::synchronous(Box::new(source), PickerMemory::default())
            };
            column([checkbox("Wrap", self.on)
                .on_change(|demo: &mut Self, on| demo.on = on)
                .into()])
            .overlay_if(
                self.picking,
                quick_pick(open)
                    .on_run(|demo: &mut Self, file| {
                        demo.opened = Some(file);
                        demo.picking = false;
                    })
                    .on_close(|demo: &mut Self| demo.picking = false),
            )
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window showing the demo with its picker open, its rows taken in.
    fn picking() -> TreeWindow<Demo> {
        let demo = Demo {
            on: false,
            opened: None,
            picking: true,
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(demo, "t", 800, 600);
        window.about_to_wait();
        window
    }

    #[test]
    fn test_typing_filters_the_rows_and_enter_runs_the_one_left() {
        let mut window = picking();
        let drawn = window.drawn_text().join(" ");
        assert!(
            drawn.contains("notes.txt") && drawn.contains("todo.md"),
            "{drawn}"
        );
        window.type_text("todo");
        window.about_to_wait();
        let drawn = window.drawn_text().join(" ");
        assert!(!drawn.contains("notes.txt"), "{drawn}");
        window.press("RET");
        assert_eq!(window.app().opened, Some(2));
        assert!(!window.app().picking);
    }

    #[test]
    fn test_the_rows_are_described_and_a_click_on_one_runs_it() {
        let mut window = picking();
        let tree = window.ui_text();
        assert!(
            tree.contains("Dialog \"Open\"") && tree.contains("ListItem \"todo.md\""),
            "{tree}"
        );
        let row = window.find(Role::ListItem, "todo.md");
        window.act(AccessAction::Click(row));
        assert_eq!(window.app().opened, Some(2));
        assert!(!window.app().picking);
    }

    #[test]
    fn test_esc_closes_the_picker_and_the_keys_reach_the_window_again() {
        let mut window = picking();
        window.press("<space>");
        assert!(
            !window.app().on,
            "the picker keeps the keyboard while it shows"
        );
        window.press("<esc>");
        assert!(!window.app().picking);
        assert_eq!(window.app().opened, None);
        window.press("<space>");
        assert!(window.app().on, "the focus went back to the checkbox");
    }

    #[test]
    fn test_a_picker_shown_again_opens_a_new_session() {
        let mut window = picking();
        window.type_text("todo");
        window.press("<esc>");
        window.update(|demo| demo.picking = true);
        window.about_to_wait();
        let drawn = window.drawn_text().join(" ");
        assert!(
            drawn.contains("notes.txt"),
            "the query was dropped: {drawn}"
        );
    }
}

//! [`ViewHost`]: a tree of elements drawn in an area of a window that
//! misty does not run, for an app with a window of its own (a terminal, an
//! editor) that shows a page described by a view in one of its panes. The
//! state the tree is built from is a [`HostedView`]: misty lays the tree
//! out in the area, draws it, routes the inputs the app hands it through
//! it, and runs the callbacks its controls were given on the state, as it
//! does for an [`App`](crate::App)'s window.
//!
//! The app stays in charge of its window: it decides which inputs reach
//! the host (the keys while its pane has the focus, the pointer over the
//! area) and learns from [`ViewHost::handle`] whether the tree used each
//! one, so a key no element took goes on to the app's own keymap.
//!
//! ```
//! use misty::view::{checkbox, column};
//! use misty::{Element, FocusId, HostedView, ViewHost};
//!
//! struct Page {
//!     wrap: bool,
//! }
//!
//! impl HostedView for Page {
//!     fn view(&self) -> Element<'_, Self> {
//!         column([checkbox("Wrap long lines", self.wrap)
//!             .on_change(|page: &mut Self, on| page.wrap = on)
//!             .into()])
//!     }
//! }
//!
//! // One host per page shown, each under an id of its own.
//! let host: ViewHost<Page> = ViewHost::new(FocusId::new("pane").child(1));
//! # let _ = host;
//! ```

use std::fmt;
use std::marker::PhantomData;
use std::time::Instant;

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::base::geometry::Rect;
use super::super::base::key::Key;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::app::AppCx;
use super::element::Element;
use super::host::Host;
use super::store::Store;

// ========================================================================
// Data Structures
// ========================================================================

/// A tree of elements in an area of a window an app draws itself, built
/// from a [`HostedView`]: what lasts of the tree between rebuilds (each
/// control's widget, the focus, the scroll), as a window's tree keeps it.
pub struct ViewHost<S> {
    /// The state the tree is built from, by type only: the app lends it
    /// for each call.
    state: PhantomData<fn(&mut S)>,
    /// The tree's widgets, focus, and layout.
    store: Store,
}

/// A [`HostedView`] lent for one call, as the [`Host`] a store drives.
struct Lent<'a, S>(&'a mut S);

/// How far [`ViewHost::scroll`] moves a scrolling container, gliding
/// there: for the app's own keys (`C-e`, PageDown, `g g`), which the view
/// does not take.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ScrollStep {
    /// To the end of the content.
    End,
    /// This many viewports down, up for negative.
    Pages(f32),
    /// This many pixels down, up for negative.
    Pixels(f32),
    /// This many rows of text down, up for negative.
    Rows(f32),
    /// To the start of the content.
    Start,
}

// ========================================================================
// Traits
// ========================================================================

/// State described by a view and shown in an area of a window through a
/// [`ViewHost`]: the hosted counterpart of an [`App`](crate::App), with
/// only the hooks an area has.
pub trait HostedView: Sized + 'static {
    /// The tree, built from the state. Called on every input and every
    /// frame, so it only reads the state; the controls' callbacks change
    /// it.
    fn view(&self) -> Element<'_, Self>;

    /// A key no element took: returns whether the state used it, else
    /// [`ViewHost::handle`] says the key was left, for the app's own
    /// keymap. The default uses none.
    fn key(&mut self, _key: Key, _cx: &mut EventCx<'_>) -> bool {
        false
    }
}

// ========================================================================
// ViewHost Implementations
// ========================================================================

impl<S: HostedView> ViewHost<S> {
    /// A host whose tree makes its ids under `id`, which no other host in
    /// the window shares, so two hosts showing the same view (the same
    /// page in two panes) keep their focus and their screen reader nodes
    /// apart.
    pub fn new(id: FocusId) -> Self {
        Self {
            state: PhantomData,
            store: Store::scoped(id),
        }
    }

    /// Draws `state`'s tree over `area` of `frame` in `theme`: the area
    /// filled with the theme's background, the tree laid out in it, its
    /// layers, its tooltip, and the focus ring.
    pub fn draw(&mut self, state: &S, frame: &mut Frame, area: Rect, theme: &Theme) {
        frame.fill_rect(area, theme.background);
        self.store.render_view(state.view(), theme, frame, area);
    }

    /// Hands `input` to the element of `state`'s tree it is for, against
    /// where the last [`ViewHost::draw`] put each one, runs the callbacks
    /// it made on `state`, and offers a key no element took to
    /// [`HostedView::key`]. Returns whether the tree or the state used
    /// `input`; one it did not is the app's.
    pub fn handle(&mut self, state: &mut S, input: &Input, cx: &mut EventCx<'_>) -> bool {
        self.store
            .host_input(&mut Lent(state), input, cx.reborrow())
            .1
    }

    /// Does what a screen reader's `action` asks of `state`'s tree, as
    /// the last [`ViewHost::draw`] described it: gives an element the
    /// keyboard, presses a button or a pressable element, or steps a
    /// control's value, running the callbacks it made on `state`. Returns
    /// whether an element took it.
    pub fn access_action(&mut self, state: &mut S, action: AccessAction) -> bool {
        self.store.act(&mut Lent(state), action).1
    }

    /// Steps the tree's animations and timers, running on `state` what
    /// time changed; returns when the next step is due, or `None` at rest.
    /// Call it each time the app's event loop is about to sleep.
    pub fn about_to_wait(&mut self, state: &mut S, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.store.host_about_to_wait(&mut Lent(state), cx)
    }

    /// Gives the keyboard to the element named `name`
    /// ([`Element::id`](crate::Element::id)) once the next frame draws it.
    pub fn focus(&mut self, name: impl Into<String>) {
        self.store.request_focus(name.into());
    }

    /// Where the focused control's caret was drawn last, for an input
    /// method's candidates; `None` while no text is being typed.
    pub fn ime_area(&self) -> Option<Rect> {
        self.store.ime_caret()
    }

    /// Moves the keyboard `delta` tab stops on (back for a negative
    /// `delta`), as Tab and Shift+Tab do: for the app's own keys (`j` and
    /// `k` down a page's rows).
    pub fn move_focus(&mut self, delta: isize) {
        self.store.move_focus(delta);
    }

    /// Scrolls the scrolling container named `name` by `step`, as the
    /// last frame laid it out; says whether that moved it (`false` at the
    /// end it went toward, or for no such container).
    pub fn scroll(&mut self, name: &str, step: ScrollStep) -> bool {
        self.store.scroll_named(name, step)
    }

    /// How far down the scrolling container named `name` is scrolled, in
    /// pixels, as last drawn.
    pub fn offset(&mut self, name: &str) -> Option<f32> {
        self.store.offset_named(name)
    }

    /// Where the element named `name` starts down the content of the
    /// scrolling container it is in, in pixels, as last drawn: to tell
    /// which of a page's sections is at the top.
    pub fn top_of(&self, name: &str) -> Option<f32> {
        self.store.top_named(name)
    }

    /// Scrolls the container around the element named `name` until the
    /// element is at its top, as far as the content goes; says whether
    /// that moved it.
    pub fn reveal_top(&mut self, name: &str) -> bool {
        self.store.reveal_top_named(name)
    }
}

impl<S> fmt::Debug for ViewHost<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewHost").finish_non_exhaustive()
    }
}

// ========================================================================
// Lent Implementations
// ========================================================================

impl<S: HostedView> Host for Lent<'_, S> {
    type State = S;

    fn view(&self) -> Element<'_, S> {
        self.0.view()
    }

    fn state(&mut self) -> &mut S {
        self.0
    }

    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        self.0.key(key, cx.event_cx())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::super::base::geometry::Point;
    use super::super::super::base::key::{KeyCode, Mods};
    use super::super::super::input::event::{ButtonInput, PointerButton};
    use super::super::super::testing::TestWindow;
    use super::super::controls::checkbox;
    use super::super::element::column;
    use super::*;

    /// A page of one checkbox, counting the keys it was left.
    #[derive(Default)]
    struct Page {
        left: usize,
        on: bool,
    }

    impl HostedView for Page {
        fn view(&self) -> Element<'_, Self> {
            column([checkbox("Wrap", self.on)
                .on_change(|page: &mut Self, on| page.on = on)
                .into()])
        }

        fn key(&mut self, _key: Key, _cx: &mut EventCx<'_>) -> bool {
            self.left += 1;
            false
        }
    }

    /// A page of two checkboxes, the second named.
    #[derive(Default)]
    struct Pair {
        first: bool,
        second: bool,
    }

    impl HostedView for Pair {
        fn view(&self) -> Element<'_, Self> {
            column([
                checkbox("First", self.first)
                    .on_change(|pair: &mut Self, on| pair.first = on)
                    .into(),
                Element::from(
                    checkbox("Second", self.second)
                        .on_change(|pair: &mut Self, on| pair.second = on),
                )
                .id("second"),
            ])
        }
    }

    #[test]
    fn test_the_app_gives_the_keyboard_to_a_named_element_of_a_hosted_view() {
        let mut window = TestWindow::new(400, 300);
        let mut host = ViewHost::new(FocusId::new("test.pair"));
        let mut pair = Pair::default();
        host.focus("second");
        for _ in 0..2 {
            let mut frame = window.frame();
            let bounds = frame.bounds();
            host.draw(&pair, &mut frame, bounds, &Theme::dark());
        }
        let space = Input::Key(Key::new(KeyCode::Char(' '), Mods::NONE));
        assert!(host.handle(&mut pair, &space, &mut window.cx()));
        assert!(pair.second && !pair.first, "the named one took the key");
    }

    /// A page of 100 rows in a scrolling body, the 50th named.
    struct Long;

    impl HostedView for Long {
        fn view(&self) -> Element<'_, Self> {
            let rows = (0..100).map(|at| {
                let line = column([super::super::element::text(format!("row {at}")).into()]);
                match at {
                    50 => line.id("fifty"),
                    _ => line,
                }
            });
            column([column(rows).scroll().id("body")])
        }
    }

    #[test]
    fn test_the_app_scrolls_a_named_container_and_brings_a_named_element_to_its_top() {
        let mut window = TestWindow::new(400, 300);
        window.set_reduced_motion(true);
        let mut host = ViewHost::new(FocusId::new("test.long"));
        let draw = |host: &mut ViewHost<Long>, window: &TestWindow| {
            let mut frame = window.frame();
            let bounds = frame.bounds();
            host.draw(&Long, &mut frame, bounds, &Theme::dark());
            frame.cell_metrics().height
        };
        let row = draw(&mut host, &window);
        assert_eq!(host.offset("body"), Some(0.0));
        assert!(host.scroll("body", ScrollStep::Rows(3.0)));
        draw(&mut host, &window);
        assert_eq!(host.offset("body"), Some(3.0 * row));
        assert!(host.reveal_top("fifty"));
        draw(&mut host, &window);
        assert_eq!(host.offset("body"), host.top_of("fifty"));
        assert!(host.scroll("body", ScrollStep::Start));
        draw(&mut host, &window);
        assert_eq!(host.offset("body"), Some(0.0));
        assert!(!host.scroll("body", ScrollStep::Rows(-1.0)), "at the top");
        assert!(
            !host.scroll("nothing", ScrollStep::End),
            "no such container"
        );
    }

    #[test]
    fn test_a_screen_reader_s_click_reaches_the_hosted_element() {
        let window = TestWindow::new(400, 300);
        let mut host = host();
        let mut page = Page::default();
        let mut frame = window.frame();
        let bounds = frame.bounds();
        host.draw(&page, &mut frame, bounds, &Theme::dark());
        let target = super::super::super::testing::node_named(&frame, "Wrap")
            .expect("the checkbox is described");
        assert!(host.access_action(&mut page, AccessAction::Click(target)));
        assert!(page.on, "clicked on");
    }

    fn host() -> ViewHost<Page> {
        ViewHost::new(FocusId::new("test.page"))
    }

    #[test]
    fn test_the_tree_is_laid_out_in_its_area_and_a_click_there_runs_its_callback() {
        let mut window = TestWindow::new(800, 600);
        let (mut host, mut page) = (host(), Page::default());
        let area = Rect::new(400.0, 300.0, 400.0, 300.0);
        let mut frame = window.frame();
        host.draw(&page, &mut frame, area, &Theme::dark());
        let click = |x: f32, y: f32| {
            Input::PointerButton(ButtonInput::press(PointerButton::Left, Point::new(x, y)))
        };
        let cell = frame.cell_metrics();
        let outside = click(cell.width, cell.height / 2.0);
        assert!(
            !host.handle(&mut page, &outside, &mut window.cx()),
            "left to the app"
        );
        assert!(!page.on);
        let inside = click(area.x + cell.width, area.y + cell.height / 2.0);
        assert!(host.handle(&mut page, &inside, &mut window.cx()));
        assert!(page.on, "the checkbox at the area's top left");
    }

    #[test]
    fn test_a_key_no_element_takes_is_offered_to_the_state_then_left_to_the_app() {
        let mut window = TestWindow::new(800, 600);
        let (mut host, mut page) = (host(), Page::default());
        let mut frame = window.frame();
        let bounds = frame.bounds();
        host.draw(&page, &mut frame, bounds, &Theme::dark());
        let key = |code| Input::Key(Key::new(code, Mods::NONE));
        assert!(
            host.handle(&mut page, &key(KeyCode::Tab), &mut window.cx()),
            "Tab focuses the checkbox"
        );
        assert!(host.handle(&mut page, &key(KeyCode::Char(' ')), &mut window.cx()));
        assert!(page.on, "Space flips the focused checkbox");
        assert!(!host.handle(&mut page, &key(KeyCode::Char('q')), &mut window.cx()));
        assert_eq!(page.left, 1, "offered to the state first");
    }
}

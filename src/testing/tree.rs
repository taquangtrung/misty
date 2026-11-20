//! [`TreeWindow`]: a headless window for an [`App`] described by a view,
//! built on [`TestWindow`]. It owns the app, draws its tree into a frame
//! after every input, finds elements by their role and accessible name,
//! and sends them keys, text, clicks, and screen reader actions, as the
//! runtime would, with no event loop and no display.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use winit::window;

use accesskit::{Node, NodeId, Role};

use super::super::app::access::AccessAction;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
#[cfg(test)]
use super::super::app::offscreen::Snapshot;
use super::super::app::window::Size;
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::parse_key_sequence;
use super::super::input::event::{ButtonInput, Input, PointerButton};
use super::super::platform::waker::{UserEvent, Waker};
use super::super::platform::window_id::WindowId;
use super::super::view::app::{App, AppCx, Asks, Update};
use super::super::view::store::Store;
use super::{TestWindow, access_tree, drawn_text, typed_key, ui_text};

// ========================================================================
// Constants
// ========================================================================

/// The moves a [`TreeWindow::drag`] makes between its press and release.
const DRAG_STEPS: usize = 4;

/// The raw id of the first window a [`TreeWindow`] shows, as a test
/// window's.
const FIRST_WINDOW_ID: u64 = 1;

// ========================================================================
// Data Structures
// ========================================================================

/// A window with no window system behind it for an [`App`]: the app, its
/// tree's state, and the frame drawn after the last input.
///
/// ```
/// use misty::accesskit::Role;
/// use misty::testing::TreeWindow;
/// use misty::view::{checkbox, column};
/// use misty::{App, Element, Theme, WindowId};
///
/// struct Demo {
///     theme: Theme,
///     wrap: bool,
/// }
///
/// impl App for Demo {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         column([checkbox("Wrap", self.wrap)
///             .on_change(|app: &mut Self, on| app.wrap = on)
///             .into()])
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
///
/// let demo = Demo { theme: Theme::dark(), wrap: false };
/// let mut window = TreeWindow::new(demo, "Demo", 400, 200);
/// window.click(Role::CheckBox, "Wrap");
/// assert!(window.app().wrap);
/// window.press("<space>");
/// assert!(!window.app().wrap);
/// ```
pub struct TreeWindow<A: App> {
    /// The app.
    app: A,
    /// What the app asked of the runtime so far.
    asks: Asks,
    /// The windows that closed.
    closed: Vec<WindowId>,
    /// Whether this window holds its thread's clock, since
    /// [`TreeWindow::advance`] first moved it.
    clock: bool,
    /// The frame drawn after the last input.
    frame: Frame,
    /// What the app's wakers sent, not yet run.
    inbox: Receiver<UserEvent<Update<A>>>,
    /// The app's other windows, which [`TreeWindow::switch_to`] brings in.
    others: Vec<Pane>,
    /// The tree's state.
    store: Store,
    /// The window's title, which names its accessibility tree's root.
    title: String,
    /// The waker the app was started with, which tests clone.
    waker: Waker<Update<A>>,
    /// The window the inputs come through.
    window: TestWindow,
}

/// One of the app's windows the test is not looking at: what
/// [`TreeWindow`] keeps of the window it is.
struct Pane {
    /// The frame drawn last.
    frame: Frame,
    /// The tree's state.
    store: Store,
    /// The window's title.
    title: String,
    /// The window.
    window: TestWindow,
}

// ========================================================================
// TreeWindow Implementations
// ========================================================================

impl<A: App> TreeWindow<A> {
    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// A window titled `title`, `width` by `height` physical pixels at
    /// scale factor 1, showing `app`, started with [`App::start`] and its
    /// first frame drawn. What the app's waker sends waits until
    /// [`TreeWindow::deliver`] runs it.
    pub fn new(mut app: A, title: &str, width: u32, height: u32) -> Self {
        let id = WindowId(window::WindowId::from(FIRST_WINDOW_ID));
        let window = TestWindow::opening(width, height, id);
        let frame = window.frame();
        let (waker, inbox) = Waker::channel();
        app.start(waker.clone());
        let mut tree = Self {
            app,
            asks: Asks::default(),
            closed: Vec::new(),
            clock: false,
            frame,
            inbox,
            others: Vec::new(),
            store: Store::default(),
            title: title.to_owned(),
            waker,
            window,
        };
        tree.redraw();
        tree
    }

    /// The app, as the inputs sent so far left it.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Changes the app's state with `change`, as a message from elsewhere
    /// would, and redraws.
    pub fn update(&mut self, change: impl FnOnce(&mut A)) {
        change(&mut self.app);
        self.redraw();
    }

    /// A waker for the app, as [`App::start`] was given, for a test to send
    /// updates through as another thread would.
    pub fn waker(&self) -> Waker<Update<A>> {
        self.waker.clone()
    }

    /// Runs each update the app's wakers sent so far on the app, in the
    /// order sent, redrawing after each; returns how many ran.
    pub fn deliver(&mut self) -> usize {
        let mut delivered = 0;
        while let Ok(event) = self.inbox.try_recv() {
            match event {
                UserEvent::Message(update) => {
                    update(&mut self.app);
                    self.asks.exit |= self.app.should_exit();
                    self.redraw();
                    delivered += 1;
                }
                // A channel waker posts only messages and wake-ups.
                UserEvent::Access(_) | UserEvent::Appearance(_) | UserEvent::Wake => {}
            }
        }
        delivered
    }

    /// Where the window's input method is anchored as of the last frame:
    /// the caret of the focused element that takes text (a text field, a
    /// text area, a [`custom`](crate::view::custom) element with
    /// [`ime_caret`](crate::view::Custom::ime_caret)), in physical pixels;
    /// `None` while none does, when the input method is disabled. Send
    /// the composition with [`TreeWindow::send`] and
    /// [`Input::Ime`](crate::Input::Ime).
    pub fn ime_area(&self) -> Option<Rect> {
        self.store.ime_caret()
    }

    /// The window the inputs come through: its clipboard, its pointer, and
    /// what widgets asked of it.
    pub fn window(&mut self) -> &mut TestWindow {
        &mut self.window
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Sends `input` as the runtime would: through the tree to the element
    /// it is for, running the callbacks it makes, the keys no element took
    /// going to [`App::key`]; then redraws.
    pub fn send(&mut self, input: &Input) {
        self.send_at(input, clock::now());
    }

    /// [`TreeWindow::send`] with `input` made at `now`, for a test of
    /// presses in a row (a double click) to say how far apart they came
    /// rather than race the clock.
    pub fn send_at(&mut self, input: &Input, now: Instant) {
        self.window.input.follow_at(input, now, 1.0);
        let id = self.window.id();
        let asks = self.store.input(&mut self.app, id, input, self.window.cx());
        self.follow_asks(asks);
        self.redraw();
    }

    /// Sends each key of `keys`, written in misty's key notation and
    /// separated by spaces (`"<tab> <tab> l"`, `"C-q"`). Panics, saying
    /// why, when `keys` is not valid notation.
    ///
    /// A key written with Shift that its key drops, since the character
    /// says what Shift did (`S-<space>` is a space, `S-g` is `G`, `S-/` is
    /// `/`), keeps the Shift where it sits: it is sent with
    /// [`EventCx::physical_key`](crate::EventCx::physical_key) at that key
    /// with Shift held, as a keyboard reports it, so Shift+Space reaches a
    /// keymap that reads key places. A place the test set with
    /// [`TestWindow::set_physical_key`](crate::testing::TestWindow::set_physical_key)
    /// wins. The keys themselves are as before.
    pub fn press(&mut self, keys: &str) {
        let parsed = parse_key_sequence(keys)
            .unwrap_or_else(|error| panic!("TreeWindow::press({keys:?}): {error}"));
        for (key, token) in parsed.into_iter().zip(keys.split_whitespace()) {
            let placed = self.window.place_written(token);
            self.send(&Input::Key(key));
            if placed {
                self.window.set_physical_key(None);
            }
        }
    }

    /// Lets go of each key of `keys`, written as for [`TreeWindow::press`]:
    /// a release no element takes reaches
    /// [`App::key_released`](crate::App::key_released). Panics, saying
    /// why, when `keys` is not valid notation.
    pub fn release(&mut self, keys: &str) {
        let keys = parse_key_sequence(keys)
            .unwrap_or_else(|error| panic!("TreeWindow::release({keys:?}): {error}"));
        for key in keys {
            self.send(&Input::KeyReleased(key));
        }
    }

    /// Types `text` into the focused control, a key per character, a line
    /// feed as Enter.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.send(&Input::Key(typed_key(c)));
        }
    }

    /// Moves the pointer to the middle of the element with `role` and the
    /// accessible name `name`, and clicks it with the left button. Panics
    /// as [`TreeWindow::find`] does when there is no such element.
    pub fn click(&mut self, role: Role, name: &str) {
        let point = self.center(role, name);
        self.click_at(point, PointerButton::Left);
    }

    /// Moves the pointer to the middle of the element with `role` and the
    /// accessible name `name`, and clicks it with the right button, as a
    /// user asking for its context menu. Panics as [`TreeWindow::find`]
    /// does when there is no such element.
    pub fn right_click(&mut self, role: Role, name: &str) {
        let point = self.center(role, name);
        self.click_at(point, PointerButton::Right);
    }

    /// Moves the pointer to `point` and clicks there with `button`.
    pub fn click_at(&mut self, point: Point, button: PointerButton) {
        self.send(&Input::PointerMoved(point));
        self.send(&Input::PointerButton(ButtonInput::press(button, point)));
        self.send(&Input::PointerButton(ButtonInput::release(button, point)));
    }

    /// Presses the left button at `from`, moves the pointer to `to` in a
    /// few steps, and releases it there: a drag of a tab, a divider, or a
    /// row.
    pub fn drag(&mut self, from: Point, to: Point) {
        self.send(&Input::PointerMoved(from));
        let press = ButtonInput::press(PointerButton::Left, from);
        self.send(&Input::PointerButton(press));
        for step in 1..=DRAG_STEPS {
            let share = step as f32 / DRAG_STEPS as f32;
            let point = Point::new(
                from.x + (to.x - from.x) * share,
                from.y + (to.y - from.y) * share,
            );
            self.send(&Input::PointerMoved(point));
        }
        let release = ButtonInput::release(PointerButton::Left, to);
        self.send(&Input::PointerButton(release));
    }

    /// Does what a screen reader asks with `action` (a click, a step, a
    /// focus), naming an element by the id [`TreeWindow::find`] returns;
    /// then redraws.
    pub fn act(&mut self, action: AccessAction) {
        let id = self.window.id();
        let asks = self.store.access_action(&mut self.app, id, action);
        self.follow_asks(asks);
        self.redraw();
    }

    /// Steps the tree's animations (carets, scroll glides, progress
    /// sweeps, a tooltip's delay), what time changes in the app's state
    /// (toasts expiring), and the app's own timers through
    /// [`App::about_to_wait`] at the time now, as the runtime does when it
    /// is about to sleep; returns when the next step is due, or `None` at
    /// rest; then redraws.
    pub fn about_to_wait(&mut self) -> Option<Instant> {
        self.about_to_wait_at(clock::now())
    }

    /// [`TreeWindow::about_to_wait`] with the app told the time is `now`,
    /// for a test of a timer to step past its deadline without waiting
    /// for it. The tree's animations step at the time misty reads
    /// ([`TreeWindow::now`]); [`TreeWindow::advance`] moves both.
    pub fn about_to_wait_at(&mut self, now: Instant) -> Option<Instant> {
        let id = self.window.id();
        let mut app_cx = AppCx::new(self.window.cx());
        let app_due = self.app.about_to_wait(now, &mut app_cx);
        let asks = app_cx.asks();
        self.follow_asks(asks);
        let next = self
            .store
            .about_to_wait(&mut self.app, id, &mut self.window.cx());
        self.redraw();
        next.into_iter().chain(app_due).min()
    }

    /// Moves time on by `by` without waiting, then steps everything that
    /// runs on time as [`TreeWindow::about_to_wait`] does: the tree's
    /// animations (caret blinks, glides, a tooltip's or which-key popup's
    /// delay, toasts expiring) and the app's timers, told the new time.
    /// Returns when the next step is due, or `None` at rest. The first
    /// call holds the clock misty reads on this thread at the time it is
    /// now; from then on time moves only when a test advances it, inputs
    /// sent included (presses in a row are counted by it), until the
    /// window is dropped. The quick pick does not follow the held clock
    /// yet: its query pause runs on the system's.
    pub fn advance(&mut self, by: Duration) -> Option<Instant> {
        self.clock = true;
        let now = clock::advance(by);
        self.about_to_wait_at(now)
    }

    /// The time misty reads now: the clock [`TreeWindow::advance`] holds,
    /// or the system's before it first moved it. A test sets an app's
    /// timers from it.
    pub fn now(&self) -> Instant {
        clock::now()
    }

    /// Resizes the window to `width` by `height` physical pixels, as the
    /// user dragging its edge would: [`App::resized`] is told, and the
    /// tree is laid out again at the new size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.window.resize(width, height);
        let id = self.window.id();
        self.app.resized(id, Size::new(width, height), 1.0);
        self.redraw();
    }

    // --------------------------------------------------------------------
    // Windows
    // --------------------------------------------------------------------

    /// The app's windows open now, the one the test looks at first: the
    /// first window and those the app opened with
    /// [`AppCx::open_window`].
    pub fn windows(&self) -> Vec<WindowId> {
        std::iter::once(self.window.id())
            .chain(self.others.iter().map(|pane| pane.window.id()))
            .filter(|id| !self.closed.contains(id))
            .collect()
    }

    /// The window the inputs go to and the readings come from.
    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    /// Sends the inputs to, and reads from, the app's window `id` from now
    /// on, one [`TreeWindow::windows`] lists. Panics, naming the windows
    /// open, when the app has no such window.
    pub fn switch_to(&mut self, id: WindowId) {
        if id == self.window.id() {
            return;
        }
        let Some(at) = self.others.iter().position(|pane| pane.window.id() == id) else {
            panic!(
                "TreeWindow::switch_to({id:?}): the app has no such window; its windows are {:?}",
                self.windows()
            );
        };
        let pane = &mut self.others[at];
        std::mem::swap(&mut self.frame, &mut pane.frame);
        std::mem::swap(&mut self.store, &mut pane.store);
        std::mem::swap(&mut self.title, &mut pane.title);
        std::mem::swap(&mut self.window, &mut pane.window);
        self.redraw();
    }

    // --------------------------------------------------------------------
    // Reading
    // --------------------------------------------------------------------

    /// The id of the element with `role` and the accessible name `name` (empty
    /// for an element without one) in the last frame, to name it in a screen reader's action. Panics with
    /// the elements the frame does describe when none matches.
    pub fn find(&self, role: Role, name: &str) -> FocusId {
        let (id, _) = self.node(role, name);
        FocusId::from_raw(id.0)
    }

    /// What the last frame describes to screen readers, as [`ui_text`]
    /// writes it.
    pub fn ui_text(&self) -> String {
        ui_text(&self.frame, &self.title)
    }

    /// The text the last frame holds, as [`drawn_text`] reads it.
    pub fn drawn_text(&self) -> Vec<String> {
        drawn_text(&self.frame)
    }

    /// The frame drawn after the last input.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Whether the window the test looks at closed, or the app quit.
    pub fn is_closed(&self) -> bool {
        self.asks.exit || self.closed.contains(&self.window.id())
    }

    /// The tree's state, for misty's own tests to look inside.
    #[cfg(test)]
    pub(crate) fn store(&mut self) -> &mut Store {
        &mut self.store
    }

    /// The tree drawn again and read back as pixels, for misty's own
    /// tests of how elements look, or `None` when no GPU adapter can draw
    /// it.
    #[cfg(test)]
    pub(crate) fn render(&mut self) -> Option<Snapshot> {
        let id = self.window.id();
        let (store, app) = (&mut self.store, &self.app);
        self.window.render(|frame| store.render(app, id, frame))
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// Takes in what the app asked of the runtime in one step: the
    /// windows it opened (set up, drawn) and closed (the app told), the
    /// window the test looks at closing, and quitting.
    fn follow_asks(&mut self, asks: Asks) {
        self.asks.exit |= asks.exit;
        if let Some(name) = asks.focus {
            self.store.request_focus(name);
        }
        if asks.hints {
            self.store.request_hints();
        }
        let current = self.window.id();
        if asks.close {
            self.close(current);
        }
        let (opened, closed) = self.window.take_windows();
        for (id, options) in opened {
            let size = options.size();
            let window = TestWindow::opening(size.width, size.height, id);
            let mut pane = Pane {
                frame: window.frame(),
                store: Store::default(),
                title: options.title().to_owned(),
                window,
            };
            pane.store.render(&self.app, id, &mut pane.frame);
            self.others.push(pane);
        }
        for id in closed {
            self.close(id);
        }
    }

    /// Notes that window `id` closed and tells the app, once.
    fn close(&mut self, id: WindowId) {
        if self.closed.contains(&id) {
            return;
        }
        self.closed.push(id);
        self.others.retain(|pane| pane.window.id() != id);
        self.app.window_closed(id);
    }

    /// Draws a new frame of the app, holding still when the app says so;
    /// a window that closed keeps its last frame.
    fn redraw(&mut self) {
        if self.is_closed() {
            return;
        }
        if let Some(reduced_motion) = self.app.reduced_motion() {
            self.window.set_reduced_motion(reduced_motion);
        }
        let mut frame = self.window.frame();
        let id = self.window.id();
        self.store.render(&self.app, id, &mut frame);
        self.frame = frame;
    }

    /// The middle of the element with `role` and the accessible name
    /// `name` in the last frame, in physical pixels: where a click or a
    /// drag aimed at it lands. Panics as [`TreeWindow::find`] does when
    /// there is no such element, and when it was described without
    /// bounds.
    pub fn center(&self, role: Role, name: &str) -> Point {
        let (_, node) = self.node(role, name);
        let bounds = node.bounds().unwrap_or_else(|| {
            panic!("TreeWindow: {role:?} {name:?} was described with no bounds")
        });
        Point::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        )
    }

    /// The node with `role` and the name `name` in the last frame's
    /// accessibility tree.
    fn node(&self, role: Role, name: &str) -> (NodeId, Node) {
        let tree = access_tree(&self.frame, &self.title);
        let found = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == role && node.label().unwrap_or_default() == name);
        match found {
            Some((id, node)) => (*id, node.clone()),
            None => {
                let described: Vec<String> = tree
                    .nodes
                    .iter()
                    .map(|(_, node)| format!("{:?} {:?}", node.role(), node.label().unwrap_or("")))
                    .collect();
                panic!(
                    "TreeWindow: no {role:?} named {name:?}; the window describes {}",
                    described.join(", ")
                );
            }
        }
    }
}

impl<A: App> Drop for TreeWindow<A> {
    /// Lets the thread's clock follow the system's again, when this window
    /// held it.
    fn drop(&mut self) {
        if self.clock {
            clock::release();
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::app::window::WindowOptions;
    use crate::base::key::{Key, KeyCode, Mods, PhysicalKey};
    use crate::base::theme::Theme;
    use crate::input::event::FileInput;
    use crate::view::controls::{button, text_field};
    use crate::view::element::{column, text};
    use crate::widgets::drag::FileDropped;
    use crate::{Element, parse_key};

    /// How long typing must settle before the probe searches.
    const SETTLE: Duration = Duration::from_millis(300);

    /// An app recording what its lifecycle hooks heard.
    struct Probe {
        /// Whether the user asked to close a window the probe kept open.
        asked: bool,
        /// Whether the probe refuses to close a window when asked.
        busy: bool,
        /// The windows that closed.
        closed: Vec<WindowId>,
        /// The files dropped where no element took them.
        dropped: Vec<PathBuf>,
        /// Each focus change of a window.
        focus: Vec<bool>,
        /// Whether a callback asked to move the keyboard to the Open
        /// button.
        jump_wanted: bool,
        /// The keys no element took, with where each sat.
        placed: Vec<(Key, Option<PhysicalKey>)>,
        /// The query typed.
        query: String,
        /// The keys let go that no element took.
        released: Vec<Key>,
        /// When the typed query is searched for, while typing settles.
        search_due: Option<Instant>,
        /// The queries searched for.
        searched: Vec<String>,
        /// The second window, while it is open.
        second: Option<WindowId>,
        /// Whether a callback asked for the second window.
        second_wanted: bool,
        /// The sizes the window was resized to.
        sizes: Vec<Size>,
        /// The colors it is drawn in.
        theme: Theme,
    }

    impl App for Probe {
        fn view(&self, window: WindowId) -> Element<'_, Self> {
            if Some(window) == self.second {
                return column([text("Second window").into()]);
            }
            column([
                text_field(&self.query)
                    .on_change(|probe: &mut Self, query| {
                        probe.query = query;
                        probe.search_due = Some(Instant::now() + SETTLE);
                    })
                    .into(),
                Element::from(
                    button("Open").on_click(|probe: &mut Self| probe.second_wanted = true),
                )
                .id("open"),
                Element::from(button("Jump").on_click(|probe: &mut Self| probe.jump_wanted = true))
                    .tooltip("Focus Open"),
            ])
        }

        fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
            if key == parse_key("<f9>").unwrap() {
                cx.close(cx.window());
                return true;
            }
            self.placed.push((key, cx.event_cx().physical_key()));
            false
        }

        fn key_released(&mut self, key: Key, _cx: &mut AppCx<'_>) {
            self.released.push(key);
        }

        fn about_to_wait(&mut self, now: Instant, cx: &mut AppCx<'_>) -> Option<Instant> {
            if std::mem::take(&mut self.jump_wanted) {
                cx.focus("open");
            }
            if std::mem::take(&mut self.second_wanted) {
                self.second = cx.open_window(WindowOptions::new("Second")).ok();
            }
            match self.search_due {
                Some(due) if due <= now => {
                    self.search_due = None;
                    self.searched.push(self.query.clone());
                    None
                }
                due => due,
            }
        }

        fn close_requested(&mut self, _window: WindowId, _cx: &mut AppCx<'_>) -> bool {
            self.asked = true;
            !self.busy
        }

        fn window_closed(&mut self, window: WindowId) {
            self.closed.push(window);
            if Some(window) == self.second {
                self.second = None;
            }
        }

        fn focus_changed(&mut self, _window: WindowId, focused: bool) {
            self.focus.push(focused);
        }

        fn resized(&mut self, _window: WindowId, size: Size, _scale_factor: f64) {
            self.sizes.push(size);
        }

        fn file_dropped(&mut self, _window: WindowId, file: FileDropped, _cx: &mut AppCx<'_>) {
            self.dropped.push(file.path);
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window showing a fresh probe.
    fn window() -> TreeWindow<Probe> {
        let probe = Probe {
            asked: false,
            busy: false,
            closed: Vec::new(),
            dropped: Vec::new(),
            focus: Vec::new(),
            jump_wanted: false,
            placed: Vec::new(),
            query: String::new(),
            released: Vec::new(),
            search_due: None,
            searched: Vec::new(),
            second: None,
            second_wanted: false,
            sizes: Vec::new(),
            theme: Theme::dark(),
        };
        TreeWindow::new(probe, "Probe", 400, 200)
    }

    #[test]
    fn test_a_focus_asked_for_from_a_callback_moves_once_the_timers_run() {
        let mut window = window();
        window.click(Role::Button, "Jump");
        window.about_to_wait();
        let text = window.ui_text();
        let focused = text.lines().find(|line| line.ends_with(" focused"));
        assert!(
            focused.is_some_and(|line| line.contains("\"Open\"")),
            "{text}"
        );
    }

    #[test]
    fn test_the_app_reads_where_a_key_sat_beside_what_it_typed() {
        let mut window = window();
        let place = PhysicalKey::new(KeyCode::Char('/'), Mods::CTRL | Mods::SHIFT);
        window.window().set_physical_key(Some(place));
        window.press("C-?");
        assert_eq!(
            window.app().placed,
            [(parse_key("C-?").unwrap(), Some(place))]
        );
    }

    /// An app of text and no control, keeping every key it gets and
    /// where the key sat.
    struct Keys {
        /// The keys, each with its place.
        placed: Vec<(Key, Option<PhysicalKey>)>,
        /// The colors it is drawn in.
        theme: Theme,
    }

    impl App for Keys {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            column([text("keys").into()])
        }

        fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
            self.placed.push((key, cx.event_cx().physical_key()));
            true
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_a_shift_the_key_notation_drops_is_kept_in_the_key_s_place() {
        let keys = Keys {
            placed: Vec::new(),
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(keys, "t", 200, 100);
        window.press("S-<space> <space> S-g S-/ C-S-f");
        let at = |code, mods| Some(PhysicalKey::new(code, mods));
        let key = |spec| parse_key(spec).unwrap();
        assert_eq!(
            window.app().placed,
            [
                (key("<space>"), at(KeyCode::Char(' '), Mods::SHIFT)),
                (key("<space>"), None),
                (key("G"), at(KeyCode::Char('g'), Mods::SHIFT)),
                (key("/"), at(KeyCode::Char('/'), Mods::SHIFT)),
                (key("C-S-f"), None),
            ]
        );
        let place = PhysicalKey::new(KeyCode::Char('x'), Mods::NONE);
        window.window().set_physical_key(Some(place));
        window.press("S-<space>");
        let last = window.app().placed.last().copied();
        assert_eq!(
            last,
            Some((key("<space>"), Some(place))),
            "the test's own place"
        );
    }

    #[test]
    fn test_a_key_let_go_reaches_only_the_release_hook() {
        let mut window = window();
        window.press("<f5>");
        window.release("<f5> a");
        let pressed: Vec<Key> = window.app().placed.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            pressed,
            [parse_key("<f5>").unwrap()],
            "App::key sees presses only"
        );
        let released = [parse_key("<f5>").unwrap(), parse_key("a").unwrap()];
        assert_eq!(window.app().released, released);
    }

    #[test]
    fn test_advancing_the_held_clock_shows_a_tooltip_without_waiting() {
        let mut window = window();
        let jump = window.center(Role::Button, "Jump");
        window.send(&Input::PointerMoved(jump));
        window.advance(Duration::from_millis(100));
        assert!(!window.ui_text().contains("Tooltip"), "not yet due");
        let started = Instant::now();
        window.advance(Duration::from_secs(1));
        assert!(window.ui_text().contains("Tooltip \"Focus Open\""));
        assert!(started.elapsed() < Duration::from_secs(1), "no waiting");
    }

    #[test]
    fn test_presses_in_a_row_are_counted_by_the_held_clock() {
        let mut window = window();
        let open = window.center(Role::Button, "Open");
        window.advance(Duration::ZERO);
        window.click_at(open, PointerButton::Left);
        window.advance(Duration::from_secs(5));
        window.click_at(open, PointerButton::Left);
        let count = window.window().cx().click_count();
        assert_eq!(count, 1, "five held seconds apart is no double click");
        window.click_at(open, PointerButton::Left);
        assert_eq!(window.window().cx().click_count(), 2, "no time passed");
    }

    #[test]
    fn test_a_debounce_timer_fires_once_typing_has_settled() {
        let mut window = window();
        window.click(Role::TextInput, "");
        window.type_text("ab");
        let due = window.app().search_due.expect("typing set the timer");
        let early = window.about_to_wait_at(due - SETTLE / 2);
        assert!(early.is_some_and(|next| next <= due));
        assert!(window.app().searched.is_empty());
        window.about_to_wait_at(due);
        assert_eq!(window.app().searched, ["ab"]);
        assert_eq!(window.app().search_due, None);
    }

    #[test]
    fn test_a_callback_opens_a_second_window_described_by_its_own_view() {
        let mut window = window();
        let first = window.window_id();
        window.click(Role::Button, "Open");
        window.about_to_wait();
        let second = window.app().second.expect("the window opened");
        assert_eq!(window.windows(), [first, second]);
        window.switch_to(second);
        assert!(
            window
                .drawn_text()
                .iter()
                .any(|line| line.contains("Second window"))
        );
        window.press("<f9>");
        assert!(window.is_closed());
        assert_eq!(window.app().closed, [second]);
        assert_eq!(window.app().second, None);
        window.switch_to(first);
        assert!(!window.is_closed());
        assert_eq!(window.windows(), [first]);
    }

    #[test]
    fn test_a_close_request_the_app_refuses_keeps_the_window_until_it_closes_it() {
        let mut window = window();
        window.update(|probe| probe.busy = true);
        window.send(&Input::CloseRequested);
        assert!(window.app().asked);
        assert!(!window.is_closed());
        window.press("<f9>");
        assert!(window.is_closed());
        let id = window.window_id();
        assert_eq!(window.app().closed, [id]);
    }

    #[test]
    fn test_focus_changes_resizes_and_stray_file_drops_reach_the_app() {
        let mut window = window();
        window.send(&Input::FocusLost);
        window.send(&Input::FocusGained);
        window.resize(640, 480);
        let file = FileInput {
            path: PathBuf::from("/tmp/notes.md"),
            position: Some(Point::new(390.0, 190.0)),
        };
        window.send(&Input::FileDropped(file));
        let probe = window.app();
        assert_eq!(probe.focus, [false, true]);
        assert_eq!(probe.sizes, [Size::new(640, 480)]);
        assert_eq!(probe.dropped, [PathBuf::from("/tmp/notes.md")]);
        assert_eq!(window.frame().bounds().width, 640.0);
    }
}

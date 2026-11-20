//! [`TestApp`]: a headless window for an [`Application`] run by
//! [`run`](crate::run), built on [`TestWindow`]. It owns the app, calls it
//! as the event loop would (its first window's `init`, then inputs, screen
//! reader actions, messages, and ticks), carries out what it asks of the
//! runtime, and draws a frame after every call, with no event loop and no
//! display.

use std::sync::mpsc::Receiver;
use std::time::Instant;

use super::super::app::access::AccessAction;
use super::super::app::application::{AppContext, Application, OpenView};
use super::super::app::context::Requests;
use super::super::app::frame::Frame;
use super::super::base::appearance::Appearance;
use super::super::base::key::parse_key_sequence;
use super::super::input::event::Input;
use super::super::platform::waker::{UserEvent, Waker};
use super::super::view::app::Asks;
use super::{TestWindow, drawn_text, typed_key, ui_text};

// ========================================================================
// Data Structures
// ========================================================================

/// A window with no window system behind it for an [`Application`]: the
/// app, the messages its [`Waker`]s sent, and the frame drawn after the
/// last call. Messages wait until [`TestApp::deliver`] hands them on, so a
/// test decides when background work lands.
///
/// ```
/// use misty::testing::TestApp;
/// use misty::{AppContext, Application, Frame, Input, WindowId};
///
/// #[derive(Default)]
/// struct Demo {
///     sums: Vec<u32>,
/// }
///
/// impl Application for Demo {
///     type Message = u32;
///
///     fn input(&mut self, cx: &mut AppContext<'_, u32>, window: WindowId, input: &Input) {
///         if let Input::CloseRequested = input {
///             cx.close_window(window);
///         }
///     }
///
///     fn message(&mut self, _cx: &mut AppContext<'_, u32>, sum: u32) {
///         self.sums.push(sum);
///     }
///
///     fn render(&mut self, _window: WindowId, _frame: &mut Frame) {}
/// }
///
/// let mut test = TestApp::new(Demo::default(), "Demo", 400, 200);
/// let waker = test.waker();
/// std::thread::spawn(move || waker.send(1 + 2)).join().unwrap().unwrap();
/// assert_eq!(test.deliver(), 1);
/// assert_eq!(test.app().sums, [3]);
/// test.send(&Input::CloseRequested);
/// assert!(test.is_closed());
/// ```
pub struct TestApp<A: Application> {
    /// The app.
    app: A,
    /// The user's appearance settings, as the test set them.
    appearance: Appearance,
    /// What the app asked of the runtime so far.
    asks: Asks,
    /// The frame drawn after the last call.
    frame: Frame,
    /// What the app's wakers sent, not yet handed on.
    inbox: Receiver<UserEvent<A::Message>>,
    /// The app's reduced-motion override: `None` to follow `appearance`.
    reduced_motion: Option<bool>,
    /// The window's title, which names its accessibility tree's root.
    title: String,
    /// The waker the app's contexts hand out clones of.
    waker: Waker<A::Message>,
    /// The window the inputs come through.
    window: TestWindow,
}

// ========================================================================
// TestApp Implementations
// ========================================================================

impl<A: Application> TestApp<A> {
    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// A window titled `title`, `width` by `height` physical pixels at
    /// scale factor 1, opened for `app`: [`Application::init`] runs for it
    /// and its first frame is drawn. The app's contexts have no GPU, and
    /// [`AppContext::open_window`] refuses with
    /// [`Error::Headless`](crate::Error::Headless).
    pub fn new(app: A, title: &str, width: u32, height: u32) -> Self {
        let window = TestWindow::new(width, height);
        let frame = window.frame();
        let (waker, inbox) = Waker::channel();
        let mut test = Self {
            app,
            appearance: Appearance::default(),
            asks: Asks::default(),
            frame,
            inbox,
            reduced_motion: None,
            title: title.to_owned(),
            waker,
            window,
        };
        let id = test.window.id();
        test.call_app(|app, cx| app.init(cx, id));
        test
    }

    /// The app, as the calls made so far left it.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Changes the app's state with `change`, as a test's setup would, and
    /// redraws.
    pub fn update(&mut self, change: impl FnOnce(&mut A)) {
        change(&mut self.app);
        self.redraw();
    }

    /// A waker for the app, as [`AppContext::waker`] hands out, for a test
    /// to send messages through as another thread would.
    pub fn waker(&self) -> Waker<A::Message> {
        self.waker.clone()
    }

    /// The window the inputs come through: its clipboard, its pointer, and
    /// what the app asked of it (redraws, a pointer shape).
    pub fn window(&mut self) -> &mut TestWindow {
        &mut self.window
    }

    // --------------------------------------------------------------------
    // Calls
    // --------------------------------------------------------------------

    /// Sends `input` as the runtime would: follows it (the pointer, the
    /// modifiers), hands it to [`Application::input`], and redraws.
    pub fn send(&mut self, input: &Input) {
        self.window.input.follow(input);
        let id = self.window.id();
        self.call_app(|app, cx| app.input(cx, id, input));
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
            .unwrap_or_else(|error| panic!("TestApp::press({keys:?}): {error}"));
        for (key, token) in parsed.into_iter().zip(keys.split_whitespace()) {
            let placed = self.window.place_written(token);
            self.send(&Input::Key(key));
            if placed {
                self.window.set_physical_key(None);
            }
        }
    }

    /// Types `text`, a key per character, a line feed as Enter.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.send(&Input::Key(typed_key(c)));
        }
    }

    /// Does what a screen reader asks with `action`, through
    /// [`Application::access_action`]; then redraws.
    pub fn act(&mut self, action: AccessAction) {
        let id = self.window.id();
        self.call_app(|app, cx| app.access_action(cx, id, action));
    }

    /// Hands each message the app's wakers sent so far to
    /// [`Application::message`], in the order sent, redrawing after each;
    /// returns how many were handed on.
    pub fn deliver(&mut self) -> usize {
        let mut delivered = 0;
        while let Ok(event) = self.inbox.try_recv() {
            match event {
                UserEvent::Message(message) => {
                    self.call_app(|app, cx| app.message(cx, message));
                    delivered += 1;
                }
                // A channel waker posts only messages and wake-ups; screen
                // readers and the system's settings reach a real window.
                UserEvent::Access(_) | UserEvent::Appearance(_) | UserEvent::Wake => {}
            }
        }
        delivered
    }

    /// Changes the user's appearance settings to `appearance`, as the
    /// system would: the app gets [`Input::AppearanceChanged`] unless
    /// nothing changed, and its reduced-motion setting is followed unless
    /// the app overrides it.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        if appearance == self.appearance {
            return;
        }
        self.appearance = appearance;
        self.follow_reduced_motion();
        let input = Input::AppearanceChanged(appearance);
        let id = self.window.id();
        self.call_app(|app, cx| app.input(cx, id, &input));
    }

    /// Calls [`Application::about_to_wait`], as the event loop does before
    /// it sleeps, returning when the app next needs it, or `None` at rest;
    /// then redraws.
    pub fn about_to_wait(&mut self) -> Option<Instant> {
        self.call_app(|app, cx| app.about_to_wait(cx))
    }

    // --------------------------------------------------------------------
    // Reading
    // --------------------------------------------------------------------

    /// What the last frame describes to screen readers, as [`ui_text`]
    /// writes it.
    pub fn ui_text(&self) -> String {
        ui_text(&self.frame, &self.title)
    }

    /// The text the last frame holds, as [`drawn_text`] reads it.
    pub fn drawn_text(&self) -> Vec<String> {
        drawn_text(&self.frame)
    }

    /// The frame drawn after the last call.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Whether the app asked to close the window or to quit.
    pub fn is_closed(&self) -> bool {
        self.asks.close || self.asks.exit
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// Calls the app with a context for the window, carries out what it
    /// asked for, and redraws, returning what `call` returns.
    fn call_app<R>(
        &mut self,
        call: impl FnOnce(&mut A, &mut AppContext<'_, A::Message>) -> R,
    ) -> R {
        let view = OpenView {
            geometry: self.window.geometry(),
            id: self.window.id(),
            state: self.window.state(),
            #[cfg(feature = "dialogs")]
            window: None,
        };
        let mut cx = AppContext::new(
            &self.waker,
            None,
            vec![view],
            &mut self.window.clipboard,
            self.appearance,
            self.reduced_motion,
            None,
        );
        let value = call(&mut self.app, &mut cx);
        let requests = cx.into_requests();
        self.follow_requests(requests);
        self.redraw();
        value
    }

    /// Carries out what the app asked for in one call: a close or a quit
    /// is noted, a reduced-motion override followed, and what was asked of
    /// the window kept for the test to read through [`TestApp::window`].
    fn follow_requests(&mut self, requests: Requests) {
        let Requests {
            actions,
            closed,
            cursors,
            exit,
            // Always empty: a test app's requests never mint windows.
            minted: _,
            mints: _,
            // Always empty: `open_window` refuses with no event loop.
            opened: _,
            redraws,
            reduced_motion,
            scheduled,
        } = requests;
        self.asks.close |= closed.contains(&self.window.id());
        self.asks.exit |= exit;
        if let Some(reduced_motion) = reduced_motion {
            self.reduced_motion = reduced_motion;
            self.follow_reduced_motion();
        }
        let asked = &mut self.window.requests;
        asked.actions.extend(actions);
        asked.cursors.extend(cursors);
        asked.redraws.extend(redraws);
        asked.scheduled.extend(scheduled);
    }

    /// Has the window cut animations short as the app's override says, or
    /// else as the user's settings do.
    fn follow_reduced_motion(&mut self) {
        let reduced_motion = self
            .reduced_motion
            .unwrap_or(self.appearance.reduced_motion);
        self.window.set_reduced_motion(reduced_motion);
    }

    /// Draws a new frame of the app.
    fn redraw(&mut self) {
        let mut frame = self.window.frame();
        let id = self.window.id();
        self.app.render(id, &mut frame);
        self.frame = frame;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::thread;

    use super::super::super::app::window::WindowOptions;
    use super::super::super::platform::error::Error;
    use super::super::super::platform::window_id::WindowId;
    use super::*;

    /// An app that records what reaches it.
    #[derive(Default)]
    struct Recorder {
        /// The messages handed on, in order.
        messages: Vec<u32>,
        /// What opening a window answered, once asked with `o`.
        opened: Option<Result<WindowId, Error>>,
    }

    impl Application for Recorder {
        type Message = u32;

        fn input(&mut self, cx: &mut AppContext<'_, u32>, window: WindowId, input: &Input) {
            match input {
                Input::CloseRequested => cx.close_window(window),
                Input::Key(key) if *key == typed_key('m') => cx.set_reduced_motion(Some(false)),
                Input::Key(key) if *key == typed_key('o') => {
                    self.opened = Some(cx.open_window(WindowOptions::new("Second")));
                }
                Input::Key(key) if *key == typed_key('w') => {
                    // Sent from inside a callback, as a spawned job's first
                    // report would be.
                    let _ = cx.waker().send(7);
                }
                _ => {}
            }
        }

        fn message(&mut self, _cx: &mut AppContext<'_, u32>, message: u32) {
            self.messages.push(message);
        }

        fn render(&mut self, _window: WindowId, _frame: &mut Frame) {}
    }

    fn recorder() -> TestApp<Recorder> {
        TestApp::new(Recorder::default(), "Recorder", 200, 100)
    }

    #[test]
    fn test_messages_wait_for_deliver_and_arrive_in_the_order_sent() {
        let mut test = recorder();
        let waker = test.waker();
        thread::spawn(move || {
            waker.send(1).unwrap();
            waker.send(2).unwrap();
        })
        .join()
        .unwrap();
        test.type_text("w");
        assert!(test.app().messages.is_empty());

        assert_eq!(test.deliver(), 3);
        assert_eq!(test.app().messages, [1, 2, 7]);
        assert_eq!(test.deliver(), 0);
    }

    #[test]
    fn test_opening_another_window_is_refused_as_headless() {
        let mut test = recorder();
        test.type_text("o");
        assert!(matches!(test.app().opened, Some(Err(Error::Headless))));
    }

    #[test]
    fn test_closing_its_window_marks_the_app_closed() {
        let mut test = recorder();
        assert!(!test.is_closed());
        test.send(&Input::CloseRequested);
        assert!(test.is_closed());
    }

    #[test]
    fn test_the_apps_reduced_motion_override_beats_the_users_setting() {
        let mut test = recorder();
        let reduced = Appearance {
            reduced_motion: true,
            ..Appearance::default()
        };
        test.set_appearance(reduced);
        assert!(test.frame().reduced_motion());

        test.type_text("m");
        assert!(!test.frame().reduced_motion());
        test.set_appearance(Appearance::default());
        test.set_appearance(reduced);
        assert!(!test.frame().reduced_motion());
    }
}

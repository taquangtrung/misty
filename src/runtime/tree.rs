//! [`run_app`]: runs an [`App`] described by a view, through the same
//! event loop as [`run`]: misty implements [`Application`] for a runner
//! holding the app and one tree [`Store`] per window, so windows, frames,
//! and panics are handled as for any app.

use std::collections::HashMap;
use std::time::Instant;

use super::super::app::access::AccessAction;
use super::super::app::application::{AppContext, Application};
use super::super::app::frame::Frame;
use super::super::app::hang::HangWatch;
use super::super::app::window::{Size, WindowOptions};
use super::super::base::key::KeyScheme;
use super::super::input::event::Input;
use super::super::platform::error::Error;
use super::super::platform::window_id::WindowId;
use super::super::view::app::{App, AppCx, Update};
use super::super::view::store::Store;
use super::event_loop::run;

// ========================================================================
// Data Structures
// ========================================================================

/// The [`Application`] a tree app runs as: the app and each window's tree
/// state.
struct TreeRunner<A> {
    /// The window the last input came to, which [`App::about_to_wait`]
    /// reaches.
    active: Option<WindowId>,
    /// The app.
    app: A,
    /// When the app's [`App::about_to_wait`] last asked to be called again.
    app_due: Option<Instant>,
    /// Whether the app has been started and told the user's appearance
    /// settings yet.
    followed: bool,
    /// Each open window's tree state.
    stores: HashMap<WindowId, Store>,
}

// ========================================================================
// TreeRunner Implementations
// ========================================================================

impl<A: App> Application for TreeRunner<A> {
    type Message = Update<A>;

    fn init(&mut self, cx: &mut AppContext<'_, Update<A>>, window: WindowId) {
        if !self.followed {
            self.followed = true;
            self.app.start(cx.waker());
            self.app.follow_appearance(cx.appearance());
        }
        cx.set_reduced_motion(self.app.reduced_motion());
        self.stores.insert(window, Store::default());
    }

    /// Runs `update` on the app, then redraws every window, and quits once
    /// the app says so.
    fn message(&mut self, cx: &mut AppContext<'_, Update<A>>, update: Update<A>) {
        update(&mut self.app);
        for window in self.stores.keys() {
            cx.event_cx(*window).request_redraw();
        }
        cx.set_reduced_motion(self.app.reduced_motion());
        if self.app.should_exit() {
            cx.exit();
        }
    }

    fn hang_watch(&self) -> HangWatch {
        self.app.hang_watch()
    }

    fn key_scheme(&self) -> KeyScheme {
        self.app.key_scheme()
    }

    fn on_panic(&mut self, cx: &mut AppContext<'_, Update<A>>, why: &str) {
        self.app.on_panic(why);
        for window in self.stores.keys() {
            cx.event_cx(*window).request_redraw();
        }
    }

    fn input(&mut self, cx: &mut AppContext<'_, Update<A>>, window: WindowId, input: &Input) {
        self.active = Some(window);
        let store = self.stores.entry(window).or_default();
        let asks = store.input(&mut self.app, window, input, cx.event_cx(window));
        if let Some(name) = asks.focus {
            store.request_focus(name);
        }
        if asks.hints {
            store.request_hints();
        }
        cx.set_reduced_motion(self.app.reduced_motion());
        if asks.close {
            self.stores.remove(&window);
            cx.close_window(window);
            self.app.window_closed(window);
        }
        if asks.exit {
            cx.exit();
        }
    }

    /// Forgets the windows that closed since, runs the app's own
    /// [`App::about_to_wait`] on the window that had the last input
    /// (redrawing every window once a time it asked for has come, as
    /// [`App::redraw_when_due`] allows), then steps every window's tree.
    fn about_to_wait(&mut self, cx: &mut AppContext<'_, Update<A>>) -> Option<Instant> {
        let open: Vec<WindowId> = cx.window_ids().collect();
        let closed: Vec<WindowId> = self
            .stores
            .keys()
            .filter(|window| !open.contains(window))
            .copied()
            .collect();
        for window in closed {
            self.stores.remove(&window);
            self.app.window_closed(window);
        }
        let active = self
            .active
            .filter(|window| open.contains(window))
            .or_else(|| open.first().copied());
        let now = Instant::now();
        // A timer that came due changed the app's state for every window.
        let fired = self.app.redraw_when_due() && self.app_due.is_some_and(|due| due <= now);
        let app_due = active.and_then(|window| {
            let mut app_cx = AppCx::new(cx.event_cx(window));
            let due = self.app.about_to_wait(now, &mut app_cx);
            let asks = app_cx.asks();
            if let Some(name) = asks.focus
                && let Some(store) = self.stores.get_mut(&window)
            {
                store.request_focus(name);
                cx.event_cx(window).request_redraw();
            }
            if asks.hints
                && let Some(store) = self.stores.get_mut(&window)
            {
                store.request_hints();
                cx.event_cx(window).request_redraw();
            }
            if asks.close {
                cx.close_window(window);
            }
            if asks.exit {
                cx.exit();
            }
            due
        });
        self.app_due = app_due;
        if fired {
            for window in self.stores.keys() {
                cx.event_cx(*window).request_redraw();
            }
        }
        let app = &mut self.app;
        self.stores
            .iter_mut()
            .filter_map(|(window, store)| {
                store.about_to_wait(app, *window, &mut cx.event_cx(*window))
            })
            .chain(app_due)
            .min()
    }

    fn resized(
        &mut self,
        _cx: &mut AppContext<'_, Update<A>>,
        window: WindowId,
        size: Size,
        scale_factor: f64,
    ) {
        self.app.resized(window, size, scale_factor);
    }

    fn render(&mut self, window: WindowId, frame: &mut Frame) {
        let store = self.stores.entry(window).or_default();
        store.render(&self.app, window, frame);
    }

    fn access_action(
        &mut self,
        cx: &mut AppContext<'_, Update<A>>,
        window: WindowId,
        action: AccessAction,
    ) {
        cx.event_cx(window).request_redraw();
        let store = self.stores.entry(window).or_default();
        let asks = store.access_action(&mut self.app, window, action);
        cx.set_reduced_motion(self.app.reduced_motion());
        if asks.exit {
            cx.exit();
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Runs `app`, an [`App`] described by a view, in a window opened with
/// `options`, until the app quits or its last window closes: a close
/// request closes its window, and [`AppCx::exit`](crate::AppCx::exit)
/// quits. Must be called on the main thread; returns only when the app
/// has quit or the first window could not be brought up.
pub fn run_app(app: impl App, options: WindowOptions) -> Result<(), Error> {
    let runner = TreeRunner {
        active: None,
        app,
        app_due: None,
        followed: false,
        stores: HashMap::new(),
    };
    run(runner, options)
}

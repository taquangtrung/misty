//! What an app implements, [`Application`], and what the runtime hands it:
//! an [`AppContext`] to ask for things and a [`Frame`] to draw into.
//!
//! [`Frame`]: crate::Frame

use std::sync::Arc;
use std::time::Instant;

use winit::event_loop::ActiveEventLoop;
#[cfg(feature = "dialogs")]
use winit::window::Window;

use super::super::base::appearance::Appearance;
use super::super::base::key::KeyScheme;
use super::super::input::event::Input;
use super::super::platform::clipboard::SystemClipboard;
use super::super::platform::error::Error;
use super::super::platform::waker::Waker;
use super::super::platform::window_id::WindowId;
use super::super::render::gpu::Gpu;
use super::access::AccessAction;
use super::context::{EventCx, NewWindow, Requests, WindowState};
use super::frame::Frame;
use super::hang::HangWatch;
use super::window::{Size, WindowGeometry, WindowOptions, monitors};

// ========================================================================
// Data Structures
// ========================================================================

/// What an app reaches the runtime through from a callback, for what is
/// the whole app's: opening and closing windows, quitting, the user's
/// appearance settings, the GPU, the clipboard, and the [`Waker`] that
/// lets other threads send the app messages of type `M`. What is asked of
/// one window (a redraw, a title, a move) is asked through that window's
/// [`EventCx`], from [`AppContext::event_cx`]. Requests are carried out as
/// soon as the callback returns.
#[derive(Debug)]
pub struct AppContext<'a, M: 'static> {
    /// The user's appearance settings, as the runtime last read them.
    appearance: Appearance,
    /// The event loop, which opens windows; `None` for an app driven
    /// headlessly by the `testing` feature's `TestApp`.
    event_loop: Option<&'a ActiveEventLoop>,
    /// The GPU, once the first window has made it.
    gpu: Option<Gpu>,
    /// What the app asked for.
    requests: Requests,
    /// The event loop's inbox.
    waker: &'a Waker<M>,
    /// The clipboard the runtime lends every callback.
    clipboard: &'a mut SystemClipboard,
    /// Whether animations are cut short whatever the user's settings say
    /// (`Some`), or follow them (`None`).
    reduced_motion: Option<bool>,
    /// The open windows.
    windows: Vec<OpenView>,
}

/// An open window as a callback sees it.
#[derive(Clone, Debug)]
pub(crate) struct OpenView {
    /// Where it is and how big, as the runtime last read it.
    pub(crate) geometry: WindowGeometry,
    /// Its id.
    pub(crate) id: WindowId,
    /// Its pointer, modifiers, and maximized state.
    pub(crate) state: WindowState,
    /// The window itself, which a file dialog sits over; `None` for a
    /// headless test's window.
    #[cfg(feature = "dialogs")]
    pub(crate) window: Option<Arc<Window>>,
}

// ========================================================================
// Traits
// ========================================================================

/// An app run by [`run`](crate::run). The runtime owns the windows and the
/// GPU and calls these methods on the app's behalf, all on the main thread.
///
/// A panic in any of them is caught: the frame or event it struck is
/// dropped, [`Application::on_panic`] is told, and the app carries on, so
/// one bug does not lose the user's work. Only a sixth panic within ten
/// seconds, which carrying on would spin on, is let end the app.
pub trait Application: 'static {
    /// What other threads send the app through a [`Waker`], such as the
    /// result of work done in the background. `()` when the app needs none.
    type Message: Send + 'static;

    /// Whether the app quits when its last window closes (the default),
    /// rather than running on with no window, as a tray app or one that
    /// reopens a window on a message does.
    fn exit_when_last_window_closes(&self) -> bool {
        true
    }

    /// Called once for each window, when it is open but before its first
    /// frame: the window [`run`](crate::run) opens, and every one opened
    /// with [`AppContext::open_window`].
    fn init(&mut self, _cx: &mut AppContext<'_, Self::Message>, _window: WindowId) {}

    /// Called with every [`Input`] the user gives `window`: the pointer,
    /// keys, input method text, focus, and files dragged in. Resizes,
    /// scale changes, and redraws are the runtime's and never reach here;
    /// the window is redrawn after each of them. Closing is up to the app:
    /// on [`Input::CloseRequested`], call [`AppContext::close_window`] (the
    /// app quits with its last window unless
    /// [`Application::exit_when_last_window_closes`] says otherwise) or
    /// [`AppContext::exit`] to quit.
    fn input(&mut self, cx: &mut AppContext<'_, Self::Message>, window: WindowId, input: &Input);

    /// Called with each message sent through a [`Waker`], in the order sent.
    fn message(&mut self, _cx: &mut AppContext<'_, Self::Message>, _message: Self::Message) {}

    /// Called after `window` was resized, or moved to a display of another
    /// scale, with its inner size in logical pixels and its scale factor
    /// now. The runtime has already resized what it draws into and asked
    /// for a frame, so an app that only draws the window needs nothing
    /// here; one that saves its window size, or lays out something of its
    /// own ahead of the frame, does. The default does nothing.
    fn resized(
        &mut self,
        _cx: &mut AppContext<'_, Self::Message>,
        _window: WindowId,
        _size: Size,
        _scale_factor: f64,
    ) {
    }

    /// Called each time the event loop is about to sleep, after the events
    /// that woke it. Returns when the app next needs to be called here even
    /// if no event comes (a blink, an animation step, a timeout), or `None`
    /// to sleep until an event, a message, or a redraw asked for with
    /// [`EventCx::request_redraw_at`].
    ///
    /// An app with animating widgets (a caret's blink, a scroll's glide, a
    /// spinner) forwards to each one's [`Widget::about_to_wait`] through
    /// [`AppContext::event_cx`] and returns the earliest instant they
    /// answer.
    ///
    /// [`Widget::about_to_wait`]: crate::Widget::about_to_wait
    fn about_to_wait(&mut self, _cx: &mut AppContext<'_, Self::Message>) -> Option<Instant> {
        None
    }

    /// Draws the next frame of `window` into `frame`, describing it to
    /// screen readers on the way (see [`AccessAction`](crate::AccessAction)).
    fn render(&mut self, window: WindowId, frame: &mut Frame);

    /// Called when an assistive technology asks something of `window`:
    /// to focus a widget, to click one, or to step a slider's or a
    /// stepper's value. The default does nothing; an app with a
    /// [`FocusTree`](crate::FocusTree) hands focus requests to
    /// [`FocusTree::focus`](crate::FocusTree::focus), and clicks and steps
    /// to the widget named, through its `access_action`.
    fn access_action(
        &mut self,
        _cx: &mut AppContext<'_, Self::Message>,
        _window: WindowId,
        _action: AccessAction,
    ) {
    }

    /// Called after a callback panicked with the message `why` and the app
    /// carried on. Its state may be half updated by the callback that did
    /// not finish. The default logs `why` as an error (the `log` crate);
    /// an app may show it instead. A panic here is not caught.
    fn on_panic(&mut self, _cx: &mut AppContext<'_, Self::Message>, why: &str) {
        log::error!("the app panicked and carried on: {why}");
    }

    /// What to do about a callback that does not return, asked once as the
    /// app starts: the main thread draws every window, so one callback
    /// stuck in a loop freezes them all, which a panic guard cannot catch.
    /// The default logs a [`HangReport`](crate::HangReport) after five
    /// seconds; an app may write a report and exit or start again instead
    /// ([`HangWatch::on_hang`]), or watch nothing ([`HangWatch::off`]).
    fn hang_watch(&self) -> HangWatch {
        HangWatch::default()
    }

    /// Which keyboard conventions misty's widgets follow beyond the
    /// platform's, asked once as the app starts: none by default, so
    /// lists and text fields behave as the platform's do;
    /// [`KeyScheme::VIM_AND_EMACS`] for a keyboard-first app.
    fn key_scheme(&self) -> KeyScheme {
        KeyScheme::default()
    }
}

// ========================================================================
// AppContext Implementations
// ========================================================================

impl<'a, M: 'static> AppContext<'a, M> {
    /// A context with no requests yet, handing out clones of `waker` and
    /// lending `clipboard`, for a callback of `event_loop` (`None` when
    /// driven headlessly) while `windows` are open, the user's settings
    /// are `appearance`, and the app's reduced-motion override is
    /// `reduced_motion`.
    pub(crate) fn new(
        waker: &'a Waker<M>,
        event_loop: Option<&'a ActiveEventLoop>,
        windows: Vec<OpenView>,
        clipboard: &'a mut SystemClipboard,
        appearance: Appearance,
        reduced_motion: Option<bool>,
        gpu: Option<Gpu>,
    ) -> Self {
        Self {
            appearance,
            clipboard,
            event_loop,
            gpu,
            reduced_motion,
            requests: Requests::default(),
            waker,
            windows,
        }
    }

    /// What the app asked for, for the runtime to carry out.
    pub(crate) fn into_requests(self) -> Requests {
        self.requests
    }

    /// The context `window`'s widgets handle inputs and ticks through, and
    /// the app asks things of the window through (a redraw, a title, a
    /// move): what is asked joins this callback's requests, carried out
    /// once it returns. A window that is not open ignores what is asked.
    pub fn event_cx(&mut self, window: WindowId) -> EventCx<'_> {
        let open = self.windows.iter().find(|open| open.id == window);
        let mut state = open.map_or_else(WindowState::default, |open| open.state);
        state.reduced_motion = self.reduced_motion();
        let geometry = open.map(|open| open.geometry);
        EventCx::new(window, state, &mut self.requests, self.clipboard)
            .with_geometry(geometry)
            .with_event_loop(self.event_loop)
    }

    /// The clipboard, to copy to and paste from.
    pub fn clipboard(&mut self) -> &mut SystemClipboard {
        self.clipboard
    }

    /// The GPU misty draws with, for an app that renders into a
    /// [`GpuTexture`](crate::GpuTexture) of its own; keep a clone from
    /// [`Application::init`]. `None` only before the first window opens.
    pub fn gpu(&self) -> Option<&Gpu> {
        self.gpu.as_ref()
    }

    /// The user's appearance settings now: light or dark, the accent
    /// color, high contrast, and less motion, as far as the system says.
    /// Changes arrive as [`Input::AppearanceChanged`].
    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    /// Whether widgets cut their animations short now (scrolls jump,
    /// fades and spinners hold still): the override
    /// [`AppContext::set_reduced_motion`] set, or else the user's
    /// [`Appearance::reduced_motion`]. Widgets read it from
    /// [`EventCx::reduced_motion`] and
    /// [`Frame::reduced_motion`](crate::Frame::reduced_motion).
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
            .unwrap_or(self.appearance.reduced_motion)
    }

    /// Cuts every window's animations short (`Some(true)`), lets them run
    /// (`Some(false)`) whatever the user's settings say, or follows the
    /// settings again (`None`, the default): for an app's own setting or a
    /// `--reduced-motion` flag. Every window is redrawn.
    pub fn set_reduced_motion(&mut self, reduced_motion: Option<bool>) {
        self.reduced_motion = reduced_motion;
        self.requests.reduced_motion = Some(reduced_motion);
    }

    /// Quits the app: every window closes and [`run`](crate::run) returns.
    pub fn exit(&mut self) {
        self.requests.exit = true;
    }

    /// Opens another window with `options`, sharing the GPU and the fonts
    /// of the others. It is set up as soon as the callback returns, then
    /// [`Application::init`] runs for it and its inputs and frames arrive
    /// under the id returned. Refused when the window system will not open
    /// it, and with [`Error::Headless`] in a headless test.
    pub fn open_window(&mut self, options: WindowOptions) -> Result<WindowId, Error> {
        let event_loop = self.event_loop.ok_or(Error::Headless)?;
        let attributes = options.window_attributes(&monitors(event_loop));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(Error::create_window)?,
        );
        let id = WindowId(window.id());
        self.requests.opened.push(NewWindow { options, window });
        Ok(id)
    }

    /// Closes `window` once the callback returns; no more inputs or frames
    /// come for it. Closing the last window quits the app unless
    /// [`Application::exit_when_last_window_closes`] says otherwise.
    pub fn close_window(&mut self, window: WindowId) {
        self.requests.closed.push(window);
    }

    /// The open window `window`, for a file dialog to sit over.
    #[cfg(feature = "dialogs")]
    pub(crate) fn window_handle(&self, window: WindowId) -> Option<Arc<Window>> {
        self.windows
            .iter()
            .find(|open| open.id == window)
            .and_then(|open| open.window.clone())
    }

    /// The windows open now, in the order opened.
    pub(crate) fn window_ids(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.iter().map(|open| open.id)
    }

    /// A handle another thread can send the app messages through.
    pub fn waker(&self) -> Waker<M> {
        self.waker.clone()
    }
}

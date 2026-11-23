//! [`App`]: an app described by a view, which misty builds, lays out,
//! draws, and dispatches, and [`AppCx`], what its hooks reach the window
//! and the runtime through: the window's clipboard and title, opening and
//! closing windows, moving the keyboard focus, and quitting.

use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::app::window::{Size, WindowOptions};
use super::super::base::appearance::Appearance;
use super::super::base::key::Key;
use super::super::base::theme::Theme;
use super::super::platform::error::Error;
use super::super::platform::waker::Waker;
use super::super::platform::window_id::WindowId;
use super::super::widgets::drag::FileDropped;
use super::element::Element;

// ========================================================================
// Data Structures
// ========================================================================

/// A change to an [`App`]'s state that another thread sends through the
/// [`Waker`] [`App::start`] was given: misty runs it on the app's state on
/// the event loop's thread, then redraws every window.
///
/// ```no_run
/// use std::thread;
///
/// use misty::{Update, Waker};
///
/// struct Clock {
///     ticks: u64,
/// }
///
/// fn count_seconds(waker: Waker<Update<Clock>>) {
///     thread::spawn(move || {
///         // Fails once the app has quit: nothing is left to count for.
///         while waker.send(Box::new(|clock: &mut Clock| clock.ticks += 1)).is_ok() {
///             thread::sleep(std::time::Duration::from_secs(1));
///         }
///     });
/// }
/// ```
pub type Update<A> = Box<dyn FnOnce(&mut A) + Send>;

/// What an [`App`]'s hooks reach through: the window the input came to
/// (its clipboard, its title, a redraw), and the requests to open and
/// close windows or to quit, carried out once the hook returns.
#[derive(Debug)]
pub struct AppCx<'a> {
    /// Whether the app asked to close the window.
    close: bool,
    /// The window the key came to.
    cx: EventCx<'a>,
    /// Whether the app asked to quit.
    exit: bool,
    /// The id of the element the app asked to give the keyboard, if any.
    focus: Option<String>,
    /// Whether the app asked for hint labels over the window's controls.
    hints: bool,
}

/// What an app asked of the runtime while misty handled one input.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Asks {
    /// Close the window the input came to.
    pub(crate) close: bool,
    /// Quit.
    pub(crate) exit: bool,
    /// Give the keyboard to the element of the window the input came to
    /// with this id.
    pub(crate) focus: Option<String>,
    /// Show hint labels over the window's controls.
    pub(crate) hints: bool,
}

// ========================================================================
// Traits
// ========================================================================

/// An app described by a view: misty builds its windows' trees from
/// [`App::view`], lays them out, draws them, describes them to screen
/// readers, routes the user's input through them, and runs the callbacks
/// their controls were given on the app's state. Run it with
/// [`run_app`](crate::run_app); test it headlessly with
/// `misty::testing::TreeWindow`.
///
/// ```no_run
/// use misty::view::{checkbox, column, field, form};
/// use misty::{App, Element, Theme, WindowId, WindowOptions};
///
/// struct Demo {
///     theme: Theme,
///     wrap: bool,
/// }
///
/// impl App for Demo {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         column([form([field(
///             "Editor",
///             checkbox("Wrap long lines", self.wrap).on_change(|app: &mut Self, on| app.wrap = on),
///         )])])
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
///
/// fn main() -> Result<(), misty::Error> {
///     let demo = Demo { theme: Theme::dark(), wrap: true };
///     misty::run_app(demo, WindowOptions::new("demo"))
/// }
/// ```
pub trait App: Sized + 'static {
    /// The contents of `window`, built from the app's state: one app may
    /// describe many windows. Called on every input and every frame, so it
    /// only reads the state; the controls' callbacks change it.
    fn view(&self, window: WindowId) -> Element<'_, Self>;

    /// A key no element took: the app's keymap runs here. Returns whether
    /// the app used it. The default uses none. While a modal layer shows,
    /// only a layer made [`Overlay::keys_to_app`](crate::view::Overlay::keys_to_app)
    /// hands on the keys its elements leave; others keep them.
    fn key(&mut self, _key: Key, _cx: &mut AppCx<'_>) -> bool {
        false
    }

    /// A key let go that no element took, its code as it was pressed with
    /// the modifiers held as it was let go: for what lasts while a key is
    /// held (a scroll speeding up). Only this hook sees releases;
    /// [`App::key`] never does. It comes even while a modal layer keeps
    /// the keys, since a release ends what a press began. No release
    /// comes for a key held as the window loses the keyboard
    /// ([`App::focus_changed`]), so treat that as letting every key go.
    /// [`AppCx::event_cx`] says where the key sits
    /// ([`EventCx::physical_key`](crate::EventCx::physical_key)). The
    /// default does nothing.
    fn key_released(&mut self, _key: Key, _cx: &mut AppCx<'_>) {}

    /// Whether the app is done: checked after each input and screen reader
    /// action, and once it says `true` the app quits. A callback, which
    /// reaches only the app's state, quits by setting what this reads (a
    /// dialog's Quit button). The default never quits this way; a key
    /// quits through [`AppCx::exit`].
    fn should_exit(&self) -> bool {
        false
    }

    /// Called once, as the first window opens and before anything else,
    /// with the [`Waker`] that hands the app [`Update`]s from other
    /// threads (a folder read, a timer's tick) and wakes the event loop.
    /// Keep it, or clones of it, for the work that needs it; a
    /// [`Picker`](crate::picker::Picker) or a
    /// [`Thumbnails`](crate::Thumbnails) cache takes one too. The default
    /// drops it.
    fn start(&mut self, _waker: Waker<Update<Self>>) {}

    /// Called each time the event loop is about to sleep, after the inputs
    /// and updates that woke it, with the time `now`: the place an app's
    /// timers run (a search debounced until typing settles, an autosave),
    /// and where state a callback set becomes a request only `cx` can make
    /// (a callback sets `settings_open`; this opens the window). Returns
    /// when it next needs calling even if nothing happens, or `None` to
    /// wait for the next input; [`Deadlines`](crate::Deadlines) keeps
    /// several timers. Misty redraws every window once a time it returned
    /// has come, unless [`App::redraw_when_due`] says not to;
    /// `cx.event_cx().request_redraw()` asks sooner. `cx` is the
    /// window that had the last input, or the first window. Tests call it
    /// through `TreeWindow::about_to_wait` and choose `now` with
    /// `TreeWindow::about_to_wait_at`. The default asks for nothing.
    fn about_to_wait(&mut self, _now: Instant, _cx: &mut AppCx<'_>) -> Option<Instant> {
        None
    }

    /// Whether misty redraws every window once a time
    /// [`App::about_to_wait`] returned has come. An app that asks for a
    /// redraw itself whenever a timer changed what it shows says `false`,
    /// so a timer that only polls for work done elsewhere (a reply not
    /// there yet) costs no frame. The default redraws.
    fn redraw_when_due(&self) -> bool {
        true
    }

    /// The user asked to close `window` (its close button, Alt+F4, a title
    /// bar's close button without `on_close`): returns whether it closes
    /// now. An app with unsaved work returns `false` and asks first, then
    /// closes it with [`AppCx::close`] once the user says so. The default
    /// closes it.
    fn close_requested(&mut self, _window: WindowId, _cx: &mut AppCx<'_>) -> bool {
        true
    }

    /// `window` closed, however it was closed: the app forgets what it kept
    /// for it. The app quits with its last window. The default does
    /// nothing.
    fn window_closed(&mut self, _window: WindowId) {}

    /// `window` gained (`true`) or lost the keyboard focus to another
    /// window or app. The default does nothing.
    fn focus_changed(&mut self, _window: WindowId, _focused: bool) {}

    /// `window` was resized, or moved to a display of another scale: its
    /// inner size in logical pixels and its scale factor now. Misty lays
    /// the tree out again on its own; an app that saves its window size
    /// keeps it here. The default does nothing.
    fn resized(&mut self, _window: WindowId, _size: Size, _scale_factor: f64) {}

    /// A file dragged in from another app was dropped on `window` where no
    /// element took it with
    /// [`Element::on_file_drop`](crate::Element::on_file_drop): its path,
    /// and where the pointer was last seen. The default ignores it.
    fn file_dropped(&mut self, _window: WindowId, _file: FileDropped, _cx: &mut AppCx<'_>) {}

    /// A callback, a key hook, or an [`Update`] panicked with `why`: misty
    /// caught it and carries on, the app's state as the panic left it, and
    /// redraws. The default logs `why` as an error (the `log` crate); an
    /// app may show it instead. A panic here is not caught.
    fn on_panic(&mut self, why: &str) {
        log::error!("the app panicked and carried on: {why}");
    }

    /// The user's appearance settings (light or dark, the accent color,
    /// less motion), once when the first window opens and again whenever
    /// they change, for an app that follows them with
    /// [`Appearance::theme`]. The default ignores them.
    fn follow_appearance(&mut self, _appearance: Appearance) {}

    /// Whether the windows hold still whatever the user's settings say:
    /// `Some(true)` makes glides and fades jump, `Some(false)` lets them
    /// move, and `None` (the default) follows the user's less-motion
    /// setting. Read as the first window opens and after each input,
    /// update, and screen reader action, for an app with a setting or a
    /// `--reduced-motion` flag of its own.
    fn reduced_motion(&self) -> Option<bool> {
        None
    }

    /// The theme the windows are drawn in.
    fn theme(&self) -> &Theme;
}

// ========================================================================
// AppCx Implementations
// ========================================================================

impl<'a> AppCx<'a> {
    /// A context for a key that came to the window of `cx`, asking nothing
    /// yet.
    pub(crate) fn new(cx: EventCx<'a>) -> Self {
        Self {
            close: false,
            cx,
            exit: false,
            focus: None,
            hints: false,
        }
    }

    /// Closes the window the input came to; the app quits with its last
    /// window.
    pub fn close_window(&mut self) {
        self.close = true;
    }

    /// Closes `window`, any of the app's windows, once the hook returns;
    /// [`App::window_closed`] follows. The app quits with its last window.
    pub fn close(&mut self, window: WindowId) {
        match window == self.cx.window() {
            true => self.close = true,
            false => self.cx.close_window(window),
        }
    }

    /// Opens another window with `options`, its contents
    /// [`App::view`] called with the id returned, once the hook returns.
    /// Fails as the window system refuses.
    pub fn open_window(&mut self, options: WindowOptions) -> Result<WindowId, Error> {
        self.cx.open_window(options)
    }

    /// The window the input came to.
    pub fn window(&self) -> WindowId {
        self.cx.window()
    }

    /// Gives the keyboard, and the focus ring, to the element of the
    /// window the input came to known by `id` (its [`Element::id`]) when
    /// the window is next drawn, as spatial pane moves (Alt+H, J, K, L)
    /// need: a control takes it itself; a container passes it to its
    /// first tab stop, unless the keyboard is inside it already. A focus
    /// trap that is open (a modal layer, a menu) keeps it out unless the
    /// element is inside the trap. Logs a warning, saying why, when no
    /// element has the id or nothing inside it takes the keyboard. A
    /// callback, which reaches only the app's state, asks through a field
    /// that [`App::about_to_wait`] reads and passes here.
    ///
    /// [`Element::id`]: crate::Element::id
    pub fn focus(&mut self, id: impl Into<String>) {
        self.focus = Some(id.into());
    }

    /// Puts hint labels, as Vimium puts on links, over everything of the
    /// window the input came to that a screen reader can click (a button,
    /// each tab of a strip, each title bar button, a pressable element)
    /// or that takes typed text, inside the innermost modal layer while
    /// one shows, when the window is next drawn: typing a label clicks its
    /// target as a screen reader would, and gives a text field (or a
    /// target no widget took the click for) the keyboard; Esc, a press, or a
    /// wheel turn takes the labels away. While they show they take every
    /// key, as [`Hints`](crate::Hints) does. A callback asks through a
    /// field that [`App::about_to_wait`] reads and passes here.
    pub fn show_hints(&mut self) {
        self.hints = true;
    }

    /// Quits the app: every window closes.
    pub fn exit(&mut self) {
        self.exit = true;
    }

    /// The window the input came to, to read the clipboard, set its title,
    /// or ask for a redraw.
    pub fn event_cx(&mut self) -> &mut EventCx<'a> {
        &mut self.cx
    }

    /// What the app asked of the runtime.
    pub(crate) fn asks(&self) -> Asks {
        Asks {
            close: self.close,
            exit: self.exit,
            focus: self.focus.clone(),
            hints: self.hints,
        }
    }
}

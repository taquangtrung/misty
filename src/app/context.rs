//! [`EventCx`]: what a widget, and the app, reach one window through while
//! they handle an input or a tick. It only collects requests (a redraw, a
//! pointer shape, a title, moving the window) for the runtime to carry out
//! once the app's callback returns, and lends what every widget may need
//! (the clipboard, the pointer, the modifiers held, whether to cut
//! animations short), so it needs no live event loop: an app gets one from
//! [`AppContext::event_cx`](crate::AppContext::event_cx), and a test from
//! the `testing` feature's `TestWindow`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use winit::event_loop::ActiveEventLoop;
use winit::window::{self, Window};

use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Mods, PhysicalKey};
use super::super::platform::clipboard::SystemClipboard;
use super::super::platform::error::Error;
use super::super::platform::resize::ResizeEdge;
use super::super::platform::window_id::WindowId;
use super::super::render::font::Font;
use super::super::render::image::Image;
use super::window::{OptionAsAlt, Position, WindowGeometry, WindowOptions, monitors};

// ========================================================================
// Constants
// ========================================================================

/// The id the first window a headless test opens is given; the next count
/// up from it, far from the ids a test window starts with.
const FIRST_HEADLESS_WINDOW: u64 = 1 << 40;

/// The id the next window a headless test opens is given.
static NEXT_HEADLESS_WINDOW: AtomicU64 = AtomicU64::new(FIRST_HEADLESS_WINDOW);

// ========================================================================
// Data Structures
// ========================================================================

/// What a widget handling an input or a tick for one window, and the app,
/// reach the window through: requests carried out once the callback
/// returns (a redraw, a title, a move), the pointer and modifiers as the
/// runtime last read them, whether to cut animations short, and the
/// clipboard.
#[derive(Debug)]
pub struct EventCx<'a> {
    /// The clipboard every widget copies to and pastes from.
    clipboard: &'a mut SystemClipboard,
    /// The event loop new windows are opened on, or `None` where there is
    /// none (a test).
    event_loop: Option<&'a ActiveEventLoop>,
    /// Where the window is and how big, as the runtime last read it;
    /// `None` for a window that is not open.
    geometry: Option<WindowGeometry>,
    /// What the app and its widgets asked for.
    requests: &'a mut Requests,
    /// The window's pointer, modifiers, and maximized state.
    state: WindowState,
    /// The window the input came to.
    window: WindowId,
}

/// What lasts between one window's inputs, as the runtime last read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WindowState {
    /// How many presses in a row the last pointer press made.
    pub(crate) clicks: u8,
    /// Whether the key being handled is a repeat of a key held down.
    pub(crate) key_repeat: bool,
    /// Whether the window is maximized.
    pub(crate) maximized: bool,
    /// The modifier keys held.
    pub(crate) mods: Mods,
    /// Where the key being handled sits, if the window system said.
    pub(crate) physical_key: Option<PhysicalKey>,
    /// Where the pointer is, while it is over the window.
    pub(crate) pointer: Option<Point>,
    /// Whether widgets cut their animations short.
    pub(crate) reduced_motion: bool,
}

/// What an app and its widgets asked for in one callback.
#[derive(Debug, Default)]
pub(crate) struct Requests {
    /// What was asked of each window's window system, in order.
    pub(crate) actions: Vec<(WindowId, WindowAction)>,
    /// The windows the app asked to close.
    pub(crate) closed: Vec<WindowId>,
    /// The pointer shapes widgets asked for, in the order they asked.
    pub(crate) cursors: Vec<(WindowId, CursorIcon)>,
    /// Whether the app asked to quit.
    pub(crate) exit: bool,
    /// The windows a headless test opened, with what they were opened
    /// with, for the test to set up.
    pub(crate) minted: Vec<(WindowId, WindowOptions)>,
    /// Whether a window opened with no event loop is set up by the test
    /// that holds these requests, rather than refused.
    pub(crate) mints: bool,
    /// The windows the app opened, still to get their surfaces.
    pub(crate) opened: Vec<NewWindow>,
    /// The reduced-motion override the app set last, if it set one:
    /// `Some(None)` to follow the user's settings again.
    pub(crate) reduced_motion: Option<Option<bool>>,
    /// The windows the app asked to draw again, soon.
    pub(crate) redraws: Vec<WindowId>,
    /// The windows the app asked to draw again later, with when.
    pub(crate) scheduled: Vec<(WindowId, Instant)>,
}

/// A window opened in a callback, with what it was opened with.
#[derive(Debug)]
pub(crate) struct NewWindow {
    /// How it was opened: its font and when it shows.
    pub(crate) options: WindowOptions,
    /// The window itself.
    pub(crate) window: Arc<Window>,
}

/// Something asked of a window's window system.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WindowAction {
    /// Let the user move the window with the pointer.
    Drag,
    /// Raise the window and give it the keyboard focus.
    Focus,
    /// Minimize the window.
    Minimize,
    /// Move the window's outer top left corner here on the desktop.
    Move(Position),
    /// Let the user resize the window from this edge with the pointer.
    Resize(ResizeEdge),
    /// Draw the window's text in this font from the next frame.
    SetFont(Font),
    /// Show this picture for the window, or the system's default.
    SetIcon(Option<Image>),
    /// Anchor the input method's candidate window beside this area.
    SetImeArea(Rect),
    /// Let the input method compose (`true`), or pass keys through.
    SetImeAllowed(bool),
    /// Make these Option keys Alt on macOS.
    SetOptionAsAlt(OptionAsAlt),
    /// Name the window this, in the system's title bar, task bar, and
    /// switcher, and to screen readers.
    SetTitle(String),
    /// Let what the app leaves transparent show the desktop (`true`), or
    /// keep the window opaque.
    SetTransparent(bool),
    /// Show the window (`true`) or hide it.
    SetVisible(bool),
    /// Fill the screen with the window, or restore it when it does.
    ToggleFullscreen,
    /// Maximize the window, or restore it when it is maximized.
    ToggleMaximize,
}

// ========================================================================
// EventCx Implementations
// ========================================================================

impl<'a> EventCx<'a> {
    /// A context for `window` in `state`, adding to `requests` and lending
    /// `clipboard`.
    pub(crate) fn new(
        window: WindowId,
        state: WindowState,
        requests: &'a mut Requests,
        clipboard: &'a mut SystemClipboard,
    ) -> Self {
        Self {
            clipboard,
            event_loop: None,
            geometry: None,
            requests,
            state,
            window,
        }
    }

    /// This context lent for a while: the same window, clipboard, and
    /// requests, for a step that takes a context of its own.
    pub(crate) fn reborrow(&mut self) -> EventCx<'_> {
        EventCx {
            clipboard: &mut *self.clipboard,
            event_loop: self.event_loop,
            geometry: self.geometry,
            requests: &mut *self.requests,
            state: self.state,
            window: self.window,
        }
    }

    /// This context, saying the window is where `geometry` says.
    pub(crate) fn with_geometry(self, geometry: Option<WindowGeometry>) -> Self {
        Self { geometry, ..self }
    }

    /// This context opening new windows on `event_loop`.
    pub(crate) fn with_event_loop(self, event_loop: Option<&'a ActiveEventLoop>) -> Self {
        Self { event_loop, ..self }
    }

    /// Opens a window with `options`, set up and drawn once this callback
    /// returns: on the event loop, or by the headless test holding the
    /// requests. Fails with [`Error::Headless`] where neither can, and as
    /// the window system refuses.
    pub(crate) fn open_window(&mut self, options: WindowOptions) -> Result<WindowId, Error> {
        if let Some(event_loop) = self.event_loop {
            let attributes = options.window_attributes(&monitors(event_loop));
            let window = Arc::new(
                event_loop
                    .create_window(attributes)
                    .map_err(Error::create_window)?,
            );
            let id = WindowId(window.id());
            self.requests.opened.push(NewWindow { options, window });
            return Ok(id);
        }
        if !self.requests.mints {
            return Err(Error::Headless);
        }
        let raw = NEXT_HEADLESS_WINDOW.fetch_add(1, Ordering::Relaxed);
        let id = WindowId(window::WindowId::from(raw));
        self.requests.minted.push((id, options));
        Ok(id)
    }

    /// Closes `window` once this callback returns.
    pub(crate) fn close_window(&mut self, window: WindowId) {
        self.requests.closed.push(window);
    }

    /// The window the input came to.
    pub fn window(&self) -> WindowId {
        self.window
    }

    /// Where the pointer is in the window, in physical pixels, or `None`
    /// while it is outside.
    pub fn pointer(&self) -> Option<Point> {
        self.state.pointer
    }

    /// The modifier keys held.
    pub fn mods(&self) -> Mods {
        self.state.mods
    }

    /// Whether the [`Input::Key`](crate::Input::Key) being handled is the
    /// window system repeating a key held down rather than a new press: a
    /// prefix's keymap takes only the first press, and a terminal says
    /// which it was to a program that asked.
    pub fn is_key_repeat(&self) -> bool {
        self.state.key_repeat
    }

    /// Where the key being handled ([`Input::Key`](crate::Input::Key) or
    /// [`Input::KeyReleased`](crate::Input::KeyReleased)) sits on the
    /// keyboard, with every modifier held, Shift included, for chords
    /// written by key place: Ctrl+Shift+/ comes as the key `C-?` and the
    /// physical key `/` with Ctrl and Shift. `None` for a key at a place
    /// misty has no name for (a numpad or media key), when the window
    /// system named none, and in a test that set none
    /// ([`TestWindow::set_physical_key`](crate::testing::TestWindow::set_physical_key)).
    /// A tree app reads it in [`App::key`](crate::App::key) through
    /// [`AppCx::event_cx`](crate::AppCx::event_cx).
    pub fn physical_key(&self) -> Option<PhysicalKey> {
        self.state.physical_key
    }

    /// How many presses of one button in a row the
    /// [`Input::PointerButton`](crate::Input::PointerButton) being handled
    /// makes, each soon enough after and close enough to the one before:
    /// 1 for a single press, 2 for a double click, 3 for a triple, and on
    /// up while presses keep coming. A release reports the count of the
    /// press it ends. A widget that selects a word on a double click and
    /// a line on a triple reads it on the press.
    pub fn click_count(&self) -> u8 {
        self.state.clicks
    }

    /// Whether the window is maximized, as the window system last said.
    pub fn is_maximized(&self) -> bool {
        self.state.maximized
    }

    /// Where the window is, how big, and whether it is maximized, to save
    /// when it closes (in [`Input::CloseRequested`](crate::Input::CloseRequested))
    /// and open it there again with [`WindowOptions::with_geometry`];
    /// `None` for a window that is not open.
    pub fn geometry(&self) -> Option<WindowGeometry> {
        self.geometry
    }

    /// Whether to cut animations short: scrolls jump, fades and spinners
    /// hold still. The user's [`Appearance::reduced_motion`](crate::Appearance::reduced_motion)
    /// setting unless the app overrode it with
    /// [`AppContext::set_reduced_motion`](crate::AppContext::set_reduced_motion).
    /// A widget reads it where it starts or steps an animation, and while
    /// drawing from [`Frame::reduced_motion`](crate::Frame::reduced_motion).
    pub fn reduced_motion(&self) -> bool {
        self.state.reduced_motion
    }

    /// The clipboard, to copy to and paste from.
    pub fn clipboard(&mut self) -> &mut SystemClipboard {
        self.clipboard
    }

    /// Asks for a new frame of the window soon:
    /// [`Application::render`](crate::Application::render) runs for it
    /// again. Nothing is redrawn on its own except after a resize or a
    /// scale change.
    pub fn request_redraw(&mut self) {
        self.requests.redraws.push(self.window);
    }

    /// Asks for a new frame of the window at `at`, waking the event loop
    /// for it, such as for the next step of a blink; asked again before
    /// then, the earliest time wins.
    pub fn request_redraw_at(&mut self, at: Instant) {
        self.requests.scheduled.push((self.window, at));
    }

    /// Shows `icon` as the pointer over the window once the callback
    /// returns. Within one callback any other shape wins over
    /// [`CursorIcon::Default`], so a widget the pointer left asking for the
    /// default never undoes the shape of the widget it entered; among
    /// other shapes the last wins.
    pub fn set_cursor(&mut self, icon: CursorIcon) {
        self.requests.cursors.push((self.window, icon));
    }

    /// Lets the user move the window with the pointer, as if dragging a
    /// system title bar. Ask while the left button is pressed; otherwise,
    /// or where the window system does not support it, nothing happens.
    pub fn drag_window(&mut self) {
        self.act(WindowAction::Drag);
    }

    /// Lets the user resize the window from `edge` with the pointer. Ask
    /// while the left button is pressed; otherwise, or where the window
    /// system does not support it, nothing happens.
    pub fn drag_resize_window(&mut self, edge: ResizeEdge) {
        self.act(WindowAction::Resize(edge));
    }

    /// Minimizes the window.
    pub fn minimize(&mut self) {
        self.act(WindowAction::Minimize);
    }

    /// Maximizes the window, or restores it when it is maximized.
    pub fn toggle_maximize(&mut self) {
        self.act(WindowAction::ToggleMaximize);
    }

    /// Fills the screen with the window, with no frame or title bar, or
    /// restores it when it does. On macOS it is the system's own full
    /// screen, in a Space of its own.
    pub fn toggle_fullscreen(&mut self) {
        self.act(WindowAction::ToggleFullscreen);
    }

    /// Shows the window (`true`) or hides it, keeping it open: a popup hides
    /// on [`Input::FocusLost`](crate::Input::FocusLost) and shows again when
    /// next wanted. Wayland cannot hide a window; there the app closes it
    /// and opens it again.
    pub fn set_visible(&mut self, visible: bool) {
        self.act(WindowAction::SetVisible(visible));
    }

    /// Moves the window's outer top left corner to `position` on the
    /// desktop. Wayland lets no app place its windows, so nothing happens
    /// there.
    pub fn move_window(&mut self, position: Position) {
        self.act(WindowAction::Move(position));
    }

    /// Raises the window and gives it the keyboard focus, such as when a
    /// second launch of the app hands over its arguments. Window systems
    /// may refuse a window that did not just get an input, flashing its
    /// task bar entry instead.
    pub fn focus_window(&mut self) {
        self.act(WindowAction::Focus);
    }

    /// Names the window `title`: in the system's title bar, task bar, and
    /// switcher, and to screen readers.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.act(WindowAction::SetTitle(title.into()));
    }

    /// Shows `icon` for the window, or the system's default for `None`,
    /// where the system takes a window's icon (see
    /// [`WindowOptions::with_icon`]).
    pub fn set_icon(&mut self, icon: Option<Image>) {
        self.act(WindowAction::SetIcon(icon));
    }

    /// Lets what the app leaves transparent show the desktop behind
    /// (`true`), as [`WindowOptions::with_transparent`] does when the
    /// window opens, or makes the window opaque again: for a translucency
    /// setting the user changes while the app runs. Where the GPU surface
    /// cannot composite with alpha, the window stays opaque.
    pub fn set_transparent(&mut self, transparent: bool) {
        self.act(WindowAction::SetTransparent(transparent));
    }

    /// Makes the Option keys `option_as_alt` names act as Alt in the window
    /// on macOS, for a setting the user changes while the app runs; other
    /// systems ignore it.
    pub fn set_option_as_alt(&mut self, option_as_alt: OptionAsAlt) {
        self.act(WindowAction::SetOptionAsAlt(option_as_alt));
    }

    /// Draws the window's text in `font` from its next frame, and sizes its
    /// cells for it ([`Frame::cell_metrics`](crate::Frame::cell_metrics)):
    /// for a font or size the user changes while the app runs. The window
    /// is redrawn.
    pub fn set_font(&mut self, font: Font) {
        self.act(WindowAction::SetFont(font));
        self.request_redraw();
    }

    /// Lets the window's input method compose text (`true`), or has it
    /// pass keys straight through (`false`).
    pub(crate) fn set_ime_allowed(&mut self, allowed: bool) {
        self.act(WindowAction::SetImeAllowed(allowed));
    }

    /// Anchors the window's input method candidate window beside `area`,
    /// the text cursor, in physical pixels.
    pub(crate) fn set_ime_cursor_area(&mut self, area: Rect) {
        self.act(WindowAction::SetImeArea(area));
    }

    /// Asks `action` of the window's window system.
    fn act(&mut self, action: WindowAction) {
        self.requests.actions.push((self.window, action));
    }
}

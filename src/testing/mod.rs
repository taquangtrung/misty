//! A headless test harness for apps built on misty, behind the `testing`
//! feature: a [`TestWindow`] stands in for an open window, so a test feeds
//! widgets [`Input`]s through a real [`EventCx`] and reads back what they
//! asked of the runtime (a redraw, a pointer shape) and wrote to the
//! clipboard, with no event loop and no display. Frames are built with
//! [`TestWindow::frame`] and inspected with [`drawn_text`] and, for what
//! they describe to screen readers, [`access_tree`] and [`access_node`], or
//! rendered to pixels with [`TestWindow::render`] on whatever GPU adapter
//! is found (`None`, after saying why once, when there is none).
//! [`ui_text`] writes what a frame describes as indented text, one line per
//! node, for a test to compare whole against what it expects. A
//! [`TreeWindow`] does the same for a whole [`App`](crate::App): it owns
//! the app, sends its tree keys, text, clicks, and screen reader actions,
//! and reads back what it drew and described. A [`TestApp`] does it for
//! an [`Application`](crate::Application) run by [`run`](crate::run): it
//! calls the app as the event loop would, hands on the messages its
//! [`Waker`](crate::Waker)s send, and reads back its frames.
//!
//! ```
//! use misty::testing::TestWindow;
//! use misty::{ButtonInput, Input, Point, PointerButton, TextField, Widget};
//!
//! let mut window = TestWindow::new(320, 80);
//! let mut field = TextField::new();
//! let mut frame = window.frame();
//! let area = frame.bounds();
//! field.draw(&mut frame, area, &misty::Theme::dark());
//! let press = ButtonInput::press(PointerButton::Left, Point::new(10.0, 10.0));
//! window.feed(&mut field, &Input::PointerButton(press));
//! assert!(window.take_redraw());
//! ```

use std::fmt::Write;

use accesskit::{Action, Node, NodeId, Role, Toggled, TreeUpdate};
use winit::window;

use super::app::access::find_node;
#[cfg(test)]
use super::app::context::WindowAction;
use super::app::context::{EventCx, Requests, WindowState};
use super::app::focus::FocusId;
use super::app::frame::Frame;
use super::app::offscreen;
use super::app::widget::Widget;
use super::app::window::{Size, WindowGeometry, WindowOptions};
use super::base::cursor::CursorIcon;
use super::base::geometry::{Point, Rect};
use super::base::key::{Key, KeyCode, KeyScheme, Mods, PhysicalKey, folded_shift_place};
use super::input::event::{Input, InputReader};
use super::platform::clipboard::SystemClipboard;
use super::platform::window_id::WindowId;
use super::render::gpu::Gpu;

pub use super::app::offscreen::{FrameTimes, Snapshot, time_frames};
pub use app::TestApp;
pub use tree::TreeWindow;

mod app;
mod tree;

/// What the tests of an app's picker sources build on: a [`picker::FakeSource`]
/// whose rows, batches, failures, actions, and previews a test scripts, keys
/// fed to a picker as a [`picker::Script`] in misty's key notation, and a
/// [`picker::Clock`] the test moves, so no test waits on a thread. Open the
/// picker with [`Picker::synchronous`](crate::picker::Picker::synchronous).
///
/// ```
/// use misty::picker::{Picker, PickerMemory, Reply, RowKey};
/// use misty::testing::TestWindow;
/// use misty::testing::picker::{Clock, FakeOutcome, FakeSource, PICK, Picked, Script};
///
/// let source = FakeSource::rows(&["one", "two"]);
/// let mut picker = Picker::synchronous(Box::new(source), PickerMemory::default());
/// let mut window = TestWindow::new(400, 300);
/// let replies = Script::parse("t w RET").run(&mut picker, &mut Clock::new(), &mut window.cx());
/// let picked = Picked { action: PICK, row: RowKey(1) };
/// assert_eq!(replies.last(), Some(&Reply::Run(FakeOutcome::Picked(picked))));
/// ```
pub mod picker {
    pub use super::super::widgets::picker::harness::{
        Clock, FakeFeed, FakeLog, FakeOutcome, FakeSource, PICK, Picked, Script, batch, open, rows,
    };
}

// ========================================================================
// Constants
// ========================================================================

/// The id every test window has.
const TEST_WINDOW_ID: u64 = 1;

/// What each level of [`ui_text`] is indented by.
const UI_TEXT_INDENT: &str = "  ";

// ========================================================================
// Data Structures
// ========================================================================

/// A window with no window system behind it: what lasts between inputs,
/// what widgets asked for, and a clipboard that never leaves the process.
#[derive(Debug)]
pub struct TestWindow {
    /// The clipboard lent to widgets, in the process only.
    clipboard: SystemClipboard,
    /// Its height in physical pixels.
    height: u32,
    /// Its id.
    id: WindowId,
    /// What lasts between inputs: the pointer and the modifiers held.
    input: InputReader,
    /// Whether the keys sent count as repeats of a key held down.
    key_repeat: bool,
    /// Which keyboard conventions widgets follow.
    key_scheme: KeyScheme,
    /// Whether the window counts as maximized.
    maximized: bool,
    /// Where the keys sent sit, as [`EventCx::physical_key`] reports.
    physical_key: Option<PhysicalKey>,
    /// Whether widgets cut their animations short.
    reduced_motion: bool,
    /// What widgets asked for since the test last looked.
    requests: Requests,
    /// Its width in physical pixels.
    width: u32,
}

// ========================================================================
// TestWindow Implementations
// ========================================================================

impl TestWindow {
    /// A window `width` by `height` physical pixels at scale factor 1, the
    /// pointer outside it and no modifier held.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            clipboard: SystemClipboard::in_process(),
            height,
            id: WindowId(window::WindowId::from(TEST_WINDOW_ID)),
            input: InputReader::default(),
            key_repeat: false,
            key_scheme: KeyScheme::STANDARD,
            maximized: false,
            physical_key: None,
            reduced_motion: false,
            requests: Requests::default(),
            width,
        }
    }

    /// A window as [`TestWindow::new`] makes, its widgets taking the Vim
    /// and Emacs keys ([`KeyScheme::VIM_AND_EMACS`]), for misty's tests of
    /// those keys.
    #[cfg(test)]
    pub(crate) fn keyboard_first(width: u32, height: u32) -> Self {
        let mut window = Self::new(width, height);
        window.key_scheme = KeyScheme::VIM_AND_EMACS;
        window
    }

    /// The window's id, which the contexts it hands out name.
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// A window `width` by `height` physical pixels known as `id`, whose
    /// contexts open windows for the test that holds it to set up
    /// ([`TestWindow::take_windows`]) rather than refusing.
    pub(crate) fn opening(width: u32, height: u32, id: WindowId) -> Self {
        let mut window = Self::new(width, height);
        window.id = id;
        window.requests.mints = true;
        window
    }

    /// The windows opened, with what they were opened with, and those
    /// asked to close since the last call, in order.
    pub(crate) fn take_windows(&mut self) -> (Vec<(WindowId, WindowOptions)>, Vec<WindowId>) {
        let opened = std::mem::take(&mut self.requests.minted);
        let closed = std::mem::take(&mut self.requests.closed);
        (opened, closed)
    }

    /// Makes the window `width` by `height` physical pixels from now on.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Counts the keys sent from now on as repeats of a key held down
    /// (`true`), as [`EventCx::is_key_repeat`] reports, or as new presses.
    pub fn set_key_repeat(&mut self, repeat: bool) {
        self.key_repeat = repeat;
    }

    /// Reports the keys sent from now on as sitting at `place` (`None`, the
    /// default, for no place), as [`EventCx::physical_key`] says: a key
    /// sent as `C-?` with `PhysicalKey::new(KeyCode::Char('/'), Mods::CTRL
    /// | Mods::SHIFT)` is Ctrl+Shift+/ on a US layout.
    pub fn set_physical_key(&mut self, place: Option<PhysicalKey>) {
        self.physical_key = place;
    }

    /// Sets where the key written `token` sits, when the notation writes
    /// Shift on a key that drops it (`S-<space>`) and the test set no place
    /// itself; says whether it did, for the sender to clear it after.
    pub(crate) fn place_written(&mut self, token: &str) -> bool {
        if self.physical_key.is_some() {
            return false;
        }
        self.physical_key = folded_shift_place(token);
        self.physical_key.is_some()
    }

    /// Counts the window as maximized, or not, from now on.
    pub fn set_maximized(&mut self, maximized: bool) {
        self.maximized = maximized;
    }

    /// Has widgets follow `key_scheme` from now on, as an app's
    /// [`Application::key_scheme`](crate::Application::key_scheme) has
    /// them: [`KeyScheme::STANDARD`] until said otherwise.
    pub fn set_key_scheme(&mut self, key_scheme: KeyScheme) {
        self.key_scheme = key_scheme;
    }

    /// Has widgets cut their animations short (`true`) from now on, as the
    /// user's reduced-motion setting or
    /// [`AppContext::set_reduced_motion`](crate::AppContext::set_reduced_motion)
    /// would: what [`EventCx::reduced_motion`] and the frames from
    /// [`TestWindow::frame`] and [`TestWindow::render`] say.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    /// A context for the window as it is now, as an app gets from
    /// [`AppContext::event_cx`](crate::AppContext::event_cx). Its
    /// [`EventCx::geometry`] is the window's size, maximized as
    /// [`TestWindow::set_maximized`] said, with no position.
    pub fn cx(&mut self) -> EventCx<'_> {
        let state = self.state();
        let geometry = self.geometry();
        let id = self.id();
        EventCx::new(id, state, &mut self.requests, &mut self.clipboard)
            .with_geometry(Some(geometry))
    }

    /// Follows `input` as the runtime would (the pointer, the modifiers),
    /// then hands it to `handle` with a context, returning what `handle`
    /// returns: a widget's response.
    pub fn send<R>(
        &mut self,
        input: &Input,
        handle: impl FnOnce(&Input, &mut EventCx<'_>) -> R,
    ) -> R {
        self.input.follow(input);
        handle(input, &mut self.cx())
    }

    /// Follows `input` as the runtime would, then hands it to `widget`,
    /// returning its response.
    pub fn feed<W: Widget>(&mut self, widget: &mut W, input: &Input) -> W::Response {
        self.send(input, |input, cx| widget.handle(input, cx))
    }

    /// Whether a redraw was asked for since the last call.
    pub fn take_redraw(&mut self) -> bool {
        let asked = !self.requests.redraws.is_empty();
        self.requests.redraws.clear();
        asked
    }

    /// What was asked of the window system since the last call, in order.
    #[cfg(test)]
    pub(crate) fn take_actions(&mut self) -> Vec<WindowAction> {
        self.requests
            .actions
            .drain(..)
            .map(|(_, action)| action)
            .collect()
    }

    /// The pointer shape last asked for, if any, as the runtime would pick
    /// it among the shapes asked in one callback.
    pub fn cursor(&self) -> Option<CursorIcon> {
        self.requests.cursors.last().map(|(_, icon)| *icon)
    }

    /// Where the pointer is in the window, as the inputs sent left it.
    pub fn pointer(&self) -> Option<Point> {
        self.input.pointer()
    }

    /// The modifiers held, as the inputs sent left them.
    pub fn mods(&self) -> Mods {
        self.input.mods()
    }

    /// The clipboard widgets copy to and paste from.
    pub fn clipboard(&mut self) -> &mut SystemClipboard {
        &mut self.clipboard
    }

    /// An empty frame the size of the window, measuring text with the
    /// system fonts and no GPU behind it, to draw widgets into so they lay
    /// themselves out (most hit test where they were last drawn) and to
    /// read back with [`drawn_text`].
    pub fn frame(&self) -> Frame {
        let mut frame = offscreen::plain_frame(self.width, self.height);
        frame.set_reduced_motion(self.reduced_motion);
        frame
    }

    /// A frame the size of the window drawn by `draw` and read back as
    /// pixels, or `None`, after saying why once per run, when no GPU
    /// adapter can draw it.
    pub fn render(&self, draw: impl FnOnce(&mut Frame)) -> Option<Snapshot> {
        let reduced_motion = self.reduced_motion;
        offscreen::render(self.width, self.height, |frame| {
            frame.set_reduced_motion(reduced_motion);
            draw(frame);
        })
    }

    /// What [`TestWindow::render`] draws, with the frame at
    /// `scale_factor`.
    #[cfg(test)]
    pub(crate) fn render_at(
        &self,
        scale_factor: f64,
        draw: impl FnOnce(&mut Frame),
    ) -> Option<Snapshot> {
        let reduced_motion = self.reduced_motion;
        offscreen::render_at(self.width, self.height, scale_factor, |frame| {
            frame.set_reduced_motion(reduced_motion);
            draw(frame);
        })
    }

    /// What lasts between inputs, as the runtime would read it now.
    fn state(&self) -> WindowState {
        WindowState {
            clicks: self.input.click_count(),
            key_repeat: self.key_repeat,
            key_scheme: self.key_scheme,
            maximized: self.maximized,
            mods: self.input.mods(),
            physical_key: self.physical_key,
            pointer: self.input.pointer(),
            reduced_motion: self.reduced_motion,
        }
    }

    /// The window's size, maximized as [`TestWindow::set_maximized`] said,
    /// with no position.
    fn geometry(&self) -> WindowGeometry {
        WindowGeometry {
            maximized: self.maximized,
            position: None,
            size: Size::new(self.width, self.height),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The GPU [`TestWindow::render`] draws with, to make the
/// [`GpuTexture`](crate::GpuTexture)s a test renders into and draws; `None`
/// when no GPU adapter can draw, as `render` then says.
pub fn gpu() -> Option<Gpu> {
    offscreen::gpu()
}

/// The key that types `c`: a line feed as Enter, anything else as itself.
fn typed_key(c: char) -> Key {
    let code = match c {
        '\n' => KeyCode::Enter,
        c => KeyCode::Char(c),
    };
    Key::new(code, Mods::NONE)
}

/// The text `frame` holds, one string per piece of text drawn at one place
/// (a line of cells comes in several pieces around its wide characters),
/// in the order drawn: the base layer, then the overlay.
pub fn drawn_text(frame: &Frame) -> Vec<String> {
    [&frame.draw, &frame.overlay]
        .into_iter()
        .flat_map(|layer| layer.text())
        .map(|segment| segment.runs.iter().map(|run| run.text.as_str()).collect())
        .collect()
}

/// The accessibility tree `frame` describes, as the window would send it to
/// a screen reader: every node [`Frame::access`] and the widgets' `access`
/// methods added, under a window node titled `title`.
pub fn access_tree(frame: &Frame, title: &str) -> TreeUpdate {
    frame.access.tree_update(title)
}

/// The controls `frame` describes that say nothing of what they are: each
/// one a press acts on, or a button, tab, link, or menu item, with no name
/// for a screen reader, which is also what an icon-only control's tooltip
/// says. Each as its role and number, for a test to assert there are none,
/// so a control showing only an icon cannot ship without words.
pub fn unnamed_controls(frame: &Frame) -> Vec<String> {
    let tree = access_tree(frame, "unnamed controls");
    tree.nodes
        .iter()
        .filter(|(_, node)| is_control(node))
        .filter(|(_, node)| node.label().is_none_or(|label| label.trim().is_empty()))
        .map(|(id, node)| format!("{:?} {}", node.role(), id.0))
        .collect()
}

/// The elements `frame` drew that scroll themselves (a
/// [`lazy_column`](crate::view::lazy_column), a [`list`](crate::view::list),
/// a [`table`](crate::view::table)) inside a scrolling container, which
/// leaves them no height to fill: each by its kind and place in the tree,
/// for a test to assert there are none. Such an element goes in a
/// [`Page::filled`](crate::view::Page::filled) page, not a scrolling one.
pub fn nested_scrolls(frame: &Frame) -> Vec<String> {
    frame.nested_scrolls.clone()
}

/// The controls `draw` draws of `state` that the pointer does not light:
/// each control [`unnamed_controls`] would look at, and each region that
/// takes only the pointer's press, is hovered in turn,
/// `input` handing the pointer's moves to `state`, and one whose frame stays
/// as it was is named, by its label or else its role and number, for a test
/// to assert there are none. `state` is whatever the test drives: a
/// [`TestWindow`] and a widget, a page, an app's window. Every clickable part
/// lights under the pointer (see [`Element::unlit`](crate::Element::unlit)
/// for the exception).
pub fn unlit_controls<S>(
    state: &mut S,
    input: impl FnMut(&mut S, &Input),
    mut draw: impl FnMut(&mut S) -> Frame,
) -> Vec<String> {
    unlit_in(state, input, |state| Look::of(&draw(state)))
}

/// What a frame showed, kept to compare with another.
pub(crate) struct Look {
    /// What it painted, base layer and overlay, as written out.
    paint: String,
    /// What can be pressed, by name and the middle of where it was drawn:
    /// the controls a screen reader is told of, and the regions that take
    /// only the pointer's press.
    targets: Vec<(String, Point)>,
}

impl Look {
    /// What `frame` showed.
    pub(crate) fn of(frame: &Frame) -> Self {
        let tree = access_tree(frame, "unlit controls");
        let middle_of =
            |rect: &Rect| Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        // What asked to stay unlit (a field) is no fault, however described.
        let unlit: Vec<Point> = frame
            .pressables
            .iter()
            .filter(|(_, _, lit)| !lit)
            .map(|(rect, _, _)| middle_of(rect))
            .collect();
        let mut targets: Vec<(String, Point)> = tree
            .nodes
            .iter()
            .filter(|(_, node)| is_control(node))
            .filter_map(|(id, node)| {
                let bounds = node.bounds()?;
                let name = match node.label() {
                    Some(label) if !label.trim().is_empty() => label.to_string(),
                    _ => format!("{:?} {}", node.role(), id.0),
                };
                let middle = Point::new(
                    ((bounds.x0 + bounds.x1) / 2.0) as f32,
                    ((bounds.y0 + bounds.y1) / 2.0) as f32,
                );
                Some((name, middle))
            })
            .filter(|(_, middle)| !unlit.contains(middle))
            .collect();
        for (rect, name, lit) in &frame.pressables {
            let middle = middle_of(rect);
            if *lit && !targets.iter().any(|(_, known)| *known == middle) {
                targets.push((name.clone(), middle));
            }
        }
        Self {
            paint: format!("{:?}{:?}", frame.draw, frame.overlay),
            targets,
        }
    }
}

/// [`unlit_controls`] over what `look` sees of `state` each time.
pub(crate) fn unlit_in<S>(
    state: &mut S,
    mut input: impl FnMut(&mut S, &Input),
    mut look: impl FnMut(&mut S) -> Look,
) -> Vec<String> {
    let targets = look(state).targets;
    let mut unlit = Vec::new();
    for (name, middle) in targets {
        input(state, &Input::PointerLeft);
        let still = look(state).paint;
        input(state, &Input::PointerMoved(middle));
        if look(state).paint == still {
            unlit.push(name);
        }
    }
    input(state, &Input::PointerLeft);
    unlit
}

/// Whether `node` is a control: one a press acts on, or a button, tab,
/// link, or menu item.
fn is_control(node: &Node) -> bool {
    node.supports_action(Action::Click)
        || matches!(
            node.role(),
            Role::Button | Role::Link | Role::Tab | Role::MenuItem
        )
}

/// The id of the node `frame` describes as `label`, for a test to act on
/// it as a screen reader would ([`AccessAction`](crate::AccessAction)).
pub fn node_named(frame: &Frame, label: &str) -> Option<FocusId> {
    let tree = access_tree(frame, "node named");
    tree.nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .map(|(id, _)| FocusId::from_raw(id.0))
}

/// The node `id` describes in `tree`, if `tree` has one.
pub fn access_node(tree: &TreeUpdate, id: FocusId) -> Option<&Node> {
    find_node(tree, id)
}

/// What `frame` describes to screen readers (see [`access_tree`]), written
/// as text: one line per node, indented two spaces a level under the window
/// node titled `title`, each saying the node's role, then its label and
/// value in quotes, its number, and the states it has (`checked`,
/// `unchecked`, `mixed`, `selected`, `expanded`, `collapsed`, `disabled`,
/// `modal`, `focused`). A test compares the whole text against what it
/// expects, so a change anywhere in the window shows in the diff:
///
/// ```text
/// Window "Notes"
///   Dialog "Unsaved changes" modal
///     Paragraph "Save notes.txt before closing?"
///     Button "Save" focused
/// ```
pub fn ui_text(frame: &Frame, title: &str) -> String {
    let tree = access_tree(frame, title);
    let mut text = String::new();
    if let Some(root) = tree.tree.as_ref().map(|info| info.root) {
        write_ui_node(&tree, root, 0, &mut text);
    }
    text
}

/// Writes node `id` of `tree` and the nodes under it to `text`, `depth`
/// levels in.
fn write_ui_node(tree: &TreeUpdate, id: NodeId, depth: usize, text: &mut String) {
    let Some((_, node)) = tree.nodes.iter().find(|(node_id, _)| *node_id == id) else {
        return;
    };
    text.push_str(&UI_TEXT_INDENT.repeat(depth));
    // Writing to a `String` never fails.
    let _ = write!(text, "{:?}", node.role());
    if let Some(label) = node.label() {
        let _ = write!(text, " {label:?}");
    }
    if let Some(value) = node.value() {
        let _ = write!(text, " value={value:?}");
    }
    if let Some(number) = node.numeric_value() {
        let _ = write!(text, " value={number}");
    }
    for state in ui_states(node, tree.focus == id) {
        text.push(' ');
        text.push_str(state);
    }
    text.push('\n');
    for child in node.children() {
        write_ui_node(tree, *child, depth + 1, text);
    }
}

/// The names of the states `node` has, `focused` among them when it has
/// the focus, in the order [`ui_text`] lists them.
fn ui_states(node: &Node, focused: bool) -> Vec<&'static str> {
    let toggled = node.toggled().map(|toggled| match toggled {
        Toggled::False => "unchecked",
        Toggled::Mixed => "mixed",
        Toggled::True => "checked",
    });
    let expanded = node
        .is_expanded()
        .map(|expanded| if expanded { "expanded" } else { "collapsed" });
    [
        toggled,
        node.is_selected().unwrap_or(false).then_some("selected"),
        expanded,
        node.is_disabled().then_some("disabled"),
        node.is_modal().then_some("modal"),
        focused.then_some("focused"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::Focusable;
    use crate::base::geometry::Rect;
    use crate::base::styled::StyledText;
    use crate::base::theme::{Theme, ThemeToken};
    use crate::input::event::{ButtonInput, PointerButton};
    use crate::render::paragraph::Paragraph;
    use crate::widgets::dialog::{Dialog, DialogButton};

    #[test]
    fn test_presses_sent_in_a_row_reach_widgets_as_double_and_triple_clicks() {
        let mut window = TestWindow::new(200, 100);
        let at = Point::new(10.0, 10.0);
        let press = Input::PointerButton(ButtonInput::press(PointerButton::Left, at));
        let release = Input::PointerButton(ButtonInput::release(PointerButton::Left, at));
        let counts: Vec<u8> = [&press, &release, &press, &release, &press]
            .into_iter()
            .map(|input| window.send(input, |_, cx| cx.click_count()))
            .collect();
        assert_eq!(counts, [1, 1, 2, 2, 3]);
    }

    #[test]
    fn test_drawn_text_reads_back_each_piece_in_the_order_drawn() {
        let window = TestWindow::new(200, 100);
        let mut frame = window.frame();
        let theme = Theme::dark();
        let bounds = frame.bounds();
        let line = |text: &str| StyledText::plain(text, ThemeToken::Foreground.into());
        frame.overlay(|frame| {
            frame.draw_text(&line("on top"), Point::new(0.0, 40.0), bounds, &theme);
        });
        frame.draw_text(&line("below"), Point::new(0.0, 0.0), bounds, &theme);
        assert_eq!(drawn_text(&frame), ["below", "on top"]);
    }

    #[test]
    fn test_a_control_with_no_name_is_called_out_and_a_named_one_is_not() {
        let window = TestWindow::new(400, 300);
        let mut frame = window.frame();
        let bounds = frame.bounds();
        let mut named = Node::new(accesskit::Role::Button);
        named.set_label("Save");
        frame.access(FocusId::new("named"), bounds, named);
        let mut bare = Node::new(accesskit::Role::GenericContainer);
        bare.add_action(Action::Click);
        frame.access(FocusId::new("bare"), bounds, bare);
        let unnamed = unnamed_controls(&frame);
        assert_eq!(unnamed.len(), 1, "{unnamed:?}");
        assert!(unnamed[0].starts_with("GenericContainer"), "{unnamed:?}");
    }

    #[test]
    fn test_ui_text_writes_the_tree_indented_with_values_states_and_focus() {
        let window = TestWindow::new(800, 600);
        let mut frame = window.frame();
        let id = FocusId::new("dialog");
        let mut dialog = Dialog::new(
            "Unsaved changes",
            vec![Paragraph::plain(
                "Save first?",
                ThemeToken::Foreground.into(),
            )],
        )
        .with_button(DialogButton::new(false, "Discard"))
        .with_button(DialogButton::new(true, "Save").primary());
        let bounds = frame.bounds();
        let rect = dialog.draw(&mut frame, bounds, &Theme::dark());
        dialog.access(&mut frame, id);
        let mut toggle = Node::new(accesskit::Role::CheckBox);
        toggle.set_label("Wrap");
        toggle.set_toggled(Toggled::True);
        toggle.set_disabled();
        frame.access(
            FocusId::new("wrap"),
            Rect::new(0.0, 0.0, 10.0, 10.0),
            toggle,
        );
        let focusable = Focusable {
            id,
            parent: None,
            rect,
        };
        frame.access.set_focus(vec![focusable], Some(id));
        assert_eq!(
            ui_text(&frame, "Notes"),
            "Window \"Notes\"\n\
             \x20 Dialog \"Unsaved changes\" modal\n\
             \x20   Paragraph \"Save first?\"\n\
             \x20   Button \"Discard\"\n\
             \x20   Button \"Save\" focused\n\
             \x20 CheckBox \"Wrap\" checked disabled\n"
        );
    }
}

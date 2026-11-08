//! [`Input`]: what the user did in a window, as apps and widgets see it.
//! The window runtime reads the window system's events once per window
//! and hands the app one [`Input`] for each that means something: the
//! pointer moving, leaving, pressing a button, or turning the wheel; a key
//! typed; text composed by an input method; the window gaining or losing
//! the keyboard; files dragged in from other apps; the user's appearance
//! settings changing; or a request to close.
//!
//! The runtime keeps what lasts between events, so no widget has to: the
//! pointer's position and the modifiers held ride along with each press
//! and wheel turn, and a key comes as a misty [`Key`] already normalized,
//! with releases, keys an input method is composing with, and presses the
//! window system made up (X11 replays held keys on focus) left out.
//! Positions are in physical pixels from the window's top left corner.
//!
//! Window-system events with no input meaning (a resize, a scale change,
//! a redraw) are the runtime's: it resizes the surface and redraws.

use std::path::{Path, PathBuf};

use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};

use super::super::base::appearance::Appearance;
use super::super::base::geometry::Point;
use super::super::base::key::{Key, Mods};
use super::winit_keys;

// ========================================================================
// Data Structures
// ========================================================================

/// One thing the user did in a window.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Input {
    /// The user changed their system's appearance settings (light or dark,
    /// the accent color, high contrast, less motion); this is the whole
    /// appearance now. Every open window gets it. Follow it with
    /// [`Appearance::theme`]; widgets follow its less motion on their own
    /// (see [`EventCx::reduced_motion`](crate::EventCx::reduced_motion)).
    AppearanceChanged(Appearance),
    /// The user asked to close the window (its close button, Alt+F4).
    /// Closing is up to the app.
    CloseRequested,
    /// A file dragged in from another app was dropped on the window.
    FileDropped(FileInput),
    /// A file dragged in from another app came over the window; one input
    /// for each file of the drag.
    FileHovered(FileInput),
    /// The files dragged over the window left it, or the drag was given up.
    FileHoverCancelled,
    /// The window gained the keyboard.
    FocusGained,
    /// The window lost the keyboard; the modifiers count as released.
    FocusLost,
    /// The input method composed or committed text.
    Ime(ImeInput),
    /// A key was pressed, or repeated while held
    /// ([`EventCx::is_key_repeat`](crate::EventCx::is_key_repeat) says
    /// which).
    Key(Key),
    /// A key was let go: its code as it was pressed, with the modifiers
    /// held as it was let go. Only a widget that sends every key on (a
    /// terminal reporting releases to the program in it, a game) wants
    /// these; a keymap leaves them.
    KeyReleased(Key),
    /// A pointer button was pressed or released.
    PointerButton(ButtonInput),
    /// The pointer left the window.
    PointerLeft,
    /// The pointer moved to this position in the window.
    PointerMoved(Point),
    /// The wheel or a touchpad scrolled.
    Wheel(WheelInput),
}

/// A press or release of a pointer button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonInput {
    /// Which button went down or up.
    pub button: PointerButton,
    /// The modifiers held.
    pub mods: Mods,
    /// Where the pointer was.
    pub position: Point,
    /// Whether the button went down (`true`) or up.
    pub pressed: bool,
}

/// A file dragged in from another app, and where the pointer was last seen
/// over the window (window systems report no position during such a drag,
/// and none at all when the pointer never moved over the window).
#[derive(Clone, Debug, PartialEq)]
pub struct FileInput {
    /// The file.
    pub path: PathBuf,
    /// Where the pointer was last seen in the window.
    pub position: Option<Point>,
}

/// What the input method did.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ImeInput {
    /// The composition ended in this text, to insert.
    Commit(String),
    /// The input method was turned off.
    Disabled,
    /// The input method was turned on.
    Enabled,
    /// The text being composed changed; empty when the composition was
    /// cleared.
    Preedit(Composition),
}

/// Text an input method is composing, not yet part of the text it goes
/// into.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composition {
    /// The input method's cursor in `text`, as a byte range, or `None`
    /// when it shows none.
    pub cursor: Option<(usize, usize)>,
    /// The text composed so far, shown at the cursor until committed.
    pub text: String,
}

/// A pointer button.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum PointerButton {
    /// The back button, on the side of some mice.
    Back,
    /// The forward button, on the side of some mice.
    Forward,
    /// The primary button.
    Left,
    /// The middle button, often the wheel pressed.
    Middle,
    /// Another button, by the window system's number.
    Other(u16),
    /// The secondary button, which opens context menus.
    Right,
}

/// How far a wheel or a touchpad scrolled.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum WheelDelta {
    /// Notches of a wheel, as lines: `x` across, `y` down the page;
    /// positive `y` scrolls toward the top.
    Lines(Point),
    /// Pixels of a touchpad or a smooth wheel, signed as for lines.
    Pixels(Point),
}

/// A turn of the wheel or a touchpad scroll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelInput {
    /// How far.
    pub delta: WheelDelta,
    /// The modifiers held.
    pub mods: Mods,
    /// Where the pointer was.
    pub position: Point,
}

/// Reads one window's events into inputs, keeping what lasts between them:
/// the pointer, the modifiers held, and whether the input method is
/// composing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct InputReader {
    /// Whether an input method is composing text, whose keys are its own.
    composing: bool,
    /// Where the pointer was last seen, even after it left, for a release
    /// that ends a drag outside the window.
    last_position: Point,
    /// The modifier keys held.
    mods: Mods,
    /// Where the pointer is, while it is over the window.
    pointer: Option<Point>,
    /// Whether the last key pressed is a repeat of a key held down.
    repeat: bool,
}

// ========================================================================
// Input Implementations
// ========================================================================

impl Input {
    /// Where the pointer was for an input about the pointer (a move, a
    /// button, the wheel, a file dragged in), or `None` for any other.
    pub fn position(&self) -> Option<Point> {
        match self {
            Self::FileDropped(file) | Self::FileHovered(file) => file.position,
            Self::PointerButton(button) => Some(button.position),
            Self::PointerMoved(position) => Some(*position),
            Self::Wheel(wheel) => Some(wheel.position),
            Self::AppearanceChanged(_)
            | Self::CloseRequested
            | Self::FileHoverCancelled
            | Self::FocusGained
            | Self::FocusLost
            | Self::Ime(_)
            | Self::Key(_)
            | Self::KeyReleased(_)
            | Self::PointerLeft => None,
        }
    }

    /// The key, when this is a key press.
    pub fn key(&self) -> Option<Key> {
        match self {
            Self::Key(key) => Some(*key),
            Self::KeyReleased(_)
            | Self::AppearanceChanged(_)
            | Self::CloseRequested
            | Self::FileDropped(_)
            | Self::FileHovered(_)
            | Self::FileHoverCancelled
            | Self::FocusGained
            | Self::FocusLost
            | Self::Ime(_)
            | Self::PointerButton(_)
            | Self::PointerLeft
            | Self::PointerMoved(_)
            | Self::Wheel(_) => None,
        }
    }

    /// The press, when this is a press of `button`.
    pub fn press(&self, button: PointerButton) -> Option<&ButtonInput> {
        match self {
            Self::PointerButton(input) if input.pressed && input.button == button => Some(input),
            Self::AppearanceChanged(_)
            | Self::CloseRequested
            | Self::FileDropped(_)
            | Self::FileHovered(_)
            | Self::FileHoverCancelled
            | Self::FocusGained
            | Self::FocusLost
            | Self::Ime(_)
            | Self::Key(_)
            | Self::KeyReleased(_)
            | Self::PointerButton(_)
            | Self::PointerLeft
            | Self::PointerMoved(_)
            | Self::Wheel(_) => None,
        }
    }

    /// The release, when this is a release of `button`.
    pub fn release(&self, button: PointerButton) -> Option<&ButtonInput> {
        match self {
            Self::PointerButton(input) if !input.pressed && input.button == button => Some(input),
            Self::AppearanceChanged(_)
            | Self::CloseRequested
            | Self::FileDropped(_)
            | Self::FileHovered(_)
            | Self::FileHoverCancelled
            | Self::FocusGained
            | Self::FocusLost
            | Self::Ime(_)
            | Self::Key(_)
            | Self::KeyReleased(_)
            | Self::PointerButton(_)
            | Self::PointerLeft
            | Self::PointerMoved(_)
            | Self::Wheel(_) => None,
        }
    }
}

// ========================================================================
// ButtonInput Implementations
// ========================================================================

impl ButtonInput {
    /// A press of `button` at `position` with no modifiers held.
    pub fn press(button: PointerButton, position: Point) -> Self {
        Self {
            button,
            mods: Mods::NONE,
            position,
            pressed: true,
        }
    }

    /// A release of `button` at `position` with no modifiers held.
    pub fn release(button: PointerButton, position: Point) -> Self {
        Self {
            pressed: false,
            ..Self::press(button, position)
        }
    }

    /// This input with `mods` held.
    pub fn with_mods(self, mods: Mods) -> Self {
        Self { mods, ..self }
    }
}

// ========================================================================
// WheelInput Implementations
// ========================================================================

impl WheelInput {
    /// A scroll by `delta` at `position` with no modifiers held.
    pub fn new(delta: WheelDelta, position: Point) -> Self {
        Self {
            delta,
            mods: Mods::NONE,
            position,
        }
    }

    /// This input with `mods` held.
    pub fn with_mods(self, mods: Mods) -> Self {
        Self { mods, ..self }
    }
}

// ========================================================================
// InputReader Implementations
// ========================================================================

impl InputReader {
    /// Where the pointer is, while it is over the window.
    pub(crate) fn pointer(&self) -> Option<Point> {
        self.pointer
    }

    /// The modifiers held.
    pub(crate) fn mods(&self) -> Mods {
        self.mods
    }

    /// Whether the last key pressed is a repeat of a key held down.
    pub(crate) fn key_repeat(&self) -> bool {
        self.repeat
    }

    /// Updates what lasts between events with `input`, an input made
    /// without a window system (by a test), as reading its event would.
    pub(crate) fn follow(&mut self, input: &Input) {
        match input {
            Input::FocusLost => {
                self.composing = false;
                self.mods = Mods::NONE;
            }
            Input::Ime(ImeInput::Commit(_) | ImeInput::Disabled) => self.composing = false,
            Input::Ime(ImeInput::Preedit(composition)) => {
                self.composing = !composition.text.is_empty();
            }
            Input::PointerButton(ButtonInput { mods, .. })
            | Input::Wheel(WheelInput { mods, .. }) => {
                self.mods = *mods;
            }
            Input::PointerLeft => self.pointer = None,
            Input::PointerMoved(position) => {
                self.pointer = Some(*position);
                self.last_position = *position;
            }
            Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(ImeInput::Enabled)
            | Input::Key(_)
            | Input::KeyReleased(_) => {}
        }
    }

    /// The input `event` means, or `None` when it means none (a key
    /// release, a modifier change, a resize). What lasts between events
    /// is updated either way.
    pub(crate) fn read(&mut self, event: &WindowEvent) -> Option<Input> {
        let input = match event {
            WindowEvent::CloseRequested => Input::CloseRequested,
            WindowEvent::CursorLeft { .. } => Input::PointerLeft,
            WindowEvent::CursorMoved { position, .. } => {
                Input::PointerMoved(Point::new(position.x as f32, position.y as f32))
            }
            WindowEvent::DroppedFile(path) => Input::FileDropped(self.file(path)),
            WindowEvent::Focused(true) => Input::FocusGained,
            WindowEvent::Focused(false) => Input::FocusLost,
            WindowEvent::HoveredFile(path) => Input::FileHovered(self.file(path)),
            WindowEvent::HoveredFileCancelled => Input::FileHoverCancelled,
            WindowEvent::Ime(ime) => Input::Ime(ime_of(ime)),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } if !(self.composing || *is_synthetic) => {
                if event.state == ElementState::Pressed {
                    self.repeat = event.repeat;
                }
                winit_keys::key_input(event, self.mods)?
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.mods = winit_keys::mods_of(modifiers.state());
                return None;
            }
            WindowEvent::MouseInput { button, state, .. } => Input::PointerButton(ButtonInput {
                button: button_of(*button),
                mods: self.mods,
                position: self.last_position,
                pressed: *state == ElementState::Pressed,
            }),
            WindowEvent::MouseWheel { delta, .. } => Input::Wheel(WheelInput {
                delta: wheel_of(*delta),
                mods: self.mods,
                position: self.last_position,
            }),
            _ => return None,
        };
        self.follow(&input);
        Some(input)
    }

    /// `path`, dragged in, with where the pointer was last seen.
    fn file(&self, path: &Path) -> FileInput {
        FileInput {
            path: path.to_path_buf(),
            position: self.pointer,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The misty button `button` is.
fn button_of(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Back => PointerButton::Back,
        MouseButton::Forward => PointerButton::Forward,
        MouseButton::Left => PointerButton::Left,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Other(number) => PointerButton::Other(number),
        MouseButton::Right => PointerButton::Right,
    }
}

/// The misty input method input `ime` is.
fn ime_of(ime: &Ime) -> ImeInput {
    match ime {
        Ime::Commit(text) => ImeInput::Commit(text.clone()),
        Ime::Disabled => ImeInput::Disabled,
        Ime::Enabled => ImeInput::Enabled,
        Ime::Preedit(text, cursor) => ImeInput::Preedit(Composition {
            cursor: *cursor,
            text: text.clone(),
        }),
    }
}

/// The misty wheel delta `delta` is.
fn wheel_of(delta: MouseScrollDelta) -> WheelDelta {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => WheelDelta::Lines(Point::new(x, y)),
        MouseScrollDelta::PixelDelta(position) => {
            WheelDelta::Pixels(Point::new(position.x as f32, position.y as f32))
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;
    use winit::event::{DeviceId, Modifiers};
    use winit::keyboard::ModifiersState;

    use super::*;

    fn moved(x: f64, y: f64) -> WindowEvent {
        WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: PhysicalPosition::new(x, y),
        }
    }

    fn pressed() -> WindowEvent {
        WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Pressed,
            button: MouseButton::Left,
        }
    }

    fn preedit(text: &str) -> WindowEvent {
        WindowEvent::Ime(Ime::Preedit(text.to_owned(), None))
    }

    #[test]
    fn test_a_press_carries_the_pointer_and_the_modifiers_held() {
        let mut reader = InputReader::default();
        reader.read(&moved(12.0, 34.0));
        let ctrl = Modifiers::from(ModifiersState::CONTROL);
        assert_eq!(reader.read(&WindowEvent::ModifiersChanged(ctrl)), None);
        let press = ButtonInput::press(PointerButton::Left, Point::new(12.0, 34.0));
        assert_eq!(
            reader.read(&pressed()),
            Some(Input::PointerButton(press.with_mods(Mods::CTRL)))
        );
    }

    #[test]
    fn test_a_release_after_the_pointer_left_is_where_it_was_last_seen() {
        let mut reader = InputReader::default();
        reader.read(&moved(5.0, 6.0));
        reader.read(&WindowEvent::CursorLeft {
            device_id: DeviceId::dummy(),
        });
        assert_eq!(reader.pointer, None);
        let input = reader.read(&pressed()).expect("a press");
        assert_eq!(input.position(), Some(Point::new(5.0, 6.0)));
    }

    #[test]
    fn test_a_file_dragged_in_is_where_the_pointer_was_last_seen_over_the_window() {
        let mut reader = InputReader::default();
        let path = PathBuf::from("/tmp/a.png");
        let unseen = reader.read(&WindowEvent::HoveredFile(path.clone()));
        assert_eq!(unseen.and_then(|input| input.position()), None);
        reader.read(&moved(7.0, 8.0));
        let seen = reader.read(&WindowEvent::DroppedFile(path));
        assert_eq!(
            seen.and_then(|input| input.position()),
            Some(Point::new(7.0, 8.0))
        );
    }

    #[test]
    fn test_composition_holds_keys_until_committed_or_cleared() {
        let mut reader = InputReader::default();
        reader.read(&preedit("k"));
        assert!(reader.composing);
        reader.read(&preedit(""));
        assert!(!reader.composing, "an empty preedit ends the composition");
        reader.read(&preedit("か"));
        reader.read(&WindowEvent::Ime(Ime::Commit("か".to_owned())));
        assert!(!reader.composing);
    }

    #[test]
    fn test_losing_focus_forgets_held_modifiers() {
        let mut reader = InputReader::default();
        let held = Modifiers::from(ModifiersState::CONTROL);
        reader.read(&WindowEvent::ModifiersChanged(held));
        assert_eq!(reader.mods, Mods::CTRL);
        assert_eq!(
            reader.read(&WindowEvent::Focused(false)),
            Some(Input::FocusLost)
        );
        assert_eq!(reader.mods, Mods::NONE, "Ctrl was released elsewhere");
    }
}

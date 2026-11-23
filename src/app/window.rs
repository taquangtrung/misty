//! How a window is opened: [`WindowOptions`] and the choices it holds,
//! among them the [`WindowGeometry`] an app saved when the window last
//! closed, to open it where the user left it.
//!
//! A popup window, such as a dictionary's lookup popup over another app,
//! is a window with [`Decorations::Frameless`], [`WindowLevel::AlwaysOnTop`],
//! no task bar entry ([`WindowOptions::with_taskbar`]), and a position
//! ([`WindowOptions::with_position`]), hidden again by the app on
//! [`Input::FocusLost`](crate::Input::FocusLost) with
//! [`EventCx::set_visible`](crate::EventCx::set_visible).
//! What each window system allows:
//!
//! - Position: X11, Windows, and macOS place a window where it asks, and
//!   [`EventCx::move_window`](crate::EventCx::move_window) moves it.
//!   Wayland lets no app place its own top-level windows: the compositor
//!   does, and both are ignored there.
//! - The pointer: no window system tells misty where the pointer is
//!   outside its own windows. Inside one, an input's position plus the
//!   window's [`WindowGeometry::position`] is the pointer on the desktop
//!   (not on Wayland, which does not say where windows are).
//! - Always on top: X11, Windows, and macOS; on Wayland only where the
//!   compositor offers it.
//! - No task bar entry: Windows; on X11 the window is marked a utility
//!   window, which most window managers keep off the task bar; macOS shows
//!   apps in the Dock, never windows, and Wayland has no such request.
//! - Opening without the keyboard ([`WindowOptions::with_active`]):
//!   Windows and macOS through the system's own request; X11 through the
//!   EWMH hint that a window wants no focus when first shown, which window
//!   managers that follow EWMH honor; Wayland has no such request, and the
//!   compositor decides (misty passes it no activation token, which a
//!   compositor that prevents focus stealing takes as not asking).
//! - Transparency: where the GPU surface composites with alpha (most
//!   Wayland and macOS setups, Windows with a compositing surface); on
//!   others the window stays opaque.

use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use thiserror::Error;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::ActiveEventLoop;
#[cfg(target_os = "macos")]
use winit::platform::macos::{OptionAsAlt as WinitOptionAsAlt, WindowAttributesExtMacOS};
#[cfg(target_os = "windows")]
use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use winit::platform::{
    wayland::WindowAttributesExtWayland,
    x11::{WindowAttributesExtX11, WindowType},
};
use winit::window::{Icon, WindowAttributes, WindowLevel as WinitLevel};

use super::super::base::geometry::{Point, Rect};
use super::super::render::DEFAULT_BUDGET_BYTES;
use super::super::render::font::Font;
use super::super::render::image::Image;

// ========================================================================
// Constants
// ========================================================================

/// The radius a frameless window's corners are rounded by unless
/// [`WindowOptions::with_rounded_corners`] or
/// [`WindowOptions::with_square_corners`] says otherwise, in logical pixels.
const DEFAULT_CORNER_RADIUS: u16 = 8;

/// The size a window opens at unless [`WindowOptions::with_size`] says
/// otherwise, in logical pixels.
const DEFAULT_SIZE: Size = Size::new(960, 600);

/// The word that ends the text form of a maximized window's geometry.
const MAXIMIZED_WORD: &str = "maximized";

/// What separates a position's two numbers in the text form.
const POSITION_SEPARATOR: char = ',';

/// What separates a size's width and height in the text form.
const SIZE_SEPARATOR: char = 'x';

// ========================================================================
// Data Structures
// ========================================================================

/// How to open a window. Start from [`WindowOptions::new`] and change what
/// you need with the `with_` methods.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowOptions {
    /// Whether the window takes the keyboard and comes to the front when
    /// it opens.
    active: bool,
    /// The name the desktop knows the app by (its desktop entry's), or
    /// `None` for winit's default.
    app_id: Option<String>,
    /// Whether the system draws the title bar and border.
    decorations: Decorations,
    /// The font text is drawn in.
    font: Font,
    /// The picture the system shows for the window, or `None` for its
    /// default.
    icon: Option<Image>,
    /// Whether the window stays above others.
    level: WindowLevel,
    /// Which Option keys are Alt on macOS rather than typing characters.
    option_as_alt: OptionAsAlt,
    /// Whether the window opens maximized.
    maximized: bool,
    /// The smallest size the user may resize to, or `None` for no limit.
    min_size: Option<Size>,
    /// Where the window opens, or `None` to let the system place it.
    position: Option<Position>,
    /// The radius a frameless window's corners are rounded by, in logical
    /// pixels, or `None` for square corners.
    rounded_corners: Option<u16>,
    /// When the window becomes visible.
    show: ShowPolicy,
    /// The size the window opens at.
    size: Size,
    /// Whether the window has an entry in the task bar.
    taskbar: bool,
    /// The most bytes of picture textures the window keeps on the GPU.
    texture_budget: usize,
    /// The title the system shows in its title bar, task bar, and switcher.
    title: String,
    /// Whether what the app leaves transparent shows the desktop behind.
    transparent: bool,
}

/// Which Option keys on a Mac keyboard are Alt, so a chord with them
/// arrives as `M-` rather than as the character macOS types with Option.
/// Other systems have no Option key and ignore it.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum OptionAsAlt {
    /// Both Option keys.
    Both,
    /// The left Option key; the right one types characters.
    Left,
    /// Neither: Option types characters, as macOS has it (the default).
    #[default]
    None,
    /// The right Option key; the left one types characters.
    Right,
}

/// A window's inner size (its content, without the system frame) in logical
/// pixels, which the system scales by the display's scale factor.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Size {
    /// The height in logical pixels.
    pub height: u32,
    /// The width in logical pixels.
    pub width: u32,
}

/// Where a window's outer top left corner is (its system frame included,
/// if any) on the desktop, in logical pixels: `x` from the left, `y` from
/// the top, negative on a display left of or above the primary one.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Position {
    /// Logical pixels from the left of the desktop.
    pub x: i32,
    /// Logical pixels from the top of the desktop.
    pub y: i32,
}

/// Where a window is and how big, to save when it closes and hand back to
/// [`WindowOptions::with_geometry`] when it opens again. Read it with
/// [`EventCx::geometry`](crate::EventCx::geometry).
/// While a window is maximized, its position and size are those it goes
/// back to when restored.
///
/// It reads and writes a plain text form for an app's settings file: the
/// position, the size, and `maximized` when it is, such as
/// `120,80 1280x720` or `120,80 1280x720 maximized`; just `1280x720` when
/// the window system does not tell windows where they are (Wayland).
///
/// ```
/// use misty::WindowGeometry;
///
/// let geometry: WindowGeometry = "120,80 1280x720 maximized".parse().unwrap();
/// assert!(geometry.maximized);
/// assert_eq!(geometry.to_string(), "120,80 1280x720 maximized");
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WindowGeometry {
    /// Whether the window is maximized.
    pub maximized: bool,
    /// Where the window is, or `None` where the window system does not
    /// say.
    pub position: Option<Position>,
    /// The window's inner size.
    pub size: Size,
}

/// Why text is not a [`WindowGeometry`].
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum GeometryError {
    /// The text is not in the geometry's form.
    #[error(
        "`{0}` is not a window geometry: write `X,Y WIDTHxHEIGHT`, then `maximized` if it is, \
         such as `120,80 1280x720` or `1280x720 maximized` (the position may be left out)"
    )]
    Malformed(String),
    /// The size has no width or no height.
    #[error("`{0}` is no window size: a window is at least 1x1, such as `1280x720`")]
    ZeroSize(String),
}

/// A display's area on the desktop in physical pixels and its scale factor,
/// to tell whether a saved position is still on one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Monitor {
    /// The display's area.
    pub(crate) area: Rect,
    /// Its physical pixels per logical pixel.
    pub(crate) scale_factor: f64,
}

/// Who draws the frame around a window.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Decorations {
    /// No system title bar or border: the app draws its own chrome. On
    /// Windows the system drop shadow and thin edge are kept.
    Frameless,
    /// The system's own title bar and border.
    #[default]
    System,
}

/// Whether a window stays above the others.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum WindowLevel {
    /// Above every window that is not on top itself, as a popup or a
    /// floating tool is. Not every Wayland compositor allows it.
    AlwaysOnTop,
    /// Stacked with the others, raised when the user picks it.
    #[default]
    Normal,
}

/// When a new window becomes visible.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ShowPolicy {
    /// Hidden until its first frame has been presented, so it never shows an
    /// empty or garbage surface. On Wayland, where a window cannot be hidden,
    /// it appears with its first frame anyway.
    #[default]
    AfterFirstFrame,
    /// Visible as soon as it is created, before anything is drawn.
    Immediately,
}

// ========================================================================
// WindowOptions Implementations
// ========================================================================

impl WindowOptions {
    /// A window titled `title`, at a default size, with system decorations,
    /// drawing text in the desktop's size ([`Font::desktop`]), shown after
    /// its first frame.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            active: true,
            app_id: None,
            decorations: Decorations::default(),
            font: Font::desktop(),
            icon: None,
            level: WindowLevel::default(),
            maximized: false,
            option_as_alt: OptionAsAlt::default(),
            min_size: None,
            position: None,
            show: ShowPolicy::default(),
            size: DEFAULT_SIZE,
            taskbar: true,
            texture_budget: DEFAULT_BUDGET_BYTES,
            title: title.into(),
            transparent: false,
            rounded_corners: Some(DEFAULT_CORNER_RADIUS),
        }
    }

    /// These options with the Option keys `option_as_alt` names acting as
    /// Alt on macOS.
    pub fn with_option_as_alt(self, option_as_alt: OptionAsAlt) -> Self {
        Self {
            option_as_alt,
            ..self
        }
    }

    /// These options with the frame drawn as `decorations` says.
    pub fn with_decorations(self, decorations: Decorations) -> Self {
        Self {
            decorations,
            ..self
        }
    }

    /// These options with text drawn in `font`.
    pub fn with_font(self, font: Font) -> Self {
        Self { font, ..self }
    }

    /// These options with `icon` as the picture the system shows for the
    /// window, in its title bar, task bar, and switcher, on X11 and
    /// Windows; Wayland and macOS take an app's icon from its desktop entry
    /// or bundle instead. A square of 32 to 256 pixels a side suits every
    /// system. [`EventCx::set_icon`](crate::EventCx::set_icon)
    /// changes it later.
    pub fn with_icon(self, icon: Image) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// These options naming the app to the desktop as `app_id`, the name of
    /// its desktop entry (`StartupWMClass`), so the desktop groups its
    /// windows under that entry's icon and name: the Wayland app id and
    /// the X11 window class. Other systems take it from the bundle or the
    /// executable and ignore it.
    pub fn with_app_id(self, app_id: impl Into<String>) -> Self {
        Self {
            app_id: Some(app_id.into()),
            ..self
        }
    }

    /// These options with the window opening where `geometry` says, as an
    /// app saved it when the window last closed: at its size, maximized if
    /// it was, and at its position while that is still on a display (the
    /// system places a window saved on a display since unplugged).
    pub fn with_geometry(self, geometry: WindowGeometry) -> Self {
        Self {
            maximized: geometry.maximized,
            position: geometry.position,
            size: geometry.size,
            ..self
        }
    }

    /// These options with the window stacked as `level` says.
    pub fn with_level(self, level: WindowLevel) -> Self {
        Self { level, ..self }
    }

    /// These options with the window's outer top left corner at
    /// `position` on the desktop, where the window system lets apps place
    /// windows (not Wayland) and while a display holds it.
    pub fn with_position(self, position: Position) -> Self {
        Self {
            position: Some(position),
            ..self
        }
    }

    /// These options with the window taking the keyboard as it opens
    /// (`true`, the default) or opening without it, behind the window the
    /// user is in, as test and benchmark runs open theirs so they never
    /// pull the focus away. See the module docs for which window systems
    /// honor it. The window still takes the keyboard when the user clicks
    /// it, or the app asks with
    /// [`EventCx::focus_window`](crate::EventCx::focus_window).
    pub fn with_active(self, active: bool) -> Self {
        Self { active, ..self }
    }

    /// Whether the window takes the keyboard as it opens.
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// These options with the window listed in the task bar (`true`, the
    /// default) or left off it, as a popup is. See the module docs for
    /// which window systems honor it.
    pub fn with_taskbar(self, taskbar: bool) -> Self {
        Self { taskbar, ..self }
    }

    /// These options keeping at most `bytes` of picture textures (images,
    /// SVG documents, and the pages or covers an app draws as images) on
    /// the GPU for the window, rather than 256 MiB: more for an app that
    /// pages back and forth through large pictures, less for a small tool
    /// window. Past the budget the textures drawn least recently are let
    /// go and uploaded again when next drawn; a frame that needs more than
    /// the budget keeps all it draws.
    pub fn with_texture_budget(self, bytes: usize) -> Self {
        Self {
            texture_budget: bytes,
            ..self
        }
    }

    /// These options with a frameless window's corners rounded, as modern
    /// desktops round their windows: by the system itself on macOS (the
    /// window keeps a native frame whose title bar is hidden under the
    /// app's own, so its corners and shadow are the system's) and on
    /// Windows 11 (which rounds by its own radius), and by `radius` logical
    /// pixels elsewhere. Windows 10 and a maximized window keep square
    /// corners. Only a [`Decorations::Frameless`] window is rounded; a
    /// system frame rounds itself. A frameless window is rounded by 8
    /// logical pixels unless this or [`WindowOptions::with_square_corners`]
    /// says otherwise.
    pub fn with_rounded_corners(self, radius: u16) -> Self {
        Self {
            rounded_corners: Some(radius),
            ..self
        }
    }

    /// These options with a frameless window's corners left square, so
    /// misty needs no transparency to cut them out.
    pub fn with_square_corners(self) -> Self {
        Self {
            rounded_corners: None,
            ..self
        }
    }

    /// These options with what the app leaves transparent showing the
    /// desktop behind (`true`): the color given to
    /// [`Frame::clear`](crate::Frame::clear) and anything drawn over it
    /// keep their alpha. Where the GPU surface cannot composite with
    /// alpha, the window stays opaque.
    pub fn with_transparent(self, transparent: bool) -> Self {
        Self {
            transparent,
            ..self
        }
    }

    /// These options with the window resizable down to `min_size` and no
    /// smaller.
    pub fn with_min_size(self, min_size: Size) -> Self {
        Self {
            min_size: Some(min_size),
            ..self
        }
    }

    /// These options with the window shown as `show` says.
    pub fn with_show(self, show: ShowPolicy) -> Self {
        Self { show, ..self }
    }

    /// These options with the window opening at `size`.
    pub fn with_size(self, size: Size) -> Self {
        Self { size, ..self }
    }

    /// The most bytes of picture textures the window keeps on the GPU.
    pub(crate) fn texture_budget(&self) -> usize {
        self.texture_budget
    }

    /// The font text is drawn in.
    pub(crate) fn font(&self) -> &Font {
        &self.font
    }

    /// The window's title.
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// When the window becomes visible.
    pub(crate) fn show(&self) -> ShowPolicy {
        self.show
    }

    /// The size the window opens at.
    pub(crate) fn size(&self) -> Size {
        self.size
    }

    /// Whether what the app leaves transparent shows the desktop behind:
    /// asked for, or needed for the corners misty rounds itself.
    pub(crate) fn transparent(&self) -> bool {
        self.transparent || self.drawn_corner_radius().is_some()
    }

    /// The attributes winit opens this window with: always hidden, since
    /// the accessibility adapter must be made before a window first shows;
    /// the runtime shows it as [`WindowOptions::show`] says. A position is
    /// kept only while it lies on one of `monitors`.
    /// The radius misty rounds the window's corners by itself, in logical
    /// pixels: a frameless window's, on the platforms whose systems do not
    /// round it; `None` for square corners or a system that rounds them.
    pub(crate) fn drawn_corner_radius(&self) -> Option<u16> {
        let frameless = matches!(self.decorations, Decorations::Frameless);
        let system_rounds = cfg!(any(target_os = "macos", target_os = "windows"));
        self.rounded_corners.filter(|_| frameless && !system_rounds)
    }

    pub(crate) fn window_attributes(&self, monitors: &[Monitor]) -> WindowAttributes {
        let system_frame = match self.decorations {
            Decorations::Frameless => false,
            Decorations::System => true,
        };
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let rounded = !system_frame && self.rounded_corners.is_some();
        let level = match self.level {
            WindowLevel::AlwaysOnTop => WinitLevel::AlwaysOnTop,
            WindowLevel::Normal => WinitLevel::Normal,
        };
        let attributes = WindowAttributes::default()
            .with_active(self.active)
            .with_decorations(system_frame)
            .with_inner_size(self.size.to_logical())
            .with_maximized(self.maximized)
            .with_title(self.title.as_str())
            .with_transparent(self.transparent())
            .with_window_icon(self.icon.as_ref().and_then(window_icon))
            .with_visible(false)
            .with_window_level(level);
        let attributes = match self.position.filter(|position| position.is_on(monitors)) {
            Some(position) => {
                attributes.with_position(LogicalPosition::new(position.x, position.y))
            }
            None => attributes,
        };
        #[cfg(target_os = "macos")]
        let attributes = attributes.with_option_as_alt(match self.option_as_alt {
            OptionAsAlt::Both => WinitOptionAsAlt::Both,
            OptionAsAlt::Left => WinitOptionAsAlt::OnlyLeft,
            OptionAsAlt::None => WinitOptionAsAlt::None,
            OptionAsAlt::Right => WinitOptionAsAlt::OnlyRight,
        });
        // A rounded frameless window keeps the system's frame, its title bar
        // see-through and hidden with its buttons under the app's own, so the
        // system rounds its corners and casts its shadow.
        #[cfg(target_os = "macos")]
        let attributes = match rounded {
            true => attributes
                .with_decorations(true)
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_titlebar_buttons_hidden(true)
                .with_fullsize_content_view(true),
            false => attributes,
        };
        // Windows 11 rounds a frameless window when asked; Windows 10 ignores it.
        #[cfg(target_os = "windows")]
        let attributes = match rounded {
            true => attributes.with_corner_preference(CornerPreference::Round),
            false => attributes,
        };
        // Without a system frame, Windows also drops the drop shadow and edge,
        // leaving a flat rectangle that blends into what is behind it.
        #[cfg(target_os = "windows")]
        let attributes = attributes
            .with_undecorated_shadow(!system_frame)
            .with_skip_taskbar(!self.taskbar);
        // X11 has no request to skip the task bar that winit makes, but
        // window managers keep utility windows off it.
        #[cfg(all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        ))]
        let attributes = match self.taskbar {
            true => attributes,
            false => attributes.with_x11_window_type(vec![WindowType::Utility]),
        };
        #[cfg(all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        ))]
        let attributes = match &self.app_id {
            Some(app_id) => {
                let attributes = WindowAttributesExtX11::with_name(attributes, app_id, app_id);
                WindowAttributesExtWayland::with_name(attributes, app_id, app_id)
            }
            None => attributes,
        };
        match self.min_size {
            Some(min_size) => attributes.with_min_inner_size(min_size.to_logical()),
            None => attributes,
        }
    }
}

// ========================================================================
// Position Implementations
// ========================================================================

impl Position {
    /// The point `x`, `y` logical pixels from the desktop's top left.
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Whether this position lies on one of `monitors`, each scaling it by
    /// its own factor.
    fn is_on(self, monitors: &[Monitor]) -> bool {
        monitors.iter().any(|monitor| {
            let scale = monitor.scale_factor as f32;
            let point = Point::new(self.x as f32 * scale, self.y as f32 * scale);
            monitor.area.contains(point)
        })
    }
}

// ========================================================================
// Size Implementations
// ========================================================================

impl Size {
    /// A size `width` by `height` logical pixels.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { height, width }
    }

    /// This size as winit takes it.
    pub(crate) fn to_logical(self) -> LogicalSize<u32> {
        LogicalSize::new(self.width, self.height)
    }
}

// ========================================================================
// WindowGeometry Implementations
// ========================================================================

impl Display for WindowGeometry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(position) = self.position {
            write!(f, "{}{POSITION_SEPARATOR}{} ", position.x, position.y)?;
        }
        write!(f, "{}{SIZE_SEPARATOR}{}", self.size.width, self.size.height)?;
        if self.maximized {
            write!(f, " {MAXIMIZED_WORD}")?;
        }
        Ok(())
    }
}

impl FromStr for WindowGeometry {
    type Err = GeometryError;

    /// Reads the text form [`WindowGeometry`] describes, spaces around it
    /// allowed.
    fn from_str(text: &str) -> Result<Self, GeometryError> {
        let malformed = || GeometryError::Malformed(text.to_owned());
        let mut words: Vec<&str> = text.split_whitespace().collect();
        let maximized = words.last() == Some(&MAXIMIZED_WORD);
        if maximized {
            words.pop();
        }
        let (position, size) = match words.as_slice() {
            [size] => (None, *size),
            [position, size] => (Some(*position), *size),
            _ => return Err(malformed()),
        };
        let position = match position {
            Some(position) => {
                let (x, y) = position
                    .split_once(POSITION_SEPARATOR)
                    .ok_or_else(malformed)?;
                let x = x.parse().map_err(|_| malformed())?;
                let y = y.parse().map_err(|_| malformed())?;
                Some(Position::new(x, y))
            }
            None => None,
        };
        let (width, height) = size.split_once(SIZE_SEPARATOR).ok_or_else(malformed)?;
        let width: u32 = width.parse().map_err(|_| malformed())?;
        let height: u32 = height.parse().map_err(|_| malformed())?;
        if width == 0 || height == 0 {
            return Err(GeometryError::ZeroSize(size.to_owned()));
        }
        Ok(Self {
            maximized,
            position,
            size: Size::new(width, height),
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The displays `event_loop` knows of now.
pub(crate) fn monitors(event_loop: &ActiveEventLoop) -> Vec<Monitor> {
    event_loop
        .available_monitors()
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            Monitor {
                area: Rect::new(
                    position.x as f32,
                    position.y as f32,
                    size.width as f32,
                    size.height as f32,
                ),
                scale_factor: monitor.scale_factor(),
            }
        })
        .collect()
}

/// `image` as the window system takes an icon, or `None`, after saying
/// why, when it refuses the pixels.
pub(crate) fn window_icon(image: &Image) -> Option<Icon> {
    Icon::from_rgba(image.pixels().to_vec(), image.width(), image.height())
        .map_err(|error| log::warn!("the window icon was not set: {error}"))
        .ok()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_texture_budget_is_256_mib_unless_set() {
        assert_eq!(WindowOptions::new("t").texture_budget(), 256 * 1024 * 1024);
        let options = WindowOptions::new("t").with_texture_budget(1 << 30);
        assert_eq!(options.texture_budget(), 1 << 30);
    }

    #[test]
    fn test_default_window_is_hidden_until_drawn_and_system_framed() {
        let attributes = WindowOptions::new("t").window_attributes(&[]);
        assert!(!attributes.visible, "shown only after the first frame");
        assert!(attributes.decorations);
    }

    #[test]
    fn test_options_map_to_frameless_and_open_hidden_even_when_shown_at_once() {
        let attributes = WindowOptions::new("t")
            .with_decorations(Decorations::Frameless)
            .with_show(ShowPolicy::Immediately)
            .window_attributes(&[]);
        assert!(
            !attributes.visible,
            "the runtime shows it after making its accessibility adapter"
        );
        assert!(!attributes.decorations);
    }

    #[test]
    fn test_an_icon_reaches_the_window_system_and_none_is_its_default() {
        assert!(
            WindowOptions::new("t")
                .window_attributes(&[])
                .window_icon
                .is_none()
        );
        let icon = Image::from_rgba(2, 2, vec![u8::MAX; 16]).expect("an image");
        let attributes = WindowOptions::new("t")
            .with_icon(icon)
            .window_attributes(&[]);
        assert!(attributes.window_icon.is_some());
    }

    #[test]
    fn test_popup_options_reach_the_window_system() {
        let attributes = WindowOptions::new("t")
            .with_level(WindowLevel::AlwaysOnTop)
            .with_transparent(true)
            .with_position(Position::new(100, 100))
            .window_attributes(&[Monitor {
                area: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                scale_factor: 1.0,
            }]);
        assert_eq!(attributes.window_level, WinitLevel::AlwaysOnTop);
        assert!(attributes.transparent);
        assert!(attributes.position.is_some());
        let plain = WindowOptions::new("t").window_attributes(&[]);
        assert_eq!(plain.window_level, WinitLevel::Normal);
        assert!(!plain.transparent);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn test_a_frameless_window_is_rounded_unless_asked_for_square_corners() {
        let frameless = WindowOptions::new("t").with_decorations(Decorations::Frameless);
        assert_eq!(frameless.drawn_corner_radius(), Some(DEFAULT_CORNER_RADIUS));
        assert!(frameless.transparent(), "to cut the corners out");
        let square = frameless.with_square_corners();
        assert_eq!(square.drawn_corner_radius(), None);
        assert!(!square.transparent());
        assert_eq!(WindowOptions::new("t").drawn_corner_radius(), None);
    }

    #[test]
    fn test_an_app_id_is_kept_for_the_desktop_and_none_by_default() {
        let named = WindowOptions::new("t").with_app_id("frontier");
        assert_eq!(named.app_id.as_deref(), Some("frontier"));
        assert_eq!(WindowOptions::new("t").app_id, None);
        let _ = named.window_attributes(&[]);
    }

    #[test]
    fn test_a_window_opened_inactive_asks_for_no_focus() {
        let options = WindowOptions::new("t").with_active(false);
        assert!(!options.window_attributes(&[]).active);
        assert!(!options.is_active());
        assert!(WindowOptions::new("t").window_attributes(&[]).active);
    }

    #[test]
    fn test_geometry_text_round_trips_with_and_without_position_and_maximized() {
        for text in ["120,80 1280x720", "-1920,0 800x600 maximized", "1280x720"] {
            let geometry: WindowGeometry = text.parse().expect(text);
            assert_eq!(geometry.to_string(), text);
        }
        let spaced: WindowGeometry = "  10,20   300x200  maximized ".parse().expect("spaced");
        assert_eq!(
            spaced,
            WindowGeometry {
                maximized: true,
                position: Some(Position::new(10, 20)),
                size: Size::new(300, 200),
            }
        );
    }

    #[test]
    fn test_bad_geometry_text_says_which_form_to_write() {
        for text in [
            "",
            "maximized",
            "1280",
            "a,b 10x10",
            "10,20 30x40 50x60",
            "10 20x30",
        ] {
            assert_eq!(
                text.parse::<WindowGeometry>(),
                Err(GeometryError::Malformed(text.to_owned())),
                "{text:?}"
            );
        }
        let error = "0x720".parse::<WindowGeometry>().expect_err("no width");
        assert_eq!(error, GeometryError::ZeroSize("0x720".to_owned()));
        let message = "1280"
            .parse::<WindowGeometry>()
            .expect_err("bad")
            .to_string();
        assert!(message.contains("`120,80 1280x720`"), "{message}");
    }

    #[test]
    fn test_a_saved_position_is_kept_only_while_a_display_holds_it() {
        let monitors = [
            Monitor {
                area: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                scale_factor: 1.0,
            },
            Monitor {
                area: Rect::new(1920.0, 0.0, 3840.0, 2160.0),
                scale_factor: 2.0,
            },
        ];
        assert!(Position::new(100, 100).is_on(&monitors));
        assert!(
            Position::new(1500, 500).is_on(&monitors),
            "on the second display at twice the scale"
        );
        assert!(
            !Position::new(-500, 100).is_on(&monitors),
            "a display since unplugged"
        );
        let geometry = WindowGeometry {
            maximized: true,
            position: Some(Position::new(-500, 100)),
            size: Size::new(800, 600),
        };
        let attributes = WindowOptions::new("t")
            .with_geometry(geometry)
            .window_attributes(&monitors);
        assert_eq!(attributes.position, None, "left to the system");
        assert!(attributes.maximized);
        let kept = WindowOptions::new("t")
            .with_geometry(WindowGeometry {
                position: Some(Position::new(100, 100)),
                ..geometry
            })
            .window_attributes(&monitors);
        assert!(kept.position.is_some());
    }
}

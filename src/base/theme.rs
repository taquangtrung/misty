//! A [`Theme`]: the named colors widgets and chrome are drawn in (each
//! named by a [`ThemeToken`]), the [`Palette`] cell colors resolve
//! against, the [`SyntaxColors`] of highlighted source text, the
//! [`Shape`] of its boxes, the font its chrome is set in
//! ([`ChromeText`]), and the words its widgets show ([`Strings`]). Two are
//! built in,
//! [`Theme::dark`] and [`Theme::light`]; with the `kdl` feature, a theme
//! file can lay its own colors and shape over either.

use std::f32::consts::SQRT_2;

use super::box_style::{Border, BoxStyle, Shadow};
use super::cell::CellColor;
use super::color::{Rgb, Rgba};
use super::geometry::Point;
use super::motion::Motion;
use super::palette::{ANSI_COLORS, Palette};
use super::strings::Strings;
use super::syntax::SyntaxColors;

// ========================================================================
// Constants
// ========================================================================

/// The dark theme's ANSI colors, normal then bright: black, red, green,
/// yellow, blue, magenta, cyan, white.
const DARK_ANSI: [Rgb; ANSI_COLORS] = [
    Rgb::new(0x45, 0x47, 0x5a),
    Rgb::new(0xf3, 0x8b, 0xa8),
    Rgb::new(0xa6, 0xe3, 0xa1),
    Rgb::new(0xf9, 0xe2, 0xaf),
    Rgb::new(0x89, 0xb4, 0xfa),
    Rgb::new(0xf5, 0xc2, 0xe7),
    Rgb::new(0x94, 0xe2, 0xd5),
    Rgb::new(0xba, 0xc2, 0xde),
    Rgb::new(0x58, 0x5b, 0x70),
    Rgb::new(0xf3, 0x8b, 0xa8),
    Rgb::new(0xa6, 0xe3, 0xa1),
    Rgb::new(0xf9, 0xe2, 0xaf),
    Rgb::new(0x89, 0xb4, 0xfa),
    Rgb::new(0xf5, 0xc2, 0xe7),
    Rgb::new(0x94, 0xe2, 0xd5),
    Rgb::new(0xa6, 0xad, 0xc8),
];

/// The light theme's ANSI colors, in the same order.
const LIGHT_ANSI: [Rgb; ANSI_COLORS] = [
    Rgb::new(0x5c, 0x5f, 0x77),
    Rgb::new(0xd2, 0x0f, 0x39),
    Rgb::new(0x40, 0xa0, 0x2b),
    Rgb::new(0xdf, 0x8e, 0x1d),
    Rgb::new(0x1e, 0x66, 0xf5),
    Rgb::new(0xea, 0x76, 0xcb),
    Rgb::new(0x17, 0x92, 0x99),
    Rgb::new(0xac, 0xb0, 0xbe),
    Rgb::new(0x6c, 0x6f, 0x85),
    Rgb::new(0xd2, 0x0f, 0x39),
    Rgb::new(0x40, 0xa0, 0x2b),
    Rgb::new(0xdf, 0x8e, 0x1d),
    Rgb::new(0x1e, 0x66, 0xf5),
    Rgb::new(0xea, 0x76, 0xcb),
    Rgb::new(0x17, 0x92, 0x99),
    Rgb::new(0xbc, 0xc0, 0xcc),
];

/// The dark theme's background, shared by the theme and its palette.
const DARK_BACKGROUND: Rgb = Rgb::new(0x1e, 0x1e, 0x2e);

/// The dark theme's foreground, shared by the theme and its palette.
const DARK_FOREGROUND: Rgb = Rgb::new(0xcd, 0xd6, 0xf4);

/// The dark theme's cursor color, shared by the theme and its palette.
const DARK_CURSOR: Rgb = Rgb::new(0xf5, 0xe0, 0xdc);

/// The light theme's background, shared by the theme and its palette.
const LIGHT_BACKGROUND: Rgb = Rgb::new(0xef, 0xf1, 0xf5);

/// The light theme's foreground, shared by the theme and its palette.
const LIGHT_FOREGROUND: Rgb = Rgb::new(0x4c, 0x4f, 0x69);

/// The light theme's cursor color, shared by the theme and its palette.
const LIGHT_CURSOR: Rgb = Rgb::new(0xdc, 0x8a, 0x78);

/// How opaque the built-in themes' modal backdrop is, out of 255.
const BACKDROP_ALPHA: u8 = 140;

// ========================================================================
// Data Structures
// ========================================================================

/// What an app's chrome is drawn and worded with: its colors, the shape of
/// its boxes, its fonts, and the words misty's widgets show. Widget and
/// chrome code takes every color, box style, and built-in word from here,
/// never from a literal, so switching the theme restyles (or translates)
/// all of it. The default is [`Theme::dark`].
///
/// Fields are added to the theme as misty grows, so build one from a
/// built-in with struct-update syntax, changing only what the app sets:
/// `Theme { accent, ..Theme::dark() }`. A theme spelled out field by field
/// stops compiling when a field is added.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Theme {
    /// The color that marks focus and the active item.
    pub accent: Rgb,
    /// How opaque the backdrop dimming the app behind a modal (a dialog,
    /// the settings page, a centered quick pick) is, from 0 (clear) to 255
    /// (hiding the app); drawn in [`Theme::background`].
    pub backdrop_alpha: u8,
    /// The background of the content area: what a window is cleared to,
    /// what fields and the controls drawn on the accent sit on, and what a
    /// cell's [`CellColor::Default`] background means.
    pub background: Rgb,
    /// How opaque cells with the default background are drawn, 0
    /// (see-through) to 255 (solid): below 255 they let what is behind a
    /// transparent window show through, while cells with a background of
    /// their own stay solid.
    pub background_alpha: u8,
    /// Window borders and the lines between panes.
    pub border: Rgb,
    /// The font menus and other chrome set their labels in: the cell
    /// grid's monospace font, or the proportional one.
    pub chrome_text: ChromeText,
    /// The color of the text cursor: a field's or an editor's caret, and a
    /// cell grid's cursor.
    pub cursor: Rgb,
    /// The background of the line the cursor is on, in an editor.
    pub cursor_line: Rgb,
    /// The background of a control that destroys something, such as a
    /// window's close button, while the pointer is over it.
    pub danger: Rgb,
    /// Primary text and icons: labels, content, the active item's title,
    /// and what a cell's [`CellColor::Default`] foreground means.
    pub foreground: Rgb,
    /// The background of a chrome control while the pointer is over it.
    pub hover: Rgb,
    /// How long the widgets' motions take: hover fades, caret blinks.
    pub motion: Motion,
    /// Secondary text: hints, inactive titles, disabled items.
    pub muted: Rgb,
    /// Text and icons drawn on the [`Theme::danger`] color, such as the
    /// cross on a hovered close button.
    pub on_danger: Rgb,
    /// The 256 colors a [`CellColor::Indexed`] names. The default
    /// foreground and background and the cursor are the theme's own
    /// [`Theme::foreground`], [`Theme::background`], and [`Theme::cursor`].
    pub palette: Palette,
    /// The background of a panel raised over the content: a menu, a popup,
    /// a completion list.
    pub raised: Rgb,
    /// A scrollbar's thumb.
    pub scrollbar: Rgb,
    /// The background of the search match the cursor is on.
    pub search_current: Rgb,
    /// The background of the other search matches.
    pub search_match: Rgb,
    /// The background of selected content.
    pub selection: Rgb,
    /// How round boxes are, how thick their borders, and whether floating
    /// ones cast a shadow: [`Shape::GRID`] or [`Shape::MODERN`], or one of
    /// the app's own.
    pub shape: Shape,
    /// The words misty's widgets show and tell screen readers (a title
    /// bar's button names, a tree's loading row), English in the built-in
    /// themes; set them to translate every widget.
    pub strings: Strings,
    /// The background of chrome around the content: title bar, tab strip,
    /// status bar.
    pub surface: Rgb,
    /// The colors of highlighted source text, by highlight name.
    pub syntax: SyntaxColors,
    /// The mark of a warning, such as the bar on a warning toast.
    pub warning: Rgb,
}

/// A named color of a [`Theme`], for code that picks a color by role and
/// resolves it later, against whichever theme is current.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ThemeToken {
    /// [`Theme::accent`].
    Accent,
    /// [`Theme::background`].
    Background,
    /// [`Theme::border`].
    Border,
    /// [`Theme::cursor`].
    Cursor,
    /// [`Theme::cursor_line`].
    CursorLine,
    /// [`Theme::danger`].
    Danger,
    /// [`Theme::foreground`].
    Foreground,
    /// [`Theme::hover`].
    Hover,
    /// [`Theme::muted`].
    Muted,
    /// [`Theme::on_danger`].
    OnDanger,
    /// [`Theme::raised`].
    Raised,
    /// [`Theme::scrollbar`].
    Scrollbar,
    /// [`Theme::search_current`].
    SearchCurrent,
    /// [`Theme::search_match`].
    SearchMatch,
    /// [`Theme::selection`].
    Selection,
    /// [`Theme::surface`].
    Surface,
    /// [`Theme::warning`].
    Warning,
}

/// The font a theme sets its chrome's labels in (menu items and their
/// keys, for now). Content keeps its own font either way: a terminal's
/// cells, an editor's text, a table's data.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ChromeText {
    /// The cell grid's monospace font, each character in its cells: the
    /// look of a terminal. The built-in themes' choice.
    #[default]
    Cells,
    /// The proportional font, each character as wide as its shape, as most
    /// desktop apps set their menus.
    Proportional,
}

/// The shape of a theme's boxes (popups, menus, dialogs, panels, fields),
/// in whole logical pixels: scaled to the display when drawn, so a theme
/// looks the same on every screen. [`Shape::GRID`] keeps boxes square and
/// flat, as on a terminal's cell grid; [`Shape::MODERN`] rounds them and
/// lifts floating ones with a soft shadow.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Shape {
    /// How thick box borders are; 0 for none.
    pub border_width: u16,
    /// The radius of the corners of controls and highlights inside boxes
    /// (buttons, fields, a selected row); 0 for square. Smaller than
    /// [`Shape::corner_radius`], so a highlight sits evenly inside its box.
    pub control_radius: u16,
    /// The radius of box corners; 0 for square.
    pub corner_radius: u16,
    /// The shadow a floating box casts; `None` for none.
    pub shadow: Option<ShapeShadow>,
}

/// The shadow a floating box casts, in whole logical pixels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ShapeShadow {
    /// How far the shadow fades out; see [`Shadow::blur`].
    pub blur: u16,
    /// The shadow's color at its darkest, blended by its alpha.
    pub color: Rgba,
    /// How far right of the box the shadow is moved; negative for left.
    pub offset_x: i16,
    /// How far below the box the shadow is moved; negative for above.
    pub offset_y: i16,
}

// ========================================================================
// Theme Implementations
// ========================================================================

impl Theme {
    /// The built-in dark theme: light text on a deep blue-grey.
    pub fn dark() -> Self {
        Self {
            accent: Rgb::new(0x89, 0xb4, 0xfa),
            backdrop_alpha: BACKDROP_ALPHA,
            background: DARK_BACKGROUND,
            background_alpha: u8::MAX,
            border: Rgb::new(0x45, 0x47, 0x5a),
            chrome_text: ChromeText::Cells,
            cursor: DARK_CURSOR,
            cursor_line: Rgb::new(0x2a, 0x2b, 0x3c),
            danger: Rgb::new(0xd2, 0x0f, 0x39),
            foreground: DARK_FOREGROUND,
            hover: Rgb::new(0x31, 0x32, 0x44),
            motion: Motion::default(),
            muted: Rgb::new(0x6c, 0x70, 0x86),
            on_danger: Rgb::new(0xff, 0xff, 0xff),
            palette: Palette::new(DARK_ANSI),
            raised: Rgb::new(0x26, 0x26, 0x37),
            scrollbar: Rgb::new(0x45, 0x47, 0x5a),
            search_current: Rgb::new(0xfa, 0xb3, 0x87),
            search_match: Rgb::new(0x57, 0x51, 0x3d),
            selection: Rgb::new(0x58, 0x5b, 0x70),
            shape: Shape::GRID,
            strings: Strings::english(),
            surface: Rgb::new(0x18, 0x18, 0x25),
            syntax: dark_syntax(),
            warning: Rgb::new(0xf9, 0xe2, 0xaf),
        }
    }

    /// The built-in light theme: dark text on a pale grey.
    pub fn light() -> Self {
        Self {
            accent: Rgb::new(0x1e, 0x66, 0xf5),
            backdrop_alpha: BACKDROP_ALPHA,
            background: LIGHT_BACKGROUND,
            background_alpha: u8::MAX,
            border: Rgb::new(0xbc, 0xc0, 0xcc),
            chrome_text: ChromeText::Cells,
            cursor: LIGHT_CURSOR,
            cursor_line: Rgb::new(0xe6, 0xe9, 0xef),
            danger: Rgb::new(0xd2, 0x0f, 0x39),
            foreground: LIGHT_FOREGROUND,
            hover: Rgb::new(0xcc, 0xd0, 0xda),
            motion: Motion::default(),
            muted: Rgb::new(0x8c, 0x8f, 0xa1),
            on_danger: Rgb::new(0xff, 0xff, 0xff),
            palette: Palette::new(LIGHT_ANSI),
            raised: Rgb::new(0xff, 0xff, 0xff),
            scrollbar: Rgb::new(0xbc, 0xc0, 0xcc),
            search_current: Rgb::new(0xfe, 0x64, 0x0b),
            search_match: Rgb::new(0xf5, 0xe0, 0xb0),
            selection: Rgb::new(0xac, 0xb0, 0xbe),
            shape: Shape::GRID,
            strings: Strings::english(),
            surface: Rgb::new(0xe6, 0xe9, 0xef),
            syntax: light_syntax(),
            warning: Rgb::new(0xdf, 0x8e, 0x1d),
        }
    }

    /// The color `token` names in this theme.
    pub fn color(&self, token: ThemeToken) -> Rgb {
        match token {
            ThemeToken::Accent => self.accent,
            ThemeToken::Background => self.background,
            ThemeToken::Border => self.border,
            ThemeToken::Cursor => self.cursor,
            ThemeToken::CursorLine => self.cursor_line,
            ThemeToken::Danger => self.danger,
            ThemeToken::Foreground => self.foreground,
            ThemeToken::Hover => self.hover,
            ThemeToken::Muted => self.muted,
            ThemeToken::OnDanger => self.on_danger,
            ThemeToken::Raised => self.raised,
            ThemeToken::Scrollbar => self.scrollbar,
            ThemeToken::SearchCurrent => self.search_current,
            ThemeToken::SearchMatch => self.search_match,
            ThemeToken::Selection => self.selection,
            ThemeToken::Surface => self.surface,
            ThemeToken::Warning => self.warning,
        }
    }

    /// The cell color `color` used as a foreground: [`CellColor::Default`]
    /// is [`Theme::foreground`], an indexed color the palette's entry.
    pub fn foreground_of(&self, color: CellColor) -> Rgb {
        self.resolve(color, self.foreground)
    }

    /// The cell color `color` used as a background: [`CellColor::Default`]
    /// is [`Theme::background`], an indexed color the palette's entry.
    pub fn background_of(&self, color: CellColor) -> Rgb {
        self.resolve(color, self.background)
    }

    /// `color` with [`CellColor::Default`] meaning `default`.
    fn resolve(&self, color: CellColor, default: Rgb) -> Rgb {
        match color {
            CellColor::Default => default,
            CellColor::Indexed(index) => self.palette.indexed(index),
            CellColor::Rgb(rgb) => rgb,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

// ========================================================================
// ShapeShadow Implementations
// ========================================================================

impl ShapeShadow {
    /// A soft, translucent black shadow a little below the box: the one
    /// [`Shape::MODERN`] casts.
    pub const SOFT: Self = Self {
        blur: 12,
        color: Rgba::new(0, 0, 0, 0x59),
        offset_x: 0,
        offset_y: 4,
    };
}

// ========================================================================
// Shape Implementations
// ========================================================================

impl Shape {
    /// Square, flat boxes with a one-pixel border and no shadow: the look
    /// of a terminal's cell grid. The built-in themes' shape.
    pub const GRID: Self = Self {
        border_width: 1,
        control_radius: 0,
        corner_radius: 0,
        shadow: None,
    };

    /// Boxes with softly rounded corners and a one-pixel border, controls
    /// and highlights rounded a little less; floating boxes cast a soft
    /// shadow below them.
    pub const MODERN: Self = Self {
        border_width: 1,
        control_radius: 4,
        corner_radius: 6,
        shadow: Some(ShapeShadow::SOFT),
    };

    /// How thick box borders are on a display scaled by `scale_factor`, in
    /// physical pixels: whole pixels, and at least one when there is a
    /// border at all, so a thin border never vanishes.
    pub fn border(&self, scale_factor: f64) -> f32 {
        match self.border_width {
            0 => 0.0,
            width => (f32::from(width) * scale_factor as f32).round().max(1.0),
        }
    }

    /// How far inside a box's edge its content stays on a display scaled
    /// by `scale_factor`, in whole physical pixels: past the border, and far
    /// enough in that a square row of content (a highlighted item) keeps
    /// clear of the rounded corners. The border's width for square boxes.
    pub fn content_inset(&self, scale_factor: f64) -> f32 {
        let border = self.border(scale_factor);
        let radius = f32::from(self.corner_radius) * scale_factor as f32;
        // Inside the border the box is rounded by `radius - border`, about a
        // center `radius` in from both edges. A content corner `inset` in
        // from both edges touches that curve when
        // `(radius - inset) * SQRT_2 == radius - border`.
        let clear = radius - (radius - border).max(0.0) / SQRT_2;
        border.max(clear.ceil())
    }

    /// The style of a box resting in place, such as a panel or a field,
    /// filled with `fill` and lined with `border` (theme colors), on a
    /// display scaled by `scale_factor`.
    pub fn flat_box(&self, fill: Rgb, border: Rgb, scale_factor: f64) -> BoxStyle {
        let width = self.border(scale_factor);
        BoxStyle {
            border: (width > 0.0).then_some(Border {
                color: Rgba::from(border),
                width,
            }),
            corner_radius: f32::from(self.corner_radius) * scale_factor as f32,
            fill: Rgba::from(fill),
            shadow: None,
        }
    }

    /// The style of a control or highlight filled with `fill` (a theme
    /// color), such as a button or the selected row of a list, on a display
    /// scaled by `scale_factor`: rounded by [`Shape::control_radius`], with
    /// no border or shadow. A square one is a plain fill.
    pub fn highlight(&self, fill: Rgb, scale_factor: f64) -> BoxStyle {
        BoxStyle {
            border: None,
            corner_radius: f32::from(self.control_radius) * scale_factor as f32,
            fill: Rgba::from(fill),
            shadow: None,
        }
    }

    /// The style of a box floating over the content, such as a popup, a
    /// menu, or a dialog: [`Shape::flat_box`] with the shape's shadow.
    pub fn floating_box(&self, fill: Rgb, border: Rgb, scale_factor: f64) -> BoxStyle {
        let scale = scale_factor as f32;
        BoxStyle {
            shadow: self.shadow.map(|shadow| Shadow {
                blur: f32::from(shadow.blur) * scale,
                color: shadow.color,
                offset: Point::new(
                    f32::from(shadow.offset_x) * scale,
                    f32::from(shadow.offset_y) * scale,
                ),
            }),
            ..self.flat_box(fill, border, scale_factor)
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The dark theme's syntax colors: pastels on the deep background, plain
/// variables, comments a step brighter than muted text.
fn dark_syntax() -> SyntaxColors {
    syntax_colors(&[
        ("attribute", Rgb::new(0xf9, 0xe2, 0xaf)),
        ("boolean", Rgb::new(0xfa, 0xb3, 0x87)),
        ("comment", Rgb::new(0x7f, 0x84, 0x9c)),
        ("constant", Rgb::new(0xfa, 0xb3, 0x87)),
        ("constructor", Rgb::new(0x74, 0xc7, 0xec)),
        ("embedded", Rgb::new(0xf2, 0xcd, 0xcd)),
        ("escape", Rgb::new(0xf5, 0xc2, 0xe7)),
        ("function", Rgb::new(0x89, 0xb4, 0xfa)),
        ("function.builtin", Rgb::new(0x89, 0xdc, 0xeb)),
        ("function.macro", Rgb::new(0x94, 0xe2, 0xd5)),
        ("keyword", Rgb::new(0xcb, 0xa6, 0xf7)),
        ("label", Rgb::new(0x74, 0xc7, 0xec)),
        ("number", Rgb::new(0xfa, 0xb3, 0x87)),
        ("operator", Rgb::new(0x89, 0xdc, 0xeb)),
        ("property", Rgb::new(0xb4, 0xbe, 0xfe)),
        ("punctuation", Rgb::new(0x93, 0x99, 0xb2)),
        ("string", Rgb::new(0xa6, 0xe3, 0xa1)),
        ("string.escape", Rgb::new(0xf5, 0xc2, 0xe7)),
        ("string.special", Rgb::new(0xf5, 0xc2, 0xe7)),
        ("text.literal", Rgb::new(0xa6, 0xe3, 0xa1)),
        ("text.reference", Rgb::new(0xb4, 0xbe, 0xfe)),
        ("text.title", Rgb::new(0x89, 0xb4, 0xfa)),
        ("text.uri", Rgb::new(0xf5, 0xe0, 0xdc)),
        ("type", Rgb::new(0xf9, 0xe2, 0xaf)),
        ("variable.builtin", Rgb::new(0xf3, 0x8b, 0xa8)),
        ("variable.parameter", Rgb::new(0xeb, 0xa0, 0xac)),
    ])
}

/// The light theme's syntax colors: the same roles as the dark theme's,
/// deepened to read on the pale background.
fn light_syntax() -> SyntaxColors {
    syntax_colors(&[
        ("attribute", Rgb::new(0xdf, 0x8e, 0x1d)),
        ("boolean", Rgb::new(0xfe, 0x64, 0x0b)),
        ("comment", Rgb::new(0x7c, 0x7f, 0x93)),
        ("constant", Rgb::new(0xfe, 0x64, 0x0b)),
        ("constructor", Rgb::new(0x20, 0x9f, 0xb5)),
        ("embedded", Rgb::new(0xdd, 0x78, 0x78)),
        ("escape", Rgb::new(0xea, 0x76, 0xcb)),
        ("function", Rgb::new(0x1e, 0x66, 0xf5)),
        ("function.builtin", Rgb::new(0x04, 0xa5, 0xe5)),
        ("function.macro", Rgb::new(0x17, 0x92, 0x99)),
        ("keyword", Rgb::new(0x88, 0x39, 0xef)),
        ("label", Rgb::new(0x20, 0x9f, 0xb5)),
        ("number", Rgb::new(0xfe, 0x64, 0x0b)),
        ("operator", Rgb::new(0x04, 0xa5, 0xe5)),
        ("property", Rgb::new(0x72, 0x87, 0xfd)),
        ("punctuation", Rgb::new(0x7c, 0x7f, 0x93)),
        ("string", Rgb::new(0x40, 0xa0, 0x2b)),
        ("string.escape", Rgb::new(0xea, 0x76, 0xcb)),
        ("string.special", Rgb::new(0xea, 0x76, 0xcb)),
        ("text.literal", Rgb::new(0x40, 0xa0, 0x2b)),
        ("text.reference", Rgb::new(0x72, 0x87, 0xfd)),
        ("text.title", Rgb::new(0x1e, 0x66, 0xf5)),
        ("text.uri", Rgb::new(0xdc, 0x8a, 0x78)),
        ("type", Rgb::new(0xdf, 0x8e, 0x1d)),
        ("variable.builtin", Rgb::new(0xd2, 0x0f, 0x39)),
        ("variable.parameter", Rgb::new(0xe6, 0x45, 0x53)),
    ])
}

/// Syntax colors giving each name of `colors` its color.
fn syntax_colors(colors: &[(&str, Rgb)]) -> SyntaxColors {
    colors
        .iter()
        .fold(SyntaxColors::new(), |syntax, (name, color)| {
            syntax.with(*name, *color)
        })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A rough brightness, enough to tell light from dark.
    fn brightness(color: Rgb) -> u32 {
        u32::from(color.r) + u32::from(color.g) + u32::from(color.b)
    }

    #[test]
    fn test_each_builtin_theme_puts_text_on_the_side_its_name_says() {
        let dark = Theme::dark();
        assert!(brightness(dark.foreground) > brightness(dark.background));
        let light = Theme::light();
        assert!(brightness(light.foreground) < brightness(light.background));
    }

    #[test]
    fn test_muted_text_sits_between_the_background_and_the_foreground() {
        for theme in [Theme::dark(), Theme::light()] {
            let [background, muted, foreground] =
                [theme.background, theme.muted, theme.foreground].map(brightness);
            assert!(
                background.min(foreground) < muted && muted < background.max(foreground),
                "{theme:?}"
            );
        }
    }

    #[test]
    fn test_default_cell_colors_follow_a_theme_changed_by_struct_update() {
        let (foreground, background) = (Rgb::new(1, 2, 3), Rgb::new(4, 5, 6));
        let theme = Theme {
            background,
            foreground,
            ..Theme::dark()
        };
        assert_eq!(theme.foreground_of(CellColor::Default), foreground);
        assert_eq!(theme.background_of(CellColor::Default), background);
        let exact = Rgb::new(9, 8, 7);
        assert_eq!(theme.foreground_of(CellColor::Rgb(exact)), exact);
        assert_eq!(
            theme.background_of(CellColor::Indexed(15)),
            theme.palette.indexed(15)
        );
    }

    #[test]
    fn test_the_builtin_themes_keep_the_grid_shape() {
        assert_eq!(Theme::dark().shape, Shape::GRID);
        assert_eq!(Theme::light().shape, Shape::GRID);
    }

    #[test]
    fn test_a_grid_box_is_square_and_flat_even_floating() {
        let theme = Theme::dark();
        let style = Shape::GRID.floating_box(theme.surface, theme.border, 1.0);
        assert_eq!(style.corner_radius, 0.0);
        assert_eq!(style.shadow, None);
        assert_eq!(style.border.map(|border| border.width), Some(1.0));
    }

    #[test]
    fn test_a_modern_box_scales_with_the_display_and_floats_on_a_shadow() {
        let theme = Theme::dark();
        let flat = Shape::MODERN.flat_box(theme.surface, theme.border, 2.0);
        assert_eq!(flat.corner_radius, 12.0, "6 logical pixels at 2x");
        assert_eq!(flat.shadow, None, "a resting box casts none");
        let floating = Shape::MODERN.floating_box(theme.surface, theme.border, 2.0);
        let shadow = floating.shadow.expect("a floating box casts one");
        assert_eq!(shadow.blur, 24.0);
        assert_eq!(shadow.offset, Point::new(0.0, 8.0));
        assert_eq!(floating.fill, Rgba::from(theme.surface));
    }

    #[test]
    fn test_a_thin_border_stays_a_whole_pixel_and_none_stays_none() {
        assert_eq!(Shape::GRID.border(0.5), 1.0, "never thinner than a pixel");
        assert_eq!(Shape::GRID.border(1.5), 2.0, "rounded to a whole pixel");
        let borderless = Shape {
            border_width: 0,
            ..Shape::MODERN
        };
        assert_eq!(borderless.border(2.0), 0.0);
        let theme = Theme::dark();
        assert_eq!(
            borderless.flat_box(theme.surface, theme.border, 2.0).border,
            None
        );
    }

    #[test]
    fn test_content_clears_the_border_and_the_rounded_corners() {
        assert_eq!(Shape::GRID.content_inset(1.0), 1.0, "just the border");
        assert_eq!(Shape::GRID.content_inset(2.0), 2.0);
        // 6 - 5 / sqrt(2) is about 2.46: three pixels keep a square row's
        // corner inside the curve.
        assert_eq!(Shape::MODERN.content_inset(1.0), 3.0);
        assert_eq!(Shape::MODERN.content_inset(2.0), 5.0, "12 - 10 / sqrt(2)");
    }

    #[test]
    fn test_a_highlight_is_rounded_less_than_its_box_and_plain_on_the_grid() {
        let theme = Theme::dark();
        let grid = Shape::GRID.highlight(theme.selection, 2.0);
        assert_eq!(grid.corner_radius, 0.0);
        assert_eq!((grid.border, grid.shadow), (None, None), "a plain fill");
        let modern = Shape::MODERN.highlight(theme.selection, 2.0);
        assert_eq!(modern.corner_radius, 8.0, "4 logical pixels at 2x");
        assert!(
            modern.corner_radius
                < Shape::MODERN
                    .flat_box(theme.surface, theme.border, 2.0)
                    .corner_radius
        );
        assert_eq!(modern.fill, Rgba::from(theme.selection));
    }
}

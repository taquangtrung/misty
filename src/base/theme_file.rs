//! Theme files in KDL, laid over a built-in [`Theme`]: the format is
//! described on [`Theme::from_kdl`].

use std::error::Error as StdError;

use kdl::{KdlDocument, KdlNode};
use thiserror::Error;

use super::color::{HexColorError, Rgb, Rgba};
use super::palette::ANSI_COLORS;
use super::strings::Strings;
use super::theme::{ChromeText, Emphasis, Shape, ShapeShadow, Theme};

// ========================================================================
// Constants
// ========================================================================

/// The node naming the theme a file starts from.
const BASE: &str = "base";

/// The color a list's selected row follows when it has none of its own.
const SELECTION: &str = "selection";

/// A list's selected row's own color.
const SELECTED_ROW: &str = "selected-row";

/// The color matched characters follow when they have none of their own.
const ACCENT: &str = "accent";

/// Matched characters' own color.
const MATCH_TEXT: &str = "match-text";

/// The node holding a file's colors.
const COLORS: &str = "colors";

/// The block naming ANSI colors, inside `colors`.
const ANSI: &str = "ansi";

/// The node holding a file's box shape.
const SHAPE: &str = "shape";

/// The node naming the font chrome labels are set in.
const CHROME_TEXT: &str = "chrome-text";

/// The node naming how chrome stresses words.
const EMPHASIS: &str = "emphasis";

/// The node holding the words widgets show.
const STRINGS: &str = "strings";

/// The name of [`Shape::GRID`] in a `shape` node.
const GRID: &str = "grid";

/// The name of [`Shape::MODERN`] in a `shape` node.
const MODERN: &str = "modern";

/// The block setting palette entries by number, inside `colors`.
const INDEXED: &str = "indexed";

/// The block coloring highlight names, inside `colors`.
const SYNTAX: &str = "syntax";

/// The node setting how opaque the default background is, inside `colors`.
const BACKGROUND_ALPHA: &str = "background-alpha";

/// The node setting how opaque a modal's backdrop is, inside `colors`.
const BACKDROP_ALPHA: &str = "backdrop-alpha";

/// The node setting one palette entry, inside `indexed`.
const INDEXED_COLOR: &str = "color";

/// The prefix that names the bright variant of an ANSI color.
const BRIGHT_PREFIX: &str = "bright-";

/// The names of the 8 normal ANSI colors, in palette order. The bright
/// eight follow them under the same names with [`BRIGHT_PREFIX`].
const ANSI_NAMES: [&str; ANSI_COLORS / 2] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// The base name of [`Theme::dark`].
const DARK: &str = "dark";

/// The base name of [`Theme::light`].
const LIGHT: &str = "light";

/// What separates the parts of a node's path in errors.
const PATH_SEPARATOR: &str = ".";

// ========================================================================
// Data Structures
// ========================================================================

/// Why a theme file could not be read. Paths name nodes from the top of the
/// file, joined with dots, as in `colors.ansi.purple`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ThemeFileError {
    /// `background-alpha` or `backdrop-alpha` is missing, not a whole
    /// number, or past 255; carries the node's path.
    #[error("{0}: the alpha must be a whole number from 0 to 255")]
    BadAlpha(String),
    /// An `indexed` entry's number is missing, not a whole number, or past
    /// 255; carries the node's path.
    #[error("{0}: the palette index must be a whole number from 0 to 255")]
    BadIndex(String),
    /// A color value is not a `#rrggbb` hex color.
    #[error(transparent)]
    InvalidColor(InvalidColor),
    /// A node that needs a value has none, or one that is not a string;
    /// carries the node's path.
    #[error("{0}: expected a quoted value")]
    MissingValue(String),
    /// A size in a `shape` block is missing, not a whole number, or out of
    /// its range.
    #[error("{path}: expected a whole number of pixels from {min} to {max}")]
    BadPixels {
        /// The path of the node holding the value.
        path: String,
        /// The smallest value the setting takes.
        min: i64,
        /// The largest value the setting takes.
        max: i64,
    },
    /// The file is not valid KDL; the parser's own error, saying where, is
    /// the [`source`](std::error::Error::source).
    #[error("not valid KDL: {0}")]
    Syntax(#[source] Box<dyn StdError + Send + Sync>),
    /// `base` names no built-in theme; carries the name given.
    #[error("{0:?} is not a built-in theme; use \"dark\" or \"light\"")]
    UnknownBase(String),
    /// `chrome-text` names no font choice; carries the name given.
    #[error("{0:?} is not a chrome text; use \"cells\" or \"proportional\"")]
    UnknownChromeText(String),
    /// `emphasis` names no emphasis; carries the name given.
    #[error("{0:?} is not an emphasis; use \"bold\" or \"regular\"")]
    UnknownEmphasis(String),
    /// `shape` names no built-in shape; carries the name given.
    #[error("{0:?} is not a built-in shape; use \"grid\" or \"modern\"")]
    UnknownShape(String),
    /// A node the format does not define; carries its path.
    #[error("{0}: not a theme setting")]
    UnknownNode(String),
}

/// A color value in a theme file that is not a hex color.
#[derive(Debug, Error)]
#[error("{path}: {source}")]
pub struct InvalidColor {
    /// The path of the node holding the value.
    pub path: String,
    /// What is wrong with the value.
    pub source: HexColorError,
}

// ========================================================================
// Theme Implementations
// ========================================================================

impl Theme {
    /// The theme a KDL theme file describes: the built-in theme its `base`
    /// names ([`Theme::dark`] when it names none), with the file's colors
    /// laid over it.
    ///
    /// # Format
    ///
    /// A file names the theme it starts from and the colors it changes;
    /// every color it leaves out keeps the base theme's value:
    ///
    /// ```kdl
    /// base "light"
    /// colors {
    ///     background "#fdf6e3"
    ///     accent "#268bd2"
    ///     ansi {
    ///         red "#dc322f"
    ///         bright-red "#cb4b16"
    ///     }
    ///     indexed {
    ///         color 208 "#ff8700"
    ///     }
    ///     syntax {
    ///         keyword "#859900"
    ///         function.method "#268bd2"
    ///     }
    /// }
    /// ```
    ///
    /// `base` is `"dark"` or `"light"`. The scalar colors are `accent`,
    /// `background`, `border`, `cursor`, `cursor-line`, `danger`,
    /// `foreground`, `hover`, `match-text`, `muted`, `on-danger`, `raised`, `scrollbar`,
    /// `scrollbar-hover`, `search-current`, `search-match`, `selected-row`, `selection`,
    /// `surface`, and `warning` (a file that sets `selection` but not
    /// `selected-row` lights list rows in its `selection` too, and one that sets `accent` but not
    /// `match-text` draws matched characters in its `accent`); `background`, `foreground`, and `cursor` are also what the
    /// cell grid's default colors and cursor are drawn in.
    /// `background-alpha`, a whole number from 0 to 255, sets how opaque
    /// cells with the default background are drawn (see
    /// [`Theme::background_alpha`]), and
    /// `backdrop-alpha` how opaque the backdrop behind a modal is (see
    /// [`Theme::backdrop_alpha`]). `ansi`
    /// names the 16 ANSI colors (`black` to `white`, and each with a `bright-`
    /// prefix), and `indexed` sets any of the 256 palette entries by number.
    /// `syntax` colors highlighted source text by highlight name (see
    /// [`SyntaxColors`](crate::SyntaxColors)); any name is taken, so
    /// a highlighter with names of its own can be themed. Colors are
    /// `#rrggbb`.
    ///
    /// `chrome-text` sets the font chrome labels are set in ([`ChromeText`]):
    /// `"cells"`, the monospace cell grid, or `"proportional"`:
    ///
    /// ```kdl
    /// chrome-text "proportional"
    /// ```
    ///
    /// `emphasis` sets how chrome stresses words ([`Emphasis`]): `"bold"`,
    /// the built-in themes' choice, or `"regular"`, for chrome drawn at one
    /// weight throughout:
    ///
    /// ```kdl
    /// emphasis "regular"
    /// ```
    ///
    /// A `shape` block sets the [`Shape`] of the theme's boxes, in whole
    /// logical pixels. It may start from a built-in shape, `"grid"` (square
    /// and flat) or `"modern"` (rounded, with a shadow under floating boxes),
    /// and change any part of it:
    ///
    /// ```kdl
    /// shape "modern" {
    ///     corner-radius 8
    ///     control-radius 5
    ///     border-width 1
    ///     shadow-blur 16
    ///     shadow-offset 0 6
    ///     shadow-color "#00000080"
    /// }
    /// ```
    ///
    /// Without a name the shape starts from the base theme's. Setting any
    /// `shadow-` value on a shape with no shadow adds [`ShapeShadow::SOFT`]
    /// first and changes that; `shadow-color` takes `#rrggbbaa`, so a shadow
    /// can be translucent. A shape with no shadow starts from `"grid"`.
    ///
    /// A `strings` block translates the words misty's widgets show
    /// ([`Strings`]), each named as its field with dashes for underscores; a
    /// template takes its value where it says `{}`:
    ///
    /// ```kdl
    /// strings {
    ///     close "Schließen"
    ///     load-failed "Laden fehlgeschlagen: {}"
    /// }
    /// ```
    pub fn from_kdl(text: &str) -> Result<Self, ThemeFileError> {
        Self::dark().with_kdl(text)
    }

    /// This theme with a KDL theme file's colors laid over it, in the format
    /// [`Theme::from_kdl`] describes. A `base` in the file replaces this
    /// theme with that built-in one first.
    pub fn with_kdl(&self, text: &str) -> Result<Self, ThemeFileError> {
        let document: KdlDocument = text
            .parse()
            .map_err(|error| ThemeFileError::Syntax(Box::new(error)))?;
        let mut theme = self.clone();
        // The base comes first wherever it is written, so no color is lost
        // to a base named after it.
        for node in document.nodes() {
            if node.name().value() == BASE {
                theme = builtin_theme(&string_value(node, BASE)?)?;
            }
        }
        for node in document.nodes() {
            match node.name().value() {
                BASE => {}
                COLORS => apply_colors(&mut theme, node)?,
                SHAPE => apply_shape(&mut theme.shape, node)?,
                CHROME_TEXT => theme.chrome_text = chrome_text(node)?,
                EMPHASIS => theme.emphasis = emphasis(node)?,
                STRINGS => apply_strings(&mut theme.strings, node)?,
                other => return Err(ThemeFileError::UnknownNode(other.to_owned())),
            }
        }
        Ok(theme)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The built-in theme named `name`.
fn builtin_theme(name: &str) -> Result<Theme, ThemeFileError> {
    match name {
        DARK => Ok(Theme::dark()),
        LIGHT => Ok(Theme::light()),
        other => Err(ThemeFileError::UnknownBase(other.to_owned())),
    }
}

/// Sets every color the `colors` node names in `theme`.
fn apply_colors(theme: &mut Theme, colors: &KdlNode) -> Result<(), ThemeFileError> {
    // A selection color with no row color of its own lights list rows too,
    // as the built-in themes do.
    let names = || child_nodes(colors).iter().map(|node| node.name().value());
    let row_follows =
        names().any(|name| name == SELECTION) && !names().any(|name| name == SELECTED_ROW);
    let match_follows =
        names().any(|name| name == ACCENT) && !names().any(|name| name == MATCH_TEXT);
    for node in child_nodes(colors) {
        let name = node.name().value();
        let path = join_path(COLORS, name);
        match name {
            ANSI => apply_ansi(theme, node, &path)?,
            INDEXED => apply_indexed(theme, node, &path)?,
            SYNTAX => apply_syntax(theme, node, &path)?,
            BACKGROUND_ALPHA => theme.background_alpha = alpha_value(node, path)?,
            BACKDROP_ALPHA => theme.backdrop_alpha = alpha_value(node, path)?,
            scalar => {
                let color = color_value(node, &path)?;
                let slot = scalar_slot(theme, scalar).ok_or(ThemeFileError::UnknownNode(path))?;
                *slot = color;
            }
        }
    }
    if row_follows {
        theme.selected_row = theme.selection;
    }
    if match_follows {
        theme.match_text = theme.accent;
    }
    Ok(())
}

/// Sets `shape` from the `shape` node: first the built-in shape it names,
/// if any, then each size it sets.
fn apply_shape(shape: &mut Shape, node: &KdlNode) -> Result<(), ThemeFileError> {
    if let Some(name) = node
        .entries()
        .first()
        .and_then(|entry| entry.value().as_string())
    {
        *shape = match name {
            GRID => Shape::GRID,
            MODERN => Shape::MODERN,
            other => return Err(ThemeFileError::UnknownShape(other.to_owned())),
        };
    }
    for child in child_nodes(node) {
        let name = child.name().value();
        let path = join_path(SHAPE, name);
        match name {
            "border-width" => shape.border_width = length(child, &path)?,
            "control-radius" => shape.control_radius = length(child, &path)?,
            "corner-radius" => shape.corner_radius = length(child, &path)?,
            "highlight-inset" => shape.highlight_inset = length(child, &path)?,
            "row-padding" => shape.row_padding = length(child, &path)?,
            "shadow-blur" => shadow(shape).blur = length(child, &path)?,
            "shadow-offset" => {
                let shadow = shadow(shape);
                shadow.offset_x = offset(child, 0, &path)?;
                shadow.offset_y = offset(child, 1, &path)?;
            }
            "shadow-color" => shadow(shape).color = rgba_value(child, &path)?,
            _ => return Err(ThemeFileError::UnknownNode(path)),
        }
    }
    Ok(())
}

/// Sets every string the `strings` node names in `strings`.
fn apply_strings(strings: &mut Strings, node: &KdlNode) -> Result<(), ThemeFileError> {
    for child in child_nodes(node) {
        let name = child.name().value();
        let path = join_path(STRINGS, name);
        let value = string_value(child, &path)?;
        let slot = strings
            .named_mut(name)
            .ok_or(ThemeFileError::UnknownNode(path))?;
        *slot = value.into();
    }
    Ok(())
}

/// The font choice the `chrome-text` node names.
fn chrome_text(node: &KdlNode) -> Result<ChromeText, ThemeFileError> {
    match string_value(node, CHROME_TEXT)?.as_str() {
        "cells" => Ok(ChromeText::Cells),
        "proportional" => Ok(ChromeText::Proportional),
        other => Err(ThemeFileError::UnknownChromeText(other.to_owned())),
    }
}

/// The emphasis the `emphasis` node names.
fn emphasis(node: &KdlNode) -> Result<Emphasis, ThemeFileError> {
    match string_value(node, EMPHASIS)?.as_str() {
        "bold" => Ok(Emphasis::Bold),
        "regular" => Ok(Emphasis::Regular),
        other => Err(ThemeFileError::UnknownEmphasis(other.to_owned())),
    }
}

/// `shape`'s shadow, added as [`ShapeShadow::SOFT`] when it has none.
fn shadow(shape: &mut Shape) -> &mut ShapeShadow {
    shape.shadow.get_or_insert(ShapeShadow::SOFT)
}

/// `node`'s first value as a size: a whole number of pixels, 0 or more.
fn length(node: &KdlNode, path: &str) -> Result<u16, ThemeFileError> {
    whole_number(node, 0)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| bad_pixels(path, 0, u16::MAX.into()))
}

/// `node`'s value at `index` as an offset: a whole number of pixels,
/// negative for left or up.
fn offset(node: &KdlNode, index: usize, path: &str) -> Result<i16, ThemeFileError> {
    whole_number(node, index)
        .and_then(|value| i16::try_from(value).ok())
        .ok_or_else(|| bad_pixels(path, i16::MIN.into(), i16::MAX.into()))
}

/// `node`'s value at `index`, if it is a whole number.
fn whole_number(node: &KdlNode, index: usize) -> Option<i128> {
    node.entries()
        .get(index)
        .and_then(|entry| entry.value().as_integer())
}

/// `node`'s first value as an alpha from 0 to 255; the node is at `path`.
fn alpha_value(node: &KdlNode, path: String) -> Result<u8, ThemeFileError> {
    whole_number(node, 0)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or(ThemeFileError::BadAlpha(path))
}

/// The error for a size at `path` that is not a whole number from `min`
/// to `max`.
fn bad_pixels(path: &str, min: i64, max: i64) -> ThemeFileError {
    ThemeFileError::BadPixels {
        path: path.to_owned(),
        min,
        max,
    }
}

/// Sets the ANSI colors the `ansi` node at `path` names.
fn apply_ansi(theme: &mut Theme, ansi: &KdlNode, path: &str) -> Result<(), ThemeFileError> {
    for node in child_nodes(ansi) {
        let name = node.name().value();
        let path = join_path(path, name);
        let (bright, base_name) = match name.strip_prefix(BRIGHT_PREFIX) {
            Some(base_name) => (true, base_name),
            None => (false, name),
        };
        let Some(position) = ANSI_NAMES.iter().position(|known| *known == base_name) else {
            return Err(ThemeFileError::UnknownNode(path));
        };
        let index = position + usize::from(bright) * ANSI_NAMES.len();
        theme.palette.colors[index] = color_value(node, &path)?;
    }
    Ok(())
}

/// Sets the palette entries the `indexed` node at `path` names.
fn apply_indexed(theme: &mut Theme, indexed: &KdlNode, path: &str) -> Result<(), ThemeFileError> {
    for node in child_nodes(indexed) {
        let path = join_path(path, node.name().value());
        if node.name().value() != INDEXED_COLOR {
            return Err(ThemeFileError::UnknownNode(path));
        }
        let index = node
            .entries()
            .first()
            .and_then(|entry| entry.value().as_integer())
            .and_then(|index| u8::try_from(index).ok())
            .ok_or_else(|| ThemeFileError::BadIndex(path.clone()))?;
        let hex = node
            .entries()
            .get(1)
            .and_then(|entry| entry.value().as_string())
            .ok_or_else(|| ThemeFileError::MissingValue(path.clone()))?;
        theme.palette.colors[usize::from(index)] = parse_color(hex, &path)?;
    }
    Ok(())
}

/// Sets the colors of the highlight names the `syntax` node at `path`
/// names.
fn apply_syntax(theme: &mut Theme, syntax: &KdlNode, path: &str) -> Result<(), ThemeFileError> {
    for node in child_nodes(syntax) {
        let name = node.name().value();
        let color = color_value(node, &join_path(path, name))?;
        theme.syntax.set(name, color);
    }
    Ok(())
}

/// The theme field the scalar color `name` sets, or `None` when no color
/// has that name.
fn scalar_slot<'a>(theme: &'a mut Theme, name: &str) -> Option<&'a mut Rgb> {
    match name {
        "accent" => Some(&mut theme.accent),
        "background" => Some(&mut theme.background),
        "border" => Some(&mut theme.border),
        "cursor" => Some(&mut theme.cursor),
        "cursor-line" => Some(&mut theme.cursor_line),
        "danger" => Some(&mut theme.danger),
        "foreground" => Some(&mut theme.foreground),
        "hover" => Some(&mut theme.hover),
        "match-text" => Some(&mut theme.match_text),
        "muted" => Some(&mut theme.muted),
        "on-danger" => Some(&mut theme.on_danger),
        "raised" => Some(&mut theme.raised),
        "scrollbar" => Some(&mut theme.scrollbar),
        "scrollbar-hover" => Some(&mut theme.scrollbar_hover),
        "search-current" => Some(&mut theme.search_current),
        "search-match" => Some(&mut theme.search_match),
        "selected-row" => Some(&mut theme.selected_row),
        "selection" => Some(&mut theme.selection),
        "surface" => Some(&mut theme.surface),
        "warning" => Some(&mut theme.warning),
        _ => None,
    }
}

/// The nodes inside `node`'s braces; none when it has no braces.
fn child_nodes(node: &KdlNode) -> &[KdlNode] {
    node.children().map_or(&[], KdlDocument::nodes)
}

/// `node`'s first value as a string.
fn string_value(node: &KdlNode, path: &str) -> Result<String, ThemeFileError> {
    node.entries()
        .first()
        .and_then(|entry| entry.value().as_string())
        .map(str::to_owned)
        .ok_or_else(|| ThemeFileError::MissingValue(path.to_owned()))
}

/// `node`'s first value as a color.
fn color_value(node: &KdlNode, path: &str) -> Result<Rgb, ThemeFileError> {
    parse_color(&string_value(node, path)?, path)
}

/// `node`'s first value as a color that keeps its alpha.
fn rgba_value(node: &KdlNode, path: &str) -> Result<Rgba, ThemeFileError> {
    Rgba::from_hex(&string_value(node, path)?).map_err(|source| {
        ThemeFileError::InvalidColor(InvalidColor {
            path: path.to_owned(),
            source,
        })
    })
}

/// `hex`, the value of the node at `path`, as a color. Theme colors are
/// opaque, so a `#rrggbbaa` value's alpha is dropped.
fn parse_color(hex: &str, path: &str) -> Result<Rgb, ThemeFileError> {
    let Rgba { b, g, r, .. } = Rgba::from_hex(hex).map_err(|source| {
        ThemeFileError::InvalidColor(InvalidColor {
            path: path.to_owned(),
            source,
        })
    })?;
    Ok(Rgb::new(r, g, b))
}

/// The path of the node `name` inside the node at `parent`.
fn join_path(parent: &str, name: &str) -> String {
    [parent, name].join(PATH_SEPARATOR)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_file_overlays_only_the_colors_it_names() {
        let theme = Theme::from_kdl(
            r##"
            colors {
                accent "#010203"
                ansi { bright-red "#0a0b0c"; black "#000001" }
                indexed { color 208 "#ff8700" }
            }
            "##,
        )
        .unwrap();
        let dark = Theme::dark();
        assert_eq!(theme.accent, Rgb::new(1, 2, 3));
        assert_eq!(theme.palette.colors[9], Rgb::new(10, 11, 12));
        assert_eq!(theme.palette.colors[0], Rgb::new(0, 0, 1));
        assert_eq!(theme.palette.colors[208], Rgb::new(0xff, 0x87, 0));
        assert_eq!(theme.palette.colors[1], dark.palette.colors[1]);
        assert_eq!(theme.surface, dark.surface);
    }

    #[test]
    fn test_editor_colors_and_the_alphas_are_read() {
        let theme = Theme::from_kdl(
            "colors { cursor-line \"#010203\"; search-match \"#040506\"; background-alpha 200; backdrop-alpha 90 }",
        )
        .unwrap();
        assert_eq!(theme.cursor_line, Rgb::new(1, 2, 3));
        assert_eq!(theme.search_match, Rgb::new(4, 5, 6));
        assert_eq!(theme.background_alpha, 200);
        assert_eq!(theme.backdrop_alpha, 90);
        assert_eq!(theme.raised, Theme::dark().raised, "left out, the base's");
        let error = Theme::from_kdl("colors { background-alpha 300 }").unwrap_err();
        assert!(
            matches!(error, ThemeFileError::BadAlpha(path) if path == "colors.background-alpha")
        );
    }

    #[test]
    fn test_a_selected_row_follows_the_selection_unless_it_has_its_own_color() {
        let follows = Theme::from_kdl("colors { selection \"#010203\" }").unwrap();
        assert_eq!(follows.selected_row, Rgb::new(1, 2, 3));
        let own =
            Theme::from_kdl("colors { selected-row \"#040506\"; selection \"#010203\" }").unwrap();
        assert_eq!(own.selected_row, Rgb::new(4, 5, 6));
        assert_eq!(own.selection, Rgb::new(1, 2, 3));
        let untouched = Theme::from_kdl("colors { accent \"#010203\" }").unwrap();
        assert_eq!(untouched.selected_row, Theme::dark().selected_row);
    }

    #[test]
    fn test_matched_text_follows_the_accent_unless_it_has_its_own_color() {
        let follows = Theme::from_kdl("colors { accent \"#010203\" }").unwrap();
        assert_eq!(follows.match_text, Rgb::new(1, 2, 3));
        let own =
            Theme::from_kdl("colors { match-text \"#040506\"; accent \"#010203\" }").unwrap();
        assert_eq!(own.match_text, Rgb::new(4, 5, 6));
        let untouched = Theme::from_kdl("colors { selection \"#010203\" }").unwrap();
        assert_eq!(untouched.match_text, Theme::dark().match_text);
    }

    #[test]
    fn test_syntax_colors_set_dotted_names_and_keep_the_base_for_the_rest() {
        let theme = Theme::from_kdl(
            r##"colors { syntax { function.method "#010203"; keyword "#040506" } }"##,
        )
        .unwrap();
        let dark = Theme::dark();
        assert_eq!(
            theme.syntax.color("function.method.call"),
            Some(Rgb::new(1, 2, 3))
        );
        assert_eq!(theme.syntax.color("keyword"), Some(Rgb::new(4, 5, 6)));
        assert_eq!(theme.syntax.color("string"), dark.syntax.color("string"));
        assert!(matches!(
            Theme::from_kdl(r#"colors { syntax { keyword "blue" } }"#).unwrap_err(),
            ThemeFileError::InvalidColor(InvalidColor { path, .. }) if path == "colors.syntax.keyword"
        ));
    }

    #[test]
    fn test_base_applies_first_wherever_it_is_written() {
        let theme = Theme::dark()
            .with_kdl(r##"colors { muted "#123456" }; base "light""##)
            .unwrap();
        assert_eq!(theme.muted, Rgb::new(0x12, 0x34, 0x56));
        assert_eq!(theme.background, Theme::light().background);
    }

    #[test]
    fn test_with_kdl_without_a_base_keeps_the_theme_it_is_called_on() {
        let theme = Theme::light()
            .with_kdl(r##"colors { cursor "#ffffff" }"##)
            .unwrap();
        assert_eq!(theme.surface, Theme::light().surface);
        assert_eq!(theme.cursor, Rgb::new(0xff, 0xff, 0xff));
    }

    #[test]
    fn test_bad_files_say_what_is_wrong_and_where() {
        let error = |text: &str| Theme::from_kdl(text).unwrap_err();
        assert!(matches!(error("colors {"), ThemeFileError::Syntax(_)));
        assert!(matches!(
            error(r#"base "solarized""#),
            ThemeFileError::UnknownBase(name) if name == "solarized"
        ));
        assert!(matches!(
            error(r##"colors { ansi { purple "#000000" } }"##),
            ThemeFileError::UnknownNode(path) if path == "colors.ansi.purple"
        ));
        assert!(matches!(
            error(r##"colors { shadow "#000000" }"##),
            ThemeFileError::UnknownNode(path) if path == "colors.shadow"
        ));
        assert!(matches!(
            error("colors { accent 7 }"),
            ThemeFileError::MissingValue(path) if path == "colors.accent"
        ));
        assert!(matches!(
            error(r##"colors { indexed { color 256 "#000000" } }"##),
            ThemeFileError::BadIndex(path) if path == "colors.indexed.color"
        ));
        assert!(matches!(
            error(r#"colors { accent "blue" }"#),
            ThemeFileError::InvalidColor(InvalidColor {
                path,
                source: HexColorError::MissingHash(_),
            }) if path == "colors.accent"
        ));
        assert!(matches!(
            error("palette {}"),
            ThemeFileError::UnknownNode(path) if path == "palette"
        ));
    }

    #[test]
    fn test_a_shape_starts_from_the_named_preset_and_changes_what_it_sets() {
        let theme = Theme::from_kdl(
            r#"shape "modern" { corner-radius 8; control-radius 5; shadow-offset -2 6 }"#,
        )
        .unwrap();
        let shadow = theme.shape.shadow.expect("the modern shadow is kept");
        assert_eq!(theme.shape.corner_radius, 8);
        assert_eq!(theme.shape.control_radius, 5);
        assert_eq!(theme.shape.border_width, Shape::MODERN.border_width);
        assert_eq!((shadow.offset_x, shadow.offset_y), (-2, 6));
        assert_eq!(shadow.blur, ShapeShadow::SOFT.blur);
        assert_eq!(theme.accent, Theme::dark().accent, "colors are untouched");
    }

    #[test]
    fn test_a_shadow_value_on_a_flat_shape_adds_a_soft_shadow() {
        let theme = Theme::from_kdl(r##"shape "grid" { shadow-color "#00000080" }"##).unwrap();
        let shadow = theme.shape.shadow.expect("a shadow was added");
        assert_eq!(shadow.color, Rgba::new(0, 0, 0, 0x80), "its alpha is kept");
        assert_eq!(shadow.blur, ShapeShadow::SOFT.blur);
        assert_eq!(theme.shape.corner_radius, 0, "still square");
    }

    #[test]
    fn test_a_shape_without_a_name_keeps_the_base_shape() {
        let theme = Theme {
            shape: Shape::MODERN,
            ..Theme::dark()
        }
        .with_kdl("shape { border-width 2 }")
        .unwrap();
        assert_eq!(theme.shape.border_width, 2);
        assert_eq!(theme.shape.corner_radius, Shape::MODERN.corner_radius);
    }

    #[test]
    fn test_bad_shapes_say_what_is_wrong_and_where() {
        let error = |text: &str| Theme::from_kdl(text).unwrap_err();
        assert!(matches!(
            error(r#"shape "glass""#),
            ThemeFileError::UnknownShape(name) if name == "glass"
        ));
        assert!(matches!(
            error("shape { corner-radius -1 }"),
            ThemeFileError::BadPixels { path, min: 0, .. } if path == "shape.corner-radius"
        ));
        assert!(matches!(
            error("shape { corner-radius 2.5 }"),
            ThemeFileError::BadPixels { path, .. } if path == "shape.corner-radius"
        ));
        assert!(matches!(
            error("shape { shadow-offset 4 }"),
            ThemeFileError::BadPixels { path, min: -32768, .. } if path == "shape.shadow-offset"
        ));
        assert!(matches!(
            error("shape { blur 4 }"),
            ThemeFileError::UnknownNode(path) if path == "shape.blur"
        ));
    }

    #[test]
    fn test_chrome_text_picks_the_font_and_rejects_others() {
        assert_eq!(Theme::dark().chrome_text, ChromeText::Proportional);
        let theme = Theme::from_kdl(r#"chrome-text "cells""#).unwrap();
        assert_eq!(theme.chrome_text, ChromeText::Cells);
        assert!(matches!(
            Theme::from_kdl(r#"chrome-text "serif""#).unwrap_err(),
            ThemeFileError::UnknownChromeText(name) if name == "serif"
        ));
    }

    #[test]
    fn test_emphasis_is_optional_picks_the_weight_and_rejects_others() {
        let plain = Theme::from_kdl(
            r#"base "light"
            shape "modern""#,
        )
        .unwrap();
        assert_eq!(plain.emphasis, Emphasis::Bold, "bold unless the file says");
        assert_eq!(plain.shape, Shape::MODERN);
        let theme = Theme::from_kdl(r#"emphasis "regular""#).unwrap();
        assert_eq!(theme.emphasis, Emphasis::Regular);
        assert!(matches!(
            Theme::from_kdl(r#"emphasis "heavy""#).unwrap_err(),
            ThemeFileError::UnknownEmphasis(name) if name == "heavy"
        ));
    }

    #[test]
    fn test_strings_translate_what_they_name_and_keep_english_for_the_rest() {
        let theme = Theme::from_kdl(
            r#"strings {
                close "Schließen"
                load-failed "Laden fehlgeschlagen: {}"
            }"#,
        )
        .unwrap();
        assert_eq!(theme.strings.close, "Schließen");
        assert_eq!(theme.strings.load_failed, "Laden fehlgeschlagen: {}");
        assert_eq!(theme.strings.menu, "Menu", "the rest stays English");
        assert!(matches!(
            Theme::from_kdl(r#"strings { shut "x" }"#).unwrap_err(),
            ThemeFileError::UnknownNode(path) if path == "strings.shut"
        ));
        assert!(matches!(
            Theme::from_kdl("strings { close 3 }").unwrap_err(),
            ThemeFileError::MissingValue(path) if path == "strings.close"
        ));
    }
}

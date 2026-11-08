//! How a cell is drawn: its [`CellColor`]s, its on-or-off [`Attributes`], and its
//! [`UnderlineStyle`] and underline color, together a [`CellStyle`] of fourteen
//! bytes.

use std::ops::BitOr;

use super::super::color::Rgb;

// ========================================================================
// Data Structures
// ========================================================================

/// A cell's colors and attributes. The default draws the glyph in the
/// default foreground on the default background with no attributes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CellStyle {
    /// The on-or-off attributes: bold, dim, italic, reverse, strikethrough,
    /// overline, blink, and conceal.
    pub attributes: Attributes,
    /// The color behind the glyph.
    pub background: CellColor,
    /// The glyph's color.
    pub foreground: CellColor,
    /// Whether and how the cell is underlined.
    pub underline: UnderlineStyle,
    /// The color of the underline; [`CellColor::Default`] draws it in the
    /// glyph's color, as a terminal does without an underline color.
    pub underline_color: CellColor,
}

/// The on-or-off attributes of a [`CellStyle`], as a set of flags.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Attributes(u8);

/// A cell color. Only [`CellColor::Rgb`] names an exact color; the others are
/// resolved by the renderer from its theme, so a theme change recolors them.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CellColor {
    /// The theme's default foreground or background, whichever the color is
    /// used as.
    #[default]
    Default,
    /// An entry of the theme's 256-color palette: 0 to 15 are the named ANSI
    /// colors, 16 to 231 a 6x6x6 color cube, 232 to 255 a grey ramp.
    Indexed(u8),
    /// An exact 24-bit color, not touched by the theme.
    Rgb(Rgb),
}

/// How a cell is underlined.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum UnderlineStyle {
    /// A wavy line, the usual mark for a spelling or diagnostic error.
    Curly,
    /// A dashed line.
    Dashed,
    /// A dotted line.
    Dotted,
    /// Two parallel lines.
    Double,
    /// Not underlined.
    #[default]
    None,
    /// One line.
    Single,
}

// ========================================================================
// Attributes Implementations
// ========================================================================

impl Attributes {
    /// A glyph shown only while its source's blink timer says so
    /// ([`CellSource::blink_visible`](super::CellSource::blink_visible)), as
    /// a terminal blinks SGR 5 text.
    pub const BLINK: Self = Self(1 << 5);
    /// A heavier glyph weight.
    pub const BOLD: Self = Self(1 << 0);
    /// A glyph kept in the cell but not drawn, as a terminal hides a typed
    /// password (SGR 8): its background and lines still are.
    pub const CONCEAL: Self = Self(1 << 6);
    /// The glyph blended toward its background, for de-emphasized text.
    pub const DIM: Self = Self(1 << 1);
    /// A slanted glyph.
    pub const ITALIC: Self = Self(1 << 2);
    /// No attribute.
    pub const NONE: Self = Self(0);
    /// A line along the cell's top edge (SGR 53).
    pub const OVERLINE: Self = Self(1 << 7);
    /// Foreground and background swapped when drawn.
    pub const REVERSE: Self = Self(1 << 3);
    /// A line through the middle of the glyph.
    pub const STRIKETHROUGH: Self = Self(1 << 4);

    /// Whether every attribute in `other` is in this set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// This set with the attributes in `other` taken out.
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl BitOr for Attributes {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_without_removes_only_the_given_attributes() {
        let attributes = Attributes::BOLD | Attributes::ITALIC;
        let italic = attributes.without(Attributes::BOLD | Attributes::DIM);
        assert!(italic.contains(Attributes::ITALIC));
        assert!(!italic.contains(Attributes::BOLD));
        assert!(
            !italic.contains(Attributes::DIM),
            "removing an absent flag adds nothing"
        );
    }
}

//! [`SyntaxColors`]: the colors of highlighted source text, by highlight
//! name. A syntax highlighter names what each span of text is
//! (`keyword`, `string`, `function.method`); the theme gives each name a
//! color, and a name it has no color for falls back to its first dotted
//! parts (`function.method` to `function`), then to plain text.

use std::collections::BTreeMap;

use super::color::Rgb;

// ========================================================================
// Constants
// ========================================================================

/// What separates the parts of a dotted highlight name.
const NAME_SEPARATOR: char = '.';

// ========================================================================
// Data Structures
// ========================================================================

/// The color of each highlight name a theme colors. Names are free text,
/// so a highlighter with its own names can be themed too.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SyntaxColors {
    /// The color of each name given one.
    colors: BTreeMap<String, Rgb>,
}

// ========================================================================
// SyntaxColors Implementations
// ========================================================================

impl SyntaxColors {
    /// No colors: all highlighted text is drawn plain.
    pub fn new() -> Self {
        Self::default()
    }

    /// These colors with `name` drawn in `color`.
    pub fn with(mut self, name: impl Into<String>, color: Rgb) -> Self {
        self.set(name, color);
        self
    }

    /// Draws text highlighted as `name` in `color`.
    pub fn set(&mut self, name: impl Into<String>, color: Rgb) {
        self.colors.insert(name.into(), color);
    }

    /// The color of text highlighted as `name`: its own color, else that
    /// of its longest prefix of whole dotted parts given one; `None` when
    /// neither has a color, for plain text.
    pub fn color(&self, name: &str) -> Option<Rgb> {
        let mut name = name;
        loop {
            if let Some(color) = self.colors.get(name) {
                return Some(*color);
            }
            name = &name[..name.rfind(NAME_SEPARATOR)?];
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_name_falls_back_to_its_longest_colored_prefix_of_whole_parts() {
        let red = Rgb::new(255, 0, 0);
        let blue = Rgb::new(0, 0, 255);
        let colors = SyntaxColors::new()
            .with("function", red)
            .with("function.method", blue);
        assert_eq!(colors.color("function.method.call"), Some(blue));
        assert_eq!(colors.color("function.macro"), Some(red));
        assert_eq!(colors.color("functional"), None, "whole parts only");
        assert_eq!(colors.color("keyword"), None);
    }
}

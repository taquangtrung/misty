//! The 256-color palette a theme gives cell content, whose first 16 entries
//! are the named ANSI colors: what a [`CellColor::Indexed`] names. The
//! default foreground and background and the cursor are the theme's own
//! colors, which [`Theme::foreground_of`](crate::Theme::foreground_of) and
//! [`Theme::background_of`](crate::Theme::background_of) resolve a cell's
//! colors against.
//!
//! [`CellColor::Indexed`]: crate::grid::CellColor::Indexed

use super::color::Rgb;

// ========================================================================
// Constants
// ========================================================================

/// The number of named ANSI colors: 8 normal, then 8 bright.
pub const ANSI_COLORS: usize = 16;

/// The number of entries in the palette a
/// [`CellColor::Indexed`](crate::grid::CellColor::Indexed) picks from.
pub const PALETTE_SIZE: usize = 256;

/// The first palette index of the grey ramp, just past the color cube.
const GREY_START: usize = 232;

/// The steps along each axis of the color cube.
const CUBE_SIDE: usize = 6;

/// The channel value of each cube step, as xterm defines them.
const CUBE_LEVELS: [u8; CUBE_SIDE] = [0, 95, 135, 175, 215, 255];

/// The channel value of the darkest grey in the ramp.
const GREY_BASE: u8 = 8;

/// How much brighter each grey in the ramp is than the one before.
const GREY_STEP: u8 = 10;

// ========================================================================
// Data Structures
// ========================================================================

/// The indexed colors cell content is drawn in: what a
/// [`CellColor::Indexed`](crate::grid::CellColor::Indexed) in a cell grid or a
/// [`TextColor::Palette`](crate::TextColor::Palette) color names.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Palette {
    /// Every indexed color: the ANSI colors at 0 to 15 (black,
    /// red, green, yellow, blue, magenta, cyan, white, then the bright eight
    /// in the same order), a 6x6x6 color cube at 16 to 231, and a grey ramp
    /// at 232 to 255.
    pub colors: [Rgb; PALETTE_SIZE],
}

// ========================================================================
// Palette Implementations
// ========================================================================

impl Palette {
    /// A palette with `ansi` as entries 0 to 15 and xterm's color cube and
    /// grey ramp after them.
    pub fn new(ansi: [Rgb; ANSI_COLORS]) -> Self {
        let mut colors = [Rgb::default(); PALETTE_SIZE];
        colors[..ANSI_COLORS].copy_from_slice(&ansi);
        for (index, color) in colors.iter_mut().enumerate().skip(ANSI_COLORS) {
            *color = xterm_color(index);
        }
        Self { colors }
    }

    /// Palette entry `index`.
    pub fn indexed(&self, index: u8) -> Rgb {
        self.colors[usize::from(index)]
    }
}

// ========================================================================
// Functions
// ========================================================================

/// xterm's color for palette entry `index`, from 16 up: a step of the color
/// cube below 232, a grey from there.
fn xterm_color(index: usize) -> Rgb {
    if index >= GREY_START {
        // At most 23 steps of 10 from 8: always a byte.
        let steps = u8::try_from(index - GREY_START).unwrap_or(u8::MAX);
        let grey = GREY_BASE.saturating_add(steps.saturating_mul(GREY_STEP));
        return Rgb::new(grey, grey, grey);
    }
    let cube = index - ANSI_COLORS;
    let level = |step: usize| CUBE_LEVELS[step % CUBE_SIDE];
    Rgb::new(
        level(cube / (CUBE_SIDE * CUBE_SIDE)),
        level(cube / CUBE_SIDE),
        level(cube),
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        Palette::new(std::array::from_fn(|index| Rgb::new(index as u8, 0, 0)))
    }

    #[test]
    fn test_the_first_sixteen_indexes_are_the_ansi_colors() {
        let palette = palette();
        assert_eq!(palette.indexed(9), Rgb::new(9, 0, 0));
        assert_eq!(palette.indexed(15), Rgb::new(15, 0, 0));
    }

    #[test]
    fn test_cube_indexes_map_red_green_blue_in_that_order() {
        let palette = palette();
        assert_eq!(palette.indexed(16), Rgb::new(0, 0, 0));
        // 16 + 36 * 5 + 6 * 1 + 2: full red, one green step, two blue steps.
        assert_eq!(palette.indexed(16 + 180 + 6 + 2), Rgb::new(255, 95, 135));
        assert_eq!(palette.indexed(231), Rgb::new(255, 255, 255));
    }

    #[test]
    fn test_grey_ramp_runs_from_8_to_238_in_steps_of_10() {
        let palette = palette();
        assert_eq!(palette.indexed(232), Rgb::new(8, 8, 8));
        assert_eq!(palette.indexed(255), Rgb::new(238, 238, 238));
    }
}

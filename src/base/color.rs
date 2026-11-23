//! Colors as 8-bit sRGB channels: [`Rgb`] for a cell's opaque true color,
//! [`Rgba`] where coverage matters too, such as a window's background.
//!
//! Channels are stored as written in CSS and theme files (sRGB encoded, 0 to
//! 255). A GPU renderer that writes to an sRGB surface wants linear values
//! instead; [`Rgba::to_linear`] converts.

use thiserror::Error;

// ========================================================================
// Constants
// ========================================================================

/// The character every hex color starts with.
const HEX_PREFIX: char = '#';

/// The number of hex digits in `#rrggbb`.
const RGB_DIGITS: usize = 6;

/// The number of hex digits in `#rrggbbaa`.
const RGBA_DIGITS: usize = 8;

/// The hex digits that spell one channel.
const DIGITS_PER_CHANNEL: usize = 2;

/// The bits one hex digit holds.
const BITS_PER_DIGIT: u32 = 4;

/// The largest channel value, as a float, for converting to the unit range.
const CHANNEL_MAX: f32 = 255.0;

/// sRGB decoding: encoded values at or below this are on the linear segment.
const SRGB_LINEAR_THRESHOLD: f32 = 0.04045;

/// sRGB decoding: the slope of the linear segment.
const SRGB_LINEAR_SLOPE: f32 = 12.92;

/// sRGB decoding: the offset of the power segment.
const SRGB_OFFSET: f32 = 0.055;

/// The weights of red, green, and blue in relative luminance, as WCAG
/// defines it.
const LUMINANCE_WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// What WCAG adds to both luminances of a contrast ratio, standing for the
/// glare of a real screen.
const CONTRAST_FLARE: f32 = 0.05;

/// sRGB decoding: the exponent of the power segment.
const SRGB_GAMMA: f32 = 2.4;

// ========================================================================
// Data Structures
// ========================================================================

/// An opaque 24-bit color in sRGB, one byte per channel.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rgb {
    /// Blue, 0 to 255.
    pub b: u8,
    /// Green, 0 to 255.
    pub g: u8,
    /// Red, 0 to 255.
    pub r: u8,
}

/// A 32-bit color in sRGB with straight (not premultiplied) alpha, one byte
/// per channel. An alpha of 255 is opaque, 0 fully transparent.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rgba {
    /// Alpha, 0 (transparent) to 255 (opaque).
    pub a: u8,
    /// Blue, 0 to 255.
    pub b: u8,
    /// Green, 0 to 255.
    pub g: u8,
    /// Red, 0 to 255.
    pub r: u8,
}

/// Why a string is not a hex color. Each variant carries the string as given.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum HexColorError {
    /// A character after the `#` is not a hex digit, as in `#12345g`.
    #[error("{0:?} holds a character that is not a hex digit")]
    InvalidDigit(String),
    /// The string does not start with `#`, as in `112233`.
    #[error("{0:?} does not start with '#'")]
    MissingHash(String),
    /// The `#` is followed by neither 6 nor 8 hex digits, as in `#123`.
    #[error("{0:?} is neither #rrggbb nor #rrggbbaa")]
    WrongLength(String),
}

// ========================================================================
// Rgb Implementations
// ========================================================================

impl Rgb {
    /// The color with channels `r`, `g`, and `b`.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r }
    }

    /// This color with alpha `a`, from 0 (clear) to 255 (opaque): how a
    /// theme color is drawn see-through, such as a dimmed backdrop or a
    /// faint fill under a selection.
    pub const fn with_alpha(self, a: u8) -> Rgba {
        Rgba::new(self.r, self.g, self.b, a)
    }

    /// This color with `share` of `other` mixed in, channel by channel: 0
    /// keeps this color, 1 gives `other`, and shares outside are held to
    /// those ends. For a shade derived from two theme colors, such as a
    /// header a little toward the border color.
    pub fn mix(self, other: Self, share: f32) -> Self {
        let share = share.clamp(0.0, 1.0);
        let blend =
            |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * share).round() as u8;
        Self::new(
            blend(self.r, other.r),
            blend(self.g, other.g),
            blend(self.b, other.b),
        )
    }

    /// The contrast ratio between this color and `other`, as WCAG defines
    /// it: from 1 (the same luminance) to 21 (black on white), the same
    /// either way round.
    pub fn contrast(self, other: Self) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + CONTRAST_FLARE) / (a.min(b) + CONTRAST_FLARE)
    }

    /// The relative luminance, from 0 (black) to 1 (white).
    fn luminance(self) -> f32 {
        let [r, g, b, _] = Rgba::from(self).to_linear();
        let [wr, wg, wb] = LUMINANCE_WEIGHTS;
        wr * r + wg * g + wb * b
    }
}

// ========================================================================
// Rgba Implementations
// ========================================================================

impl Rgba {
    /// Opaque black.
    pub const BLACK: Self = Self::new(0, 0, 0, u8::MAX);

    /// The color with channels `r`, `g`, `b`, and alpha `a`.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { a, b, g, r }
    }

    /// The opaque color with channels `r`, `g`, and `b`.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, u8::MAX)
    }

    /// Reads `#rrggbb` (opaque) or `#rrggbbaa`. Digits may be upper or lower
    /// case; nothing may come before the `#` or after the last digit.
    pub fn from_hex(hex: &str) -> Result<Self, HexColorError> {
        let Some(digits) = hex.strip_prefix(HEX_PREFIX) else {
            return Err(HexColorError::MissingHash(hex.to_owned()));
        };
        // Lengths are in bytes, so a non-ASCII character either fails the
        // length check or the digit check, never splits a pair.
        if digits.len() != RGB_DIGITS && digits.len() != RGBA_DIGITS {
            return Err(HexColorError::WrongLength(hex.to_owned()));
        }
        let mut channels = [u8::MAX; 4];
        for (channel, pair) in channels
            .iter_mut()
            .zip(digits.as_bytes().chunks(DIGITS_PER_CHANNEL))
        {
            let (Some(high), Some(low)) = (hex_value(pair[0]), hex_value(pair[1])) else {
                return Err(HexColorError::InvalidDigit(hex.to_owned()));
            };
            *channel = high << BITS_PER_DIGIT | low;
        }
        let [r, g, b, a] = channels;
        Ok(Self::new(r, g, b, a))
    }

    /// The channels as `[r, g, b, a]` in the unit range, still sRGB encoded:
    /// what a non-sRGB surface stores as is.
    pub fn to_normalized(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a].map(|channel| f32::from(channel) / CHANNEL_MAX)
    }

    /// The channels as `[r, g, b, a]` in the unit range with the sRGB transfer
    /// undone, as shaders and sRGB surfaces expect. Alpha is linear already
    /// and is only scaled.
    pub fn to_linear(self) -> [f32; 4] {
        let [r, g, b, a] = self.to_normalized();
        [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
    }
}

impl From<Rgb> for Rgba {
    /// `rgb` made opaque.
    fn from(rgb: Rgb) -> Self {
        Self::rgb(rgb.r, rgb.g, rgb.b)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The value of one ASCII hex digit, or `None` for any other byte.
fn hex_value(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

/// One sRGB-encoded channel in the unit range, decoded to linear light.
fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= SRGB_LINEAR_THRESHOLD {
        encoded / SRGB_LINEAR_SLOPE
    } else {
        ((encoded + SRGB_OFFSET) / (1.0 + SRGB_OFFSET)).powf(SRGB_GAMMA)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f32 = 1e-4;

    #[test]
    fn test_mix_moves_each_channel_by_the_share_and_holds_it_to_the_ends() {
        let (black, white) = (Rgb::new(0, 0, 0), Rgb::new(255, 255, 255));
        assert_eq!(black.mix(white, 0.4), Rgb::new(102, 102, 102));
        assert_eq!(
            Rgb::new(10, 200, 40).mix(Rgb::new(30, 100, 40), 0.5),
            Rgb::new(20, 150, 40)
        );
        assert_eq!(black.mix(white, -1.0), black);
        assert_eq!(black.mix(white, 2.0), white);
    }

    #[test]
    fn test_contrast_spans_one_to_twenty_one_and_ignores_order() {
        let (black, white) = (Rgb::new(0, 0, 0), Rgb::new(255, 255, 255));
        assert!((black.contrast(white) - 21.0).abs() < TOLERANCE);
        assert!((white.contrast(black) - 21.0).abs() < TOLERANCE);
        assert!((white.contrast(white) - 1.0).abs() < TOLERANCE);
        // Green weighs far more than blue.
        let (green, blue) = (Rgb::new(0, 255, 0), Rgb::new(0, 0, 255));
        assert!(green.contrast(black) > blue.contrast(black));
    }

    #[test]
    fn test_hex_reads_channels_in_order_and_defaults_alpha_to_opaque() {
        assert_eq!(
            Rgba::from_hex("#1a2B3c"),
            Ok(Rgba::new(0x1a, 0x2b, 0x3c, u8::MAX))
        );
        assert_eq!(
            Rgba::from_hex("#11223380"),
            Ok(Rgba::new(0x11, 0x22, 0x33, 0x80))
        );
    }

    #[test]
    fn test_malformed_hex_says_what_is_wrong() {
        let wrong_length = |s: &str| Err(HexColorError::WrongLength(s.into()));
        assert_eq!(
            Rgba::from_hex("112233"),
            Err(HexColorError::MissingHash("112233".into()))
        );
        assert_eq!(Rgba::from_hex("#"), wrong_length("#"));
        assert_eq!(Rgba::from_hex("#123"), wrong_length("#123"));
        assert_eq!(Rgba::from_hex("#1122334"), wrong_length("#1122334"));
        assert_eq!(
            Rgba::from_hex(" #112233"),
            Err(HexColorError::MissingHash(" #112233".into()))
        );
        assert_eq!(
            Rgba::from_hex("#12345g"),
            Err(HexColorError::InvalidDigit("#12345g".into()))
        );
        assert_eq!(
            Rgba::from_hex("#+12345"),
            Err(HexColorError::InvalidDigit("#+12345".into()))
        );
    }

    #[test]
    fn test_non_ascii_hex_is_rejected_without_panicking() {
        // Six bytes after the '#', but three two-byte characters.
        assert_eq!(
            Rgba::from_hex("#ééé"),
            Err(HexColorError::InvalidDigit("#ééé".into()))
        );
    }

    #[test]
    fn test_linear_undoes_the_srgb_curve_but_not_alpha() {
        let [r, g, b, a] = Rgba::new(0x80, 0, u8::MAX, 0x80).to_linear();
        assert!(
            (r - 0.2158605).abs() < TOLERANCE,
            "mid grey decodes darker: {r}"
        );
        assert_eq!(g, 0.0);
        assert!((b - 1.0).abs() < TOLERANCE);
        assert!(
            (a - 128.0 / 255.0).abs() < TOLERANCE,
            "alpha is only scaled: {a}"
        );
    }
}

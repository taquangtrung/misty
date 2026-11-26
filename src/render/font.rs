//! The font a window draws text in, [`Font`], and the size of one grid cell
//! in it, [`CellMetrics`].

use super::desktop_font;

// ========================================================================
// Constants
// ========================================================================

/// The font size used when none is given or the given one is not a number,
/// in logical pixels.
const DEFAULT_SIZE: f32 = 15.0;

/// The smallest font size drawn, in logical pixels. Smaller sizes round the
/// line height down to nothing.
const MIN_SIZE: f32 = 6.0;

/// The largest font size drawn, in logical pixels.
const MAX_SIZE: f32 = 72.0;

/// The line height as a multiple of the font size, unless
/// [`Font::with_line_height`] says otherwise.
const LINE_HEIGHT_RATIO: f32 = 1.3;

/// The weight the cell grid's text is drawn in unless
/// [`Font::with_weights`] says otherwise: regular.
const DEFAULT_WEIGHT: u16 = 400;

/// The weight bold cells are drawn in unless [`Font::with_weights`] says
/// otherwise: bold.
const DEFAULT_BOLD_WEIGHT: u16 = 700;

/// The lightest weight drawn, as CSS numbers weights: thin.
const MIN_WEIGHT: u16 = 100;

/// The heaviest weight drawn: black.
const MAX_WEIGHT: u16 = 900;

/// How many decoration lines fit in a cell's height: an underline is the
/// cell height divided by this, rounded, and never thinner than a pixel.
const LINES_PER_CELL: f32 = 16.0;

/// How far above the baseline a strikethrough sits, as a share of the
/// distance from the cell top to the baseline.
const STRIKE_RAISE: f32 = 0.3;

// ========================================================================
// Data Structures
// ========================================================================

/// The font a window draws text in. The default is the system monospace
/// font at 15 logical pixels, with the family's ligatures.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The family name, such as `"JetBrains Mono"`, or `None` for the
    /// system's monospace font. Characters the family lacks are drawn from
    /// other installed fonts.
    family: Option<String>,
    /// Whether runs of characters the family draws as one glyph (`->`,
    /// `!=` in a programming font) are, in the cell grid's text.
    ligatures: bool,
    /// The line height as a multiple of the size.
    line_height: f32,
    /// The size in logical pixels.
    size: f32,
    /// The weights its regular and bold text are drawn in.
    weights: FaceWeights,
}

/// The weights a [`Font`] draws its regular and bold text in, as CSS
/// numbers them: 100 thin, 400 regular, 700 bold, 900 black.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FaceWeights {
    /// The weight of bold text.
    pub(crate) bold: u16,
    /// The weight of regular text.
    pub(crate) normal: u16,
}

/// The size of one grid cell in physical pixels, and where text and lines
/// sit inside it. Every cell of a grid has the same metrics, measured from
/// the window's font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// How far below the cell's top edge the text baseline is.
    pub baseline: f32,
    /// The cell height, the font's line height.
    pub height: f32,
    /// The cell width, the advance of the font's characters.
    pub width: f32,
}

// ========================================================================
// Font Implementations
// ========================================================================

impl Font {
    /// The system monospace font at `size` logical pixels. Sizes outside
    /// 6 to 72 are drawn at the nearer bound.
    pub fn new(size: f32) -> Self {
        Self {
            family: None,
            ligatures: true,
            line_height: LINE_HEIGHT_RATIO,
            size,
            weights: FaceWeights::default(),
        }
    }

    /// The system monospace font at the size the desktop sets its
    /// interface text in: GNOME's interface font setting on Linux, 13
    /// pixels on macOS, and 14 on Windows; 15 where the desktop names
    /// none. A window draws in it unless
    /// [`WindowOptions::with_font`](crate::WindowOptions::with_font) says
    /// otherwise.
    pub fn desktop() -> Self {
        Self::new(desktop_font::interface_size().unwrap_or(DEFAULT_SIZE))
    }

    /// This font in the family named `family`.
    pub fn with_family(self, family: impl Into<String>) -> Self {
        Self {
            family: Some(family.into()),
            ..self
        }
    }

    /// This font with its ligatures drawn in the cell grid's text, or with
    /// `false`, each character on its own glyph, as a terminal that wants
    /// every column to read as typed sets. Paragraphs keep them either way.
    pub fn with_ligatures(self, ligatures: bool) -> Self {
        Self { ligatures, ..self }
    }

    /// This font with its regular text drawn at the weight `normal` and
    /// its bold text at `bold`, as CSS numbers weights (300 light, 400
    /// regular, 500 medium, 700 bold), for a light or a heavier family
    /// face; weights outside 100 to 900 are drawn at the nearer bound. A
    /// weight the family lacks is drawn in its nearest face.
    pub fn with_weights(self, normal: u16, bold: u16) -> Self {
        let weights = FaceWeights {
            bold: bold.clamp(MIN_WEIGHT, MAX_WEIGHT),
            normal: normal.clamp(MIN_WEIGHT, MAX_WEIGHT),
        };
        Self { weights, ..self }
    }

    /// This font with lines `ratio` times its size high, and so cells as
    /// tall, for an app matching a grid it lays out itself; ratios below 1
    /// or not a number keep the default of 1.3.
    pub fn with_line_height(self, ratio: f32) -> Self {
        let line_height = match ratio.is_finite() && ratio >= 1.0 {
            true => ratio,
            false => LINE_HEIGHT_RATIO,
        };
        Self {
            line_height,
            ..self
        }
    }

    /// The cell this font draws in on a display scaled by `scale_factor`,
    /// exactly as [`Frame::cell_metrics_of`](crate::Frame::cell_metrics_of)
    /// measures it, without a frame: for an app that lays out a grid in it
    /// before the first frame, or again when the font or the scale factor
    /// changes. It is measured with the font system [`ParagraphMeasurer`](crate::ParagraphMeasurer)s
    /// shape with, made by the first call (which takes some milliseconds;
    /// make it on a worker thread to keep it off startup) and cheap after.
    pub fn cell_metrics(&self, scale_factor: f64) -> CellMetrics {
        super::measurer::measure_cell(self, scale_factor)
    }

    /// The line height, in physical pixels, for this font drawn
    /// `font_size` physical pixels high: whole pixels, so rows of cells
    /// tile without seams.
    pub(crate) fn line_height_px(&self, font_size: f32) -> f32 {
        (font_size * self.line_height).round().max(1.0)
    }

    /// Whether the cell grid's text draws the family's ligatures.
    pub(crate) fn has_ligatures(&self) -> bool {
        self.ligatures
    }

    /// The family name, or `None` for the system monospace font.
    pub(crate) fn family(&self) -> Option<&str> {
        self.family.as_deref()
    }

    /// The weights its regular and bold text are drawn in.
    pub(crate) fn weights(&self) -> FaceWeights {
        self.weights
    }

    /// The size in physical pixels on a display scaled by `scale_factor`,
    /// held to what can be drawn.
    pub(crate) fn physical_size(&self, scale_factor: f64) -> f32 {
        let size = match self.size.is_finite() {
            true => self.size.clamp(MIN_SIZE, MAX_SIZE),
            false => DEFAULT_SIZE,
        };
        size * scale_factor as f32
    }
}

// ========================================================================
// FaceWeights Implementations
// ========================================================================

impl FaceWeights {
    /// Regular and bold, as a font is drawn unless
    /// [`Font::with_weights`] says otherwise.
    pub(crate) const DEFAULT: Self = Self {
        bold: DEFAULT_BOLD_WEIGHT,
        normal: DEFAULT_WEIGHT,
    };
}

impl Default for FaceWeights {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Default for Font {
    fn default() -> Self {
        Self::new(DEFAULT_SIZE)
    }
}

// ========================================================================
// CellMetrics Implementations
// ========================================================================

impl CellMetrics {
    /// How many whole columns fit in `width` pixels.
    pub fn cols_in(&self, width: f32) -> usize {
        whole_cells(width, self.width)
    }

    /// How many whole rows fit in `height` pixels.
    pub fn rows_in(&self, height: f32) -> usize {
        whole_cells(height, self.height)
    }

    /// The thickness of underlines, strikethroughs, and thin cursors: whole
    /// pixels, at least one.
    pub(crate) fn line_thickness(&self) -> f32 {
        (self.height / LINES_PER_CELL).round().max(1.0)
    }

    /// The top of a single underline below the cell's top edge: just under
    /// the baseline, but never past the cell's bottom.
    pub(crate) fn underline_top(&self) -> f32 {
        let thickness = self.line_thickness();
        (self.baseline + thickness).min(self.height - thickness)
    }

    /// The top of a strikethrough below the cell's top edge.
    pub(crate) fn strikethrough_top(&self) -> f32 {
        (self.baseline * (1.0 - STRIKE_RAISE)).round()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How many cells `cell` pixels long fit in `length` pixels; none when
/// either is not a positive number.
fn whole_cells(length: f32, cell: f32) -> usize {
    if !(length > 0.0 && cell > 0.0) {
        return 0;
    }
    // Truncating is the point: a partial cell does not count.
    (length / cell).floor() as usize
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> CellMetrics {
        CellMetrics {
            baseline: 15.0,
            height: 20.0,
            width: 9.5,
        }
    }

    #[test]
    fn test_weights_default_to_regular_and_bold_and_are_held_to_the_css_range() {
        let weights = Font::default().weights();
        assert_eq!((weights.normal, weights.bold), (400, 700));
        let light = Font::default().with_weights(300, 500).weights();
        assert_eq!((light.normal, light.bold), (300, 500));
        let wild = Font::default().with_weights(0, 1200).weights();
        assert_eq!((wild.normal, wild.bold), (100, 900));
    }

    #[test]
    fn test_physical_size_is_clamped_scaled_and_survives_nan() {
        assert_eq!(Font::new(15.0).physical_size(2.0), 30.0);
        assert_eq!(Font::new(1.0).physical_size(1.0), MIN_SIZE);
        assert_eq!(Font::new(500.0).physical_size(1.0), MAX_SIZE);
        assert_eq!(Font::new(f32::NAN).physical_size(1.5), DEFAULT_SIZE * 1.5);
    }

    #[test]
    fn test_grid_counts_only_whole_cells() {
        let metrics = metrics();
        assert_eq!(metrics.cols_in(95.0), 10);
        assert_eq!(metrics.cols_in(94.9), 9);
        assert_eq!(metrics.rows_in(59.0), 2);
        assert_eq!(metrics.rows_in(0.0), 0);
        assert_eq!(metrics.cols_in(-5.0), 0);
        assert_eq!(metrics.cols_in(f32::NAN), 0);
    }

    #[test]
    fn test_underline_stays_inside_the_cell() {
        let low_baseline = CellMetrics {
            baseline: 19.5,
            ..metrics()
        };
        let thickness = low_baseline.line_thickness();
        assert!(low_baseline.underline_top() + thickness <= low_baseline.height);
        assert_eq!(metrics().underline_top(), 16.0, "just under the baseline");
    }

    #[test]
    fn test_line_thickness_is_at_least_a_pixel() {
        let tiny = CellMetrics {
            baseline: 5.0,
            height: 6.0,
            width: 4.0,
        };
        assert_eq!(tiny.line_thickness(), 1.0);
        let large = CellMetrics {
            height: 48.0,
            ..metrics()
        };
        assert_eq!(large.line_thickness(), 3.0);
    }
}

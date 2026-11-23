//! [`ParagraphMeasurer`]: paragraph layout off the UI thread, for an app
//! that measures many paragraphs (a book's, a long document's) in the
//! background so that scrolling never waits on shaping, and pages them by
//! their line boundaries; and paragraphs rendered into images, for an app
//! that caches text as a picture.
//!
//! Measurers shape with a font system of their own, one for the process,
//! not the one the windows draw with: shaping thousands of paragraphs
//! while holding the windows' fonts would stall every frame drawn
//! meanwhile, and a font system for each measurer would cost its setup
//! and memory each time. Their font system is made from the same scan of the installed fonts, with misty's inline
//! box font and the app's fonts added in the same order, so it picks the
//! same fonts (fallbacks included) and lays out exactly as
//! [`Frame::paragraph_layout`](crate::Frame::paragraph_layout) does.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use glyphon::{FontSystem, SwashCache};

use super::super::base::color::Rgba;
use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::app_fonts;
use super::font::{CellMetrics, Font};
use super::image::Image;
use super::paragraph::{self, Paragraph, ParagraphFont, ParagraphLayout};
use super::text::measure_cell_in;

// ========================================================================
// Constants
// ========================================================================

/// The font system every measurer shapes with, made with the first.
static SHAPER: OnceLock<Arc<Mutex<Shaper>>> = OnceLock::new();

// ========================================================================
// Data Structures
// ========================================================================

/// Lays out [`Paragraph`]s exactly as a window's [`Frame`](crate::Frame)
/// does, on any thread: their heights for a scroll layout, and their
/// [`ParagraphLayout`]s (line by line, with carets, hit testing, and
/// inline boxes) for paging and placing text not on screen.
///
/// Get one from [`Frame::paragraph_measurer`](crate::Frame::paragraph_measurer),
/// which gives it the window's font and scale factor, and send it (or a
/// clone) to a worker thread. Every measurer shapes with one font system
/// of the process, separate from the windows', taking turns with it:
/// measuring never holds up a frame, and several workers measure no
/// faster than one. The first measurer makes that font system, which
/// takes some milliseconds; it is cheap after. A measurer keeps the font
/// and scale factor it was made with, so get a new one when either
/// changes, and measure again.
#[derive(Clone)]
pub struct ParagraphMeasurer {
    /// The window's proportional font and scale factor.
    font: ParagraphFont,
    /// The font system it shapes with.
    shaper: Arc<Mutex<Shaper>>,
}

/// The font system measurers shape with.
struct Shaper {
    /// How many of the app's fonts ([`app_fonts`]) are loaded.
    app_fonts: usize,
    /// The font database and shaper.
    font_system: FontSystem,
    /// The glyphs rasterized for [`ParagraphMeasurer::render`].
    swash_cache: SwashCache,
}

// ========================================================================
// ParagraphMeasurer Implementations
// ========================================================================

impl ParagraphMeasurer {
    /// A measurer laying out in `font`, the process's font system for
    /// measuring made now unless it was already.
    pub(crate) fn new(font: ParagraphFont) -> Self {
        Self {
            font,
            shaper: Arc::clone(shaper()),
        }
    }

    /// A measurer laying out as a window whose font is `font` does on a
    /// display scaled by `scale_factor`, as the one
    /// [`Frame::paragraph_measurer`](crate::Frame::paragraph_measurer) gives
    /// there, without a frame.
    pub fn for_font(font: &Font, scale_factor: f64) -> Self {
        Self::new(ParagraphFont {
            line_height: font.cell_metrics(scale_factor).height,
            scale_factor: scale_factor as f32,
            size: font.physical_size(scale_factor),
        })
    }

    /// How high `paragraph` is when wrapped at `width` physical pixels:
    /// what [`Frame::measure_paragraph`](crate::Frame::measure_paragraph)
    /// says. Use [`ParagraphMeasurer::layout`] instead when the lines are
    /// needed too: its height is the same, and the paragraph is shaped
    /// once.
    pub fn measure(&self, paragraph: &Paragraph, width: f32) -> f32 {
        self.layout(paragraph, Rect::new(0.0, 0.0, width, 0.0))
            .height()
    }

    /// Where `paragraph` lands drawn in `rect` (its width wraps the lines,
    /// its top left places them): what
    /// [`Frame::paragraph_layout`](crate::Frame::paragraph_layout) says.
    pub fn layout(&self, paragraph: &Paragraph, rect: Rect) -> ParagraphLayout {
        let mut guard = self.shaper.lock().unwrap_or_else(PoisonError::into_inner);
        let font_system = guard.synced();
        paragraph::layout_alone(font_system, self.font, paragraph, rect)
    }

    /// `paragraph` wrapped at `width` physical pixels and drawn in `theme`'s
    /// colors over `background`, as an image `width` wide (at least a
    /// pixel) and as tall as [`ParagraphMeasurer::measure`] says: its span
    /// backgrounds, glyphs, underlines, and strikethroughs, as a frame draws
    /// them, for an app that caches text as a picture or draws it where no
    /// frame is (an inline box's room is left as the background).
    pub fn render(
        &self,
        paragraph: &Paragraph,
        width: f32,
        theme: &Theme,
        background: Rgba,
    ) -> Image {
        let mut guard = self.shaper.lock().unwrap_or_else(PoisonError::into_inner);
        let shaper = &mut *guard;
        shaper.app_fonts = app_fonts::sync(&mut shaper.font_system, shaper.app_fonts);
        paragraph::render_alone(
            &mut shaper.font_system,
            &mut shaper.swash_cache,
            self.font,
            paragraph,
            width,
            theme,
            background,
        )
    }
}

impl fmt::Debug for ParagraphMeasurer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParagraphMeasurer")
            .field("font", &self.font)
            .finish_non_exhaustive()
    }
}

// ========================================================================
// Shaper Implementations
// ========================================================================

impl Shaper {
    /// A font system of the installed fonts and the app's.
    fn new() -> Self {
        let (font_system, app_fonts) = app_fonts::font_system();
        Self {
            app_fonts,
            font_system,
            swash_cache: SwashCache::new(),
        }
    }

    /// The font system, with the app's fonts loaded since it was made
    /// added first.
    fn synced(&mut self) -> &mut FontSystem {
        self.app_fonts = app_fonts::sync(&mut self.font_system, self.app_fonts);
        &mut self.font_system
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The font system every measurer shapes with, made now unless it was
/// already.
fn shaper() -> &'static Arc<Mutex<Shaper>> {
    SHAPER.get_or_init(|| Arc::new(Mutex::new(Shaper::new())))
}

/// The cell `font` draws in on a display scaled by `scale_factor`, measured
/// with the measurers' font system: what [`Font::cell_metrics`] says.
pub(crate) fn measure_cell(font: &Font, scale_factor: f64) -> CellMetrics {
    let mut guard = shaper().lock().unwrap_or_else(PoisonError::into_inner);
    measure_cell_in(guard.synced(), font, scale_factor)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::app::offscreen;
    use crate::base::styled::{FontFamily, Span, TextColor, TextSize};
    use crate::base::theme::Theme;
    use crate::base::theme::ThemeToken;
    use crate::render::paragraph::{InlineBox, TextAlign};

    const TEXT: TextColor = TextColor::Token(ThemeToken::Foreground);

    /// A paragraph of every kind of span: bold, italic, larger, monospace,
    /// a link, right-to-left text, and an inline box, justified.
    fn mixed() -> Paragraph {
        Paragraph::plain("A chapter opens with plain words, ", TEXT)
            .with(Span::new("bold ones, ", TEXT).bold())
            .with(Span::new("italic ones, ", TEXT).italic())
            .with(Span::new("larger ones, ", TEXT).size(TextSize::Scale(1.6)))
            .with(Span::new("code(), ", TEXT).family(FontFamily::Monospace))
            .with_box(InlineBox::new(24.0, 30.0).with_baseline(22.0))
            .with_link(Span::new(" a link, ", TEXT), "misty:docs")
            .with(Span::new("שלום עולם, and the end.", TEXT))
            .with_align(TextAlign::Justify)
    }

    #[test]
    fn test_a_measurer_lays_out_exactly_as_the_frame_does_on_another_thread() {
        let frame = offscreen::plain_frame(800, 600);
        let rects = [
            Rect::new(0.0, 0.0, 180.0, 0.0),
            Rect::new(12.0, 30.0, 333.5, 0.0),
            Rect::new(0.0, 0.0, 2000.0, 0.0),
        ];
        let expected: Vec<ParagraphLayout> = rects
            .iter()
            .map(|rect| frame.paragraph_layout(&mixed(), *rect))
            .collect();
        let heights: Vec<f32> = rects
            .iter()
            .map(|rect| frame.measure_paragraph(&mixed(), rect.width))
            .collect();
        let measurer = frame.paragraph_measurer();
        let worker = measurer.clone();
        let (layouts, measured) = thread::spawn(move || {
            let layouts: Vec<ParagraphLayout> = rects
                .iter()
                .map(|rect| worker.layout(&mixed(), *rect))
                .collect();
            let measured: Vec<f32> = rects
                .iter()
                .map(|rect| worker.measure(&mixed(), rect.width))
                .collect();
            (layouts, measured)
        })
        .join()
        .expect("the worker measured");
        assert_eq!(layouts, expected);
        assert_eq!(measured, heights);
        let lines = |layout: &ParagraphLayout| layout.lines().collect::<Vec<_>>();
        assert_eq!(lines(&layouts[1]), lines(&expected[1]), "the same lines");
        assert!(layouts[0].lines().len() > 1, "lines of a wrapped paragraph");
        assert_eq!(expected[0].inline_boxes().len(), 1, "the box was laid out");
        assert!(expected[0].height() > expected[2].height(), "it wrapped");
    }

    #[test]
    fn test_a_font_s_cell_is_measured_without_a_frame_as_the_frame_measures_it() {
        let frame = offscreen::plain_frame(800, 600);
        for font in [
            Font::new(15.0),
            Font::new(13.5).with_line_height(1.4),
            Font::new(20.0).with_family("DejaVu Sans Mono"),
        ] {
            let alone = thread::spawn({
                let font = font.clone();
                let scale_factor = frame.scale_factor();
                move || font.cell_metrics(scale_factor)
            })
            .join()
            .expect("the worker measured");
            assert_eq!(alone, frame.cell_metrics_of(&font), "{font:?}");
        }
    }

    #[test]
    fn test_a_measurer_for_the_window_s_font_lays_out_as_the_frame_s_does() {
        let frame = offscreen::plain_frame(800, 600);
        let rect = Rect::new(0.0, 0.0, 240.0, 0.0);
        let alone = ParagraphMeasurer::for_font(&Font::default(), frame.scale_factor());
        assert_eq!(
            alone.layout(&mixed(), rect),
            frame.paragraph_layout(&mixed(), rect)
        );
    }

    #[test]
    fn test_a_rendered_paragraph_holds_its_text_and_backgrounds_at_its_measured_size() {
        let theme = Theme::dark();
        let background = Rgba::new(10, 20, 30, u8::MAX);
        let marked = Rgba::new(200, 40, 40, u8::MAX);
        let paragraph = Paragraph::plain("Some words to wrap, and then ", TEXT)
            .with(Span::new("marked ones", TEXT).background(TextColor::Rgba(marked)));
        let measurer = ParagraphMeasurer::for_font(&Font::default(), 1.0);
        let width = 120.5;
        let image = measurer.render(&paragraph, width, &theme, background);
        assert_eq!(image.width(), 121, "as wide as asked, to a whole pixel");
        assert_eq!(
            image.height(),
            measurer.measure(&paragraph, width).ceil() as u32,
            "as tall as its lines"
        );
        let pixels: Vec<Rgba> = image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&[r, g, b, a]| Rgba::new(r, g, b, a))
            .collect();
        assert_eq!(pixels[0], background, "the background where no text is");
        assert!(pixels.contains(&marked), "the span's background");
        let foreground = theme.foreground;
        assert!(
            pixels.iter().any(|pixel| {
                pixel.r.abs_diff(foreground.r) < 8
                    && pixel.g.abs_diff(foreground.g) < 8
                    && pixel.b.abs_diff(foreground.b) < 8
            }),
            "the glyphs in the text color"
        );
    }
}

//! [`ColorField`]: a color edited as `#rrggbb` in a framed field, with a
//! swatch of it at the field's left. The color changes as soon as the text
//! reads as an opaque color; until then the text is the user's draft, and
//! the color the app holds shows again once the field loses focus, or on
//! Esc.

use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::draft::{Draft, DraftEvent};

// ========================================================================
// Constants
// ========================================================================

/// What a color's text starts with.
const HEX_PREFIX: char = '#';

/// The blank cells between the swatch and the field.
const SWATCH_GAP_CELLS: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A color edited as `#rrggbb`, beside a swatch of it.
#[derive(Debug)]
pub struct ColorField {
    /// The field the color is typed in.
    draft: Draft,
    /// The color the app holds.
    value: Rgb,
}

/// What a [`ColorField`] is synced to: [`Control::sync`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorFieldProps<'a> {
    /// What screen readers call it, if anything.
    pub label: Option<&'a str>,
    /// The color the app holds.
    pub value: Rgb,
}

/// What became of an input offered to a [`ColorField`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorFieldResponse {
    /// The user typed this color.
    Changed(Rgb),
    /// The field used the input; the color is as it was.
    Handled,
    /// The input is not the field's.
    Ignored,
}

// ========================================================================
// ColorField Implementations
// ========================================================================

impl ColorField {
    /// A field showing black.
    pub fn new() -> Self {
        Self {
            draft: Draft::new(),
            value: Rgb::new(0, 0, 0),
        }
    }

    /// The color the field holds.
    pub fn value(&self) -> Rgb {
        self.value
    }

    /// Describes the field to screen readers as `id`.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        self.draft.access(frame, id);
    }

    /// The caret's cell, for an input method's candidates.
    pub fn cursor_area(&self) -> Option<Rect> {
        self.draft.cursor_area()
    }
}

impl Default for ColorField {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for ColorField {
    type Output = ();
    type Response = ColorFieldResponse;

    /// Edits the text; a text that reads as a new color changes it.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ColorFieldResponse {
        match self.draft.handle(input, cx, &hex(self.value)) {
            DraftEvent::Edited(text) => match parse_hex(&text) {
                Some(color) if color != self.value => {
                    self.value = color;
                    ColorFieldResponse::Changed(color)
                }
                Some(_) | None => ColorFieldResponse::Handled,
            },
            DraftEvent::Handled => ColorFieldResponse::Handled,
            DraftEvent::Ignored => ColorFieldResponse::Ignored,
        }
    }

    /// Draws the swatch, a cell row square, then the field in the rest of
    /// `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let side = area.height.min(area.width);
        frame.fill_rect(Rect::new(area.x, area.y, side, side), self.value);
        let gap = (SWATCH_GAP_CELLS * metrics.width).round();
        let left = side + gap;
        let field = Rect::new(
            area.x + left,
            area.y,
            (area.width - left).max(0.0),
            area.height,
        );
        self.draft.draw(frame, field, theme);
    }

    fn set_focused(&mut self, focused: bool) {
        self.draft.set_focused(focused);
    }

    fn is_focused(&self) -> bool {
        self.draft.is_focused()
    }
}

impl Control for ColorField {
    type Props<'a> = ColorFieldProps<'a>;
    type Value = Rgb;

    /// Takes the app's color, shown unless the user is typing one.
    fn sync(&mut self, props: ColorFieldProps<'_>) {
        self.value = props.value;
        self.draft.sync(props.label, &hex(props.value));
    }

    /// The field's height: one row and its frame.
    fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        self.draft.measure(width, metrics, scale_factor)
    }

    /// The color the user typed.
    fn changed(&self, response: ColorFieldResponse) -> Option<Rgb> {
        match response {
            ColorFieldResponse::Changed(color) => Some(color),
            ColorFieldResponse::Handled | ColorFieldResponse::Ignored => None,
        }
    }
}

impl Outcome for ColorFieldResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `color` as `#rrggbb`.
pub(crate) fn hex(color: Rgb) -> String {
    format!("{HEX_PREFIX}{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// The opaque color `text` writes as `#rrggbb` (or `#rrggbbff`), spaces
/// around it left out.
pub(crate) fn parse_hex(text: &str) -> Option<Rgb> {
    let Rgba { a, b, g, r } = Rgba::from_hex(text.trim()).ok()?;
    (a == u8::MAX).then_some(Rgb::new(r, g, b))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::{Key, KeyCode, Mods};
    use crate::testing::TestWindow;

    fn key(code: KeyCode) -> Input {
        Input::Key(Key::new(code, Mods::NONE))
    }

    fn typed(
        field: &mut ColorField,
        text: &str,
        window: &mut TestWindow,
    ) -> Vec<ColorFieldResponse> {
        text.chars()
            .map(|c| field.handle(&key(KeyCode::Char(c)), &mut window.cx()))
            .collect()
    }

    #[test]
    fn test_a_color_changes_once_its_text_reads_as_one_and_a_draft_is_kept_till_then() {
        let mut window = TestWindow::new(200, 40);
        let mut field = ColorField::new();
        field.sync(ColorFieldProps {
            label: None,
            value: Rgb::new(0, 0, 0),
        });
        field.set_focused(true);
        for _ in 0..7 {
            field.handle(&key(KeyCode::Backspace), &mut window.cx());
        }
        let responses = typed(&mut field, "#1a2b3c", &mut window);
        let changed = responses
            .iter()
            .filter(|response| matches!(response, ColorFieldResponse::Changed(_)))
            .count();
        assert_eq!(changed, 1, "only the whole color: {responses:?}");
        assert_eq!(field.value(), Rgb::new(0x1a, 0x2b, 0x3c));
        // A half-typed color survives the app's sync while focused.
        field.handle(&key(KeyCode::Backspace), &mut window.cx());
        field.sync(ColorFieldProps {
            label: None,
            value: field.value(),
        });
        assert_eq!(field.draft.text(), "#1a2b3", "the draft is kept");
        field.set_focused(false);
        field.sync(ColorFieldProps {
            label: None,
            value: field.value(),
        });
        assert_eq!(field.draft.text(), "#1a2b3c", "the color again once left");
    }

    #[test]
    fn test_hex_reads_back_what_it_writes_and_refuses_a_translucent_color() {
        let color = Rgb::new(0xab, 0x01, 0xff);
        assert_eq!(parse_hex(&hex(color)), Some(color));
        assert_eq!(parse_hex("#ab01ff80"), None);
        assert_eq!(parse_hex("#12"), None);
    }
}

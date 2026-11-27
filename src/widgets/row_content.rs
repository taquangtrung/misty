//! [`RowContent`]: what one row of a list-like widget shows when plain
//! text is not enough: a leading icon, styled text (match highlights, muted
//! parts), and short badges at the end, such as a file's git status letter
//! or a tag's colored dot. A [`List`](crate::widgets::List), a
//! [`Table`](crate::widgets::Table)'s cells, a [`TreeView`](crate::widgets::TreeView)'s
//! items, and an [`IconGrid`](crate::widgets::IconGrid)'s labels take it; their
//! sources give plain text unless they say more. Rows are set on the cell
//! grid unless the widget lays them out in the proportional font
//! ([`RowFont`]).

use super::super::app::frame::Frame;
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::render::icon::logical;
use super::super::render::image::Svg;
use super::super::render::paragraph::TextAlign;
use super::label::{fit_proportional, styled_paragraph};

// ========================================================================
// Constants
// ========================================================================

/// What a [`Badge::Dot`] draws: one cell wide.
const DOT_MARK: &str = "\u{25cf}";

/// The cells between the icon and the text.
const ICON_GAP_CELLS: f32 = 0.5;

/// The cells between two badges, and between the text and the badges.
const BADGE_GAP_CELLS: f32 = 1.0;

/// The space above and below the icon inside the row, in logical pixels.
const ICON_INSET: f32 = 2.0;

/// The color of plain row text.
const TEXT_COLOR: TextColor = TextColor::Token(ThemeToken::Foreground);

// ========================================================================
// Data Structures
// ========================================================================

/// What a row shows: text in styled runs, with an optional icon before it
/// and badges at the row's end.
///
/// ```
/// use misty::{Span, StyledText, ThemeToken};
/// use misty::widgets::{Badge, RowContent};
///
/// let row = RowContent::styled(
///     StyledText::new()
///         .with(Span::new("main", ThemeToken::Foreground.into()))
///         .with(Span::new(".rs", ThemeToken::Muted.into())),
/// )
/// .with_badge(Badge::Text(Span::new("M", ThemeToken::Warning.into())))
/// .with_badge(Badge::Dot(ThemeToken::Accent));
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RowContent {
    /// The badges at the end, left to right.
    badges: Vec<Badge>,
    /// The icon before the text.
    icon: Option<RowIcon>,
    /// The text.
    text: StyledText,
}

/// The icon at the start of a row: an SVG, tinted one theme color (a
/// symbolic icon) or drawn in its own colors (a colored file type icon).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowIcon {
    /// The document.
    svg: Svg,
    /// The theme color it is drawn in, or `None` for its own colors.
    tint: Option<ThemeToken>,
}

/// The font a widget sets its rows' text in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RowFont {
    /// On the cell grid, each character in its cells, so columns line up.
    #[default]
    Cells,
    /// In the proportional font, each character as wide as its shape.
    Proportional,
}

/// A short mark at the end of a row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Badge {
    /// A dot in a theme color, such as a tag's.
    Dot(ThemeToken),
    /// A short text, such as a git status letter (`M`, `A`, `?`).
    Text(Span),
}

// ========================================================================
// RowContent Implementations
// ========================================================================

impl RowContent {
    /// A row of `text` in the foreground color, with no icon or badges:
    /// what a source gives unless it says more.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::styled(StyledText::plain(text, TEXT_COLOR))
    }

    /// A row of styled `text`, such as a name with the characters a filter
    /// matched highlighted ([`fuzzy_highlight`](crate::fuzzy_highlight)).
    pub fn styled(text: StyledText) -> Self {
        Self {
            badges: Vec::new(),
            icon: None,
            text,
        }
    }

    /// This row with `icon` before its text.
    pub fn with_icon(self, icon: RowIcon) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// This row with `badge` after the badges it has.
    pub fn with_badge(mut self, badge: Badge) -> Self {
        self.badges.push(badge);
        self
    }
}

// ========================================================================
// RowIcon Implementations
// ========================================================================

impl RowIcon {
    /// `svg` drawn in the theme color `tint`, as a symbolic icon (a folder
    /// outline) is.
    pub fn tinted(svg: Svg, tint: ThemeToken) -> Self {
        Self {
            svg,
            tint: Some(tint),
        }
    }

    /// `svg` drawn in its own colors, as a colored file type icon is.
    pub fn colored(svg: Svg) -> Self {
        Self { svg, tint: None }
    }
}

// ========================================================================
// Badge Implementations
// ========================================================================

impl Badge {
    /// The badge as text to draw.
    fn styled(&self) -> StyledText {
        match self {
            Self::Dot(token) => StyledText::plain(DOT_MARK, TextColor::Token(*token)),
            Self::Text(span) => StyledText::from(span.clone()),
        }
    }
}

// ========================================================================
// RowFont Implementations
// ========================================================================

impl RowFont {
    /// How wide `text` is in this font, in pixels.
    fn width(self, frame: &Frame, text: &StyledText) -> f32 {
        match self {
            Self::Cells => text.width() as f32 * frame.cell_metrics().width,
            Self::Proportional => frame.paragraph_width(&styled_paragraph(text)),
        }
    }

    /// `text` cut at a whole character to fit `room` pixels in this font,
    /// ending in an ellipsis when it asked for one.
    fn fit(self, frame: &Frame, text: &StyledText, room: f32) -> StyledText {
        match self {
            Self::Cells => text.fit(frame.cell_metrics().cols_in(room)),
            Self::Proportional => fit_proportional(frame, text, room),
        }
    }

    /// Draws `text`, already cut to its room, on the line a cell tall whose
    /// top left corner is `origin`, only what falls inside `clip`.
    fn draw(self, frame: &mut Frame, text: &StyledText, origin: Point, clip: Rect, theme: &Theme) {
        match self {
            Self::Cells => {
                frame.draw_text(text, origin, clip, theme);
            }
            Self::Proportional => {
                let paragraph = styled_paragraph(text);
                // A pixel to spare, so rounding never wraps the last word.
                let width = frame.paragraph_width(&paragraph).ceil() + 1.0;
                let rect = Rect::new(origin.x, origin.y, width, frame.cell_metrics().height);
                frame.clipped(clip, |frame| {
                    frame.draw_paragraph(&paragraph, rect, theme);
                });
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Draws `icon` filling `square`: tinted as it says, or in its own colors.
pub(crate) fn draw_row_icon(frame: &mut Frame, icon: &RowIcon, square: Rect, theme: &Theme) {
    match icon.tint {
        Some(tint) => frame.draw_icon(&icon.svg, square, theme.color(tint)),
        None => frame.draw_svg(&icon.svg, square),
    }
}

/// Draws `content` on one line in `rect` with its text in `font`: the
/// badges at its right end, as many as fit, and the icon and text placed
/// by `align` in the room left, the text cut with an ellipsis.
pub(crate) fn draw_content_in(
    frame: &mut Frame,
    content: &RowContent,
    rect: Rect,
    align: TextAlign,
    theme: &Theme,
    font: RowFont,
) {
    if rect.width <= 0.0 {
        return;
    }
    let metrics = frame.cell_metrics();
    let mut right = rect.x + rect.width;
    for badge in content.badges.iter().rev() {
        let text = badge.styled();
        let width = font.width(frame, &text);
        let x = (right - width).round();
        if x < rect.x {
            break;
        }
        let clip = Rect::new(x, rect.y, width.ceil(), rect.height);
        font.draw(frame, &text, Point::new(x, rect.y), clip, theme);
        right = x - BADGE_GAP_CELLS * metrics.width;
    }
    let room = (right - rect.x).max(0.0);
    let icon_room = match content.icon {
        Some(_) => metrics.height + (ICON_GAP_CELLS * metrics.width).round(),
        None => 0.0,
    };
    let line = font.fit(
        frame,
        &content.text.clone().with_ellipsis(),
        (room - icon_room).max(0.0),
    );
    let block = icon_room + font.width(frame, &line);
    let spare = (room - block).max(0.0);
    let x = match align {
        TextAlign::Justify | TextAlign::Start => rect.x,
        TextAlign::Center => rect.x + (spare / 2.0).floor(),
        TextAlign::End => rect.x + spare,
    };
    if let Some(icon) = &content.icon
        && icon_room <= room
    {
        let inset = logical(ICON_INSET, frame.scale_factor());
        let side = (metrics.height - 2.0 * inset).max(0.0);
        let top = (rect.y + (rect.height - side) / 2.0).round();
        let square = Rect::new((x + inset).round(), top, side, side);
        draw_row_icon(frame, icon, square, theme);
    }
    let text_x = (x + icon_room).round();
    let clip = Rect::new(text_x, rect.y, (right - text_x).max(0.0), rect.height);
    font.draw(frame, &line, Point::new(text_x, rect.y), clip, theme);
}

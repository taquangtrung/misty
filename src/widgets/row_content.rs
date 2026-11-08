//! [`RowContent`]: what one row of a list-like widget shows when plain
//! text is not enough: a leading icon, styled text (match highlights, muted
//! parts), and short badges at the end, such as a file's git status letter
//! or a tag's colored dot. A [`List`](crate::List), a
//! [`Table`](crate::Table)'s cells, a [`TreeView`](crate::TreeView)'s
//! items, and an [`IconGrid`](crate::IconGrid)'s labels take it; their
//! sources give plain text unless they say more.

use super::super::app::frame::Frame;
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::render::icon::logical;
use super::super::render::image::Svg;
use super::super::render::paragraph::TextAlign;

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
/// use misty::{Badge, RowContent, Span, StyledText, ThemeToken};
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
// Functions
// ========================================================================

/// Draws `content` on one line in `rect`: the badges at its right end, as
/// many as fit, and the icon and text placed by `align` in the room left,
/// the text cut with an ellipsis.
pub(crate) fn draw_content(
    frame: &mut Frame,
    content: &RowContent,
    rect: Rect,
    align: TextAlign,
    theme: &Theme,
) {
    if rect.width <= 0.0 {
        return;
    }
    let metrics = frame.cell_metrics();
    let mut right = rect.x + rect.width;
    for badge in content.badges.iter().rev() {
        let text = badge.styled();
        let width = text.width() as f32 * metrics.width;
        let x = (right - width).round();
        if x < rect.x {
            break;
        }
        let clip = Rect::new(x, rect.y, width, rect.height);
        frame.draw_text(&text, Point::new(x, rect.y), clip, theme);
        right = x - BADGE_GAP_CELLS * metrics.width;
    }
    let room = (right - rect.x).max(0.0);
    let icon_room = match content.icon {
        Some(_) => metrics.height + (ICON_GAP_CELLS * metrics.width).round(),
        None => 0.0,
    };
    let line = content
        .text
        .clone()
        .with_ellipsis()
        .fit(metrics.cols_in((room - icon_room).max(0.0)));
    let block = icon_room + line.width() as f32 * metrics.width;
    let spare = (room - block).max(0.0);
    let x = match align {
        TextAlign::Start => rect.x,
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
        match icon.tint {
            Some(tint) => frame.draw_icon(&icon.svg, square, theme.color(tint)),
            None => frame.draw_svg(&icon.svg, square),
        }
    }
    let text_x = (x + icon_room).round();
    let clip = Rect::new(text_x, rect.y, (right - text_x).max(0.0), rect.height);
    frame.draw_text(&line, Point::new(text_x, rect.y), clip, theme);
}

//! How a [`SettingsPage`] is drawn: the page over its backdrop, the scope
//! tabs, the sidebar of sections, the filter line, the rows with their
//! values, and the footer of hints.

use std::time::Instant;

use super::super::super::app::frame::Frame;
use super::super::super::app::widget::Widget;
use super::super::super::base::box_style::{Border, BoxStyle};
use super::super::super::base::color::Rgba;
use super::super::super::base::geometry::{Point, Rect};
use super::super::super::base::key::key_label;
use super::super::super::base::strings::{self, Strings};
use super::super::super::base::styled::{Span, StyledText, TextColor};
use super::super::super::base::text;
use super::super::super::base::theme::{Theme, ThemeToken};
use super::super::super::render::font::CellMetrics;
use super::super::super::render::icon::logical;
use super::super::command::{CommandKeys, CommandRegistry, KeyConflict, KeyScope};
use super::super::glide::rows_in_sight;
use super::super::label::{draw_styled_label, label_width};
use super::super::popup::{Popup, draw_backdrop};
use super::model::SettingKind;
use super::{Editing, Focus, PageLayout, Row, Section, Setting, SettingValue, SettingsPage};

// ========================================================================
// Constants
// ========================================================================

/// The widest the page grows, in cells.
const MAX_COLS: usize = 96;

/// The tallest the page grows, in rows.
const MAX_ROWS: usize = 28;

/// The blank margin kept between the page and the window's edges, in
/// cells.
const WINDOW_MARGIN_CELLS: f32 = 2.0;

/// The width of the sidebar's section names, in cells.
const SIDEBAR_COLS: usize = 18;

/// The width of the value column at the right of each row, in cells.
const VALUE_COLS: usize = 28;

/// The cells between a row's label and its value.
const LABEL_GAP_CELLS: usize = 2;

/// The cells a color swatch takes, before the color's hex.
const SWATCH_CELLS: usize = 2;

/// The thickness of separator lines and the width of the focus bar and
/// the caret, in logical pixels.
const STROKE: f32 = 1.0;

/// The width of the accent bar marking the focused row, in logical pixels.
const FOCUS_BAR: f32 = 2.0;

/// What starts the keybindings filter line.
const FILTER_PROMPT: &str = "/ ";

/// The blank cells on each side of the Cancel button's label.
const CANCEL_PADDING_CELLS: usize = 1;

/// What separates a command's key sequences.
const SEQUENCE_SEPARATOR: &str = ", ";

/// What marks a row that has a value of its own in a later scope.
const OVERRIDDEN_MARK: &str = " ●";

/// What separates the browse hint from the scopes' hint.
const HINT_SEPARATOR: &str = "; ";

/// The blank cells at each side of a scope tab's name.
const SCOPE_PADDING_CELLS: usize = 1;

/// What a toggle shows when on.
const TOGGLE_ON: &str = "[x] ";

/// What a toggle shows when off.
const TOGGLE_OFF: &str = "[ ] ";

/// What surrounds a choice or a number, which Left and Right step.
const STEP_LEFT: &str = "< ";

/// What closes a choice or a number.
const STEP_RIGHT: &str = " >";

/// Text in the foreground color.
const LABEL_COLOR: TextColor = TextColor::Token(ThemeToken::Foreground);

/// Secondary text.
const QUIET_COLOR: TextColor = TextColor::Token(ThemeToken::Muted);

/// Text that marks focus or a pending capture.
const ACCENT_COLOR: TextColor = TextColor::Token(ThemeToken::Accent);

/// Keys claimed by more than one command.
const CONFLICT_COLOR: TextColor = TextColor::Token(ThemeToken::Danger);

// ========================================================================
// SettingsPage Implementations
// ========================================================================

impl SettingsPage {
    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws the page centered in `area` in the frame's overlay over a
    /// dimmed backdrop, the keybindings section from `registry`, and
    /// returns where the page itself was drawn.
    pub(super) fn draw_page(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        registry: &CommandRegistry,
        theme: &Theme,
    ) -> Rect {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let window = area;
        let (cell, row_height) = (metrics.width, metrics.height);
        let padding = cell.round();
        let inset = Popup::inset(&theme.shape, scale_factor);
        let stroke = logical(STROKE, scale_factor);
        let margin = WINDOW_MARGIN_CELLS * cell;
        let width = (window.width - 2.0 * margin)
            .min(MAX_COLS as f32 * cell + 2.0 * inset)
            .max(0.0)
            .floor();
        let height = (window.height - 2.0 * margin)
            .min(MAX_ROWS as f32 * row_height + 2.0 * (inset + stroke))
            .max(0.0)
            .floor();
        let x = (window.x + (window.width - width) / 2.0).round();
        let y = (window.y + (window.height - height) / 2.0).round();
        let popup = Popup::with_rect(Rect::new(x, y, width, height));
        let content = popup.content(&theme.shape, scale_factor);
        let body_top = content.y + row_height + stroke;
        let footer_y = content.y + content.height - row_height;
        let body_height = (footer_y - stroke - body_top).max(0.0);
        let sidebar_width = SIDEBAR_COLS as f32 * cell + 2.0 * padding;
        let sidebar = Rect::new(content.x, body_top, sidebar_width, body_height);
        let panel_x = sidebar.x + sidebar.width + stroke;
        let panel = Rect::new(
            panel_x,
            body_top,
            (content.x + content.width - panel_x).max(0.0),
            body_height,
        );
        let is_bindings = matches!(
            self.sections.get(self.section),
            Some(Section::Keybindings(_))
        );
        let rows = match is_bindings {
            true => Rect::new(
                panel.x,
                panel.y + row_height,
                panel.width,
                (panel.height - row_height).max(0.0),
            ),
            false => panel,
        };
        self.rows.set_count(self.row_count(registry));
        let visible = metrics.rows_in(rows.height);
        self.rows.set_visible(visible);
        self.glide.set_reduced_motion(frame.reduced_motion());
        let offset = self
            .glide
            .follow(self.rows.scroll(), visible, Instant::now());
        self.layout = Some(PageLayout {
            page: popup.rect(),
            row_height,
            rows,
            sidebar,
        });
        draw_backdrop(frame, window, theme);
        popup.draw(frame, theme);
        let conflicts = registry.conflicts(KeyScope::All);
        frame.overlay(|frame| {
            let heading_clip = Rect::new(
                content.x + padding,
                content.y,
                (content.width - 2.0 * padding).max(0.0),
                row_height,
            );
            let heading_clip = self.draw_scope_tabs(frame, heading_clip, theme);
            let heading = StyledText::from(Span::new(self.title.as_str(), LABEL_COLOR).bold());
            draw_styled_label(
                frame,
                theme,
                &heading.with_ellipsis(),
                Point::new(heading_clip.x, content.y),
                heading_clip,
            );
            let lines = [
                Rect::new(content.x, body_top - stroke, content.width, stroke),
                Rect::new(content.x, footer_y - stroke, content.width, stroke),
                Rect::new(sidebar.x + sidebar.width, body_top, stroke, body_height),
            ];
            for line in lines {
                frame.fill_rect_over_text(line, theme.border);
            }
            self.draw_sidebar(frame, sidebar, theme);
            if is_bindings {
                let filter = Rect::new(panel.x, panel.y, panel.width, row_height);
                self.draw_filter(frame, filter, theme);
            }
            frame.clipped(rows, |frame| {
                self.draw_rows(frame, rows, offset, registry, &conflicts, theme);
            });
            let footer = Rect::new(
                content.x + padding,
                footer_y,
                (content.width - 2.0 * padding).max(0.0),
                row_height,
            );
            let hint = StyledText::plain(
                self.footer(registry, &conflicts, &theme.strings),
                QUIET_COLOR,
            );
            draw_styled_label(
                frame,
                theme,
                &hint.with_ellipsis(),
                Point::new(footer.x, footer.y),
                footer,
            );
        });
        popup.rect()
    }

    // --------------------------------------------------------------------
    // Drawing Parts
    // --------------------------------------------------------------------

    /// Draws the scope tabs at the right of `area`, the heading's line,
    /// the shown one in the accent and lined with it, and returns the room
    /// left for the heading at their left.
    fn draw_scope_tabs(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        self.scope_tabs.clear();
        if self.scopes.len() < 2 {
            return area;
        }
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let widths: Vec<f32> = self
            .scopes
            .iter()
            .map(|name| {
                let padding = 2.0 * SCOPE_PADDING_CELLS as f32 * metrics.width;
                (label_width(frame, theme, name) + padding).round()
            })
            .collect();
        let mut x = area.x + area.width - widths.iter().sum::<f32>();
        for (index, (name, width)) in self.scopes.iter().zip(&widths).enumerate() {
            let tab = Rect::new(x, area.y, *width, area.height);
            let color = match index == self.scope {
                true => {
                    let style = BoxStyle {
                        border: Some(Border {
                            color: Rgba::from(theme.accent),
                            width: logical(STROKE, scale_factor),
                        }),
                        ..theme.shape.highlight(theme.surface, scale_factor)
                    };
                    frame.draw_box(tab, &style);
                    ACCENT_COLOR
                }
                false => QUIET_COLOR,
            };
            let label = StyledText::plain(name.as_str(), color);
            let label_x = x + SCOPE_PADDING_CELLS as f32 * metrics.width;
            draw_styled_label(frame, theme, &label, Point::new(label_x, area.y), tab);
            self.scope_tabs.push(tab);
            x += width;
        }
        let left = self.scope_tabs.first().map_or(area.x, |tab| tab.x);
        Rect::new(
            area.x,
            area.y,
            (left - metrics.width - area.x).max(0.0),
            area.height,
        )
    }

    /// Draws the section names in `area`, the shown one lit.
    fn draw_sidebar(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let padding = metrics.width.round();
        let bar = logical(FOCUS_BAR, frame.scale_factor());
        for (index, section) in self.sections.iter().enumerate() {
            let y = area.y + index as f32 * metrics.height;
            if y + metrics.height > area.y + area.height {
                break;
            }
            let rect = Rect::new(area.x, y, area.width, metrics.height);
            if index == self.section {
                let fill = match self.focus {
                    Focus::Rows => theme.hover,
                    Focus::Sections => theme.selection,
                };
                frame.draw_box(rect, &theme.shape.highlight(fill, frame.scale_factor()));
                if self.focus == Focus::Sections {
                    frame.fill_rect_over_text(
                        Rect::new(area.x, y, bar, metrics.height),
                        theme.accent,
                    );
                }
            }
            let title = match section {
                Section::Keybindings(title) => title.as_str(),
                Section::Settings(section) => section.title.as_str(),
            };
            let clip = Rect::new(
                area.x + padding,
                y,
                (area.width - 2.0 * padding).max(0.0),
                metrics.height,
            );
            let text = StyledText::plain(title, LABEL_COLOR).with_ellipsis();
            draw_styled_label(frame, theme, &text, Point::new(clip.x, y), clip);
        }
    }

    /// Draws the keybindings filter line in `area`.
    fn draw_filter(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let padding = metrics.width.round();
        let clip = Rect::new(
            area.x + padding,
            area.y,
            (area.width - 2.0 * padding).max(0.0),
            area.height,
        );
        let is_filtering = matches!(self.editing, Some(Editing::Filter));
        self.filter.set_focused(is_filtering);
        if !is_filtering && self.filter.text().is_empty() {
            let text = StyledText::plain(
                theme.strings.settings_filter_placeholder.as_ref(),
                QUIET_COLOR,
            );
            frame.draw_text(&text, Point::new(clip.x, area.y), clip, theme);
            return;
        }
        let prompt = StyledText::plain(FILTER_PROMPT, ACCENT_COLOR);
        frame.draw_text(&prompt, Point::new(clip.x, area.y), clip, theme);
        let prompt_width = text::width(FILTER_PROMPT) as f32 * metrics.width;
        let field = Rect::new(
            clip.x + prompt_width,
            clip.y,
            (clip.width - prompt_width).max(0.0),
            clip.height,
        );
        self.filter.draw(frame, field, theme);
    }

    /// Draws the rows in sight of the shown section in `area`, the top
    /// edge at row `offset`.
    fn draw_rows(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        offset: f32,
        registry: &CommandRegistry,
        conflicts: &[KeyConflict],
        theme: &Theme,
    ) {
        let metrics = frame.cell_metrics();
        let padding = metrics.width.round();
        let bar = logical(FOCUS_BAR, frame.scale_factor());
        // Taken while the rows borrow the page, for the field to draw.
        let mut editing = self.editing.take();
        let rows = self.rows_of(registry);
        let value_width = (VALUE_COLS as f32 * metrics.width).min(area.width / 2.0);
        let value_x = (area.x + area.width - padding - value_width).round();
        let in_sight = rows_in_sight(offset, metrics.rows_in(area.height), rows.len());
        for (index, row) in rows
            .iter()
            .enumerate()
            .take(in_sight.end)
            .skip(in_sight.start)
        {
            let y = (area.y + (index as f32 - offset) * metrics.height).round();
            let rect = Rect::new(area.x, y, area.width, metrics.height);
            let is_focused = index == self.rows.cursor() && self.focus == Focus::Rows;
            if is_focused {
                frame.draw_box(
                    rect,
                    &theme.shape.highlight(theme.hover, frame.scale_factor()),
                );
                frame.fill_rect_over_text(Rect::new(area.x, y, bar, metrics.height), theme.accent);
            }
            let label_right = value_x - LABEL_GAP_CELLS as f32 * metrics.width;
            let label_clip = Rect::new(
                area.x + padding,
                y,
                (label_right - area.x - padding).max(0.0),
                metrics.height,
            );
            let value_clip = Rect::new(value_x, y, value_width, metrics.height);
            let label = match row {
                Row::Binding(binding) => StyledText::new()
                    .with(Span::new(format!("{}: ", binding.group), QUIET_COLOR))
                    .with(Span::new(binding.title, LABEL_COLOR)),
                Row::Setting(setting) => {
                    let label = StyledText::plain(setting.label(), LABEL_COLOR);
                    match self.overridden.contains(setting.id()) {
                        true => label.with(Span::new(OVERRIDDEN_MARK, ThemeToken::Warning.into())),
                        false => label,
                    }
                }
            };
            draw_styled_label(
                frame,
                theme,
                &label.with_ellipsis(),
                Point::new(label_clip.x, y),
                label_clip,
            );
            let editing = editing.as_mut().filter(|_| is_focused);
            match (row, editing) {
                (Row::Binding(_), Some(Editing::Capture)) => {
                    let cancel = label_width(frame, theme, &theme.strings.cancel);
                    let button = cancel_button(area, y, metrics, cancel);
                    let prompt_clip =
                        Rect::new(value_x, y, (button.x - value_x).max(0.0), metrics.height);
                    let text = StyledText::from(
                        Span::new(theme.strings.key_prompt.as_ref(), ACCENT_COLOR).bold(),
                    );
                    draw_styled_label(frame, theme, &text, Point::new(value_x, y), prompt_clip);
                    let is_hovered = self.pointer.is_some_and(|point| button.contains(point));
                    let fill = match is_hovered {
                        true => theme.selection,
                        false => theme.surface,
                    };
                    let scale_factor = frame.scale_factor();
                    let style = BoxStyle {
                        border: Some(Border {
                            color: Rgba::from(theme.border),
                            width: logical(STROKE, scale_factor),
                        }),
                        ..theme.shape.highlight(fill, scale_factor)
                    };
                    frame.draw_box(button, &style);
                    let label = StyledText::plain(theme.strings.cancel.as_ref(), LABEL_COLOR);
                    let label_x = button.x + CANCEL_PADDING_CELLS as f32 * metrics.width;
                    draw_styled_label(frame, theme, &label, Point::new(label_x, y), button);
                }
                (Row::Binding(binding), _) => {
                    let text = binding_text(binding, conflicts, &theme.strings);
                    frame.draw_text(&text, Point::new(value_x, y), value_clip, theme);
                }
                (Row::Setting(_), Some(Editing::Field(line))) => {
                    frame.fill_rect(value_clip, theme.background);
                    line.draw(frame, value_clip, theme);
                }
                (Row::Setting(setting), _) => {
                    draw_value(frame, setting, value_clip, theme);
                }
            }
        }
        self.editing = editing;
    }

    /// What the footer says: how to edit what has focus, the focused
    /// setting's description, or the commands sharing the focused keys.
    /// The words come from `words`.
    pub(super) fn footer(
        &self,
        registry: &CommandRegistry,
        conflicts: &[KeyConflict],
        words: &Strings,
    ) -> String {
        match &self.editing {
            Some(Editing::Capture) => return words.settings_capture_hint.to_string(),
            Some(Editing::Field(_)) if self.escape_keeps => {
                return words.settings_field_kept_hint.to_string();
            }
            Some(Editing::Field(_)) => return words.settings_field_hint.to_string(),
            Some(Editing::Filter) => return words.settings_filter_hint.to_string(),
            None => {}
        }
        if self.focus == Focus::Rows {
            if let Some(binding) = self.focused_binding(registry)
                && let Some(shared) = conflict_note(&binding, conflicts, registry, words)
            {
                return shared;
            }
            if let Some(setting) = self.focused_setting()
                && self.overridden.contains(setting.id())
            {
                return match self.scope {
                    0 => words.settings_overridden.to_string(),
                    _ => words.settings_own_value.to_string(),
                };
            }
            if let Some(description) = self.focused_setting().and_then(Setting::description) {
                return description.to_owned();
            }
            let offered = self.focused_setting().map(Setting::names_offered);
            if let Some(offered) = offered.filter(|offered| !offered.is_empty()) {
                return strings::fill(&words.settings_names_hint, &[&offered.join(" ")]);
            }
        }
        match self.scopes.len() > 1 {
            true => format!(
                "{}{HINT_SEPARATOR}{}",
                words.settings_browse_hint, words.settings_scopes_hint
            ),
            false => words.settings_browse_hint.to_string(),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where the Cancel button of a capture on the row at `y` of the rows
/// `area` is drawn, in cells of `metrics`: at the right end of the value
/// column, its label (`label` pixels wide, as drawn in the chrome font)
/// padded on both sides.
pub(super) fn cancel_button(area: Rect, y: f32, metrics: CellMetrics, label: f32) -> Rect {
    let padding = metrics.width.round();
    let width = (label + 2.0 * CANCEL_PADDING_CELLS as f32 * metrics.width).ceil();
    let x = (area.x + area.width - padding - width).round();
    Rect::new(x, y, width, metrics.height)
}

/// The keys of `binding` as its row shows them: `words`' unbound word
/// when it has none, in the danger color when another command claims one
/// of them, and bold when they are the user's.
fn binding_text(binding: &CommandKeys, conflicts: &[KeyConflict], words: &Strings) -> StyledText {
    if binding.keys.is_empty() {
        return StyledText::plain(words.unbound.as_ref(), QUIET_COLOR);
    }
    let label = binding
        .keys
        .iter()
        .map(|sequence| key_label(sequence, words))
        .collect::<Vec<_>>()
        .join(SEQUENCE_SEPARATOR);
    let color = match is_conflicted(binding, conflicts) {
        true => CONFLICT_COLOR,
        false => LABEL_COLOR,
    };
    let span = Span::new(label, color);
    let span = match binding.overridden {
        true => span.bold(),
        false => span,
    };
    StyledText::from(span).with_ellipsis()
}

/// Whether another command claims one of `binding`'s keys.
pub(super) fn is_conflicted(binding: &CommandKeys, conflicts: &[KeyConflict]) -> bool {
    conflicts
        .iter()
        .any(|conflict| conflict.commands.contains(&binding.id))
}

/// The footer naming the other commands that claim `binding`'s keys in
/// `words`, or `None` when none do.
fn conflict_note(
    binding: &CommandKeys,
    conflicts: &[KeyConflict],
    registry: &CommandRegistry,
    words: &Strings,
) -> Option<String> {
    let conflict = conflicts
        .iter()
        .find(|conflict| conflict.commands.contains(&binding.id))?;
    let others: Vec<&str> = conflict
        .commands
        .iter()
        .filter(|id| **id != binding.id)
        .map(|id| registry.command(id).map_or(*id, |command| command.title))
        .collect();
    let keys = key_label(&conflict.keys, words);
    let others = others.join(SEQUENCE_SEPARATOR);
    Some(strings::fill(&words.key_conflict, &[&keys, &others]))
}

/// Draws the value of `setting` in `area`: a check box, a choice or number
/// between step marks, text, or a color swatch and its hex; bold when it
/// differs from the default.
fn draw_value(frame: &mut Frame, setting: &Setting, area: Rect, theme: &Theme) {
    let metrics = frame.cell_metrics();
    let value = setting.value_text(&theme.strings);
    let styled = |text: String, color: TextColor| {
        let span = Span::new(text, color);
        match setting.is_modified() {
            true => span.bold(),
            false => span,
        }
    };
    let text = match (setting.kind(), setting.value()) {
        (SettingKind::Toggle, SettingValue::Toggle(on)) => {
            let (mark, color) = match on {
                true => (TOGGLE_ON, ACCENT_COLOR),
                false => (TOGGLE_OFF, QUIET_COLOR),
            };
            StyledText::new()
                .with(Span::new(mark, color))
                .with(styled(value, LABEL_COLOR))
        }
        (SettingKind::Choice(_) | SettingKind::Number(_), _) => StyledText::new()
            .with(Span::new(STEP_LEFT, QUIET_COLOR))
            .with(styled(value, LABEL_COLOR))
            .with(Span::new(STEP_RIGHT, QUIET_COLOR)),
        (SettingKind::Color, SettingValue::Color(color)) => {
            let inset = (metrics.height / 4.0).round();
            let swatch = Rect::new(
                area.x,
                area.y + inset,
                SWATCH_CELLS as f32 * metrics.width,
                (metrics.height - 2.0 * inset).max(1.0),
            );
            // The swatch shows the user's own color, the value itself.
            frame.fill_rect(swatch, *color);
            let gap = Span::new(" ".repeat(SWATCH_CELLS + 1), LABEL_COLOR);
            StyledText::new().with(gap).with(styled(value, LABEL_COLOR))
        }
        (
            SettingKind::Color | SettingKind::Names(_) | SettingKind::Text | SettingKind::Toggle,
            _,
        ) => StyledText::from(styled(value, LABEL_COLOR)),
    };
    frame.draw_text(
        &text.with_ellipsis(),
        Point::new(area.x, area.y),
        area,
        theme,
    );
}

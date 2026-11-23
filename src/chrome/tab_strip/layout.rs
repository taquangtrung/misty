//! Where the parts of a tab strip are, as plain geometry: which tabs fit,
//! each tab's body, icon, and close button, the new-tab button, and what a point
//! hits. No drawing happens here, so the layout can be tested alone and the
//! drawing and the hit testing always agree.

use super::super::super::base::geometry::{Point, Rect};
use super::{Tab, TabClose, TabStripHit, TabStyle, TabWidth};
use crate::render::font::CellMetrics;

// ========================================================================
// Constants
// ========================================================================

/// The strip's height as a multiple of the cell height.
const STRIP_HEIGHT_RATIO: f32 = 1.6;

/// The close button's side as a share of the strip's height.
const CLOSE_SHARE: f32 = 0.6;

/// The fewest cells of room a tab makes for its title, so a short title
/// still gives a tab wide enough to aim at.
const MIN_TITLE_CELLS: usize = 6;

/// The most cells of room a tab makes for its title; a longer title ends
/// in an ellipsis.
const MAX_TITLE_CELLS: usize = 20;

/// The cells a tab's icon or spinner takes before its title, the gap after
/// it included.
const LEAD_CELLS: usize = 2;

/// The icon's side as a share of the cell height.
const ICON_SHARE: f32 = 0.75;

/// The pixels a rounded width may have lost, given back to the title's
/// room so a title the tab was sized for is never cut.
const ROUNDING_SLACK: f32 = 0.5;

/// The prev and next buttons when both overflow directions show.
const BOTH_STEP_BUTTONS: usize = 2;

/// How much of the narrowest tab shows at the left end of an overflowing strip
/// when earlier tabs are hidden, so the icon and start of the title remain legible.
const LEFT_PEEK_SHARE: f32 = 0.8;

/// How much of the narrowest tab shows at the right end of an overflowing strip
/// past which tabs are hidden, so the strip plainly goes on.
const PEEK_SHARE: f32 = 0.6;

/// Minimum width in logical pixels for a peeking tab at either end of an
/// overflowing strip, so it stays legible even with a compact font.
const MIN_PEEK_WIDTH: f32 = 60.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Where the parts of a tab strip are, in physical pixels. Only the tabs
/// that fit are laid out: the strip scrolls by whole tabs, starting at
/// `first`, with part of the tab before and after them showing at an end
/// past which tabs are hidden.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabStripLayout {
    /// The whole strip.
    pub(crate) bar: Rect,
    /// The index of the first tab shown.
    pub(crate) first: usize,
    /// The largest `first` worth scrolling to: from it on, every remaining
    /// tab fits.
    pub(crate) last_first: usize,
    /// The new-tab button, just after the last tab shown (and any overflow
    /// navigation buttons), or `None` on a strip without one.
    pub(crate) new_tab: Option<Rect>,
    /// The next-tab button when tabs overflow, or `None` when all tabs fit.
    pub(crate) next_tab: Option<Rect>,
    /// The previous-tab button when tabs overflow, or `None` when all tabs fit.
    pub(crate) prev_tab: Option<Rect>,
    /// Physical pixels per logical pixel.
    pub(crate) scale_factor: f64,
    /// The tabs shown, left to right.
    pub(crate) slots: Vec<TabSlot>,
}

/// How a strip's tabs are drawn: where their close buttons show and how
/// wide they are.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TabOptions {
    /// Which tabs show their close button.
    pub(crate) close: TabClose,
    /// The fewest logical pixels wide any tab may be, or `0.0` for the
    /// title-based minimum alone.
    pub(crate) min_width: f32,
    /// Whether the strip goes without its new-tab button.
    pub(crate) no_new_tab: bool,
    /// How the tabs that are not active are drawn.
    pub(crate) style: TabStyle,
    /// How wide each tab is.
    pub(crate) width: TabWidth,
}

/// Where one shown tab is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabSlot {
    /// The whole tab, which activates it.
    pub(crate) body: Rect,
    /// The close button inside the tab, at its right.
    pub(crate) close: Rect,
    /// The tab's index among all tabs.
    pub(crate) index: usize,
    /// Whether only part of the tab shows, at an end of an overflowing
    /// strip: `body` is the part shown, its start, while the title, icon,
    /// and close button keep their places in the whole tab, cut to `body`.
    pub(crate) peek: bool,
    /// Where the icon or the spinner goes, a square before the title, or
    /// `None` for a tab showing neither.
    pub(crate) lead: Option<Rect>,
    /// How wide the title may be drawn before it is cut, in pixels: the
    /// room the tab was sized for, less what cutting it to the strip took.
    pub(crate) title_room: f32,
    /// Where the title text starts.
    pub(crate) title_origin: Point,
}

/// The sizes every tab shares, in physical pixels.
#[derive(Clone, Copy, Debug)]
struct TabSizes {
    /// The close button's side.
    close: f32,
    /// One cell's width.
    cell: f32,
    /// The icon's side.
    icon: f32,
    /// Whether each tab is a square showing only its icon.
    icon_only: bool,
    /// The strip's height.
    height: f32,
    /// The space left of the title and right of the close button.
    padding: f32,
    /// How far below the strip's top the title's line box starts.
    title_top: f32,
}

// ========================================================================
// TabStripLayout Implementations
// ========================================================================

/// How tall a strip is for cells of `metrics`.
pub(crate) fn strip_height(metrics: CellMetrics) -> f32 {
    TabSizes::new(metrics, TabClose::default()).height
}

impl TabStripLayout {
    /// The layout of `tabs` along the top of `area` in cells of `metrics`
    /// on a display scaled by `scale_factor`, drawn as `options` says,
    /// their titles `title_widths` wide (one for each tab, as labels in the
    /// chrome font, in pixels), scrolled to start at `first` or as close to
    /// it as still fills the strip, and scrolled further if needed to show
    /// the tab at index `reveal`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        area: Rect,
        metrics: CellMetrics,
        scale_factor: f64,
        options: TabOptions,
        tabs: &[Tab],
        title_widths: &[f32],
        first: usize,
        reveal: Option<usize>,
    ) -> Self {
        let sizes = TabSizes {
            icon_only: options.width == TabWidth::Icon,
            ..TabSizes::new(metrics, options.close)
        };
        let bar = Rect::new(area.x, area.y, area.width, sizes.height.min(area.height));
        let button_width = sizes.height;
        let new_tab_width = if options.no_new_tab {
            0.0
        } else {
            button_width
        };
        let step_buttons = BOTH_STEP_BUTTONS as f32 * button_width;
        // Part of a tab shows at each end only where a whole tab still fits
        // between the two.
        let narrowest = sizes.tab_width(0.0, false);
        let min_peek = (MIN_PEEK_WIDTH * scale_factor as f32).round();
        let left_peek = (narrowest * LEFT_PEEK_SHARE).round().max(min_peek);
        let right_peek = (narrowest * PEEK_SHARE).round().max(min_peek);
        let between = area.width - step_buttons - new_tab_width - left_peek - right_peek;
        let (left_peek, right_peek) = if between >= narrowest {
            (left_peek, right_peek)
        } else {
            (0.0, 0.0)
        };
        let full_viewport = (area.width - new_tab_width).max(0.0);
        let full_widths = tab_widths(
            tabs,
            title_widths,
            sizes,
            options,
            scale_factor,
            full_viewport,
        );
        // Exact shares add up to the strip give or take a rounding error,
        // which must not count as overflowing it.
        let overflowing = last_first(&full_widths, full_viewport + ROUNDING_SLACK) > 0;
        let (first, last_first, viewport, widths, prev_tab, has_next) = match overflowing {
            false => (0, 0, full_viewport, full_widths, None, false),
            true => {
                let end_viewport = (area.width - button_width - new_tab_width).max(0.0);
                let mid_viewport = (area.width - step_buttons - new_tab_width).max(0.0);
                let widths = tab_widths(
                    tabs,
                    title_widths,
                    sizes,
                    options,
                    scale_factor,
                    mid_viewport,
                );
                // At the end only the part before shows; between both do.
                let last_first = last_first(&widths, end_viewport - left_peek + ROUNDING_SLACK);
                let first = scroll_to_reveal(
                    &widths,
                    mid_viewport - left_peek - right_peek + ROUNDING_SLACK,
                    first.min(last_first),
                    reveal,
                );
                let has_prev = first > 0;
                let prev_w = if has_prev { button_width } else { 0.0 };
                let before = if has_prev { left_peek } else { 0.0 };
                let no_next_viewport = (area.width - prev_w - before - new_tab_width).max(0.0);
                let tail: f32 = widths[first..].iter().sum();
                let has_next = first + 1 < tabs.len() && tail > no_next_viewport + ROUNDING_SLACK;
                let next_w = if has_next { button_width } else { 0.0 };
                let viewport = (area.width - prev_w - next_w - new_tab_width).max(0.0);
                let prev_tab = has_prev.then(|| Rect::new(bar.x, bar.y, button_width, bar.height));
                (first, last_first, viewport, widths, prev_tab, has_next)
            }
        };
        let tabs_x = bar.x
            + if prev_tab.is_some() {
                button_width
            } else {
                0.0
            };
        let before = if prev_tab.is_some() { left_peek } else { 0.0 };
        let after = if has_next { right_peek } else { 0.0 };
        let room = viewport - before - after + ROUNDING_SLACK;
        let mut slots = Vec::new();
        if before > 0.0 {
            // Its start shows, the icon and the title's first letters, which
            // say what it is; its end is mostly blank room for the close
            // button.
            let index = first - 1;
            let x = tabs_x.round();
            let shown = Rect::new(tabs_x, bar.y, before, bar.height);
            slots.push(sizes.peek(
                index,
                x,
                bar.y,
                widths[index],
                tabs[index].has_lead(),
                shown,
            ));
        }
        // Where the next tab starts, unrounded; each tab's edges are rounded
        // from where they truly fall.
        let start = tabs_x + before;
        let mut x = start;
        let mut next = first;
        for (index, &width) in widths.iter().enumerate().skip(first) {
            let shown_width = match index == first {
                // The first tab is shown even when it is too wide, cut down
                // to the strip.
                true => width.min((viewport - before).max(0.0)),
                false if x - start + width <= room => width,
                false => break,
            };
            let (left, right) = (x.round(), (x + shown_width).round());
            let has_lead = tabs[index].has_lead();
            slots.push(sizes.slot(index, left, bar.y, right - left, has_lead));
            x += shown_width;
            next = index + 1;
        }
        // What room is left before the next-tab button shows part of the tab
        // after, never less than the part saved for it.
        if after > 0.0 && next < tabs.len() {
            let left = x.round();
            let end = (tabs_x + viewport).round();
            let shown = Rect::new(left, bar.y, (end - left).max(0.0), bar.height);
            slots.push(sizes.peek(
                next,
                left,
                bar.y,
                widths[next],
                tabs[next].has_lead(),
                shown,
            ));
            x = tabs_x + viewport;
        }
        let right_x = x.min(tabs_x + viewport).round();
        let next_tab = has_next.then(|| Rect::new(right_x, bar.y, button_width, bar.height));
        let new_tab_x = right_x + if has_next { button_width } else { 0.0 };
        Self {
            bar,
            first,
            last_first,
            new_tab: (!options.no_new_tab)
                .then(|| Rect::new(new_tab_x, bar.y, button_width, bar.height)),
            next_tab,
            prev_tab,
            scale_factor,
            slots,
        }
    }

    /// What `point` hits, with tab indices looked up in `tabs`, the tabs the
    /// layout was made from.
    pub(crate) fn hit(&self, point: Point, tabs: &[Tab]) -> Option<TabStripHit> {
        if !self.bar.contains(point) {
            return None;
        }
        if self.prev_tab.is_some_and(|rect| rect.contains(point)) {
            return Some(TabStripHit::PrevTab);
        }
        if self.next_tab.is_some_and(|rect| rect.contains(point)) {
            return Some(TabStripHit::NextTab);
        }
        if self.new_tab.is_some_and(|rect| rect.contains(point)) {
            return Some(TabStripHit::NewTab);
        }
        let slot = self.slots.iter().find(|slot| slot.body.contains(point))?;
        let tab = tabs.get(slot.index)?;
        // Part of a tab is there to be gone to, not closed.
        match tab.closable && !slot.peek && slot.close.contains(point) {
            true => Some(TabStripHit::Close(tab.id)),
            false => Some(TabStripHit::Tab(tab.id)),
        }
    }

    /// The gap among the tabs shown nearest `point` while it is over the
    /// strip: the index of the tab it comes before (one past the last tab
    /// shown for the gap after it) and its x. `None` off the strip or with
    /// no tab shown.
    pub(crate) fn gap_at(&self, point: Point) -> Option<(usize, f32)> {
        if !self.bar.contains(point) {
            return None;
        }
        let last = self.slots.last()?;
        let before = self
            .slots
            .iter()
            .find(|slot| point.x < slot.body.x + slot.body.width / 2.0);
        Some(match before {
            Some(slot) => (slot.index, slot.body.x),
            None => (last.index + 1, last.body.x + last.body.width),
        })
    }
}

// ========================================================================
// TabSizes Implementations
// ========================================================================

impl TabSizes {
    /// The sizes for cells of `metrics`, making room for a close button
    /// unless `close` shows none.
    fn new(metrics: CellMetrics, close: TabClose) -> Self {
        let height = (metrics.height * STRIP_HEIGHT_RATIO).round();
        let close = match close {
            TabClose::ActiveOrHovered | TabClose::Hovered => (height * CLOSE_SHARE).round(),
            TabClose::Never => 0.0,
        };
        Self {
            close,
            cell: metrics.width,
            icon: (metrics.height * ICON_SHARE).round(),
            icon_only: false,
            height,
            padding: metrics.width.round(),
            title_top: ((height - metrics.height) / 2.0).round(),
        }
    }

    /// The width of a tab whose title is `title` pixels wide, with an icon
    /// or spinner before it when `has_lead`.
    fn tab_width(&self, title: f32, has_lead: bool) -> f32 {
        if self.icon_only {
            return self.height;
        }
        let room = title.clamp(
            MIN_TITLE_CELLS as f32 * self.cell,
            MAX_TITLE_CELLS as f32 * self.cell,
        );
        let lead = self.lead_width(has_lead);
        (2.0 * self.padding + lead + room + self.close).round()
    }

    /// The width the icon or spinner takes before the title, its gap
    /// included, when `has_lead`.
    fn lead_width(&self, has_lead: bool) -> f32 {
        match has_lead {
            true => LEAD_CELLS as f32 * self.cell,
            false => 0.0,
        }
    }

    /// The slot of the tab at `index`, `width` wide from `x`, `y`, with an
    /// icon or spinner before the title when `has_lead`.
    fn slot(&self, index: usize, x: f32, y: f32, width: f32, has_lead: bool) -> TabSlot {
        if self.icon_only {
            return self.icon_slot(index, x, y, width, has_lead);
        }
        let inset = (self.padding / 2.0).round();
        let close_x = x + width - inset - self.close;
        let close_y = y + ((self.height - self.close) / 2.0).round();
        let lead_width = self.lead_width(has_lead);
        let lead = has_lead.then(|| {
            let icon_y = y + ((self.height - self.icon) / 2.0).round();
            Rect::new(x + self.padding, icon_y, self.icon, self.icon)
        });
        let title_x = x + self.padding + lead_width;
        // The half pixel gives back what rounding the tab's width took.
        let free = width - 2.0 * self.padding - lead_width - self.close + ROUNDING_SLACK;
        TabSlot {
            body: Rect::new(x, y, width, self.height),
            close: Rect::new(close_x, close_y, self.close, self.close),
            index,
            lead,
            peek: false,
            title_room: free.max(0.0),
            title_origin: Point::new(title_x, y + self.title_top),
        }
    }

    /// The slot of the tab at `index`, a square `width` wide from `x`, `y`,
    /// its icon or spinner centered when `has_lead`, with no room for a
    /// title or a close button.
    fn icon_slot(&self, index: usize, x: f32, y: f32, width: f32, has_lead: bool) -> TabSlot {
        let icon_x = x + ((width - self.icon) / 2.0).round();
        let icon_y = y + ((self.height - self.icon) / 2.0).round();
        let middle = Point::new(x + width / 2.0, y + self.title_top);
        TabSlot {
            body: Rect::new(x, y, width, self.height),
            close: Rect::new(middle.x, middle.y, 0.0, 0.0),
            index,
            lead: has_lead.then(|| Rect::new(icon_x, icon_y, self.icon, self.icon)),
            peek: false,
            title_room: 0.0,
            title_origin: middle,
        }
    }

    /// The slot of the tab at `index`, laid out whole `width` wide from
    /// `x`, `y`, with an icon or spinner before the title when `has_lead`,
    /// of which only `shown` shows.
    fn peek(
        &self,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        has_lead: bool,
        shown: Rect,
    ) -> TabSlot {
        TabSlot {
            body: shown,
            peek: true,
            ..self.slot(index, x, y, width.round(), has_lead)
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Each tab's width across `viewport`: never less than its title needs or
/// `options.min_width`, nor more than the widest a tab is drawn.
fn tab_widths(
    tabs: &[Tab],
    title_widths: &[f32],
    sizes: TabSizes,
    options: TabOptions,
    scale_factor: f64,
    viewport: f32,
) -> Vec<f32> {
    // Exact, not floored: the edges are rounded where they are placed, so as
    // the strip widens each edge moves a pixel at a time, where floored shares
    // held still and then jumped a pixel per tab before them.
    let share = match options.width {
        TabWidth::Fill if !tabs.is_empty() => viewport / tabs.len() as f32,
        TabWidth::Fill | TabWidth::Fit | TabWidth::Icon => 0.0,
    };
    // A square stays one, whatever the least width.
    let min_width = match options.width {
        TabWidth::Icon => 0.0,
        TabWidth::Fill | TabWidth::Fit => (options.min_width * scale_factor as f32).round(),
    };
    tabs.iter()
        .zip(title_widths)
        .map(|(tab, &title)| {
            let fit = sizes.tab_width(title, tab.has_lead()).max(min_width);
            let widest = sizes.tab_width(f32::INFINITY, tab.has_lead());
            fit.max(share.min(widest))
        })
        .collect()
}

/// The smallest index from which every tab of `widths` fits in `viewport`
/// pixels; the last tab's index when even it alone does not fit.
fn last_first(widths: &[f32], viewport: f32) -> usize {
    let mut total = 0.0;
    for (index, width) in widths.iter().enumerate().rev() {
        total += width;
        if total > viewport {
            return (index + 1).min(widths.len() - 1);
        }
    }
    0
}

/// `first`, moved as little as needed for the tab at `reveal` to fit in
/// `viewport` pixels along with the tabs before it from `first` on.
fn scroll_to_reveal(widths: &[f32], viewport: f32, first: usize, reveal: Option<usize>) -> usize {
    let Some(reveal) = reveal.filter(|&reveal| reveal < widths.len()) else {
        return first;
    };
    if reveal <= first {
        return reveal;
    }
    let mut first = first;
    while first < reveal && widths[first..=reveal].iter().sum::<f32>() > viewport {
        first += 1;
    }
    first
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::TabId;
    use super::*;
    use crate::base::text;

    /// A 32 pixel strip: padding 10, close button 19 (rounded from 19.2),
    /// new-tab button 32 wide.
    const METRICS: CellMetrics = CellMetrics {
        baseline: 15.0,
        height: 20.0,
        width: 10.0,
    };

    /// Tabs titled with `cells` characters each, ids 0 up.
    fn tabs(cells: &[usize]) -> Vec<Tab> {
        cells
            .iter()
            .enumerate()
            .map(|(index, &cells)| Tab::new(TabId(index as u64), "x".repeat(cells)))
            .collect()
    }

    /// The widths of `tabs`' titles on the cell grid of `metrics`.
    fn cell_widths(tabs: &[Tab], metrics: CellMetrics) -> Vec<f32> {
        tabs.iter()
            .map(|tab| text::width(&tab.title) as f32 * metrics.width)
            .collect()
    }

    fn layout(width: f32, tabs: &[Tab], first: usize, reveal: Option<usize>) -> TabStripLayout {
        TabStripLayout::new(
            Rect::new(0.0, 0.0, width, 600.0),
            METRICS,
            1.0,
            TabOptions::default(),
            tabs,
            &cell_widths(tabs, METRICS),
            first,
            reveal,
        )
    }

    #[test]
    fn test_tab_width_follows_the_title_between_its_bounds() {
        let tabs = tabs(&[1, 10, 50]);
        let layout = layout(2000.0, &tabs, 0, None);
        let widths: Vec<f32> = layout.slots.iter().map(|slot| slot.body.width).collect();
        // Padding 2 * 10, close 19, title 6, 10, and 20 cells.
        assert_eq!(widths, [99.0, 139.0, 239.0]);
        let rooms: Vec<f32> = layout.slots.iter().map(|slot| slot.title_room).collect();
        assert_eq!(
            rooms,
            [60.5, 100.5, 200.5],
            "the room sized for, cut at 20 cells"
        );
        assert_eq!(
            layout.new_tab.expect("a new-tab button").x,
            99.0 + 139.0 + 239.0
        );
    }

    /// `tabs` laid out from 0 in a strip `width` wide, drawn as `options`
    /// says.
    fn layout_with(options: TabOptions, width: f32, tabs: &[Tab]) -> TabStripLayout {
        let area = Rect::new(0.0, 0.0, width, 600.0);
        let widths = cell_widths(tabs, METRICS);
        TabStripLayout::new(area, METRICS, 1.0, options, tabs, &widths, 0, None)
    }

    #[test]
    fn test_filling_tabs_share_the_strip_up_to_the_widest_a_tab_gets() {
        let fill = TabOptions {
            width: TabWidth::Fill,
            ..TabOptions::default()
        };
        let widths = |layout: TabStripLayout| -> Vec<f32> {
            layout.slots.iter().map(|slot| slot.body.width).collect()
        };
        // 432 less the 32 wide new-tab button leaves 400 to share.
        assert_eq!(
            widths(layout_with(fill, 432.0, &tabs(&[1, 1]))),
            [200.0, 200.0]
        );
        assert_eq!(
            widths(layout_with(fill, 432.0, &tabs(&[1]))),
            [239.0],
            "a lone tab stops at 20 cells of title"
        );
        assert_eq!(
            widths(layout_with(fill, 432.0, &tabs(&[1, 1, 1, 1, 1]))),
            [99.0, 99.0, 99.0, 71.0],
            "never narrower than its title needs; the remaining two scroll, part of the first showing"
        );
    }

    #[test]
    fn test_filling_tabs_edges_move_a_pixel_at_a_time_as_the_strip_widens() {
        let fill = TabOptions {
            width: TabWidth::Fill,
            ..TabOptions::default()
        };
        let three = tabs(&[1, 1, 1]);
        let edges = |width: f32| -> Vec<f32> {
            let layout = layout_with(fill, width, &three);
            let mut edges: Vec<f32> = layout.slots.iter().map(|slot| slot.body.x).collect();
            edges.push(layout.new_tab.expect("a new-tab button").x);
            edges
        };
        let mut before = edges(400.0);
        for width in 401..=440 {
            let after = edges(width as f32);
            for (old, new) in before.iter().zip(&after) {
                assert!((new - old).abs() <= 1.0, "{width}: {before:?} to {after:?}");
            }
            assert_eq!(
                after[3],
                width as f32 - 32.0,
                "{width}: the tabs fill the strip"
            );
            before = after;
        }
    }

    #[test]
    fn test_tabs_without_close_buttons_make_no_room_and_their_corner_hits_the_tab() {
        let never = TabOptions {
            close: TabClose::Never,
            ..TabOptions::default()
        };
        let tabs = tabs(&[1]);
        let layout = layout_with(never, 800.0, &tabs);
        let slot = &layout.slots[0];
        assert_eq!(slot.body.width, 80.0, "padding 2 * 10 and 6 cells of title");
        let corner = Point::new(slot.body.x + slot.body.width - 6.0, 16.0);
        assert_eq!(layout.hit(corner, &tabs), Some(TabStripHit::Tab(TabId(0))));
    }

    #[test]
    fn test_icon_tabs_are_squares_with_the_icon_centered_and_no_room_for_a_title() {
        let icon = TabOptions {
            min_width: 150.0,
            width: TabWidth::Icon,
            ..TabOptions::default()
        };
        let mut tabs = tabs(&[1, 30]);
        for tab in &mut tabs {
            tab.icon = Some("folder".into());
        }
        let layout = layout_with(icon, 800.0, &tabs);
        let bodies: Vec<Rect> = layout.slots.iter().map(|slot| slot.body).collect();
        assert_eq!(
            bodies,
            [
                Rect::new(0.0, 0.0, 32.0, 32.0),
                Rect::new(32.0, 0.0, 32.0, 32.0)
            ],
            "squares as tall as the strip, whatever the title or least width"
        );
        let second = &layout.slots[1];
        // The 15 pixel icon (0.75 of 20) centered in the 32 pixel square.
        assert_eq!(second.lead, Some(Rect::new(41.0, 9.0, 15.0, 15.0)));
        assert_eq!(second.title_room, 0.0);
        let corner = Point::new(62.0, 2.0);
        assert_eq!(layout.hit(corner, &tabs), Some(TabStripHit::Tab(TabId(1))));
    }

    #[test]
    fn test_a_strip_without_a_new_tab_button_gives_its_room_to_the_tabs() {
        let fill = TabOptions {
            no_new_tab: true,
            width: TabWidth::Fill,
            ..TabOptions::default()
        };
        let tabs = tabs(&[1, 1]);
        let layout = layout_with(fill, 400.0, &tabs);
        assert_eq!(layout.new_tab, None);
        let last = layout.slots.last().expect("a tab");
        assert_eq!(
            last.body.x + last.body.width,
            400.0,
            "the tabs fill the strip"
        );
        let end = Point::new(398.0, 16.0);
        assert_eq!(layout.hit(end, &tabs), Some(TabStripHit::Tab(TabId(1))));
    }

    #[test]
    fn test_the_gap_under_a_point_is_the_nearer_edge_of_the_tab_under_it() {
        let tabs = tabs(&[1, 1, 1, 1]);
        // Four tabs 99 pixels wide, scrolled to the second: the prev button
        // (32), 79 of the first, the second whole from 111, 60 of the third,
        // the next and new-tab buttons.
        let layout = layout(334.0, &tabs, 1, None);
        let gap = |x: f32| layout.gap_at(Point::new(x, 10.0));
        assert_eq!(
            gap(121.0),
            Some((1, 111.0)),
            "the left half of the whole tab"
        );
        assert_eq!(gap(180.0), Some((2, 210.0)), "its right half");
        assert_eq!(
            layout.gap_at(Point::new(10.0, 40.0)),
            None,
            "below the strip"
        );
    }

    #[test]
    fn test_a_wide_title_gets_two_cells_per_character() {
        let tabs = [Tab::new(TabId(0), "漢字テスト".to_owned())];
        let layout = layout(2000.0, &tabs, 0, None);
        assert_eq!(layout.slots[0].title_room, 100.5);
        assert_eq!(
            layout.slots[0].body.width, 139.0,
            "as wide as 10 narrow cells"
        );
    }

    #[test]
    fn test_a_fractional_cell_width_still_fits_the_whole_title() {
        let metrics = CellMetrics {
            width: 9.1,
            ..METRICS
        };
        let tabs = tabs(&[6, 7, 13]);
        let area = Rect::new(0.0, 0.0, 2000.0, 600.0);
        let widths = cell_widths(&tabs, metrics);
        let layout = TabStripLayout::new(
            area,
            metrics,
            1.0,
            TabOptions::default(),
            &tabs,
            &widths,
            0,
            None,
        );
        for (slot, width) in layout.slots.iter().zip(widths) {
            assert!(
                metrics.cols_in(slot.title_room) >= metrics.cols_in(width),
                "{slot:?}"
            );
            let title_end = slot.title_origin.x + slot.title_room;
            assert!(title_end <= slot.close.x, "{slot:?}");
        }
    }

    #[test]
    fn test_overflow_shows_part_of_the_hidden_tabs_and_pins_the_new_tab_button() {
        // Each tab is 139 wide; 79 shows of one hidden before, and what room
        // is left of one hidden after.
        let tabs = tabs(&[10; 5]);
        let shown = |layout: &TabStripLayout| -> Vec<(usize, bool, Rect)> {
            let slots = layout.slots.iter();
            slots
                .map(|slot| (slot.index, slot.peek, slot.body))
                .collect()
        };
        let part = |x: f32, width: f32| Rect::new(x, 0.0, width, 32.0);
        let start = layout(425.0, &tabs, 0, None);
        assert_eq!(start.last_first, 3);
        assert_eq!(start.prev_tab, None);
        assert_eq!(
            shown(&start),
            [
                (0, false, part(0.0, 139.0)),
                (1, false, part(139.0, 139.0)),
                (2, true, part(278.0, 83.0)),
            ]
        );
        assert_eq!(start.next_tab, Some(part(361.0, 32.0)));
        assert_eq!(start.new_tab, Some(part(393.0, 32.0)));

        let mid = layout(425.0, &tabs, 1, None);
        assert_eq!(mid.prev_tab, Some(part(0.0, 32.0)));
        assert_eq!(
            shown(&mid),
            [
                (0, true, part(32.0, 79.0)),
                (1, false, part(111.0, 139.0)),
                (2, true, part(250.0, 111.0)),
            ]
        );
        assert_eq!(mid.next_tab, Some(part(361.0, 32.0)));

        let end = layout(425.0, &tabs, 3, None);
        assert_eq!(end.next_tab, None, "no next tabs hidden at the end");
        assert_eq!(
            shown(&end),
            [
                (2, true, part(32.0, 79.0)),
                (3, false, part(111.0, 139.0)),
                (4, false, part(250.0, 139.0)),
            ]
        );
        assert_eq!(end.new_tab, Some(part(389.0, 32.0)));
    }

    #[test]
    fn test_part_of_a_tab_shows_its_start_and_is_gone_to_not_closed() {
        let tabs = tabs(&[10; 5]);
        let layout = layout(396.0, &tabs, 1, None);
        let before = &layout.slots[0];
        assert!(before.peek);
        // The whole tab starts where its part does, so its title shows.
        assert_eq!(before.title_origin.x, before.body.x + 10.0);
        let after = layout.slots.last().expect("the part after");
        let corner = Point::new(after.body.x + after.body.width - 2.0, 16.0);
        assert!(
            after.close.x > after.body.x + after.body.width,
            "its close is cut off"
        );
        assert_eq!(layout.hit(corner, &tabs), Some(TabStripHit::Tab(TabId(2))));
    }

    #[test]
    fn test_revealing_a_tab_scrolls_only_as_far_as_needed() {
        // At 565, two tabs show whole between part of a tab at each end.
        let tabs = tabs(&[10; 5]);
        let whole = |layout: &TabStripLayout| -> Vec<usize> {
            let slots = layout.slots.iter().filter(|slot| !slot.peek);
            slots.map(|slot| slot.index).collect()
        };
        let right = layout(565.0, &tabs, 0, Some(3));
        assert_eq!(
            whole(&right),
            [2, 3, 4],
            "scrolled to the end, all of it whole"
        );
        let left = layout(565.0, &tabs, 3, Some(1));
        assert_eq!(left.first, 1, "tab 1 becomes the first shown whole");
        assert_eq!(whole(&left), [1, 2]);
        let shown = layout(565.0, &tabs, 1, Some(2));
        assert_eq!(shown.first, 1, "a tab shown whole scrolls nothing");
    }

    #[test]
    fn test_a_wider_strip_scrolls_back_to_fill_itself() {
        let tabs = tabs(&[10; 5]);
        let layout = layout(1000.0, &tabs, 4, None);
        assert_eq!(layout.first, 0, "all five fit, so none is scrolled off");
        assert_eq!(layout.slots.len(), 5);
        assert_eq!(layout.prev_tab, None);
        assert_eq!(layout.next_tab, None);
    }

    #[test]
    fn test_a_strip_narrower_than_one_tab_still_shows_it_cut_down() {
        let tabs = tabs(&[10, 10]);
        let layout = layout(164.0, &tabs, 0, Some(1));
        assert_eq!(layout.slots.len(), 1);
        let slot = &layout.slots[0];
        assert_eq!(slot.index, 1);
        assert_eq!(slot.body.width, 100.0);
        assert!(slot.close.x + slot.close.width <= slot.body.x + slot.body.width);
        assert!(slot.title_origin.x + slot.title_room <= slot.close.x);
    }

    #[test]
    fn test_compact_font_keeps_peek_at_least_min_peek_width() {
        let compact = CellMetrics {
            width: 6.0,
            height: 24.0,
            ..METRICS
        };
        let tabs = tabs(&[10; 5]);
        let widths = cell_widths(&tabs, compact);
        let layout = TabStripLayout::new(
            Rect::new(0.0, 0.0, 400.0, 400.0),
            compact,
            1.0,
            TabOptions::default(),
            &tabs,
            &widths,
            1,
            None,
        );
        let before = &layout.slots[0];
        assert!(before.peek);
        assert!(before.body.width >= MIN_PEEK_WIDTH);
    }

    #[test]
    fn test_hits_tell_the_close_button_from_the_tab_and_the_new_tab_button() {
        let tabs = tabs(&[10, 10]);
        let layout = layout(800.0, &tabs, 0, None);
        let hit = |x: f32, y: f32| layout.hit(Point::new(x, y), &tabs);
        // The first tab's close button spans x 115 to 134, y 7 to 26.
        assert_eq!(hit(5.0, 16.0), Some(TabStripHit::Tab(TabId(0))));
        assert_eq!(hit(120.0, 16.0), Some(TabStripHit::Close(TabId(0))));
        assert_eq!(hit(120.0, 2.0), Some(TabStripHit::Tab(TabId(0))));
        assert_eq!(hit(140.0, 16.0), Some(TabStripHit::Tab(TabId(1))));
        assert_eq!(hit(280.0, 16.0), Some(TabStripHit::NewTab));
        assert_eq!(hit(500.0, 16.0), None, "empty strip past the button");
        assert_eq!(hit(5.0, 40.0), None, "below the strip");
    }

    #[test]
    fn test_an_icon_or_a_spinner_widens_the_tab_and_moves_its_title_right() {
        let mut tabs = tabs(&[10, 10, 10]);
        tabs[1].icon = Some("file".into());
        tabs[2].busy = true;
        let layout = layout(2000.0, &tabs, 0, None);
        let [plain, icon, busy] = [0, 1, 2].map(|index| &layout.slots[index]);
        assert_eq!(plain.lead, None);
        assert_eq!(icon.body.width, plain.body.width + 20.0, "two cells more");
        assert_eq!(
            busy.body.width, icon.body.width,
            "the spinner takes the icon's place"
        );
        let lead = icon.lead.expect("an icon square");
        // The 15 pixel icon (0.75 of 20) sits after the padding, centered.
        assert_eq!(lead, Rect::new(icon.body.x + 10.0, 9.0, 15.0, 15.0));
        assert_eq!(icon.title_origin.x, icon.body.x + 30.0);
        assert_eq!(icon.title_room, plain.title_room);
    }

    #[test]
    fn test_no_tabs_leaves_only_the_new_tab_button() {
        let layout = layout(800.0, &[], 0, Some(0));
        assert!(layout.slots.is_empty());
        assert_eq!(layout.new_tab.expect("a new-tab button").x, 0.0);
        assert_eq!(layout.last_first, 0);
    }

    #[test]
    fn test_overflow_adds_prev_and_next_buttons_that_hit_test() {
        let tabs = tabs(&[10; 5]);
        let layout = layout(396.0, &tabs, 1, None);
        let prev = layout.prev_tab.expect("prev button when first tab hidden");
        assert_eq!(prev.x, 0.0, "prev button sits at the leftmost edge");
        let next = layout.next_tab.expect("next button when overflowing");
        let center = |rect: Rect| Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        assert_eq!(layout.hit(center(prev), &tabs), Some(TabStripHit::PrevTab));
        assert_eq!(layout.hit(center(next), &tabs), Some(TabStripHit::NextTab));
        assert_eq!(
            layout.hit(center(layout.new_tab.expect("a new-tab button")), &tabs),
            Some(TabStripHit::NewTab)
        );
    }

    #[test]
    fn test_min_width_keeps_short_tabs_at_least_that_wide() {
        let options = TabOptions {
            min_width: 150.0,
            ..TabOptions::default()
        };
        let tabs = tabs(&[1, 20]);
        let layout = layout_with(options, 800.0, &tabs);
        assert_eq!(layout.slots[0].body.width, 150.0);
        assert_eq!(layout.slots[1].body.width, 239.0);
    }
}

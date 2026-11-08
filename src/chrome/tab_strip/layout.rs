//! Where the parts of a tab strip are, as plain geometry: which tabs fit,
//! each tab's body, icon, and close button, the new-tab button, and what a point
//! hits. No drawing happens here, so the layout can be tested alone and the
//! drawing and the hit testing always agree.

use super::super::super::base::geometry::{Point, Rect};
use super::{Tab, TabStripHit};
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

// ========================================================================
// Data Structures
// ========================================================================

/// Where the parts of a tab strip are, in physical pixels. Only the tabs
/// that fit are laid out: the strip scrolls by whole tabs, starting at
/// `first`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabStripLayout {
    /// The whole strip.
    pub(crate) bar: Rect,
    /// The index of the first tab shown.
    pub(crate) first: usize,
    /// The largest `first` worth scrolling to: from it on, every remaining
    /// tab fits.
    pub(crate) last_first: usize,
    /// The new-tab button, just after the last tab shown.
    pub(crate) new_tab: Rect,
    /// Physical pixels per logical pixel.
    pub(crate) scale_factor: f64,
    /// The tabs shown, left to right.
    pub(crate) slots: Vec<TabSlot>,
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
    TabSizes::new(metrics).height
}

impl TabStripLayout {
    /// The layout of `tabs` along the top of `area` in cells of `metrics`
    /// on a display scaled by `scale_factor`, their titles
    /// `title_widths` wide (one for each tab, as labels in the chrome font,
    /// in pixels), scrolled to start at `first` or as close to it as still
    /// fills the strip, and scrolled further if needed to show the tab at
    /// index `reveal`.
    pub(crate) fn new(
        area: Rect,
        metrics: CellMetrics,
        scale_factor: f64,
        tabs: &[Tab],
        title_widths: &[f32],
        first: usize,
        reveal: Option<usize>,
    ) -> Self {
        let sizes = TabSizes::new(metrics);
        let bar = Rect::new(area.x, area.y, area.width, sizes.height.min(area.height));
        let new_tab_width = sizes.height;
        let viewport = (area.width - new_tab_width).max(0.0);
        let widths: Vec<f32> = tabs
            .iter()
            .zip(title_widths)
            .map(|(tab, &title)| sizes.tab_width(title, tab.has_lead()))
            .collect();
        let last_first = last_first(&widths, viewport);
        let first = scroll_to_reveal(&widths, viewport, first.min(last_first), reveal);
        let mut slots = Vec::new();
        let mut x = bar.x;
        for (index, &width) in widths.iter().enumerate().skip(first) {
            let shown_width = match slots.is_empty() {
                // The first tab is shown even when it is too wide, cut down
                // to the strip.
                true => width.min(viewport),
                false if x + width <= bar.x + viewport => width,
                false => break,
            };
            let has_lead = tabs[index].has_lead();
            slots.push(sizes.slot(index, x, bar.y, shown_width, has_lead));
            x += shown_width;
        }
        let new_tab_x = x.min(bar.x + viewport);
        Self {
            bar,
            first,
            last_first,
            new_tab: Rect::new(new_tab_x, bar.y, new_tab_width, bar.height),
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
        if self.new_tab.contains(point) {
            return Some(TabStripHit::NewTab);
        }
        let slot = self.slots.iter().find(|slot| slot.body.contains(point))?;
        let id = tabs.get(slot.index)?.id;
        match slot.close.contains(point) {
            true => Some(TabStripHit::Close(id)),
            false => Some(TabStripHit::Tab(id)),
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
    /// The sizes for cells of `metrics`.
    fn new(metrics: CellMetrics) -> Self {
        let height = (metrics.height * STRIP_HEIGHT_RATIO).round();
        Self {
            close: (height * CLOSE_SHARE).round(),
            cell: metrics.width,
            icon: (metrics.height * ICON_SHARE).round(),
            height,
            padding: metrics.width.round(),
            title_top: ((height - metrics.height) / 2.0).round(),
        }
    }

    /// The width of a tab whose title is `title` pixels wide, with an icon
    /// or spinner before it when `has_lead`.
    fn tab_width(&self, title: f32, has_lead: bool) -> f32 {
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
            title_room: free.max(0.0),
            title_origin: Point::new(title_x, y + self.title_top),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

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
        assert_eq!(layout.new_tab.x, 99.0 + 139.0 + 239.0);
    }

    #[test]
    fn test_the_gap_under_a_point_is_the_nearer_edge_of_the_tab_under_it() {
        let tabs = tabs(&[1, 1, 1]);
        // Three tabs 99 pixels wide, room for two, scrolled to the second.
        let layout = layout(232.0, &tabs, 1, None);
        let gap = |x: f32| layout.gap_at(Point::new(x, 10.0));
        assert_eq!(
            gap(10.0),
            Some((1, 0.0)),
            "the left half of the first shown"
        );
        assert_eq!(gap(60.0), Some((2, 99.0)), "its right half");
        assert_eq!(gap(190.0), Some((3, 198.0)), "past the last tab's middle");
        assert_eq!(gap(215.0), Some((3, 198.0)), "over the new-tab button");
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
        let layout = TabStripLayout::new(area, metrics, 1.0, &tabs, &widths, 0, None);
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
    fn test_overflow_shows_only_whole_tabs_and_pins_the_new_tab_button() {
        // Each tab is 139 wide; 332 - 32 leaves 300 for tabs: two fit.
        let tabs = tabs(&[10; 5]);
        let layout = layout(332.0, &tabs, 0, None);
        assert_eq!(layout.slots.len(), 2);
        assert_eq!(layout.last_first, 3);
        assert_eq!(layout.new_tab, Rect::new(278.0, 0.0, 32.0, 32.0));
    }

    #[test]
    fn test_revealing_a_tab_scrolls_only_as_far_as_needed() {
        let tabs = tabs(&[10; 5]);
        let right = layout(332.0, &tabs, 0, Some(3));
        assert_eq!(right.first, 2, "tab 3 becomes the last shown");
        assert_eq!(right.slots.last().map(|slot| slot.index), Some(3));
        let left = layout(332.0, &tabs, 3, Some(1));
        assert_eq!(left.first, 1, "tab 1 becomes the first shown");
        let shown = layout(332.0, &tabs, 1, Some(2));
        assert_eq!(shown.first, 1, "an already shown tab scrolls nothing");
    }

    #[test]
    fn test_a_wider_strip_scrolls_back_to_fill_itself() {
        let tabs = tabs(&[10; 5]);
        let layout = layout(1000.0, &tabs, 4, None);
        assert_eq!(layout.first, 0, "all five fit, so none is scrolled off");
        assert_eq!(layout.slots.len(), 5);
    }

    #[test]
    fn test_a_strip_narrower_than_one_tab_still_shows_it_cut_down() {
        let tabs = tabs(&[10, 10]);
        let layout = layout(132.0, &tabs, 0, Some(1));
        assert_eq!(layout.slots.len(), 1);
        let slot = &layout.slots[0];
        assert_eq!(slot.index, 1);
        assert_eq!(slot.body.width, 100.0);
        assert!(slot.close.x + slot.close.width <= slot.body.x + slot.body.width);
        assert!(slot.title_origin.x + slot.title_room <= slot.close.x);
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
        assert_eq!(layout.new_tab.x, 0.0);
        assert_eq!(layout.last_first, 0);
    }
}

//! The parts a page of an app is built from, as a settings page, a list of
//! extensions, a keybinding table, or a dashboard is: a [`page`] with its
//! heading, scope tabs, actions, sidebar, content, and a footer of details
//! on what is selected, the [`sidebar`] choosing what it shows, [`section`]s grouping
//! [`item`]s in cards, and the [`badge`]s, [`keycap`]s, and [`divider`]s
//! inside them. Words are set in the proportional (UI) font, a key cap's
//! key on the cell grid; a badge and a key cap are as wide as what they
//! say. Each is made of the elements of [`misty::view`](super) and
//! becomes one with `.into()`, so a page lays itself out, scrolls, takes
//! the keyboard, and describes itself to screen readers as any tree does,
//! in a window misty runs or in an area a [`ViewHost`](super::ViewHost)
//! draws.
//!
//! A page is one of two kinds, chosen by how its content is given:
//!
//! - [`Page::scrolling`] for content built in full, which misty scrolls
//!   when it does not fit: a form, sections of items, a table of a few
//!   hundred rows. A row an app's own cursor is on stays in view with
//!   [`Element::keep_in_view`].
//! - [`Page::filled`] for content as tall as the room under the heading,
//!   holding an element that scrolls itself and builds only what shows:
//!   a [`lazy_column`](super::lazy_column), a [`list`](super::list), a
//!   [`table`](super::table). Such an element inside a scrolling page has
//!   no height to fill; [`nested_scrolls`](crate::testing::nested_scrolls)
//!   finds it in a test.
//!
//! ```
//! use misty::view::{badge, item, page, section, sidebar, text};
//! use misty::{Element, HostedView, ThemeToken};
//!
//! struct Extensions {
//!     names: Vec<String>,
//!     shown: usize,
//! }
//!
//! impl HostedView for Extensions {
//!     fn view(&self) -> Element<'_, Self> {
//!         let rows = self.names.iter().map(|name| {
//!             item(name.as_str())
//!                 .description("Runs at start")
//!                 .badge(badge("wasm", ThemeToken::Accent))
//!                 .into()
//!         });
//!         page("Extensions")
//!             .sidebar(sidebar(["Installed", "Available"], self.shown, |page: &mut Self, at| {
//!                 page.shown = at;
//!             }))
//!             .scrolling(section("Installed").items(rows))
//!             .footer(text("Asks to read ~/Notes").muted())
//!             .into()
//!     }
//! }
//! ```

use std::borrow::Cow;
use std::rc::Rc;

use super::super::base::theme::ThemeToken;
use super::super::render::layout::Align;
use super::controls::Callback;
use super::element::{Element, Text, column, row, text};
use super::props::{Corners, Extent, Press, Space};

// ========================================================================
// Constants
// ========================================================================

/// The cell columns between the cells of a grid's row.
const GRID_GAP_COLS: f32 = 2.0;

/// How wide a page's sidebar is, in cell columns.
const SIDEBAR_COLS: f32 = 22.0;

/// The rows above and below a page's heading and footer; the room at
/// their sides is the theme's ([`Spacing::page_inset`](crate::Spacing)).
const BAND_PADDING_ROWS: f32 = 0.5;

/// The rows above and below a page's content under its heading.
const CONTENT_PADDING_ROWS: f32 = 1.0;

/// The rows between a page's sections, and between a section's heading
/// and its card.
const SECTION_GAP_ROWS: f32 = 1.5;
const HEADING_GAP_ROWS: f32 = 0.5;

/// The blank space inside an item, in cell columns across and rows down.
const ITEM_PADDING: (f32, f32) = (1.5, 0.5);

/// The blank space inside a sidebar entry, in cell columns across and rows
/// down.
const ENTRY_PADDING: (f32, f32) = (1.0, 0.25);

/// The blank space at the sides of a badge and of a key cap, in logical
/// pixels.
const CHIP_PADDING_PX: f32 = 6.0;

/// How thick the shown scope tab's underline is, in logical pixels; a
/// divider is a hairline.
const UNDERLINE_PX: f32 = 2.0;

/// How wide the bar marking the shown sidebar entry is, in logical pixels.
const MARK_PX: f32 = 2.0;

/// Spacing, padding, and font scale for a [`SectionHeader`].
const SECTION_HEADER_CHEVRON_PX: f32 = 10.0;
const SECTION_HEADER_GAP_PX: f32 = 6.0;
const SECTION_HEADER_PADDING_PX: (f32, f32) = (8.0, 3.0);
const SECTION_HEADER_SCALE: f32 = 0.85;
const SECTION_HEADER_SPACE_ABOVE_PX: f32 = 8.0;

/// Disclosure chevrons for a collapsible [`SectionHeader`].
const OPEN_CHEVRON: &str = "\u{25be}";
const CLOSED_CHEVRON: &str = "\u{25b8}";

// ========================================================================
// Data Structures
// ========================================================================

/// A page before its kind is chosen: its heading, the actions and scope
/// tabs at the heading's right, and its sidebar. Made by [`page`]; given
/// its content by [`Page::scrolling`] or [`Page::filled`], which make it
/// an element.
pub struct Page<'a, A> {
    parts: PageParts<'a, A>,
}

/// A page whose content is built in full and scrolled by misty when it
/// does not fit. Made by [`Page::scrolling`].
pub struct ScrollingPage<'a, A> {
    /// What the page shows.
    content: Element<'a, A>,
    parts: PageParts<'a, A>,
    /// The id the scrolling content takes, if the app names it.
    scroll_id: Option<Cow<'a, str>>,
}

/// A page whose content is as tall as the room under its heading, never
/// scrolled itself: an element inside it scrolls and builds only what
/// shows. Made by [`Page::filled`].
pub struct FilledPage<'a, A> {
    /// What the page shows.
    content: Element<'a, A>,
    parts: PageParts<'a, A>,
}

/// What every kind of page has around its content.
struct PageParts<'a, A> {
    /// The elements at the heading's right, after the scope tabs.
    actions: Vec<Element<'a, A>>,
    /// The room above and below the content, where the app set it
    /// ([`Page::content_padding`]); else the rows the kind of page has.
    content_padding: Option<Space>,
    /// The details of what is selected, under the content, if any.
    footer: Option<Element<'a, A>>,
    /// The scope tabs at the heading's right, if any.
    scopes: Option<Element<'a, A>>,
    /// What chooses what the content shows, at the page's left, if any.
    sidebar: Option<Element<'a, A>>,
    /// The room at the sides of the heading, content, and footer, where
    /// the app set it ([`Page::side_padding`]); else the theme's page
    /// inset.
    side_padding: Option<Space>,
    /// The page's heading, if it has one: a page whose name the tab
    /// already says ([`untitled_page`]) has none.
    title: Option<Cow<'a, str>>,
}

/// A group of items under a heading, in a card. Made by [`section`].
pub struct Section<'a, A> {
    /// What the section is for, under its heading, if anything.
    description: Option<Cow<'a, str>>,
    /// The items, top to bottom.
    items: Vec<Element<'a, A>>,
    /// The heading.
    title: Cow<'a, str>,
}

/// A sidebar or panel section header: an uppercase title band with an
/// optional disclosure chevron, count pill, and trailing controls. Made by
/// [`section_header`].
pub struct SectionHeader<'a, A> {
    /// A short count or summary shown in a pill at the right.
    count: Option<Cow<'a, str>>,
    /// An optional icon or element placed before the title.
    leading: Option<Element<'a, A>>,
    /// Callback when pressed by a pointer.
    on_pointer_press: Option<Callback<A, Press>>,
    /// Callback when activated by pointer or keyboard.
    on_press: Option<Callback<A, Press>>,
    /// Whether the section is expanded, collapsed, or not collapsible.
    open: Option<bool>,
    /// Whether to draw breathing room and a top rule above the header.
    separated: bool,
    /// The section title, displayed uppercase.
    title: Cow<'a, str>,
    /// Hover text explaining the section or its action.
    tooltip: Option<Cow<'a, str>>,
    /// Controls placed at the right end of the header.
    trailing: Vec<Element<'a, A>>,
}

/// One row of a section: a label and what describes it at the left, what
/// acts on it at the right. Made by [`item`].
pub struct Item<'a, A> {
    /// The chips after the label.
    badges: Vec<Element<'a, A>>,
    /// The line under the label, if any.
    description: Option<Text<'a>>,
    /// The id it keeps its state and takes the keyboard under, if the app
    /// names it.
    id: Option<Cow<'a, str>>,
    /// What it says.
    label: Text<'a>,
    /// What pressing it runs, if anything.
    on_press: Option<Callback<A, Press>>,
    /// What a pointer press on it runs, keys aside, if anything.
    on_pointer_press: Option<Callback<A, Press>>,
    /// Whether it is the one the app's cursor is on.
    selected: bool,
    /// What its tooltip says, if it has one.
    tooltip: Option<Cow<'a, str>>,
    /// The controls at its right, in order.
    trailing: Vec<Element<'a, A>>,
}

// ========================================================================
// Page Implementations
// ========================================================================

impl<'a, A: 'static> Page<'a, A> {
    /// This page with `down` above and below its content, in place of the
    /// rows misty leaves: to fit one page to what is around it, as
    /// `Space::Cells(0.0)` puts a table's headings right under the tab that
    /// shows it.
    pub fn content_padding(mut self, down: Space) -> Self {
        self.parts.content_padding = Some(down);
        self
    }

    /// This page with `across` at each side of its heading, content, and
    /// footer, in place of the theme's page inset
    /// ([`Spacing::page_inset`](crate::Spacing)). To change every page, set
    /// the theme's spacing instead.
    pub fn side_padding(mut self, across: Space) -> Self {
        self.parts.side_padding = Some(across);
        self
    }

    /// This page with `element` at its heading's right: a button, a menu.
    pub fn action(mut self, element: impl Into<Element<'a, A>>) -> Self {
        self.parts.actions.push(element.into());
        self
    }

    /// This page with a tab for each of `names` at its heading's right,
    /// the `shown`th underlined in the accent: the scopes its values are
    /// kept in (User and Workspace) or the views of its content. Pressing
    /// one runs `on_choose` with its index.
    pub fn scopes<N: Into<Cow<'a, str>>>(
        mut self,
        names: impl IntoIterator<Item = N>,
        shown: usize,
        on_choose: impl Fn(&mut A, usize) + 'static,
    ) -> Self {
        let on_choose = Rc::new(on_choose);
        let tabs = names.into_iter().enumerate().map(|(index, name)| {
            let on_choose = Rc::clone(&on_choose);
            let label = ui(name);
            let label = match index == shown {
                true => label.color(ThemeToken::Accent),
                false => label.muted(),
            };
            let underline = match index == shown {
                true => ThemeToken::Accent,
                false => ThemeToken::Background,
            };
            column([
                label.into(),
                rule(underline, Extent::Px(UNDERLINE_PX)).width(Extent::Fraction(1.0)),
            ])
            .padding(1.0, 0.0)
            .on_press(move |state: &mut A, _| on_choose(state, index))
        });
        self.parts.scopes = Some(row(tabs).gap(Space::Cells(0.0)));
        self
    }

    /// This page with `sidebar` at its left, on the theme's surface: what
    /// chooses what the content shows, such as a [`sidebar()`].
    pub fn sidebar(mut self, sidebar: impl Into<Element<'a, A>>) -> Self {
        self.parts.sidebar = Some(sidebar.into());
        self
    }

    /// This page showing `content`, built in full, under its heading,
    /// misty scrolling it when it does not fit.
    pub fn scrolling(self, content: impl Into<Element<'a, A>>) -> ScrollingPage<'a, A> {
        ScrollingPage {
            content: content.into(),
            parts: self.parts,
            scroll_id: None,
        }
    }

    /// This page showing `content` under its heading as tall as the room
    /// there, never scrolled itself: for content holding an element that
    /// scrolls and builds only what shows, such as a
    /// [`lazy_column`](super::lazy_column).
    pub fn filled(self, content: impl Into<Element<'a, A>>) -> FilledPage<'a, A> {
        FilledPage {
            content: content.into(),
            parts: self.parts,
        }
    }
}

impl<'a, A: 'static> PageParts<'a, A> {
    /// `content` with the page's room around it: at its sides as the
    /// bands have, and above and below a row under a heading, as little as
    /// the bands have on a page without one, so what it shows starts at
    /// its top.
    fn pad_content(&self, content: Element<'a, A>) -> Element<'a, A> {
        let down = match (self.content_padding, &self.title) {
            (Some(down), _) => down,
            (None, Some(_)) => Space::Cells(CONTENT_PADDING_ROWS),
            (None, None) => Space::Cells(BAND_PADDING_ROWS),
        };
        padded(content, self.side_padding, down)
    }

    /// A band of the page, its heading or its footer, with the page's room
    /// at its sides.
    fn pad_band(&self, band: Element<'a, A>) -> Element<'a, A> {
        padded(band, self.side_padding, Space::Cells(BAND_PADDING_ROWS))
    }

    /// The page laid out around `content`: the heading over a line, then
    /// the sidebar beside the content, then the footer under a line.
    fn around(mut self, content: Element<'a, A>) -> Element<'a, A> {
        let has_heading = self.title.is_some() || self.scopes.is_some() || !self.actions.is_empty();
        let mut heading: Vec<Element<'a, A>> = self
            .title
            .as_ref()
            .map(|title| ui(title.clone()).emphasized().into())
            .into_iter()
            .collect();
        heading.push(column(Vec::new()).grow(1.0));
        heading.extend(self.scopes.take());
        heading.extend(std::mem::take(&mut self.actions));
        let heading = self.pad_band(row(heading));
        let body = match self.sidebar.take() {
            Some(sidebar) => row([
                column([sidebar])
                    .width(Extent::Cells(SIDEBAR_COLS))
                    .shrink(0.0)
                    .background(ThemeToken::Surface)
                    .corners(Corners::Square),
                vertical_divider(),
                content,
            ]),
            None => row([content]),
        }
        .gap(Space::Cells(0.0))
        .align(Align::Stretch)
        .grow(1.0);
        let mut parts = match has_heading {
            true => vec![heading, divider(), body],
            false => vec![body],
        };
        if let Some(footer) = self.footer.take() {
            parts.push(divider());
            parts.push(self.pad_band(column([footer])));
        }
        column(parts).gap(Space::Cells(0.0)).grow(1.0)
    }
}

impl<'a, A: 'static> ScrollingPage<'a, A> {
    /// This page with `details` at its foot, under a line (see
    /// [`FilledPage::footer`]).
    pub fn footer(mut self, details: impl Into<Element<'a, A>>) -> Self {
        self.parts.footer = Some(details.into());
        self
    }

    /// This page's scrolling content named `id`, so the app's own keys can
    /// scroll it ([`ViewHost::scroll`](super::ViewHost::scroll)).
    pub fn scroll_id(mut self, id: impl Into<Cow<'a, str>>) -> Self {
        self.scroll_id = Some(id.into());
        self
    }
}

impl<'a, A: 'static> FilledPage<'a, A> {
    /// This page with `details` at its foot, under a line: more about what
    /// is selected than its row says (a process's paths and command line,
    /// a setting's description). Not the keys the page takes: a page names
    /// those where the app lists keys, and the details change with the
    /// selection.
    pub fn footer(mut self, details: impl Into<Element<'a, A>>) -> Self {
        self.parts.footer = Some(details.into());
        self
    }
}

impl<'a, A: 'static> From<ScrollingPage<'a, A>> for Element<'a, A> {
    fn from(page: ScrollingPage<'a, A>) -> Self {
        let mut content = page
            .parts
            .pad_content(column([page.content]))
            .scroll()
            .grow(1.0);
        if let Some(id) = page.scroll_id {
            content = content.id(id);
        }
        page.parts.around(content)
    }
}

impl<'a, A: 'static> From<FilledPage<'a, A>> for Element<'a, A> {
    fn from(page: FilledPage<'a, A>) -> Self {
        // The content takes all the room, for what scrolls inside it to
        // fill, its scrollbar in the padding at the page's edge.
        let content = page
            .parts
            .pad_content(column([page.content.grow(1.0)]))
            .bars_in_padding()
            .grow(1.0);
        page.parts.around(content)
    }
}

// ========================================================================
// Section Implementations
// ========================================================================

impl<'a, A: 'static> Section<'a, A> {
    /// This section with `description` under its heading.
    pub fn description(self, description: impl Into<Cow<'a, str>>) -> Self {
        Self {
            description: Some(description.into()),
            ..self
        }
    }

    /// This section with `item` after its others: an [`item()`], or any
    /// element.
    pub fn item(mut self, item: impl Into<Element<'a, A>>) -> Self {
        self.items.push(item.into());
        self
    }

    /// This section with each of `items` after its others.
    pub fn items(mut self, items: impl IntoIterator<Item = Element<'a, A>>) -> Self {
        self.items.extend(items);
        self
    }
}

impl<'a, A: 'static> From<Section<'a, A>> for Element<'a, A> {
    fn from(section: Section<'a, A>) -> Self {
        let mut heading = vec![ui(section.title).emphasized().into()];
        heading.extend(
            section
                .description
                .map(|description| ui(description).muted().into()),
        );
        let count = section.items.len();
        let mut rows = Vec::with_capacity(2 * count);
        for (index, item) in section.items.into_iter().enumerate() {
            rows.push(item);
            if index + 1 < count {
                rows.push(divider());
            }
        }
        let card = column(rows)
            .gap(Space::Cells(0.0))
            .border(ThemeToken::Border)
            .corners(Corners::Box);
        column([column(heading).gap(Space::Cells(0.0)), card])
            .gap(Space::Cells(HEADING_GAP_ROWS))
            .padding(0.0, SECTION_GAP_ROWS / 2.0)
    }
}

// ========================================================================
// SectionHeader Implementations
// ========================================================================

impl<'a, A: 'static> SectionHeader<'a, A> {
    /// Show a compact count or summary pill at the right of the header.
    pub fn count(self, count: impl Into<Cow<'a, str>>) -> Self {
        Self {
            count: Some(count.into()),
            ..self
        }
    }

    /// Place an icon or element before the title.
    pub fn leading(self, leading: impl Into<Element<'a, A>>) -> Self {
        Self {
            leading: Some(leading.into()),
            ..self
        }
    }

    /// Run `apply` when a pointer presses the header, lighting it on hover.
    pub fn on_pointer_press(self, apply: impl Fn(&mut A, Press) + 'static) -> Self {
        Self {
            on_pointer_press: Some(Rc::new(apply)),
            ..self
        }
    }

    /// Run `apply` when activated by pointer or keyboard, lighting it on hover.
    pub fn on_press(self, apply: impl Fn(&mut A, Press) + 'static) -> Self {
        Self {
            on_press: Some(Rc::new(apply)),
            ..self
        }
    }

    /// Show a disclosure chevron reflecting whether the section is expanded.
    pub fn open(self, open: bool) -> Self {
        Self {
            open: Some(open),
            ..self
        }
    }

    /// Add a gap and horizontal rule above the header to separate sections.
    pub fn separated(self, separated: bool) -> Self {
        Self { separated, ..self }
    }

    /// Show `tooltip` when the pointer rests on the header.
    pub fn tooltip(self, tooltip: impl Into<Cow<'a, str>>) -> Self {
        Self {
            tooltip: Some(tooltip.into()),
            ..self
        }
    }

    /// Place `control` at the right end of the header.
    pub fn trailing(mut self, control: impl Into<Element<'a, A>>) -> Self {
        self.trailing.push(control.into());
        self
    }
}

impl<'a, A: 'static> From<SectionHeader<'a, A>> for Element<'a, A> {
    fn from(header: SectionHeader<'a, A>) -> Self {
        let mut parts: Vec<Element<'a, A>> = Vec::new();
        if let Some(open) = header.open {
            let glyph = match open {
                true => OPEN_CHEVRON,
                false => CLOSED_CHEVRON,
            };
            parts.push(
                Element::from(ui(glyph).muted().scale(SECTION_HEADER_SCALE))
                    .width(Extent::Px(SECTION_HEADER_CHEVRON_PX)),
            );
        }
        if let Some(leading) = header.leading {
            parts.push(leading);
        }
        parts.push(
            Element::from(
                ui(header.title.to_uppercase())
                    .muted()
                    .emphasized()
                    .scale(SECTION_HEADER_SCALE),
            )
            .grow(1.0),
        );
        if let Some(count) = header.count {
            parts.push(Element::from(ui(count).muted().scale(SECTION_HEADER_SCALE)));
        }
        parts.extend(header.trailing);
        let mut bar = row(parts)
            .align(Align::Center)
            .gap(Space::Px(SECTION_HEADER_GAP_PX))
            .padding_px(SECTION_HEADER_PADDING_PX.0, SECTION_HEADER_PADDING_PX.1);
        if let Some(apply) = header.on_press {
            bar = bar
                .hover_border(ThemeToken::Border)
                .on_press(move |app, press| apply(app, press));
        } else if let Some(apply) = header.on_pointer_press {
            bar = bar
                .hover_border(ThemeToken::Border)
                .on_pointer_press(move |app, press| apply(app, press));
        }
        if let Some(tooltip) = header.tooltip {
            bar = bar.tooltip(tooltip);
        }
        if header.separated {
            let gap = column(Vec::<Element<'_, A>>::new())
                .height(Extent::Px(SECTION_HEADER_SPACE_ABOVE_PX));
            let line = rule(ThemeToken::Border, Extent::Hairline);
            return column([gap, line, bar]).gap(Space::Px(0.0));
        }
        bar
    }
}

// ========================================================================
// Item Implementations
// ========================================================================

impl<'a, A: 'static> Item<'a, A> {
    /// This item with `badge` after its label, after its other badges: a
    /// [`badge()`], a [`keycap()`].
    pub fn badge(mut self, badge: impl Into<Element<'a, A>>) -> Self {
        self.badges.push(badge.into());
        self
    }

    /// This item with `description` in a muted line under its label.
    pub fn description(self, description: impl Into<Cow<'a, str>>) -> Self {
        Self {
            description: Some(ui(description).muted()),
            ..self
        }
    }

    /// This item keeping its state and taking the keyboard under `id`, as
    /// [`Element::id`] names an element: for an item among items that come
    /// and go.
    pub fn id(self, id: impl Into<Cow<'a, str>>) -> Self {
        Self {
            id: Some(id.into()),
            ..self
        }
    }

    /// This item running `apply` when pressed (a click, or Enter or Space
    /// while it has the keyboard), its background lighting up under the
    /// pointer.
    pub fn on_press(self, apply: impl Fn(&mut A, Press) + 'static) -> Self {
        Self {
            on_press: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This item running `apply` when a pointer button presses it, and only
    /// then, its background lighting up under the pointer: for a page whose
    /// keymap moves its own cursor ([`Item::selected`]), where a tab stop
    /// on every item would get in the way. [`Press::clicks`] tells a
    /// double click.
    pub fn on_pointer_press(self, apply: impl Fn(&mut A, Press) + 'static) -> Self {
        Self {
            on_pointer_press: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This item marked as the one the app's cursor is on, or not: on the
    /// hover color, lined with the accent, and kept in view by
    /// the scrolling content around it.
    pub fn selected(self, selected: bool) -> Self {
        Self { selected, ..self }
    }

    /// This item with `control` at its right, after its other controls: a
    /// checkbox, a button, a select.
    pub fn trailing(mut self, control: impl Into<Element<'a, A>>) -> Self {
        self.trailing.push(control.into());
        self
    }

    /// This item showing `details` in a tooltip once the pointer rests on
    /// it: what it means beyond its label and description.
    pub fn tooltip(self, details: impl Into<Cow<'a, str>>) -> Self {
        Self {
            tooltip: Some(details.into()),
            ..self
        }
    }
}

impl<'a, A: 'static> From<Item<'a, A>> for Element<'a, A> {
    fn from(item: Item<'a, A>) -> Self {
        let mut title = vec![item.label.into()];
        title.extend(item.badges);
        let mut left = vec![row(title)];
        left.extend(item.description.map(Element::from));
        let mut parts = vec![column(left).gap(Space::Cells(0.0)).grow(1.0)];
        parts.extend(item.trailing);
        let mut element = row(parts).padding(ITEM_PADDING.0, ITEM_PADDING.1);
        if let Some(id) = item.id {
            element = element.id(id);
        }
        if let Some(details) = item.tooltip {
            element = element.tooltip(details);
        }
        if let Some(on_press) = item.on_press {
            element = element
                .hover_background(ThemeToken::CursorLine)
                .on_press(move |state: &mut A, press| on_press(state, press));
        }
        if let Some(on_pointer_press) = item.on_pointer_press {
            element = element
                .hover_background(ThemeToken::CursorLine)
                .on_pointer_press(move |state: &mut A, press| on_pointer_press(state, press));
        }
        match item.selected {
            // A quiet fill with a thin accent line, not a menu's bright band,
            // so muted text (a description) stays readable on it.
            // Its hover is the shared light, since the cursor line it rests
            // on is what the others take under the pointer.
            true => element
                .background(ThemeToken::CursorLine)
                .hover_background(ThemeToken::Hover)
                .border(ThemeToken::Accent)
                .corners(Corners::Control)
                .keep_in_view(),
            false => element,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `element` with `across` at its sides, the theme's page inset where
/// that is `None`, and `down` above and below it.
fn padded<A>(element: Element<'_, A>, across: Option<Space>, down: Space) -> Element<'_, A> {
    let split = |space: Space| match space {
        Space::Cells(cells) => (cells, 0.0),
        Space::Px(px) => (0.0, px),
    };
    let (columns, across_px) = across.map_or((0.0, 0.0), split);
    let (rows, down_px) = split(down);
    let shown = element
        .padding(columns, rows)
        .padding_px(across_px, down_px);
    match across {
        None => shown.page_inset(),
        Some(_) => shown,
    }
}

/// A page headed `title`, with nothing in it yet: give it a sidebar, scope
/// tabs, and actions as it has them, then its content as a scrolling or a
/// filled page ([`Page::scrolling`], [`Page::filled`]), and a footer.
pub fn page<'a, A: 'static>(title: impl Into<Cow<'a, str>>) -> Page<'a, A> {
    Page {
        parts: PageParts {
            actions: Vec::new(),
            content_padding: None,
            footer: None,
            scopes: None,
            sidebar: None,
            side_padding: None,
            title: Some(title.into()),
        },
    }
}

/// A page with no heading, for one whose tab already names it, with
/// nothing in it yet: what it shows starts at its top. Built on as
/// [`page`] is; scope tabs or actions given it still go in a heading band.
pub fn untitled_page<'a, A: 'static>() -> Page<'a, A> {
    Page {
        parts: PageParts {
            actions: Vec::new(),
            content_padding: None,
            footer: None,
            scopes: None,
            sidebar: None,
            side_padding: None,
            title: None,
        },
    }
}

/// A column of `entries` for a page's sidebar, the `shown`th lit and
/// marked with the accent; pressing one runs `on_choose` with its index.
pub fn sidebar<'a, A: 'static, N: Into<Cow<'a, str>>>(
    entries: impl IntoIterator<Item = N>,
    shown: usize,
    on_choose: impl Fn(&mut A, usize) + 'static,
) -> Element<'a, A> {
    let on_choose = Rc::new(on_choose);
    let entries = entries.into_iter().enumerate().map(|(index, name)| {
        let on_choose = Rc::clone(&on_choose);
        let is_shown = index == shown;
        let mark = match is_shown {
            true => ThemeToken::Accent,
            false => ThemeToken::Surface,
        };
        let label = match is_shown {
            true => ui(name),
            false => ui(name).muted(),
        };
        let entry = row([
            rule(mark, Extent::Px(MARK_PX))
                .width(Extent::Px(MARK_PX))
                .height(Extent::Fraction(1.0)),
            label.into(),
        ])
        .align(Align::Stretch)
        .padding(ENTRY_PADDING.0, ENTRY_PADDING.1)
        .corners(Corners::Control)
        .hover_background(ThemeToken::CursorLine)
        .on_press(move |state: &mut A, _| on_choose(state, index));
        // The entry shown rests on the cursor line the others take under the
        // pointer, so its own hover is the shared light.
        match is_shown {
            true => entry
                .background(ThemeToken::CursorLine)
                .hover_background(ThemeToken::Hover),
            false => entry,
        }
    });
    column(entries).gap(Space::Cells(0.0)).padding(1.0, 0.5)
}

/// `cells` in rows of `columns` (at least one), each cell an equal share
/// of its row's width, the last row's left empty where it is short: cards
/// of a dashboard, as many across as the width the app has calls for.
pub fn grid<'a, A: 'static>(
    columns: usize,
    cells: impl IntoIterator<Item = Element<'a, A>>,
) -> Element<'a, A> {
    let columns = columns.max(1);
    let mut rows: Vec<Element<'a, A>> = Vec::new();
    let mut cells = cells.into_iter().peekable();
    while cells.peek().is_some() {
        let mut line: Vec<Element<'a, A>> = cells
            .by_ref()
            .take(columns)
            .map(|cell| column([cell]).grow(1.0))
            .collect();
        while line.len() < columns {
            line.push(column(Vec::new()).grow(1.0));
        }
        rows.push(
            row(line)
                .gap(Space::Cells(GRID_GAP_COLS))
                .align(Align::Stretch),
        );
    }
    column(rows).gap(Space::Cells(0.0))
}

/// A group of items under the heading `title`, with none yet.
pub fn section<'a, A: 'static>(title: impl Into<Cow<'a, str>>) -> Section<'a, A> {
    Section {
        description: None,
        items: Vec::new(),
        title: title.into(),
    }
}

/// A sidebar or panel section header titled `title`.
pub fn section_header<'a, A: 'static>(title: impl Into<Cow<'a, str>>) -> SectionHeader<'a, A> {
    SectionHeader {
        count: None,
        leading: None,
        on_pointer_press: None,
        on_press: None,
        open: None,
        separated: false,
        title: title.into(),
        tooltip: None,
        trailing: Vec::new(),
    }
}

/// A row of a section saying `label`, with nothing else yet.
pub fn item<'a, A: 'static>(label: impl Into<Cow<'a, str>>) -> Item<'a, A> {
    Item {
        badges: Vec::new(),
        description: None,
        id: None,
        label: ui(label),
        on_pointer_press: None,
        on_press: None,
        selected: false,
        tooltip: None,
        trailing: Vec::new(),
    }
}

/// A chip saying `label` in the theme's `tone` color, lined with it: a
/// status (enabled, refused), a kind, where a value comes from.
pub fn badge<'a, A: 'static>(label: impl Into<Cow<'a, str>>, tone: ThemeToken) -> Element<'a, A> {
    row([ui(label).color(tone).into()])
        .width(Extent::Auto)
        .border(tone)
        .corners(Corners::Control)
        .padding_px(CHIP_PADDING_PX, 0.0)
}

/// A key as a key cap, `label` on the raised color lined with the border:
/// a keybinding's chord.
pub fn keycap<'a, A: 'static>(label: impl Into<Cow<'a, str>>) -> Element<'a, A> {
    row([text(label).into()])
        .width(Extent::Auto)
        .background(ThemeToken::Raised)
        .border(ThemeToken::Border)
        .corners(Corners::Control)
        .padding_px(CHIP_PADDING_PX, 0.0)
}

/// [`keycap`] in the proportional UI font, for a page whose every word is
/// in it.
pub fn proportional_keycap<'a, A: 'static>(label: impl Into<Cow<'a, str>>) -> Element<'a, A> {
    row([ui(label).into()])
        .width(Extent::Auto)
        .background(ThemeToken::Raised)
        .border(ThemeToken::Border)
        .corners(Corners::Control)
        .padding_px(CHIP_PADDING_PX, 0.0)
}

/// A line across its column in the border color, between the parts of a
/// page or the items of a card.
pub fn divider<'a, A: 'static>() -> Element<'a, A> {
    rule(ThemeToken::Border, Extent::Hairline)
}

/// A line down its row in the border color, between a sidebar and the
/// content.
pub fn vertical_divider<'a, A: 'static>() -> Element<'a, A> {
    column(Vec::new())
        .width(Extent::Hairline)
        .shrink(0.0)
        .background(ThemeToken::Border)
        .corners(Corners::Square)
}

/// `words` as the page's own text: in the proportional (UI) font, as
/// headings, labels, and descriptions read, rather than on the cell grid
/// keys and data are set on.
fn ui<'a>(words: impl Into<Cow<'a, str>>) -> Text<'a> {
    text(words).proportional()
}

/// A bar `thickness` tall in the theme's `token` color.
fn rule<'a, A: 'static>(token: ThemeToken, thickness: Extent) -> Element<'a, A> {
    column(Vec::new())
        .height(thickness)
        .background(token)
        .corners(Corners::Square)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use accesskit::Role;

    use super::super::super::base::color::Rgb;
    use super::super::super::base::theme::{Shape, Spacing, Theme};
    use super::super::super::platform::window_id::WindowId;
    use super::super::super::testing::TreeWindow;
    use super::super::app::App;
    use super::super::controls::{button, checkbox};
    use super::*;

    /// A page of two sections, the second holding a checkbox.
    struct Demo {
        enabled: bool,
        opened: Vec<&'static str>,
        shown: usize,
        theme: Theme,
    }

    impl App for Demo {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let installed = section("Installed")
                .item(
                    item("Alpha")
                        .badge(badge("wasm", ThemeToken::Accent))
                        .on_press(|demo: &mut Self, _| demo.opened.push("Alpha")),
                )
                .item(
                    item("Beta")
                        .selected(true)
                        .on_press(|demo: &mut Self, _| demo.opened.push("Beta"))
                        .trailing(
                            checkbox("Enabled", self.enabled)
                                .on_change(|demo: &mut Self, on| demo.enabled = on),
                        ),
                );
            page("Extensions")
                .sidebar(sidebar(
                    ["Installed", "Available"],
                    self.shown,
                    |demo: &mut Self, at| {
                        demo.shown = at;
                    },
                ))
                .scopes(["User", "Workspace"], 0, |demo: &mut Self, at| {
                    demo.shown = 10 + at
                })
                .scrolling(installed)
                .footer(text("Alpha runs at start").muted())
                .into()
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    fn window() -> TreeWindow<Demo> {
        let demo = Demo {
            enabled: false,
            opened: Vec::new(),
            shown: 0,
            theme: Theme::dark(),
        };
        TreeWindow::new(demo, "page", 1000, 700)
    }

    #[test]
    fn test_a_sidebar_entry_and_a_scope_tab_pressed_run_their_callbacks() {
        let mut window = window();
        window.click(Role::Button, "Available");
        assert_eq!(window.app().shown, 1);
        window.click(Role::Button, "Workspace");
        assert_eq!(window.app().shown, 11);
    }

    #[test]
    fn test_an_item_is_pressed_by_its_text_and_its_control_keeps_its_own_click() {
        let mut window = window();
        window.click(Role::Button, "Alpha wasm");
        assert_eq!(window.app().opened, ["Alpha"]);
        window.click(Role::CheckBox, "Enabled");
        assert!(window.app().enabled);
        assert_eq!(
            window.app().opened,
            ["Alpha"],
            "the checkbox took the press"
        );
    }

    /// Three buttons in a grid two across.
    struct Grid {
        theme: Theme,
    }

    impl App for Grid {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let cells = ["A", "B", "C"].map(|name| {
                super::super::controls::button(name)
                    .on_click(|_: &mut Self| {})
                    .into()
            });
            column([grid(2, cells)])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_a_grid_fills_its_rows_in_turn_each_cell_an_equal_share() {
        let window = TreeWindow::new(
            Grid {
                theme: Theme::dark(),
            },
            "grid",
            800,
            400,
        );
        let [a, b, c] = ["A", "B", "C"].map(|name| window.center(Role::Button, name));
        assert_eq!(a.y, b.y, "A and B share the first row");
        assert!(b.x > a.x + 300.0, "B in the right half");
        assert!(c.y > a.y, "C on the next row");
        assert_eq!(c.x, a.x, "under A, the row's other half empty");
    }

    /// A page holding a list, scrolling or filled.
    struct Listing {
        filled: bool,
        rows: Vec<String>,
        theme: Theme,
    }

    impl App for Listing {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let rows = super::super::data::list(&self.rows, 0);
            let shown = page("Listing");
            match self.filled {
                true => shown.filled(rows).into(),
                false => shown.scrolling(rows).into(),
            }
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_a_list_in_a_scrolling_page_is_found_and_in_a_filled_page_is_not() {
        let listing = |filled| Listing {
            filled,
            rows: (0..50).map(|at| format!("row {at}")).collect(),
            theme: Theme::dark(),
        };
        let scrolling = TreeWindow::new(listing(false), "listing", 600, 400);
        let nested = crate::testing::nested_scrolls(scrolling.frame());
        assert_eq!(nested.len(), 1, "{nested:?}");
        let filled = TreeWindow::new(listing(true), "listing", 600, 400);
        assert_eq!(
            crate::testing::nested_scrolls(filled.frame()),
            Vec::<String>::new()
        );
    }

    /// A page holding one button, headed or not, padded as it asks.
    struct Padded {
        content_padding: Option<Space>,
        side_padding: Option<Space>,
        theme: Theme,
        titled: bool,
    }

    impl Padded {
        fn untitled() -> Self {
            Self {
                content_padding: None,
                side_padding: None,
                theme: Theme::dark(),
                titled: false,
            }
        }

        /// Where the button's face starts, at its left and its top, in a
        /// window at scale 1.
        fn button_corner(self) -> (f32, f32) {
            let window = TreeWindow::new(self, "padded", 600, 400);
            let tree = crate::testing::access_tree(window.frame(), "padded");
            let (_, node) = tree
                .nodes
                .iter()
                .find(|(_, node)| node.role() == Role::Button)
                .unwrap();
            let bounds = node.bounds().unwrap();
            (bounds.x0 as f32, bounds.y0 as f32)
        }
    }

    impl App for Padded {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let mut shown = match self.titled {
                true => page("Padded"),
                false => untitled_page(),
            };
            if let Some(down) = self.content_padding {
                shown = shown.content_padding(down);
            }
            if let Some(across) = self.side_padding {
                shown = shown.side_padding(across);
            }
            shown.filled(column([button("Go").into()])).into()
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_an_untitled_page_draws_no_heading_and_starts_higher() {
        let titled = TreeWindow::new(
            Padded {
                titled: true,
                ..Padded::untitled()
            },
            "padded",
            600,
            400,
        );
        assert!(titled.drawn_text().iter().any(|line| line == "Padded"));
        let untitled = TreeWindow::new(Padded::untitled(), "padded", 600, 400);
        assert!(!untitled.drawn_text().iter().any(|line| line == "Padded"));
        let (_, top) = Padded::untitled().button_corner();
        let (_, titled_top) = Padded {
            titled: true,
            ..Padded::untitled()
        }
        .button_corner();
        assert!(top < titled_top, "{top} against {titled_top}");
    }

    #[test]
    fn test_the_themes_page_inset_sets_where_a_page_starts() {
        let inset = |page_inset| {
            Padded {
                theme: Theme {
                    spacing: Spacing { page_inset },
                    ..Theme::dark()
                },
                ..Padded::untitled()
            }
            .button_corner()
            .0
        };
        assert_eq!(inset(30) - inset(6), 24.0);
    }

    #[test]
    fn test_a_pages_own_padding_overrides_the_theme() {
        let (left, top) = Padded {
            content_padding: Some(Space::Cells(0.0)),
            side_padding: Some(Space::Px(0.0)),
            ..Padded::untitled()
        }
        .button_corner();
        assert_eq!((left, top), (0.0, 0.0));
    }

    /// Two buttons stacked, the first's row taking the theme's row
    /// padding where `padded`.
    struct Rowed {
        padded: bool,
        theme: Theme,
    }

    impl App for Rowed {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let first = row([button("First").into()]);
            let first = match self.padded {
                true => first.row_padding(),
                false => first,
            };
            column([first, button("Second").into()])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_row_padding_takes_the_shapes_room_above_and_below() {
        let second = |padded, shape| {
            let theme = Theme {
                shape,
                ..Theme::dark()
            };
            let window = TreeWindow::new(Rowed { padded, theme }, "rowed", 600, 400);
            window.center(Role::Button, "Second").y
        };
        let room = 2.0 * f32::from(Shape::MODERN.row_padding);
        assert_eq!(
            second(true, Shape::MODERN) - second(false, Shape::MODERN),
            room
        );
        assert_eq!(second(true, Shape::GRID), second(false, Shape::GRID));
    }

    /// A divider off the pixel grid, as layout in cells and a scaled
    /// display leave one.
    struct Ruled {
        theme: Theme,
    }

    impl App for Ruled {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let above = column(Vec::new()).height(Extent::Px(10.6));
            column([above, divider()]).gap(Space::Cells(0.0))
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_a_divider_is_one_whole_pixel_wherever_it_falls() {
        let theme = Theme::dark();
        let (border, background) = (theme.border, theme.background);
        let mut window = TreeWindow::new(Ruled { theme }, "ruled", 200, 40);
        // At 112 dpi, a scale a logical pixel spreads over two rows at.
        let Some(shot) = window.render_at(1.17) else {
            return;
        };
        let rows: Vec<Rgb> = (8..14).map(|y| shot.pixel(100.0, y as f32 + 0.5)).collect();
        let lines = rows.iter().filter(|color| **color == border).count();
        assert_eq!(lines, 1, "one row in the border color: {rows:?}");
        assert!(
            rows.iter()
                .all(|color| *color == border || *color == background),
            "no row blended: {rows:?}"
        );
    }

    #[test]
    fn test_the_page_draws_its_heading_sections_and_footer() {
        let window = window();
        let drawn = window.drawn_text().join("\n");
        for words in [
            "Extensions",
            "Installed",
            "Alpha",
            "wasm",
            "Beta",
            "Alpha runs at start",
        ] {
            assert!(drawn.contains(words), "{words:?} in {drawn:?}");
        }
    }

    struct HeaderDemo {
        added: usize,
        open: bool,
        theme: Theme,
    }

    impl App for HeaderDemo {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let header = section_header("Staged")
                .open(self.open)
                .count("3")
                .trailing(
                    super::super::controls::button("Add").on_click(|app: &mut Self| {
                        app.added += 1;
                    }),
                )
                .on_press(|app: &mut Self, _| app.open = !app.open);
            column([header.into()])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_section_header_draws_uppercase_title_count_and_toggles_independently_of_trailing() {
        let mut window = TreeWindow::new(
            HeaderDemo {
                added: 0,
                open: true,
                theme: Theme::dark(),
            },
            "header",
            400,
            200,
        );
        let drawn = window.drawn_text().join(" ");
        assert!(drawn.contains("STAGED"), "uppercase title in {drawn:?}");
        assert!(drawn.contains("3"), "count pill in {drawn:?}");
        assert!(drawn.contains(OPEN_CHEVRON), "open chevron in {drawn:?}");

        window.click(Role::Button, "Add");
        assert_eq!(window.app().added, 1);
        assert!(
            window.app().open,
            "trailing button click must not toggle header"
        );

        window.click(Role::Button, &format!("{OPEN_CHEVRON} STAGED 3"));
        assert!(!window.app().open, "clicking header toggles open state");
        let drawn = window.drawn_text().join(" ");
        assert!(
            drawn.contains(CLOSED_CHEVRON),
            "closed chevron after toggle in {drawn:?}"
        );
    }
}

//! Where the parts of a title bar are, as plain geometry: the window
//! buttons, the menu button, the app's buttons, the menubar's titles, the
//! room for the title or the tabs, and what a point hits. No drawing happens here, so the
//! layout can be tested alone and the drawing and the hit testing always
//! agree.

use super::super::super::base::geometry::{Point, Rect};
use super::super::super::platform::resize::ResizeEdge;
use super::super::super::render::font::CellMetrics;
use super::{
    APP_BUTTON_PADDING_CELLS, MENU_TITLE_PADDING_CELLS, TitleBarHit, WindowButton, WindowButtons,
};

// ========================================================================
// Constants
// ========================================================================

/// The bar's height as a multiple of the cell height.
const BAR_HEIGHT_RATIO: f32 = 1.6;

/// A button's width as a multiple of the bar's height.
const BUTTON_WIDTH_RATIO: f32 = 1.5;

// ========================================================================
// Data Structures
// ========================================================================

/// Where the parts of a title bar are in a window, in physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TitleBarLayout {
    /// The app's buttons, left to right, at the end of the room beside
    /// the menu button or the window buttons; those that did not fit are
    /// left out from the last.
    pub(crate) app_buttons: Vec<Rect>,
    /// The whole bar along the window's top.
    pub(crate) bar: Rect,
    /// Where the window buttons go.
    pub(crate) buttons: WindowButtons,
    /// The close button, outermost, or `None` without window buttons.
    pub(crate) close: Option<Rect>,
    /// The maximize or restore button, or `None` without window buttons:
    /// beside close at the right edge, innermost at the left.
    pub(crate) maximize: Option<Rect>,
    /// The menu button at the left, or `None` when the bar has none.
    pub(crate) menu: Option<Rect>,
    /// The menubar's titles, left to right, after the menu button.
    pub(crate) menu_titles: Vec<Rect>,
    /// The minimize button, or `None` without window buttons: innermost
    /// at the right edge, beside close at the left.
    pub(crate) minimize: Option<Rect>,
    /// The room between the menus and the window buttons, where the title
    /// or the tabs go.
    pub(crate) middle: Rect,
    /// Physical pixels per logical pixel.
    pub(crate) scale_factor: f64,
    /// The whole window.
    pub(crate) window: Rect,
}

// ========================================================================
// Functions
// ========================================================================

/// How tall a title bar is for cells of `metrics`.
pub(crate) fn bar_height(metrics: CellMetrics) -> f32 {
    (metrics.height * BAR_HEIGHT_RATIO).round()
}

// ========================================================================
// TitleBarLayout Implementations
// ========================================================================

impl TitleBarLayout {
    /// The layout in `window`, sized for cells of `metrics` on a display
    /// scaled by `scale_factor`, with a menu button when `has_menu_button`,
    /// a menubar of titles `menu_widths` wide (as labels in the chrome
    /// font, in pixels, before their padding), and the app's buttons, each
    /// as wide as its content in `app_widths` (0 for an icon alone, which
    /// takes a window button's width). The app's buttons sit at the end of
    /// the room, beside the menu button when the window buttons are at the
    /// left and beside the window buttons otherwise. Titles and app buttons
    /// that would reach what sits beside them are left out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        window: Rect,
        metrics: CellMetrics,
        scale_factor: f64,
        has_menu_button: bool,
        menu_widths: &[f32],
        app_widths: &[f32],
        buttons: WindowButtons,
    ) -> Self {
        let height = bar_height(metrics);
        let button_width = (height * BUTTON_WIDTH_RATIO).round();
        let right = window.x + window.width;
        // The `index`th button from the edge the buttons sit at, close
        // first.
        let button = |index: f32| match buttons {
            WindowButtons::Left => Some(Rect::new(
                window.x + button_width * index,
                window.y,
                button_width,
                height,
            )),
            WindowButtons::None => None,
            WindowButtons::Right => Some(Rect::new(
                right - button_width * (index + 1.0),
                window.y,
                button_width,
                height,
            )),
        };
        // Close is outermost at either edge; the left edge then reads
        // close, minimize, maximize, the right minimize, maximize, close.
        let (close, maximize, minimize) = match buttons {
            WindowButtons::Left => (button(0.0), button(2.0), button(1.0)),
            WindowButtons::None | WindowButtons::Right => (button(0.0), button(1.0), button(2.0)),
        };
        let menu = has_menu_button.then(|| match buttons {
            WindowButtons::Left => Rect::new(right - button_width, window.y, button_width, height),
            WindowButtons::None | WindowButtons::Right => {
                Rect::new(window.x, window.y, button_width, height)
            }
        });
        // The room between what sits at the left edge and what sits at the
        // right one, where the menubar's titles go first.
        let (mut x, room_end) = match buttons {
            WindowButtons::Left => (
                maximize.map_or(window.x, |rect| rect.x + rect.width),
                menu.map_or(right, |menu| menu.x),
            ),
            WindowButtons::None | WindowButtons::Right => (
                menu.map_or(window.x, |menu| menu.x + menu.width),
                minimize.map_or(right, |rect| rect.x),
            ),
        };
        let app_padding = 2.0 * APP_BUTTON_PADDING_CELLS as f32 * metrics.width;
        let app_width = |content: f32| match content > 0.0 {
            true => (content + app_padding).round().max(button_width),
            false => button_width,
        };
        // As many of the app's buttons as fit, from the first, packed
        // against the end of the room.
        let mut fitted = 0;
        let mut total = 0.0;
        for content in app_widths {
            let width = app_width(*content);
            if room_end - total - width < x {
                break;
            }
            total += width;
            fitted += 1;
        }
        let end = room_end - total;
        let mut app_x = end;
        let app_buttons = app_widths[..fitted]
            .iter()
            .map(|content| {
                let width = app_width(*content);
                let rect = Rect::new(app_x, window.y, width, height);
                app_x += width;
                rect
            })
            .collect();
        let mut menu_titles = Vec::new();
        let padding = 2.0 * MENU_TITLE_PADDING_CELLS as f32 * metrics.width;
        for title in menu_widths {
            let width = (title + padding).round();
            if x + width > end {
                break;
            }
            menu_titles.push(Rect::new(x, window.y, width, height));
            x += width;
        }
        Self {
            app_buttons,
            bar: Rect::new(window.x, window.y, window.width, height),
            buttons,
            close,
            maximize,
            menu,
            menu_titles,
            middle: Rect::new(x, window.y, (end - x).max(0.0), height),
            minimize,
            scale_factor,
            window,
        }
    }

    /// Where `button` is, or `None` without window buttons.
    pub(crate) fn button(&self, button: WindowButton) -> Option<Rect> {
        match button {
            WindowButton::Close => self.close,
            WindowButton::Maximize => self.maximize,
            WindowButton::Minimize => self.minimize,
        }
    }

    /// What `point` hits, on a window that is `maximized` or not. Without
    /// window buttons the system frames the window: the bar has no resize
    /// border and its empty room does not move the window.
    pub(crate) fn hit(&self, point: Point, maximized: bool) -> Option<TitleBarHit> {
        let framed = self.buttons == WindowButtons::None;
        if !maximized
            && !framed
            && let Some(edge) = ResizeEdge::at(point, self.window, self.scale_factor)
        {
            return Some(TitleBarHit::Resize(edge));
        }
        let buttons = [
            WindowButton::Close,
            WindowButton::Maximize,
            WindowButton::Minimize,
        ];
        if let Some(button) = buttons.into_iter().find(|button| {
            self.button(*button)
                .is_some_and(|rect| rect.contains(point))
        }) {
            return Some(TitleBarHit::Button(button));
        }
        if self.menu.is_some_and(|menu| menu.contains(point)) {
            return Some(TitleBarHit::MenuButton);
        }
        if let Some(index) = self
            .app_buttons
            .iter()
            .position(|rect| rect.contains(point))
        {
            return Some(TitleBarHit::AppButton(index));
        }
        if let Some(index) = self
            .menu_titles
            .iter()
            .position(|rect| rect.contains(point))
        {
            return Some(TitleBarHit::MenuTitle(index));
        }
        (!framed && self.bar.contains(point)).then_some(TitleBarHit::Drag)
    }
}

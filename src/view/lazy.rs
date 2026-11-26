//! A [`lazy_column`]: rows of any elements, as many as an app has, of
//! which misty builds and lays out only those that show. The app gives the
//! rows through a [`LazySource`], which says how many there are and builds
//! row `n` when asked, and says which row its cursor is on; misty measures
//! the rows it builds, scrolls (by the wheel, the scrollbar, and to the
//! cursor when the cursor moves), and asks for the rows that come into
//! view. A list of thousands of rows costs no more to draw than one that
//! fits.
//!
//! It takes the height its column leaves, as a [`list`](super::list)
//! does, so it goes in a [`Page::filled`](super::Page::filled) page, or a
//! column of its own, never inside a scrolling container, which leaves no
//! height to fill.
//!
//! ```
//! use misty::view::{lazy_column, page, text, LazySource};
//! use misty::{Element, HostedView};
//!
//! struct Names {
//!     names: Vec<String>,
//!     cursor: usize,
//! }
//!
//! impl LazySource<Names> for Names {
//!     fn count(&self) -> usize {
//!         self.names.len()
//!     }
//!
//!     fn row(&self, index: usize) -> Element<'_, Names> {
//!         text(self.names[index].as_str()).into()
//!     }
//! }
//!
//! impl HostedView for Names {
//!     fn view(&self) -> Element<'_, Self> {
//!         page("Names").filled(lazy_column(self, self.cursor)).into()
//!     }
//! }
//! ```

use std::ops::Range;
use std::rc::Rc;

use super::controls::Callback;
use super::element::{Element, Kind, Lazy};

// ========================================================================
// Data Structures
// ========================================================================

/// Rows of elements built as they come into view. Made by
/// [`lazy_column`].
pub struct LazyColumn<'a, A> {
    cursor: usize,
    on_shown: Option<Callback<A, Range<usize>>>,
    rows: &'a dyn LazySource<A>,
}

// ========================================================================
// Traits
// ========================================================================

/// The rows of a [`lazy_column`]: how many there are, and row `index` as an
/// element, built only while it shows. Give each row the same height for
/// the scrollbar to be exact; misty measures them as they show.
pub trait LazySource<A> {
    /// How many rows there are.
    fn count(&self) -> usize;

    /// Row `index`, one of those that show.
    fn row(&self, index: usize) -> Element<'_, A>;
}

// ========================================================================
// LazyColumn Implementations
// ========================================================================

impl<A> LazyColumn<'_, A> {
    /// What runs on the app's state when other rows show than last
    /// reported (and once when it is first drawn): the rows on screen, a
    /// row partly in sight among them, for the app's own keys that go by
    /// what shows (`H`, `M`, `L`, a page). Reported once the frame that
    /// showed them is drawn, as
    /// [`Element::on_size_change`](crate::Element::on_size_change) is.
    pub fn on_shown(mut self, apply: impl Fn(&mut A, Range<usize>) + 'static) -> Self {
        self.on_shown = Some(Rc::new(apply));
        self
    }
}

impl<'a, A> From<LazyColumn<'a, A>> for Element<'a, A> {
    fn from(column: LazyColumn<'a, A>) -> Self {
        let mut element = Element::new(Kind::Lazy(Lazy {
            cursor: column.cursor,
            rows: column.rows,
        }));
        if let Some(apply) = column.on_shown {
            element.extras().on_rows_shown = Some(apply);
        }
        element
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The rows `rows` gives, each built as it comes into view, the `cursor`th
/// kept in view each time it moves: a long list made of any elements (a
/// table's cells, buttons, icons), of which only those that show cost
/// anything.
pub fn lazy_column<'a, A, S: LazySource<A>>(rows: &'a S, cursor: usize) -> LazyColumn<'a, A> {
    LazyColumn {
        cursor,
        on_shown: None,
        rows,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::super::super::base::geometry::Point;
    use super::super::super::base::theme::Theme;
    use super::super::super::input::event::{Input, WheelDelta, WheelInput};
    use super::super::super::platform::window_id::WindowId;
    use super::super::super::testing::TreeWindow;
    use super::super::app::App;
    use super::super::element::text;
    use super::super::page::page;
    use super::*;

    /// Ten thousand rows in a filled page, or a scrolling one, counting
    /// the rows it was asked to build.
    struct Many {
        built: Cell<usize>,
        cursor: usize,
        filled: bool,
        shown: Range<usize>,
        theme: Theme,
    }

    impl LazySource<Many> for Many {
        fn count(&self) -> usize {
            10_000
        }

        fn row(&self, index: usize) -> Element<'_, Many> {
            self.built.set(self.built.get() + 1);
            text(format!("row {index:05}")).into()
        }
    }

    impl App for Many {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            self.built.set(0);
            let rows = lazy_column(self, self.cursor).on_shown(|many: &mut Self, shown| {
                many.shown = shown;
            });
            match self.filled {
                true => page("Many").filled(rows).into(),
                false => page("Many").scrolling(rows).into(),
            }
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    fn window(filled: bool) -> TreeWindow<Many> {
        let many = Many {
            built: Cell::new(0),
            cursor: 0,
            filled,
            shown: 0..0,
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(many, "many", 600, 400);
        window.window().set_reduced_motion(true);
        settle(&mut window);
        window
    }

    /// Draw until the lazy column built the rows that show.
    fn settle(window: &mut TreeWindow<Many>) {
        for _ in 0..4 {
            window.about_to_wait();
        }
    }

    fn shown(window: &TreeWindow<Many>) -> Vec<String> {
        window
            .drawn_text()
            .into_iter()
            .filter(|text| text.starts_with("row "))
            .collect()
    }

    #[test]
    fn test_only_the_rows_around_what_shows_are_built_and_they_fill_the_page() {
        let window = window(true);
        let rows = shown(&window);
        assert_eq!(rows.first().map(String::as_str), Some("row 00000"));
        assert!(rows.len() >= 10, "the page's height filled: {rows:?}");
        let built = window.app().built.get();
        assert!(built < 100, "of ten thousand, {built} built");
    }

    #[test]
    fn test_the_cursor_moved_to_the_last_row_brings_it_into_view() {
        let mut window = window(true);
        window.update(|many| many.cursor = 9_999);
        settle(&mut window);
        let rows = shown(&window);
        assert!(rows.contains(&"row 09999".to_string()), "{rows:?}");
        assert!(!rows.contains(&"row 00000".to_string()));
        assert!(window.app().built.get() < 100);
    }

    #[test]
    fn test_the_rows_on_screen_are_reported_as_they_change() {
        let mut window = window(true);
        let shown = window.app().shown.clone();
        assert_eq!(shown.start, 0);
        assert_eq!(
            shown.len(),
            self::shown(&window).len(),
            "every row drawn, {shown:?}"
        );
        window.update(|many| many.cursor = 9_999);
        settle(&mut window);
        let shown = window.app().shown.clone();
        assert_eq!(shown.end, 10_000, "{shown:?}");
    }

    #[test]
    fn test_the_wheel_scrolls_the_rows() {
        let mut window = window(true);
        window.send(&Input::Wheel(WheelInput {
            delta: WheelDelta::Lines(Point::new(0.0, -20.0)),
            mods: Default::default(),
            position: Point::new(300.0, 200.0),
        }));
        settle(&mut window);
        let rows = shown(&window);
        assert!(!rows.contains(&"row 00000".to_string()), "{rows:?}");
    }

    /// A few rows that each take a press.
    struct Pressed {
        theme: Theme,
    }

    impl LazySource<Pressed> for Pressed {
        fn count(&self) -> usize {
            3
        }

        fn row(&self, index: usize) -> Element<'_, Pressed> {
            let row = Element::from(text(format!("row {index}")))
                .on_pointer_press(|_: &mut Pressed, _| {})
                .hover_border(super::super::super::base::theme::ThemeToken::Border);
            match index == 0 {
                true => row.background(super::super::super::base::theme::ThemeToken::SelectedRow),
                false => row,
            }
        }
    }

    impl App for Pressed {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            page("Pressed").filled(lazy_column(self, 0)).into()
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_every_row_lights_under_the_pointer() {
        let pressed = Pressed {
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(pressed, "pressed", 600, 400);
        window.about_to_wait();
        assert_eq!(window.unlit_controls(), Vec::<String>::new());
    }

    /// A lazy column beside a line of text in a row.
    struct Beside {
        theme: Theme,
    }

    impl App for Beside {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            super::super::element::row([text("beside").into(), lazy_column(self, 0).into()])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    impl LazySource<Beside> for Beside {
        fn count(&self) -> usize {
            100
        }

        fn row(&self, index: usize) -> Element<'_, Beside> {
            text(format!("row {index:05}")).into()
        }
    }

    #[test]
    fn test_a_lazy_column_in_a_row_takes_the_rows_whole_height() {
        let beside = Beside {
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(beside, "beside", 600, 400);
        for _ in 0..4 {
            window.about_to_wait();
        }
        let rows = window
            .drawn_text()
            .into_iter()
            .filter(|text| text.starts_with("row "))
            .count();
        assert!(rows >= 10, "the row's height filled, {rows} rows shown");
    }

    #[test]
    fn test_a_lazy_column_in_a_scrolling_page_is_found() {
        let window = window(false);
        let nested = crate::testing::nested_scrolls(window.frame());
        assert_eq!(nested.len(), 1, "{nested:?}");
    }
}

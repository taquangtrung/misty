//! A reading workbench written as one tree app, showing what a large
//! desktop app needs beyond misty's controls: two shelves of books whose
//! rows are styled cards (theme surfaces, borders, a hover fill), pressed
//! to select and dragged from one shelf to the other (or moved with `m`
//! once selected, the keyboard way); files dragged in from another app
//! land on the shelf under the pointer. A filter field narrows the shelves
//! once typing has settled (a debounce timer in `App::about_to_wait`). The
//! selected book's notes show in a `CodeView`, a hand-driven widget
//! embedded with `custom`; a `canvas` beside them takes strokes drawn with
//! the pointer, and `c` clears it while it has the keyboard. "Details"
//! opens a second window, described by the same `view`, which asks before
//! it closes while it holds unsaved marks. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example workbench
//! ```

use std::path::Path;
use std::time::{Duration, Instant};

use misty::layout::Align;
use misty::view::{
    Corners, Extent, PointerDrag, Space, button, canvas, column, custom, row, text, text_field,
};
use misty::{App, AppCx, Element, Frame, Key, KeyCode, Mods, Point, Rect, Theme, ThemeToken, WindowId, WindowOptions};
use misty::widgets::{CodeView, CodeViewResponse, FileDropped};

// ========================================================================
// Constants
// ========================================================================

/// The main window's title.
const TITLE: &str = "misty workbench";

/// The details window's title.
const DETAILS_TITLE: &str = "Details";

/// How long typing in the filter must settle before the shelves follow.
const SETTLE: Duration = Duration::from_millis(300);

/// How tall a book's card is, in cell rows.
const CARD_ROWS: f32 = 2.0;

/// The space between the cards of a shelf, in logical pixels.
const CARD_GAP: f32 = 4.0;

/// How wide each shelf is, in cell columns.
const SHELF_COLUMNS: f32 = 28.0;

/// How tall the sketch canvas is, in logical pixels.
const SKETCH_HEIGHT: f32 = 160.0;

/// How large each dot of a stroke is, in physical pixels.
const DOT: f32 = 4.0;

/// The books on the shelf at start.
const BOOKS: [&str; 4] = [
    "A Pattern Language",
    "The Art of Unix Programming",
    "Structure and Interpretation",
    "Thinking, Fast and Slow",
];

// ========================================================================
// Data Structures
// ========================================================================

/// Which of the two shelves a book is on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shelf {
    /// The books to read some day.
    Library,
    /// The books being read now.
    Reading,
}

/// A book dragged from one shelf, on its way to the other.
#[derive(Clone, Debug, PartialEq)]
struct Moving {
    /// The shelf it is dragged from.
    from: Shelf,
    /// Its title.
    title: String,
}

/// The demo.
struct Workbench {
    /// The second window, while it is open.
    details: Option<WindowId>,
    /// Whether a callback asked for the details window.
    details_wanted: bool,
    /// The filter the shelves follow, once typing settled.
    filter: String,
    /// When the filter typed is applied, while typing settles.
    filter_due: Option<Instant>,
    /// The library's books.
    library: Vec<String>,
    /// The offset in the notes the code view's cursor is at.
    notes_cursor: usize,
    /// What the filter field says.
    query: String,
    /// The books being read.
    reading: Vec<String>,
    /// The book selected, if any.
    selected: Option<String>,
    /// The strokes drawn on the canvas, each a run of points from its top
    /// left.
    strokes: Vec<Vec<Point>>,
    /// The colors the windows are drawn in.
    theme: Theme,
}

// ========================================================================
// Shelf Implementations
// ========================================================================

impl Shelf {
    /// What the shelf is called.
    fn title(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Reading => "Reading",
        }
    }

    /// The other shelf.
    fn other(self) -> Self {
        match self {
            Self::Library => Self::Reading,
            Self::Reading => Self::Library,
        }
    }
}

// ========================================================================
// Workbench Implementations
// ========================================================================

impl Workbench {
    /// The demo at start: every book in the library.
    fn new() -> Self {
        Self {
            details: None,
            details_wanted: false,
            filter: String::new(),
            filter_due: None,
            library: BOOKS.iter().map(|book| (*book).to_owned()).collect(),
            notes_cursor: 0,
            query: String::new(),
            reading: Vec::new(),
            selected: None,
            strokes: Vec::new(),
            theme: Theme::default(),
        }
    }

    /// The books on `shelf`.
    fn books(&self, shelf: Shelf) -> &[String] {
        match shelf {
            Shelf::Library => &self.library,
            Shelf::Reading => &self.reading,
        }
    }

    /// The books on `shelf`, to change.
    fn books_mut(&mut self, shelf: Shelf) -> &mut Vec<String> {
        match shelf {
            Shelf::Library => &mut self.library,
            Shelf::Reading => &mut self.reading,
        }
    }

    /// Moves `title` from `from` to the other shelf.
    fn move_book(&mut self, title: &str, from: Shelf) {
        let books = self.books_mut(from);
        let Some(at) = books.iter().position(|book| book == title) else {
            return;
        };
        let book = books.remove(at);
        self.books_mut(from.other()).push(book);
    }

    /// The shelf the selected book is on.
    fn selected_shelf(&self) -> Option<Shelf> {
        let selected = self.selected.as_ref()?;
        [Shelf::Library, Shelf::Reading]
            .into_iter()
            .find(|shelf| self.books(*shelf).contains(selected))
    }

    /// The notes on the selected book.
    fn notes(&self) -> String {
        match &self.selected {
            Some(title) => format!("# {title}\n\nchapter 1: read\nchapter 2: skimmed\n"),
            None => "Select a book to see its notes.\n".to_owned(),
        }
    }

    /// Follows a pointer drag on the canvas: a stroke starts at the press
    /// and grows with each move.
    fn sketch(&mut self, drag: PointerDrag) {
        match self.strokes.last_mut() {
            Some(stroke) if stroke.first() == Some(&drag.start) => stroke.push(drag.point),
            _ => self.strokes.push(vec![drag.start, drag.point]),
        }
    }

    /// A book's card: pressed to select it, dragged to the other shelf.
    fn card<'a>(&self, title: &'a str, shelf: Shelf) -> Element<'a, Self> {
        let selected = self.selected.as_deref() == Some(title);
        let fill = match selected {
            true => ThemeToken::SelectedRow,
            false => ThemeToken::Raised,
        };
        let owned = title.to_owned();
        Element::from(text(title))
            .id(title)
            .height(Extent::Cells(CARD_ROWS))
            .padding_px(8.0, 0.0)
            .background(fill)
            .hover_background(ThemeToken::Hover)
            .border(ThemeToken::Border)
            .corners(Corners::Control)
            .access_name(title)
            .on_press(move |demo: &mut Self, _| demo.selected = Some(owned.clone()))
            .drag_source(
                Moving {
                    from: shelf,
                    title: title.to_owned(),
                },
                title,
            )
    }

    /// A shelf of the books matching the filter, taking the books dragged
    /// from the other shelf and files dropped from another app.
    fn shelf(&self, shelf: Shelf) -> Element<'_, Self> {
        let filter = self.filter.to_lowercase();
        let cards = self
            .books(shelf)
            .iter()
            .filter(|book| book.to_lowercase().contains(&filter))
            .map(|book| self.card(book, shelf));
        let heading = Element::from(text(shelf.title()).muted());
        column(std::iter::once(heading).chain(cards))
            .id(shelf.title())
            .width(Extent::Cells(SHELF_COLUMNS))
            .gap(Space::Px(CARD_GAP))
            .padding_px(8.0, 8.0)
            .background(ThemeToken::Surface)
            .border(ThemeToken::Border)
            .drop_target(
                move |moving: &Moving| moving.from != shelf,
                |demo: &mut Self, moving: Moving, _| demo.move_book(&moving.title, moving.from),
            )
            .on_file_drop(move |demo: &mut Self, file: FileDropped| {
                let name = Path::new(&file.path)
                    .file_stem()
                    .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
                demo.books_mut(shelf).push(name);
            })
    }

    /// The main window: the filter, the shelves, the notes, and the sketch.
    fn main_view(&self) -> Element<'_, Self> {
        let notes = self.notes();
        let filter = text_field(&self.query)
            .placeholder("Filter books")
            .on_change(|demo: &mut Self, query| {
                demo.query = query;
                demo.filter_due = Some(Instant::now() + SETTLE);
            });
        let details = button("Details").on_click(|demo: &mut Self| demo.details_wanted = true);
        let notes_view = custom(|| CodeView::new(""))
            .sync(move |view: &mut CodeView| {
                if view.text() != notes {
                    view.set_text(notes.as_str());
                }
            })
            .on_response(
                |view: &CodeView, response| {
                    (response == CodeViewResponse::Moved).then(|| view.cursor())
                },
                |demo: &mut Self, cursor| demo.notes_cursor = cursor,
            )
            .access(|view: &CodeView, frame, id| view.access(frame, id));
        let sketch = canvas(|frame: &mut Frame, area: Rect, theme: &Theme| {
            for point in self.strokes.iter().flatten() {
                let dot = Rect::new(area.x + point.x, area.y + point.y, DOT, DOT);
                frame.fill_rect(dot, theme.color(ThemeToken::Accent));
            }
        })
        .on_key(
            |key| key == Key::new(KeyCode::Char('c'), Mods::NONE),
            |demo: &mut Self, _| demo.strokes.clear(),
        )
        .name("Sketch");
        column([
            row([filter.into(), details.into()]),
            row([self.shelf(Shelf::Library), self.shelf(Shelf::Reading)])
                .align(Align::Stretch)
                .gap(Space::Cells(2.0)),
            Element::from(notes_view).grow(1.0),
            Element::from(sketch)
                .height(Extent::Px(SKETCH_HEIGHT))
                .border(ThemeToken::Border)
                .on_pointer_drag(|demo: &mut Self, drag| demo.sketch(drag)),
        ])
        .padding(1.0, 0.5)
    }

    /// The details window: how many books each shelf holds.
    fn details_view(&self) -> Element<'_, Self> {
        column([
            text(format!("{} in the library", self.library.len())).into(),
            text(format!("{} being read", self.reading.len())).into(),
        ])
        .padding(1.0, 0.5)
    }
}

impl App for Workbench {
    fn view(&self, window: WindowId) -> Element<'_, Self> {
        match Some(window) == self.details {
            true => self.details_view(),
            false => self.main_view(),
        }
    }

    /// Moves the selected book to the other shelf on `m`, the keyboard's
    /// way to do what dragging does; quits on Esc.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        if key == Key::new(KeyCode::Esc, Mods::NONE) {
            cx.exit();
            return true;
        }
        if key != Key::new(KeyCode::Char('m'), Mods::NONE) {
            return false;
        }
        if let (Some(shelf), Some(title)) = (self.selected_shelf(), self.selected.clone()) {
            self.move_book(&title, shelf);
        }
        true
    }

    /// Opens the details window a callback asked for, and applies the
    /// filter once typing has settled.
    fn about_to_wait(&mut self, now: Instant, cx: &mut AppCx<'_>) -> Option<Instant> {
        if std::mem::take(&mut self.details_wanted) && self.details.is_none() {
            self.details = cx.open_window(WindowOptions::new(DETAILS_TITLE)).ok();
        }
        match self.filter_due {
            Some(due) if due <= now => {
                self.filter_due = None;
                self.filter = self.query.clone();
                None
            }
            due => due,
        }
    }

    fn window_closed(&mut self, window: WindowId) {
        if Some(window) == self.details {
            self.details = None;
        }
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), misty::Error> {
    misty::run_app(Workbench::new(), WindowOptions::new(TITLE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use std::path::PathBuf;

    use misty::accesskit::Role;
    use misty::testing::TreeWindow;
    use misty::{FileInput, Input, PointerButton};

    use super::*;

    /// The window's width in the tests, in physical pixels.
    const WIDTH: u32 = 900;

    /// The window's height in the tests.
    const HEIGHT: u32 = 700;

    /// The window showing the demo at start.
    fn window() -> TreeWindow<Workbench> {
        TreeWindow::new(Workbench::new(), TITLE, WIDTH, HEIGHT)
    }

    /// A point inside the reading shelf, which stretches as tall as the
    /// library beside it: level with the library's first card.
    fn reading_shelf(window: &TreeWindow<Workbench>) -> Point {
        let card = window.center(Role::Button, BOOKS[0]);
        let width = SHELF_COLUMNS * window.frame().cell_metrics().width;
        Point::new(card.x + width, card.y)
    }

    #[test]
    fn test_a_card_dragged_to_the_other_shelf_moves_there() {
        let mut window = window();
        let from = window.center(Role::Button, BOOKS[0]);
        let to = reading_shelf(&window);
        window.drag(from, to);
        assert_eq!(window.app().reading, [BOOKS[0]]);
        assert_eq!(window.app().library.len(), BOOKS.len() - 1);
    }

    #[test]
    fn test_a_pressed_card_is_selected_and_m_moves_it_without_the_pointer() {
        let mut window = window();
        window.click(Role::Button, BOOKS[1]);
        assert_eq!(window.app().selected.as_deref(), Some(BOOKS[1]));
        window.press("m");
        assert_eq!(window.app().reading, [BOOKS[1]]);
        // The notes in the embedded code view follow the selection.
        let notes = window.app().notes();
        assert!(notes.contains(BOOKS[1]));
    }

    #[test]
    fn test_the_filter_follows_typing_once_it_settles() {
        let mut window = window();
        window.click(Role::TextInput, "");
        window.type_text("unix");
        let due = window.app().filter_due.expect("typing set the timer");
        window.about_to_wait_at(due - SETTLE / 2);
        assert!(window.app().filter.is_empty());
        window.about_to_wait_at(due);
        assert_eq!(window.app().filter, "unix");
        assert_eq!(
            window
                .drawn_text()
                .iter()
                .filter(|line| line.contains("Pattern"))
                .count(),
            0
        );
    }

    #[test]
    fn test_a_file_dropped_on_a_shelf_joins_it() {
        let mut window = window();
        let file = FileInput {
            path: PathBuf::from("/tmp/Dune.epub"),
            position: Some(reading_shelf(&window)),
        };
        window.send(&Input::FileHovered(file.clone()));
        window.send(&Input::FileDropped(file));
        assert_eq!(window.app().reading, ["Dune"]);
    }

    #[test]
    fn test_strokes_drawn_on_the_sketch_are_kept_and_c_clears_them() {
        let mut window = window();
        let tree = misty::testing::access_tree(window.frame(), TITLE);
        let sketch = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::Canvas)
            .and_then(|(_, node)| node.bounds())
            .expect("the sketch described");
        let from = Point::new(sketch.x0 as f32 + 10.0, sketch.y0 as f32 + 10.0);
        window.drag(from, Point::new(from.x + 40.0, from.y + 20.0));
        assert_eq!(window.app().strokes.len(), 1);
        assert_eq!(window.app().strokes[0][0], Point::new(10.0, 10.0));
        // The press focused the sketch, which takes `c`.
        window.click_at(from, PointerButton::Left);
        window.press("c");
        assert!(window.app().strokes.is_empty());
    }

    #[test]
    fn test_details_opens_a_second_window_with_its_own_view() {
        let mut window = window();
        let first = window.window_id();
        window.click(Role::Button, "Details");
        window.about_to_wait();
        let details = window.app().details.expect("the window opened");
        assert_eq!(window.windows(), [first, details]);
        window.switch_to(details);
        assert!(
            window
                .drawn_text()
                .iter()
                .any(|line| line == "4 in the library")
        );
        window.send(&Input::CloseRequested);
        assert_eq!(window.app().details, None);
    }
}

//! A virtualized grid of thumbnails: the PNG and JPEG files of the folder
//! named on the command line, or 10,000 generated pictures when none is
//! named. Thumbnails are decoded (or drawn) on four worker threads, a
//! little slowed down for generated pictures so the placeholders show,
//! and kept in a cache of a few hundred.
//!
//! The arrows (or `h`, `j`, `k`, `l`, `C-p`, `C-n`) move, `0` and `$` go
//! to a row's ends, and the rest of the Vim list keys move over rows as in
//! the list example (`C-f`, `C-d`, `C-e`, `H`, `M`, `L`, `g g`, `G`,
//! `z z`, counts such as `5j`; the line at the bottom shows a pending
//! count or prefix), Space selects, Enter opens (the line at the bottom
//! names what), `+` and `-` zoom. Click to move the cursor,
//! Ctrl+click to toggle the selection, double-click to open, drag from
//! empty space for a rubber band, wheel to scroll, Ctrl+wheel to zoom.
//! The rows glide to each new scroll; pass `--reduced-motion` to make them
//! jump. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example icons -- ~/Pictures
//! ```

use std::borrow::Cow;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use misty::{AppContext, Application, Error, FocusId, Frame, Image, Input, Key, KeyCode, Mods, Point, Rect, Span, StyledText, Theme, ThemeToken, Widget, WindowId, WindowOptions, key_label};
use misty::widgets::{GridSource, IconGrid, IconGridResponse, Thumbnails};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty icons";

/// The flag that turns the glide off.
const REDUCED_MOTION: &str = "--reduced-motion";

/// How many pictures are generated when no folder is named.
const GENERATED_COUNT: usize = 10_000;

/// The side of a generated picture, in pixels.
const GENERATED_SIDE: u32 = 256;

/// How long drawing a generated picture is made to take.
const GENERATED_DELAY: Duration = Duration::from_millis(30);

/// The threads thumbnails are loaded on.
const WORKERS: usize = 4;

/// The most thumbnails kept.
const CACHE_CAPACITY: usize = 400;

/// The largest scale factor thumbnails are kept sharp for.
const MAX_SCALE: f32 = 2.0;

/// The accessibility id of the grid.
const GRID: FocusId = FocusId::new("pictures");

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the grid, its thumbnails, and a status line.
struct IconsDemo {
    /// The grid widget.
    grid: IconGrid,
    /// The pictures shown.
    pictures: Pictures,
    /// Whether `--reduced-motion` asked for jumps whatever the system says.
    reduced_motion: bool,
    /// What happened last, for the status line.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The thumbnails, made once the event loop runs.
    thumbnails: Option<Thumbnails<Picture>>,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// The pictures in the grid.
struct Pictures(Vec<Picture>);

/// One picture: a file, or a generated one by number.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Picture {
    /// A file to decode.
    File(PathBuf),
    /// A picture drawn from its number.
    Generated(usize),
}

// ========================================================================
// Pictures Implementations
// ========================================================================

impl Pictures {
    /// The PNG and JPEG files in `folder`, by name, or the generated
    /// pictures when `folder` is `None`.
    fn new(folder: Option<PathBuf>) -> Self {
        let Some(folder) = folder else {
            return Self((0..GENERATED_COUNT).map(Picture::Generated).collect());
        };
        let mut files: Vec<PathBuf> = fs::read_dir(folder)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| is_picture(path))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        Self(files.into_iter().map(Picture::File).collect())
    }
}

impl GridSource for Pictures {
    type Key = Picture;

    fn item_count(&self) -> usize {
        self.0.len()
    }

    fn item_key(&self, index: usize) -> Picture {
        self.0[index].clone()
    }

    fn item_label(&self, index: usize) -> Cow<'_, str> {
        match &self.0[index] {
            Picture::File(path) => path
                .file_name()
                .map_or(Cow::Borrowed("?"), |name| name.to_string_lossy()),
            Picture::Generated(number) => Cow::Owned(format!("picture_{number}.png")),
        }
    }
}

// ========================================================================
// IconsDemo Implementations
// ========================================================================

impl IconsDemo {
    /// Acts on what the grid made of a key or an event; whether it used it.
    fn settle(&mut self, response: IconGridResponse) -> bool {
        match response {
            IconGridResponse::Activated(index) => {
                self.status = format!("opened {}", self.pictures.item_label(index));
            }
            IconGridResponse::Handled => {}
            IconGridResponse::Ignored => return false,
            _ => {}
        }
        true
    }
}

impl Application for IconsDemo {
    type Message = ();

    fn init(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
        if self.reduced_motion {
            cx.set_reduced_motion(Some(true));
        }
        let side = (IconGrid::max_thumbnail_side() * MAX_SCALE) as u32;
        self.thumbnails = Some(Thumbnails::new(
            cx.waker(),
            WORKERS,
            CACHE_CAPACITY,
            side,
            load,
        ));
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let thumbnails = self.thumbnails.as_mut()?;
        let mut window_cx = cx.event_cx(window);
        if thumbnails.poll() {
            window_cx.request_redraw();
        }
        self.grid
            .lend(&self.pictures, thumbnails)
            .about_to_wait(&mut window_cx)
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        if let Some(thumbnails) = &mut self.thumbnails {
            let response = self
                .grid
                .lend(&self.pictures, thumbnails)
                .handle(input, &mut cx.event_cx(window));
            if self.settle(response) {
                return;
            }
        }
        let escape = Input::Key(Key::new(KeyCode::Esc, Mods::NONE));
        if *input == escape || *input == Input::CloseRequested {
            cx.exit();
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let bounds = frame.bounds();
        let metrics = frame.cell_metrics();
        let area = Rect::new(
            bounds.x,
            bounds.y,
            bounds.width,
            (bounds.height - metrics.height).max(0.0),
        );
        if let Some(thumbnails) = &mut self.thumbnails {
            self.grid
                .lend(&self.pictures, thumbnails)
                .draw(frame, area, &self.theme);
            self.grid.access(frame, GRID, &self.pictures);
        }
        let status = StyledText::new()
            .with(Span::new(
                format!(
                    " {} of {}, {} selected, zoom {}   {}{}   ",
                    self.grid.cursor() + 1,
                    self.pictures.item_count(),
                    self.grid.selected().count(),
                    self.grid.zoom(),
                    self.grid
                        .keys()
                        .count()
                        .map_or_else(String::new, |count| count.to_string()),
                    key_label(self.grid.keys().pending_keys(), &self.theme.strings),
                ),
                ThemeToken::Muted.into(),
            ))
            .with(Span::new(self.status.as_str(), ThemeToken::Accent.into()))
            .with_ellipsis();
        let y = area.y + area.height;
        let clip = Rect::new(bounds.x, y, bounds.width, metrics.height);
        frame.draw_text(&status, Point::new(bounds.x, y), clip, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Loads `picture` on a worker: decodes its file, or draws it.
fn load(picture: &Picture) -> Result<Image, String> {
    match picture {
        Picture::File(path) => {
            let bytes = fs::read(path).map_err(|error| error.to_string())?;
            Image::decode(&bytes).map_err(|error| error.to_string())
        }
        Picture::Generated(number) => {
            thread::sleep(GENERATED_DELAY);
            generate(*number).map_err(|error| error.to_string())
        }
    }
}

/// Picture `number`: a diagonal gradient between two colors made from it.
fn generate(number: usize) -> Result<Image, misty::ImageError> {
    let side = GENERATED_SIDE as usize;
    let hue = (number * 47 % 256) as u8;
    let mut pixels = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let t = ((x + y) * 255 / (2 * side)) as u8;
            pixels.extend_from_slice(&[hue, t, 255 - hue / 2, 255]);
        }
    }
    Image::from_rgba(GENERATED_SIDE, GENERATED_SIDE, pixels)
}

/// Whether `path` names a PNG or JPEG file.
fn is_picture(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    matches!(extension.as_deref(), Some("png" | "jpg" | "jpeg"))
}

fn main() -> Result<(), Error> {
    let folder = env::args_os()
        .skip(1)
        .find(|arg| arg != REDUCED_MOTION)
        .map(PathBuf::from);
    let app = IconsDemo {
        grid: IconGrid::new(),
        pictures: Pictures::new(folder),
        reduced_motion: env::args_os().any(|arg| arg == REDUCED_MOTION),
        status: String::from("+ and - zoom, drag from empty space to select"),
        theme: Theme::dark(),
        thumbnails: None,
        window: None,
    };
    misty::run(app, WindowOptions::new(TITLE))
}

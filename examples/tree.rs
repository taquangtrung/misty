//! A folder tree over the file system, from the directory named on the
//! command line (or the current one). A folder's entries are read on a
//! worker thread when it is first expanded, a little slowed down so the
//! loading row shows, and handed back through the app's waker.
//!
//! Up and Down (or `j` and `k`) move, Right (`l`) expands a folder or
//! moves into it, Left (`h`) collapses it or moves to its parent, and the
//! rest of the Vim list keys move as in the list example (`C-f`, `C-d`,
//! `C-e`, `H`, `M`, `L`, `g g`, `G`, `z z`, counts such as `5j`; the line
//! at the bottom shows a pending count or prefix), Space selects, Enter
//! opens (the line at the bottom names what). A click on a folder's arrow expands or
//! collapses it. F5 reads the cursor's folder again: what stays expanded
//! and where the cursor is survive. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example tree -- ~/Documents
//! ```

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use misty::{
    AppContext, Application, Error, FocusId, Frame, Input, Key, KeyCode, Mods, Point, Rect, Span,
    StyledText, Theme, ThemeToken, TreeItem, TreeView, TreeViewResponse, Waker, Widget, WindowId,
    WindowOptions, key_label,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty tree";

/// How long a folder read is held back, so its loading row shows.
const LOAD_DELAY: Duration = Duration::from_millis(400);

/// The accessibility id of the tree.
const TREE: FocusId = FocusId::new("folders");

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the folder tree and a status line.
struct TreeDemo {
    /// What happened last, for the status line.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The tree widget, over paths.
    tree: TreeView<PathBuf>,
    /// Sends read folders back to the app, once it runs.
    waker: Option<Waker<Loaded>>,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// A folder read on a worker thread.
struct Loaded {
    /// The folder.
    folder: PathBuf,
    /// Its entries, folders first, or why it could not be read.
    entries: io::Result<Vec<TreeItem<PathBuf>>>,
}

// ========================================================================
// TreeDemo Implementations
// ========================================================================

impl TreeDemo {
    /// Acts on what the tree made of a key or an event; whether it used it.
    fn settle(&mut self, response: TreeViewResponse<PathBuf>) -> bool {
        match response {
            TreeViewResponse::Activated(path) => self.status = format!("opened {}", path.display()),
            TreeViewResponse::Collapsed(_)
            | TreeViewResponse::Expanded(_)
            | TreeViewResponse::Handled => {}
            TreeViewResponse::Ignored => return false,
            TreeViewResponse::Load(folder) => self.load(folder),
            _ => {}
        }
        true
    }

    /// Reads `folder` on a worker thread, handing its entries back
    /// through the waker.
    fn load(&mut self, folder: PathBuf) {
        let Some(waker) = self.waker.clone() else {
            return;
        };
        self.status = format!("reading {}", folder.display());
        thread::spawn(move || {
            thread::sleep(LOAD_DELAY);
            let entries = read_folder(&folder);
            // The app has quit when this fails: nothing is waiting.
            let _ = waker.send(Loaded { folder, entries });
        });
    }
}

impl Application for TreeDemo {
    type Message = Loaded;

    fn init(&mut self, cx: &mut AppContext<'_, Loaded>, window: WindowId) {
        self.window = Some(window);
        self.waker = Some(cx.waker());
    }

    fn message(&mut self, cx: &mut AppContext<'_, Loaded>, loaded: Loaded) {
        match loaded.entries {
            Ok(entries) => {
                self.status = format!("{} entries in {}", entries.len(), loaded.folder.display());
                self.tree.set_children(&loaded.folder, entries);
            }
            Err(error) => {
                self.status = format!("cannot read {}", loaded.folder.display());
                self.tree.set_load_failed(&loaded.folder, error.to_string());
            }
        }
        if let Some(window) = self.window {
            cx.event_cx(window).request_redraw();
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, Loaded>) -> Option<Instant> {
        let window = self.window?;
        self.tree.about_to_wait(&mut cx.event_cx(window))
    }

    fn input(&mut self, cx: &mut AppContext<'_, Loaded>, window: WindowId, input: &Input) {
        if input.key() == Some(Key::new(KeyCode::F(5), Mods::NONE)) {
            cx.event_cx(window).request_redraw();
            // An expanded folder at the cursor, else the folder holding it.
            let folder = self
                .tree
                .cursor()
                .map(|path| match self.tree.is_expanded(path) {
                    true => path.clone(),
                    false => path
                        .parent()
                        .map_or_else(|| path.clone(), Path::to_path_buf),
                });
            if let Some(folder) = folder.filter(|folder| self.tree.reload(folder)) {
                self.load(folder);
            }
            return;
        }
        let response = self.tree.handle(input, &mut cx.event_cx(window));
        if self.settle(response) {
            return;
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
        self.tree.draw(frame, area, &self.theme);
        self.tree.access(frame, TREE);
        let selected = self.tree.selected().len();
        let tree_keys = self.tree.keys();
        let count = tree_keys
            .count()
            .map_or_else(String::new, |count| count.to_string());
        let pending = key_label(tree_keys.pending_keys(), &self.theme.strings);
        let status = StyledText::new()
            .with(Span::new(
                format!(" {selected} selected   {count}{pending}   "),
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

/// The entries of `folder` as tree items, folders first, each group by
/// name; hidden entries left out.
fn read_folder(folder: &Path) -> io::Result<Vec<TreeItem<PathBuf>>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        entries.push((is_folder, name, entry.path()));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let items = entries
        .into_iter()
        .map(|(is_folder, name, path)| match is_folder {
            true => TreeItem::branch(path, name),
            false => TreeItem::leaf(path, name),
        })
        .collect();
    Ok(items)
}

fn main() -> Result<(), Error> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut tree = TreeView::new();
    let label = root.display().to_string();
    tree.set_roots(vec![TreeItem::branch(root, label)]);
    let app = TreeDemo {
        status: String::from("Right expands, Left collapses, F5 reloads"),
        theme: Theme::dark(),
        tree,
        waker: None,
        window: None,
    };
    misty::run(app, WindowOptions::new(TITLE))
}

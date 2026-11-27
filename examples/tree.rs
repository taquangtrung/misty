//! A folder tree over the file system, described as a tree app, from the
//! directory named on the command line (or the current one). A folder's
//! entries are read on a worker thread when it is first expanded, a
//! little slowed down so the loading row shows, and handed back to the
//! app's state through its waker.
//!
//! Up and Down (or `j` and `k`) move, Right (`l`) expands a folder or
//! moves into it, Left (`h`) collapses it or moves to its parent, and the
//! rest of the Vim list keys move as in the list example (`C-f`, `C-d`,
//! `C-e`, `H`, `M`, `L`, `g g`, `G`, `z z`, counts such as `5j`; the line
//! at the bottom shows a pending count or prefix), Space selects, Enter
//! opens (the line at the bottom names what). A click on a folder's arrow
//! expands or collapses it. F5 reads the cursor's folder again: what stays
//! expanded and where the cursor is survive. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example tree -- ~/Documents
//! ```

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use misty::view::{column, text, tree_view};
use misty::{
    App, AppCx, Element, Key, KeyCode, KeyScheme, ListKeys, Mods, Theme, ThemeToken, TreeItem,
    TreeItems, Update, Waker, WindowId, WindowOptions, key_label,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty tree";

/// How long a folder read is held back, so its loading row shows.
const LOAD_DELAY: Duration = Duration::from_millis(400);

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the folders read so far, the tree's cursor and selection,
/// and a status line.
struct TreeDemo {
    /// The item the cursor is on.
    cursor: Option<PathBuf>,
    /// The folders expanded, which F5 reads again.
    expanded: HashSet<PathBuf>,
    /// The folders and what was read of them.
    items: TreeItems<PathBuf>,
    /// The count and prefix typed toward a motion, for the status line.
    keys: ListKeys,
    /// The items selected, top to bottom.
    selected: Vec<PathBuf>,
    /// What happened last, for the status line.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
    /// Hands read folders back to the app, once it runs.
    waker: Option<Waker<Update<Self>>>,
}

// ========================================================================
// TreeDemo Implementations
// ========================================================================

impl TreeDemo {
    /// The demo over the folder `root`, nothing read yet.
    fn new(root: PathBuf) -> Self {
        let label = root.display().to_string();
        Self {
            cursor: Some(root.clone()),
            expanded: HashSet::new(),
            items: TreeItems::new(vec![TreeItem::branch(root, label)]),
            keys: ListKeys::default(),
            selected: Vec::new(),
            status: String::from("Right expands, Left collapses, F5 reloads"),
            theme: Theme::dark(),
            waker: None,
        }
    }

    /// Reads `folder` on a worker thread, handing its entries back through
    /// the waker.
    fn load(&mut self, folder: PathBuf) {
        self.expanded.insert(folder.clone());
        let Some(waker) = self.waker.clone() else {
            return;
        };
        self.status = format!("reading {}", folder.display());
        thread::spawn(move || {
            thread::sleep(LOAD_DELAY);
            let entries = read_folder(&folder);
            // The app has quit when this fails: nothing is waiting.
            let _ = waker.send(Box::new(move |demo: &mut Self| {
                demo.loaded(&folder, entries)
            }));
        });
    }

    /// Takes in the entries read of `folder`, or why they could not be.
    fn loaded(&mut self, folder: &Path, entries: io::Result<Vec<TreeItem<PathBuf>>>) {
        let folder = folder.to_path_buf();
        match entries {
            Ok(entries) => {
                self.status = format!("{} entries in {}", entries.len(), folder.display());
                self.items.set_children(&folder, entries);
            }
            Err(error) => {
                self.status = format!("cannot read {}", folder.display());
                self.items.set_load_failed(&folder, error.to_string());
            }
        }
    }

    /// Reads the cursor's folder again: the folder itself when it is
    /// expanded, else the folder holding it.
    fn reload(&mut self) {
        let folder = self
            .cursor
            .as_ref()
            .map(|path| match self.expanded.contains(path) {
                true => path.clone(),
                false => path
                    .parent()
                    .map_or_else(|| path.clone(), Path::to_path_buf),
            });
        let Some(folder) = folder else {
            return;
        };
        self.items.reload(&folder);
        if self.expanded.contains(&folder) {
            self.load(folder);
        }
    }

    /// The muted half of the status line: how many items are selected and
    /// what is typed toward a motion.
    fn position(&self) -> String {
        let count = self
            .keys
            .count()
            .map_or_else(String::new, |count| count.to_string());
        let pending = key_label(self.keys.pending_keys(), &self.theme.strings);
        format!(" {} selected   {count}{pending}   ", self.selected.len())
    }
}

impl App for TreeDemo {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        column([
            tree_view(&self.items, self.cursor.as_ref())
                .monospace()
                .selected(&self.selected)
                .on_cursor_move(|demo: &mut Self, path| demo.cursor = path)
                .on_select(|demo: &mut Self, paths| demo.selected = paths)
                .on_keys(|demo: &mut Self, keys| demo.keys = keys)
                .on_activate(|demo: &mut Self, path| {
                    demo.status = format!("opened {}", path.display());
                })
                .on_expand(|demo: &mut Self, path| {
                    demo.expanded.insert(path);
                })
                .on_collapse(|demo: &mut Self, path| {
                    demo.expanded.remove(&path);
                })
                .on_load(Self::load)
                .into(),
            text(self.position())
                .muted()
                .then(self.status.as_str())
                .color(ThemeToken::Accent)
                .into(),
        ])
    }

    fn start(&mut self, waker: Waker<Update<Self>>) {
        self.waker = Some(waker);
    }

    /// Reads the cursor's folder again on F5, and quits on Esc.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        match key {
            key if key == Key::new(KeyCode::F(5), Mods::NONE) => self.reload(),
            key if key == Key::new(KeyCode::Esc, Mods::NONE) => cx.exit(),
            _ => return false,
        }
        true
    }

    /// The Vim and Emacs keys, as a keyboard-first app takes them.
    fn key_scheme(&self) -> KeyScheme {
        KeyScheme::VIM_AND_EMACS
    }

    fn theme(&self) -> &Theme {
        &self.theme
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

fn main() -> Result<(), misty::Error> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    misty::run_app(TreeDemo::new(root), WindowOptions::new(TITLE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use std::time::Instant;

    use misty::testing::TreeWindow;

    use super::*;

    /// The window's width in the tests, in physical pixels.
    const WIDTH: u32 = 800;

    /// The window's height in the tests.
    const HEIGHT: u32 = 600;

    /// The folder the captures were made over; it need not exist.
    const ROOT: &str = "/misty-tree-root";

    /// The longest a test waits for a folder read on a worker thread.
    const READ_TIMEOUT: Duration = Duration::from_secs(10);

    /// How long a test sleeps between looks for a folder read.
    const READ_POLL: Duration = Duration::from_millis(20);

    /// What the hand-driven version of this example (an `Application`
    /// driving a `TreeView` that kept its items) described to screen
    /// readers at start, captured from it before it was rewritten; the
    /// constants after it are the same capture after Space and Enter, with
    /// the root's children handed over and the root expanded (`l`), and
    /// with the folder `a` expanded and loading (`j l`).
    const HAND_DRIVEN_START: &str = "\
Window \"misty tree\" focused
  Tree
    TreeItem \"misty-tree-root\" collapsed
";

    /// The hand-driven start's text drawn.
    const HAND_DRIVEN_START_DRAWN: [&str; 2] = [
        "misty-tree-root",
        " 0 selected      Right expands, Left collapses, F5 reloads",
    ];

    /// The hand-driven description after Space and Enter.
    const HAND_DRIVEN_OPENED: &str = "\
Window \"misty tree\" focused
  Tree
    TreeItem \"misty-tree-root\" selected collapsed
";

    /// The hand-driven text drawn after Space and Enter.
    const HAND_DRIVEN_OPENED_DRAWN: [&str; 2] = [
        "misty-tree-root",
        " 1 selected      opened /misty-tree-root",
    ];

    /// The hand-driven description with the root expanded.
    const HAND_DRIVEN_EXPANDED: &str = "\
Window \"misty tree\" focused
  Tree
    TreeItem \"misty-tree-root\" selected expanded
    TreeItem \"a\" collapsed
    TreeItem \"b.txt\"
";

    /// The hand-driven description with the folder `a` loading.
    const HAND_DRIVEN_LOADING: &str = "\
Window \"misty tree\" focused
  Tree
    TreeItem \"misty-tree-root\" selected expanded
    TreeItem \"a\" expanded
    TreeItem \"Loading…\"
    TreeItem \"b.txt\"
";

    /// The hand-driven text drawn with the folder `a` loading.
    const HAND_DRIVEN_LOADING_DRAWN: [&str; 6] = [
        "misty-tree-root",
        "a",
        "Loading",
        "…",
        "b.txt",
        " 1 selected      reading /misty-tree-root/a",
    ];

    /// The window showing the demo over `root`, its row saying `label`.
    fn window_over(root: PathBuf, label: &str) -> TreeWindow<TreeDemo> {
        let mut demo = TreeDemo::new(root.clone());
        demo.items.set_roots(vec![TreeItem::branch(root, label)]);
        TreeWindow::new(demo, TITLE, WIDTH, HEIGHT)
    }

    /// The window showing the demo over the captures' folder.
    fn window() -> TreeWindow<TreeDemo> {
        window_over(PathBuf::from(ROOT), "misty-tree-root")
    }

    /// `hand_driven` with the edit the tree makes: the keyboard is on the
    /// tree, so the cursor's row (line `cursor` of the description) is
    /// focused rather than the window.
    fn tree_described(hand_driven: &str, cursor: usize) -> String {
        hand_driven
            .lines()
            .enumerate()
            .map(|(line, text)| match line {
                0 => format!("{}\n", text.trim_end_matches(" focused")),
                _ if line == cursor => format!("{text} focused\n"),
                _ => format!("{text}\n"),
            })
            .collect()
    }

    /// Hands the captures' children of the root to `window`'s app.
    fn hand_over_children(window: &mut TreeWindow<TreeDemo>) {
        window.update(|demo| {
            let children = vec![
                TreeItem::branch(PathBuf::from("/misty-tree-root/a"), "a"),
                TreeItem::leaf(PathBuf::from("/misty-tree-root/b.txt"), "b.txt"),
            ];
            demo.items.set_children(&PathBuf::from(ROOT), children);
        });
    }

    /// Runs the updates `window`'s worker threads send until `count` ran.
    fn deliver(window: &mut TreeWindow<TreeDemo>, count: usize) {
        let start = Instant::now();
        let mut delivered = 0;
        while delivered < count {
            assert!(start.elapsed() < READ_TIMEOUT, "no folder read came back");
            thread::sleep(READ_POLL);
            delivered += window.deliver();
        }
    }

    #[test]
    fn test_the_tree_describes_and_draws_what_the_hand_driven_example_did() {
        let mut window = window();
        assert_eq!(window.ui_text(), tree_described(HAND_DRIVEN_START, 2));
        assert_eq!(window.drawn_text(), HAND_DRIVEN_START_DRAWN);
        window.press("<space> RET");
        assert_eq!(window.ui_text(), tree_described(HAND_DRIVEN_OPENED, 2));
        assert_eq!(window.drawn_text(), HAND_DRIVEN_OPENED_DRAWN);
        hand_over_children(&mut window);
        window.press("l");
        assert_eq!(window.ui_text(), tree_described(HAND_DRIVEN_EXPANDED, 2));
        window.press("j l");
        assert_eq!(window.ui_text(), tree_described(HAND_DRIVEN_LOADING, 3));
        assert_eq!(window.drawn_text(), HAND_DRIVEN_LOADING_DRAWN);
    }

    #[test]
    fn test_a_folder_is_read_on_a_worker_and_reloaded_with_f5() {
        let root = env::temp_dir().join(format!("misty-tree-test-{}", std::process::id()));
        fs::create_dir_all(root.join("inner")).expect("the test folder is made");
        fs::write(root.join("notes.txt"), "").expect("a file is made");
        let mut window = window_over(root.clone(), "root");
        window.press("l");
        assert!(window.ui_text().contains("TreeItem \"Loading…\""));
        deliver(&mut window, 1);
        let text = window.ui_text();
        assert!(text.contains("TreeItem \"inner\" collapsed"), "{text}");
        assert!(text.contains("TreeItem \"notes.txt\""), "{text}");
        let status = &window.app().status;
        assert!(status.starts_with("2 entries in"), "{status}");
        fs::write(root.join("todo.md"), "").expect("a file is made");
        window.press("j <f5>");
        let text = window.ui_text();
        assert!(
            text.contains("TreeItem \"Loading…\""),
            "the reload shows: {text}"
        );
        assert!(text.contains("expanded"), "the root stays expanded: {text}");
        deliver(&mut window, 1);
        let text = window.ui_text();
        assert!(text.contains("TreeItem \"todo.md\""), "{text}");
        let stayed = text.contains("TreeItem \"inner\" collapsed focused");
        assert!(stayed, "the cursor stayed on its folder: {text}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_a_folder_that_cannot_be_read_says_why_and_left_collapses_it() {
        let mut window = window();
        window.press("l");
        deliver(&mut window, 1);
        assert_eq!(window.app().status, "cannot read /misty-tree-root");
        window.press("h");
        let text = window.ui_text();
        assert!(
            text.contains("TreeItem \"misty-tree-root\" collapsed"),
            "{text}"
        );
        assert!(!window.app().expanded.contains(&PathBuf::from(ROOT)));
    }

    #[test]
    fn test_esc_quits() {
        let mut window = window();
        window.press("<esc>");
        assert!(window.is_closed());
    }
}

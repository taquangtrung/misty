//! [`FileBrowser`]: one folder at a time, its entries filtered by what is
//! typed, for choosing a file or a folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::super::super::base::key::{Key, KeyCode, Mods};
use super::super::row::{Highlights, MatchOn, Row, RowKey};
use super::super::source::{Action, ActionId, Feed, Matching, Resume, Source, SourceKey};
use super::{BrowserKey, BrowserWords, Chosen, FolderReader, LocalFolders, Ranker, file_name};

// ========================================================================
// Constants
// ========================================================================

/// The row standing for the folder shown.
const CURRENT_FOLDER: &str = ".";

/// The one action of every row, run by `Enter`.
const CHOOSE: ActionId = ActionId("choose");

/// The key the new file row is told apart by.
const NEW_FILE_ROW: RowKey = RowKey(u64::MAX);

// ========================================================================
// Data Structures
// ========================================================================

/// What the app makes of a choice.
type Choose<O> = Box<dyn Fn(Chosen) -> Option<O> + Send>;

/// What the app's keymap makes of a key.
type KeyMeaning = Box<dyn Fn(&Key) -> Option<BrowserKey> + Send>;

/// Where a typed path goes: the path typed, the folder shown, its folders'
/// names.
type TypedPath = Box<dyn Fn(&str, &Path, &[String]) -> PathBuf + Send>;

/// A [`Source`] showing one folder at a time: a `.` row for the folder
/// shown, its folders (each named with a trailing `/`), then (unless it
/// chooses a folder) its files, filtered by what is typed.
///
/// `Tab` or `Enter` on a folder goes into it, and `Tab` on `.` chooses the
/// folder shown; `Backspace` in an empty query, or a key the app's keys
/// say is [`BrowserKey::Up`], goes up; a path typed and ended with `/` goes
/// there (`/` alone to the root, `~/` to the home folder given); `Enter`
/// on a file or on `.` chooses it. What choosing does is the app's
/// `choose`, handed a [`Chosen`].
pub struct FileBrowser<O> {
    choose: Choose<O>,
    /// The entries shown: `.` first.
    entries: Vec<Entry>,
    /// The folder shown.
    folder: PathBuf,
    /// Whether it chooses a folder, listing no files.
    folders_only: bool,
    /// The home folder `~` stands for, if the app gave one.
    home: Option<PathBuf>,
    id: &'static str,
    /// What the app's keymap makes of a key.
    keys: KeyMeaning,
    /// Whether a query naming no entry is offered as a new file.
    new_file: bool,
    ranker: Option<Arc<dyn Ranker>>,
    reader: Box<dyn FolderReader>,
    /// Why the folder shown could not be read, if it could not.
    unreadable: Option<String>,
    /// Where a typed path goes from the folder shown, among its folders'
    /// names.
    typed_path: TypedPath,
    words: BrowserWords,
}

/// One row: the folder shown (`path` `None`), a folder, or a file.
struct Entry {
    is_folder: bool,
    label: String,
    path: Option<PathBuf>,
}

// ========================================================================
// FileBrowser Implementations
// ========================================================================

impl<O> FileBrowser<O> {
    /// A browser named `id` choosing a file (or, through `.`, the folder
    /// shown), starting at `folder`, its rows doing what `choose` makes of
    /// them.
    pub fn files(
        id: &'static str,
        words: BrowserWords,
        folder: PathBuf,
        choose: impl Fn(Chosen) -> Option<O> + Send + 'static,
    ) -> Self {
        let mut browser = Self {
            choose: Box::new(choose),
            entries: Vec::new(),
            folder,
            folders_only: false,
            home: None,
            id,
            keys: Box::new(|_| None),
            new_file: false,
            ranker: None,
            reader: Box::new(LocalFolders::new()),
            unreadable: None,
            typed_path: Box::new(literal_typed_path),
            words,
        };
        browser.list();
        browser
    }

    /// A browser named `id` choosing a folder, listing no files, starting
    /// at `folder`: `.` or a use key chooses the folder shown.
    pub fn folders(
        id: &'static str,
        words: BrowserWords,
        folder: PathBuf,
        choose: impl Fn(Chosen) -> Option<O> + Send + 'static,
    ) -> Self {
        let mut browser = Self::files(id, words, folder, choose);
        browser.folders_only = true;
        browser.list();
        browser
    }

    /// The same browser reading folders through `reader`: the files it
    /// lists (`LocalFolders::with_extensions`), or another place's.
    pub fn with_reader(mut self, reader: impl FolderReader + 'static) -> Self {
        self.reader = Box::new(reader);
        self.list();
        self
    }

    /// The same browser, `~` in a typed path standing for `home`.
    pub fn with_home(self, home: PathBuf) -> Self {
        Self {
            home: Some(home),
            ..self
        }
    }

    /// The same browser, the keys `keys` names going up or choosing the
    /// folder shown, as the app's keymap binds them.
    pub fn with_keys(self, keys: impl Fn(&Key) -> Option<BrowserKey> + Send + 'static) -> Self {
        Self {
            keys: Box::new(keys),
            ..self
        }
    }

    /// The same browser, a typed path read by `typed_path` (the path typed,
    /// the folder shown, its folders' names) rather than as written: a
    /// folder's name found by a fuzzy match, say. `~` in the answer is
    /// still the home folder.
    pub fn with_typed_paths(
        self,
        typed_path: impl Fn(&str, &Path, &[String]) -> PathBuf + Send + 'static,
    ) -> Self {
        Self {
            typed_path: Box::new(typed_path),
            ..self
        }
    }

    /// The same browser, its rows ranked and lit by `ranker` rather than
    /// matched by words.
    pub fn with_ranker(self, ranker: Arc<dyn Ranker>) -> Self {
        Self {
            ranker: Some(ranker),
            ..self
        }
    }

    /// The same browser offering a query that names no entry as a new
    /// file, after the matches, as [`BrowserWords::new_file`] names it.
    pub fn with_new_file(self) -> Self {
        Self {
            new_file: true,
            ..self
        }
    }

    /// The folder shown.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Reads the folder shown: `.`, its folders, then its files.
    fn list(&mut self) {
        let listing = match self.reader.read(&self.folder) {
            Ok(listing) => {
                self.unreadable = None;
                listing
            }
            Err(error) => {
                self.unreadable = Some(error.to_string());
                Default::default()
            }
        };
        let mut entries = vec![Entry {
            is_folder: true,
            label: CURRENT_FOLDER.to_owned(),
            path: None,
        }];
        entries.extend(listing.folders.into_iter().map(|path| Entry {
            is_folder: true,
            label: format!("{}/", file_name(&path)),
            path: Some(path),
        }));
        if !self.folders_only {
            entries.extend(listing.files.into_iter().map(|path| Entry {
                is_folder: false,
                label: file_name(&path),
                path: Some(path),
            }));
        }
        self.entries = entries;
    }

    /// Shows `folder` instead.
    fn go_to(&mut self, folder: PathBuf) -> SourceKey<O> {
        self.folder = folder;
        self.list();
        SourceKey::Reset
    }

    /// Shows the folder above, if there is one.
    fn go_up(&mut self) -> Option<SourceKey<O>> {
        let parent = self.folder.parent()?.to_path_buf();
        Some(self.go_to(parent))
    }

    /// The folder `typed` (a path ended with `/`) names from the one shown.
    fn typed_folder(&self, typed: &str) -> PathBuf {
        let names: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| entry.is_folder && entry.path.is_some())
            .filter_map(|entry| entry.label.strip_suffix('/').map(str::to_owned))
            .collect();
        let target = (self.typed_path)(typed, &self.folder, &names);
        self.expand_home(target)
    }

    /// `path` with a leading `~` made the home folder.
    fn expand_home(&self, path: PathBuf) -> PathBuf {
        let Some(home) = &self.home else {
            return path;
        };
        match path.strip_prefix("~") {
            Ok(rest) => home.join(rest),
            Err(_) => path,
        }
    }

    fn entry(&self, row: RowKey) -> Option<&Entry> {
        self.entries.get(usize::try_from(row.0).ok()?)
    }

    /// The entries as rows keyed by their places, in the order `places`
    /// gives.
    fn rows(&self, places: impl IntoIterator<Item = usize>) -> Vec<Row> {
        places
            .into_iter()
            .filter_map(|at| {
                let entry = self.entries.get(at)?;
                let row = Row::new(RowKey(at as u64), entry.label.as_str());
                Some(match entry.path {
                    None => row.with_hint(self.words.current_folder.as_str()),
                    Some(_) => row,
                })
            })
            .collect()
    }
}

impl<O> Source<O> for FileBrowser<O> {
    fn id(&self) -> &'static str {
        self.id
    }

    fn title(&self) -> String {
        format!("{}: {}", self.words.title, self.folder.display())
    }

    fn matching(&self) -> Matching {
        match self.ranker {
            Some(_) => Matching::BySource(MatchOn::LABEL),
            None => Matching::Words(MatchOn::LABEL),
        }
    }

    fn resume(&self) -> Resume {
        Resume::Reread
    }

    fn takes_word_at_point(&self) -> bool {
        false
    }

    fn feed(&mut self, query: &str) -> Feed {
        if let Some(why) = &self.unreadable {
            return Feed::Failed(why.clone());
        }
        let Some(ranker) = &self.ranker else {
            return Feed::Fixed(self.rows(0..self.entries.len()));
        };
        let labels: Vec<&str> = self
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        Feed::Fixed(self.rows(ranker.rank(&labels, query)))
    }

    fn highlights(&self, row: &Row, query: &str) -> Highlights {
        let Some(ranker) = &self.ranker else {
            return Highlights::default();
        };
        Highlights {
            label: ranker.lit(&row.label, query),
            ..Highlights::default()
        }
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        vec![Action {
            button: None,
            id: CHOOSE,
            title: self.words.open.clone(),
        }]
    }

    /// A query naming no entry, as a new file, when the app asked for it.
    fn query_row(&self, query: &str) -> Option<Row> {
        let name = query.trim();
        let named = self
            .entries
            .iter()
            .any(|entry| entry.label.trim_end_matches('/') == name);
        (self.new_file && !self.folders_only && !name.is_empty() && !named && !name.contains('/'))
            .then(|| Row::new(NEW_FILE_ROW, format!("{} {name}", self.words.new_file)))
    }

    /// `Enter` (or a click): `.` chooses the folder shown, a file the file,
    /// the new file row the file to make; a folder is gone into by `key`
    /// before this runs, so a click on one does nothing.
    fn run(&mut self, _action: ActionId, row: RowKey, query: &str) -> Option<O> {
        if row == NEW_FILE_ROW {
            let path = self.folder.join(query.trim());
            return (self.choose)(Chosen::NewFile(path));
        }
        let entry = self.entry(row)?;
        match (&entry.path, entry.is_folder) {
            (None, _) => (self.choose)(Chosen::Folder(self.folder.clone())),
            (Some(_), true) => None,
            (Some(path), false) => (self.choose)(Chosen::File(path.clone())),
        }
    }

    fn key(&mut self, key: &Key, row: Option<RowKey>, query: &str) -> Option<SourceKey<O>> {
        match (self.keys)(key) {
            Some(BrowserKey::Up) => return self.go_up(),
            Some(BrowserKey::UseFolder) => {
                let chosen = (self.choose)(Chosen::Folder(self.folder.clone()));
                return chosen.map(SourceKey::Done);
            }
            None => {}
        }
        let plain = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
        match key.code {
            KeyCode::Backspace if key.mods == Mods::NONE && query.is_empty() => self.go_up(),
            KeyCode::Char('/') if plain => {
                let target = self.typed_folder(query);
                target.is_dir().then(|| self.go_to(target))
            }
            KeyCode::Tab | KeyCode::Enter if key.mods == Mods::NONE => {
                let entry = self.entry(row?)?;
                match (&entry.path, entry.is_folder) {
                    (None, _) if key.code == KeyCode::Tab => {
                        let chosen = (self.choose)(Chosen::Folder(self.folder.clone()));
                        chosen.map(SourceKey::Done)
                    }
                    (Some(path), true) => {
                        let path = path.clone();
                        Some(self.go_to(path))
                    }
                    (None, _) | (Some(_), false) => None,
                }
            }
            _ => None,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A typed path as written: `/` alone the root, an absolute or `~` path as
/// it is, a folder of the one shown named so in any case, else the path
/// under the folder shown.
fn literal_typed_path(typed: &str, folder: &Path, names: &[String]) -> PathBuf {
    if typed.is_empty() {
        return PathBuf::from("/");
    }
    if typed.starts_with('/') || typed == "~" || typed.starts_with("~/") {
        return PathBuf::from(typed);
    }
    let lower = typed.to_lowercase();
    match names.iter().find(|name| name.to_lowercase() == lower) {
        Some(name) => folder.join(name),
        None => folder.join(typed),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::scratch;
    use super::*;

    /// A scratch folder for `test` holding `Papers/` (with a PDF) and a
    /// note.
    fn tree(test: &str) -> PathBuf {
        let dir = scratch(test);
        fs::create_dir(dir.join("Papers")).expect("folder");
        fs::write(dir.join("Papers").join("a.pdf"), b"%PDF").expect("pdf");
        fs::write(dir.join("notes.md"), b"x").expect("note");
        dir
    }

    fn words() -> BrowserWords {
        BrowserWords::new("Browse", "Open the folder", "Open").with_new_file("New file")
    }

    fn files(dir: &Path) -> FileBrowser<Chosen> {
        FileBrowser::files("test", words(), dir.to_path_buf(), Some)
    }

    fn labels<O>(browser: &FileBrowser<O>) -> Vec<&str> {
        browser
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect()
    }

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    #[test]
    fn test_tab_enters_a_folder_and_backspace_in_an_empty_query_leaves_it() {
        let dir = tree("tab");
        let mut browser = files(dir.as_path());
        assert_eq!(labels(&browser), [".", "Papers/", "notes.md"]);
        assert_eq!(
            browser.key(&key(KeyCode::Tab), Some(RowKey(1)), "pap"),
            Some(SourceKey::Reset)
        );
        assert_eq!(labels(&browser), [".", "a.pdf"]);
        assert_eq!(
            browser.key(&key(KeyCode::Backspace), None, "x"),
            None,
            "a query is edited"
        );
        assert_eq!(
            browser.key(&key(KeyCode::Backspace), None, ""),
            Some(SourceKey::Reset)
        );
        assert_eq!(browser.folder(), dir.as_path());
    }

    #[test]
    fn test_a_typed_path_ended_with_a_slash_goes_there_and_home_is_expanded() {
        let dir = tree("typed");
        let mut browser = files(dir.as_path()).with_home(dir.as_path().join("Papers"));
        let slash = key(KeyCode::Char('/'));
        assert_eq!(
            browser.key(&slash, None, "papers"),
            Some(SourceKey::Reset),
            "any case"
        );
        assert_eq!(browser.folder(), dir.as_path().join("Papers"));
        assert_eq!(browser.key(&slash, None, "nowhere"), None, "typed as text");
        browser.key(&key(KeyCode::Backspace), None, "");
        assert_eq!(browser.key(&slash, None, "~"), Some(SourceKey::Reset));
        assert_eq!(browser.folder(), dir.as_path().join("Papers"));
    }

    #[test]
    fn test_choosing_hands_back_a_file_the_folder_shown_or_a_file_to_make() {
        let dir = tree("choose");
        let mut browser = files(dir.as_path()).with_new_file();
        assert_eq!(
            browser.run(CHOOSE, RowKey(2), ""),
            Some(Chosen::File(dir.as_path().join("notes.md")))
        );
        assert_eq!(
            browser.run(CHOOSE, RowKey(0), ""),
            Some(Chosen::Folder(dir.as_path().to_path_buf()))
        );
        assert_eq!(
            browser.run(CHOOSE, RowKey(1), ""),
            None,
            "a folder is gone into, not chosen"
        );
        assert!(
            browser.query_row("notes.md").is_none(),
            "an entry's name is no new file"
        );
        let row = browser.query_row("draft.md").expect("a new file row");
        assert_eq!(row.label, "New file draft.md");
        assert_eq!(
            browser.run(CHOOSE, row.key, "draft.md"),
            Some(Chosen::NewFile(dir.as_path().join("draft.md")))
        );
    }

    #[test]
    fn test_a_folder_chooser_lists_no_files_and_its_use_key_chooses_the_folder_shown() {
        let dir = tree("chooser");
        let use_key = Key::new(KeyCode::Char('o'), Mods::CTRL);
        let mut chooser = FileBrowser::folders("test", words(), dir.as_path().to_path_buf(), Some)
            .with_keys(move |key| (*key == use_key).then_some(BrowserKey::UseFolder));
        assert_eq!(labels(&chooser), [".", "Papers/"]);
        assert_eq!(
            chooser.key(&use_key, None, ""),
            Some(SourceKey::Done(Chosen::Folder(dir.as_path().to_path_buf())))
        );
        assert!(
            chooser.query_row("draft").is_none(),
            "no new file in a chooser"
        );
    }

    #[test]
    fn test_a_ranker_ranks_and_lights_the_rows_its_way() {
        struct Prefix;
        impl Ranker for Prefix {
            fn rank(&self, labels: &[&str], query: &str) -> Vec<usize> {
                (0..labels.len())
                    .filter(|at| labels[*at].starts_with(query))
                    .collect()
            }
            fn lit(&self, _label: &str, query: &str) -> Vec<usize> {
                (0..query.chars().count()).collect()
            }
        }
        let dir = tree("ranker");
        let mut browser = files(dir.as_path()).with_ranker(Arc::new(Prefix));
        assert_eq!(browser.matching(), Matching::BySource(MatchOn::LABEL));
        let Feed::Fixed(rows) = browser.feed("no") else {
            panic!("a fixed feed");
        };
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, ["notes.md"]);
        assert_eq!(browser.highlights(&rows[0], "no").label, [0, 1]);
    }
}

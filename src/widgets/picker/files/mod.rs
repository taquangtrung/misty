//! Ready-made picker sources over the file system, behind the `files`
//! feature: a [`FileBrowser`] that walks the folders one at a time and
//! hands back the file or folder chosen, and a [`FileScan`] that lists
//! every file of some kinds under some folders, found on a thread.
//!
//! Both are plain [`Source`](super::Source)s built on the public picker
//! API, so an app shows them as any other picker and can wrap or replace
//! them. What they show in words comes from the app ([`BrowserWords`]),
//! as a source has no theme to read; how rows rank ([`Ranker`]), how a
//! typed path is read, which keys go up or use the folder shown, and where
//! a folder's entries come from ([`FolderReader`], a remote host's for
//! instance) are the app's to change.
//!
//! ```no_run
//! use std::path::PathBuf;
//!
//! use misty::picker::files::{BrowserWords, Chosen, FileBrowser};
//! use misty::picker::{Picker, PickerMemory};
//!
//! let words = BrowserWords::new("Open a file", "Open the folder shown", "Open");
//! let browser = FileBrowser::files("open-file", words, PathBuf::from("/home/me"), |chosen| {
//!     match chosen {
//!         Chosen::File(path) => Some(path),
//!         Chosen::Folder(_) | Chosen::NewFile(_) => None,
//!     }
//! });
//! let picker = Picker::synchronous(Box::new(browser), PickerMemory::default());
//! ```

mod browser;
mod scan;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use browser::FileBrowser;
pub use scan::FileScan;

// ========================================================================
// Data Structures
// ========================================================================

/// What a [`FileBrowser`] hands the app's `choose` to make its outcome of.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Chosen {
    /// A file chosen.
    File(PathBuf),
    /// The folder shown, chosen through its `.` row or a use key.
    Folder(PathBuf),
    /// A file to make, named as the query typed, in the folder shown.
    NewFile(PathBuf),
}

/// A key a [`FileBrowser`] gives a meaning, as the app's keymap says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserKey {
    /// Show the folder above the one shown.
    Up,
    /// Choose the folder shown.
    UseFolder,
}

/// The words a [`FileBrowser`] or a [`FileScan`] shows: the app's, in its
/// language.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserWords {
    /// What the `.` row says it does (`Open as folder tab`, `Use this
    /// folder`).
    pub current_folder: String,
    /// What the new file row says, before the name typed.
    pub new_file: String,
    /// What the action menu calls choosing a row (`Open`).
    pub open: String,
    /// What the picker is headed by, before the folder shown.
    pub title: String,
}

/// A folder's entries, as a [`FolderReader`] gives them: folders and files,
/// each sorted as the picker shows them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Listing {
    /// The files listed, by their paths.
    pub files: Vec<PathBuf>,
    /// The folders in it, by their paths.
    pub folders: Vec<PathBuf>,
}

/// The local file system, as a [`FolderReader`]: a folder's folders and
/// its files with one of the extensions asked for (any file without a
/// list), sorted by name in any case, hidden ones (named with a leading
/// dot) left out unless asked for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalFolders {
    /// The file extensions listed, lowercase without the dot; `None` lists
    /// every file.
    extensions: Option<Vec<String>>,
    hidden: bool,
}

// ========================================================================
// Traits
// ========================================================================

/// Where a [`FileBrowser`] reads a folder's entries: the local file system
/// ([`LocalFolders`]) unless the app gives another, such as a remote
/// host's.
pub trait FolderReader: Send {
    /// The entries of `folder`.
    fn read(&self, folder: &Path) -> io::Result<Listing>;
}

/// How a [`FileBrowser`]'s or a [`FileScan`]'s rows meet the query, for an
/// app with a matcher of its own; without one the picker matches by words.
pub trait Ranker: Send + Sync {
    /// The places of the `labels` matching `query`, best first; every place
    /// in order for an empty query.
    fn rank(&self, labels: &[&str], query: &str) -> Vec<usize>;

    /// The characters of `label` to light for `query`, by their index.
    fn lit(&self, label: &str, query: &str) -> Vec<usize>;
}

// ========================================================================
// BrowserWords Implementations
// ========================================================================

impl BrowserWords {
    /// Words heading the picker with `title`, the `.` row saying it does
    /// `current_folder`, and choosing a row called `open`; no new file row
    /// unless [`BrowserWords::with_new_file`] names it.
    pub fn new(
        title: impl Into<String>,
        current_folder: impl Into<String>,
        open: impl Into<String>,
    ) -> Self {
        Self {
            current_folder: current_folder.into(),
            new_file: String::new(),
            open: open.into(),
            title: title.into(),
        }
    }

    /// The same words, the new file row saying `new_file` before the name.
    pub fn with_new_file(self, new_file: impl Into<String>) -> Self {
        Self {
            new_file: new_file.into(),
            ..self
        }
    }
}

// ========================================================================
// LocalFolders Implementations
// ========================================================================

impl LocalFolders {
    /// Every folder and file, hidden ones left out.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only the files with one of `extensions` (any case, with or without
    /// the dot), and every folder.
    pub fn with_extensions(self, extensions: &[&str]) -> Self {
        let extensions = extensions
            .iter()
            .map(|extension| extension.trim_start_matches('.').to_lowercase())
            .collect();
        Self {
            extensions: Some(extensions),
            ..self
        }
    }

    /// Hidden folders and files listed too.
    pub fn with_hidden(self, hidden: bool) -> Self {
        Self { hidden, ..self }
    }

    /// Whether `path`, a file, is one listed.
    fn lists(&self, path: &Path) -> bool {
        let Some(extensions) = &self.extensions else {
            return true;
        };
        path.extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .is_some_and(|extension| extensions.contains(&extension))
    }

    /// Whether `path` is hidden and left out.
    fn hides(&self, path: &Path) -> bool {
        !self.hidden
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    }
}

impl FolderReader for LocalFolders {
    fn read(&self, folder: &Path) -> io::Result<Listing> {
        let mut listing = Listing::default();
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let path = entry.path();
            if self.hides(&path) {
                continue;
            }
            match entry.file_type()?.is_dir() {
                true => listing.folders.push(path),
                false if self.lists(&path) => listing.files.push(path),
                false => {}
            }
        }
        let by_name = |path: &PathBuf| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
        };
        listing.folders.sort_by_key(by_name);
        listing.files.sort_by_key(by_name);
        Ok(listing)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The name `path` is shown by: its last part, or the whole path when it
/// has none (`/`).
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// A folder under the system's temporary folder unique to `test`, made
/// empty, for a test's files.
#[cfg(test)]
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("misty-files-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch folder");
    dir
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_local_folder_lists_folders_and_the_files_asked_for_by_name() {
        let dir = scratch("listing");
        for folder in ["b", "A", ".git"] {
            fs::create_dir(dir.join(folder)).expect("folder");
        }
        for file in ["z.PDF", "a.pdf", "notes.txt", ".hidden.pdf"] {
            fs::write(dir.join(file), b"x").expect("file");
        }
        let names = |paths: &[PathBuf]| -> Vec<String> {
            paths.iter().map(|path| file_name(path)).collect()
        };
        let pdfs = LocalFolders::new()
            .with_extensions(&[".pdf"])
            .read(&dir)
            .expect("read");
        assert_eq!(names(&pdfs.folders), ["A", "b"]);
        assert_eq!(names(&pdfs.files), ["a.pdf", "z.PDF"]);
        let all = LocalFolders::new()
            .with_hidden(true)
            .read(&dir)
            .expect("read");
        assert_eq!(names(&all.folders), [".git", "A", "b"]);
        assert_eq!(all.files.len(), 4);
    }
}

//! [`FileDialog`]: the system's own dialog to open files, pick a folder, or
//! choose where to save, behind the `dialogs` feature. The dialog runs
//! beside the event loop, so the app keeps drawing, and its answer comes
//! back to [`Application::message`](crate::Application::message) as a
//! message the app makes from the paths chosen.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::thread;

use rfd::{AsyncFileDialog, FileHandle};

use super::super::platform::window_id::WindowId;
use super::application::AppContext;

// ========================================================================
// Constants
// ========================================================================

/// The name of the thread a dialog's answer is awaited on.
const DIALOG_THREAD: &str = "misty file dialog";

/// What a file pattern's extension may start with, left off by
/// [`FileDialog::with_filter`]: `*.png` and `.png` both mean `png`.
const EXTENSION_PREFIXES: [char; 2] = ['*', '.'];

// ========================================================================
// Data Structures
// ========================================================================

/// A system file dialog to show: start from [`FileDialog::open_file`],
/// [`FileDialog::open_files`], [`FileDialog::pick_folder`], or
/// [`FileDialog::save_file`], set what you need with the `with_` methods,
/// and [`FileDialog::show`] it. The paths chosen come back as a message;
/// none when the user cancels.
///
/// ```no_run
/// use std::path::PathBuf;
///
/// use misty::{AppContext, FileDialog, WindowId};
///
/// enum Message {
///     Opened(Vec<PathBuf>),
/// }
///
/// fn open(cx: &AppContext<'_, Message>, window: WindowId) {
///     FileDialog::open_file()
///         .with_title("Open a picture")
///         .with_filter("Pictures", &["png", "jpg"])
///         .show(cx, window, Message::Opened);
/// }
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileDialog {
    /// The folder the dialog starts in, or `None` for the system's choice.
    directory: Option<PathBuf>,
    /// The name a save dialog proposes.
    file_name: Option<String>,
    /// The kinds of file the user may choose among.
    filters: Vec<FileFilter>,
    /// What the dialog asks for.
    kind: DialogKind,
    /// The dialog's title, or `None` for the system's.
    title: Option<String>,
}

/// A kind of file a dialog offers: a name and the extensions it covers.
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFilter {
    /// The extensions, without a dot.
    extensions: Vec<String>,
    /// What the kind is called, such as `Pictures`.
    name: String,
}

/// What a dialog asks the user for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DialogKind {
    /// One existing file.
    #[default]
    OpenFile,
    /// One or more existing files.
    OpenFiles,
    /// One folder.
    PickFolder,
    /// Where to save a file.
    SaveFile,
}

/// The paths a dialog answers with, once the user is done.
type Answer = Pin<Box<dyn Future<Output = Vec<PathBuf>> + Send>>;

// ========================================================================
// FileDialog Implementations
// ========================================================================

impl FileDialog {
    /// A dialog to open one existing file.
    pub fn open_file() -> Self {
        Self::of(DialogKind::OpenFile)
    }

    /// A dialog to open one or more existing files.
    pub fn open_files() -> Self {
        Self::of(DialogKind::OpenFiles)
    }

    /// A dialog to pick one folder.
    pub fn pick_folder() -> Self {
        Self::of(DialogKind::PickFolder)
    }

    /// A dialog to choose where to save a file, warning before replacing
    /// one that exists.
    pub fn save_file() -> Self {
        Self::of(DialogKind::SaveFile)
    }

    /// This dialog titled `title`.
    pub fn with_title(self, title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..self
        }
    }

    /// This dialog starting in the folder `directory`.
    pub fn with_directory(self, directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: Some(directory.into()),
            ..self
        }
    }

    /// This save dialog proposing the name `file_name`.
    pub fn with_file_name(self, file_name: impl Into<String>) -> Self {
        Self {
            file_name: Some(file_name.into()),
            ..self
        }
    }

    /// This dialog offering the kind of file `name` with `extensions`,
    /// written `png`, `.png`, or `*.png` alike. The first kind added is
    /// offered first.
    pub fn with_filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        let extensions = extensions
            .iter()
            .map(|extension| extension.trim_start_matches(EXTENSION_PREFIXES).to_owned())
            .collect();
        self.filters.push(FileFilter {
            extensions,
            name: name.into(),
        });
        self
    }

    /// Shows the dialog over `window` without blocking the event loop.
    /// When the user is done, `message` makes a message of the paths chosen
    /// (empty when they cancelled; one for every kind of dialog but
    /// [`FileDialog::open_files`]), and it reaches
    /// [`Application::message`](crate::Application::message).
    pub fn show<M: Send + 'static>(
        self,
        cx: &AppContext<'_, M>,
        window: WindowId,
        message: impl FnOnce(Vec<PathBuf>) -> M + Send + 'static,
    ) {
        let mut dialog = AsyncFileDialog::new();
        if let Some(title) = self.title {
            dialog = dialog.set_title(title);
        }
        if let Some(directory) = self.directory {
            dialog = dialog.set_directory(directory);
        }
        if let Some(file_name) = self.file_name {
            dialog = dialog.set_file_name(file_name);
        }
        for filter in self.filters {
            dialog = dialog.add_filter(filter.name, &filter.extensions);
        }
        if let Some(parent) = cx.window_handle(window) {
            dialog = dialog.set_parent(parent.as_ref());
        }
        // Started here, on the main thread, as macOS requires; only the
        // wait for the answer moves to a thread.
        let answer: Answer = match self.kind {
            DialogKind::OpenFile => {
                let pending = dialog.pick_file();
                Box::pin(async move { paths(pending.await.into_iter()) })
            }
            DialogKind::OpenFiles => {
                let pending = dialog.pick_files();
                Box::pin(async move { paths(pending.await.into_iter().flatten()) })
            }
            DialogKind::PickFolder => {
                let pending = dialog.pick_folder();
                Box::pin(async move { paths(pending.await.into_iter()) })
            }
            DialogKind::SaveFile => {
                let pending = dialog.save_file();
                Box::pin(async move { paths(pending.await.into_iter()) })
            }
        };
        let waker = cx.waker();
        // A system that cannot start a thread drops the dialog's answer.
        let _ = thread::Builder::new()
            .name(DIALOG_THREAD.to_owned())
            .spawn(move || {
                let chosen = pollster::block_on(answer);
                // An app that quit meanwhile has no use for the answer.
                let _ = waker.send(message(chosen));
            });
    }

    /// A dialog asking for `kind`, with nothing else set.
    fn of(kind: DialogKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The paths of the files `handles` name.
fn paths(handles: impl Iterator<Item = FileHandle>) -> Vec<PathBuf> {
    handles.map(|handle| handle.path().to_path_buf()).collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_filter_takes_extensions_with_or_without_a_dot_or_a_star() {
        let dialog = FileDialog::open_file()
            .with_filter("Pictures", &["png", ".jpg", "*.webp"])
            .with_filter("Text", &["txt"]);
        assert_eq!(dialog.filters[0].extensions, ["png", "jpg", "webp"]);
        assert_eq!(dialog.filters[1].name, "Text", "kept in the order added");
    }
}

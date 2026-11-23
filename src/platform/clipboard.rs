//! [`SystemClipboard`]: the operating system's clipboard as text or as a
//! list of files, and on X11 and Wayland the primary selection (selected
//! text, pasted with the middle button) beside it.
//!
//! Files go as the platform's file list (`text/uri-list` on X11 and
//! Wayland, `CF_HDROP` on Windows, file URLs on macOS), which file
//! managers paste. Whether they were cut or copied travels only inside the
//! app: a file manager's own cut arrives as a copy, and a cut here arrives
//! in other apps as a copy, since the formats that carry it
//! (`x-special/gnome-copied-files`, KDE's `application/x-kde-cutselection`,
//! Windows' `Preferred DropEffect`) are out of reach of the clipboard
//! library. Text holding `file://` URIs, one a line, also reads as files.
//!
//! Every text written is also kept in the process, so copy and paste
//! inside the app keep working when the system cannot be reached: no
//! clipboard at all (a headless session), a selection the platform lacks
//! (the primary selection on Windows and macOS), or a transfer that failed.
//! Reading prefers the system, so text copied in other apps arrives.
//!
//! On X11 and Wayland the app itself serves what it copied until another
//! app takes the clipboard, so keep one `SystemClipboard` for the app's
//! whole life; what was copied is lost when it is dropped unless a
//! clipboard manager took a copy.
//!
//! The window runtime keeps that one clipboard and lends it to every
//! callback, through [`AppContext::clipboard`](crate::AppContext::clipboard)
//! and [`EventCx::clipboard`](crate::EventCx::clipboard).

use std::fmt;
use std::path::{Path, PathBuf};

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
))]
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};
use thiserror::Error;

// ========================================================================
// Constants
// ========================================================================

/// Whether the platform has a primary selection (X11 and Wayland), which
/// text fields write a selection to and paste on a middle click.
pub(crate) const HAS_PRIMARY_SELECTION: bool = cfg!(all(
    unix,
    not(any(
        target_os = "macos",
        target_os = "android",
        target_os = "emscripten"
    ))
));

// ========================================================================
// Data Structures
// ========================================================================

/// The system clipboard and primary selection, with an in-process copy of
/// what the app wrote to each.
pub struct SystemClipboard {
    /// The system's clipboard, or `None` when it could not be opened.
    backend: Option<Box<dyn ClipboardBackend>>,
    /// The text last written to the clipboard, kept in the process.
    clipboard: Option<String>,
    /// The files last written to the clipboard and how, kept in the
    /// process.
    files: Option<(Vec<PathBuf>, FileOperation)>,
    /// The text last written to the primary selection, kept in the
    /// process.
    primary: Option<String>,
}

/// Which of the system's clipboards.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClipboardKind {
    /// The clipboard of explicit copy and paste.
    Clipboard,
    /// The primary selection of X11 and Wayland: the text last selected,
    /// pasted with the middle button. Kept in the process only where the
    /// platform has none.
    Primary,
}

/// Whether files on the clipboard were copied or cut, so a paste copies
/// or moves them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileOperation {
    /// Copied: a paste copies them.
    Copy,
    /// Cut: a paste moves them.
    Cut,
}

/// Why text written to a [`SystemClipboard`] stayed in the process.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ClipboardError {
    /// The system refused the text; the reason is for people to read.
    #[error("the system clipboard refused the text: {0}")]
    Refused(String),
    /// The platform has no such clipboard, such as the primary selection
    /// outside X11 and Wayland.
    #[error("the system has no such clipboard")]
    Unsupported,
    /// No system clipboard could be opened when the app started.
    #[error("no system clipboard is available")]
    Unavailable,
}

/// The arboard clipboard, the one real [`ClipboardBackend`].
struct ArboardBackend(arboard::Clipboard);

// ========================================================================
// Traits
// ========================================================================

/// Where text goes to reach other apps: the system, or a fake in tests.
trait ClipboardBackend: Send {
    /// The text on `kind`.
    fn get(&mut self, kind: ClipboardKind) -> Result<String, arboard::Error>;

    /// Puts `text` on `kind`.
    fn set(&mut self, kind: ClipboardKind, text: &str) -> Result<(), arboard::Error>;

    /// The files on the clipboard.
    fn get_files(&mut self) -> Result<Vec<PathBuf>, arboard::Error>;

    /// Puts `files` on the clipboard.
    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), arboard::Error>;
}

// ========================================================================
// SystemClipboard Implementations
// ========================================================================

impl SystemClipboard {
    /// The system clipboard, or the in-process copy alone when the system
    /// has none to open (no display, no clipboard service).
    pub fn new() -> Self {
        let backend = arboard::Clipboard::new()
            .ok()
            .map(|clipboard| Box::new(ArboardBackend(clipboard)) as Box<dyn ClipboardBackend>);
        Self::with_backend(backend)
    }

    /// Whether the system clipboard was opened, rather than only the
    /// in-process copy.
    pub fn is_system(&self) -> bool {
        self.backend.is_some()
    }

    /// The text on `kind`: what the system holds, `None` when it holds no
    /// text, or what the app last wrote when the system cannot be read.
    pub fn read_text(&mut self, kind: ClipboardKind) -> Option<String> {
        let read = self.backend.as_mut().map(|backend| backend.get(kind));
        match read {
            Some(Ok(text)) => Some(text),
            Some(Err(arboard::Error::ContentNotAvailable)) => None,
            Some(Err(_)) | None => self.slot(kind).clone(),
        }
    }

    /// Puts `text` on `kind`, keeping it in the process too, so the app
    /// reads it back even when the system refused it, as the error says.
    pub fn write_text(&mut self, kind: ClipboardKind, text: &str) -> Result<(), ClipboardError> {
        *self.slot(kind) = Some(text.to_owned());
        let Some(backend) = self.backend.as_mut() else {
            return Err(ClipboardError::Unavailable);
        };
        backend.set(kind, text).map_err(write_error)
    }

    /// Puts `files` on the clipboard, copied or cut as `operation` says,
    /// keeping them in the process too, so the app reads them back (cut or
    /// copied) even when the system refused them, as the error says.
    pub fn set_files(
        &mut self,
        files: &[PathBuf],
        operation: FileOperation,
    ) -> Result<(), ClipboardError> {
        self.files = Some((files.to_vec(), operation));
        let Some(backend) = self.backend.as_mut() else {
            return Err(ClipboardError::Unavailable);
        };
        backend.set_files(files).map_err(write_error)
    }

    /// The files on the clipboard and whether they were cut: the system's
    /// file list (cut when it is the list this app cut last), else files
    /// named by `file://` URIs in its text; what the app last wrote when
    /// the system cannot be read; `None` when it holds no files.
    pub fn files(&mut self) -> Option<(Vec<PathBuf>, FileOperation)> {
        let read = self.backend.as_mut().map(|backend| backend.get_files());
        let files = match read {
            Some(Ok(files)) if !files.is_empty() => files,
            Some(Ok(_) | Err(arboard::Error::ContentNotAvailable)) => {
                let text = self.read_text(ClipboardKind::Clipboard)?;
                let files = parse_uri_list(&text);
                if files.is_empty() {
                    return None;
                }
                files
            }
            Some(Err(_)) | None => return self.files.clone(),
        };
        let operation = match &self.files {
            Some((own, operation)) if *own == files => *operation,
            Some(_) | None => FileOperation::Copy,
        };
        Some((files, operation))
    }

    /// A clipboard that never reaches the system, for tests.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn in_process() -> Self {
        Self::with_backend(None)
    }

    /// A clipboard over `backend`, or in-process only when `None`.
    fn with_backend(backend: Option<Box<dyn ClipboardBackend>>) -> Self {
        Self {
            backend,
            clipboard: None,
            files: None,
            primary: None,
        }
    }

    /// The in-process copy of `kind`.
    fn slot(&mut self, kind: ClipboardKind) -> &mut Option<String> {
        match kind {
            ClipboardKind::Clipboard => &mut self.clipboard,
            ClipboardKind::Primary => &mut self.primary,
        }
    }
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SystemClipboard {
    /// Says whether the system's clipboard is open, never what it holds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemClipboard")
            .field("is_system", &self.is_system())
            .finish_non_exhaustive()
    }
}

// ========================================================================
// ArboardBackend Implementations
// ========================================================================

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
))]
impl ClipboardBackend for ArboardBackend {
    fn get(&mut self, kind: ClipboardKind) -> Result<String, arboard::Error> {
        self.0.get().clipboard(linux_kind(kind)).text()
    }

    fn set(&mut self, kind: ClipboardKind, text: &str) -> Result<(), arboard::Error> {
        self.0.set().clipboard(linux_kind(kind)).text(text)
    }

    fn get_files(&mut self) -> Result<Vec<PathBuf>, arboard::Error> {
        self.0.get().file_list()
    }

    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), arboard::Error> {
        self.0.set().file_list(files)
    }
}

#[cfg(not(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
)))]
impl ClipboardBackend for ArboardBackend {
    fn get(&mut self, kind: ClipboardKind) -> Result<String, arboard::Error> {
        match kind {
            ClipboardKind::Clipboard => self.0.get_text(),
            ClipboardKind::Primary => Err(arboard::Error::ClipboardNotSupported),
        }
    }

    fn set(&mut self, kind: ClipboardKind, text: &str) -> Result<(), arboard::Error> {
        match kind {
            ClipboardKind::Clipboard => self.0.set_text(text),
            ClipboardKind::Primary => Err(arboard::Error::ClipboardNotSupported),
        }
    }

    fn get_files(&mut self) -> Result<Vec<PathBuf>, arboard::Error> {
        self.0.get().file_list()
    }

    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), arboard::Error> {
        self.0.set().file_list(files)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The [`ClipboardError`] for arboard's refusal to write: unsupported
/// when the clipboard kind does not exist here, else refused, saying why.
fn write_error(error: arboard::Error) -> ClipboardError {
    match error {
        arboard::Error::ClipboardNotSupported => ClipboardError::Unsupported,
        error => ClipboardError::Refused(error.to_string()),
    }
}

/// The files `text` names as a `text/uri-list`: one `file://` URI a line,
/// percent-escaped, lines starting with `#` and other schemes left out.
pub(crate) fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let rest = line.strip_prefix("file://")?;
            // An empty host or `localhost` is this machine; others are not
            // files here.
            let path = match rest.strip_prefix("localhost") {
                Some(path) => path,
                None if rest.starts_with('/') => rest,
                None => return None,
            };
            path_of(&percent_decode(path))
        })
        .collect()
}

/// `text` with each `%XX` escape turned back into its byte.
fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    decoded
}

/// The path of a URI's decoded `bytes`: as they are on Unix, which allows
/// any bytes; as UTF-8 elsewhere, `/C:/x` read as `C:/x`.
fn path_of(bytes: &[u8]) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(Path::new(std::ffi::OsStr::from_bytes(bytes)).to_path_buf())
    }
    #[cfg(not(unix))]
    {
        let text = std::str::from_utf8(bytes).ok()?;
        let text = match text.as_bytes().get(2) {
            Some(b':') => &text[1..],
            Some(_) | None => text,
        };
        Some(Path::new(text).to_path_buf())
    }
}

/// The X11 and Wayland selection `kind` names.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
))]
fn linux_kind(kind: ClipboardKind) -> LinuxClipboardKind {
    match kind {
        ClipboardKind::Clipboard => LinuxClipboardKind::Clipboard,
        ClipboardKind::Primary => LinuxClipboardKind::Primary,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A system clipboard holding text per kind, refusing kinds listed in
    /// `unsupported`.
    #[derive(Default)]
    struct FakeBackend {
        files: Option<Vec<PathBuf>>,
        held: HashMap<ClipboardKind, String>,
        unsupported: Vec<ClipboardKind>,
    }

    impl ClipboardBackend for FakeBackend {
        fn get(&mut self, kind: ClipboardKind) -> Result<String, arboard::Error> {
            if self.unsupported.contains(&kind) {
                return Err(arboard::Error::ClipboardNotSupported);
            }
            self.held
                .get(&kind)
                .cloned()
                .ok_or(arboard::Error::ContentNotAvailable)
        }

        fn set(&mut self, kind: ClipboardKind, text: &str) -> Result<(), arboard::Error> {
            if self.unsupported.contains(&kind) {
                return Err(arboard::Error::ClipboardNotSupported);
            }
            self.held.insert(kind, text.to_owned());
            Ok(())
        }

        fn get_files(&mut self) -> Result<Vec<PathBuf>, arboard::Error> {
            match &self.files {
                Some(files) => Ok(files.clone()),
                None => Err(arboard::Error::ContentNotAvailable),
            }
        }

        fn set_files(&mut self, files: &[PathBuf]) -> Result<(), arboard::Error> {
            self.files = Some(files.to_vec());
            Ok(())
        }
    }

    fn with_fake(fake: FakeBackend) -> SystemClipboard {
        SystemClipboard::with_backend(Some(Box::new(fake)))
    }

    #[test]
    fn test_reads_prefer_the_system_so_text_from_other_apps_arrives() {
        let mut held = HashMap::new();
        held.insert(ClipboardKind::Clipboard, "from another app".to_owned());
        let mut clipboard = with_fake(FakeBackend {
            held,
            ..FakeBackend::default()
        });
        assert_eq!(
            clipboard.read_text(ClipboardKind::Clipboard).as_deref(),
            Some("from another app")
        );
        assert_eq!(clipboard.read_text(ClipboardKind::Primary), None);
    }

    #[test]
    fn test_an_unsupported_selection_keeps_its_text_in_the_process() {
        let mut clipboard = with_fake(FakeBackend {
            unsupported: vec![ClipboardKind::Primary],
            ..FakeBackend::default()
        });
        let written = clipboard.write_text(ClipboardKind::Primary, "selected");
        assert!(matches!(written, Err(ClipboardError::Unsupported)));
        assert_eq!(
            clipboard.read_text(ClipboardKind::Primary).as_deref(),
            Some("selected")
        );
        assert!(
            clipboard
                .write_text(ClipboardKind::Clipboard, "copied")
                .is_ok()
        );
        assert_eq!(
            clipboard.read_text(ClipboardKind::Clipboard).as_deref(),
            Some("copied")
        );
    }

    #[test]
    fn test_without_a_system_clipboard_copy_and_paste_stay_in_the_process() {
        let mut clipboard = SystemClipboard::with_backend(None);
        assert!(!clipboard.is_system());
        assert_eq!(clipboard.read_text(ClipboardKind::Clipboard), None);
        let written = clipboard.write_text(ClipboardKind::Clipboard, "kept");
        assert!(matches!(written, Err(ClipboardError::Unavailable)));
        assert_eq!(
            clipboard.read_text(ClipboardKind::Clipboard).as_deref(),
            Some("kept")
        );
        assert_eq!(clipboard.read_text(ClipboardKind::Primary), None);
    }

    #[test]
    fn test_a_uri_list_names_local_files_with_escapes_and_skips_the_rest() {
        let list = "# copied by a file manager\r\nfile:///home/me/My%20Notes.txt\r\n\
                    file://localhost/tmp/caf%C3%A9.md\r\nhttps://example.org/x\r\n\
                    file://other-host/x\r\nfile:///tmp/100%25\r\n";
        assert_eq!(
            parse_uri_list(list),
            [
                PathBuf::from("/home/me/My Notes.txt"),
                PathBuf::from("/tmp/café.md"),
                PathBuf::from("/tmp/100%"),
            ]
        );
    }

    #[test]
    fn test_files_round_trip_in_process_and_a_cut_stays_a_cut_only_for_its_own_list() {
        let files = vec![PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/ü")];
        let mut alone = SystemClipboard::in_process();
        assert!(
            alone.set_files(&files, FileOperation::Cut).is_err(),
            "no system"
        );
        assert_eq!(alone.files(), Some((files.clone(), FileOperation::Cut)));
        let mut clipboard = with_fake(FakeBackend::default());
        clipboard
            .set_files(&files, FileOperation::Cut)
            .expect("the fake takes files");
        assert_eq!(clipboard.files(), Some((files.clone(), FileOperation::Cut)));
        let mut other = with_fake(FakeBackend {
            files: Some(vec![PathBuf::from("/elsewhere")]),
            ..FakeBackend::default()
        });
        other.files = Some((files, FileOperation::Cut));
        assert_eq!(
            other.files(),
            Some((vec![PathBuf::from("/elsewhere")], FileOperation::Copy)),
            "another app's files arrive copied"
        );
    }

    #[test]
    fn test_text_of_file_uris_reads_as_files_and_other_text_as_none() {
        let mut held = HashMap::new();
        held.insert(ClipboardKind::Clipboard, "file:///tmp/x.txt\n".to_owned());
        let mut clipboard = with_fake(FakeBackend {
            held,
            ..FakeBackend::default()
        });
        assert_eq!(
            clipboard.files(),
            Some((vec![PathBuf::from("/tmp/x.txt")], FileOperation::Copy))
        );
        clipboard
            .write_text(ClipboardKind::Clipboard, "just words")
            .expect("the fake takes text");
        assert_eq!(clipboard.files(), None);
    }
}

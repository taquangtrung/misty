//! [`FileScan`]: every file of some kinds under some folders, found on a
//! thread, for opening one.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use super::super::row::{Highlights, MatchOn, Row, RowKey};
use super::super::source::{Action, ActionId, Batch, Feed, Matching, Resume, Source};
use super::{BrowserWords, Ranker, file_name};

// ========================================================================
// Constants
// ========================================================================

/// The one action of every row, run by `Enter`.
const CHOOSE: ActionId = ActionId("choose");

/// The name of the thread a scan runs on.
const SCAN_THREAD: &str = "misty file scan";

// ========================================================================
// Data Structures
// ========================================================================

/// A [`Source`] of every file with one of some extensions under some
/// folders (hidden folders and files left out), found on a thread the
/// first time it is fed and listed by name with its path as a hint;
/// choosing one does what the app's `choose` makes of its path.
pub struct FileScan<O> {
    choose: Box<dyn Fn(PathBuf) -> Option<O> + Send>,
    /// The extensions found, lowercase without the dot.
    extensions: Vec<String>,
    /// What the scan found, once done.
    found: Arc<Mutex<Option<Vec<PathBuf>>>>,
    id: &'static str,
    ranker: Option<Arc<dyn Ranker>>,
    roots: Vec<PathBuf>,
    scanning: bool,
    words: BrowserWords,
}

// ========================================================================
// FileScan Implementations
// ========================================================================

impl<O> FileScan<O> {
    /// A scan named `id` of the files with one of `extensions` (any case,
    /// with or without the dot) under `roots`, headed by the words' title.
    pub fn new(
        id: &'static str,
        words: BrowserWords,
        roots: Vec<PathBuf>,
        extensions: &[&str],
        choose: impl Fn(PathBuf) -> Option<O> + Send + 'static,
    ) -> Self {
        Self {
            choose: Box::new(choose),
            extensions: extensions
                .iter()
                .map(|extension| extension.trim_start_matches('.').to_lowercase())
                .collect(),
            found: Arc::default(),
            id,
            ranker: None,
            roots,
            scanning: false,
            words,
        }
    }

    /// The same scan, its rows ranked and lit by `ranker` rather than
    /// matched by words.
    pub fn with_ranker(self, ranker: Arc<dyn Ranker>) -> Self {
        Self {
            ranker: Some(ranker),
            ..self
        }
    }

    /// The files found matching `query` as rows keyed by their places.
    fn rows(found: &[PathBuf], ranker: Option<&dyn Ranker>, query: &str) -> Vec<Row> {
        let names: Vec<String> = found.iter().map(|path| file_name(path)).collect();
        let places = match ranker {
            Some(ranker) => {
                let labels: Vec<&str> = names.iter().map(String::as_str).collect();
                ranker.rank(&labels, query)
            }
            None => (0..found.len()).collect(),
        };
        places
            .into_iter()
            .filter_map(|at| {
                let path = found.get(at)?;
                Some(
                    Row::new(RowKey(at as u64), names[at].as_str())
                        .with_hint(path.display().to_string()),
                )
            })
            .collect()
    }
}

impl<O> Source<O> for FileScan<O> {
    fn id(&self) -> &'static str {
        self.id
    }

    fn title(&self) -> String {
        let found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        match found.as_ref() {
            Some(found) => format!("{} ({})", self.words.title, found.len()),
            None => format!("{}…", self.words.title),
        }
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

    /// The files matching `query`; the first time, the scan starts on a
    /// thread and its rows come as one batch when it is done.
    fn feed(&mut self, query: &str) -> Feed {
        let ranker = self.ranker.clone();
        if let Some(found) = self
            .found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            return Feed::Fixed(Self::rows(found, ranker.as_deref(), query));
        }
        if self.scanning {
            return Feed::Fixed(Vec::new());
        }
        self.scanning = true;
        let (sender, receiver) = mpsc::channel();
        let (roots, extensions) = (self.roots.clone(), self.extensions.clone());
        let (store, query) = (Arc::clone(&self.found), query.to_owned());
        let spawned = thread::Builder::new()
            .name(SCAN_THREAD.into())
            .spawn(move || {
                let mut found = Vec::new();
                for root in &roots {
                    scan(root, &extensions, &mut found);
                }
                found.sort();
                found.dedup();
                let rows = Self::rows(&found, ranker.as_deref(), &query);
                *store.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
                let _ = sender.send(Batch {
                    done: true,
                    rows,
                    ..Batch::default()
                });
            });
        match spawned {
            Ok(_) => Feed::Stream(receiver),
            Err(error) => Feed::Failed(error.to_string()),
        }
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

    fn run(&mut self, _action: ActionId, row: RowKey, _query: &str) -> Option<O> {
        let found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        let path = found.as_ref()?.get(usize::try_from(row.0).ok()?)?.clone();
        drop(found);
        (self.choose)(path)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Adds the files under `folder` with one of `extensions` to `found`,
/// hidden folders and files left out, links into folders not followed.
fn scan(folder: &Path, extensions: &[String], found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            scan(&path, extensions, found);
            continue;
        }
        let listed = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .is_some_and(|extension| extensions.contains(&extension));
        if listed {
            found.push(path);
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::scratch;
    use super::*;

    #[test]
    fn test_a_scan_finds_the_files_asked_for_under_its_folders_on_a_thread() {
        let dir = scratch("scan");
        let nested = dir.join("a").join("b");
        fs::create_dir_all(&nested).expect("folders");
        fs::create_dir(dir.as_path().join(".cache")).expect("hidden");
        fs::write(nested.join("deep.PDF"), b"x").expect("pdf");
        fs::write(dir.as_path().join("top.pdf"), b"x").expect("pdf");
        fs::write(dir.as_path().join("notes.md"), b"x").expect("note");
        fs::write(dir.as_path().join(".cache").join("hidden.pdf"), b"x").expect("hidden pdf");
        let words = BrowserWords::new("Papers", "", "Open");
        let mut scan = FileScan::new(
            "test",
            words,
            vec![dir.as_path().to_path_buf()],
            &["pdf"],
            Some,
        );
        let Feed::Stream(receiver) = scan.feed("") else {
            panic!("a stream the first time");
        };
        let batch = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the scan's batch");
        let names: Vec<&str> = batch.rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            names,
            ["deep.PDF", "top.pdf"],
            "sorted by path, hidden left out"
        );
        assert!(batch.done);
        assert_eq!(scan.title(), "Papers (2)");
        assert_eq!(
            scan.run(CHOOSE, RowKey(1), ""),
            Some(dir.as_path().join("top.pdf"))
        );
        assert!(matches!(scan.feed("top"), Feed::Fixed(_)), "found once");
    }
}

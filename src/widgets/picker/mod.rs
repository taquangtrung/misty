//! The picker framework: picker sessions over sources of rows, matched on a
//! thread of their own, in the manner of Emacs' Helm. Every type-to-filter
//! list an app has (a command palette, quick open, a project search, a
//! question with a few answers) is a [`Picker`] over a [`Source`].
//!
//! - A [`Source`] says where its rows come from (all at once, or streamed
//!   from work of its own), how they meet the query ([`Matching`]), what
//!   each row can do ([`Action`]), and what it previews. [`FixedSource`] is
//!   a fixed list with one thing to do on a row, built in one call.
//! - A [`Row`] is a label, and may have a description after it, a detail
//!   on a second line, a hint at its right, and an icon; [`MatchOn`] names
//!   the fields the query meets and lights.
//! - The query is words in any order, each found whole in any case: `pick
//!   rs` finds `src/picker/mod.rs`. A word written `/like this/` is a
//!   regular expression, one starting with `!` leaves out the rows it
//!   matches, and text in double quotes is matched as written ([`Query`]).
//! - A [`Picker`] never waits on its source, and the app never waits on
//!   the picker: rows stream in, matches come back from the matcher's
//!   worker, and [`Picker::tick`] takes both in. The selection stays on
//!   its row as rows arrive, and on the best match until it is moved.
//! - The app tells the picker [`PickerAct`]s from its own keymap, and the
//!   keys it binds to none, which go to the query. Choosing a row hands
//!   back the source's outcome ([`Reply::Run`]) for the app to carry out.
//! - What a picker remembers ([`History`], [`Recency`], [`PickerMemory`])
//!   the app keeps with its own state; with misty's `serde` feature it
//!   serializes.
//! - With misty's `files` feature, [`files`] has ready-made sources over
//!   the file system: a folder browser and a scan of files by kind.

mod act;
#[cfg(feature = "files")]
pub mod files;
#[cfg(any(test, feature = "testing"))]
pub(crate) mod harness;
mod history;
mod list;
mod matcher;
mod query;
mod row;
mod session;
mod source;

pub use act::PickerAct;
pub use history::{History, Recency, Use};
pub use list::FixedSource;
pub use query::Query;
pub use row::{Field, Highlights, MatchOn, Row, RowKey};
pub use session::{
    MenuView, Parked, Picker, PickerMemory, PreviewUpdate, Reply, SavedLines, SessionView, Status,
    Tick, ViewRow,
};
pub use source::{Action, ActionId, Batch, Feed, Matching, PreviewKind, Resume, Source, SourceKey};

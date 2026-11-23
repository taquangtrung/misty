//! The cell model: a grid of [`Cell`]s, each a grapheme drawn in a [`CellStyle`],
//! read by a renderer through [`CellSource`]. A terminal emulator can store
//! these same types in its own screen; [`CellBuffer`] is a ready-made grid
//! for everything else.

mod buffer;
mod style;
mod value;

pub use buffer::{CellBuffer, CellSource};
pub use style::{Attributes, CellColor, CellStyle, UnderlineStyle};
pub use value::{Cell, CellHighlight, CellWidth, Cursor, CursorShape};

//! The text of a field, [`LineEdit`]: text, a caret, and a selection
//! between the caret and an anchor, changed by the edits every text field
//! makes, with undo and a kill buffer. One line for a
//! [`TextField`](super::TextField), or several, split by line feeds, for a
//! [`TextArea`](super::super::text_area::TextArea). No keys, clipboard, or
//! drawing: the widget around it brings those.

use std::mem;
use std::ops::Range;

use super::super::super::base::text;

// ========================================================================
// Constants
// ========================================================================

/// The most changes undo remembers; older ones are forgotten.
const MAX_UNDO: usize = 100;

/// What ends a line of a multiline text.
const LINE_FEED: &str = "\n";

// ========================================================================
// Data Structures
// ========================================================================

/// The text of a field, the caret in it, and the selected part.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LineEdit {
    /// Where the selection started, as a byte offset; `None` when nothing
    /// is selected. Never equal to the caret.
    anchor: Option<usize>,
    /// The caret, as a byte offset into `text` on a char boundary.
    caret: usize,
    /// The text the last kill took, for a yank.
    killed: String,
    /// What the last change was, so typed characters undo together.
    last: Option<ChangeKind>,
    /// Whether the text keeps line feeds; one line leaves them out.
    multiline: bool,
    /// Lines to go back to with redo, the newest last.
    redo: Vec<Snapshot>,
    /// The text.
    text: String,
    /// Lines to go back to with undo, the newest last.
    undo: Vec<Snapshot>,
}

/// What a change to a [`LineEdit`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Edit {
    /// The text changed.
    Changed,
    /// Only the caret or the selection moved, or the edit had nothing to
    /// do.
    Moved,
}

/// Which kind of change, for merging typed characters into one undo step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChangeKind {
    /// Anything else: a deletion, a paste, a kill.
    Other,
    /// Characters typed one at a time.
    Typing,
}

/// The text and caret undo goes back to.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    /// The caret.
    caret: usize,
    /// The text.
    text: String,
}

/// What kind of character, for words and double clicks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CharClass {
    /// A line feed, which a double click never selects across.
    Break,
    /// Punctuation and symbols.
    Other,
    /// Spaces.
    Space,
    /// Letters, digits, and underscores.
    Word,
}

// ========================================================================
// LineEdit Implementations
// ========================================================================

impl LineEdit {
    // --------------------------------------------------------------------
    // Reading
    // --------------------------------------------------------------------

    /// A line holding `text` (line breaks and control characters left out)
    /// with the caret at its end.
    pub(crate) fn with_text(text: &str) -> Self {
        Self::from_text(text, false)
    }

    /// Lines holding `text` (control characters but line feeds left out)
    /// with the caret at its end.
    pub(crate) fn multiline(text: &str) -> Self {
        Self::from_text(text, true)
    }

    /// Text holding `text`, keeping line feeds when `multiline`, the caret
    /// at its end.
    fn from_text(text: &str, multiline: bool) -> Self {
        let text = clean(text, multiline);
        Self {
            caret: text.len(),
            multiline,
            text,
            ..Self::default()
        }
    }

    /// The text.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The caret, as a byte offset.
    pub(crate) fn caret(&self) -> usize {
        self.caret
    }

    /// The selected bytes, or `None` when nothing is selected.
    pub(crate) fn selection(&self) -> Option<Range<usize>> {
        self.anchor
            .map(|anchor| anchor.min(self.caret)..anchor.max(self.caret))
    }

    /// The selection's anchor, or `None` when nothing is selected.
    pub(crate) fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// The selected text, or `None` when nothing is selected.
    pub(crate) fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.text[range])
    }

    /// The cells the text before byte offset `offset` takes.
    pub(crate) fn cells_to(&self, offset: usize) -> usize {
        text::width(&self.text[..offset])
    }

    /// The char boundary nearest `cells` cells from the start: before a
    /// character when `cells` falls in its left half, after it otherwise.
    pub(crate) fn offset_at_cells(&self, cells: f32) -> usize {
        let mut start = 0.0;
        for (index, c) in self.text.char_indices() {
            let width = text::char_width(c) as f32;
            if cells < start + width / 2.0 {
                return index;
            }
            start += width;
        }
        self.text.len()
    }

    // --------------------------------------------------------------------
    // Moving
    // --------------------------------------------------------------------

    /// Moves the caret to byte offset `offset`, extending the selection
    /// from where the caret was when `extend`, else dropping it.
    pub(crate) fn move_to(&mut self, offset: usize, extend: bool) -> Edit {
        self.anchor = match extend {
            true => Some(self.anchor.unwrap_or(self.caret)),
            false => None,
        };
        self.caret = offset;
        if self.anchor == Some(offset) {
            self.anchor = None;
        }
        self.last = None;
        Edit::Moved
    }

    /// Moves one character left; without `extend`, a selection collapses
    /// to its start instead.
    pub(crate) fn move_left(&mut self, extend: bool) -> Edit {
        match (self.selection(), extend) {
            (Some(range), false) => self.move_to(range.start, false),
            _ => self.move_to(self.previous_boundary(self.caret), extend),
        }
    }

    /// Moves one character right; without `extend`, a selection collapses
    /// to its end instead.
    pub(crate) fn move_right(&mut self, extend: bool) -> Edit {
        match (self.selection(), extend) {
            (Some(range), false) => self.move_to(range.end, false),
            _ => self.move_to(self.next_boundary(self.caret), extend),
        }
    }

    /// Moves to the start of the word before the caret.
    pub(crate) fn move_word_left(&mut self, extend: bool) -> Edit {
        self.move_to(self.word_start_before(self.caret), extend)
    }

    /// Moves to the end of the word after the caret.
    pub(crate) fn move_word_right(&mut self, extend: bool) -> Edit {
        self.move_to(self.word_end_after(self.caret), extend)
    }

    /// Selects the whole text, the caret at its end.
    pub(crate) fn select_all(&mut self) -> Edit {
        self.move_to(0, false);
        self.move_to(self.text.len(), true)
    }

    /// Selects the run of characters of one kind around byte offset
    /// `offset` (a word, spaces, or punctuation), as a double click does.
    pub(crate) fn select_word_at(&mut self, offset: usize) -> Edit {
        let target = self.text[offset..]
            .chars()
            .next()
            .or_else(|| self.text[..offset].chars().next_back())
            .map(CharClass::of);
        let Some(target) = target else {
            return Edit::Moved;
        };
        let start = self.text[..offset]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| CharClass::of(c) == target)
            .last()
            .map_or(offset, |(index, _)| index);
        let end = self.text[offset..]
            .char_indices()
            .find(|&(_, c)| CharClass::of(c) != target)
            .map_or(self.text.len(), |(index, _)| offset + index);
        self.move_to(start, false);
        self.move_to(end, true)
    }

    // --------------------------------------------------------------------
    // Changing
    // --------------------------------------------------------------------

    /// Replaces the whole text with `text` (control characters left out,
    /// line feeds too on one line), the caret at its end; forgets undo.
    pub(crate) fn set_text(&mut self, text: &str) {
        let killed = mem::take(&mut self.killed);
        *self = Self {
            killed,
            ..Self::from_text(text, self.multiline)
        };
    }

    /// Types `c` at the caret, over the selection.
    pub(crate) fn type_char(&mut self, c: char) -> Edit {
        if c.is_control() {
            return Edit::Moved;
        }
        self.replace_selection(c.encode_utf8(&mut [0; 4]), ChangeKind::Typing)
    }

    /// Starts a new line at the caret, over the selection, as its own undo
    /// step; nothing on one line.
    pub(crate) fn insert_newline(&mut self) -> Edit {
        match self.multiline {
            true => self.replace_selection(LINE_FEED, ChangeKind::Other),
            false => Edit::Moved,
        }
    }

    /// Inserts `text` at the caret over the selection, leaving out control
    /// characters (and line feeds, which one line cannot hold), and moves
    /// the caret past it.
    pub(crate) fn insert(&mut self, text: &str) -> Edit {
        let typed = clean(text, self.multiline);
        if typed.is_empty() {
            return Edit::Moved;
        }
        self.replace_selection(&typed, ChangeKind::Other)
    }

    /// Deletes the selection, or the character before the caret.
    pub(crate) fn delete_backward(&mut self) -> Edit {
        let range = self
            .selection()
            .unwrap_or(self.previous_boundary(self.caret)..self.caret);
        self.delete(range)
    }

    /// Deletes the selection, or the character after the caret.
    pub(crate) fn delete_forward(&mut self) -> Edit {
        let range = self
            .selection()
            .unwrap_or(self.caret..self.next_boundary(self.caret));
        self.delete(range)
    }

    /// Kills (deletes, keeping for a yank) the bytes `range`; nothing is
    /// kept when it is empty.
    pub(crate) fn kill(&mut self, range: Range<usize>) -> Edit {
        if range.is_empty() {
            return Edit::Moved;
        }
        self.killed = self.text[range.clone()].to_owned();
        self.delete(range)
    }

    /// Kills from the caret to the end of its line, or the line feed
    /// ending it when the caret is there already.
    pub(crate) fn kill_to_end(&mut self) -> Edit {
        let end = match self.line_end(self.caret) {
            end if end == self.caret => self.next_boundary(end),
            end => end,
        };
        self.kill(self.caret..end)
    }

    /// Kills from the start of the caret's line to the caret.
    pub(crate) fn kill_to_start(&mut self) -> Edit {
        self.kill(self.line_start(self.caret)..self.caret)
    }

    /// Kills the selection, or the word before the caret.
    pub(crate) fn kill_word_backward(&mut self) -> Edit {
        let range = self
            .selection()
            .unwrap_or(self.word_start_before(self.caret)..self.caret);
        self.kill(range)
    }

    /// Kills the word after the caret.
    pub(crate) fn kill_word_forward(&mut self) -> Edit {
        self.kill(self.caret..self.word_end_after(self.caret))
    }

    /// Inserts the text the last kill took.
    pub(crate) fn yank(&mut self) -> Edit {
        let killed = self.killed.clone();
        self.insert(&killed)
    }

    /// Takes the text and caret back to before the last change.
    pub(crate) fn undo(&mut self) -> Edit {
        let Some(snapshot) = self.undo.pop() else {
            return Edit::Moved;
        };
        let current = self.snapshot();
        self.redo.push(current);
        self.restore(snapshot)
    }

    /// Makes again the last change undone.
    pub(crate) fn redo(&mut self) -> Edit {
        let Some(snapshot) = self.redo.pop() else {
            return Edit::Moved;
        };
        let current = self.snapshot();
        self.undo.push(current);
        self.restore(snapshot)
    }

    /// Puts `text` in place of the selection, or at the caret, as a change
    /// of `kind`.
    fn replace_selection(&mut self, text: &str, kind: ChangeKind) -> Edit {
        let range = self.selection().unwrap_or(self.caret..self.caret);
        let merges = kind == ChangeKind::Typing && self.last == Some(kind) && range.is_empty();
        if !merges {
            self.remember();
        }
        self.text.replace_range(range.clone(), text);
        self.caret = range.start + text.len();
        self.anchor = None;
        self.last = Some(kind);
        Edit::Changed
    }

    /// Deletes the bytes `range`, leaving the caret at its start.
    fn delete(&mut self, range: Range<usize>) -> Edit {
        if range.is_empty() {
            return Edit::Moved;
        }
        self.remember();
        self.text.replace_range(range.clone(), "");
        self.caret = range.start;
        self.anchor = None;
        self.last = Some(ChangeKind::Other);
        Edit::Changed
    }

    /// Keeps the line as it is now for undo, forgetting what redo held.
    fn remember(&mut self) {
        let snapshot = self.snapshot();
        self.undo.push(snapshot);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The text and caret now.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            caret: self.caret,
            text: self.text.clone(),
        }
    }

    /// Goes back to `snapshot`, nothing selected.
    fn restore(&mut self, snapshot: Snapshot) -> Edit {
        self.text = snapshot.text;
        self.caret = snapshot.caret;
        self.anchor = None;
        self.last = None;
        Edit::Changed
    }

    // --------------------------------------------------------------------
    // Boundaries
    // --------------------------------------------------------------------

    /// Where the line holding byte offset `offset` starts.
    pub(crate) fn line_start(&self, offset: usize) -> usize {
        self.text[..offset]
            .rfind(LINE_FEED)
            .map_or(0, |index| index + LINE_FEED.len())
    }

    /// Where the line holding byte offset `offset` ends, before its line
    /// feed.
    pub(crate) fn line_end(&self, offset: usize) -> usize {
        self.text[offset..]
            .find(LINE_FEED)
            .map_or(self.text.len(), |index| offset + index)
    }

    /// The char boundary before `offset`, or the start.
    fn previous_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    /// The char boundary after `offset`, or the end.
    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map_or(offset, |c| offset + c.len_utf8())
    }

    /// Where the word before `offset` starts, past any other characters
    /// between them; the start when there is no word.
    fn word_start_before(&self, offset: usize) -> usize {
        let mut chars = self.text[..offset].char_indices().rev().peekable();
        while chars
            .next_if(|&(_, c)| !CharClass::of(c).is_word())
            .is_some()
        {}
        let mut start = 0;
        while let Some((index, _)) = chars.next_if(|&(_, c)| CharClass::of(c).is_word()) {
            start = index;
        }
        start
    }

    /// Where the word after `offset` ends, past any other characters
    /// between them; the end when there is no word.
    fn word_end_after(&self, offset: usize) -> usize {
        let mut chars = self.text[offset..].char_indices().peekable();
        while chars
            .next_if(|&(_, c)| !CharClass::of(c).is_word())
            .is_some()
        {}
        while chars
            .next_if(|&(_, c)| CharClass::of(c).is_word())
            .is_some()
        {}
        chars
            .peek()
            .map_or(self.text.len(), |&(index, _)| offset + index)
    }
}

// ========================================================================
// CharClass Implementations
// ========================================================================

impl CharClass {
    /// The class of `c`.
    fn of(c: char) -> Self {
        if c == '\n' {
            Self::Break
        } else if c.is_alphanumeric() || c == '_' {
            Self::Word
        } else if c.is_whitespace() {
            Self::Space
        } else {
            Self::Other
        }
    }

    /// Whether this is the class words are made of.
    fn is_word(self) -> bool {
        self == Self::Word
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `text` without control characters, keeping line feeds when
/// `multiline`.
fn clean(text: &str, multiline: bool) -> String {
    text.chars()
        .filter(|&c| !c.is_control() || (multiline && c == '\n'))
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typing_inserts_at_the_caret_across_multibyte_characters() {
        let mut line = LineEdit::default();
        line.type_char('a');
        line.type_char('b');
        line.move_left(false);
        line.type_char('é');
        line.type_char('漢');
        assert_eq!(line.text(), "aé漢b");
        assert_eq!(
            line.cells_to(line.caret()),
            4,
            "the wide char takes two cells"
        );
        line.delete_backward();
        line.delete_backward();
        assert_eq!(line.text(), "ab");
    }

    #[test]
    fn test_inserted_text_lands_at_the_caret_without_control_characters() {
        let mut line = LineEdit::with_text("ab");
        line.move_left(false);
        assert_eq!(line.insert("你\n好"), Edit::Changed);
        assert_eq!(line.text(), "a你好b");
        assert_eq!(line.caret(), "a你好".len());
        assert_eq!(line.insert("\r"), Edit::Moved);
    }

    #[test]
    fn test_shift_motions_select_and_plain_motions_collapse_to_an_end() {
        let mut line = LineEdit::with_text("hello world");
        line.move_word_left(true);
        assert_eq!(line.selected_text(), Some("world"));
        line.move_left(true);
        assert_eq!(line.selected_text(), Some(" world"));
        line.move_right(false);
        assert_eq!(
            (line.selection(), line.caret()),
            (None, 11),
            "collapsed to the end"
        );
        line.move_word_left(true);
        line.move_left(false);
        assert_eq!(line.caret(), 6, "collapsed to the start");
        line.move_right(true);
        line.move_left(true);
        assert_eq!(line.selection(), None, "back at the anchor selects nothing");
    }

    #[test]
    fn test_typing_and_deleting_replace_the_selection() {
        let mut line = LineEdit::with_text("hello world");
        line.move_word_left(true);
        line.type_char('W');
        assert_eq!(line.text(), "hello W");
        line.select_all();
        assert_eq!(line.delete_backward(), Edit::Changed);
        assert_eq!(line.text(), "");
    }

    #[test]
    fn test_words_skip_punctuation_and_spaces_between_them() {
        let mut line = LineEdit::with_text("foo.bar  baz");
        line.move_word_left(false);
        assert_eq!(line.caret(), 9);
        line.move_word_left(false);
        assert_eq!(line.caret(), 4);
        line.move_word_left(false);
        assert_eq!(line.caret(), 0);
        line.move_word_right(false);
        assert_eq!(line.caret(), 3);
        line.move_word_right(false);
        assert_eq!(line.caret(), 7);
    }

    #[test]
    fn test_a_double_click_selects_the_run_of_one_kind_under_it() {
        let mut line = LineEdit::with_text("let x_1 = 42;");
        line.select_word_at(5);
        assert_eq!(line.selected_text(), Some("x_1"));
        line.select_word_at(7);
        assert_eq!(line.selected_text(), Some(" "));
        line.select_word_at(13);
        assert_eq!(line.selected_text(), Some(";"), "at the end, the last char");
    }

    #[test]
    fn test_kills_keep_their_text_for_a_yank() {
        let mut line = LineEdit::with_text("new tab now");
        line.kill_word_backward();
        assert_eq!(line.text(), "new tab ");
        line.move_to(0, false);
        line.kill_word_forward();
        assert_eq!(line.text(), " tab ");
        line.move_to(line.text().len(), false);
        line.yank();
        assert_eq!(line.text(), " tab new");
        line.move_to(4, false);
        line.kill_to_start();
        line.kill_to_end();
        assert_eq!(line.text(), "");
        line.yank();
        assert_eq!(line.text(), " new", "the last kill");
    }

    #[test]
    fn test_typed_characters_undo_together_and_redo_comes_back() {
        let mut line = LineEdit::default();
        for c in "abc".chars() {
            line.type_char(c);
        }
        line.delete_backward();
        line.undo();
        assert_eq!(line.text(), "abc");
        line.undo();
        assert_eq!(line.text(), "", "the typed run is one step");
        assert_eq!(line.undo(), Edit::Moved);
        line.redo();
        assert_eq!((line.text(), line.caret()), ("abc", 3));
        line.type_char('d');
        assert_eq!(line.redo(), Edit::Moved, "a new change forgets redo");
    }

    #[test]
    fn test_a_cell_position_rounds_to_the_nearest_boundary() {
        let line = LineEdit::with_text("a漢b");
        assert_eq!(line.offset_at_cells(0.4), 0);
        assert_eq!(line.offset_at_cells(0.6), 1);
        assert_eq!(
            line.offset_at_cells(1.9),
            1,
            "the left half of the wide char"
        );
        assert_eq!(line.offset_at_cells(2.1), 4);
        assert_eq!(line.offset_at_cells(9.0), 5);
        assert_eq!(line.offset_at_cells(-3.0), 0);
    }

    #[test]
    fn test_edits_with_nothing_to_delete_change_nothing() {
        let mut line = LineEdit::default();
        assert_eq!(line.delete_backward(), Edit::Moved);
        assert_eq!(line.kill_to_end(), Edit::Moved);
        assert_eq!(line.type_char('\n'), Edit::Moved);
    }

    #[test]
    fn test_multiline_text_keeps_line_feeds_and_kills_by_line() {
        let mut lines = LineEdit::multiline("one\r\ntwo\tthree");
        assert_eq!(lines.text(), "one\ntwothree", "only line feeds stay");
        lines.move_to(5, false);
        lines.kill_to_end();
        assert_eq!(lines.text(), "one\nt", "to the line's end");
        lines.move_to(3, false);
        lines.kill_to_end();
        assert_eq!(lines.text(), "onet", "at the end, the line feed");
        lines.insert("\nx\n");
        lines.kill_to_start();
        assert_eq!(lines.text(), "one\nx\nt", "nothing before the caret");
        lines.move_to(5, false);
        lines.kill_to_start();
        assert_eq!(lines.text(), "one\n\nt");
    }

    #[test]
    fn test_a_new_line_is_its_own_undo_step_and_one_line_takes_none() {
        let mut lines = LineEdit::multiline("");
        lines.type_char('a');
        lines.insert_newline();
        lines.type_char('b');
        lines.undo();
        assert_eq!(lines.text(), "a\n");
        lines.undo();
        assert_eq!(lines.text(), "a");
        let mut line = LineEdit::with_text("a");
        assert_eq!(line.insert_newline(), Edit::Moved);
        line.set_text("x\ny");
        assert_eq!(line.text(), "xy");
    }

    #[test]
    fn test_a_double_click_on_a_line_end_never_selects_into_the_next_line() {
        let mut lines = LineEdit::multiline("ab  \n  cd");
        lines.select_word_at(3);
        assert_eq!(lines.selected_text(), Some("  "));
    }
}

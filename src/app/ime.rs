//! Input methods (IME): text composed in an input method (Pinyin, Kana,
//! Vietnamese Telex, dead-key accents) is shown inline at the focused text
//! widget's cursor while it is composed, underlined as not yet typed, and
//! typed into the widget once committed.
//!
//! Two parts share the work. [`ImeFocus`] is the window's side: the app
//! calls [`ImeFocus::follow`] with the cursor area of the focused text
//! widget, or `None` when no widget takes text, and the input method is
//! enabled or disabled to match and its candidate window kept beside the
//! cursor; only changes reach the window system. The widgets' side is the
//! [`Preedit`] each text widget keeps from the [`ImeInput`]s the app hands
//! its [`Widget::handle`](crate::Widget::handle) (`TextField`, an app's
//! own text widgets, and the quick pick and settings page that hold text
//! fields), drawn at its cursor and dropped on commit or when the input
//! method is disabled.

use super::super::base::geometry::Rect;
use super::super::base::text;
use super::super::input::event::{Composition, ImeInput};
use super::context::EventCx;

// ========================================================================
// Data Structures
// ========================================================================

/// Whether a window's input method is enabled and where its candidate
/// window is anchored, as last told to the window system.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImeFocus {
    /// Whether the input method is enabled.
    allowed: bool,
    /// The cursor area last reported, or `None` since it was enabled.
    area: Option<Rect>,
}

/// What [`ImeFocus`] must tell the window system to follow a new focus.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FocusChange {
    /// Enable (`true`) or disable (`false`) the input method, or leave it.
    allowed: Option<bool>,
    /// The new cursor area to report, or `None` to report nothing.
    area: Option<Rect>,
}

/// Text an input method is composing and has not yet committed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preedit {
    /// The caret, as a byte offset into `text`, or `None` when the input
    /// method hides it.
    caret: Option<usize>,
    /// The text.
    text: String,
}

/// What an [`ImeInput`] did to a widget's preedit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreeditChange {
    /// The input method committed this text, to be typed; the preedit is
    /// gone.
    Commit(String),
    /// The preedit changed or went away, to be redrawn.
    Changed,
    /// Nothing changed.
    Unchanged,
}

// ========================================================================
// ImeFocus Implementations
// ========================================================================

impl ImeFocus {
    /// A window whose input method is disabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the input method is enabled.
    pub fn is_allowed(&self) -> bool {
        self.allowed
    }

    /// Follows the focus of the window `cx` stands for: with `area`, the cursor of the
    /// focused text widget in physical pixels, the input method is enabled
    /// and its candidate window anchored there; with `None`, when no
    /// focused widget takes text (a Vim Normal mode, a list), it is
    /// disabled so keys reach the app as keys. Only changes are sent. Call
    /// it after the focus or the cursor may have moved, such as from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait),
    /// with the cursor area the widget had in its last frame.
    pub fn follow(&mut self, cx: &mut EventCx<'_>, area: Option<Rect>) {
        let change = self.change_to(area);
        if let Some(allowed) = change.allowed {
            cx.set_ime_allowed(allowed);
        }
        if let Some(area) = change.area {
            cx.set_ime_cursor_area(area);
        }
    }

    /// What to tell the window system to follow a focus with cursor
    /// `area`, recording it as told.
    fn change_to(&mut self, area: Option<Rect>) -> FocusChange {
        let allowed = area.is_some();
        let change = FocusChange {
            allowed: (allowed != self.allowed).then_some(allowed),
            area: area.filter(|_| area != self.area),
        };
        self.allowed = allowed;
        self.area = area;
        change
    }
}

// ========================================================================
// Preedit Implementations
// ========================================================================

impl Preedit {
    /// The preedit of an [`ImeInput::Preedit`] with `text` and the
    /// input method's `cursor` range, or `None` when `text` is empty (the
    /// preedit was cleared). A caret off a char boundary is hidden.
    pub(crate) fn new(text: &str, cursor: Option<(usize, usize)>) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let caret = cursor
            .map(|(start, _)| start)
            .filter(|&caret| text.is_char_boundary(caret));
        Some(Self {
            caret,
            text: text.to_owned(),
        })
    }

    /// The text being composed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The caret as a byte offset into [`Preedit::text`], or `None` when
    /// the input method hides it: where a widget laying the preedit out in
    /// the proportional font puts its caret.
    pub(crate) fn caret(&self) -> Option<usize> {
        self.caret
    }

    /// The cells from the preedit's start to its caret, or `None` when the
    /// input method hides the caret.
    pub fn caret_cells(&self) -> Option<usize> {
        self.caret.map(|caret| text::width(&self.text[..caret]))
    }

    /// Applies `ime` to a widget's `preedit`: a new preedit replaces it, a
    /// commit or a disabled input method drops it.
    pub fn apply(preedit: &mut Option<Self>, ime: &ImeInput) -> PreeditChange {
        let next = match ime {
            ImeInput::Commit(text) => {
                *preedit = None;
                return PreeditChange::Commit(text.clone());
            }
            ImeInput::Disabled => None,
            ImeInput::Enabled => return PreeditChange::Unchanged,
            ImeInput::Preedit(Composition { cursor, text }) => Self::new(text, *cursor),
        };
        match *preedit == next {
            true => PreeditChange::Unchanged,
            false => {
                *preedit = next;
                PreeditChange::Changed
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CURSOR: Rect = Rect::new(10.0, 20.0, 8.0, 16.0);

    #[test]
    fn test_focus_enables_once_reports_moves_and_disables_when_text_loses_focus() {
        let mut focus = ImeFocus::new();
        let first = focus.change_to(Some(CURSOR));
        assert_eq!((first.allowed, first.area), (Some(true), Some(CURSOR)));
        assert_eq!(focus.change_to(Some(CURSOR)), FocusChange::default());
        let moved = Rect::new(18.0, 20.0, 8.0, 16.0);
        let step = focus.change_to(Some(moved));
        assert_eq!((step.allowed, step.area), (None, Some(moved)));
        let off = focus.change_to(None);
        assert_eq!((off.allowed, off.area), (Some(false), None));
        assert!(!focus.is_allowed());
        // Enabled again at the same place, the area is reported again.
        assert_eq!(focus.change_to(Some(moved)).area, Some(moved));
    }

    #[test]
    fn test_a_preedit_is_replaced_dropped_on_commit_and_on_disable() {
        let mut preedit = None;
        let compose = ImeInput::Preedit(Composition {
            cursor: Some((2, 2)),
            text: "ni".into(),
        });
        assert_eq!(
            Preedit::apply(&mut preedit, &compose),
            PreeditChange::Changed
        );
        assert_eq!(
            Preedit::apply(&mut preedit, &compose),
            PreeditChange::Unchanged
        );
        assert_eq!(preedit.as_ref().map(Preedit::text), Some("ni"));
        let commit = ImeInput::Commit("你".into());
        assert_eq!(
            Preedit::apply(&mut preedit, &commit),
            PreeditChange::Commit("你".into())
        );
        assert_eq!(preedit, None);
        Preedit::apply(&mut preedit, &compose);
        assert_eq!(
            Preedit::apply(&mut preedit, &ImeInput::Disabled),
            PreeditChange::Changed
        );
        assert_eq!(preedit, None);
        let cleared = ImeInput::Preedit(Composition::default());
        assert_eq!(
            Preedit::apply(&mut preedit, &cleared),
            PreeditChange::Unchanged
        );
    }

    #[test]
    fn test_the_caret_counts_cells_and_hides_off_a_char_boundary() {
        let wide = Preedit::new("啊b", Some((3, 3))).expect("a preedit");
        assert_eq!(wide.caret_cells(), Some(2));
        let inside = Preedit::new("啊", Some((1, 1))).expect("a preedit");
        assert_eq!(inside.caret_cells(), None);
        assert_eq!(Preedit::new("a", None).and_then(|p| p.caret_cells()), None);
    }
}

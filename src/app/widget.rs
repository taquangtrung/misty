//! [`Widget`]: the one contract every misty widget keeps, so an app drives
//! a title bar, a list, and an editor the same way: hand it each
//! [`Input`] meant for it through [`Widget::handle`], which answers with a
//! response typed for that widget, and draw it into an area with
//! [`Widget::draw`], which returns what the drawing tells the app (the
//! area left for content, where a popup landed), typed the same way.
//!
//! What a widget needs besides the input reaches it through the
//! [`EventCx`]: the window's pointer and modifiers, the clipboard, and the
//! requests it makes (a redraw, a pointer shape). A widget that shows data
//! the app owns (a list's rows, an editor's text, the panes of a split) is
//! [`Lent`] that data for one call by its `lend` method, and the loan is
//! the widget: `list.lend(&rows).draw(frame, area, theme)`.
//!
//! The contract is per call and keeps no tree of widgets: the app still
//! owns its widgets and decides which input goes where, by hand or
//! through a [`FocusTree`](crate::FocusTree).
//!
//! Two more calls complete it, each with a default for the widgets it
//! does not concern:
//!
//! - **Focus.** A widget never asks a tree whether it has the keyboard;
//!   the app tells it, once per frame before drawing it, through
//!   [`Widget::set_focused`]. With a [`FocusTree`](crate::FocusTree) that
//!   is one call per widget: [`FocusTree::sync`](crate::FocusTree::sync)
//!   for a widget registered under one id, or
//!   [`FocusTree::sync_within`](crate::FocusTree::sync_within) for a
//!   container focused while anything inside it is; only a widget that
//!   answers to several ids of its own is told by hand, focused when any
//!   of them is.
//! - **Animation.** When the runtime calls
//!   [`Application::about_to_wait`](crate::Application::about_to_wait),
//!   the app forwards to [`Widget::about_to_wait`] on each widget that
//!   may be animating (a caret blinking, a scroll gliding, a spinner, a
//!   tooltip's delay) and returns the earliest instant they answer.
//!
//! Animations follow the user's reduced-motion setting on their own: a
//! widget reads [`EventCx::reduced_motion`] and
//! [`Frame::reduced_motion`] where it starts or steps one.

use std::time::Instant;

use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::context::EventCx;
use super::frame::Frame;

// ========================================================================
// Data Structures
// ========================================================================

/// A widget lent the data it shows, for one call: the widget `W` and the
/// loan `M`, a reference to the app's data (or a few, in a struct the
/// widget names). Made by the widget's `lend` method; it is itself a
/// [`Widget`].
#[derive(Debug)]
pub struct Lent<'a, W, M> {
    /// The data lent.
    pub(crate) model: M,
    /// The widget.
    pub(crate) widget: &'a mut W,
}

// ========================================================================
// Traits
// ========================================================================

/// What every widget's response says besides its payload: whether the
/// widget used the input, so a dispatcher such as
/// [`FocusTree::dispatch`](crate::FocusTree::dispatch) stops there or
/// offers it to the widget below.
pub trait Outcome {
    /// Whether the widget left the input alone.
    fn is_ignored(&self) -> bool;

    /// Keeps this response in `slot`, for the app to act on once the
    /// input has been handed out, and says whether the widget used the
    /// input: what a [`FocusTree::dispatch`](crate::FocusTree::dispatch)
    /// closure answers for the widget.
    fn keep(self, slot: &mut Option<Self>) -> bool
    where
        Self: Sized,
    {
        let used = !self.is_ignored();
        *slot = Some(self);
        used
    }
}

/// What every widget does: react to an input and draw itself.
pub trait Widget {
    /// What reacting to an input answers: whether the widget used it, and
    /// what the app should act on (an item chosen, a tab closed).
    type Response: Outcome;

    /// What drawing returns: the area left for the app's content, where
    /// the widget landed (for a focus trap), or nothing.
    type Output;

    /// Reacts to `input`, asking `cx` for what it needs: pointer inputs
    /// over the widget, keys and input method text while the app gives it
    /// the keyboard, and the window losing focus. A widget offered an
    /// input that is not its own answers that it ignored it.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Self::Response;

    /// Draws the widget into `area` of `frame` in `theme`'s colors. A
    /// popup or a dialog places itself inside `area` (usually the whole
    /// window), a bar along its edge.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Self::Output;

    /// Tells the widget whether it has the keyboard (or, for a container,
    /// whether something inside it does): a widget takes keys and draws
    /// its caret, cursor, or accent only while it does. Called by the app
    /// before drawing the widget, usually through
    /// [`FocusTree::sync`](crate::FocusTree::sync). The default, for a
    /// widget that never takes the keyboard, ignores it.
    fn set_focused(&mut self, _focused: bool) {}

    /// Whether the app last said the widget has the keyboard, through
    /// [`Widget::set_focused`]; always `false` for a widget that never
    /// takes it.
    fn is_focused(&self) -> bool {
        false
    }

    /// Steps what the widget animates or waits on (a caret's blink, a
    /// scroll's glide, a tooltip's delay, rows a source sent), asking `cx`
    /// for the redraws that shows, and returns when it next needs this
    /// call; `None` at rest. The app calls it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait)
    /// for each widget that may be animating and returns the earliest
    /// instant; a widget whose app never calls it shows its animations'
    /// ends at once. The default, for a widget that never animates, rests.
    fn about_to_wait(&mut self, _cx: &mut EventCx<'_>) -> Option<Instant> {
        None
    }
}

// ========================================================================
// Outcome Implementations
// ========================================================================

impl Outcome for () {
    /// A widget that takes no input leaves every input alone.
    fn is_ignored(&self) -> bool {
        true
    }
}

// ========================================================================
// Lent Implementations
// ========================================================================

impl<'a, W, M> Lent<'a, W, M> {
    /// `widget` lent `model`.
    pub(crate) fn new(widget: &'a mut W, model: M) -> Self {
        Self { model, widget }
    }
}

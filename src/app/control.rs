//! [`Control`]: what a form control (a button, a checkbox, a slider, a
//! text field) offers besides the [`Widget`] contract, so every control is
//! measured, kept in step with the app's state, and read for its changes
//! the same way.
//!
//! A control is *controlled*: its value comes from the app's state, and
//! the user's edits reach the app as values to store, not as state the
//! control keeps. [`Control::sync`] copies the app's props (the value, the
//! options, whether it is enabled) into the control each frame, keeping
//! the view state only the control knows (the caret, the scroll, an open
//! list, a drag); [`Control::changed`] reads the value a response reports,
//! for the app to store; and [`Control::measure`] says how tall the control
//! is drawn at a width, for the app to lay its rows out.

use super::super::render::font::CellMetrics;
use super::widget::Widget;

// ========================================================================
// Traits
// ========================================================================

/// A form control: a [`Widget`] the app measures, hands its state each
/// frame, and reads the values its responses report.
pub trait Control: Widget {
    /// What the app's state says the control shows: its value, its
    /// options, whether it takes input, borrowed for one
    /// [`Control::sync`].
    type Props<'a>
    where
        Self: 'a;

    /// What a change reports for the app to store: the new value.
    type Value;

    /// Copies `props` into the control, keeping the state only the control
    /// knows (the caret, the scroll, an open list, a drag). Props equal to
    /// the last ones change nothing, so the app may call it every frame,
    /// before handing the control an input or drawing it.
    fn sync(&mut self, props: Self::Props<'_>);

    /// How tall the control is drawn `width` pixels wide with `metrics` at
    /// `scale_factor`; `f32::INFINITY` asks the height at any width. Every
    /// control takes all three, so an app measures each the same way.
    fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32;

    /// The value `response` reports as changed, for the app to store; `None`
    /// for a response that changed nothing.
    fn changed(&self, response: Self::Response) -> Option<Self::Value>;
}

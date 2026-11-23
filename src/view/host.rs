//! [`Host`]: what a [`Store`](super::store::Store) builds a tree from and
//! hands back what the tree left: the state its elements read and its
//! callbacks change, and the hooks the keys and window events no element
//! took go to. A window an [`App`] describes is one host
//! ([`WindowHost`]); an area of a window someone else draws, holding a
//! [`HostedView`](super::embed::HostedView), is another.

use super::super::base::appearance::Appearance;
use super::super::base::key::Key;
use super::super::platform::window_id::WindowId;
use super::super::widgets::drag::FileDropped;
use super::app::{App, AppCx};
use super::element::Element;

// ========================================================================
// Data Structures
// ========================================================================

/// One window of an [`App`], as a [`Host`]: the app's tree for the window
/// and the app's hooks.
pub(crate) struct WindowHost<'a, A> {
    /// The app.
    pub(crate) app: &'a mut A,
    /// The window the tree is built for.
    pub(crate) window: WindowId,
}

// ========================================================================
// Traits
// ========================================================================

/// What a tree is built from and what it hands back to. Every hook but
/// [`Host::view`], [`Host::state`], and [`Host::key`] does nothing by
/// default, as nothing but a window has a use for it.
pub(crate) trait Host {
    /// The state the tree's elements read and its callbacks change.
    type State: 'static;

    /// The tree, built from the state.
    fn view(&self) -> Element<'_, Self::State>;

    /// The state, for the callbacks an input made to run on.
    fn state(&mut self) -> &mut Self::State;

    /// A key no element took; whether the host used it.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool;

    /// A key let go that no element took.
    fn key_released(&mut self, _key: Key, _cx: &mut AppCx<'_>) {}

    /// The user asked to close the window: whether it closes now.
    fn close_requested(&mut self, _cx: &mut AppCx<'_>) -> bool {
        false
    }

    /// The window gained or lost the keyboard.
    fn focus_changed(&mut self, _focused: bool) {}

    /// The user's appearance settings changed.
    fn follow_appearance(&mut self, _appearance: Appearance) {}

    /// A file was dropped where no element took it.
    fn file_dropped(&mut self, _file: FileDropped, _cx: &mut AppCx<'_>) {}

    /// Whether the host is done, so the app quits.
    fn should_exit(&self) -> bool {
        false
    }
}

// ========================================================================
// WindowHost Implementations
// ========================================================================

impl<A: App> Host for WindowHost<'_, A> {
    type State = A;

    fn view(&self) -> Element<'_, A> {
        self.app.view(self.window)
    }

    fn state(&mut self) -> &mut A {
        self.app
    }

    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        self.app.key(key, cx)
    }

    fn key_released(&mut self, key: Key, cx: &mut AppCx<'_>) {
        self.app.key_released(key, cx);
    }

    fn close_requested(&mut self, cx: &mut AppCx<'_>) -> bool {
        self.app.close_requested(self.window, cx)
    }

    fn focus_changed(&mut self, focused: bool) {
        self.app.focus_changed(self.window, focused);
    }

    fn follow_appearance(&mut self, appearance: Appearance) {
        self.app.follow_appearance(appearance);
    }

    fn file_dropped(&mut self, file: FileDropped, cx: &mut AppCx<'_>) {
        self.app.file_dropped(self.window, file, cx);
    }

    fn should_exit(&self) -> bool {
        self.app.should_exit()
    }
}

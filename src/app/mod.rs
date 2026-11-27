//! What an app is made of and reaches the runtime through: the
//! [`application`] trait and its context, the [`frame`] it draws into,
//! the widget [`context`], the [`widget`] contract and the form
//! [`control`] one, the [`focus`] tree
//! that dispatches input, the
//! [`access`]ibility tree, input method focus ([`ime`]), [`window`]
//! options, and, for tests, [`offscreen`] rendering. Behind features: the
//! system's file dialogs (`file_dialog`, `dialogs`), global hotkeys
//! (`hotkey`, `global-hotkey`), desktop notifications (`notify`,
//! `notify`), a single running copy (`single_instance`,
//! `single-instance`), and the tray icon (`tray`, `tray`).

pub(crate) mod access;
pub(crate) mod application;
pub(crate) mod context;
pub(crate) mod control;
#[cfg(feature = "dialogs")]
pub(crate) mod file_dialog;
pub(crate) mod focus;
pub(crate) mod frame;
pub(crate) mod hang;
#[cfg(feature = "global-hotkey")]
pub(crate) mod hotkey;
pub(crate) mod ime;
#[cfg(feature = "notify")]
pub(crate) mod notify;
#[cfg(any(test, feature = "testing"))]
pub(crate) mod offscreen;
#[cfg(feature = "single-instance")]
pub(crate) mod single_instance;
#[cfg(feature = "tray")]
pub(crate) mod tray;
pub(crate) mod widget;
pub(crate) mod window;

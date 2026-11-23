//! [`WindowId`]: the name of one open window, as the app knows it.

use winit::window;

// ========================================================================
// Data Structures
// ========================================================================

/// Names one window for as long as it is open.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WindowId(pub(crate) window::WindowId);

//! The chrome around a window's content, for an app that drives its own
//! window: the [`TitleBar`], the [`TabStrip`], the [`SplitView`] of panes,
//! and the [`StatusBar`]. Chrome is built on the widgets below it, never
//! the other way. An app described as a tree of elements builds the same
//! chrome with [`view`](crate::view).

pub(crate) mod split_view;
pub(crate) mod status_bar;
pub(crate) mod tab_strip;
pub(crate) mod title_bar;

// ========================================================================
// Public API
// ========================================================================

pub use self::split_view::{DropZone, SplitView, SplitViewResponse};
pub use self::status_bar::{
    ModeBadge, StatusBar, StatusBarResponse, StatusLine, StatusPrompt, StatusSegment,
};
pub use self::tab_strip::{
    TabClose, TabDrag, TabId, TabStrip, TabStripHit, TabStripResponse, TabStyle, TabWidth,
};
pub use self::title_bar::{
    TitleAlign, TitleBar, TitleBarButton, TitleBarHit, TitleBarResponse, WindowButton,
    WindowButtons,
};

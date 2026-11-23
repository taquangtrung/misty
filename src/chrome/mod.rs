//! The chrome around a window's content: the [`title_bar`], the
//! [`tab_strip`], the [`split_view`] of panes, and the [`status_bar`].
//! Chrome is built on the widgets below it, never the other way.

pub(crate) mod split_view;
pub(crate) mod status_bar;
pub(crate) mod tab_strip;
pub(crate) mod title_bar;

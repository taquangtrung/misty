//! The layer above `base`: what talks to the operating system and the event
//! loop without drawing or knowing about widgets: the user's
//! [`appearance`] settings, the [`clipboard`],
//! [`error`]s, window [`resize`] edges, the [`waker`] other threads post
//! through, the [`schedule`] of redraws, and each window's [`window_id`].

pub(crate) mod appearance;
pub(crate) mod clipboard;
pub(crate) mod error;
pub(crate) mod resize;
pub(crate) mod schedule;
pub(crate) mod waker;
pub(crate) mod window_id;

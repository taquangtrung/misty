//! The top layer: the [`event_loop`] that opens windows, reads their
//! events into inputs, calls the app (each call [`guard`]ed against
//! panics and [`watchdog`]ed against hangs), and draws its frames, and the
//! runner of an app described by a view ([`tree`]).

pub(crate) mod event_loop;
pub(crate) mod guard;
pub(crate) mod tree;
pub(crate) mod watchdog;

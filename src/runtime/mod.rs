//! The top layer: the [`event_loop`] that opens windows, reads their
//! events into inputs, calls the app (each call [`guard`]ed against
//! panics), and draws its frames.

pub(crate) mod event_loop;
pub(crate) mod guard;

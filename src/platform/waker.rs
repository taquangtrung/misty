//! [`Waker`]: how a background thread hands the app a message, or wakes
//! the event loop, while the loop sleeps.

use std::fmt::{self, Debug, Formatter};

use thiserror::Error;
use winit::event_loop::{EventLoopClosed, EventLoopProxy};

use super::super::base::appearance::Appearance;

// ========================================================================
// Data Structures
// ========================================================================

/// A handle any thread may hold to reach the app: [`Waker::send`] delivers
/// a message to [`Application::message`], and [`Waker::wake`] only wakes
/// the event loop, so [`Application::about_to_wait`] runs. Get one from
/// [`AppContext::waker`] and clone it freely.
///
/// [`Application::message`]: crate::Application::message
/// [`Application::about_to_wait`]: crate::Application::about_to_wait
/// [`AppContext::waker`]: crate::AppContext::waker
pub struct Waker<M: 'static> {
    /// The event loop's inbox.
    proxy: EventLoopProxy<UserEvent<M>>,
}

/// Why [`Waker::send`] could not deliver a message: the app has quit.
/// The message comes back, through [`SendError::into_message`].
#[derive(Error)]
#[error("the app has quit, so the message was not delivered")]
pub struct SendError<M>(M);

/// What a [`Waker`] puts in the event loop's inbox.
#[derive(Debug)]
pub(crate) enum UserEvent<M> {
    /// The user's appearance settings changed to this.
    Appearance(Appearance),
    /// What an assistive technology asked of a window.
    Access(accesskit_winit::Event),
    /// A message for the app.
    Message(M),
    /// Nothing but a wake-up.
    Wake,
}

// ========================================================================
// SendError Implementations
// ========================================================================

impl<M> SendError<M> {
    /// The message that was not delivered.
    pub fn into_message(self) -> M {
        self.0
    }
}

impl<M> Debug for SendError<M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendError").finish_non_exhaustive()
    }
}

// ========================================================================
// Waker Implementations
// ========================================================================

impl<M: 'static> Waker<M> {
    /// A waker posting to `proxy`.
    pub(crate) fn new(proxy: EventLoopProxy<UserEvent<M>>) -> Self {
        Self { proxy }
    }

    /// Hands `message` to the app, waking the event loop if it sleeps. Once
    /// the app has quit, the message comes back in the error.
    pub fn send(&self, message: M) -> Result<(), SendError<M>> {
        self.proxy
            .send_event(UserEvent::Message(message))
            .map_err(|EventLoopClosed(event)| match event {
                UserEvent::Message(message) => SendError(message),
                // Sent as a message just above, so it comes back as one.
                UserEvent::Access(_) | UserEvent::Appearance(_) | UserEvent::Wake => {
                    unreachable!("a message came back as another event")
                }
            })
    }

    /// The event loop's inbox, for the accessibility adapters.
    pub(crate) fn proxy(&self) -> EventLoopProxy<UserEvent<M>> {
        self.proxy.clone()
    }

    /// Wakes the event loop without a message, for an app that polls its
    /// own channels in [`Application::about_to_wait`]. Does nothing once the
    /// app has quit.
    ///
    /// [`Application::about_to_wait`]: crate::Application::about_to_wait
    pub fn wake(&self) {
        // A loop that has closed has nothing left to wake.
        let _ = self.proxy.send_event(UserEvent::Wake);
    }
}

impl<M> From<accesskit_winit::Event> for UserEvent<M> {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Access(event)
    }
}

impl<M: 'static> Clone for Waker<M> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
        }
    }
}

impl<M: 'static> Debug for Waker<M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Waker").finish_non_exhaustive()
    }
}

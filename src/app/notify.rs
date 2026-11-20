//! [`Notification`]: a desktop notification, behind the `notify` feature,
//! through the `notify-rust` crate: the freedesktop notification service
//! over D-Bus on Linux and the BSDs, toasts on Windows, and the
//! notification center on macOS. A click on it, a press of one of its
//! buttons, or its closing comes back to
//! [`Application::message`](crate::Application::message) as a message the
//! app makes from a [`NotificationEvent`].

use std::thread;

use notify_rust::Notification as SystemNotification;
#[cfg(not(target_os = "macos"))]
use notify_rust::Urgency as SystemUrgency;
use thiserror::Error;

use super::application::AppContext;

// ========================================================================
// Constants
// ========================================================================

/// The action the notification service reports a click on the
/// notification itself as.
const DEFAULT_ACTION: &str = "default";

/// What `notify-rust` reports a notification closed without an action as.
const CLOSED_ACTION: &str = "__closed";

/// The name of the thread a notification's answer is awaited on.
const THREAD_NAME: &str = "misty notification";

// ========================================================================
// Data Structures
// ========================================================================

/// A desktop notification to show: start from [`Notification::new`], add
/// what you need with the `with_` methods, and [`Notification::show`] it.
///
/// ```no_run
/// use misty::{AppContext, Notification, NotificationEvent, Urgency};
///
/// enum Message {
///     Notified(NotificationEvent),
/// }
///
/// fn done(cx: &AppContext<'_, Message>) {
///     let shown = Notification::new("Export finished")
///         .with_body("report.pdf is ready")
///         .with_urgency(Urgency::Low)
///         .with_action("open", "Open")
///         .show(cx, Message::Notified);
///     if let Err(error) = shown {
///         eprintln!("{error}");
///     }
/// }
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Notification {
    /// The buttons, as an id and a label each.
    actions: Vec<(String, String)>,
    /// The text under the title.
    body: String,
    /// An icon name from the icon theme or a path to an image file.
    icon: Option<String>,
    /// The title.
    title: String,
    /// How much the notification matters.
    urgency: Urgency,
}

/// How much a notification matters, which the system may show by staying
/// longer or interrupting a do-not-disturb mode. Windows and Linux follow
/// it; macOS does not.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Urgency {
    /// It needs the user now, such as a failure; it stays until closed.
    Critical,
    /// It can wait, such as a background task finishing.
    Low,
    /// Everything else.
    #[default]
    Normal,
}

/// What the user did with a notification.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum NotificationEvent {
    /// The user pressed the button with this id (on macOS, its label).
    Action(String),
    /// The user clicked the notification itself.
    Clicked,
    /// The notification closed without either: dismissed or timed out.
    Closed,
}

/// Why a notification could not be shown.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NotifyError {
    /// The system's notification service refused it, for this reason.
    #[error(
        "the notification could not be shown ({0}): on Linux a notification service must run \
         on the session bus (every desktop has one; a bare window manager needs one such as \
         dunst or mako)"
    )]
    Unavailable(String),
}

// ========================================================================
// Notification Implementations
// ========================================================================

impl Notification {
    /// A notification titled `title`, with no body, icon, or buttons, of
    /// normal urgency.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// This notification with `body` under its title.
    pub fn with_body(self, body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            ..self
        }
    }

    /// This notification showing `icon`: a name from the icon theme (Linux)
    /// or a path to an image file.
    pub fn with_icon(self, icon: impl Into<String>) -> Self {
        Self {
            icon: Some(icon.into()),
            ..self
        }
    }

    /// This notification as urgent as `urgency` says.
    pub fn with_urgency(self, urgency: Urgency) -> Self {
        Self { urgency, ..self }
    }

    /// This notification with a button labeled `label`, reported as
    /// [`NotificationEvent::Action`] with `id` when pressed. Notification
    /// services may show only the first few buttons, or none.
    pub fn with_action(mut self, id: impl Into<String>, label: impl Into<String>) -> Self {
        self.actions.push((id.into(), label.into()));
        self
    }

    /// Shows the notification, then hands what the user does with it to
    /// [`Application::message`](crate::Application::message) as
    /// `message(event)`, once, without blocking the event loop. Refused
    /// when the system has no notification service.
    pub fn show<M: Send + 'static>(
        self,
        cx: &AppContext<'_, M>,
        message: impl FnOnce(NotificationEvent) -> M + Send + 'static,
    ) -> Result<(), NotifyError> {
        let mut system = SystemNotification::new();
        system.summary(&self.title).body(&self.body);
        // The freedesktop service reports clicks only on a notification
        // with a default action; elsewhere it would be one more button.
        #[cfg(all(unix, not(target_os = "macos")))]
        system.action(DEFAULT_ACTION, "");
        for (id, label) in &self.actions {
            system.action(id, label);
        }
        if let Some(icon) = &self.icon {
            system.icon(icon);
        }
        #[cfg(not(target_os = "macos"))]
        system.urgency(match self.urgency {
            Urgency::Critical => SystemUrgency::Critical,
            Urgency::Low => SystemUrgency::Low,
            Urgency::Normal => SystemUrgency::Normal,
        });
        let handle = system
            .show()
            .map_err(|error| NotifyError::Unavailable(error.to_string()))?;
        let waker = cx.waker();
        thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                handle.wait_for_action(|action| {
                    // An app that has quit wants no answer.
                    let _ = waker.send(message(event_of(action)));
                });
            })
            .map_err(|error| NotifyError::Unavailable(error.to_string()))?;
        Ok(())
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the user did, from the action `notify-rust` reports.
fn event_of(action: &str) -> NotificationEvent {
    match action {
        DEFAULT_ACTION => NotificationEvent::Clicked,
        CLOSED_ACTION => NotificationEvent::Closed,
        id => NotificationEvent::Action(id.to_owned()),
    }
}

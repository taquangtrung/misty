//! What an app does about a hang: a callback on the main thread that runs
//! past a limit, which a panic guard cannot catch. The app gives its
//! [`HangWatch`] (see [`Application::hang_watch`]); the runtime's watchdog
//! thread tells it a [`HangReport`] of each callback that hangs.
//!
//! [`Application::hang_watch`]: super::application::Application::hang_watch

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

// ========================================================================
// Constants
// ========================================================================

/// How long a callback runs before it counts as hung, unless the app's
/// [`HangWatch`] says otherwise: far past any frame or input, short of the
/// minutes a stuck window loses the user.
const DEFAULT_LIMIT: Duration = Duration::from_secs(5);

// ========================================================================
// Data Structures
// ========================================================================

/// What an app does about a callback that does not return: how long one
/// may run, and what is called, on the watchdog thread, once it has run
/// longer and again each limit after while it stays stuck. The default
/// logs the first [`HangReport`] of each hang as an error (the `log`
/// crate) after five seconds; [`HangWatch::off`] watches nothing.
#[derive(Clone)]
pub struct HangWatch {
    /// How long a callback may run.
    pub(crate) limit: Duration,
    /// What is told of a hang, or `None` to watch nothing.
    pub(crate) on_hang: Option<Arc<dyn Fn(&HangReport) + Send + Sync>>,
}

/// A callback on the main thread that has run past its [`HangWatch`]'s
/// limit, as the watchdog thread found it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct HangReport {
    /// The memory the process is using, in bytes, where the system says.
    pub memory: Option<u64>,
    /// What the main thread was doing.
    pub phase: HangPhase,
    /// How many times this hang was told before: 0 the first time, once
    /// it passed the limit, and one more each limit after while it lasts.
    pub repeats: u32,
    /// How long it has been at it.
    pub stuck_for: Duration,
}

/// What the main thread was doing when it hung.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[repr(u8)]
pub enum HangPhase {
    /// Drawing a frame: [`Application::render`](crate::Application::render).
    Drawing,
    /// Handling an input or a window's change.
    Input,
    /// Handling a [`Waker`](crate::Waker) message.
    Message,
    /// The work before the loop waits:
    /// [`Application::about_to_wait`](crate::Application::about_to_wait).
    Waiting,
}

// ========================================================================
// HangWatch Implementations
// ========================================================================

impl HangWatch {
    /// The default watch: a callback may run five seconds, and a hang is
    /// logged.
    pub fn new() -> Self {
        Self::default()
    }

    /// A watch of nothing: no thread runs, no hang is told.
    pub fn off() -> Self {
        Self {
            on_hang: None,
            ..Self::default()
        }
    }

    /// This watch letting a callback run `limit` before it counts as hung.
    pub fn limit(self, limit: Duration) -> Self {
        Self { limit, ..self }
    }

    /// This watch calling `on_hang` with each hang, on the watchdog thread:
    /// once the callback passes the limit, and again each limit after while
    /// it lasts, [`HangReport::repeats`] counting, so an app may report a
    /// hang at once and end the process only if it goes on. The main thread
    /// is the one stuck, so `on_hang` cannot reach the app's state; it may
    /// write a report, end the process, or start the app again.
    pub fn on_hang(self, on_hang: impl Fn(&HangReport) + Send + Sync + 'static) -> Self {
        Self {
            on_hang: Some(Arc::new(on_hang)),
            ..self
        }
    }
}

impl Default for HangWatch {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            on_hang: Some(Arc::new(|report: &HangReport| {
                if report.repeats == 0 {
                    log::error!("{report}");
                }
            })),
        }
    }
}

impl fmt::Debug for HangWatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HangWatch")
            .field("limit", &self.limit)
            .field("on", &self.on_hang.is_some())
            .finish()
    }
}

// ========================================================================
// HangReport Implementations
// ========================================================================

impl HangReport {
    /// A report of the main thread `phase` for `stuck_for`, told `repeats`
    /// times before, the memory unknown: what an app's tests hand its
    /// [`HangWatch::on_hang`].
    pub fn new(phase: HangPhase, stuck_for: Duration, repeats: u32) -> Self {
        Self {
            memory: None,
            phase,
            repeats,
            stuck_for,
        }
    }
}

impl fmt::Display for HangReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the main thread has been {} for {:.1}s without returning",
            self.phase,
            self.stuck_for.as_secs_f32()
        )?;
        if let Some(memory) = self.memory {
            write!(f, ", with {} MB in use", memory / (1024 * 1024))?;
        }
        Ok(())
    }
}

// ========================================================================
// HangPhase Implementations
// ========================================================================

impl HangPhase {
    /// The phase stored as `value`.
    pub(crate) fn from_u8(value: u8) -> Self {
        match value {
            v if v == Self::Drawing as u8 => Self::Drawing,
            v if v == Self::Message as u8 => Self::Message,
            v if v == Self::Waiting as u8 => Self::Waiting,
            _ => Self::Input,
        }
    }
}

impl fmt::Display for HangPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Drawing => "drawing a frame",
            Self::Input => "handling an input",
            Self::Message => "handling a message",
            Self::Waiting => "doing its work before waiting",
        })
    }
}

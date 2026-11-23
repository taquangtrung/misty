//! Why [`run`](crate::run) stopped early.

use std::error::Error as StdError;

use thiserror::Error;
use wgpu::{CreateSurfaceError, RequestAdapterError, RequestDeviceError};
use winit::error::{EventLoopError, OsError};

// ========================================================================
// Data Structures
// ========================================================================

/// Why the app could not start or keep running. Each variant is one step of
/// bringing up a window and its GPU surface, and carries the window
/// system's or the GPU driver's own error as its
/// [`source`](std::error::Error::source), to log or show; the cause's type
/// is not part of misty's API, so match on the variant only.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The GPU driver could not make a drawing surface for the window.
    #[error("could not create a GPU surface for the window: {0}")]
    CreateSurface(#[source] Box<dyn StdError + Send + Sync>),
    /// The window system refused to open the window.
    #[error("could not open the window: {0}")]
    CreateWindow(#[source] Box<dyn StdError + Send + Sync>),
    /// The window system's event loop could not start or failed while running.
    #[error("the event loop failed: {0}")]
    EventLoop(#[source] Box<dyn StdError + Send + Sync>),
    /// No GPU, not even a software one, can draw to the window.
    #[error("no GPU adapter can draw to the window: {0}")]
    NoAdapter(#[source] Box<dyn StdError + Send + Sync>),
    /// A headless test app (the `testing` feature's `TestApp`) asked to
    /// open another window, which needs a window system; test each window
    /// with a `TestApp` of its own.
    #[error("a headless test app cannot open windows; give each window a TestApp of its own")]
    Headless,
    /// The GPU adapter was found but would not open a device.
    #[error("the GPU adapter would not open a device: {0}")]
    RequestDevice(#[source] Box<dyn StdError + Send + Sync>),
    /// The GPU adapter cannot present to the window's surface in any format.
    #[error("the GPU adapter cannot present to the window")]
    SurfaceUnsupported,
}

// ========================================================================
// Error Implementations
// ========================================================================

impl Error {
    /// A surface the GPU driver would not create.
    pub(crate) fn create_surface(cause: CreateSurfaceError) -> Self {
        Self::CreateSurface(Box::new(cause))
    }

    /// A window the window system would not open.
    pub(crate) fn create_window(cause: OsError) -> Self {
        Self::CreateWindow(Box::new(cause))
    }

    /// An event loop that would not start or failed while running.
    pub(crate) fn event_loop(cause: EventLoopError) -> Self {
        Self::EventLoop(Box::new(cause))
    }

    /// No adapter that can draw.
    pub(crate) fn no_adapter(cause: RequestAdapterError) -> Self {
        Self::NoAdapter(Box::new(cause))
    }

    /// An adapter that would not open a device.
    pub(crate) fn request_device(cause: RequestDeviceError) -> Self {
        Self::RequestDevice(Box::new(cause))
    }
}

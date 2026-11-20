//! The GPU every window draws with, [`GpuContext`]: one instance, adapter,
//! device, and queue, shared by all windows' surfaces; and [`Gpu`], the
//! device and queue lent to an app that renders into textures of its own.

use std::sync::Arc;

use pollster::block_on;
#[cfg(any(test, feature = "testing"))]
use wgpu::DeviceType;
use wgpu::{
    Adapter, Backends, Device, DeviceDescriptor, DownlevelFlags, Instance, InstanceDescriptor,
    PowerPreference, Queue, RequestAdapterError, RequestAdapterOptions, Surface, TextureFormat,
    TextureUsages,
};
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

use super::super::platform::error::Error;

// ========================================================================
// Constants
// ========================================================================

/// The format of every [`GpuTexture`](crate::GpuTexture): RGBA, eight bits
/// a channel, sRGB-encoded values with straight alpha, as an image's
/// pixels are and as Vello writes.
pub(crate) const APP_TEXTURE_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// The format of an app texture's sRGB view, which encodes the linear
/// color written through it into the texture's sRGB-encoded values.
pub(crate) const APP_TEXTURE_SRGB_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

// ========================================================================
// Data Structures
// ========================================================================

/// The GPU and the connection to it. Made once, with the first window, and
/// shared by every window after it.
pub(crate) struct GpuContext {
    /// The adapter the device was opened on.
    pub(crate) adapter: Adapter,
    /// The device every GPU resource is made on.
    pub(crate) device: Device,
    /// The connection to the graphics API, which makes surfaces.
    instance: Instance,
    /// The device's command queue.
    pub(crate) queue: Queue,
}

/// The GPU misty draws with, lent to an app that renders into textures of
/// its own: a vector renderer such as Vello, a video decoder, a 3D view.
/// The app makes a [`GpuTexture`](crate::GpuTexture) on it, renders into
/// the texture with [`Gpu::device`] and [`Gpu::queue`], and draws it with
/// [`Frame::draw_texture`](crate::Frame::draw_texture). Work the app
/// submits to the queue before a frame is drawn is finished before the
/// frame reads the texture. Cloning it is cheap; every clone is the same
/// GPU. [`AppContext::gpu`](crate::AppContext::gpu) gives it out.
#[derive(Clone, Debug)]
pub struct Gpu {
    /// The device every GPU resource is made on.
    device: Device,
    /// The device's command queue.
    queue: Queue,
    /// Whether an app texture can be viewed in its sRGB format.
    srgb_views: bool,
    /// What an app texture can be used for on this adapter.
    texture_usages: TextureUsages,
}

// ========================================================================
// Gpu Implementations
// ========================================================================

impl Gpu {
    /// The device: make the app's pipelines, buffers, and renderers on it
    /// (`vello::Renderer::new(gpu.device(), …)`).
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The device's command queue: submit the app's rendering on it.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// What an app texture can be used for: sampled by misty, copied into,
    /// and rendered into, plus written as a storage texture (as Vello
    /// writes) where the adapter allows it for the format.
    pub(crate) fn texture_usages(&self) -> TextureUsages {
        self.texture_usages
    }

    /// Whether an app texture can be viewed in its sRGB format: the
    /// adapter allows a view in another format than the texture's own,
    /// which OpenGL may not.
    pub(crate) fn has_srgb_views(&self) -> bool {
        self.srgb_views
    }
}

// ========================================================================
// GpuContext Implementations
// ========================================================================

impl GpuContext {
    /// The GPU context and the surface of `window`, the first window: on the
    /// low-power GPU if there is a choice, or on a software adapter when no
    /// hardware one can draw to the window. Vulkan, Metal, or DX12 come
    /// first; when none can draw to the window (an old driver, a virtual
    /// machine, a virtual display), OpenGL is tried, unless `WGPU_BACKEND`
    /// chose the backends. `display` is the event loop's connection to the
    /// window system, which the GL backend draws through.
    pub(crate) fn new(
        display: OwnedDisplayHandle,
        window: Arc<Window>,
    ) -> Result<(Self, Surface<'static>), Error> {
        let primary = Self::for_window(Backends::PRIMARY, display.clone(), window.clone());
        let Err(error) = primary else {
            return primary;
        };
        let Some(fallback) = fallback_backends(Backends::from_env()) else {
            return Err(error);
        };
        log::warn!(
            "no Vulkan, Metal, or DX12 adapter can draw the window ({error}); trying OpenGL"
        );
        // When OpenGL fails too, the first failure says more.
        Self::for_window(fallback, display, window).map_err(|_| error)
    }

    /// The GPU context and the surface of `window` on `backends`, or on the
    /// ones `WGPU_BACKEND` names when it is set.
    fn for_window(
        backends: Backends,
        display: OwnedDisplayHandle,
        window: Arc<Window>,
    ) -> Result<(Self, Surface<'static>), Error> {
        let descriptor = InstanceDescriptor {
            backends,
            ..InstanceDescriptor::new_with_display_handle(Box::new(display))
        };
        let instance = Instance::new(descriptor.with_env());
        let surface = instance
            .create_surface(window)
            .map_err(Error::create_surface)?;
        let adapter = request_adapter(&instance, Some(&surface))?;
        let gpu = Self::open(instance, adapter)?;
        Ok((gpu, surface))
    }

    /// A GPU context with no window, drawing only into textures: on any
    /// backend, GL included, unless `WGPU_BACKEND` narrows the choice. Each
    /// adapter is tried in turn, hardware before software, until one opens
    /// a device: a GPU reached without a window system may be listed and
    /// still refuse.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn headless() -> Result<Self, Error> {
        let descriptor = InstanceDescriptor {
            backends: Backends::PRIMARY | Backends::SECONDARY,
            ..InstanceDescriptor::new_without_display_handle()
        }
        .with_env();
        let backends = descriptor.backends;
        let instance = Instance::new(descriptor);
        let mut adapters = block_on(instance.enumerate_adapters(backends));
        adapters.sort_by_key(|adapter| adapter.get_info().device_type == DeviceType::Cpu);
        let mut refusal = None;
        for adapter in adapters {
            match Self::open(instance.clone(), adapter) {
                Ok(gpu) => return Ok(gpu),
                Err(error) => refusal = Some(error),
            }
        }
        match refusal {
            Some(error) => Err(error),
            // No adapter at all: asking for one says why.
            None => Self::open(instance.clone(), request_adapter(&instance, None)?),
        }
    }

    /// The context of a device opened on `adapter` of `instance`.
    fn open(instance: Instance, adapter: Adapter) -> Result<Self, Error> {
        let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor::default()))
            .map_err(Error::request_device)?;
        // wgpu's default handler panics on any validation error. Log it
        // instead: what failed is not drawn, and the app goes on.
        device.on_uncaptured_error(Arc::new(|error| log::error!("GPU error: {error}")));
        Ok(Self {
            adapter,
            device,
            instance,
            queue,
        })
    }

    /// The device and queue as an app borrows them.
    pub(crate) fn lend(&self) -> Gpu {
        let wanted = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC
            | TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::STORAGE_BINDING;
        let allowed = self
            .adapter
            .get_texture_format_features(APP_TEXTURE_FORMAT)
            .allowed_usages;
        let downlevel = self.adapter.get_downlevel_capabilities().flags;
        Gpu {
            device: self.device.clone(),
            queue: self.queue.clone(),
            srgb_views: downlevel.contains(DownlevelFlags::VIEW_FORMATS),
            texture_usages: wanted & allowed,
        }
    }

    /// A surface for `window`, a window opened after the first.
    pub(crate) fn create_surface(&self, window: Arc<Window>) -> Result<Surface<'static>, Error> {
        self.instance
            .create_surface(window)
            .map_err(Error::create_surface)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The backends to try a window on when the primary ones fail, given the
/// ones `WGPU_BACKEND` chose: OpenGL, or none when the variable chose,
/// since the user's choice is kept.
fn fallback_backends(from_env: Option<Backends>) -> Option<Backends> {
    match from_env {
        Some(_) => None,
        None => Some(Backends::GL),
    }
}

/// An adapter that can draw to `surface` (any adapter when `None`),
/// preferring low power. When no hardware adapter qualifies (a virtual
/// display, a missing driver), falls back to the platform's software
/// adapter.
fn request_adapter(instance: &Instance, surface: Option<&Surface<'_>>) -> Result<Adapter, Error> {
    let request = |force_fallback_adapter| -> Result<Adapter, RequestAdapterError> {
        block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: surface,
            force_fallback_adapter,
            power_preference: PowerPreference::LowPower,
            ..RequestAdapterOptions::default()
        }))
    };
    request(false)
        .or_else(|_| request(true))
        .map_err(Error::no_adapter)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_window_falls_back_to_gl_unless_wgpu_backend_chose() {
        assert_eq!(fallback_backends(None), Some(Backends::GL));
        assert_eq!(fallback_backends(Some(Backends::VULKAN)), None);
    }
}

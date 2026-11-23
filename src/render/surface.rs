//! A window's GPU surface: the swapchain configuration kept in step with
//! the window's size, drawn to with the shared [`GpuContext`].
//!
//! Frames are presented without waiting for the display where the driver
//! allows it ([`PresentMode::Mailbox`]): the window's thread, which also
//! hears every key and click, must not park until the compositor hands a
//! buffer back. A compositor the CPU starves (a build on every core) would
//! hold each frame, and the window would seem to hang while it waited.

use wgpu::{
    CompositeAlphaMode, CurrentSurfaceTexture, PresentMode, Surface, SurfaceConfiguration,
    SurfaceTexture, TextureFormat,
};

use super::super::base::color::Rgba;
use super::super::platform::error::Error;
use super::gpu::GpuContext;

// ========================================================================
// Data Structures
// ========================================================================

/// A window's drawing surface.
pub(crate) struct WindowSurface {
    /// Whether the surface can composite with premultiplied alpha.
    can_blend: bool,
    /// The swapchain size and format, reapplied on every resize.
    config: SurfaceConfiguration,
    /// The alpha mode the surface takes when it is opaque.
    opaque_mode: CompositeAlphaMode,
    /// The surface itself, which keeps the window alive.
    surface: Surface<'static>,
}

// ========================================================================
// WindowSurface Implementations
// ========================================================================

impl WindowSurface {
    /// `surface` configured for `gpu` at `width` by `height` physical
    /// pixels, composited with its alpha over the desktop when
    /// `transparent` asks and the surface can.
    pub(crate) fn new(
        gpu: &GpuContext,
        surface: Surface<'static>,
        width: u32,
        height: u32,
        transparent: bool,
    ) -> Result<Self, Error> {
        let max_side = gpu.device.limits().max_texture_dimension_2d;
        let mut config = surface
            .get_default_config(
                &gpu.adapter,
                clamp_side(width, max_side),
                clamp_side(height, max_side),
            )
            .ok_or(Error::SurfaceUnsupported)?;
        let capabilities = surface.get_capabilities(&gpu.adapter);
        config.present_mode = present_mode(&capabilities.present_modes, config.present_mode);
        let can_blend = capabilities
            .alpha_modes
            .contains(&CompositeAlphaMode::PreMultiplied);
        let mut this = Self {
            can_blend,
            opaque_mode: config.alpha_mode,
            config,
            surface,
        };
        this.set_transparent(gpu, transparent);
        Ok(this)
    }

    /// Composites the surface with its alpha over the desktop when
    /// `transparent` asks and it can, or opaque.
    pub(crate) fn set_transparent(&mut self, gpu: &GpuContext, transparent: bool) {
        self.config.alpha_mode = match transparent && self.can_blend {
            true => CompositeAlphaMode::PreMultiplied,
            false => self.opaque_mode,
        };
        self.surface.configure(&gpu.device, &self.config);
    }

    /// Whether the surface composites with its alpha over the desktop, so
    /// what a frame leaves see-through shows what is behind the window.
    pub(crate) fn is_blending(&self) -> bool {
        self.config.alpha_mode == CompositeAlphaMode::PreMultiplied
    }

    /// The pixel format of the swapchain's textures.
    pub(crate) fn format(&self) -> TextureFormat {
        self.config.format
    }

    /// `color`, the color a frame is cleared to, as this surface takes it:
    /// premultiplied by its alpha when the surface composites over the
    /// desktop, as is.
    pub(crate) fn clear_color(&self, color: Rgba) -> Rgba {
        match self.config.alpha_mode {
            CompositeAlphaMode::PreMultiplied => premultiplied(color),
            _ => color,
        }
    }

    /// The swapchain size, `(width, height)` in physical pixels.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Resizes the swapchain to `width` by `height` physical pixels. A zero
    /// side (a minimized window) is drawn at one pixel, and a side past the
    /// GPU's largest texture at that largest size, rather than refused.
    pub(crate) fn resize(&mut self, gpu: &GpuContext, width: u32, height: u32) {
        let max_side = gpu.device.limits().max_texture_dimension_2d;
        let width = clamp_side(width, max_side);
        let height = clamp_side(height, max_side);
        if (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&gpu.device, &self.config);
    }

    /// The next swapchain texture, reconfiguring once when the surface went
    /// out of date, or `None` when there is nothing to draw to right now
    /// (occluded, timed out) and the caller may try again later.
    pub(crate) fn acquire_texture(&mut self, gpu: &GpuContext) -> Option<SurfaceTexture> {
        match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture)
            | CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
            CurrentSurfaceTexture::Lost | CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&gpu.device, &self.config);
                match self.surface.get_current_texture() {
                    CurrentSurfaceTexture::Success(texture)
                    | CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
                    CurrentSurfaceTexture::Lost
                    | CurrentSurfaceTexture::Occluded
                    | CurrentSurfaceTexture::Outdated
                    | CurrentSurfaceTexture::Timeout
                    | CurrentSurfaceTexture::Validation => None,
                }
            }
            CurrentSurfaceTexture::Occluded
            | CurrentSurfaceTexture::Timeout
            | CurrentSurfaceTexture::Validation => None,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How to present frames, of the modes `available`: one that never makes
/// the window's thread wait on the display where there is one, else
/// `default`, the one every surface has.
fn present_mode(available: &[PresentMode], default: PresentMode) -> PresentMode {
    if available.contains(&PresentMode::Mailbox) {
        PresentMode::Mailbox
    } else {
        default
    }
}

/// `color` with each channel scaled by its alpha.
fn premultiplied(color: Rgba) -> Rgba {
    let scale = |channel: u8| (u16::from(channel) * u16::from(color.a) / u16::from(u8::MAX)) as u8;
    Rgba::new(scale(color.r), scale(color.g), scale(color.b), color.a)
}

/// One side of the surface, held to what the GPU can configure: at least a
/// pixel and at most `max_side`.
fn clamp_side(side: u32, max_side: u32) -> u32 {
    side.clamp(1, max_side)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frames_skip_the_wait_for_the_display_where_the_driver_offers_it() {
        let offered = [
            PresentMode::Fifo,
            PresentMode::Immediate,
            PresentMode::Mailbox,
        ];
        assert_eq!(
            present_mode(&offered, PresentMode::Fifo),
            PresentMode::Mailbox
        );
        assert_eq!(
            present_mode(&[PresentMode::Fifo], PresentMode::Fifo),
            PresentMode::Fifo,
            "every surface presents in turn, so that is kept where nothing else is"
        );
    }
}

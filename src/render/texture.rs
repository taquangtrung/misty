//! [`GpuTexture`]: a texture an app renders into on misty's GPU, drawn in a
//! frame like an image.

use wgpu::{
    Extent3d, TexelCopyBufferLayout, Texture, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureView, TextureViewDescriptor,
};

use super::gpu::{APP_TEXTURE_FORMAT, APP_TEXTURE_SRGB_FORMAT, Gpu};
use super::image::{BYTES_PER_PIXEL, ImageError, PictureId};

// ========================================================================
// Constants
// ========================================================================

/// The label GPU debuggers show for app textures.
const LABEL: &str = "misty app texture";

// ========================================================================
// Data Structures
// ========================================================================

/// A texture an app renders into on misty's [`Gpu`], with a renderer of its
/// own (Vello, a video decoder, a 3D view), and draws with
/// [`Frame::draw_texture`](crate::Frame::draw_texture) as it would an
/// image: stretched over a rect, cut by clips, faded by opacity.
///
/// Its format is [`GpuTexture::FORMAT`]: RGBA, eight bits a channel,
/// sRGB-encoded values with straight (not premultiplied) alpha. It can be
/// sampled, copied into and out of, rendered into as a color attachment,
/// and, where the adapter allows it ([`GpuTexture::is_storage`]), written
/// as a storage texture, as Vello's `render_to_texture` writes. Rendering
/// into it again changes what the next frame draws; nothing needs to be
/// uploaded. A new size needs a new texture. Clones share the texture.
#[derive(Clone, Debug)]
pub struct GpuTexture {
    /// Names it to the renderer's texture cache.
    id: PictureId,
    /// Whether it can be viewed in [`GpuTexture::SRGB_FORMAT`].
    srgb_views: bool,
    /// The texture.
    texture: Texture,
}

// ========================================================================
// GpuTexture Implementations
// ========================================================================

impl GpuTexture {
    /// The format of every app texture.
    pub const FORMAT: TextureFormat = APP_TEXTURE_FORMAT;

    /// The format of its sRGB view ([`GpuTexture::srgb_view`]).
    pub const SRGB_FORMAT: TextureFormat = APP_TEXTURE_SRGB_FORMAT;

    /// A transparent texture `width` by `height` pixels on `gpu`. Fails
    /// with [`ImageError::Empty`] for a zero side, and with
    /// [`ImageError::TooLarge`] for a side past the GPU's largest texture
    /// (split the content into tiles, or render it smaller and stretch it).
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::Empty);
        }
        let max_side = gpu.device().limits().max_texture_dimension_2d;
        if width > max_side || height > max_side {
            return Err(ImageError::TooLarge { max_side });
        }
        let texture = gpu.device().create_texture(&TextureDescriptor {
            dimension: TextureDimension::D2,
            format: Self::FORMAT,
            label: Some(LABEL),
            mip_level_count: 1,
            sample_count: 1,
            size: Extent3d {
                depth_or_array_layers: 1,
                height,
                width,
            },
            usage: gpu.texture_usages(),
            view_formats: match gpu.has_srgb_views() {
                true => &[APP_TEXTURE_SRGB_FORMAT],
                false => &[],
            },
        });
        Ok(Self {
            id: PictureId::next(),
            srgb_views: gpu.has_srgb_views(),
            texture,
        })
    }

    /// The texture, to render into: make a view of it with
    /// `texture().create_view(&Default::default())`.
    pub fn texture(&self) -> &Texture {
        &self.texture
    }

    /// A view of it in [`GpuTexture::SRGB_FORMAT`], for a renderer that
    /// blends in linear light (gamma-correct text, say): what it writes
    /// through the view is encoded to the texture's sRGB values on the
    /// way. `None` where the adapter allows no view in another format
    /// (OpenGL may not); render into the texture itself there, writing
    /// sRGB-encoded values.
    pub fn srgb_view(&self) -> Option<TextureView> {
        self.srgb_views.then(|| {
            // An sRGB format cannot be written as storage, so the view leaves
            // that usage out.
            let usage = self.texture.usage() - TextureUsages::STORAGE_BINDING;
            self.texture.create_view(&TextureViewDescriptor {
                format: Some(APP_TEXTURE_SRGB_FORMAT),
                usage: Some(usage),
                ..TextureViewDescriptor::default()
            })
        })
    }

    /// Writes `pixels` over the whole texture on `gpu`: RGBA, four bytes a
    /// pixel with straight alpha, row by row from the top left, sRGB-encoded
    /// as [`GpuTexture::FORMAT`] holds them, for an app that draws its
    /// pictures on the CPU (a decoded image, a page) and redraws one of the
    /// same size in place. The next frame draws them. Fails with
    /// [`ImageError::WrongLength`], writing nothing, when `pixels` are not
    /// exactly the texture's.
    pub fn write_rgba(&self, gpu: &Gpu, pixels: &[u8]) -> Result<(), ImageError> {
        let (width, height) = (self.width(), self.height());
        let expected = width as usize * height as usize * BYTES_PER_PIXEL;
        if pixels.len() != expected {
            return Err(ImageError::WrongLength(pixels.len()));
        }
        gpu.queue().write_texture(
            self.texture.as_image_copy(),
            pixels,
            TexelCopyBufferLayout {
                bytes_per_row: Some(width * BYTES_PER_PIXEL as u32),
                offset: 0,
                rows_per_image: Some(height),
            },
            self.texture.size(),
        );
        Ok(())
    }

    /// Its width in pixels.
    pub fn width(&self) -> u32 {
        self.texture.width()
    }

    /// Its height in pixels.
    pub fn height(&self) -> u32 {
        self.texture.height()
    }

    /// Whether it can be written as a storage texture, as a compute shader
    /// (Vello's) writes its output. Most adapters allow it; OpenGL ES may
    /// not, and an app falls back to rendering into it as a color
    /// attachment, or to [`Frame::draw_image`](crate::Frame::draw_image).
    pub fn is_storage(&self) -> bool {
        self.texture
            .usage()
            .contains(TextureUsages::STORAGE_BINDING)
    }

    /// The name the renderer's texture cache knows it by.
    pub(crate) fn id(&self) -> PictureId {
        self.id
    }
}

impl PartialEq for GpuTexture {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

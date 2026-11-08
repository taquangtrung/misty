//! Draws [`PictureDraw`]s, images, SVG documents, and app textures, as
//! textured instanced four-vertex strips, one texture each, kept in a
//! [`TextureCache`] across frames: an image is uploaded once, an SVG once
//! per size it is drawn at, and an app texture is only bound, since the app
//! renders into it.

use std::collections::HashMap;
use std::mem;
use std::num::NonZeroU64;
use std::ops::Range;

use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, BlendState,
    Buffer, BufferAddress, BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState,
    ColorWrites, Extent3d, FilterMode, FragmentState, MultisampleState, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    TexelCopyBufferLayout, Texture, TextureDescriptor, TextureDimension, TextureFormat,
    TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor, TextureViewDimension,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode, include_wgsl,
};

use super::super::base::color::Rgba;
use super::super::base::geometry::Rect;
use super::color_for_target;
use super::draw::{PictureDraw, PictureSource};
use super::gpu::GpuContext;
use super::image::{BYTES_PER_PIXEL, PictureId, raster_box};
use super::quad::float_bytes;
use super::texture::GpuTexture;

// ========================================================================
// Constants
// ========================================================================

/// The label GPU debuggers show for the picture pipeline and its resources.
const LABEL: &str = "misty pictures";

/// The floats each picture instance holds: a rect, a tint, a mode and an
/// opacity, and the part of the texture shown.
const FLOATS_PER_PICTURE: usize = 16;

/// The bytes of one picture instance.
const PICTURE_SIZE: BufferAddress = (FLOATS_PER_PICTURE * mem::size_of::<f32>()) as BufferAddress;

/// The bytes of the viewport uniform: a size and padding to 16 bytes.
const VIEWPORT_SIZE: BufferAddress = 4 * mem::size_of::<f32>() as BufferAddress;

/// The vertices of one picture's strip.
const VERTICES_PER_PICTURE: Range<u32> = 0..4;

/// How many pictures the instance buffer holds before it first grows.
const INITIAL_CAPACITY: usize = 16;

/// How many frames a texture is kept without being drawn.
const EVICT_AFTER_FRAMES: u64 = 120;

/// The most texture bytes kept; past it the least recently drawn go first.
const BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// The mode of an instance drawn in its own colors.
const OWN_COLORS: f32 = 0.0;

/// The mode of an instance drawn in its tint.
const TINTED: f32 = 1.0;

/// The mode of an instance drawn in its own colors, decoded from sRGB by
/// the shader: an app texture drawn to an sRGB target, sampled through a
/// view of its own format, since not every backend (OpenGL) can view it
/// as sRGB.
const DECODED: f32 = 2.0;

/// The quarter turns that make a whole turn.
const QUARTER_TURNS: u8 = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// The pipeline that draws pictures, the buffers it reads, and the
/// textures of the pictures drawn lately.
pub(crate) struct PicturePipeline {
    /// The textures of pictures drawn lately.
    cache: TextureCache<GpuPicture>,
    /// How many pictures `instances` has room for.
    capacity: usize,
    /// The texture of each uploaded instance, in order.
    draws: Vec<(TextureKey, bool)>,
    /// The pictures of the frame being drawn.
    instances: Buffer,
    /// The instances of each layer.
    layers: Vec<Range<u32>>,
    /// The pipeline itself.
    pipeline: RenderPipeline,
    /// Samples every picture, linearly, clamped at its edges.
    sampler: Sampler,
    /// Samples a picture drawn with sharp pixels: the nearest texel.
    sampler_nearest: Sampler,
    /// Whether the target's format is sRGB, which wants linear colors and
    /// sRGB textures.
    srgb: bool,
    /// How a picture's texture and the sampler are bound.
    texture_layout: BindGroupLayout,
    /// The target size, for converting pixels to clip space.
    viewport: Buffer,
    /// Binds the viewport uniform.
    viewport_group: BindGroup,
}

/// One picture's texture on the GPU.
struct GpuPicture {
    /// Binds the texture and the sampler.
    bind_group: BindGroup,
    /// Binds the texture and the nearest sampler.
    bind_group_nearest: BindGroup,
    /// The texture, kept alive while it is bound.
    _texture: Texture,
}

/// Textures by what they show, kept while they are drawn now and then
/// and within a byte budget.
struct TextureCache<V> {
    /// The most bytes kept, unless the frame being drawn needs more.
    budget: usize,
    /// The textures.
    entries: HashMap<TextureKey, CacheEntry<V>>,
    /// The number of the frame being drawn.
    frame: u64,
    /// The bytes all entries take.
    total: usize,
}

/// One cached texture.
struct CacheEntry<V> {
    /// The bytes it takes.
    bytes: usize,
    /// The last frame it was drawn in.
    last_used: u64,
    /// The texture.
    value: V,
}

/// What a texture shows.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TextureKey {
    /// An image.
    Image(PictureId),
    /// An SVG document rasterized at one size.
    Svg(SvgKey),
    /// A texture the app renders into.
    Texture(PictureId),
}

/// An SVG document rasterized at one size.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SvgKey {
    /// The raster's height in pixels.
    height: u32,
    /// The document.
    id: PictureId,
    /// The raster's width in pixels.
    width: u32,
}

/// A picture's pixels ready to upload.
struct Pixels<'a> {
    /// RGBA, straight alpha, row by row.
    data: &'a [u8],
    /// The height in pixels.
    height: u32,
    /// The width in pixels.
    width: u32,
}

// ========================================================================
// PicturePipeline Implementations
// ========================================================================

impl PicturePipeline {
    /// A pipeline drawing to textures of `format`.
    pub(crate) fn new(gpu: &GpuContext, format: TextureFormat) -> Self {
        let device = &gpu.device;
        let viewport_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                count: None,
                ty: BindingType::Buffer {
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(VIEWPORT_SIZE),
                    ty: BufferBindingType::Uniform,
                },
                visibility: ShaderStages::VERTEX,
            }],
            label: Some(LABEL),
        });
        let texture_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    count: None,
                    ty: BindingType::Texture {
                        multisampled: false,
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                    },
                    visibility: ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    count: None,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    visibility: ShaderStages::FRAGMENT,
                },
            ],
            label: Some(LABEL),
        });
        let viewport = device.create_buffer(&BufferDescriptor {
            label: Some(LABEL),
            mapped_at_creation: false,
            size: VIEWPORT_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        let viewport_group = device.create_bind_group(&BindGroupDescriptor {
            entries: &[BindGroupEntry {
                binding: 0,
                resource: viewport.as_entire_binding(),
            }],
            label: Some(LABEL),
            layout: &viewport_layout,
        });
        let sampler = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            label: Some(LABEL),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..SamplerDescriptor::default()
        });
        let sampler_nearest = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            label: Some(LABEL),
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Linear,
            ..SamplerDescriptor::default()
        });
        Self {
            cache: TextureCache::new(BUDGET_BYTES),
            capacity: INITIAL_CAPACITY,
            draws: Vec::new(),
            instances: create_instance_buffer(gpu, INITIAL_CAPACITY),
            layers: Vec::new(),
            pipeline: create_pipeline(gpu, format, &viewport_layout, &texture_layout),
            sampler,
            sampler_nearest,
            srgb: format.is_srgb(),
            texture_layout,
            viewport,
            viewport_group,
        }
    }

    /// Uploads the pictures of `layers` for a target `width` by `height`
    /// pixels, making the textures not cached yet. A picture whose texture
    /// cannot be made (larger than the GPU allows) is reported and left
    /// out.
    pub(crate) fn prepare(
        &mut self,
        gpu: &GpuContext,
        (width, height): (u32, u32),
        layers: &[&[PictureDraw]],
    ) {
        let viewport = [width as f32, height as f32, 0.0, 0.0];
        gpu.queue
            .write_buffer(&self.viewport, 0, &float_bytes(&viewport));
        self.draws.clear();
        self.layers.clear();
        let mut floats = Vec::new();
        for layer in layers {
            let start = instance_index(self.draws.len());
            for draw in layer.iter() {
                let (key, rect) = match &draw.source {
                    PictureSource::Image(image) => (TextureKey::Image(image.id()), draw.rect),
                    PictureSource::Texture(texture) => {
                        (TextureKey::Texture(texture.id()), draw.rect)
                    }
                    PictureSource::Svg(svg) => {
                        let raster = raster_box(draw.rect);
                        let key = TextureKey::Svg(SvgKey {
                            height: raster.height as u32,
                            id: svg.id(),
                            width: raster.width as u32,
                        });
                        (key, raster)
                    }
                };
                if !self.ensure_texture(gpu, key, &draw.source) {
                    continue;
                }
                self.draws.push((key, draw.style.nearest));
                let shown = draw.crop.unwrap_or(rect);
                floats.extend([shown.x, shown.y, shown.width, shown.height]);
                let decoded = self.srgb && matches!(draw.source, PictureSource::Texture(_));
                let (tint, mode) = match (draw.tint, decoded) {
                    (Some(color), _) => (color_for_target(Rgba::from(color), self.srgb), TINTED),
                    (None, true) => ([0.0; 4], DECODED),
                    (None, false) => ([0.0; 4], OWN_COLORS),
                };
                floats.extend(tint);
                let turns = f32::from(draw.style.quarter_turns % QUARTER_TURNS);
                let flipped = f32::from(u8::from(draw.style.flipped));
                floats.extend([mode, draw.opacity, turns, flipped]);
                floats.extend(texture_part(rect, shown));
            }
            self.layers.push(start..instance_index(self.draws.len()));
        }
        if self.draws.len() > self.capacity {
            self.capacity = self.draws.len().next_power_of_two();
            self.instances = create_instance_buffer(gpu, self.capacity);
        }
        if !floats.is_empty() {
            gpu.queue
                .write_buffer(&self.instances, 0, &float_bytes(&floats));
        }
    }

    /// Draws the uploaded pictures of layer `layer` into `pass`.
    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, layer: usize) {
        let Some(range) = self.layers.get(layer).filter(|range| !range.is_empty()) else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.viewport_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        for index in range.clone() {
            let (key, nearest) = self.draws[index as usize];
            let Some(picture) = self.cache.get(key) else {
                continue;
            };
            let group = match nearest {
                true => &picture.bind_group_nearest,
                false => &picture.bind_group,
            };
            pass.set_bind_group(1, group, &[]);
            pass.draw(VERTICES_PER_PICTURE, index..index + 1);
        }
    }

    /// Ends the frame: textures not drawn lately, or past the budget, are
    /// let go.
    pub(crate) fn finish_frame(&mut self) {
        self.cache.end_frame();
    }

    /// Makes sure the texture of `key`, showing `source`, is cached and
    /// marked drawn this frame; whether it is.
    fn ensure_texture(
        &mut self,
        gpu: &GpuContext,
        key: TextureKey,
        source: &PictureSource,
    ) -> bool {
        if self.cache.touch(key) {
            return true;
        }
        if let PictureSource::Texture(texture) = source {
            // The app's memory, not the cache's: it counts for nothing
            // against the budget, and letting it go drops only the binding.
            let picture = self.bind_texture(gpu, texture);
            self.cache.insert(key, 0, picture);
            return true;
        }
        let raster;
        let pixels = match (source, key) {
            (PictureSource::Image(image), _) => Pixels {
                data: image.pixels(),
                height: image.height(),
                width: image.width(),
            },
            (PictureSource::Svg(svg), TextureKey::Svg(size)) => {
                let Some(rasterized) = svg.rasterize(size.width, size.height) else {
                    return false;
                };
                raster = rasterized;
                Pixels {
                    data: &raster.pixels,
                    height: raster.height,
                    width: raster.width,
                }
            }
            (PictureSource::Svg(_), TextureKey::Image(_) | TextureKey::Texture(_))
            | (PictureSource::Texture(_), _) => return false,
        };
        let max_side = gpu.device.limits().max_texture_dimension_2d;
        if pixels.width > max_side || pixels.height > max_side {
            log::warn!(
                "a {}x{} picture is larger than the GPU's {max_side} pixel textures; not drawn",
                pixels.width,
                pixels.height
            );
            return false;
        }
        let bytes = pixels.data.len();
        let picture = self.upload(gpu, &pixels);
        self.cache.insert(key, bytes, picture);
        true
    }

    /// A texture holding `pixels`, bound with the sampler.
    fn upload(&self, gpu: &GpuContext, pixels: &Pixels<'_>) -> GpuPicture {
        let size = Extent3d {
            depth_or_array_layers: 1,
            height: pixels.height,
            width: pixels.width,
        };
        // An sRGB texture decodes to linear as it is sampled, as an sRGB
        // target wants; any other target takes the bytes as they are.
        let format = match self.srgb {
            true => TextureFormat::Rgba8UnormSrgb,
            false => TextureFormat::Rgba8Unorm,
        };
        let texture = gpu.device.create_texture(&TextureDescriptor {
            dimension: TextureDimension::D2,
            format,
            label: Some(LABEL),
            mip_level_count: 1,
            sample_count: 1,
            size,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            pixels.data,
            TexelCopyBufferLayout {
                bytes_per_row: Some(pixels.width * BYTES_PER_PIXEL as u32),
                offset: 0,
                rows_per_image: Some(pixels.height),
            },
            size,
        );
        let view = texture.create_view(&TextureViewDescriptor::default());
        self.bind(gpu, texture, &view)
    }

    /// The app's `texture`, bound with the sampler through a view of its
    /// own format: an sRGB target has the shader decode it
    /// ([`DECODED`]).
    fn bind_texture(&self, gpu: &GpuContext, texture: &GpuTexture) -> GpuPicture {
        // Only sampled, whatever else the texture is for.
        let view = texture.texture().create_view(&TextureViewDescriptor {
            usage: Some(TextureUsages::TEXTURE_BINDING),
            ..TextureViewDescriptor::default()
        });
        self.bind(gpu, texture.texture().clone(), &view)
    }

    /// `texture`, seen through `view`, bound with each sampler.
    fn bind(&self, gpu: &GpuContext, texture: Texture, view: &TextureView) -> GpuPicture {
        let bind = |sampler: &Sampler| {
            gpu.device.create_bind_group(&BindGroupDescriptor {
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(sampler),
                    },
                ],
                label: Some(LABEL),
                layout: &self.texture_layout,
            })
        };
        GpuPicture {
            bind_group: bind(&self.sampler),
            bind_group_nearest: bind(&self.sampler_nearest),
            _texture: texture,
        }
    }
}

// ========================================================================
// TextureCache Implementations
// ========================================================================

impl<V> TextureCache<V> {
    /// An empty cache keeping at most `budget` bytes.
    fn new(budget: usize) -> Self {
        Self {
            budget,
            entries: HashMap::new(),
            frame: 0,
            total: 0,
        }
    }

    /// The texture of `key`, if cached.
    fn get(&self, key: TextureKey) -> Option<&V> {
        self.entries.get(&key).map(|entry| &entry.value)
    }

    /// Marks the texture of `key` drawn this frame; whether it is cached.
    fn touch(&mut self, key: TextureKey) -> bool {
        let frame = self.frame;
        self.entries
            .get_mut(&key)
            .map(|entry| entry.last_used = frame)
            .is_some()
    }

    /// Caches `value`, `bytes` large, as the texture of `key`, drawn this
    /// frame.
    fn insert(&mut self, key: TextureKey, bytes: usize, value: V) {
        let entry = CacheEntry {
            bytes,
            last_used: self.frame,
            value,
        };
        self.total += bytes;
        if let Some(replaced) = self.entries.insert(key, entry) {
            self.total -= replaced.bytes;
        }
    }

    /// Ends a frame: drops the textures not drawn for
    /// [`EVICT_AFTER_FRAMES`] frames, then, while over the budget, the
    /// least recently drawn of those not drawn this frame.
    fn end_frame(&mut self) {
        let frame = self.frame;
        let stale: Vec<TextureKey> = self
            .entries
            .iter()
            .filter(|(_, entry)| frame - entry.last_used > EVICT_AFTER_FRAMES)
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            self.remove(key);
        }
        while self.total > self.budget {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.last_used < frame)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key);
            let Some(key) = oldest else {
                break;
            };
            self.remove(key);
        }
        self.frame += 1;
    }

    /// Drops the texture of `key`.
    fn remove(&mut self, key: TextureKey) {
        if let Some(entry) = self.entries.remove(&key) {
            self.total -= entry.bytes;
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The part of a texture stretched over `rect` that falls in `shown`, a
/// part of `rect`: its left, top, width, and height as fractions of the
/// texture.
fn texture_part(rect: Rect, shown: Rect) -> [f32; 4] {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    [
        (shown.x - rect.x) / rect.width,
        (shown.y - rect.y) / rect.height,
        shown.width / rect.width,
        shown.height / rect.height,
    ]
}

/// `index` as an instance index. No frame comes near `u32::MAX` pictures.
fn instance_index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// An instance buffer with room for `capacity` pictures.
fn create_instance_buffer(gpu: &GpuContext, capacity: usize) -> Buffer {
    gpu.device.create_buffer(&BufferDescriptor {
        label: Some(LABEL),
        mapped_at_creation: false,
        size: capacity as BufferAddress * PICTURE_SIZE,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
    })
}

/// The picture pipeline for targets of `format`, reading the viewport
/// through `viewport_layout` and each texture through `texture_layout`.
fn create_pipeline(
    gpu: &GpuContext,
    format: TextureFormat,
    viewport_layout: &BindGroupLayout,
    texture_layout: &BindGroupLayout,
) -> RenderPipeline {
    let device = &gpu.device;
    let shader = device.create_shader_module(include_wgsl!("picture.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        bind_group_layouts: &[Some(viewport_layout), Some(texture_layout)],
        label: Some(LABEL),
        ..PipelineLayoutDescriptor::default()
    });
    let float4 = mem::size_of::<[f32; 4]>() as BufferAddress;
    let attributes = [0, 1, 2, 3].map(|location| VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: float4 * BufferAddress::from(location),
        shader_location: location,
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        cache: None,
        depth_stencil: None,
        fragment: Some(FragmentState {
            compilation_options: PipelineCompilationOptions::default(),
            entry_point: Some("fs_main"),
            module: &shader,
            targets: &[Some(ColorTargetState {
                blend: Some(BlendState::ALPHA_BLENDING),
                format,
                write_mask: ColorWrites::ALL,
            })],
        }),
        label: Some(LABEL),
        layout: Some(&pipeline_layout),
        multisample: MultisampleState::default(),
        multiview_mask: None,
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleStrip,
            ..PrimitiveState::default()
        },
        vertex: VertexState {
            buffers: &[Some(VertexBufferLayout {
                array_stride: PICTURE_SIZE,
                attributes: &attributes,
                step_mode: VertexStepMode::Instance,
            })],
            compilation_options: PipelineCompilationOptions::default(),
            entry_point: Some("vs_main"),
            module: &shader,
        },
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::image::{Image, ImageError, Svg};
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgb;
    use crate::base::geometry::Rect;
    use crate::base::theme::Theme;

    /// A 10 unit square in the middle of a 20 unit view.
    const SQUARE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
        <rect x="5" y="5" width="10" height="10" fill="#000000"/></svg>"##;

    fn key() -> TextureKey {
        let image = Image::from_rgba(1, 1, vec![0; BYTES_PER_PIXEL]).expect("an image");
        TextureKey::Image(image.id())
    }

    #[test]
    fn test_a_texture_not_drawn_for_a_while_is_let_go() {
        let mut cache = TextureCache::new(usize::MAX);
        let (kept, dropped) = (key(), key());
        cache.insert(kept, 4, ());
        cache.insert(dropped, 4, ());
        for _ in 0..=EVICT_AFTER_FRAMES {
            assert!(cache.touch(kept));
            cache.end_frame();
        }
        assert!(cache.touch(dropped), "drawn within the window, kept");
        for _ in 0..=EVICT_AFTER_FRAMES + 1 {
            cache.touch(kept);
            cache.end_frame();
        }
        assert!(cache.get(kept).is_some());
        assert!(cache.get(dropped).is_none());
        assert_eq!(cache.total, 4);
    }

    #[test]
    fn test_over_budget_the_least_recently_drawn_go_but_never_this_frames() {
        let mut cache = TextureCache::new(10);
        let (oldest, older, current) = (key(), key(), key());
        cache.insert(oldest, 6, ());
        cache.end_frame();
        cache.insert(older, 6, ());
        cache.end_frame();
        // 18 bytes over a budget of 10: the oldest goes, then the older.
        cache.insert(current, 6, ());
        cache.end_frame();
        assert!(cache.get(oldest).is_none());
        assert!(cache.get(older).is_none());
        assert!(cache.get(current).is_some());
        // A frame needing more than the budget keeps all it drew.
        let (a, b) = (key(), key());
        cache.insert(a, 20, ());
        cache.insert(b, 20, ());
        cache.touch(current);
        cache.end_frame();
        assert!(cache.get(a).is_some() && cache.get(b).is_some());
    }

    #[test]
    fn test_replacing_a_texture_counts_its_bytes_once() {
        let mut cache = TextureCache::new(usize::MAX);
        let same = key();
        cache.insert(same, 8, ());
        cache.insert(same, 12, ());
        assert_eq!(cache.total, 12);
    }

    #[test]
    fn test_an_image_stretches_over_its_rect_and_an_icon_takes_its_tint() {
        let theme = Theme::dark();
        let (red, green, blue, white) = (
            Rgb::new(255, 0, 0),
            Rgb::new(0, 255, 0),
            Rgb::new(0, 0, 255),
            Rgb::new(255, 255, 255),
        );
        let pixels = [red, green, blue, white]
            .iter()
            .flat_map(|color| [color.r, color.g, color.b, u8::MAX])
            .collect();
        let image = Image::from_rgba(2, 2, pixels).expect("an image");
        let icon = Svg::parse(SQUARE_SVG.as_bytes()).expect("an svg");
        let Some(snapshot) = offscreen::render(100, 50, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.draw_image(&image, Rect::new(0.0, 0.0, 40.0, 40.0));
            frame.draw_icon(&icon, Rect::new(50.0, 0.0, 40.0, 40.0), theme.accent);
        }) else {
            return;
        };
        snapshot.assert_pixel(5.0, 5.0, red, "the top left texel");
        snapshot.assert_pixel(35.0, 5.0, green, "the top right texel");
        snapshot.assert_pixel(5.0, 35.0, blue, "the bottom left texel");
        snapshot.assert_pixel(35.0, 35.0, white, "the bottom right texel");
        snapshot.assert_pixel(45.0, 20.0, theme.background, "between them");
        snapshot.assert_pixel(70.0, 20.0, theme.accent, "the icon, tinted");
        snapshot.assert_pixel(52.0, 2.0, theme.background, "outside its shape");
    }

    #[test]
    fn test_an_app_texture_draws_what_was_rendered_into_it_and_clips_like_an_image() {
        let Some(gpu) = offscreen::gpu() else {
            return;
        };
        let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        let texture = GpuTexture::new(&gpu, 2, 1).expect("a texture");
        let pixels: Vec<u8> = [red, blue]
            .iter()
            .flat_map(|color| [color.r, color.g, color.b, u8::MAX])
            .collect();
        gpu.queue().write_texture(
            texture.texture().as_image_copy(),
            &pixels,
            TexelCopyBufferLayout {
                bytes_per_row: Some(2 * BYTES_PER_PIXEL as u32),
                offset: 0,
                rows_per_image: Some(1),
            },
            texture.texture().size(),
        );
        let theme = Theme::dark();
        let Some(snapshot) = offscreen::render(100, 50, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.draw_texture(&texture, Rect::new(0.0, 0.0, 40.0, 40.0));
            frame.clipped(Rect::new(70.0, 0.0, 20.0, 40.0), |frame| {
                frame.draw_texture(&texture, Rect::new(50.0, 0.0, 40.0, 40.0));
            });
        }) else {
            return;
        };
        snapshot.assert_pixel(5.0, 20.0, red, "the left texel");
        snapshot.assert_pixel(35.0, 20.0, blue, "the right texel");
        snapshot.assert_pixel(60.0, 20.0, theme.background, "clipped away");
        snapshot.assert_pixel(85.0, 20.0, blue, "the part the clip shows");
        assert!(matches!(
            GpuTexture::new(&gpu, 0, 4),
            Err(ImageError::Empty)
        ));
    }

    #[test]
    fn test_what_is_written_through_the_srgb_view_draws_encoded() {
        let Some(gpu) = offscreen::gpu() else {
            return;
        };
        let texture = GpuTexture::new(&gpu, 4, 4).expect("a texture");
        let Some(view) = texture.srgb_view() else {
            return;
        };
        // Linear 0.2158 is sRGB 128: a mid grey once encoded.
        let linear = 0.215_860_5;
        let mut encoder = gpu.device().create_command_encoder(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        a: 1.0,
                        b: linear,
                        g: linear,
                        r: linear,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                resolve_target: None,
                view: &view,
            })],
            ..Default::default()
        });
        gpu.queue().submit([encoder.finish()]);
        let Some(snapshot) = offscreen::render(10, 10, |frame| {
            frame.draw_texture(&texture, Rect::new(0.0, 0.0, 10.0, 10.0));
        }) else {
            return;
        };
        snapshot.assert_pixel(5.0, 5.0, Rgb::new(128, 128, 128), "the encoded grey");
    }

    #[test]
    fn test_with_opacity_fades_images_and_boxes_alike_over_the_background() {
        let white = Rgb::new(255, 255, 255);
        let pixels = [white; 4]
            .iter()
            .flat_map(|color| [color.r, color.g, color.b, u8::MAX])
            .collect();
        let image = Image::from_rgba(2, 2, pixels).expect("an image");
        let Some(snapshot) = offscreen::render(100, 50, |frame| {
            frame.clear(Rgba::BLACK);
            frame.with_opacity(0.5, |frame| {
                frame.draw_image(&image, Rect::new(0.0, 0.0, 40.0, 40.0));
                frame.fill_rect(Rect::new(50.0, 0.0, 40.0, 40.0), white);
            });
        }) else {
            return;
        };
        let image_pixel = snapshot.pixel(20.0, 20.0);
        let box_pixel = snapshot.pixel(70.0, 20.0);
        assert!(
            image_pixel.r > 0 && image_pixel.r < u8::MAX,
            "half way to white: {image_pixel:?}"
        );
        assert!(
            image_pixel.r.abs_diff(box_pixel.r) <= 1,
            "faded the same way, but for rounding: {image_pixel:?} {box_pixel:?}"
        );
    }

    #[test]
    fn test_a_clipped_image_shows_only_its_part_inside_the_clip_unsqueezed() {
        let theme = Theme::dark();
        let (red, green) = (Rgb::new(255, 0, 0), Rgb::new(0, 255, 0));
        let pixels = [red, green, red, green]
            .iter()
            .flat_map(|color| [color.r, color.g, color.b, u8::MAX])
            .collect();
        let image = Image::from_rgba(2, 2, pixels).expect("an image");
        let Some(snapshot) = offscreen::render(100, 50, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.clipped(Rect::new(30.0, 0.0, 100.0, 50.0), |frame| {
                frame.draw_image(&image, Rect::new(0.0, 0.0, 40.0, 40.0));
            });
        }) else {
            return;
        };
        snapshot.assert_pixel(10.0, 20.0, theme.background, "left of the clip");
        snapshot.assert_pixel(35.0, 20.0, green, "the right texel, where it was");
        snapshot.assert_pixel(45.0, 20.0, theme.background, "past the image");
    }
}

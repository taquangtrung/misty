//! Draws [`PathDraw`]s, vector paths tessellated into antialiased
//! triangles, in one color or a linear gradient. Every gradient of a frame
//! is a row of one ramp texture, so any number of them share the one
//! pipeline. Paths are drawn in runs between the fills of their layer, so
//! a path keeps its place in paint order among boxes.

use std::mem;
use std::num::NonZeroU64;
use std::ops::Range;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendState, Buffer, BufferAddress,
    BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, Extent3d,
    FragmentState, IndexFormat, MultisampleState, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, ShaderStages, TexelCopyBufferLayout, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureViewDescriptor,
    TextureViewDimension, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode, include_wgsl,
};

use super::super::base::color::Rgba;
use super::super::base::geometry::Rect;
use super::super::base::paint::Paint;
use super::draw::PathDraw;
use super::gpu::GpuContext;
use super::quad::float_bytes;
use super::{LAYERS, color_for_target};

// ========================================================================
// Constants
// ========================================================================

/// The label GPU debuggers show for the vector pipeline and its resources.
const LABEL: &str = "misty vectors";

/// The floats each vertex holds: position, coverage, and gradient offset;
/// color; clip; and ramp row.
const FLOATS_PER_VERTEX: usize = 13;

/// The bytes of one vertex.
const VERTEX_SIZE: BufferAddress = (FLOATS_PER_VERTEX * mem::size_of::<f32>()) as BufferAddress;

/// The bytes of one index.
const INDEX_SIZE: BufferAddress = mem::size_of::<u32>() as BufferAddress;

/// The bytes of the viewport uniform: a size and padding to 16 bytes.
const VIEWPORT_SIZE: BufferAddress = 4 * mem::size_of::<f32>() as BufferAddress;

/// How many vertices and indices the buffers hold before they first grow.
const INITIAL_CAPACITY: usize = 1024;

/// The texels of one gradient's row in the ramp texture: enough that its
/// steps never show.
const RAMP_TEXELS: u32 = 256;

/// The format of the ramp texture: floats, already in the target's color
/// space, read texel by texel.
const RAMP_FORMAT: TextureFormat = TextureFormat::Rgba32Float;

/// The floats of one ramp texel.
const FLOATS_PER_TEXEL: u32 = 4;

/// The ramp row a one-color vertex names: none.
const NO_RAMP: f32 = -1.0;

/// What a gradient's color is multiplied by before its opacity: nothing.
const WHITE: [f32; 3] = [1.0; 3];

// ========================================================================
// Data Structures
// ========================================================================

/// The pipeline that draws paths, and the buffers and ramps it reads.
pub(crate) struct VectorPipeline {
    /// Binds the viewport uniform and the ramp texture.
    bind_group: BindGroup,
    /// How many indices `indices` has room for.
    index_capacity: usize,
    /// The triangles of the frame being drawn.
    indices: Buffer,
    /// The layout `bind_group` follows, for rebinding a grown ramp.
    layout: BindGroupLayout,
    /// The pipeline itself.
    pipeline: RenderPipeline,
    /// How many gradient rows `ramps` has room for.
    ramp_capacity: u32,
    /// The gradients of the frame being drawn, a row each.
    ramps: Texture,
    /// The runs of each layer, in paint order.
    runs: [Vec<VectorRun>; LAYERS],
    /// How many vertices `vertices` has room for.
    vertex_capacity: usize,
    /// The vertices of the frame being drawn.
    vertices: Buffer,
    /// The target size, for converting pixels to clip space.
    viewport: Buffer,
}

/// Paths of one layer drawn together, between the same fills.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VectorRun {
    /// How many of the layer's fills are drawn before the run.
    pub(crate) fills_before: usize,
    /// The run's triangles, as a range of the index buffer.
    pub(crate) indices: Range<u32>,
}

/// What a frame's paths upload: their vertices, triangles, and ramps.
#[derive(Debug, Default)]
struct Upload {
    /// Three indices per triangle, into all the frame's vertices.
    indices: Vec<u32>,
    /// The ramp texels, row after row.
    ramps: Vec<f32>,
    /// How many ramp rows `ramps` holds.
    ramp_rows: u32,
    /// The runs of each layer.
    runs: [Vec<VectorRun>; LAYERS],
    /// The vertices' floats.
    vertices: Vec<f32>,
}

// ========================================================================
// VectorPipeline Implementations
// ========================================================================

impl VectorPipeline {
    /// A pipeline drawing to textures of `format`.
    pub(crate) fn new(gpu: &GpuContext, format: TextureFormat) -> Self {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    count: None,
                    ty: BindingType::Buffer {
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(VIEWPORT_SIZE),
                        ty: BufferBindingType::Uniform,
                    },
                    visibility: ShaderStages::VERTEX,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    count: None,
                    ty: BindingType::Texture {
                        multisampled: false,
                        sample_type: TextureSampleType::Float { filterable: false },
                        view_dimension: TextureViewDimension::D2,
                    },
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
        let ramps = create_ramps(gpu, 1);
        let bind_group = create_bind_group(gpu, &layout, &viewport, &ramps);
        Self {
            bind_group,
            index_capacity: INITIAL_CAPACITY,
            indices: create_buffer(gpu, INITIAL_CAPACITY, INDEX_SIZE, BufferUsages::INDEX),
            pipeline: create_pipeline(gpu, format, &layout),
            layout,
            ramp_capacity: 1,
            ramps,
            runs: Default::default(),
            vertex_capacity: INITIAL_CAPACITY,
            vertices: create_buffer(gpu, INITIAL_CAPACITY, VERTEX_SIZE, BufferUsages::VERTEX),
            viewport,
        }
    }

    /// Uploads the paths of `layers` for a target `width` by `height`
    /// pixels whose format is sRGB when `srgb`, growing the buffers and the
    /// ramp texture when they do not fit.
    pub(crate) fn prepare(
        &mut self,
        gpu: &GpuContext,
        (width, height): (u32, u32),
        srgb: bool,
        layers: [&[PathDraw]; LAYERS],
    ) {
        let viewport = [width as f32, height as f32, 0.0, 0.0];
        gpu.queue
            .write_buffer(&self.viewport, 0, &float_bytes(&viewport));
        let whole = Rect::new(0.0, 0.0, width as f32, height as f32);
        let max_rows = gpu.device.limits().max_texture_dimension_2d;
        let upload = Upload::build(layers, whole, srgb, max_rows);
        self.runs = upload.runs;
        let vertex_count = upload.vertices.len() / FLOATS_PER_VERTEX;
        if vertex_count > self.vertex_capacity {
            self.vertex_capacity = vertex_count.next_power_of_two();
            self.vertices =
                create_buffer(gpu, self.vertex_capacity, VERTEX_SIZE, BufferUsages::VERTEX);
        }
        if upload.indices.len() > self.index_capacity {
            self.index_capacity = upload.indices.len().next_power_of_two();
            self.indices = create_buffer(gpu, self.index_capacity, INDEX_SIZE, BufferUsages::INDEX);
        }
        if upload.ramp_rows > self.ramp_capacity {
            self.ramp_capacity = upload.ramp_rows.next_power_of_two().min(max_rows);
            self.ramps = create_ramps(gpu, self.ramp_capacity);
            self.bind_group = create_bind_group(gpu, &self.layout, &self.viewport, &self.ramps);
        }
        if !upload.vertices.is_empty() {
            gpu.queue
                .write_buffer(&self.vertices, 0, &float_bytes(&upload.vertices));
            let indices: Vec<u8> = upload
                .indices
                .iter()
                .flat_map(|index| index.to_ne_bytes())
                .collect();
            gpu.queue.write_buffer(&self.indices, 0, &indices);
        }
        if upload.ramp_rows > 0 {
            gpu.queue.write_texture(
                self.ramps.as_image_copy(),
                &float_bytes(&upload.ramps),
                TexelCopyBufferLayout {
                    bytes_per_row: Some(RAMP_TEXELS * FLOATS_PER_TEXEL * f32_size()),
                    offset: 0,
                    rows_per_image: Some(upload.ramp_rows),
                },
                ramp_extent(upload.ramp_rows),
            );
        }
    }

    /// The runs of layer `layer` uploaded last, in paint order.
    pub(crate) fn runs(&self, layer: usize) -> &[VectorRun] {
        &self.runs[layer]
    }

    /// Draws the uploaded triangles at index positions `indices` into
    /// `pass`.
    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, indices: Range<u32>) {
        if indices.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), IndexFormat::Uint32);
        pass.draw_indexed(indices, 0, 0..1);
    }
}

// ========================================================================
// Upload Implementations
// ========================================================================

impl Upload {
    /// What the paths of `layers` upload for a target covering `whole`
    /// whose format is sRGB when `srgb`, with at most `max_rows` gradients
    /// (any past them are drawn in their first stop's color).
    fn build(layers: [&[PathDraw]; LAYERS], whole: Rect, srgb: bool, max_rows: u32) -> Self {
        let mut upload = Self::default();
        for (layer, paths) in layers.iter().enumerate() {
            for path in paths.iter() {
                let start = index_count(upload.indices.len());
                upload.add(path, whole, srgb, max_rows);
                let end = index_count(upload.indices.len());
                let runs = &mut upload.runs[layer];
                match runs.last_mut() {
                    Some(run) if run.fills_before == path.fills_before => run.indices.end = end,
                    Some(_) | None => runs.push(VectorRun {
                        fills_before: path.fills_before,
                        indices: start..end,
                    }),
                }
            }
        }
        upload
    }

    /// Adds `path`'s vertices, triangles, and gradient row.
    fn add(&mut self, path: &PathDraw, whole: Rect, srgb: bool, max_rows: u32) {
        let (color, ramp) = match &path.paint {
            Paint::LinearGradient(gradient) if self.ramp_rows < max_rows => {
                let row = self.ramp_rows as f32;
                self.ramp_rows += 1;
                let last = (RAMP_TEXELS - 1) as f32;
                for texel in 0..RAMP_TEXELS {
                    let color = gradient.color_at(texel as f32 / last);
                    self.ramps.extend(color_for_target(color, srgb));
                }
                let [r, g, b] = WHITE;
                ([r, g, b, path.opacity], row)
            }
            Paint::LinearGradient(gradient) => {
                let first = gradient
                    .stops
                    .first()
                    .map_or(Rgba::new(0, 0, 0, 0), |stop| stop.color);
                (with_opacity(first, path.opacity, srgb), NO_RAMP)
            }
            Paint::Solid(color) => (with_opacity(*color, path.opacity, srgb), NO_RAMP),
        };
        let clip = path.clip.unwrap_or(whole);
        let base = index_count(self.vertices.len() / FLOATS_PER_VERTEX);
        for vertex in &path.vertices {
            self.vertices.extend([
                vertex.position.x,
                vertex.position.y,
                vertex.coverage,
                vertex.gradient,
            ]);
            self.vertices.extend(color);
            self.vertices
                .extend([clip.x, clip.y, clip.width, clip.height]);
            self.vertices.push(ramp);
        }
        self.indices
            .extend(path.indices.iter().map(|index| base + index));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `color` in the target's channels with its alpha scaled by `opacity`.
fn with_opacity(color: Rgba, opacity: f32, srgb: bool) -> [f32; 4] {
    let [r, g, b, a] = color_for_target(color, srgb);
    [r, g, b, a * opacity]
}

/// `count` as an index. No frame comes near `u32::MAX` vertices or
/// indices; one that did is cut short rather than wrapped.
fn index_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The bytes of one float.
fn f32_size() -> u32 {
    mem::size_of::<f32>() as u32
}

/// The size of a ramp texture of `rows` gradients.
fn ramp_extent(rows: u32) -> Extent3d {
    Extent3d {
        depth_or_array_layers: 1,
        height: rows,
        width: RAMP_TEXELS,
    }
}

/// A ramp texture with room for `rows` gradients.
fn create_ramps(gpu: &GpuContext, rows: u32) -> Texture {
    gpu.device.create_texture(&TextureDescriptor {
        dimension: TextureDimension::D2,
        format: RAMP_FORMAT,
        label: Some(LABEL),
        mip_level_count: 1,
        sample_count: 1,
        size: ramp_extent(rows),
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// A buffer with room for `capacity` items of `size` bytes, used as
/// `usage`.
fn create_buffer(
    gpu: &GpuContext,
    capacity: usize,
    size: BufferAddress,
    usage: BufferUsages,
) -> Buffer {
    gpu.device.create_buffer(&BufferDescriptor {
        label: Some(LABEL),
        mapped_at_creation: false,
        size: capacity as BufferAddress * size,
        usage: usage | BufferUsages::COPY_DST,
    })
}

/// The bind group of `viewport` and `ramps`, following `layout`.
fn create_bind_group(
    gpu: &GpuContext,
    layout: &BindGroupLayout,
    viewport: &Buffer,
    ramps: &Texture,
) -> BindGroup {
    let view = ramps.create_view(&TextureViewDescriptor::default());
    gpu.device.create_bind_group(&BindGroupDescriptor {
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: viewport.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(&view),
            },
        ],
        label: Some(LABEL),
        layout,
    })
}

/// The vector pipeline for targets of `format`, reading its bindings
/// through `layout`.
fn create_pipeline(
    gpu: &GpuContext,
    format: TextureFormat,
    layout: &BindGroupLayout,
) -> RenderPipeline {
    let device = &gpu.device;
    let shader = device.create_shader_module(include_wgsl!("vector.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        bind_group_layouts: &[Some(layout)],
        label: Some(LABEL),
        ..PipelineLayoutDescriptor::default()
    });
    let float4 = mem::size_of::<[f32; 4]>() as BufferAddress;
    let attributes = [
        VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: 0,
            shader_location: 0,
        },
        VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: float4,
            shader_location: 1,
        },
        VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: 2 * float4,
            shader_location: 2,
        },
        VertexAttribute {
            format: VertexFormat::Float32,
            offset: 3 * float4,
            shader_location: 3,
        },
    ];
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
            topology: PrimitiveTopology::TriangleList,
            ..PrimitiveState::default()
        },
        vertex: VertexState {
            buffers: &[Some(VertexBufferLayout {
                array_stride: VERTEX_SIZE,
                attributes: &attributes,
                step_mode: VertexStepMode::Vertex,
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
    use std::f32::consts::FRAC_PI_4;

    use crate::app::offscreen::{Snapshot, render};
    use crate::base::color::{Rgb, Rgba};
    use crate::base::geometry::{Point, Rect};
    use crate::base::paint::{GradientStop, LinearGradient, Stroke};
    use crate::base::path::VectorPath;
    use crate::base::theme::Theme;
    use crate::base::transform::Transform;
    use crate::{BoxStyle, Frame};

    const SIZE: u32 = 64;

    /// A frame cleared to the theme's background with `draw` drawn on it.
    fn draw(theme: &Theme, draw: impl FnOnce(&mut Frame)) -> Option<Snapshot> {
        render(SIZE, SIZE, |frame| {
            frame.clear(Rgba::from(theme.background));
            draw(frame);
        })
    }

    fn blended(snapshot: &Snapshot, x: f32, y: f32, a: Rgb, b: Rgb) -> bool {
        let pixel = snapshot.pixel(x, y);
        pixel != a && pixel != b
    }

    #[test]
    fn test_a_filled_triangle_covers_its_inside_and_blends_its_edge() {
        let theme = Theme::dark();
        let triangle = VectorPath::polygon(&[
            Point::new(8.0, 56.0),
            Point::new(32.0, 8.0),
            Point::new(56.0, 56.0),
        ]);
        let Some(snapshot) = draw(&theme, |frame| frame.fill_path(&triangle, theme.accent)) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(32.0, 40.0, theme.accent, "inside");
        snapshot.assert_pixel(10.0, 10.0, background, "outside");
        snapshot.assert_pixel(32.0, 59.0, background, "below the base");
        // The slanted edge crosses the pixels left of x = 20 at y = 32.
        assert!(
            (14..24).any(|x| blended(&snapshot, x as f32, 32.0, theme.accent, background)),
            "the slanted edge is antialiased"
        );
    }

    #[test]
    fn test_a_stroke_is_as_wide_as_asked() {
        let theme = Theme::dark();
        let line = VectorPath::polyline(&[Point::new(8.0, 20.0), Point::new(56.0, 20.0)]);
        let Some(snapshot) = draw(&theme, |frame| {
            frame.stroke_path(&line, &Stroke::new(4.0), theme.accent);
        }) else {
            return;
        };
        let background = theme.background;
        for y in 18..22 {
            snapshot.assert_pixel(30.0, y as f32, theme.accent, "inside the width");
        }
        snapshot.assert_pixel(30.0, 17.0, background, "above the line");
        snapshot.assert_pixel(30.0, 22.0, background, "below the line");
        snapshot.assert_pixel(4.0, 20.0, background, "past its butt end");
    }

    #[test]
    fn test_a_dashed_line_leaves_gaps() {
        let theme = Theme::dark();
        let line = VectorPath::polyline(&[Point::new(0.0, 32.5), Point::new(64.0, 32.5)]);
        let dashed = Stroke::new(3.0).with_dash(&[10.0, 10.0], 0.0);
        let Some(snapshot) = draw(&theme, |frame| {
            frame.stroke_path(&line, &dashed, theme.accent)
        }) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(5.0, 32.0, theme.accent, "the first dash");
        snapshot.assert_pixel(15.0, 32.0, background, "the first gap");
        snapshot.assert_pixel(25.0, 32.0, theme.accent, "the second dash");
    }

    #[test]
    fn test_a_rotated_rect_turns_into_a_diamond() {
        let theme = Theme::dark();
        let square = VectorPath::rect(Rect::new(-10.0, -10.0, 20.0, 20.0));
        let turn = Transform::rotate(FRAC_PI_4).then(Transform::translate(32.0, 32.0));
        let Some(snapshot) = draw(&theme, |frame| {
            frame.with_transform(turn, |frame| frame.fill_path(&square, theme.accent));
        }) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(32.0, 20.0, theme.accent, "the diamond's top half");
        snapshot.assert_pixel(23.0, 23.0, background, "where the square's corner was");
        snapshot.assert_pixel(32.0, 32.0, theme.accent, "the center");
    }

    #[test]
    fn test_a_clip_cuts_a_path() {
        let theme = Theme::dark();
        let all = VectorPath::rect(Rect::new(0.0, 0.0, 64.0, 64.0));
        let Some(snapshot) = draw(&theme, |frame| {
            frame.clipped(Rect::new(16.0, 16.0, 16.0, 16.0), |frame| {
                frame.fill_path(&all, theme.accent);
            });
        }) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(16.0, 16.0, theme.accent, "the clip's first pixel");
        snapshot.assert_pixel(31.0, 31.0, theme.accent, "the clip's last pixel");
        snapshot.assert_pixel(32.0, 20.0, background, "right of the clip");
        snapshot.assert_pixel(15.0, 20.0, background, "left of the clip");
    }

    #[test]
    fn test_a_path_paints_between_the_boxes_drawn_around_it() {
        let theme = Theme::dark();
        let box_style = |color: Rgb| BoxStyle {
            border: None,
            corner_radius: 0.0,
            fill: Rgba::from(color),
            shadow: None,
        };
        let Some(snapshot) = draw(&theme, |frame| {
            frame.draw_box(Rect::new(0.0, 0.0, 40.0, 64.0), &box_style(theme.surface));
            frame.fill_path(
                &VectorPath::rect(Rect::new(20.0, 0.0, 30.0, 64.0)),
                theme.accent,
            );
            frame.draw_box(Rect::new(40.0, 0.0, 24.0, 64.0), &box_style(theme.danger));
        }) else {
            return;
        };
        snapshot.assert_pixel(10.0, 32.0, theme.surface, "the first box alone");
        snapshot.assert_pixel(30.0, 32.0, theme.accent, "the path over the first box");
        snapshot.assert_pixel(45.0, 32.0, theme.danger, "the second box over the path");
    }

    #[test]
    fn test_a_gradient_runs_through_its_stops() {
        let theme = Theme::dark();
        let red = Rgb::new(255, 0, 0);
        let blue = Rgb::new(0, 0, 255);
        let gradient = LinearGradient {
            end: Point::new(56.0, 0.0),
            start: Point::new(8.0, 0.0),
            stops: vec![
                GradientStop {
                    color: Rgba::from(red),
                    offset: 0.0,
                },
                GradientStop {
                    color: Rgba::from(blue),
                    offset: 1.0,
                },
            ],
        };
        let Some(snapshot) = draw(&theme, |frame| {
            frame.fill_path(&VectorPath::rect(Rect::new(0.0, 0.0, 64.0, 64.0)), gradient);
        }) else {
            return;
        };
        snapshot.assert_pixel(2.0, 32.0, red, "before the start");
        snapshot.assert_pixel(60.0, 32.0, blue, "past the end");
        let middle = snapshot.pixel(32.0, 32.0);
        assert!(
            middle.r.abs_diff(middle.b) < 16 && middle.r > 100,
            "half way, found {middle:?}"
        );
    }
}

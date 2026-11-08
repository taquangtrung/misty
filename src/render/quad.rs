//! Draws [`Quad`]s, rectangles plain or rounded, bordered, and blurred,
//! as instanced four-vertex strips shaded by their distance to the edge.

use std::array;
use std::mem;
use std::num::NonZeroU64;
use std::ops::Range;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BlendState, Buffer, BufferAddress, BufferBindingType,
    BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, FragmentState, MultisampleState,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology,
    RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderStages, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode, include_wgsl,
};

use super::super::base::geometry::Rect;
use super::color_for_target;
use super::draw::Quad;
use super::gpu::GpuContext;

// ========================================================================
// Constants
// ========================================================================

/// The label GPU debuggers show for the quad pipeline and its resources.
const LABEL: &str = "misty quads";

/// The floats each quad instance holds: its rect, fill color, border
/// color, shape (radius, border width, blur, and padding), and clip.
const FLOATS_PER_QUAD: usize = 20;

/// The vertex attributes of an instance, four floats each, in the order
/// [`FLOATS_PER_QUAD`] lists them.
const ATTRIBUTES_PER_QUAD: usize = 5;

/// The border color of a quad with no border: never seen, since its width
/// is 0.
const NO_BORDER: [f32; 4] = [0.0; 4];

/// The bytes of one quad instance.
const QUAD_SIZE: BufferAddress = (FLOATS_PER_QUAD * mem::size_of::<f32>()) as BufferAddress;

/// The bytes of the viewport uniform: a size and padding to 16 bytes.
const VIEWPORT_SIZE: BufferAddress = 4 * mem::size_of::<f32>() as BufferAddress;

/// The vertices of one quad's strip.
const VERTICES_PER_QUAD: Range<u32> = 0..4;

/// How many quads the instance buffer holds before it first grows.
const INITIAL_CAPACITY: usize = 256;

// ========================================================================
// Data Structures
// ========================================================================

/// The pipeline that draws quads, and the buffers it reads.
pub(crate) struct QuadPipeline {
    /// Binds the viewport uniform.
    bind_group: BindGroup,
    /// How many quads `instances` has room for.
    capacity: usize,
    /// The quads of the frame being drawn.
    instances: Buffer,
    /// The pipeline itself.
    pipeline: RenderPipeline,
    /// The target size, for converting pixels to clip space.
    viewport: Buffer,
}

// ========================================================================
// QuadPipeline Implementations
// ========================================================================

impl QuadPipeline {
    /// A pipeline drawing to textures of `format`.
    pub(crate) fn new(gpu: &GpuContext, format: TextureFormat) -> Self {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
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
        let viewport = device.create_buffer(&BufferDescriptor {
            label: Some(LABEL),
            mapped_at_creation: false,
            size: VIEWPORT_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            entries: &[BindGroupEntry {
                binding: 0,
                resource: viewport.as_entire_binding(),
            }],
            label: Some(LABEL),
            layout: &layout,
        });
        Self {
            bind_group,
            capacity: INITIAL_CAPACITY,
            instances: create_instance_buffer(gpu, INITIAL_CAPACITY),
            pipeline: create_pipeline(gpu, format, &layout),
            viewport,
        }
    }

    /// Uploads the quads of `layers`, one after another, for a target
    /// `width` by `height` pixels whose format is sRGB when `srgb`, growing
    /// the instance buffer when they do not fit.
    pub(crate) fn prepare(
        &mut self,
        gpu: &GpuContext,
        (width, height): (u32, u32),
        srgb: bool,
        layers: &[&[Quad]],
    ) {
        let count = layers.iter().map(|layer| layer.len()).sum::<usize>();
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.instances = create_instance_buffer(gpu, self.capacity);
        }
        let viewport = [width as f32, height as f32, 0.0, 0.0];
        gpu.queue
            .write_buffer(&self.viewport, 0, &float_bytes(&viewport));
        let whole = Rect::new(0.0, 0.0, width as f32, height as f32);
        let mut floats = Vec::with_capacity(count * FLOATS_PER_QUAD);
        for quad in layers.iter().copied().flatten() {
            floats.extend(rect_floats(quad.rect));
            floats.extend(color_for_target(quad.color, srgb));
            let (border_color, border_width) = match quad.border {
                Some(border) => (color_for_target(border.color, srgb), border.width),
                None => (NO_BORDER, 0.0),
            };
            floats.extend(border_color);
            floats.extend([quad.radius, border_width, quad.blur, 0.0]);
            floats.extend(rect_floats(quad.clip.unwrap_or(whole)));
        }
        if !floats.is_empty() {
            gpu.queue
                .write_buffer(&self.instances, 0, &float_bytes(&floats));
        }
    }

    /// Draws the uploaded quads at positions `range` into `pass`.
    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, range: Range<u32>) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(VERTICES_PER_QUAD, range);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// An instance buffer with room for `capacity` quads.
fn create_instance_buffer(gpu: &GpuContext, capacity: usize) -> Buffer {
    gpu.device.create_buffer(&BufferDescriptor {
        label: Some(LABEL),
        mapped_at_creation: false,
        size: capacity as BufferAddress * QUAD_SIZE,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
    })
}

/// The quad pipeline for targets of `format`, reading uniforms through
/// `layout`.
fn create_pipeline(
    gpu: &GpuContext,
    format: TextureFormat,
    layout: &BindGroupLayout,
) -> RenderPipeline {
    let device = &gpu.device;
    let shader = device.create_shader_module(include_wgsl!("quad.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        bind_group_layouts: &[Some(layout)],
        label: Some(LABEL),
        ..PipelineLayoutDescriptor::default()
    });
    let float4 = mem::size_of::<[f32; 4]>() as BufferAddress;
    let attributes: [VertexAttribute; ATTRIBUTES_PER_QUAD] =
        array::from_fn(|index| VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: index as BufferAddress * float4,
            shader_location: index as u32,
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
                array_stride: QUAD_SIZE,
                attributes: &attributes,
                step_mode: VertexStepMode::Instance,
            })],
            compilation_options: PipelineCompilationOptions::default(),
            entry_point: Some("vs_main"),
            module: &shader,
        },
    })
}

/// `rect` as the four floats an instance holds: left, top, width, height.
fn rect_floats(rect: Rect) -> [f32; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

/// `floats` as the bytes the GPU reads, in the machine's byte order.
pub(super) fn float_bytes(floats: &[f32]) -> Vec<u8> {
    floats
        .iter()
        .flat_map(|float| float.to_ne_bytes())
        .collect()
}

//! Draws [`Quad`]s, rectangles plain or rounded, bordered, and blurred,
//! as instanced four-vertex strips shaded by their distance to the edge.
//! A quad drawn inside a transform is drawn by a pipeline of its own,
//! which reads the transform from a second instance buffer
//! ([`placement_floats`]); a frame with none never fills that buffer.

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
use super::super::base::transform::Transform;
use super::color_for_target;
use super::draw::{Quad, Transformed};
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

/// The floats each quad's or picture's placement holds: its transform's
/// matrix; the transform's translation, its pixels per unit, and whether it
/// turns rectangles off the axes; and its clip in window pixels.
pub(super) const FLOATS_PER_PLACEMENT: usize = 12;

/// The vertex attributes of a placement, four floats each, in the order
/// [`FLOATS_PER_PLACEMENT`] lists them, after an instance's own.
const ATTRIBUTES_PER_PLACEMENT: usize = 3;

/// The bytes of one placement.
pub(super) const PLACEMENT_SIZE: BufferAddress =
    (FLOATS_PER_PLACEMENT * mem::size_of::<f32>()) as BufferAddress;

/// The own clip of a transformed quad that has none: far larger than any
/// window, so it clips nothing wherever the transform puts it.
pub(super) const UNCLIPPED: Rect = Rect::new(-1.0e9, -1.0e9, 2.0e9, 2.0e9);

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
    /// Whether each uploaded quad is transformed, in order; empty when none
    /// of the frame's is.
    placed: Vec<bool>,
    /// The pipeline drawing transformed quads.
    placed_pipeline: RenderPipeline,
    /// How many placements `placements` has room for.
    placement_capacity: usize,
    /// Each uploaded quad's placement, when any is transformed.
    placements: Buffer,
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
            instances: create_instance_buffer(gpu, INITIAL_CAPACITY, QUAD_SIZE),
            pipeline: create_pipeline(gpu, format, &layout, false),
            placed: Vec::new(),
            placed_pipeline: create_pipeline(gpu, format, &layout, true),
            placement_capacity: INITIAL_CAPACITY,
            placements: create_instance_buffer(gpu, INITIAL_CAPACITY, PLACEMENT_SIZE),
            viewport,
        }
    }

    /// Uploads `quads`, `count` of them, in order, for a target `width` by
    /// `height` pixels whose format is sRGB when `srgb`, growing the
    /// instance buffer when they do not fit.
    pub(crate) fn prepare<'q>(
        &mut self,
        gpu: &GpuContext,
        (width, height): (u32, u32),
        srgb: bool,
        count: usize,
        quads: impl IntoIterator<Item = &'q Quad>,
    ) {
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.instances = create_instance_buffer(gpu, self.capacity, QUAD_SIZE);
        }
        let viewport = [width as f32, height as f32, 0.0, 0.0];
        gpu.queue
            .write_buffer(&self.viewport, 0, &float_bytes(&viewport));
        let whole = Rect::new(0.0, 0.0, width as f32, height as f32);
        let mut floats = Vec::with_capacity(count * FLOATS_PER_QUAD);
        self.placed.clear();
        let mut placements = Vec::new();
        let mut any_placed = false;
        for (index, quad) in quads.into_iter().take(count).enumerate() {
            if quad.transform.is_some() && !any_placed {
                // The first transformed quad: those before it get the
                // identity, so every quad has a placement at its index.
                any_placed = true;
                self.placed = vec![false; index];
                placements.reserve(count * FLOATS_PER_PLACEMENT);
                for _ in 0..index {
                    placements.extend(placement_floats(None, whole));
                }
            }
            if any_placed {
                self.placed.push(quad.transform.is_some());
                placements.extend(placement_floats(quad.transform.as_ref(), whole));
            }
            floats.extend(rect_floats(quad.rect));
            floats.extend(color_for_target(quad.color, srgb));
            let (border_color, border_width) = match quad.border {
                Some(border) => (color_for_target(border.color, srgb), border.width),
                None => (NO_BORDER, 0.0),
            };
            floats.extend(border_color);
            floats.extend([quad.radius, border_width, quad.blur, 0.0]);
            let unclipped = match quad.transform {
                Some(_) => UNCLIPPED,
                None => whole,
            };
            floats.extend(rect_floats(quad.clip.unwrap_or(unclipped)));
        }
        if !floats.is_empty() {
            gpu.queue
                .write_buffer(&self.instances, 0, &float_bytes(&floats));
        }
        if !placements.is_empty() {
            if self.placed.len() > self.placement_capacity {
                self.placement_capacity = self.placed.len().next_power_of_two();
                self.placements =
                    create_instance_buffer(gpu, self.placement_capacity, PLACEMENT_SIZE);
            }
            gpu.queue
                .write_buffer(&self.placements, 0, &float_bytes(&placements));
        }
    }

    /// Draws the uploaded quads at positions `range` into `pass`: in one
    /// draw when the frame has no transformed quad, otherwise a draw per
    /// run of transformed or untransformed ones.
    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, range: Range<u32>) {
        if range.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        if self.placed.is_empty() {
            pass.set_pipeline(&self.pipeline);
            pass.draw(VERTICES_PER_QUAD, range);
            return;
        }
        pass.set_vertex_buffer(1, self.placements.slice(..));
        let is_placed = |index: u32| self.placed.get(index as usize).copied().unwrap_or(false);
        let mut start = range.start;
        while start < range.end {
            let placed = is_placed(start);
            let end = (start..range.end)
                .find(|&index| is_placed(index) != placed)
                .unwrap_or(range.end);
            let pipeline = match placed {
                true => &self.placed_pipeline,
                false => &self.pipeline,
            };
            pass.set_pipeline(pipeline);
            pass.draw(VERTICES_PER_QUAD, start..end);
            start = end;
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The floats of the placement of a quad or picture drawn through
/// `transform`, in a target whose whole area is `whole`: the identity, and
/// no clip, for one drawn as it is.
pub(super) fn placement_floats(
    transform: Option<&Transformed>,
    whole: Rect,
) -> [f32; FLOATS_PER_PLACEMENT] {
    let (matrix, clip) = match transform {
        Some(transform) => (transform.matrix, transform.clip.unwrap_or(whole)),
        None => (Transform::IDENTITY, whole),
    };
    let [a, b, c, d, e, f] = matrix.coefficients();
    let turned = f32::from(u8::from(!matrix.is_axis_aligned()));
    let [x, y, width, height] = rect_floats(clip);
    [
        a,
        b,
        c,
        d,
        e,
        f,
        matrix.length_scale(),
        turned,
        x,
        y,
        width,
        height,
    ]
}

/// A vertex buffer with room for `capacity` instances of `stride` bytes.
fn create_instance_buffer(gpu: &GpuContext, capacity: usize, stride: BufferAddress) -> Buffer {
    gpu.device.create_buffer(&BufferDescriptor {
        label: Some(LABEL),
        mapped_at_creation: false,
        size: capacity as BufferAddress * stride,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
    })
}

/// The quad pipeline for targets of `format`, reading uniforms through
/// `layout`: the one for transformed quads, which reads their placements
/// too, when `placed`.
fn create_pipeline(
    gpu: &GpuContext,
    format: TextureFormat,
    layout: &BindGroupLayout,
    placed: bool,
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
    let placement_attributes: [VertexAttribute; ATTRIBUTES_PER_PLACEMENT] =
        array::from_fn(|index| VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: index as BufferAddress * float4,
            shader_location: (ATTRIBUTES_PER_QUAD + index) as u32,
        });
    let instances = VertexBufferLayout {
        array_stride: QUAD_SIZE,
        attributes: &attributes,
        step_mode: VertexStepMode::Instance,
    };
    let placements = VertexBufferLayout {
        array_stride: PLACEMENT_SIZE,
        attributes: &placement_attributes,
        step_mode: VertexStepMode::Instance,
    };
    let (vertex_entry, fragment_entry) = match placed {
        true => ("vs_placed", "fs_placed"),
        false => ("vs_main", "fs_main"),
    };
    let both = [Some(instances.clone()), Some(placements)];
    let buffers = match placed {
        true => &both[..],
        false => &both[..1],
    };
    device.create_render_pipeline(&RenderPipelineDescriptor {
        cache: None,
        depth_stencil: None,
        fragment: Some(FragmentState {
            compilation_options: PipelineCompilationOptions::default(),
            entry_point: Some(fragment_entry),
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
            buffers,
            compilation_options: PipelineCompilationOptions::default(),
            entry_point: Some(vertex_entry),
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

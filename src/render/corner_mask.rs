//! Cuts a window's rounded corners out of what its frame drew, for a
//! frameless window on a system that does not round it: every pixel is
//! scaled by how much of it lies inside the window's rounded rectangle, so
//! on a surface composited with its alpha the corners show the desktop.

use std::num::NonZeroU64;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BlendComponent, BlendFactor, BlendOperation, BlendState,
    Buffer, BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites,
    FragmentState, MultisampleState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderStages,
    TextureFormat, VertexState, include_wgsl,
};

use super::gpu::GpuContext;
use super::quad::float_bytes;

// ========================================================================
// Constants
// ========================================================================

/// The label GPU debuggers show for the corner mask and its resources.
const LABEL: &str = "misty corner mask";

/// The bytes of the mask's uniform: the target's size, the radius, and a
/// float of padding.
const MASK_SIZE: u64 = 16;

/// A target pixel scaled by the mask's alpha, color and alpha alike, which
/// keeps a premultiplied pixel premultiplied.
const SCALE_BY_MASK: BlendComponent = BlendComponent {
    dst_factor: BlendFactor::SrcAlpha,
    operation: BlendOperation::Add,
    src_factor: BlendFactor::Zero,
};

// ========================================================================
// Data Structures
// ========================================================================

/// The pipeline that cuts out a window's rounded corners.
pub(crate) struct CornerMask {
    /// Binds the mask's uniform.
    bind_group: BindGroup,
    /// Draws the triangle over the target.
    pipeline: RenderPipeline,
    /// The target's size and the corners' radius.
    uniform: Buffer,
}

// ========================================================================
// CornerMask Implementations
// ========================================================================

impl CornerMask {
    /// The mask for targets of `format`.
    pub(crate) fn new(gpu: &GpuContext, format: TextureFormat) -> Self {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                count: None,
                ty: BindingType::Buffer {
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(MASK_SIZE),
                    ty: BufferBindingType::Uniform,
                },
                visibility: ShaderStages::FRAGMENT,
            }],
            label: Some(LABEL),
        });
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some(LABEL),
            mapped_at_creation: false,
            size: MASK_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
            label: Some(LABEL),
            layout: &layout,
        });
        let shader = device.create_shader_module(include_wgsl!("corner_mask.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            bind_group_layouts: &[Some(&layout)],
            label: Some(LABEL),
            ..PipelineLayoutDescriptor::default()
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            cache: None,
            depth_stencil: None,
            fragment: Some(FragmentState {
                compilation_options: PipelineCompilationOptions::default(),
                entry_point: Some("fs_main"),
                module: &shader,
                targets: &[Some(ColorTargetState {
                    blend: Some(BlendState {
                        alpha: SCALE_BY_MASK,
                        color: SCALE_BY_MASK,
                    }),
                    format,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            label: Some(LABEL),
            layout: Some(&pipeline_layout),
            multisample: MultisampleState::default(),
            multiview_mask: None,
            primitive: PrimitiveState::default(),
            vertex: VertexState {
                buffers: &[],
                compilation_options: PipelineCompilationOptions::default(),
                entry_point: Some("vs_main"),
                module: &shader,
            },
        });
        Self {
            bind_group,
            pipeline,
            uniform,
        }
    }

    /// Readies the mask for a target `width` by `height` physical pixels
    /// whose corners are rounded by `radius` physical pixels.
    pub(crate) fn prepare(&self, gpu: &GpuContext, (width, height): (u32, u32), radius: f32) {
        let floats = [width as f32, height as f32, radius, 0.0];
        gpu.queue
            .write_buffer(&self.uniform, 0, &float_bytes(&floats));
    }

    /// Cuts the corners out of what `pass` drew so far.
    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

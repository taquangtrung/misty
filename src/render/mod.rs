//! Drawing, below the app and its widgets: the [`gpu`] and window
//! [`surface`]s, the [`draw`] list a frame fills, [`font`]s, cell
//! [`grid`]s and [`text_layout`], proportional [`paragraph`]s, pictures
//! ([`image`]), app-rendered [`texture`]s, and [`icon`] strokes, the flexbox [`layout`] engine, and
//! the renderer that draws [`DrawList`]s into a window's swapchain
//! texture: for each layer, fills (with vector paths, [`tessellate`]d on
//! the CPU, drawn in order among them), then pictures, then text, then
//! decorations, in one render pass over a cleared target.

pub(crate) mod app_fonts;
pub(crate) mod draw;
pub(crate) mod font;
pub(crate) mod gpu;
pub(crate) mod grid;
pub(crate) mod icon;
#[cfg(feature = "icon-theme")]
pub(crate) mod icon_theme;
pub(crate) mod image;
pub mod layout;
pub(crate) mod paragraph;
mod picture;
mod quad;
mod shape_cache;
pub(crate) mod surface;
pub(crate) mod tessellate;
mod text;
pub(crate) mod text_layout;
pub(crate) mod texture;
mod vector;

use wgpu::{
    Color, CommandEncoderDescriptor, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDescriptor, StoreOp, TextureFormat, TextureView,
};

use self::draw::{DrawList, Quad};
use self::gpu::GpuContext;
use super::base::color::Rgba;

#[cfg(test)]
pub(crate) use shape_cache::KEEP_FRAMES;
pub(crate) use text::{Fonts, ShapingFont, shape_text};

use picture::PicturePipeline;
use quad::QuadPipeline;
use text::TextPipeline;
use vector::VectorPipeline;

// ========================================================================
// Constants
// ========================================================================

/// The label GPU debuggers show for the frame's command encoder.
const FRAME_ENCODER_LABEL: &str = "misty frame";

/// The label GPU debuggers show for the frame's render pass.
const FRAME_PASS_LABEL: &str = "misty frame";

/// How many layers a frame draws: the base and the overlay.
pub(crate) const LAYERS: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// One window's pipelines, made for its surface's format.
pub(crate) struct Renderer {
    /// Draws images and SVG documents.
    pictures: PicturePipeline,
    /// Draws fills and decorations.
    quads: QuadPipeline,
    /// Whether the target's format is sRGB, which wants linear colors.
    srgb: bool,
    /// Draws text.
    text: TextPipeline,
    /// Draws vector paths.
    vectors: VectorPipeline,
}

/// What one frame draws and where.
pub(crate) struct FrameTarget<'a> {
    /// The color the target is cleared to first.
    pub(crate) clear: Rgba,
    /// What is drawn over the cleared target, in layers painted bottom to
    /// top, each wholly over the ones before.
    pub(crate) layers: [&'a DrawList; LAYERS],
    /// The target's size, `(width, height)` in physical pixels.
    pub(crate) size: (u32, u32),
    /// The texture drawn into.
    pub(crate) view: &'a TextureView,
}

// ========================================================================
// Renderer Implementations
// ========================================================================

impl Renderer {
    /// A renderer for targets of `format`.
    pub(crate) fn new(gpu: &GpuContext, format: TextureFormat) -> Self {
        Self {
            pictures: PicturePipeline::new(gpu, format),
            quads: QuadPipeline::new(gpu, format),
            srgb: format.is_srgb(),
            text: TextPipeline::new(gpu, format, LAYERS),
            vectors: VectorPipeline::new(gpu, format),
        }
    }

    /// Draws `target` and submits the work. Text that cannot be prepared (a
    /// full atlas) is reported and left out; the rest is still drawn.
    pub(crate) fn render(
        &mut self,
        gpu: &GpuContext,
        fonts: &mut Fonts,
        font: &ShapingFont<'_>,
        target: FrameTarget<'_>,
    ) {
        // Each layer's fills then decorations, so every quad range is one
        // contiguous run of the instance buffer.
        let quads: Vec<&[Quad]> = target
            .layers
            .iter()
            .flat_map(|layer| [layer.fills.as_slice(), layer.decorations.as_slice()])
            .collect();
        self.quads.prepare(gpu, target.size, self.srgb, &quads);
        let paths = target.layers.map(|layer| layer.paths.as_slice());
        self.vectors.prepare(gpu, target.size, self.srgb, paths);
        let pictures = target.layers.map(|layer| layer.pictures.as_slice());
        self.pictures.prepare(gpu, target.size, &pictures);
        let texts = target.layers.map(|layer| layer.text.as_slice());
        let text_ready = match self.text.prepare(gpu, fonts, font, target.size, &texts) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("text not drawn: {error}");
                false
            }
        };
        let mut encoder = gpu
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some(FRAME_ENCODER_LABEL),
            });
        {
            let [r, g, b, a] = color_for_target(target.clear, self.srgb).map(f64::from);
            let attachment = RenderPassColorAttachment {
                depth_slice: None,
                ops: Operations {
                    load: LoadOp::Clear(Color { a, b, g, r }),
                    store: StoreOp::Store,
                },
                resolve_target: None,
                view: target.view,
            };
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                color_attachments: &[Some(attachment)],
                label: Some(FRAME_PASS_LABEL),
                ..RenderPassDescriptor::default()
            });
            let mut start = 0;
            for (index, layer) in target.layers.iter().enumerate() {
                let fills_end = start + quad_count(layer.fills.len());
                let end = fills_end + quad_count(layer.decorations.len());
                // The fills up to each run of paths, then the run.
                let mut drawn = start;
                for run in self.vectors.runs(index) {
                    let before = (start + quad_count(run.fills_before)).min(fills_end);
                    self.quads.draw(&mut pass, drawn..before);
                    self.vectors.draw(&mut pass, run.indices.clone());
                    drawn = before;
                }
                self.quads.draw(&mut pass, drawn..fills_end);
                self.pictures.draw(&mut pass, index);
                if text_ready && let Err(error) = self.text.draw(&mut pass, index) {
                    log::warn!("text not drawn: {error}");
                }
                self.quads.draw(&mut pass, fills_end..end);
                start = end;
            }
        }
        gpu.queue.submit([encoder.finish()]);
        self.text.trim();
        self.pictures.finish_frame();
        fonts.finish_frame();
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `color` as the channels a target stores: linear for an sRGB format,
/// which encodes on write, and sRGB-encoded as is otherwise.
fn color_for_target(color: Rgba, srgb: bool) -> [f32; 4] {
    match srgb {
        true => color.to_linear(),
        false => color.to_normalized(),
    }
}

/// `count` quads as an instance index. No frame comes near `u32::MAX`
/// quads; one that did is cut short rather than wrapped.
fn quad_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

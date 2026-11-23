//! Drawing, below the app and its widgets: the [`gpu`] and window
//! [`surface`]s, the [`draw`] list a frame fills, [`font`]s, cell
//! [`grid`]s and [`text_layout`], proportional [`paragraph`]s (and their
//! off-thread [`measurer`], with the [`inline_font`] their boxes are set
//! in), pictures
//! ([`image`]), app-rendered [`texture`]s, and [`icon`] strokes, the flexbox [`layout`] engine, and
//! the renderer that draws [`DrawList`]s into a window's swapchain
//! texture, in one render pass over a cleared target: layer after layer,
//! and in each layer everything in the order it was drawn, so a later
//! draw paints over an earlier one whatever their kinds. Consecutive draws
//! of one kind (quads, vector paths [`tessellate`]d on the CPU, pictures,
//! or text) are one batch, painted with one pipeline; a change of kind
//! starts the next batch. Text drawn inside a transform is drawn as
//! pictures ([`text_raster`]).

pub(crate) mod app_fonts;
mod corner_mask;
mod desktop_font;
pub(crate) mod draw;
pub(crate) mod font;
pub(crate) mod gpu;
pub(crate) mod grid;
pub(crate) mod icon;
#[cfg(feature = "icon-theme")]
pub(crate) mod icon_theme;
pub(crate) mod image;
mod inline_font;
pub mod layout;
pub(crate) mod measurer;
pub(crate) mod paragraph;
mod picture;
mod quad;
mod shape_cache;
pub(crate) mod surface;
pub(crate) mod tessellate;
mod text;
pub(crate) mod text_layout;
mod text_raster;
pub(crate) mod texture;
mod vector;

use std::array;

use wgpu::{
    Color, CommandEncoderDescriptor, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDescriptor, StoreOp, TextureFormat, TextureView,
};

use self::draw::{BatchKind, DrawBatch, DrawList};
use self::gpu::GpuContext;
use super::base::color::Rgba;

pub(crate) use picture::DEFAULT_BUDGET_BYTES;
#[cfg(test)]
pub(crate) use shape_cache::KEEP_FRAMES;
pub(crate) use text::{Fonts, ShapingFont, shape_text};

use corner_mask::CornerMask;
use picture::PicturePipeline;
use quad::QuadPipeline;
use text::{TextBatches, TextPipeline};
use text_raster::TextRasters;
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
    /// Cuts out a frameless window's rounded corners.
    corners: CornerMask,
    /// Draws images and SVG documents.
    pictures: PicturePipeline,
    /// Draws fills and decorations.
    quads: QuadPipeline,
    /// Whether the target's format is sRGB, which wants linear colors.
    srgb: bool,
    /// Draws text.
    text: TextPipeline,
    /// The images of text drawn inside a transform.
    text_rasters: TextRasters,
    /// Draws vector paths.
    vectors: VectorPipeline,
}

/// What one frame draws and where.
pub(crate) struct FrameTarget<'a> {
    /// The color the target is cleared to first.
    pub(crate) clear: Rgba,
    /// The radius the target's corners are cut out by, in physical pixels,
    /// once everything is drawn; 0 for square corners.
    pub(crate) corner_radius: f32,
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
            corners: CornerMask::new(gpu, format),
            pictures: PicturePipeline::new(gpu, format),
            quads: QuadPipeline::new(gpu, format),
            srgb: format.is_srgb(),
            text: TextPipeline::new(gpu, format, LAYERS),
            text_rasters: TextRasters::new(),
            vectors: VectorPipeline::new(gpu, format),
        }
    }

    /// Keeps at most `bytes` of picture textures from the end of this frame
    /// on.
    pub(crate) fn set_texture_budget(&mut self, bytes: usize) {
        self.pictures.set_budget(bytes);
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
        // A layer with transformed text is drawn as a copy with that text
        // turned into pictures; any other as it is.
        let rasters = &mut self.text_rasters;
        let rastered: [Option<DrawList>; LAYERS] = target.layers.map(|layer| {
            layer.with_text_as_pictures(|segment| rasters.picture_of(fonts, font, segment))
        });
        let layers: [&DrawList; LAYERS] =
            array::from_fn(|index| rastered[index].as_ref().unwrap_or(target.layers[index]));
        let batches: [Vec<DrawBatch>; LAYERS] = layers.map(DrawList::batches);
        // Every layer's quads in the order drawn, so each batch of them is
        // one contiguous run of the instance buffer.
        let quad_count = layers.iter().map(|layer| layer.quad_count()).sum();
        let quads = layers.iter().flat_map(|layer| layer.quads());
        self.quads
            .prepare(gpu, target.size, self.srgb, quad_count, quads);
        let paths = layers.map(DrawList::paths);
        self.vectors.prepare(gpu, target.size, self.srgb, paths);
        let pictures = layers.map(DrawList::pictures);
        self.pictures.prepare(gpu, target.size, &pictures);
        let texts: Vec<TextBatches<'_>> = layers
            .iter()
            .zip(&batches)
            .map(|(layer, batches)| TextBatches {
                batches: batches
                    .iter()
                    .filter(|batch| batch.kind == BatchKind::Text)
                    .map(|batch| batch.range.clone())
                    .collect(),
                segments: layer.text(),
            })
            .collect();
        let text_ready = match self.text.prepare(gpu, fonts, font, target.size, &texts) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("text not drawn: {error}");
                false
            }
        };
        let rounded = target.corner_radius > 0.0;
        if rounded {
            self.corners.prepare(gpu, target.size, target.corner_radius);
        }
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
            // Where the layer's quads start in the instance buffer, and
            // how many text batches the layers before drew.
            let mut quads_start = 0;
            let mut text_batch = 0;
            for (index, (layer, batches)) in layers.iter().zip(&batches).enumerate() {
                for batch in batches {
                    let range = batch.range.clone();
                    match batch.kind {
                        BatchKind::Paths => self.vectors.draw(&mut pass, index, range),
                        BatchKind::Pictures => self.pictures.draw(&mut pass, index, range),
                        BatchKind::Quads => {
                            let start = quads_start + range.start;
                            let end = quads_start + range.end;
                            self.quads
                                .draw(&mut pass, quad_index(start)..quad_index(end));
                        }
                        BatchKind::Text => {
                            if text_ready && let Err(error) = self.text.draw(&mut pass, text_batch)
                            {
                                log::warn!("text not drawn: {error}");
                            }
                            text_batch += 1;
                        }
                    }
                }
                quads_start += layer.quad_count();
            }
            // Last, so nothing drawn paints the corners back in.
            if rounded {
                self.corners.draw(&mut pass);
            }
        }
        gpu.queue.submit([encoder.finish()]);
        self.text.trim();
        self.text_rasters.finish_frame();
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

/// `index` as a quad instance index. No frame comes near `u32::MAX`
/// quads; one that did is cut short rather than wrapped.
fn quad_index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

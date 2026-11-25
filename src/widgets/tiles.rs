//! [`TileSet`]: content too large to draw as one image, such as a PDF page
//! at a high zoom or a huge scan, drawn as square tiles the app renders
//! (on worker threads) at the detail the zoom wants.
//!
//! Content is cut into a grid of tiles at each level of detail: at level
//! 0 a tile covers `base_extent` content units on a side, and each level
//! up covers half as much in as many pixels. Each frame:
//!
//! 1. [`TileSet::level_for`] the [`Viewport`]'s zoom says which level is
//!    sharp enough.
//! 2. [`TileSet::missing`] lists the tiles in sight at that level the set
//!    does not hold yet: render them off the main thread, and send each
//!    [`Image`] back to the app through its [`Waker`](crate::Waker).
//! 3. [`TileSet::insert`] keeps a tile that arrived, and the app asks for
//!    a redraw.
//! 4. [`TileSet::draw`] draws the tiles in sight: a tile not there yet is
//!    covered by a coarser one the set holds, else by a placeholder.
//!
//! Tiles not drawn for a while are let go, so memory stays bounded however
//! far the user zooms and pans. Uploading is the renderer's: a tile's
//! image goes to the GPU once, when first drawn.

use std::collections::HashMap;

use super::super::app::frame::Frame;
use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::super::render::image::Image;
use super::viewport::Viewport;

// ========================================================================
// Constants
// ========================================================================

/// How many frames a tile is kept without being drawn unless the app says
/// otherwise: about two seconds of steady drawing.
pub(crate) const DEFAULT_KEEP_FRAMES: u64 = 120;

/// The finest level of detail, past which zooming in shows the finest
/// tiles larger.
const MAX_LEVEL: u8 = 20;

// ========================================================================
// Data Structures
// ========================================================================

/// Tiles of content at levels of detail, kept while they are drawn.
#[derive(Clone, Debug)]
pub struct TileSet {
    /// The content units a tile covers on a side at level 0.
    base_extent: f32,
    /// The size of the content, in its units.
    content: Rect,
    /// How many times it was drawn.
    frames: u64,
    /// How many frames a tile is kept without being drawn.
    keep_frames: u64,
    /// The pixels on a side of each tile's image.
    tile_pixels: u32,
    /// The tiles held, each with the frame it was last drawn in.
    tiles: HashMap<TileKey, (Image, u64)>,
}

/// Which tile: its level of detail, and its column and row at that level.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TileKey {
    /// Its column, from the content's left edge.
    pub col: u32,
    /// Its level of detail: each level halves the content a tile covers.
    pub level: u8,
    /// Its row, from the content's top edge.
    pub row: u32,
}

// ========================================================================
// TileSet Implementations
// ========================================================================

impl TileSet {
    /// A set of tiles `tile_pixels` on a side, covering `base_extent`
    /// content units at level 0, of content `width` by `height` units.
    pub fn new(tile_pixels: u32, base_extent: f32, width: f32, height: f32) -> Self {
        Self {
            base_extent: base_extent.max(f32::MIN_POSITIVE),
            content: Rect::new(0.0, 0.0, width.max(0.0), height.max(0.0)),
            frames: 0,
            keep_frames: DEFAULT_KEEP_FRAMES,
            tile_pixels: tile_pixels.max(1),
            tiles: HashMap::new(),
        }
    }

    /// This set keeping a tile `frames` draws after it was last drawn.
    pub fn with_keep_frames(self, frames: u64) -> Self {
        Self {
            keep_frames: frames,
            ..self
        }
    }

    /// The content units a tile at `level` covers on a side.
    pub fn extent(&self, level: u8) -> f32 {
        self.base_extent / f32::from(level.min(MAX_LEVEL)).exp2()
    }

    /// The content `key` covers, in its units.
    pub fn content_rect(&self, key: TileKey) -> Rect {
        let extent = self.extent(key.level);
        Rect::new(
            key.col as f32 * extent,
            key.row as f32 * extent,
            extent,
            extent,
        )
    }

    /// The coarsest level whose tiles hold at least a pixel per physical
    /// pixel at `zoom` (physical pixels per content unit).
    pub fn level_for(&self, zoom: f32) -> u8 {
        let wanted = zoom * self.base_extent / self.tile_pixels as f32;
        if wanted <= 1.0 {
            return 0;
        }
        (wanted.log2().ceil() as u8).min(MAX_LEVEL)
    }

    /// The tiles at `level` with a part in sight in `viewport`, row by row.
    pub fn in_sight(&self, viewport: &Viewport, level: u8) -> Vec<TileKey> {
        let Some(seen) = viewport.visible_content().intersection(&self.content) else {
            return Vec::new();
        };
        let extent = self.extent(level);
        let span = |start: f32, length: f32| {
            let first = (start / extent).floor().max(0.0) as u32;
            let end = ((start + length) / extent).ceil().max(0.0) as u32;
            first..end.max(first + 1)
        };
        let (cols, rows) = (span(seen.x, seen.width), span(seen.y, seen.height));
        rows.flat_map(|row| cols.clone().map(move |col| TileKey { col, level, row }))
            .collect()
    }

    /// The tiles in sight at `level` the set does not hold: what to render.
    pub fn missing(&self, viewport: &Viewport, level: u8) -> Vec<TileKey> {
        self.in_sight(viewport, level)
            .into_iter()
            .filter(|key| !self.tiles.contains_key(key))
            .collect()
    }

    /// Keeps `image` as the tile `key`, replacing one held.
    pub fn insert(&mut self, key: TileKey, image: Image) {
        self.tiles.insert(key, (image, self.frames));
    }

    /// Whether the set holds the tile `key`.
    pub fn contains(&self, key: TileKey) -> bool {
        self.tiles.contains_key(&key)
    }

    /// How many tiles the set holds.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Whether the set holds no tile.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Draws the tiles in sight at `level` where `viewport` shows them, cut
    /// to its area: each one held, else the coarser tile around it, else a
    /// placeholder in the theme's surface color. Then lets go of the tiles
    /// not drawn for the frames it keeps them.
    pub fn draw(&mut self, frame: &mut Frame, viewport: &Viewport, level: u8, theme: &Theme) {
        self.frames += 1;
        let now = self.frames;
        let area = viewport.area();
        frame.clipped(area, |frame| {
            for key in self.in_sight(viewport, level) {
                let rect = viewport.screen_rect(self.content_rect(key));
                let Some(cover) = self.cover(key) else {
                    if let Some(shown) = rect.intersection(&area) {
                        frame.fill_rect(shown, theme.surface);
                    }
                    continue;
                };
                let cover_rect = viewport.screen_rect(self.content_rect(cover));
                if let Some((image, drawn)) = self.tiles.get_mut(&cover) {
                    *drawn = now;
                    let image = image.clone();
                    frame.clipped(rect, |frame| frame.draw_image(&image, cover_rect));
                }
            }
        });
        let keep = self.keep_frames;
        self.tiles.retain(|_, (_, drawn)| now - *drawn <= keep);
    }

    /// The tile held that covers `key`: itself, else the nearest coarser
    /// one around it.
    fn cover(&self, key: TileKey) -> Option<TileKey> {
        (0..=key.level).rev().find_map(|level| {
            let shift = u32::from(key.level - level);
            let coarser = TileKey {
                col: key.col >> shift,
                level,
                row: key.row >> shift,
            };
            self.tiles.contains_key(&coarser).then_some(coarser)
        })
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::widget::Widget;
    use crate::testing::TestWindow;

    /// Further than any content here, to pan to its top left edge.
    const PAN_FAR: f32 = 1.0e4;

    fn tile() -> Image {
        Image::from_rgba(1, 1, vec![0; 4]).expect("an image")
    }

    /// A viewport of 1000 by 1000 content units in a 200 by 200 area at
    /// the actual size, panned to the top left.
    fn viewport() -> Viewport {
        let mut viewport = Viewport::new(1000.0, 1000.0);
        let area = Rect::new(0.0, 0.0, 200.0, 200.0);
        viewport.draw(&mut TestWindow::new(200, 200).frame(), area, &Theme::dark());
        viewport.actual_size();
        viewport.pan_by(PAN_FAR, PAN_FAR);
        viewport
    }

    #[test]
    fn test_the_level_doubles_its_detail_as_the_zoom_doubles() {
        let set = TileSet::new(256, 1024.0, 1000.0, 1000.0);
        assert_eq!(set.level_for(0.1), 0, "a tile's 256 pixels are plenty");
        assert_eq!(set.level_for(0.25), 0);
        assert_eq!(set.level_for(0.26), 1);
        assert_eq!(set.level_for(1.0), 2);
        assert_eq!(set.extent(2), 256.0);
        assert_eq!(
            set.content_rect(TileKey {
                col: 1,
                level: 2,
                row: 3
            }),
            Rect::new(256.0, 768.0, 256.0, 256.0)
        );
    }

    #[test]
    fn test_only_the_tiles_in_sight_are_missing_and_a_coarser_one_covers() {
        let mut set = TileSet::new(256, 1024.0, 1000.0, 1000.0);
        let view = viewport();
        let level = set.level_for(view.zoom());
        let missing = set.missing(&view, level);
        assert_eq!(
            missing,
            [TileKey {
                col: 0,
                level: 2,
                row: 0
            }],
            "200 units in sight of 256 a tile"
        );
        set.insert(
            TileKey {
                col: 0,
                level: 0,
                row: 0,
            },
            tile(),
        );
        assert_eq!(
            set.cover(missing[0]),
            Some(TileKey {
                col: 0,
                level: 0,
                row: 0
            })
        );
        set.insert(missing[0], tile());
        assert!(set.missing(&view, level).is_empty());
    }

    #[test]
    fn test_tiles_not_drawn_for_a_while_are_let_go() {
        let window = TestWindow::new(200, 200);
        let mut set = TileSet::new(256, 1024.0, 1000.0, 1000.0).with_keep_frames(2);
        let view = viewport();
        let far = TileKey {
            col: 3,
            level: 2,
            row: 3,
        };
        set.insert(far, tile());
        set.insert(
            TileKey {
                col: 0,
                level: 2,
                row: 0,
            },
            tile(),
        );
        let theme = Theme::dark();
        for _ in 0..3 {
            let mut frame = window.frame();
            set.draw(&mut frame, &view, 2, &theme);
        }
        assert!(!set.contains(far), "out of sight three frames");
        assert_eq!(set.len(), 1, "the one in sight stays");
    }
}

//! Pictures a frame draws besides text and fills: [`Image`]s, RGBA pixels
//! decoded from PNG, JPEG, GIF, WebP, BMP, or TIFF or made by the app, and
//! [`Svg`] documents, rasterized at the size they are drawn so an icon is
//! sharp at any scale. An [`AnimatedImage`] holds every frame of an
//! animated GIF or WebP with how long each shows, for the app to step
//! through. An [`IconSet`] names the SVG icons an app ships, for widgets
//! to draw by name in a theme color.
//!
//! Both kinds are cheap to clone (the pixels or the parsed document are
//! shared) and carry an identity the renderer caches their textures by:
//! an image is uploaded once, an SVG once per size it is drawn at.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frames, ImageFormat, RgbaImage, imageops};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};
use thiserror::Error;

use super::super::base::geometry::Rect;

// ========================================================================
// Constants
// ========================================================================

/// The bytes of one RGBA pixel.
pub(crate) const BYTES_PER_PIXEL: usize = 4;

/// The widest and tallest an SVG is rasterized, whatever its box asks.
const MAX_RASTER_SIDE: u32 = 4096;

// ========================================================================
// Data Structures
// ========================================================================

/// An RGBA image: 8 bits a channel, straight (not premultiplied) alpha,
/// row by row from the top left.
#[derive(Clone)]
pub struct Image {
    /// The height in pixels.
    height: u32,
    /// What the renderer caches the image's texture by.
    id: PictureId,
    /// The pixels.
    pixels: Arc<[u8]>,
    /// The width in pixels.
    width: u32,
}

/// How [`Frame::draw_image_as`](crate::Frame::draw_image_as) draws an
/// image: turned, mirrored, and with its pixels smoothed or kept sharp.
/// The default draws it as it is, smoothed.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ImageDraw {
    /// Whether it is mirrored left to right, after it is turned.
    pub flipped: bool,
    /// Whether each pixel is drawn as a sharp square, for an image zoomed
    /// in far enough to see them; smoothed between pixels otherwise.
    pub nearest: bool,
    /// How many quarter turns clockwise it is turned, 0 to 3 (more go
    /// round again). Turned by one or three, it fills a rect as wide as
    /// the image is tall.
    pub quarter_turns: u8,
}

/// Every frame of an animated GIF or WebP, each with how long it shows.
/// Draw [`AnimatedImage::frames`] one after another with
/// [`Frame::draw_image`](crate::Frame::draw_image), asking for the next
/// frame with [`EventCx::request_redraw_at`](crate::EventCx::request_redraw_at)
/// when the delay is up.
///
/// ```
/// use misty::AnimatedImage;
///
/// # fn gif_bytes() -> Vec<u8> { Vec::new() }
/// match AnimatedImage::decode(&gif_bytes()) {
///     Ok(animation) => {
///         let total = animation.frames().iter().map(|frame| frame.delay).sum::<std::time::Duration>();
///         println!("{} frames over {total:?}", animation.frames().len());
///     }
///     Err(error) => eprintln!("{error}"),
/// }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatedImage {
    /// The frames in the order they show; never empty.
    frames: Vec<AnimationFrame>,
}

/// One frame of an [`AnimatedImage`]: the whole picture as it shows, not
/// just what changed since the last frame.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationFrame {
    /// How long the frame shows before the next.
    pub delay: Duration,
    /// The picture.
    pub image: Image,
}

/// A parsed SVG document, drawn rasterized at the size it is drawn.
#[derive(Clone)]
pub struct Svg {
    /// What the renderer caches the document's rasters by.
    id: PictureId,
    /// The document.
    tree: Arc<Tree>,
}

/// SVG icons by name, as an app ships them, for widgets to draw.
#[derive(Clone, Debug, Default)]
pub struct IconSet {
    /// The names of the icons drawn in their own colors rather than tinted.
    colored: HashSet<String>,
    /// The icons.
    icons: HashMap<String, Svg>,
}

/// Why a picture could not be made.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ImageError {
    /// The bytes are not a PNG, JPEG, GIF, WebP, BMP, or TIFF image.
    #[error("could not decode the image: {0}")]
    Decode(String),
    /// The image has no pixels: a zero width or height.
    #[error("the image is empty")]
    Empty,
    /// The bytes are not an SVG document, or it has no size.
    #[error("could not read the SVG document: {0}")]
    Svg(String),
    /// A side is longer than the GPU's largest texture, `max_side` pixels:
    /// split the picture into tiles, or make it smaller and stretch it.
    #[error("a side is longer than the GPU's largest texture, {max_side} pixels")]
    TooLarge {
        /// The longest side the GPU takes, in pixels.
        max_side: u32,
    },
    /// The pixels are not four bytes for each pixel of the size given; the
    /// number is how many bytes there were.
    #[error("the RGBA pixels do not match the image size: {0} bytes")]
    WrongLength(usize),
}

/// Names one image or SVG document for as long as a clone of it lives.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PictureId(u64);

/// An SVG rasterized to straight-alpha RGBA, row by row.
pub(crate) struct Raster {
    /// The height in pixels.
    pub(crate) height: u32,
    /// The pixels.
    pub(crate) pixels: Vec<u8>,
    /// The width in pixels.
    pub(crate) width: u32,
}

// ========================================================================
// Image Implementations
// ========================================================================

impl Image {
    /// An image `width` by `height` pixels of RGBA `pixels`, four bytes a
    /// pixel with straight alpha, row by row from the top left.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::Empty);
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(BYTES_PER_PIXEL));
        if expected != Some(pixels.len()) {
            return Err(ImageError::WrongLength(pixels.len()));
        }
        Ok(Self {
            height,
            id: PictureId::next(),
            pixels: pixels.into(),
            width,
        })
    }

    /// The image a PNG, JPEG, GIF, WebP, BMP, or TIFF file holds: the first
    /// frame of an animated one (see [`AnimatedImage`] for them all).
    pub fn decode(bytes: &[u8]) -> Result<Self, ImageError> {
        let decoded = image::load_from_memory(bytes)
            .map_err(|error| ImageError::Decode(error.to_string()))?;
        let rgba = decoded.into_rgba8();
        Self::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())
    }

    /// The width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The largest rectangle of this image's shape inside `area`, centered
    /// in it: where to draw the image whole without stretching it.
    pub fn fit(&self, area: Rect) -> Rect {
        fit_rect(area, self.width as f32, self.height as f32)
    }

    /// This image scaled down, its shape kept, to at most `max_side`
    /// pixels wide and tall; itself when it fits already.
    pub(crate) fn shrunk_to(&self, max_side: u32) -> Self {
        if self.width <= max_side && self.height <= max_side {
            return self.clone();
        }
        let scale = max_side as f32 / self.width.max(self.height) as f32;
        let width = ((self.width as f32 * scale).round() as u32).max(1);
        let height = ((self.height as f32 * scale).round() as u32).max(1);
        let Some(rgba) = RgbaImage::from_raw(self.width, self.height, self.pixels.to_vec()) else {
            return self.clone();
        };
        let small = imageops::thumbnail(&rgba, width, height);
        Self::from_rgba(width, height, small.into_raw()).unwrap_or_else(|_| self.clone())
    }

    /// What the renderer caches the image's texture by.
    pub(crate) fn id(&self) -> PictureId {
        self.id
    }

    /// The pixels: four bytes a pixel, RGBA with straight alpha, row by
    /// row from the top left.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("height", &self.height)
            .field("id", &self.id)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Image {
    /// The same image: one made once and cloned.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Svg {}

// ========================================================================
// AnimatedImage Implementations
// ========================================================================

impl AnimatedImage {
    /// The frames of the GIF or WebP file `bytes` holds; any other image,
    /// or a WebP that is not animated, is one frame shown for no time.
    pub fn decode(bytes: &[u8]) -> Result<Self, ImageError> {
        let decode_error = |error: image::ImageError| ImageError::Decode(error.to_string());
        let frames = match image::guess_format(bytes) {
            Ok(ImageFormat::Gif) => {
                let decoder = GifDecoder::new(Cursor::new(bytes)).map_err(decode_error)?;
                animation_frames(decoder.into_frames())?
            }
            Ok(ImageFormat::WebP) => {
                let decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(decode_error)?;
                match decoder.has_animation() {
                    true => animation_frames(decoder.into_frames())?,
                    false => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        if !frames.is_empty() {
            return Ok(Self { frames });
        }
        let image = Image::decode(bytes)?;
        Ok(Self {
            frames: vec![AnimationFrame {
                delay: Duration::ZERO,
                image,
            }],
        })
    }

    /// The animation of `frames`, shown in order, as one drawn in code;
    /// `None` without a frame.
    pub fn from_frames(frames: Vec<AnimationFrame>) -> Option<Self> {
        (!frames.is_empty()).then_some(Self { frames })
    }

    /// The frames in the order they show, at least one.
    pub fn frames(&self) -> &[AnimationFrame] {
        &self.frames
    }
}

// ========================================================================
// Svg Implementations
// ========================================================================

impl Svg {
    /// The SVG document in `data`. Text and embedded raster images are not
    /// drawn: icons are shapes.
    pub fn parse(data: &[u8]) -> Result<Self, ImageError> {
        let tree = Tree::from_data(data, &Options::default())
            .map_err(|error| ImageError::Svg(error.to_string()))?;
        Ok(Self {
            id: PictureId::next(),
            tree: Arc::new(tree),
        })
    }

    /// The document's own width, in its user units.
    pub fn width(&self) -> f32 {
        self.tree.size().width()
    }

    /// The document's own height, in its user units.
    pub fn height(&self) -> f32 {
        self.tree.size().height()
    }

    /// What the renderer caches the document's rasters by.
    pub(crate) fn id(&self) -> PictureId {
        self.id
    }

    /// The document rasterized into a `width` by `height` pixel box,
    /// keeping its shape and centered in what room is left; `None` when no
    /// pixmap that size can be made.
    pub(crate) fn rasterize(&self, width: u32, height: u32) -> Option<Raster> {
        let mut pixmap = Pixmap::new(width, height)?;
        let (scale, x, y) =
            svg_placement((width as f32, height as f32), (self.width(), self.height()));
        let transform = Transform::from_translate(x, y).pre_scale(scale, scale);
        resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        let pixels = pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect();
        Some(Raster {
            height,
            pixels,
            width,
        })
    }
}

impl fmt::Debug for Svg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Svg")
            .field("height", &self.height())
            .field("id", &self.id)
            .field("width", &self.width())
            .finish_non_exhaustive()
    }
}

impl PartialEq for Svg {
    /// The same document: one parsed once and cloned.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

// ========================================================================
// IconSet Implementations
// ========================================================================

impl IconSet {
    /// No icons.
    pub fn new() -> Self {
        Self::default()
    }

    /// This set with the SVG document in `data` named `name`, replacing
    /// any icon of that name.
    pub fn with(mut self, name: impl Into<String>, data: &[u8]) -> Result<Self, ImageError> {
        self.insert(name, Svg::parse(data)?);
        Ok(self)
    }

    /// Names `svg` `name`, replacing any icon of that name. Widgets draw
    /// it tinted in their text's color, as a one-color icon is.
    pub fn insert(&mut self, name: impl Into<String>, svg: Svg) {
        let name = name.into();
        self.colored.remove(&name);
        self.icons.insert(name, svg);
    }

    /// Names `svg` `name`, replacing any icon of that name, drawn in its
    /// own colors: a file type's colored badge.
    pub fn insert_colored(&mut self, name: impl Into<String>, svg: Svg) {
        let name = name.into();
        self.colored.insert(name.clone());
        self.icons.insert(name, svg);
    }

    /// Whether the icon named `name` is drawn in its own colors.
    pub fn is_colored(&self, name: &str) -> bool {
        self.colored.contains(name)
    }

    /// The icon named `name`.
    pub fn get(&self, name: &str) -> Option<&Svg> {
        self.icons.get(name)
    }
}

// ========================================================================
// PictureId Implementations
// ========================================================================

impl PictureId {
    /// An id no picture had before.
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The largest rectangle of `width` by `height`'s shape inside `area`,
/// centered in it.
fn fit_rect(area: Rect, width: f32, height: f32) -> Rect {
    if width <= 0.0 || height <= 0.0 {
        return Rect::new(
            area.x + area.width / 2.0,
            area.y + area.height / 2.0,
            0.0,
            0.0,
        );
    }
    let scale = (area.width / width).min(area.height / height);
    let (fitted_width, fitted_height) = (width * scale, height * scale);
    Rect::new(
        area.x + (area.width - fitted_width) / 2.0,
        area.y + (area.height - fitted_height) / 2.0,
        fitted_width,
        fitted_height,
    )
}

/// The whole-pixel box an SVG drawn over `rect` is rasterized into: its
/// size rounded, at least a pixel and at most [`MAX_RASTER_SIDE`] a side,
/// at its top left corner rounded to the pixel grid, so each texel lands
/// on one pixel.
pub(crate) fn raster_box(rect: Rect) -> Rect {
    let side = |length: f32| (length.round() as u32).clamp(1, MAX_RASTER_SIDE) as f32;
    Rect::new(
        rect.x.round(),
        rect.y.round(),
        side(rect.width),
        side(rect.height),
    )
}

/// How an SVG `size` units large is placed in a raster `box_size` pixels
/// large: the scale keeping its shape, and the offset centering it.
fn svg_placement(box_size: (f32, f32), size: (f32, f32)) -> (f32, f32, f32) {
    let (box_width, box_height) = box_size;
    let (width, height) = size;
    let scale = (box_width / width).min(box_height / height);
    (
        scale,
        (box_width - width * scale) / 2.0,
        (box_height - height * scale) / 2.0,
    )
}

/// Every frame `frames` decodes, as images with their delays.
fn animation_frames(frames: Frames<'_>) -> Result<Vec<AnimationFrame>, ImageError> {
    frames
        .map(|frame| {
            let frame = frame.map_err(|error| ImageError::Decode(error.to_string()))?;
            let delay = Duration::from(frame.delay());
            let rgba = frame.into_buffer();
            let image = Image::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())?;
            Ok(AnimationFrame { delay, image })
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 by 1 unit wide rectangle, red.
    const WIDE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
        <rect width="20" height="10" fill="#ff0000"/></svg>"##;

    #[test]
    fn test_an_icon_put_in_again_plainly_is_tinted_once_more() {
        let svg = Svg::parse(b"<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'/>")
            .expect("an svg");
        let mut icons = IconSet::new();
        icons.insert_colored("rust", svg.clone());
        assert!(icons.is_colored("rust"));
        icons.insert("rust", svg);
        assert!(!icons.is_colored("rust"), "replaced by a plain icon");
        assert!(!icons.is_colored("missing"));
    }

    #[test]
    fn test_rgba_pixels_must_match_the_size_and_the_size_must_not_be_empty() {
        assert!(Image::from_rgba(2, 1, vec![0; 8]).is_ok());
        assert!(matches!(
            Image::from_rgba(2, 1, vec![0; 7]),
            Err(ImageError::WrongLength(7))
        ));
        assert!(matches!(
            Image::from_rgba(0, 1, Vec::new()),
            Err(ImageError::Empty)
        ));
        assert!(matches!(
            Image::decode(b"not an image"),
            Err(ImageError::Decode(_))
        ));
    }

    /// `rgba`, 2 by 1 pixels, encoded as `format`.
    fn encoded(rgba: &RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        rgba.write_to(&mut bytes, format).expect("encodes");
        bytes.into_inner()
    }

    #[test]
    fn test_every_still_format_decodes_to_its_pixels() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let rgba = RgbaImage::from_raw(2, 1, [red, blue].concat()).expect("2 by 1");
        for format in [
            ImageFormat::Bmp,
            ImageFormat::Gif,
            ImageFormat::Png,
            ImageFormat::Tiff,
            ImageFormat::WebP,
        ] {
            let image = Image::decode(&encoded(&rgba, format)).expect("decodes");
            assert_eq!((image.width(), image.height()), (2, 1), "{format:?}");
            assert_eq!(image.pixels(), [red, blue].concat(), "{format:?}");
            let still = AnimatedImage::decode(&encoded(&rgba, format)).expect("decodes");
            assert_eq!(still.frames().len(), 1, "{format:?}");
            assert_eq!(still.frames()[0].delay, Duration::ZERO, "{format:?}");
        }
    }

    #[test]
    fn test_an_animated_gif_gives_every_frame_with_its_delay() {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame};

        let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
        let delays = [100, 250, 40];
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            for (color, millis) in colors.iter().zip(delays) {
                let buffer = RgbaImage::from_raw(1, 1, color.to_vec()).expect("1 by 1");
                let delay = Delay::from_saturating_duration(Duration::from_millis(millis));
                encoder
                    .encode_frame(Frame::from_parts(buffer, 0, 0, delay))
                    .expect("encodes");
            }
        }
        let animation = AnimatedImage::decode(&bytes).expect("decodes");
        let frames = animation.frames();
        assert_eq!(frames.len(), colors.len());
        for ((frame, color), millis) in frames.iter().zip(colors).zip(delays) {
            assert_eq!(frame.image.pixels(), color);
            assert_eq!(frame.delay, Duration::from_millis(millis));
        }
        assert_eq!(
            Image::decode(&bytes).expect("decodes").pixels(),
            colors[0],
            "a still image is the first frame"
        );
    }

    #[test]
    fn test_clones_are_the_same_picture_and_new_ones_are_not() {
        let image = Image::from_rgba(1, 1, vec![0; 4]).expect("an image");
        let again = Image::from_rgba(1, 1, vec![0; 4]).expect("an image");
        assert_eq!(image.clone(), image);
        assert_ne!(image, again, "same pixels, different images");
    }

    #[test]
    fn test_fit_keeps_the_shape_and_centers_in_the_room_left() {
        let image = Image::from_rgba(4, 2, vec![0; 32]).expect("an image");
        let tall = image.fit(Rect::new(10.0, 10.0, 100.0, 100.0));
        assert_eq!(tall, Rect::new(10.0, 35.0, 100.0, 50.0));
        let wide = image.fit(Rect::new(0.0, 0.0, 300.0, 50.0));
        assert_eq!(wide, Rect::new(100.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn test_raster_boxes_snap_to_whole_pixels_within_limits() {
        let snapped = raster_box(Rect::new(10.4, 3.6, 15.5, 0.2));
        assert_eq!(snapped, Rect::new(10.0, 4.0, 16.0, 1.0));
        let huge = raster_box(Rect::new(0.0, 0.0, 10_000.0, 8.0));
        assert_eq!(huge.width, MAX_RASTER_SIDE as f32);
    }

    #[test]
    fn test_an_svg_fills_its_box_by_its_shape_centered() {
        let svg = Svg::parse(WIDE_SVG.as_bytes()).expect("an svg");
        assert_eq!((svg.width(), svg.height()), (20.0, 10.0));
        // 40 by 40: scaled 2, 20 pixels high, centered 10 down.
        assert_eq!(svg_placement((40.0, 40.0), (20.0, 10.0)), (2.0, 0.0, 10.0));
        let raster = svg.rasterize(40, 40).expect("a raster");
        let alpha = |x: u32, y: u32| raster.pixels[((y * 40 + x) as usize) * BYTES_PER_PIXEL + 3];
        assert_eq!(alpha(20, 5), 0, "above the shape");
        assert_eq!(alpha(20, 20), u8::MAX, "inside it");
        let red = &raster.pixels[(20 * 40 + 20) * BYTES_PER_PIXEL..][..3];
        assert_eq!(red, [255, 0, 0], "straight, not premultiplied");
        assert!(matches!(Svg::parse(b"<nope"), Err(ImageError::Svg(_))));
    }
}

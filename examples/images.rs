//! Pictures in a frame: a generated image and, when a PNG or JPEG path is
//! given, that file, each fitted whole into its half of the window; below
//! them, icons from an icon set drawn in theme colors at four sizes, and a
//! two-color SVG in its own colors. `+` and `-` grow and shrink the icons,
//! each size rasterized sharp and cached. Drop a PNG or JPEG file from a
//! file manager on the window to show it instead; the right half lights up
//! while files hover. Esc or the close button quits.
//!
//! ```sh
//! cargo run -p misty --example images -- picture.png
//! ```

use std::env;
use std::fs;
use std::path::Path;

use misty::widgets::{FileDrop, FileDropEvent, draw_drop_target};
use misty::{
    AppContext, Application, Error, Frame, IconSet, Image, Input, KeyCode, Point, Rect, StyledText,
    Svg, Theme, ThemeToken, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty images";

/// The side of the generated image, in pixels.
const GRADIENT_SIDE: u32 = 256;

/// The icon sizes drawn, in logical pixels, at the default zoom.
const ICON_SIZES: [f32; 4] = [16.0, 24.0, 32.0, 48.0];

/// How much `+` and `-` change the icon zoom.
const ZOOM_STEP: f32 = 0.25;

/// The smallest and largest icon zoom.
const ZOOM_RANGE: (f32, f32) = (0.5, 4.0);

/// The space between pictures and around them, in logical pixels.
const GAP: f32 = 16.0;

/// The share of the window's height the images take.
const IMAGES_SHARE: f32 = 0.6;

/// The icons of the demo, by name: one-color shapes drawn in a theme color.
const ICONS: [(&str, &str); 3] = [
    (
        "folder",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">
        <path d="M2 5h8l2 2h10v12H2z" fill="#000"/></svg>"##,
    ),
    (
        "file",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">
        <path d="M5 2h9l5 5v15H5z" fill="#000"/></svg>"##,
    ),
    (
        "circle",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">
        <circle cx="12" cy="12" r="9" fill="none" stroke="#000" stroke-width="2"/></svg>"##,
    ),
];

/// The theme colors the icons are drawn in, in order.
const ICON_COLORS: [ThemeToken; 3] = [
    ThemeToken::Accent,
    ThemeToken::Foreground,
    ThemeToken::Warning,
];

/// A two-color badge, drawn in its own colors.
const BADGE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="32" height="32">
    <rect width="32" height="32" rx="6" fill="#e64553"/>
    <path d="M9 16l5 5 9-10" fill="none" stroke="#ffffff" stroke-width="3"/></svg>"##;

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: two images, the icon set, and a badge.
struct Images {
    /// The two-color badge.
    badge: Svg,
    /// Files dragged in from other apps.
    file_drop: FileDrop,
    /// The generated image.
    gradient: Image,
    /// The icon set.
    icons: IconSet,
    /// The image read from the command line or dropped, or why there is
    /// none.
    loaded: Result<Image, String>,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The icons' zoom.
    zoom: f32,
}

// ========================================================================
// Images Implementations
// ========================================================================

impl Images {
    /// Draws `text` in muted color at `origin`, inside `clip`.
    fn caption(&self, frame: &mut Frame, text: &str, origin: Point, clip: Rect) {
        let text = StyledText::plain(text, ThemeToken::Muted.into()).with_ellipsis();
        frame.draw_text(&text, origin, clip, &self.theme);
    }
}

impl Application for Images {
    type Message = ();

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        if *input == Input::CloseRequested {
            cx.exit();
            return;
        }
        if let Some(file_event) = self.file_drop.handle_event(input) {
            if let FileDropEvent::Dropped(dropped) = file_event {
                self.loaded = load_path(&dropped.path);
            }
            cx.event_cx(window).request_redraw();
            return;
        }
        let Some(key) = input.key() else {
            return;
        };
        if key.code == KeyCode::Esc {
            cx.exit();
            return;
        }
        let step = if [KeyCode::Char('+'), KeyCode::Char('=')].contains(&key.code) {
            ZOOM_STEP
        } else if key.code == KeyCode::Char('-') {
            -ZOOM_STEP
        } else {
            return;
        };
        let (low, high) = ZOOM_RANGE;
        self.zoom = (self.zoom + step).clamp(low, high);
        cx.event_cx(window).request_redraw();
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let window = frame.bounds();
        let scale = frame.scale_factor() as f32;
        let metrics = frame.cell_metrics();
        let gap = GAP * scale;
        let images_height = window.height * IMAGES_SHARE;
        let half = (window.width - 3.0 * gap) / 2.0;
        let caption_height = metrics.height;
        let left = Rect::new(
            gap,
            gap + caption_height,
            half,
            images_height - caption_height,
        );
        let right = Rect::new(2.0 * gap + half, left.y, half, left.height);
        self.caption(frame, "generated", Point::new(left.x, gap), window);
        frame.draw_image(&self.gradient, self.gradient.fit(left));
        match &self.loaded {
            Ok(image) => {
                let caption = format!("{} x {}", image.width(), image.height());
                self.caption(frame, &caption, Point::new(right.x, gap), window);
                frame.draw_image(image, image.fit(right));
            }
            Err(why) => self.caption(frame, why, Point::new(right.x, gap), window),
        }
        if self.file_drop.is_hovering() {
            draw_drop_target(frame, right, true, &self.theme);
        }
        let mut x = gap;
        let top = gap + images_height + gap;
        for size in ICON_SIZES {
            let side = size * scale * self.zoom;
            for ((name, _), token) in ICONS.iter().zip(ICON_COLORS) {
                if let Some(icon) = self.icons.get(name) {
                    let rect = Rect::new(x, top, side, side);
                    frame.draw_icon(icon, rect, self.theme.color(token));
                }
                x += side + gap / 2.0;
            }
            x += gap;
        }
        let side = ICON_SIZES[ICON_SIZES.len() - 1] * scale * self.zoom;
        frame.draw_svg(&self.badge, Rect::new(x, top, side, side));
        let hint = format!("zoom {:.2}: + and - resize the icons, Esc quits", self.zoom);
        let y = window.height - gap - caption_height;
        self.caption(frame, &hint, Point::new(gap, y), window);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A square image fading from the accent color at the left to transparent
/// at the right, and darker toward the bottom.
fn gradient(theme: &Theme) -> Image {
    let accent = theme.accent;
    let side = GRADIENT_SIDE;
    let pixels = (0..side)
        .flat_map(|y| (0..side).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let shade = |channel: u8| (u32::from(channel) * (2 * side - y) / (2 * side)) as u8;
            let alpha = (u8::MAX as u32 * (side - x) / side) as u8;
            [shade(accent.r), shade(accent.g), shade(accent.b), alpha]
        })
        .collect();
    Image::from_rgba(side, side, pixels).expect("the pixels match the size")
}

/// The image at the path given on the command line, or why there is none.
fn load_argument() -> Result<Image, String> {
    let path = env::args_os()
        .nth(1)
        .ok_or_else(|| "pass or drop a PNG or JPEG file to show it here".to_owned())?;
    load_path(Path::new(&path))
}

/// The image in the file at `path`, or why it cannot be shown.
fn load_path(path: &Path) -> Result<Image, String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read the file: {error}"))?;
    Image::decode(&bytes).map_err(|error| error.to_string())
}

fn main() -> Result<(), Error> {
    let theme = Theme::dark();
    let icons = ICONS
        .iter()
        .try_fold(IconSet::new(), |set, (name, svg)| {
            set.with(*name, svg.as_bytes())
        })
        .expect("the demo's icons parse");
    let app = Images {
        badge: Svg::parse(BADGE.as_bytes()).expect("the badge parses"),
        file_drop: FileDrop::new(),
        gradient: gradient(&theme),
        icons,
        loaded: load_argument(),
        theme,
        zoom: 1.0,
    };
    misty::run(app, WindowOptions::new(TITLE))
}

//! Drag and drop: [`Drag`], a drag of some payload inside the app (a tab,
//! a list row) from the press that may start it to the release that drops
//! it, and [`FileDrop`], files dragged into a window from other apps.
//! [`draw_drop_target`] marks where a drop would land.
//!
//! A widget arms a [`Drag`] on a press over something draggable; the drag
//! starts only once the pointer has moved a few pixels with the button
//! held, so a plain click stays a click. While it runs, a label follows the
//! pointer ([`Drag::draw`]) and drop targets ask where it is
//! ([`Drag::position`]) and what it carries ([`Drag::payload`]). Releasing
//! the button drops it ([`DragResponse::Dropped`]); Escape or losing focus
//! cancels it. Dragging out of the window into another app is not
//! supported.

use std::path::PathBuf;

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::icon::{logical, outline};
use super::popup::{Popup, PopupSide};

// ========================================================================
// Constants
// ========================================================================

/// How far the pointer must move with the button held before a press
/// becomes a drag, in logical pixels.
const DRAG_THRESHOLD: f32 = 4.0;

/// The most cells a drag label shows before it is cut with an ellipsis.
const LABEL_MAX_CELLS: usize = 32;

/// How far the drag label sits right of and below the pointer, in cells.
const LABEL_OFFSET_CELLS: f32 = 1.0;

/// How opaque the wash over a drop target is, out of 255; doubled while
/// the drop would land on it.
const TARGET_ALPHA: u8 = 40;

/// The thickness of a drop target's outline, in logical pixels.
const TARGET_STROKE: f32 = 2.0;

/// The color of a drag label's text.
const LABEL_COLOR: TextColor = TextColor::Token(ThemeToken::Foreground);

// ========================================================================
// Data Structures
// ========================================================================

/// A drag inside the app carrying a payload of type `P`: idle, armed by a
/// press, or running.
#[derive(Clone, Debug)]
pub struct Drag<P> {
    /// The armed or running drag, or `None` while idle.
    state: Option<DragState<P>>,
}

/// What became of a window event offered to a [`Drag`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DragResponse<P> {
    /// The running drag was cancelled: nothing is dropped.
    Cancelled,
    /// The drag was dropped here.
    Dropped(Dropped<P>),
    /// Nothing is being dragged, or an armed press ended as a plain click;
    /// the event is the app's as usual.
    Ignored,
    /// The running drag followed the pointer; drop targets may light up.
    Moved,
    /// The press became a drag.
    Started,
}

/// A payload dropped at a point.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped<P> {
    /// What was dragged.
    pub payload: P,
    /// Where the button was released, in physical pixels.
    pub position: Point,
}

/// Files dragged into a window from other apps, as winit reports them one
/// event per file, with the pointer position last seen in the window.
///
/// winit reports no pointer motion during a drag from another app on most
/// platforms, so the position is where the pointer was last seen; it is
/// exact only once the pointer has moved in the window, and `None` before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileDrop {
    /// The files hovering now, in the order reported.
    hovered: Vec<PathBuf>,
    /// The pointer position last seen in the window.
    pointer: Option<Point>,
}

/// What a window event meant for files dragged in from other apps.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum FileDropEvent {
    /// The files left the window or the drag was abandoned.
    Cancelled,
    /// One file was dropped; a drop of several files reports each in turn.
    Dropped(FileDropped),
    /// Files hover over the window.
    Hovered(FileHover),
}

/// A file dropped on the window.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDropped {
    /// The file.
    pub path: PathBuf,
    /// Where it was dropped, as [`FileDrop`] describes.
    pub position: Option<Point>,
}

/// Files hovering over the window.
#[derive(Clone, Debug, PartialEq)]
pub struct FileHover {
    /// Every file hovering so far.
    pub paths: Vec<PathBuf>,
    /// Where they hover, as [`FileDrop`] describes.
    pub position: Option<Point>,
}

/// An armed or running drag.
#[derive(Clone, Debug, PartialEq)]
struct DragState<P> {
    /// Whether the press has become a drag.
    dragging: bool,
    /// Where the press was.
    origin: Point,
    /// What is dragged.
    payload: P,
    /// Where the pointer is.
    pointer: Point,
    /// How far, in physical pixels, the pointer must move to start it.
    threshold: f32,
}

// ========================================================================
// Drag Implementations
// ========================================================================

impl<P: Clone> Drag<P> {
    /// No drag.
    pub fn new() -> Self {
        Self { state: None }
    }

    /// Arms a drag of `payload` from a press at `at` on a display scaled by
    /// `scale_factor`; it starts once the pointer moves far enough with the
    /// button held. An earlier armed or running drag is forgotten.
    pub fn arm(&mut self, payload: P, at: Point, scale_factor: f64) {
        self.state = Some(DragState {
            dragging: false,
            origin: at,
            payload,
            pointer: at,
            threshold: logical(DRAG_THRESHOLD, scale_factor),
        });
    }

    /// Follows `input` to the window `cx` stands for: pointer motion starts and moves the
    /// drag, a left release drops it, and Escape or losing focus cancels
    /// it. Offer it every input while a drag may be armed.
    pub fn handle_event(&mut self, input: &Input, cx: &mut EventCx<'_>) -> DragResponse<P> {
        let escape = Key::new(KeyCode::Esc, Mods::NONE);
        let response = match input {
            Input::FocusLost => self.cancel(),
            Input::Key(key) if *key == escape => self.cancel(),
            Input::PointerButton(button)
                if !button.pressed && button.button == PointerButton::Left =>
            {
                self.release()
            }
            Input::PointerMoved(position) => self.move_pointer(*position),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::Wheel(_) => DragResponse::Ignored,
        };
        if !matches!(response, DragResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Whether a drag is running, past its threshold.
    pub fn is_dragging(&self) -> bool {
        self.running().is_some()
    }

    /// What the running drag carries.
    pub fn payload(&self) -> Option<&P> {
        self.running().map(|state| &state.payload)
    }

    /// Where the pointer of the running drag is.
    pub fn position(&self) -> Option<Point> {
        self.running().map(|state| state.pointer)
    }

    /// Cancels the drag: [`DragResponse::Cancelled`] when one was running,
    /// else [`DragResponse::Ignored`] (an armed press is just forgotten).
    pub fn cancel(&mut self) -> DragResponse<P> {
        match self.state.take() {
            Some(state) if state.dragging => DragResponse::Cancelled,
            Some(_) | None => DragResponse::Ignored,
        }
    }

    /// Draws `label` in a small box beside the pointer of the running drag,
    /// in the overlay; nothing while no drag runs.
    pub fn draw(&self, frame: &mut Frame, label: &str, theme: &Theme) {
        let Some(pointer) = self.position() else {
            return;
        };
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let text = StyledText::plain(label, LABEL_COLOR)
            .with_ellipsis()
            .fit(LABEL_MAX_CELLS);
        let inset = Popup::inset(&theme.shape, scale_factor);
        let padding = metrics.width.round();
        let width = text.width() as f32 * metrics.width + 2.0 * (padding + inset);
        let height = metrics.height + 2.0 * inset;
        let offset = LABEL_OFFSET_CELLS * metrics.width;
        let anchor = Rect::new(pointer.x + offset, pointer.y + offset, 0.0, 0.0);
        let popup = Popup::new(
            anchor,
            width.ceil(),
            height,
            PopupSide::Below,
            frame.bounds(),
        );
        popup.draw(frame, theme);
        let content = popup.content(&theme.shape, scale_factor);
        frame.overlay(|frame| {
            frame.draw_text(
                &text,
                Point::new(content.x + padding, content.y),
                content,
                theme,
            );
        });
    }

    /// The pointer moved to `point`: an armed press past its threshold
    /// starts, a running drag moves.
    fn move_pointer(&mut self, point: Point) -> DragResponse<P> {
        let Some(state) = &mut self.state else {
            return DragResponse::Ignored;
        };
        state.pointer = point;
        if state.dragging {
            return DragResponse::Moved;
        }
        let (dx, dy) = (point.x - state.origin.x, point.y - state.origin.y);
        match dx.hypot(dy) >= state.threshold {
            true => {
                state.dragging = true;
                DragResponse::Started
            }
            false => DragResponse::Ignored,
        }
    }

    /// The button was released: a running drag drops, an armed press is
    /// forgotten.
    fn release(&mut self) -> DragResponse<P> {
        match self.state.take() {
            Some(state) if state.dragging => DragResponse::Dropped(Dropped {
                payload: state.payload,
                position: state.pointer,
            }),
            Some(_) | None => DragResponse::Ignored,
        }
    }

    /// The running drag.
    fn running(&self) -> Option<&DragState<P>> {
        self.state.as_ref().filter(|state| state.dragging)
    }
}

impl<P: Clone> Default for Drag<P> {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// FileDrop Implementations
// ========================================================================

impl FileDrop {
    /// Nothing hovering.
    pub fn new() -> Self {
        Self::default()
    }

    /// What `input` means for files dragged in from other apps, or `None`
    /// when it is about something else. Offer it every input.
    pub fn handle_event(&mut self, input: &Input) -> Option<FileDropEvent> {
        match input {
            Input::FileDropped(file) => {
                self.hovered.clear();
                self.pointer = file.position;
                Some(FileDropEvent::Dropped(FileDropped {
                    path: file.path.clone(),
                    position: file.position,
                }))
            }
            Input::FileHovered(file) => {
                self.hovered.push(file.path.clone());
                self.pointer = file.position;
                Some(FileDropEvent::Hovered(FileHover {
                    paths: self.hovered.clone(),
                    position: file.position,
                }))
            }
            Input::FileHoverCancelled => {
                self.hovered.clear();
                Some(FileDropEvent::Cancelled)
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => None,
        }
    }

    /// Whether files hover over the window now.
    pub fn is_hovering(&self) -> bool {
        !self.hovered.is_empty()
    }

    /// The files hovering now.
    pub fn hovered(&self) -> &[PathBuf] {
        &self.hovered
    }

    /// Where the pointer was last seen in the window.
    pub fn position(&self) -> Option<Point> {
        self.pointer
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Marks `area` as a place a drag may drop: a wash and an outline in the
/// accent color, the wash stronger when `is_over` (the drop would land
/// here). Drawn in the overlay, over the content it marks.
pub fn draw_drop_target(frame: &mut Frame, area: Rect, is_over: bool, theme: &Theme) {
    let stroke = logical(TARGET_STROKE, frame.scale_factor());
    let alpha = match is_over {
        true => TARGET_ALPHA.saturating_mul(2),
        false => TARGET_ALPHA,
    };
    let accent = theme.accent;
    frame.overlay(|frame| {
        frame.fill_rect(area, accent.with_alpha(alpha));
        for edge in outline(area, stroke) {
            frame.fill_rect_over_text(edge, accent);
        }
    });
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::input::event::FileInput;

    fn armed() -> Drag<&'static str> {
        let mut drag = Drag::new();
        drag.arm("tab", Point::new(10.0, 10.0), 1.0);
        drag
    }

    #[test]
    fn test_a_press_becomes_a_drag_only_past_the_threshold() {
        let mut drag = armed();
        assert_eq!(
            drag.move_pointer(Point::new(12.0, 11.0)),
            DragResponse::Ignored
        );
        assert!(!drag.is_dragging(), "a jitter is still a click");
        assert_eq!(
            drag.move_pointer(Point::new(14.0, 10.0)),
            DragResponse::Started
        );
        assert_eq!(
            drag.move_pointer(Point::new(40.0, 30.0)),
            DragResponse::Moved
        );
        assert_eq!(drag.payload(), Some(&"tab"));
        assert_eq!(drag.position(), Some(Point::new(40.0, 30.0)));
        assert_eq!(
            drag.release(),
            DragResponse::Dropped(Dropped {
                payload: "tab",
                position: Point::new(40.0, 30.0),
            })
        );
        assert!(!drag.is_dragging());
        assert_eq!(
            drag.release(),
            DragResponse::Ignored,
            "nothing left to drop"
        );
    }

    #[test]
    fn test_a_release_before_the_threshold_is_a_click_and_drops_nothing() {
        let mut drag = armed();
        drag.move_pointer(Point::new(11.0, 10.0));
        assert_eq!(drag.release(), DragResponse::Ignored);
        assert_eq!(
            drag.move_pointer(Point::new(90.0, 90.0)),
            DragResponse::Ignored
        );
    }

    #[test]
    fn test_the_threshold_scales_with_the_display() {
        let mut drag = Drag::new();
        drag.arm(1, Point::new(0.0, 0.0), 2.0);
        assert_eq!(
            drag.move_pointer(Point::new(6.0, 0.0)),
            DragResponse::Ignored
        );
        assert_eq!(
            drag.move_pointer(Point::new(8.0, 0.0)),
            DragResponse::Started
        );
    }

    #[test]
    fn test_cancelling_reports_only_a_running_drag() {
        let mut drag = armed();
        assert_eq!(drag.cancel(), DragResponse::Ignored, "only armed");
        let mut drag = armed();
        drag.move_pointer(Point::new(50.0, 10.0));
        assert_eq!(drag.cancel(), DragResponse::Cancelled);
        assert_eq!(drag.release(), DragResponse::Ignored);
    }

    #[test]
    fn test_file_drops_report_each_file_with_the_last_pointer_position() {
        let mut drop = FileDrop::new();
        let (a, b) = (PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.png"));
        let file = |path: &PathBuf| FileInput {
            path: path.clone(),
            position: Some(Point::new(5.0, 6.0)),
        };
        drop.handle_event(&Input::FileHovered(file(&a)));
        let hovered = drop.handle_event(&Input::FileHovered(file(&b)));
        assert_eq!(
            hovered,
            Some(FileDropEvent::Hovered(FileHover {
                paths: vec![a.clone(), b.clone()],
                position: Some(Point::new(5.0, 6.0)),
            }))
        );
        assert!(drop.is_hovering());
        let dropped = drop.handle_event(&Input::FileDropped(file(&a)));
        assert_eq!(
            dropped,
            Some(FileDropEvent::Dropped(FileDropped {
                path: a.clone(),
                position: Some(Point::new(5.0, 6.0)),
            }))
        );
        assert!(!drop.is_hovering(), "a drop ends the hover");
        drop.handle_event(&Input::FileHovered(file(&a)));
        assert_eq!(
            drop.handle_event(&Input::FileHoverCancelled),
            Some(FileDropEvent::Cancelled)
        );
        assert!(drop.hovered().is_empty());
    }

    #[test]
    fn test_a_drop_target_is_outlined_in_the_accent_and_washed_over_its_content() {
        let theme = Theme::dark();
        let area = Rect::new(8.0, 8.0, 48.0, 32.0);
        let Some(snapshot) = offscreen::render(64, 48, |frame| {
            frame.clear(Rgba::from(theme.background));
            draw_drop_target(frame, area, true, &theme);
        }) else {
            return;
        };
        snapshot.assert_pixel(8.0, 20.0, theme.accent, "the outline");
        let inside = snapshot.pixel(30.0, 24.0);
        assert_ne!(inside, theme.background, "the wash tints the content");
        assert_ne!(inside, theme.accent, "the content shows through");
        snapshot.assert_pixel(2.0, 2.0, theme.background, "outside");
    }
}

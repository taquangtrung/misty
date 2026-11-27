//! The properties any element takes beside what it is: how it is sized
//! and spaced ([`Extent`], [`Space`]), the surface drawn under it from
//! the theme ([`Corners`]), what the pointer does over it ([`Press`]),
//! what it drags and takes when dropped, and the size it was laid out at
//! ([`ElementSize`]). [`Extras`] holds them for one
//! element, boxed so an element without any stays small.
//!
//! Sizes come in three units. **Cells** (cell columns across, rows down)
//! are the unit of text and of the rest of the tree (padding, a form's
//! label column), so an element sized in cells keeps its place beside text
//! whatever the font size: use them by default. **Pixels** are logical
//! pixels, multiplied by the display's scale factor, for what is drawn
//! rather than written (a thumbnail, a page, a color swatch). A
//! **fraction** is a share of the parent's size.

use std::any::Any;
use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use super::super::base::geometry::{Point, Rect};
use super::super::base::key::Mods;
use super::super::base::theme::ThemeToken;
use super::super::input::event::{PointerButton, WheelInput};
use super::super::render::font::CellMetrics;
use super::super::render::layout::{Align, Edges, Justify, Length};
use super::super::widgets::drag::FileDropped;
use super::controls::Callback;

// ========================================================================
// Data Structures
// ========================================================================

/// A width or a height an element is held to: set with
/// [`Element::width`](crate::Element::width) and the like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Extent {
    /// As large as its content, or stretched across its container: what
    /// an element is without one.
    Auto,
    /// This many cell columns across, or cell rows down: the unit of text,
    /// to use by default.
    Cells(f32),
    /// This share (0 to 1) of its container.
    Fraction(f32),
    /// This many logical pixels, scaled by the display's scale factor, for
    /// what is drawn rather than written.
    Px(f32),
}

/// A space between or around elements: set with
/// [`Element::gap`](crate::Element::gap) and
/// [`Element::padding_px`](crate::Element::padding_px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Space {
    /// This many cell columns across, or rows down.
    Cells(f32),
    /// This many logical pixels, scaled by the display's scale factor.
    Px(f32),
}

/// How round the corners of an element's surface are, from the theme's
/// [`Shape`](crate::Shape): set with
/// [`Element::corners`](crate::Element::corners).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Corners {
    /// A box's corners ([`Shape::corner_radius`](crate::Shape)): a panel,
    /// a card. The default.
    #[default]
    Box,
    /// A control's corners ([`Shape::control_radius`](crate::Shape)): a
    /// chip, a row's highlight.
    Control,
    /// Square.
    Square,
}

/// A press on an element with
/// [`Element::on_press`](crate::Element::on_press): a pointer button, or
/// Enter or Space while it has the keyboard; with
/// [`Element::on_pointer_press`](crate::Element::on_pointer_press), a
/// pointer button only.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Press {
    /// The button pressed; [`PointerButton::Left`] for a key.
    pub button: PointerButton,
    /// Whether a key (Enter or Space) or a screen reader pressed it rather
    /// than the pointer.
    pub by_key: bool,
    /// How many presses of the button in a row this one makes (2 for a
    /// double click), as [`EventCx::click_count`](crate::EventCx::click_count)
    /// counts them; 1 for a key.
    pub clicks: u8,
    /// The modifiers held.
    pub mods: Mods,
    /// Where it was pressed, from the element's top left corner in
    /// physical pixels; the element's middle for a key.
    pub point: Point,
}

/// The size an element was laid out at, as
/// [`Element::on_size_change`](crate::Element::on_size_change) reports it:
/// in physical pixels, with the display's scale factor to turn them into
/// the logical pixels of [`Extent::Px`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ElementSize {
    /// How tall it is, in physical pixels.
    pub height: f32,
    /// The display's scale factor: physical pixels to a logical one.
    pub scale_factor: f64,
    /// How wide it is, in physical pixels.
    pub width: f32,
}

/// The pointer moving with the left button held after a press on an
/// element with
/// [`Element::on_pointer_drag`](crate::Element::on_pointer_drag), every
/// point from the element's top left corner in physical pixels: a stroke
/// on a canvas, a selection rubber band, a handle being moved.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PointerDrag {
    /// The button held: [`PointerButton::Left`], or
    /// [`PointerButton::Middle`] for a middle drag (a pan), which a
    /// callback that only selects or strokes leaves alone.
    pub button: PointerButton,
    /// Whether the button was released here, ending the drag.
    pub done: bool,
    /// The modifiers held.
    pub mods: Mods,
    /// Where the pointer is now; it may lie outside the element, which
    /// keeps the pointer until the button is released.
    pub point: Point,
    /// Where the button was pressed.
    pub start: Point,
}

/// What one element takes beside what it is, every property unset until
/// the app sets it.
pub(crate) struct Extras<'a, A> {
    /// The name screen readers call it by, when it is pressable.
    pub(crate) access_name: Option<&'a str>,
    /// What dragging it carries, if it can be dragged.
    pub(crate) drag: Option<DragSource<'a>>,
    /// What dropping a dragged element on it takes, if anything.
    pub(crate) drop: Option<DropTarget<'a, A>>,
    /// What dropping a file from another app on it runs.
    pub(crate) file_drop: Option<Callback<A, FileDropped>>,
    /// Whether the scrolling container around it keeps it in view.
    pub(crate) keep_in_view: bool,
    /// What the pointer coming over it (`true`) and leaving runs.
    pub(crate) on_hover: Option<Callback<A, bool>>,
    /// What the pointer moving with the left button held after a press on
    /// it runs.
    pub(crate) on_pointer_drag: Option<Callback<A, PointerDrag>>,
    /// What the pointer moving over it, and leaving it, runs.
    pub(crate) on_pointer_move: Option<Callback<A, Option<Point>>>,
    /// What a pointer press on it runs, keys and screen readers aside.
    pub(crate) on_pointer_press: Option<Callback<A, Press>>,
    /// What a press on it runs.
    pub(crate) on_press: Option<Callback<A, Press>>,
    /// What a turn of the wheel over it runs.
    pub(crate) on_scroll: Option<Callback<A, WheelInput>>,
    /// What a lazy column showing other rows than last reported runs.
    pub(crate) on_rows_shown: Option<Callback<A, Range<usize>>>,
    /// What being laid out at another size than last reported runs.
    pub(crate) on_size_change: Option<Callback<A, ElementSize>>,
    /// How it is sized and spaced.
    pub(crate) sizing: Sizing,
    /// What is drawn under it.
    pub(crate) surface: Surface,
}

/// How an element is sized and spaced, beyond what its place gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sizing {
    /// How a container's children line up across its direction.
    pub(crate) align: Option<Align>,
    /// The space between a container's children.
    pub(crate) gap: Option<Space>,
    /// Its share of the room its container leaves along its direction.
    pub(crate) grow: Option<f32>,
    /// Its height.
    pub(crate) height: Option<Extent>,
    /// How a container's children spread along its direction.
    pub(crate) justify: Option<Justify>,
    /// The tallest it may be.
    pub(crate) max_height: Option<Extent>,
    /// The widest it may be.
    pub(crate) max_width: Option<Extent>,
    /// The shortest it may be.
    pub(crate) min_height: Option<Extent>,
    /// The narrowest it may be.
    pub(crate) min_width: Option<Extent>,
    /// Whether it has the theme's page inset ([`Spacing::page_inset`]) at
    /// its left and right, on top of its other padding.
    ///
    /// [`Spacing::page_inset`]: crate::Spacing::page_inset
    pub(crate) page_inset: bool,
    /// The blank space inside it in logical pixels, across and down.
    pub(crate) padding_px: Option<(f32, f32)>,
    /// How much it gives up when its siblings do not fit.
    pub(crate) shrink: Option<f32>,
    /// Its width.
    pub(crate) width: Option<Extent>,
}

/// What is drawn under an element, in the theme's colors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Surface {
    /// The fill.
    pub(crate) background: Option<ThemeToken>,
    /// The line around it, as thick as the theme's boxes.
    pub(crate) border: Option<ThemeToken>,
    /// How round its corners are.
    pub(crate) corners: Corners,
    /// The fill while the pointer is over it.
    pub(crate) hover_background: Option<ThemeToken>,
    /// The line around it while the pointer is over it, in place of a
    /// hover fill: the pointer outlines it and its own fill stays.
    pub(crate) hover_border: Option<ThemeToken>,
    /// Whether it stays unlit under the pointer though it can be pressed:
    /// what shows its own answer to the pointer, as a field its caret.
    pub(crate) unlit: bool,
}

/// What dragging an element carries.
pub(crate) struct DragSource<'a> {
    /// What the label following the pointer says.
    pub(crate) label: Cow<'a, str>,
    /// The modifiers a press must hold to start it, if any: such a press
    /// goes to no other pointer callback of the element.
    pub(crate) mods: Option<Mods>,
    /// What a drop target is given.
    pub(crate) payload: Payload,
}

/// What dropping a dragged element on an element takes.
pub(crate) struct DropTarget<'a, A> {
    /// Whether it takes a payload.
    pub(crate) accepts: Accepts<'a>,
    /// Which part of it a drag over it marks, given where it was drawn
    /// and the pointer from its top left: all of it without one.
    pub(crate) marker: Option<Marker>,
    /// What a drop runs, with the payload and the point, from the target's
    /// top left corner.
    pub(crate) on_drop: OnDrop<A>,
}

/// Which part of a drop target a drag marks.
pub(crate) type Marker = Rc<dyn Fn(Rect, Point) -> Rect>;

/// What a drag carries, of the type its source and its target agree on.
pub(crate) type Payload = Rc<dyn Any>;

/// Whether a drop target takes a payload.
pub(crate) type Accepts<'a> = Box<dyn Fn(&dyn Any) -> bool + 'a>;

/// What dropping a payload on a target runs, with the point from the
/// target's top left corner.
pub(crate) type OnDrop<A> = Rc<dyn Fn(&mut A, &dyn Any, Point)>;

// ========================================================================
// ElementSize Implementations
// ========================================================================

impl ElementSize {
    /// How wide it is in logical pixels, the unit of [`Extent::Px`].
    pub fn logical_width(&self) -> f32 {
        self.width / self.scale_factor as f32
    }

    /// How tall it is in logical pixels, the unit of [`Extent::Px`].
    pub fn logical_height(&self) -> f32 {
        self.height / self.scale_factor as f32
    }
}

// ========================================================================
// Extent Implementations
// ========================================================================

impl Extent {
    /// This extent as a layout length across (`across`) or down, in a
    /// frame of `metrics` and `scale`.
    pub(crate) fn length(self, across: bool, metrics: CellMetrics, scale: f64) -> Length {
        match self {
            Self::Auto => Length::Auto,
            Self::Cells(cells) => Length::Fixed(cells * cell_size(across, metrics)),
            Self::Fraction(share) => Length::Percent(share),
            Self::Px(pixels) => Length::Fixed(pixels * scale as f32),
        }
    }
}

// ========================================================================
// Space Implementations
// ========================================================================

impl Space {
    /// This space in physical pixels across (`across`) or down, in a frame
    /// of `metrics` and `scale`.
    pub(crate) fn pixels(self, across: bool, metrics: CellMetrics, scale: f64) -> f32 {
        match self {
            Self::Cells(cells) => cells * cell_size(across, metrics),
            Self::Px(pixels) => pixels * scale as f32,
        }
    }
}

// ========================================================================
// Extras Implementations
// ========================================================================

impl<A> Extras<'_, A> {
    /// Whether the element takes the pointer, or the keyboard for a press,
    /// in a region of its own.
    pub(crate) fn has_region(&self) -> bool {
        self.on_press.is_some()
            || self.on_hover.is_some()
            || self.on_scroll.is_some()
            || self.on_pointer_drag.is_some()
            || self.on_pointer_move.is_some()
            || self.on_pointer_press.is_some()
            || self.drag.is_some()
            || self.drop.is_some()
            || self.file_drop.is_some()
            || self.surface.hover_background.is_some()
            || self.surface.hover_border.is_some()
    }

    /// The fill the element takes while the pointer is over it: the one it
    /// asked for, else the theme's hover light for anything it can press,
    /// so every control answers the pointer alike (see
    /// [`crate::testing::unlit_controls`]).
    pub(crate) fn hover_fill(&self) -> Option<ThemeToken> {
        let pressable = (self.on_press.is_some() || self.on_pointer_press.is_some())
            && !self.surface.unlit
            && self.surface.hover_border.is_none();
        self.surface
            .hover_background
            .or(pressable.then_some(ThemeToken::Hover))
    }

    /// Whether anything about it changes under the pointer.
    pub(crate) fn lights_on_hover(&self) -> bool {
        self.hover_fill().is_some() || self.surface.hover_border.is_some()
    }
}

impl<A> Default for Extras<'_, A> {
    fn default() -> Self {
        Self {
            access_name: None,
            drag: None,
            drop: None,
            file_drop: None,
            keep_in_view: false,
            on_hover: None,
            on_pointer_drag: None,
            on_pointer_move: None,
            on_pointer_press: None,
            on_press: None,
            on_scroll: None,
            on_rows_shown: None,
            on_size_change: None,
            sizing: Sizing::default(),
            surface: Surface::default(),
        }
    }
}

// ========================================================================
// Sizing Implementations
// ========================================================================

impl Sizing {
    /// The padding in logical pixels, the theme's `page_inset` among it
    /// where it takes it, in physical pixels at `scale`.
    pub(crate) fn padding(&self, page_inset: u16, scale: f64) -> Edges {
        let (mut across, down) = self.padding_px.unwrap_or((0.0, 0.0));
        if self.page_inset {
            across += f32::from(page_inset);
        }
        let scale = scale as f32;
        Edges::symmetric(across * scale, down * scale)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How wide a cell column is (`across`) or how tall a row is, in
/// `metrics`.
fn cell_size(across: bool, metrics: CellMetrics) -> f32 {
    match across {
        true => metrics.width,
        false => metrics.height,
    }
}

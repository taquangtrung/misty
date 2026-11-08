//! Box layout for dialogs and panels: a small tree of [`Element`]s (rows,
//! columns, stacks, and leaves) with sizes, padding, margins, gaps, and
//! alignment, laid out by flexbox (through `taffy`) into a [`Layout`] of
//! pixel rectangles that widgets then draw into.
//!
//! Lengths are physical pixels, like every [`Rect`]. An element the app
//! wants to find again gets a key ([`Element::with_key`]), and
//! [`Layout::rect`] hands back where it landed. A leaf whose height
//! depends on its width, such as wrapped text, measures itself
//! ([`Element::measured`]).

use std::collections::HashMap;
use std::hash::Hash;

use taffy::{
    AlignItems, AvailableSpace, Dimension, FlexDirection, JustifyContent, LengthPercentage,
    LengthPercentageAuto, NodeId, Position, Size, TaffyError, TaffyTree, compute_leaf_layout,
};

use super::super::base::geometry::Rect;

// ========================================================================
// Data Structures
// ========================================================================

/// One box of a layout tree: a container of children or a leaf, with its
/// [`Style`] and an optional key of type `K` to find it by.
pub struct Element<'a, K> {
    /// The boxes inside, in order.
    children: Vec<Element<'a, K>>,
    /// How the children are placed.
    kind: ElementKind,
    /// What the layout knows it by, if anything.
    key: Option<K>,
    /// How a leaf measures its height for a width, if it does.
    measure: Option<Box<Measure<'a>>>,
    /// Its size, spacing, and alignment.
    style: Style,
}

/// A leaf's height for a given width.
pub type Measure<'a> = dyn Fn(f32) -> f32 + 'a;

/// How an element is sized, spaced, and aligns its children. Start from
/// [`Style::new`] and change what you need with the `with_` methods.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    /// How children line up across the container's direction.
    align: Align,
    /// The space between children.
    gap: f32,
    /// The height.
    height: Length,
    /// How children spread along the container's direction.
    justify: Justify,
    /// The space outside the element.
    margin: Edges,
    /// The space inside the element, around its children.
    padding: Edges,
    /// The width.
    width: Length,
}

/// A width or a height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// As large as the content needs, or, across a container's direction,
    /// stretched to the container when it aligns with [`Align::Stretch`].
    #[default]
    Auto,
    /// A share of the room left along the container's direction, by this
    /// weight against the other flexible children; across it, the whole
    /// container.
    Flex(f32),
    /// This many pixels.
    Fixed(f32),
    /// This fraction (0 to 1) of the container.
    Percent(f32),
}

/// Space on each side of a box, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    /// Below.
    pub bottom: f32,
    /// At the left.
    pub left: f32,
    /// At the right.
    pub right: f32,
    /// Above.
    pub top: f32,
}

/// Where children sit across a container's direction (vertically in a
/// row, horizontally in a column).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Align {
    /// Centered.
    Center,
    /// At the bottom or right.
    End,
    /// At the top or left.
    Start,
    /// Stretched across the container, unless sized.
    #[default]
    Stretch,
}

/// How children spread along a container's direction.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Justify {
    /// Packed in the middle.
    Center,
    /// Packed at the end.
    End,
    /// The first at the start, the last at the end, the rest spaced
    /// evenly between.
    SpaceBetween,
    /// Packed at the start.
    #[default]
    Start,
}

/// Where the keyed elements of a tree landed.
#[derive(Clone, Debug)]
pub struct Layout<K> {
    /// Each keyed element's rectangle.
    rects: HashMap<K, Rect>,
}

/// How a container places its children.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ElementKind {
    /// Top to bottom.
    Column,
    /// No children.
    Leaf,
    /// Left to right.
    Row,
    /// Over each other, each filling the container unless sized.
    Stack,
}

// ========================================================================
// Element Implementations
// ========================================================================

impl<'a, K: Copy + Eq + Hash> Element<'a, K> {
    /// A box with no children, sized by `style`.
    pub fn leaf(style: Style) -> Self {
        Self::of_kind(ElementKind::Leaf, style, Vec::new())
    }

    /// A leaf whose height `measure` gives for the width it is laid out
    /// at, as wrapped text needs.
    pub fn measured(style: Style, measure: impl Fn(f32) -> f32 + 'a) -> Self {
        Self {
            measure: Some(Box::new(measure)),
            ..Self::leaf(style)
        }
    }

    /// A box placing `children` left to right.
    pub fn row(style: Style, children: Vec<Element<'a, K>>) -> Self {
        Self::of_kind(ElementKind::Row, style, children)
    }

    /// A box placing `children` top to bottom.
    pub fn column(style: Style, children: Vec<Element<'a, K>>) -> Self {
        Self::of_kind(ElementKind::Column, style, children)
    }

    /// A box placing `children` over each other, each filling it inside
    /// its padding unless sized, a sized one at its top left.
    pub fn stack(style: Style, children: Vec<Element<'a, K>>) -> Self {
        Self::of_kind(ElementKind::Stack, style, children)
    }

    /// This element known as `key` in the [`Layout`].
    pub fn with_key(self, key: K) -> Self {
        Self {
            key: Some(key),
            ..self
        }
    }

    /// Lays this element out as the root of its tree in `area`: at its top
    /// left, with the area's size as the room it may take.
    pub fn compute(&self, area: Rect) -> Layout<K> {
        let mut rects = HashMap::new();
        // Every node is made here from this tree, so taffy, which fails
        // only on node ids from elsewhere, cannot fail.
        let _ = self.compute_into(area, &mut rects);
        Layout { rects }
    }

    /// An element of `kind`.
    fn of_kind(kind: ElementKind, style: Style, children: Vec<Element<'a, K>>) -> Self {
        Self {
            children,
            kind,
            key: None,
            measure: None,
            style,
        }
    }

    /// Lays the tree out in `area`, putting each keyed element's
    /// rectangle in `rects`.
    fn compute_into(&self, area: Rect, rects: &mut HashMap<K, Rect>) -> Result<(), TaffyError> {
        let mut taffy: TaffyTree<usize> = TaffyTree::new();
        let mut leaves: Vec<&Element<'a, K>> = Vec::new();
        let mut keyed: Vec<(NodeId, K)> = Vec::new();
        let root = self.build(&mut taffy, None, &mut leaves, &mut keyed)?;
        let available = Size {
            height: AvailableSpace::Definite(area.height),
            width: AvailableSpace::Definite(area.width),
        };
        taffy.compute_layout_with_measure(root, available, |inputs, _, context, style| {
            let measure = context
                .and_then(|index| leaves.get(*index))
                .and_then(|leaf| leaf.measure.as_ref());
            compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known, room| {
                    let width = known.width.unwrap_or(match room.width {
                        AvailableSpace::Definite(width) => width,
                        AvailableSpace::MaxContent | AvailableSpace::MinContent => 0.0,
                    });
                    let height = match measure {
                        Some(measure) => known.height.unwrap_or_else(|| measure(width)),
                        None => known.height.unwrap_or(0.0),
                    };
                    Size { height, width }
                },
            )
        })?;
        let mut origins = HashMap::new();
        origins.insert(root, (area.x, area.y));
        place(&taffy, root, (area.x, area.y), &mut origins)?;
        for (node, key) in keyed {
            let layout = taffy.layout(node)?;
            if let Some(&(x, y)) = origins.get(&node) {
                rects.insert(key, Rect::new(x, y, layout.size.width, layout.size.height));
            }
        }
        Ok(())
    }

    /// Adds this element and its children to `taffy` as a child of a
    /// container of `parent` kind, noting measured leaves in `leaves` and
    /// keyed nodes in `keyed`.
    fn build<'t>(
        &'t self,
        taffy: &mut TaffyTree<usize>,
        parent: Option<(ElementKind, Edges)>,
        leaves: &mut Vec<&'t Element<'a, K>>,
        keyed: &mut Vec<(NodeId, K)>,
    ) -> Result<NodeId, TaffyError> {
        let style = self.style.to_taffy(self.kind, parent);
        let node = match self.kind {
            ElementKind::Leaf => match self.measure {
                Some(_) => {
                    leaves.push(self);
                    taffy.new_leaf_with_context(style, leaves.len() - 1)?
                }
                None => taffy.new_leaf(style)?,
            },
            ElementKind::Column | ElementKind::Row | ElementKind::Stack => {
                let children = self
                    .children
                    .iter()
                    .map(|child| {
                        child.build(taffy, Some((self.kind, self.style.padding)), leaves, keyed)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                taffy.new_with_children(style, &children)?
            }
        };
        if let Some(key) = self.key {
            keyed.push((node, key));
        }
        Ok(node)
    }
}

// ========================================================================
// Style Implementations
// ========================================================================

impl Style {
    /// Auto width and height, no spacing, children stretched across and
    /// packed at the start.
    pub fn new() -> Self {
        Self::default()
    }

    /// This style `width` wide.
    pub fn with_width(self, width: Length) -> Self {
        Self { width, ..self }
    }

    /// This style `height` high.
    pub fn with_height(self, height: Length) -> Self {
        Self { height, ..self }
    }

    /// This style with `padding` inside.
    pub fn with_padding(self, padding: Edges) -> Self {
        Self { padding, ..self }
    }

    /// This style with `margin` outside.
    pub fn with_margin(self, margin: Edges) -> Self {
        Self { margin, ..self }
    }

    /// This style with `gap` pixels between children.
    pub fn with_gap(self, gap: f32) -> Self {
        Self { gap, ..self }
    }

    /// This style aligning children across its direction as `align` says.
    pub fn with_align(self, align: Align) -> Self {
        Self { align, ..self }
    }

    /// This style spreading children along its direction as `justify`
    /// says.
    pub fn with_justify(self, justify: Justify) -> Self {
        Self { justify, ..self }
    }

    /// The taffy style of an element of `kind` in a container of `parent`
    /// kind and padding (`None` for the root).
    fn to_taffy(self, kind: ElementKind, parent: Option<(ElementKind, Edges)>) -> taffy::Style {
        let flex_direction = match kind {
            ElementKind::Column | ElementKind::Leaf | ElementKind::Stack => FlexDirection::Column,
            ElementKind::Row => FlexDirection::Row,
        };
        let (main_is_width, in_stack) = match parent.map(|(kind, _)| kind) {
            Some(ElementKind::Row) => (true, false),
            Some(ElementKind::Stack) => (false, true),
            Some(ElementKind::Column | ElementKind::Leaf) | None => (false, false),
        };
        // An absolute child is placed from the padding box's edge, so
        // the container's padding is where its content starts.
        let inside = parent.map_or(Edges::default(), |(_, padding)| padding);
        let (main, cross) = match main_is_width {
            true => (self.width, self.height),
            false => (self.height, self.width),
        };
        let flex_grow = match (main, in_stack) {
            (Length::Flex(weight), false) => weight.max(0.0),
            (Length::Auto | Length::Fixed(_) | Length::Flex(_) | Length::Percent(_), _) => 0.0,
        };
        let flex_basis = match flex_grow > 0.0 {
            true => Dimension::length(0.0),
            false => Dimension::auto(),
        };
        let cross_flex = matches!(cross, Length::Flex(_));
        let size = Size {
            height: dimension(self.height),
            width: dimension(self.width),
        };
        let fills_stack =
            in_stack && matches!(self.width, Length::Auto) && matches!(self.height, Length::Auto);
        let inset = match (in_stack, fills_stack) {
            (true, true) => inside.to_taffy_auto(),
            (true, false) => taffy::Rect {
                bottom: LengthPercentageAuto::auto(),
                left: LengthPercentageAuto::length(inside.left),
                right: LengthPercentageAuto::auto(),
                top: LengthPercentageAuto::length(inside.top),
            },
            (false, _) => taffy::Rect {
                bottom: LengthPercentageAuto::auto(),
                left: LengthPercentageAuto::auto(),
                right: LengthPercentageAuto::auto(),
                top: LengthPercentageAuto::auto(),
            },
        };
        taffy::Style {
            align_items: align_items(self.align),
            align_self: cross_flex.then_some(AlignItems::STRETCH),
            flex_basis,
            flex_direction,
            flex_grow,
            gap: Size {
                height: LengthPercentage::length(self.gap),
                width: LengthPercentage::length(self.gap),
            },
            inset,
            justify_content: justify_content(self.justify),
            margin: self.margin.to_taffy_auto(),
            padding: self.padding.to_taffy(),
            position: match in_stack {
                true => Position::Absolute,
                false => Position::Relative,
            },
            size,
            ..taffy::Style::default()
        }
    }
}

// ========================================================================
// Edges Implementations
// ========================================================================

impl Edges {
    /// `space` pixels on every side.
    pub const fn all(space: f32) -> Self {
        Self::symmetric(space, space)
    }

    /// `horizontal` pixels at the left and right, `vertical` above and
    /// below.
    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            bottom: vertical,
            left: horizontal,
            right: horizontal,
            top: vertical,
        }
    }

    /// These edges as taffy padding.
    fn to_taffy(self) -> taffy::Rect<LengthPercentage> {
        taffy::Rect {
            bottom: LengthPercentage::length(self.bottom),
            left: LengthPercentage::length(self.left),
            right: LengthPercentage::length(self.right),
            top: LengthPercentage::length(self.top),
        }
    }

    /// These edges as taffy margins.
    fn to_taffy_auto(self) -> taffy::Rect<LengthPercentageAuto> {
        taffy::Rect {
            bottom: LengthPercentageAuto::length(self.bottom),
            left: LengthPercentageAuto::length(self.left),
            right: LengthPercentageAuto::length(self.right),
            top: LengthPercentageAuto::length(self.top),
        }
    }
}

// ========================================================================
// Layout Implementations
// ========================================================================

impl<K: Eq + Hash> Layout<K> {
    /// Where the element keyed `key` landed, or `None` when no element has
    /// that key.
    pub fn rect(&self, key: &K) -> Option<Rect> {
        self.rects.get(key).copied()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Records in `origins` the window position of every node under `node`,
/// whose top left is at `origin`.
fn place(
    taffy: &TaffyTree<usize>,
    node: NodeId,
    origin: (f32, f32),
    origins: &mut HashMap<NodeId, (f32, f32)>,
) -> Result<(), TaffyError> {
    for child in taffy.children(node)? {
        let location = taffy.layout(child)?.location;
        let at = (origin.0 + location.x, origin.1 + location.y);
        origins.insert(child, at);
        place(taffy, child, at, origins)?;
    }
    Ok(())
}

/// `length` as a taffy size; a flexible length is sized by the flex
/// factors instead.
fn dimension(length: Length) -> Dimension {
    match length {
        Length::Auto | Length::Flex(_) => Dimension::auto(),
        Length::Fixed(pixels) => Dimension::length(pixels),
        Length::Percent(fraction) => Dimension::percent(fraction),
    }
}

/// `align` as taffy's cross-axis alignment.
fn align_items(align: Align) -> AlignItems {
    match align {
        Align::Center => AlignItems::CENTER,
        Align::End => AlignItems::FLEX_END,
        Align::Start => AlignItems::FLEX_START,
        Align::Stretch => AlignItems::STRETCH,
    }
}

/// `justify` as taffy's main-axis distribution.
fn justify_content(justify: Justify) -> JustifyContent {
    match justify {
        Justify::Center => JustifyContent::CENTER,
        Justify::End => JustifyContent::FLEX_END,
        Justify::SpaceBetween => JustifyContent::SPACE_BETWEEN,
        Justify::Start => JustifyContent::FLEX_START,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(10.0, 20.0, 300.0, 200.0);

    fn fixed(width: f32, height: f32) -> Style {
        Style::new()
            .with_width(Length::Fixed(width))
            .with_height(Length::Fixed(height))
    }

    #[test]
    fn test_a_column_stacks_children_inside_its_padding_with_gaps() {
        let tree = Element::column(
            Style::new().with_padding(Edges::all(5.0)).with_gap(4.0),
            vec![
                Element::leaf(Style::new().with_height(Length::Fixed(30.0))).with_key("a"),
                Element::leaf(Style::new().with_height(Length::Fixed(10.0))).with_key("b"),
            ],
        );
        let layout = tree.compute(AREA);
        assert_eq!(
            layout.rect(&"a"),
            Some(Rect::new(15.0, 25.0, 290.0, 30.0)),
            "stretched across"
        );
        assert_eq!(layout.rect(&"b"), Some(Rect::new(15.0, 59.0, 290.0, 10.0)));
        assert_eq!(layout.rect(&"nope"), None);
    }

    #[test]
    fn test_flexible_children_share_the_room_by_weight_after_fixed_ones() {
        let tree = Element::row(
            Style::new().with_width(Length::Fixed(300.0)),
            vec![
                Element::leaf(Style::new().with_width(Length::Fixed(60.0))).with_key("fixed"),
                Element::leaf(Style::new().with_width(Length::Flex(1.0))).with_key("one"),
                Element::leaf(Style::new().with_width(Length::Flex(2.0))).with_key("two"),
            ],
        );
        let layout = tree.compute(AREA);
        let width = |key| layout.rect(&key).map(|rect| rect.width);
        assert_eq!(width("fixed"), Some(60.0));
        assert_eq!(width("one"), Some(80.0));
        assert_eq!(width("two"), Some(160.0));
        assert_eq!(layout.rect(&"two").map(|rect| rect.x), Some(150.0));
    }

    #[test]
    fn test_percent_sizes_margins_and_end_justification_place_buttons() {
        let tree = Element::row(
            Style::new()
                .with_width(Length::Percent(0.5))
                .with_justify(Justify::End)
                .with_align(Align::Center)
                .with_gap(8.0),
            vec![
                Element::leaf(fixed(40.0, 20.0)).with_key("cancel"),
                Element::leaf(fixed(30.0, 20.0).with_margin(Edges::symmetric(0.0, 10.0)))
                    .with_key("ok"),
            ],
        )
        .with_key("bar");
        let layout = tree.compute(AREA);
        assert_eq!(layout.rect(&"bar").map(|rect| rect.width), Some(150.0));
        assert_eq!(
            layout.rect(&"ok"),
            Some(Rect::new(130.0, 30.0, 30.0, 20.0)),
            "flush right"
        );
        assert_eq!(
            layout.rect(&"cancel"),
            Some(Rect::new(82.0, 30.0, 40.0, 20.0)),
            "centered"
        );
    }

    #[test]
    fn test_a_measured_leaf_gets_the_height_for_its_width() {
        let tree = Element::column(
            Style::new().with_width(Length::Fixed(120.0)),
            vec![Element::measured(Style::new(), |width| 1200.0 / width).with_key("text")],
        )
        .with_key("root");
        let layout = tree.compute(AREA);
        assert_eq!(layout.rect(&"text").map(|rect| rect.height), Some(10.0));
        assert_eq!(
            layout.rect(&"root").map(|rect| rect.height),
            Some(10.0),
            "the root wraps it"
        );
    }

    #[test]
    fn test_a_stack_lays_children_over_each_other() {
        let tree = Element::stack(
            fixed(100.0, 50.0).with_padding(Edges::all(5.0)),
            vec![
                Element::leaf(Style::new()).with_key("fill"),
                Element::leaf(fixed(10.0, 10.0)).with_key("badge"),
            ],
        );
        let layout = tree.compute(AREA);
        assert_eq!(
            layout.rect(&"fill"),
            Some(Rect::new(15.0, 25.0, 90.0, 40.0))
        );
        assert_eq!(
            layout.rect(&"badge"),
            Some(Rect::new(15.0, 25.0, 10.0, 10.0))
        );
    }
}

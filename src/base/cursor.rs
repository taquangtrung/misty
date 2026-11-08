//! [`CursorIcon`]: the shape a widget asks the pointer to take over the
//! window, as a value the runtime hands to the window system.

// ========================================================================
// Data Structures
// ========================================================================

/// A pointer shape, asked for with
/// [`EventCx::set_cursor`](crate::EventCx::set_cursor). The window system
/// draws each in its own style; where it has no such shape it shows its
/// nearest one.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum CursorIcon {
    /// A link or a shortcut is made when dropped.
    Alias,
    /// What is under the pointer can be resized left or right, such as a
    /// column.
    ColResize,
    /// A menu of commands opens on what is under the pointer.
    ContextMenu,
    /// A copy is made when dropped.
    Copy,
    /// Precise selection, such as a pixel in a picture.
    Crosshair,
    /// The window system's usual arrow.
    #[default]
    Default,
    /// The right edge is being moved.
    EResize,
    /// A horizontal edge pair, left and right, is being moved.
    EwResize,
    /// What is under the pointer can be grabbed and moved.
    Grab,
    /// What is under the pointer is being moved.
    Grabbing,
    /// Help is offered for what is under the pointer.
    Help,
    /// What is under the pointer is being moved, in any direction.
    Move,
    /// The top-right corner is being moved.
    NeResize,
    /// The top-right or bottom-left corner is being moved.
    NeswResize,
    /// The top edge is being moved.
    NResize,
    /// What was asked cannot be done here.
    NotAllowed,
    /// The top and bottom edges are being moved.
    NsResize,
    /// The top-left corner is being moved.
    NwResize,
    /// The top-left or bottom-right corner is being moved.
    NwseResize,
    /// A link or a button: clicking does something.
    Pointer,
    /// The app is busy, but still takes input.
    Progress,
    /// What is under the pointer can be resized up or down, such as a row.
    RowResize,
    /// The bottom-right corner is being moved.
    SeResize,
    /// The bottom edge is being moved.
    SResize,
    /// The bottom-left corner is being moved.
    SwResize,
    /// Text that can be selected or edited.
    Text,
    /// The app is busy and takes no input.
    Wait,
    /// The left edge is being moved.
    WResize,
    /// What is under the pointer can be zoomed in.
    ZoomIn,
    /// What is under the pointer can be zoomed out.
    ZoomOut,
}

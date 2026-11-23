//! The lowest layer: value types that need no GPU or window system: the
//! user's [`appearance`] settings, [`color`]s, the [`box_style`] of rounded, bordered, shadowed boxes,
//! the [`cell`] grid model a renderer draws from, the
//! [`theme`] and [`palette`] that resolve their colors, pixel
//! [`geometry`], [`key`]s with their notation and labels, the [`split`]
//! layout tree of panes, the cursor and scroll of a [`list`], [`text`]
//! measured in cells and [`styled`] text, [`fuzzy`] matching for pickers, the easing of a smooth
//! [`scroll`], when a [`scrollbar`] shows, the tweens, fades, and blinks of [`motion`], the [`clock`] they read the time from, the [`syntax`] colors of highlighted source text, the
//! [`strings`] misty's widgets show, and the vector [`path`]s, their
//! [`paint`] and strokes, and the [`transform`]s a frame draws them with.
//! `make layers` keeps the GPU, window, and other GUI crates out of it,
//! so it can become a crate of its own again if headless code needs it.
//!
//! The `kdl` feature reads theme files in KDL (the `theme_file` module).

pub(crate) mod appearance;
pub(crate) mod box_style;
pub(crate) mod cell;
pub(crate) mod clock;
pub(crate) mod color;
pub(crate) mod cursor;
pub(crate) mod fuzzy;
pub(crate) mod geometry;
pub(crate) mod key;
pub(crate) mod list;
pub(crate) mod motion;
pub(crate) mod notice;
pub(crate) mod paint;
pub(crate) mod palette;
pub(crate) mod path;
pub(crate) mod scroll;
pub(crate) mod scrollbar;
pub(crate) mod spinner;
pub(crate) mod split;
pub(crate) mod strings;
pub(crate) mod styled;
pub(crate) mod syntax;
pub(crate) mod text;
pub(crate) mod theme;
#[cfg(feature = "kdl")]
pub(crate) mod theme_file;
pub(crate) mod transform;

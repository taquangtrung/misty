//! What the user did, as misty sees it: the [`event`]s the runtime reads
//! from the window system (its keys turned into misty's in `winit_keys`),
//! and the [`click`] counting that tells a double click.

pub(crate) mod click;
pub(crate) mod event;
mod winit_keys;

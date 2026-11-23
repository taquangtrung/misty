//! When a scrollbar shows: [`ScrollbarShow`] says it of every scrollbar in
//! a theme, and a [`ScrollbarFade`] times one scrollbar's showing while it
//! is [`ScrollbarShow::Auto`]: in full from the last time it was wanted (a
//! scroll, the pointer over it, a drag) for [`HOLD`], then fading out over
//! [`FADE`].
//!
//! A painter reads [`ScrollbarFade::opacity`] each frame and mixes its
//! colors that far from the background toward their own, drawing nothing
//! at 0 (its strip still takes the pointer, so moving over it shows it);
//! and asks for [`ScrollbarFade::next_frame`] so the fade is drawn.

use std::time::{Duration, Instant};

use super::motion::Tween;

// ========================================================================
// Constants
// ========================================================================

/// How long a scrollbar stays in full after it was last wanted.
const HOLD: Duration = Duration::from_millis(1000);

/// How long it takes to fade out.
const FADE: Duration = Duration::from_millis(250);

// ========================================================================
// Data Structures
// ========================================================================

/// When the scrollbars of a theme show.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ScrollbarShow {
    /// Always, while there is anything to scroll.
    #[default]
    Always,
    /// While scrolling, while the pointer is over it, and for a moment
    /// after; faded out otherwise.
    Auto,
    /// Never; the view still scrolls.
    Never,
}

/// One scrollbar's showing while it is [`ScrollbarShow::Auto`]: hidden
/// until first [woken](Self::wake).
#[derive(Clone, Copy, Debug)]
pub struct ScrollbarFade {
    /// Its opacity over time, from the last wake.
    opacity: Tween<f32>,
}

// ========================================================================
// ScrollbarFade Implementations
// ========================================================================

impl ScrollbarFade {
    /// A scrollbar hidden until it is wanted.
    pub fn new() -> Self {
        let mut opacity = fade();
        opacity.finish();
        Self { opacity }
    }

    /// Shows it in full from `now`, for [`HOLD`] and then fading out: the
    /// view scrolled, or the pointer is over the scrollbar or drags it.
    pub fn wake(&mut self, now: Instant) {
        self.opacity = fade();
        self.opacity.start(now);
    }

    /// How much of it shows at `now`, shown as `show` says: from 0
    /// (nothing) to 1 (in full).
    pub fn opacity(&self, show: ScrollbarShow, now: Instant) -> f32 {
        match show {
            ScrollbarShow::Always => 1.0,
            ScrollbarShow::Auto => self.opacity.value(now),
            ScrollbarShow::Never => 0.0,
        }
    }

    /// When its next frame is due at `now`: the end of the hold, then each
    /// frame of the fade; `None` once it is still.
    pub fn next_frame(&self, show: ScrollbarShow, now: Instant) -> Option<Instant> {
        match show {
            ScrollbarShow::Auto => self.opacity.next_frame(now),
            ScrollbarShow::Always | ScrollbarShow::Never => None,
        }
    }
}

impl Default for ScrollbarFade {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The opacity of a scrollbar just woken: in full for [`HOLD`], then down
/// to nothing over [`FADE`].
fn fade() -> Tween<f32> {
    Tween::new(1.0, 0.0, FADE).with_delay(HOLD)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_auto_scrollbar_shows_when_woken_holds_then_fades_out() {
        let start = Instant::now();
        let mut fade = ScrollbarFade::new();
        assert_eq!(
            fade.opacity(ScrollbarShow::Auto, start),
            0.0,
            "hidden at first"
        );
        assert_eq!(fade.next_frame(ScrollbarShow::Auto, start), None);
        fade.wake(start);
        let held = start + HOLD / 2;
        assert_eq!(fade.opacity(ScrollbarShow::Auto, held), 1.0);
        assert_eq!(
            fade.next_frame(ScrollbarShow::Auto, held),
            Some(start + HOLD),
            "no frames while it holds"
        );
        let fading = fade.opacity(ScrollbarShow::Auto, start + HOLD + FADE / 2);
        assert!(fading > 0.0 && fading < 1.0, "{fading}");
        let gone = start + HOLD + FADE * 2;
        assert_eq!(fade.opacity(ScrollbarShow::Auto, gone), 0.0);
        assert_eq!(fade.next_frame(ScrollbarShow::Auto, gone), None);
        assert_eq!(fade.opacity(ScrollbarShow::Always, gone), 1.0);
        assert_eq!(fade.opacity(ScrollbarShow::Never, held), 0.0);
    }
}

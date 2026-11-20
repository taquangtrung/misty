//! The spinner busy work wears: braille dots circling their cell, turned
//! one frame each [`FRAME`], every spinner counted from the same epoch
//! turning in step.

use std::time::{Duration, Instant};

// ========================================================================
// Constants
// ========================================================================

/// How long one frame of the spinner shows.
pub(crate) const FRAME: Duration = Duration::from_millis(80);

/// The spinner's frames, three braille dots circling their cell.
const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ========================================================================
// Functions
// ========================================================================

/// The glyph a spinner started at `epoch` shows at `now`.
pub(crate) fn glyph_at(epoch: Instant, now: Instant) -> char {
    FRAMES[frame_at(now.saturating_duration_since(epoch)) % FRAMES.len()]
}

/// When a spinner started at `epoch` next turns after `now`.
pub(crate) fn next_turn(epoch: Instant, now: Instant) -> Instant {
    let frames = frame_at(now.saturating_duration_since(epoch)) as u32;
    epoch + FRAME * (frames + 1)
}

/// The frame a spinner is on `elapsed` after its epoch, left unwrapped.
fn frame_at(elapsed: Duration) -> usize {
    (elapsed.as_millis() / FRAME.as_millis()) as usize
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    #[test]
    fn test_the_spinner_turns_through_every_frame_and_wraps() {
        let epoch = Instant::now();
        let turn = |step: usize| glyph_at(epoch, epoch + FRAME * step as u32);
        let seen: HashSet<char> = (0..FRAMES.len()).map(turn).collect();
        assert_eq!(seen.len(), FRAMES.len());
        assert_eq!(turn(FRAMES.len()), turn(0));
    }

    #[test]
    fn test_the_next_turn_is_the_start_of_the_next_frame() {
        let epoch = Instant::now();
        let mid_second = epoch + FRAME + FRAME / 2;
        assert_eq!(next_turn(epoch, mid_second), epoch + FRAME * 2);
        assert_eq!(next_turn(epoch, epoch), epoch + FRAME);
    }
}

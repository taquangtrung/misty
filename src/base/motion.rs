//! Motion: values that change over time, for fades, slides, and blinks.
//!
//! A [`Tween`] moves one value (any [`Lerp`] type: `f32`, [`Point`],
//! [`Rect`], [`Rgb`], [`Rgba`]) from one end to the other over a duration,
//! along an [`Easing`] curve, after an optional delay, once or repeating.
//! An [`Animator`] keeps a number from 0 to 1 per key (a row, a button) and
//! says when the next frame of any of them is due; a [`HoverFade`] is an
//! animator that lights the item under the pointer and dims the rest. A
//! [`Blink`] times a text caret.
//!
//! Nothing here reads the clock: every call takes the `now` it is asked
//! about, so a test passes made-up times. The one pattern for an app:
//!
//! 1. On an input that starts a motion, call [`Tween::start`] (or
//!    [`Animator::animate_to`]) with `Instant::now()`.
//! 2. In `render`, read [`Tween::value`] at `Instant::now()` and draw it.
//! 3. In [`Application::about_to_wait`](crate::Application::about_to_wait),
//!    return [`Tween::next_frame`]: `None` once every motion is at rest, so
//!    an idle app sleeps.
//!
//! With reduced motion on (the user asked their system for it, see
//! [`Appearance`](crate::Appearance)), every motion jumps to its end.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use super::color::{Rgb, Rgba};
use super::geometry::{Point, Rect};

// ========================================================================
// Constants
// ========================================================================

/// How long a hover fade takes unless the theme says otherwise.
pub const DEFAULT_HOVER_FADE: Duration = Duration::from_millis(140);

/// How long a caret shows, then hides, in each blink unless the theme says
/// otherwise: GTK's and Windows' default half period.
pub const DEFAULT_CARET_BLINK: Duration = Duration::from_millis(530);

/// How long a caret keeps blinking after the last input before it rests,
/// shown: GNOME's default, which saves waking an idle app.
pub const BLINK_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a running motion wants a new frame: about 60 a second.
pub(crate) const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How quickly [`Easing::Spring`] settles: the exponent of its decay.
const SPRING_DAMPING: f32 = 6.0;

/// How fast [`Easing::Spring`] swings, in radians over the whole motion:
/// about one and a half swings past the end before it settles.
const SPRING_FREQUENCY: f32 = 9.0;

// ========================================================================
// Data Structures
// ========================================================================

/// How long the motions of misty's widgets take, part of a
/// [`Theme`](crate::Theme).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Motion {
    /// How long a caret shows, then hides, in each blink.
    pub caret_blink: Duration,
    /// How long a control takes to light up under the pointer, and to dim
    /// after it leaves.
    pub hover_fade: Duration,
}

/// The curve a [`Tween`] follows from its start (0) to its end (1).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Easing {
    /// Slow at first, fast at the end: for something leaving.
    EaseIn,
    /// Slow at both ends: for something moving from one place to another.
    EaseInOut,
    /// Fast at first, settling gently: for something arriving or answering
    /// an input (the default).
    #[default]
    EaseOut,
    /// At an even pace: for a progress bar or a blink.
    Linear,
    /// Overshoots the end and swings back to rest, as on a spring: for a
    /// panel snapping into place.
    Spring,
}

/// What a [`Tween`] does once it reaches its end.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Repeat {
    /// Runs back to its start, then forward again, forever: a pulse.
    Alternate,
    /// Starts over from its start, forever: a spinner.
    Loop,
    /// Stays at its end (the default).
    #[default]
    Once,
}

/// One value moving between two ends over time. Made at rest at its start;
/// [`Tween::start`] sets it moving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween<T> {
    /// How long it waits after starting before it moves.
    delay: Duration,
    /// How long one run from start to end takes.
    duration: Duration,
    /// The curve it follows.
    easing: Easing,
    /// Where it starts.
    from: T,
    /// Whether it jumps to its end instead of moving.
    reduced_motion: bool,
    /// What it does at its end.
    repeat: Repeat,
    /// When it started, or `None` at rest.
    start: Option<Instant>,
    /// Where it ends.
    to: T,
}

/// A number from 0 to 1 per key, each easing toward the target it was last
/// given: the light of each row under the pointer, the openness of each
/// section. A key never given a target is at 0.
#[derive(Clone, Debug)]
pub struct Animator<K> {
    /// How long each move takes.
    duration: Duration,
    /// The curve each move follows.
    easing: Easing,
    /// Whether moves jump instead of easing.
    reduced_motion: bool,
    /// The keys not at rest at 0.
    values: HashMap<K, Tween<f32>>,
}

/// The light of each item under the pointer: the one hovered fades in to
/// 1, every other fades out to 0. A button keeps one for itself; a list or
/// a toolbar keeps one keyed by item.
#[derive(Clone, Debug)]
pub struct HoverFade<K> {
    /// The level of each key.
    levels: Animator<K>,
}

/// The blink of a text caret: shown for an interval, hidden for one, and
/// shown again at once on every input, so it never hides while the user
/// types. It blinks only once the app drives it ([`Blink::drive`], then
/// [`Blink::next_frame`] for when to redraw); until then, and after ten
/// seconds without input, it shows steady.
/// Reduced motion does not stop it: like GTK and Windows, misty leaves the
/// blink to its own setting, [`Blink::set_enabled`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blink {
    /// Whether the app drives it, so its hidden phases get drawn and
    /// undrawn on time.
    driven: bool,
    /// Whether it blinks at all.
    enabled: bool,
    /// How long each phase lasts.
    interval: Duration,
    /// When it was last shown at once, by an input or by focus.
    reset: Option<Instant>,
}

// ========================================================================
// Traits
// ========================================================================

/// A value a [`Tween`] can move: one that can be mixed between two ends.
pub trait Lerp: Copy {
    /// The value `t` of the way from `self` to `to`: `self` at 0, `to` at
    /// 1, and past either end for `t` outside 0 to 1 (a spring's
    /// overshoot), as far as the type allows.
    fn lerp(self, to: Self, t: f32) -> Self;
}

// ========================================================================
// Motion Implementations
// ========================================================================

impl Default for Motion {
    fn default() -> Self {
        Self {
            caret_blink: DEFAULT_CARET_BLINK,
            hover_fade: DEFAULT_HOVER_FADE,
        }
    }
}

// ========================================================================
// Easing Implementations
// ========================================================================

impl Easing {
    /// How far along the curve is `t` of the way through: 0 at or before
    /// 0, exactly 1 at or after 1.
    pub fn apply(self, t: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        match self {
            Self::EaseIn => t * t * t,
            Self::EaseInOut if t < 0.5 => 4.0 * t * t * t,
            Self::EaseInOut => 1.0 - (2.0 - 2.0 * t).powi(3) / 2.0,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t),
            Self::Linear => t,
            Self::Spring => 1.0 - (-SPRING_DAMPING * t).exp() * (SPRING_FREQUENCY * t).cos(),
        }
    }
}

// ========================================================================
// Tween Implementations
// ========================================================================

impl<T: Lerp> Tween<T> {
    /// A tween at rest at `from` that moves to `to` over `duration` when
    /// started, easing out, without delay, once.
    pub fn new(from: T, to: T, duration: Duration) -> Self {
        Self {
            delay: Duration::ZERO,
            duration,
            easing: Easing::default(),
            from,
            reduced_motion: false,
            repeat: Repeat::default(),
            start: None,
            to,
        }
    }

    /// The tween following `easing`.
    pub fn with_easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }

    /// The tween waiting `delay` after it starts before it moves.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// The tween doing `repeat` at its end.
    pub fn with_repeat(mut self, repeat: Repeat) -> Self {
        self.repeat = repeat;
        self
    }

    /// Makes it jump to its end (`true`) rather than move, for users who
    /// asked their system for reduced motion. Turning it on ends a running
    /// tween at its end.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        if reduced_motion {
            self.finish();
        }
    }

    /// Sets it moving from its start at `now`; at once to its end with
    /// reduced motion or a zero duration. Starting a running tween starts
    /// it over.
    pub fn start(&mut self, now: Instant) {
        match self.reduced_motion || self.duration.is_zero() {
            true => self.finish(),
            false => self.start = Some(now),
        }
    }

    /// Sets it moving from where it is at `now` to `to`, so a fade reversed
    /// halfway turns back from where it was instead of jumping. The delay
    /// and repeat stay.
    pub fn retarget(&mut self, to: T, now: Instant) {
        self.from = self.value(now);
        self.to = to;
        self.start(now);
    }

    /// Puts it at its end at once, at rest.
    pub fn finish(&mut self) {
        self.from = self.to;
        self.start = None;
    }

    /// Puts it at `to` at once, at rest, with `to` as its end.
    pub(crate) fn jump_to(&mut self, to: T) {
        self.to = to;
        self.finish();
    }

    /// Where it ends.
    pub fn target(&self) -> T {
        self.to
    }

    /// Its value at `now`: its start before it is started, its end once
    /// done.
    pub fn value(&self, now: Instant) -> T {
        match (self.progress(now), self.start) {
            (Some(t), _) => self.from.lerp(self.to, self.easing.apply(t)),
            (None, None) => self.from,
            (None, Some(_)) => self.to,
        }
    }

    /// Whether it is waiting or moving at `now`; a repeating tween runs
    /// until reduced motion or [`Tween::finish`] stops it.
    pub fn is_animating(&self, now: Instant) -> bool {
        self.progress(now).is_some()
    }

    /// When the next frame of a tween running at `now` is due: the end of
    /// its delay, or a frame interval on; `None` at rest.
    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        let start = self.start?;
        if !self.is_animating(now) {
            return None;
        }
        Some((start + self.delay).max(now + FRAME_INTERVAL))
    }

    /// How far through its current run it is at `now`, from 0 to 1 (1 for
    /// an alternating tween on its way back to the start), or `None` at
    /// rest or once done.
    fn progress(&self, now: Instant) -> Option<f32> {
        let start = self.start?;
        let elapsed = now.saturating_duration_since(start + self.delay);
        let runs = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        if now < start + self.delay {
            return Some(0.0);
        }
        match self.repeat {
            Repeat::Once => (runs < 1.0).then_some(runs),
            Repeat::Loop => Some(runs.fract()),
            Repeat::Alternate => {
                let phase = runs % 2.0;
                Some(match phase < 1.0 {
                    true => phase,
                    false => 2.0 - phase,
                })
            }
        }
    }
}

// ========================================================================
// Animator Implementations
// ========================================================================

impl<K: Eq + Hash> Animator<K> {
    /// An animator whose moves take `duration` along `easing`.
    pub fn new(duration: Duration, easing: Easing) -> Self {
        Self {
            duration,
            easing,
            reduced_motion: false,
            values: HashMap::new(),
        }
    }

    /// Makes moves jump (`true`) rather than ease; turning it on ends every
    /// running move.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        for tween in self.values.values_mut() {
            tween.set_reduced_motion(reduced_motion);
        }
    }

    /// Makes the moves started from now on take `duration`.
    pub fn set_duration(&mut self, duration: Duration) {
        self.duration = duration;
    }

    /// Sets `key` easing from where it is at `now` toward `target`, which
    /// is held to 0 to 1. The target it already has changes nothing.
    pub fn animate_to(&mut self, key: K, target: f32, now: Instant) {
        let target = target.clamp(0.0, 1.0);
        let (duration, easing, reduced_motion) = (self.duration, self.easing, self.reduced_motion);
        let tween = self.values.entry(key).or_insert_with(|| {
            let mut tween = Tween::new(0.0, 0.0, duration).with_easing(easing);
            tween.set_reduced_motion(reduced_motion);
            tween
        });
        if tween.target() != target {
            tween.retarget(target, now);
        }
    }

    /// The value of `key` at `now`: 0 for a key never given a target.
    pub fn value(&self, key: &K, now: Instant) -> f32 {
        self.values.get(key).map_or(0.0, |tween| tween.value(now))
    }

    /// When the next frame of any running move is due at `now`, for
    /// `about_to_wait`; `None` when every key is at rest. Keys at rest at 0
    /// are forgotten on the way, so an animator of many rows stays small.
    pub fn next_frame(&mut self, now: Instant) -> Option<Instant> {
        self.values
            .retain(|_, tween| tween.is_animating(now) || tween.target() != 0.0);
        self.values
            .values()
            .filter_map(|tween| tween.next_frame(now))
            .min()
    }

    /// The keys whose target is not 0, or that are still moving at `now`.
    fn keys_lit(&self, now: Instant) -> impl Iterator<Item = &K> {
        self.values
            .iter()
            .filter(move |(_, tween)| tween.target() != 0.0 || tween.is_animating(now))
            .map(|(key, _)| key)
    }
}

// ========================================================================
// HoverFade Implementations
// ========================================================================

impl<K: Clone + Eq + Hash> HoverFade<K> {
    /// A hover fade whose fades take `duration` (a theme's
    /// [`Motion::hover_fade`]).
    pub fn new(duration: Duration) -> Self {
        Self {
            levels: Animator::new(duration, Easing::EaseOut),
        }
    }

    /// Makes fades jump (`true`) rather than ease.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.levels.set_reduced_motion(reduced_motion);
    }

    /// Makes the fades started from now on take `duration`.
    pub fn set_duration(&mut self, duration: Duration) {
        self.levels.set_duration(duration);
    }

    /// Fades `hovered` in and every other key out, from `now`; `None` when
    /// the pointer is over none.
    pub fn set_hovered(&mut self, hovered: Option<K>, now: Instant) {
        let lit: Vec<K> = self
            .levels
            .keys_lit(now)
            .filter(|key| Some(*key) != hovered.as_ref())
            .cloned()
            .collect();
        for key in lit {
            self.levels.animate_to(key, 0.0, now);
        }
        if let Some(key) = hovered {
            self.levels.animate_to(key, 1.0, now);
        }
    }

    /// How lit `key` is at `now`, from 0 to 1.
    pub fn level(&self, key: &K, now: Instant) -> f32 {
        self.levels.value(key, now)
    }

    /// When the next frame of a fade under way at `now` is due; `None` at
    /// rest.
    pub fn next_frame(&mut self, now: Instant) -> Option<Instant> {
        self.levels.next_frame(now)
    }
}

// ========================================================================
// Blink Implementations
// ========================================================================

impl Blink {
    /// A blink whose phases each last `interval` (a theme's
    /// [`Motion::caret_blink`]).
    pub fn new(interval: Duration) -> Self {
        Self {
            driven: false,
            enabled: true,
            interval,
            reset: None,
        }
    }

    /// Makes each phase from now on last `interval`.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// Makes the caret blink (`true`) or show steady, as the user's system
    /// setting for caret blinking says.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Shows the caret at once and starts its blink over from `now`: call
    /// it on every input and when the caret gains focus.
    pub fn reset(&mut self, now: Instant) {
        self.reset = Some(now);
    }

    /// Whether the caret shows at `now`.
    pub fn is_visible(&self, now: Instant) -> bool {
        match self.phase(now) {
            Some(phase) => phase % 2 == 0,
            None => true,
        }
    }

    /// Starts the blink, saying the app now schedules its frames: call it
    /// from `about_to_wait` before [`Blink::next_frame`]. A caret never
    /// driven stays steady, so it never hides on a frame that nothing
    /// redraws.
    pub fn drive(&mut self) {
        self.driven = true;
    }

    /// When the caret next shows or hides after `now`, for
    /// `about_to_wait`; `None` once it rests, or before [`Blink::drive`].
    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        let reset = self.reset?;
        let phase = self.phase(now)?;
        let next = reset + self.interval * (phase + 1);
        Some(next.min(reset + BLINK_TIMEOUT))
    }

    /// Which phase of its blink the caret is in at `now`, counting from 0
    /// (shown) at the last reset; `None` while it shows steady.
    fn phase(&self, now: Instant) -> Option<u32> {
        if !self.driven || !self.enabled || self.interval.is_zero() {
            return None;
        }
        let since = now.saturating_duration_since(self.reset?);
        if since >= BLINK_TIMEOUT {
            return None;
        }
        Some((since.as_secs_f32() / self.interval.as_secs_f32()) as u32)
    }
}

impl Default for Blink {
    fn default() -> Self {
        Self::new(DEFAULT_CARET_BLINK)
    }
}

// ========================================================================
// Lerp Implementations
// ========================================================================

impl Lerp for f32 {
    fn lerp(self, to: Self, t: f32) -> Self {
        self + (to - self) * t
    }
}

impl Lerp for Point {
    fn lerp(self, to: Self, t: f32) -> Self {
        Point::new(self.x.lerp(to.x, t), self.y.lerp(to.y, t))
    }
}

impl Lerp for Rect {
    fn lerp(self, to: Self, t: f32) -> Self {
        Rect::new(
            self.x.lerp(to.x, t),
            self.y.lerp(to.y, t),
            self.width.lerp(to.width, t),
            self.height.lerp(to.height, t),
        )
    }
}

/// Colors mix channel by channel in sRGB, as CSS transitions do, so a fade
/// between two theme colors passes the shades a designer expects; a
/// channel past either end (a spring's overshoot) is held to 0 to 255.
impl Lerp for Rgb {
    fn lerp(self, to: Self, t: f32) -> Self {
        Rgb::new(
            channel(self.r, to.r, t),
            channel(self.g, to.g, t),
            channel(self.b, to.b, t),
        )
    }
}

/// As for [`Rgb`], with alpha mixed the same way.
impl Lerp for Rgba {
    fn lerp(self, to: Self, t: f32) -> Self {
        Rgba::new(
            channel(self.r, to.r, t),
            channel(self.g, to.g, t),
            channel(self.b, to.b, t),
            channel(self.a, to.a, t),
        )
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The color channel `t` of the way from `from` to `to`, rounded and held
/// to a byte.
fn channel(from: u8, to: u8, t: f32) -> u8 {
    f32::from(from)
        .lerp(f32::from(to), t)
        .round()
        .clamp(0.0, f32::from(u8::MAX)) as u8
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const DURATION: Duration = Duration::from_millis(100);

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn test_every_easing_runs_from_exactly_zero_to_exactly_one() {
        for easing in [
            Easing::EaseIn,
            Easing::EaseInOut,
            Easing::EaseOut,
            Easing::Linear,
            Easing::Spring,
        ] {
            assert_eq!(easing.apply(-1.0), 0.0, "{easing:?}");
            assert_eq!(easing.apply(0.0), 0.0, "{easing:?}");
            assert_eq!(easing.apply(1.0), 1.0, "{easing:?}");
        }
        assert!(Easing::EaseIn.apply(0.5) < 0.5, "slow at first");
        assert!(Easing::EaseOut.apply(0.5) > 0.5, "fast at first");
        assert!(
            (Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-6,
            "symmetric"
        );
        let overshoot = (1..100).any(|step| Easing::Spring.apply(step as f32 / 100.0) > 1.0);
        assert!(overshoot, "a spring swings past its end");
    }

    #[test]
    fn test_a_tween_waits_out_its_delay_then_moves_to_its_end() {
        let start = Instant::now();
        let mut tween = Tween::new(0.0, 10.0, DURATION)
            .with_easing(Easing::Linear)
            .with_delay(Duration::from_millis(50));
        assert_eq!(tween.value(start), 0.0, "not started, at its start");
        tween.start(start);
        assert_eq!(tween.value(at(start, 25)), 0.0, "still waiting");
        assert_eq!(
            tween.next_frame(at(start, 25)),
            Some(at(start, 50)),
            "no frames while waiting"
        );
        assert!((tween.value(at(start, 100)) - 5.0).abs() < 1e-4);
        assert!(tween.is_animating(at(start, 149)));
        assert_eq!(tween.value(at(start, 150)), 10.0);
        assert_eq!(tween.next_frame(at(start, 150)), None);
    }

    #[test]
    fn test_repeating_tweens_loop_or_turn_back() {
        let start = Instant::now();
        let mut looping = Tween::new(0.0, 1.0, DURATION)
            .with_easing(Easing::Linear)
            .with_repeat(Repeat::Loop);
        let mut alternating = looping.with_repeat(Repeat::Alternate);
        looping.start(start);
        alternating.start(start);
        assert!((looping.value(at(start, 125)) - 0.25).abs() < 1e-4);
        assert!((alternating.value(at(start, 125)) - 0.75).abs() < 1e-4);
        assert!(looping.is_animating(at(start, 10_000)), "forever");
        looping.set_reduced_motion(true);
        assert!(!looping.is_animating(at(start, 10_000)));
        assert_eq!(looping.value(at(start, 125)), 1.0, "jumped to its end");
    }

    #[test]
    fn test_retargeting_turns_back_from_where_it_is() {
        let start = Instant::now();
        let mut tween = Tween::new(0.0, 1.0, DURATION).with_easing(Easing::Linear);
        tween.start(start);
        let middle = at(start, 50);
        tween.retarget(0.0, middle);
        assert!((tween.value(middle) - 0.5).abs() < 1e-4, "no jump");
        assert_eq!(tween.value(at(middle, 100)), 0.0);
    }

    #[test]
    fn test_reduced_motion_jumps_a_tween_started_after_it() {
        let start = Instant::now();
        let mut tween = Tween::new(Point::new(0.0, 0.0), Point::new(4.0, 8.0), DURATION);
        tween.set_reduced_motion(true);
        tween.start(start);
        assert_eq!(tween.value(start), Point::new(4.0, 8.0));
        assert_eq!(tween.next_frame(start), None);
    }

    #[test]
    fn test_colors_mix_by_channel_and_hold_an_overshoot() {
        let black = Rgba::new(0, 0, 0, 0);
        let white = Rgba::new(255, 255, 255, 255);
        assert_eq!(black.lerp(white, 0.5), Rgba::new(128, 128, 128, 128));
        assert_eq!(
            Rgb::new(250, 0, 0).lerp(Rgb::new(255, 0, 0), 3.0),
            Rgb::new(255, 0, 0)
        );
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0).lerp(Rect::new(10.0, 20.0, 30.0, 40.0), 0.5);
        assert_eq!(rect, Rect::new(5.0, 10.0, 20.0, 25.0));
    }

    #[test]
    fn test_an_animator_gives_the_soonest_frame_and_forgets_keys_at_zero() {
        let start = Instant::now();
        let mut animator = Animator::new(DURATION, Easing::Linear);
        animator.animate_to("a", 1.0, start);
        assert_eq!(animator.next_frame(start), Some(start + FRAME_INTERVAL));
        assert!((animator.value(&"a", at(start, 50)) - 0.5).abs() < 1e-4);
        assert_eq!(animator.value(&"b", start), 0.0, "never given a target");
        animator.animate_to("a", 0.0, at(start, 100));
        assert_eq!(animator.next_frame(at(start, 200)), None);
        assert!(animator.values.is_empty(), "at rest at 0, forgotten");
    }

    #[test]
    fn test_a_hover_fade_lights_one_key_and_dims_the_one_before() {
        let start = Instant::now();
        let mut fade = HoverFade::new(DURATION);
        fade.set_hovered(Some(1), start);
        assert_eq!(fade.level(&1, at(start, 100)), 1.0);
        fade.set_hovered(Some(2), at(start, 100));
        let later = at(start, 150);
        assert!(fade.level(&1, later) < 1.0 && fade.level(&1, later) > 0.0);
        assert!(fade.level(&2, later) > 0.0);
        fade.set_hovered(None, at(start, 300));
        assert_eq!(fade.level(&1, at(start, 400)), 0.0);
        assert_eq!(fade.level(&2, at(start, 400)), 0.0);
        assert_eq!(fade.next_frame(at(start, 400)), None);
    }

    #[test]
    fn test_a_blink_hides_on_odd_phases_and_rests_after_its_timeout() {
        let start = Instant::now();
        let mut blink = Blink::new(DURATION);
        blink.reset(start);
        assert!(blink.is_visible(at(start, 150)), "steady until driven");
        assert_eq!(blink.next_frame(start), None, "nothing to schedule yet");
        blink.drive();
        assert_eq!(blink.next_frame(start), Some(at(start, 100)));
        assert!(blink.is_visible(at(start, 50)));
        assert!(!blink.is_visible(at(start, 150)));
        assert!(blink.is_visible(at(start, 250)));
        assert_eq!(blink.next_frame(at(start, 150)), Some(at(start, 200)));
        blink.reset(at(start, 150));
        assert!(
            blink.is_visible(at(start, 160)),
            "an input shows it at once"
        );
        let idle = at(start, 150) + BLINK_TIMEOUT;
        assert!(blink.is_visible(idle));
        assert_eq!(blink.next_frame(idle), None, "an idle caret rests");
        blink.set_enabled(false);
        assert!(blink.is_visible(at(start, 250)));
    }
}

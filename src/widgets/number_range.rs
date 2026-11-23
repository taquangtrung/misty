//! [`NumberRange`]: the smallest and largest value a number control (a
//! slider, a stepper, a number setting) takes, and how far a step moves it.

// ========================================================================
// Constants
// ========================================================================

/// How many steps PageUp and PageDown move a number control.
pub(crate) const PAGE_STEPS: f64 = 10.0;

/// The base decimals are counted in.
const DECIMAL_BASE: f64 = 10.0;

/// What parts a number's whole digits from its decimals when written.
const DECIMAL_POINT: char = '.';

// ========================================================================
// Data Structures
// ========================================================================

/// The smallest and largest value of a number setting, and how far one
/// step of Left or Right moves it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberRange {
    /// The largest value.
    pub max: f64,
    /// The smallest value.
    pub min: f64,
    /// How far one step moves the value.
    pub step: f64,
}

// ========================================================================
// NumberRange Implementations
// ========================================================================

impl NumberRange {
    /// Values from `min` to `max`, `step` apart.
    pub const fn new(min: f64, max: f64, step: f64) -> Self {
        Self { max, min, step }
    }

    /// `number` held between the smallest and largest value.
    pub(crate) fn clamp(self, number: f64) -> f64 {
        number.max(self.min).min(self.max)
    }

    /// `number` held to the range and moved to the nearest whole step from
    /// the smallest value, rounded to the step's decimals so adding steps
    /// never shows float noise (`0.30000000000000004`). A step of zero or
    /// less only holds the number to the range.
    pub(crate) fn snap(self, number: f64) -> f64 {
        let clamped = self.clamp(number);
        if self.step <= 0.0 {
            return clamped;
        }
        let steps = ((clamped - self.min) / self.step).round();
        let scale = DECIMAL_BASE.powi(self.decimals() as i32);
        self.clamp(((self.min + steps * self.step) * scale).round() / scale)
    }

    /// `number` moved `steps` steps (negative for down), snapped.
    pub(crate) fn stepped(self, number: f64, steps: f64) -> f64 {
        self.snap(number + steps * self.step)
    }

    /// `number` written with as many decimals as the step has: `12` for a
    /// whole step, `12.5` for a step of `0.5`.
    pub(crate) fn format(self, number: f64) -> String {
        format!("{number:.*}", self.decimals())
    }

    /// How many decimals the step is written with.
    fn decimals(self) -> usize {
        self.step
            .to_string()
            .split_once(DECIMAL_POINT)
            .map_or(0, |(_, decimals)| decimals.len())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: NumberRange = NumberRange::new(8.0, 32.0, 0.5);

    #[test]
    fn test_a_range_snaps_to_its_steps_without_float_noise_and_writes_their_decimals() {
        let tenths = NumberRange::new(0.0, 1.0, 0.1);
        assert_eq!(tenths.stepped(0.2, 1.0), 0.3, "not 0.30000000000000004");
        assert_eq!(tenths.snap(0.74), 0.7);
        assert_eq!(tenths.snap(7.0), 1.0, "held to the range");
        assert_eq!(SIZES.snap(9.3), 9.5, "steps count from the smallest value");
        assert_eq!(SIZES.format(12.0), "12.0");
        assert_eq!(NumberRange::new(0.0, 100.0, 1.0).format(12.0), "12");
        assert_eq!(tenths.format(0.3), "0.3");
    }
}

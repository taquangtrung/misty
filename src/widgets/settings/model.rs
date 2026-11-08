//! The data of a settings page: [`Setting`]s of a few kinds, each with a
//! current and a default [`SettingValue`], grouped in [`SettingsSection`]s,
//! and the edits a key makes to one value.

use super::super::super::base::color::{Rgb, Rgba};
use super::super::super::base::strings::Strings;

// ========================================================================
// Constants
// ========================================================================

/// What starts a color written in hex.
const HEX_PREFIX: char = '#';

/// How many steps PageUp and PageDown move a number control.
pub(crate) const PAGE_STEPS: f64 = 10.0;

/// The base decimals are counted in.
const DECIMAL_BASE: f64 = 10.0;

/// What parts a number's whole digits from its decimals when written.
const DECIMAL_POINT: char = '.';

// ========================================================================
// Data Structures
// ========================================================================

/// One setting: what it is called, what kind of value it takes, and its
/// value now and by default.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    /// The value it goes back to on reset.
    default: SettingValue,
    /// A sentence shown under the rows while the setting has focus, or
    /// `None`.
    description: Option<String>,
    /// The name the app knows it by: `editor.wrap`, or one built at
    /// runtime, as a plugin's setting is.
    id: String,
    /// The kind of value, with its options or range.
    kind: SettingKind,
    /// What the row shows: `Wrap long lines`.
    label: String,
    /// The value now.
    value: SettingValue,
}

/// A titled group of settings, one panel of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsSection {
    /// The settings, top to bottom.
    pub(crate) settings: Vec<Setting>,
    /// The name in the page's sidebar: `Appearance`.
    pub(crate) title: String,
}

/// The value of a setting.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    /// The index of the chosen option.
    Choice(usize),
    /// A color.
    Color(Rgb),
    /// A number inside the setting's range.
    Number(f64),
    /// Names, in order.
    Names(Vec<String>),
    /// A line of text.
    Text(String),
    /// On or off.
    Toggle(bool),
}

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

/// The kind of value a setting takes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SettingKind {
    /// One of these options.
    Choice(Vec<String>),
    /// A color, edited as `#rrggbb`.
    Color,
    /// Names, in order, edited as words; these are the names it may take,
    /// offered while it has focus.
    Names(Vec<String>),
    /// A number in this range.
    Number(NumberRange),
    /// A line of text.
    Text,
    /// On or off.
    Toggle,
}

// ========================================================================
// Setting Implementations
// ========================================================================

impl Setting {
    /// An on or off setting, `default` until changed.
    pub fn toggle(id: impl Into<String>, label: impl Into<String>, default: bool) -> Self {
        Self::of_kind(
            id,
            label,
            SettingKind::Toggle,
            SettingValue::Toggle(default),
        )
    }

    /// A setting taking one of `options`, the one at `default` until
    /// changed (the first when `default` is past the end).
    pub fn choice(
        id: impl Into<String>,
        label: impl Into<String>,
        options: &[&str],
        default: usize,
    ) -> Self {
        let default = match default < options.len() {
            true => default,
            false => 0,
        };
        let options = options.iter().map(|option| (*option).to_owned()).collect();
        Self::of_kind(
            id,
            label,
            SettingKind::Choice(options),
            SettingValue::Choice(default),
        )
    }

    /// A number setting inside `range`, `default` (held to the range) until
    /// changed.
    pub fn number(
        id: impl Into<String>,
        label: impl Into<String>,
        default: f64,
        range: NumberRange,
    ) -> Self {
        let default = range.clamp(default);
        Self::of_kind(
            id,
            label,
            SettingKind::Number(range),
            SettingValue::Number(default),
        )
    }

    /// A one-line text setting, `default` until changed.
    pub fn text(
        id: impl Into<String>,
        label: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        let default = SettingValue::Text(default.into());
        Self::of_kind(id, label, SettingKind::Text, default)
    }

    /// A setting of names in order, `default` until changed, edited as
    /// words; `choices` are the names it may take, offered while it has
    /// focus (others are taken too).
    pub fn names(
        id: impl Into<String>,
        label: impl Into<String>,
        default: Vec<String>,
        choices: Vec<String>,
    ) -> Self {
        Self::of_kind(
            id,
            label,
            SettingKind::Names(choices),
            SettingValue::Names(default),
        )
    }

    /// A color setting, `default` until changed.
    pub fn color(id: impl Into<String>, label: impl Into<String>, default: Rgb) -> Self {
        Self::of_kind(id, label, SettingKind::Color, SettingValue::Color(default))
    }

    /// This setting with `description` shown while it has focus.
    pub fn with_description(self, description: impl Into<String>) -> Self {
        Self {
            description: Some(description.into()),
            ..self
        }
    }

    /// The name the app knows the setting by.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What the row shows.
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    /// The value now.
    pub fn value(&self) -> &SettingValue {
        &self.value
    }

    /// The options of a choice setting; empty for any other kind.
    pub(crate) fn options(&self) -> &[String] {
        match &self.kind {
            SettingKind::Choice(options) => options,
            SettingKind::Color
            | SettingKind::Names(_)
            | SettingKind::Number(_)
            | SettingKind::Text
            | SettingKind::Toggle => &[],
        }
    }

    /// The names a setting of names may take that it does not hold; empty
    /// for any other kind.
    pub(crate) fn names_offered(&self) -> Vec<&str> {
        match (&self.kind, &self.value) {
            (SettingKind::Names(choices), SettingValue::Names(held)) => choices
                .iter()
                .filter(|choice| !held.contains(choice))
                .map(String::as_str)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The sentence shown while the setting has focus.
    pub(crate) fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The kind of value it takes.
    pub(crate) fn kind(&self) -> &SettingKind {
        &self.kind
    }

    /// Sets the value to `value` when the setting takes it (the same kind,
    /// a choice that exists; a number is held to the range); whether the
    /// value changed.
    pub(crate) fn set_value(&mut self, value: SettingValue) -> Result<bool, SettingValue> {
        let value = match (&self.kind, value) {
            (SettingKind::Choice(options), SettingValue::Choice(index))
                if index < options.len() =>
            {
                SettingValue::Choice(index)
            }
            (SettingKind::Color, SettingValue::Color(color)) => SettingValue::Color(color),
            (SettingKind::Names(_), SettingValue::Names(names)) => SettingValue::Names(names),
            (SettingKind::Number(range), SettingValue::Number(number)) if number.is_finite() => {
                SettingValue::Number(range.clamp(number))
            }
            (SettingKind::Text, SettingValue::Text(text)) => SettingValue::Text(text),
            (SettingKind::Toggle, SettingValue::Toggle(on)) => SettingValue::Toggle(on),
            (
                SettingKind::Choice(_)
                | SettingKind::Color
                | SettingKind::Names(_)
                | SettingKind::Number(_)
                | SettingKind::Text
                | SettingKind::Toggle,
                refused,
            ) => return Err(refused),
        };
        let changed = value != self.value;
        self.value = value;
        Ok(changed)
    }

    /// Steps the value `delta` places: a choice to its neighbor, wrapping;
    /// a number by its step, held to the range; a toggle on for a step
    /// forward and off for a step back. Whether the value changed; other
    /// kinds do not step.
    pub(crate) fn step(&mut self, delta: isize) -> bool {
        let stepped = match (&self.kind, &self.value) {
            (SettingKind::Choice(options), SettingValue::Choice(index)) if !options.is_empty() => {
                let count = options.len() as isize;
                SettingValue::Choice((*index as isize + delta).rem_euclid(count) as usize)
            }
            (SettingKind::Number(range), SettingValue::Number(number)) => {
                SettingValue::Number(range.clamp(number + range.step * delta as f64))
            }
            (SettingKind::Toggle, SettingValue::Toggle(_)) => SettingValue::Toggle(delta > 0),
            (
                SettingKind::Choice(_)
                | SettingKind::Color
                | SettingKind::Names(_)
                | SettingKind::Number(_)
                | SettingKind::Text
                | SettingKind::Toggle,
                _,
            ) => return false,
        };
        self.set_value(stepped).unwrap_or(false)
    }

    /// The value as the row shows it: [`SettingValue::Toggle`] in
    /// `strings`' words (`On`, `Off`), anything else as
    /// [`Setting::edit_text`] writes it.
    pub(crate) fn value_text(&self, strings: &Strings) -> String {
        match &self.value {
            SettingValue::Toggle(true) => strings.settings_on.to_string(),
            SettingValue::Toggle(false) => strings.settings_off.to_string(),
            _ => self.edit_text(),
        }
    }

    /// The value as an edit of it starts: the option's name, the number,
    /// the text, `#rrggbb`. A toggle is switched, never edited as text, so
    /// its text is empty.
    pub(crate) fn edit_text(&self) -> String {
        match &self.value {
            SettingValue::Choice(index) => self.options().get(*index).cloned().unwrap_or_default(),
            SettingValue::Color(color) => hex(*color),
            SettingValue::Names(names) => names.join(" "),
            SettingValue::Number(number) => number.to_string(),
            SettingValue::Text(text) => text.clone(),
            SettingValue::Toggle(_) => String::new(),
        }
    }

    /// The value written as `text` in an edit of this setting, or `None`
    /// when it does not read as one: a number that does not parse, a color
    /// that is not `#rrggbb`. Only text, names (as words), number, and color
    /// settings are edited as text.
    pub(crate) fn parse(&self, text: &str) -> Option<SettingValue> {
        let text = text.trim();
        match &self.kind {
            SettingKind::Color => parse_hex(text).map(SettingValue::Color),
            SettingKind::Names(_) => Some(SettingValue::Names(
                text.split_whitespace().map(str::to_owned).collect(),
            )),
            SettingKind::Number(range) => text
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite())
                .map(|number| SettingValue::Number(range.clamp(number))),
            SettingKind::Text => Some(SettingValue::Text(text.to_owned())),
            SettingKind::Choice(_) | SettingKind::Toggle => None,
        }
    }

    /// Whether the value is typed into a field: text, names, a number, or
    /// a color.
    pub(crate) fn is_typed(&self) -> bool {
        matches!(
            self.kind,
            SettingKind::Color | SettingKind::Names(_) | SettingKind::Number(_) | SettingKind::Text
        )
    }

    /// Whether the value differs from the default.
    pub(crate) fn is_modified(&self) -> bool {
        self.value != self.default
    }

    /// Puts the default back; whether the value changed.
    pub(crate) fn reset(&mut self) -> bool {
        let changed = self.is_modified();
        self.value = self.default.clone();
        changed
    }

    /// A setting of `kind` whose value and default are `default`.
    fn of_kind(
        id: impl Into<String>,
        label: impl Into<String>,
        kind: SettingKind,
        default: SettingValue,
    ) -> Self {
        Self {
            default: default.clone(),
            description: None,
            id: id.into(),
            kind,
            label: label.into(),
            value: default,
        }
    }
}

// ========================================================================
// SettingsSection Implementations
// ========================================================================

impl SettingsSection {
    /// An empty section titled `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            settings: Vec::new(),
            title: title.into(),
        }
    }

    /// This section with `setting` added at its end.
    pub fn with(mut self, setting: Setting) -> Self {
        self.settings.push(setting);
        self
    }
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
    fn clamp(self, number: f64) -> f64 {
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
// Functions
// ========================================================================

/// `color` as `#rrggbb`.
fn hex(color: Rgb) -> String {
    format!("{HEX_PREFIX}{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// The opaque color `text` writes as `#rrggbb` (or `#rrggbbff`).
fn parse_hex(text: &str) -> Option<Rgb> {
    let Rgba { a, b, g, r } = Rgba::from_hex(text).ok()?;
    (a == u8::MAX).then_some(Rgb::new(r, g, b))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: NumberRange = NumberRange::new(8.0, 32.0, 0.5);

    #[test]
    fn test_a_choice_steps_both_ways_and_wraps() {
        let mut setting = Setting::choice("theme", "Theme", &["Dark", "Light", "Auto"], 0);
        assert!(setting.step(-1));
        assert_eq!(setting.value(), &SettingValue::Choice(2), "wrapped back");
        assert!(setting.step(1));
        assert_eq!(setting.edit_text(), "Dark");
    }

    #[test]
    fn test_a_number_steps_by_its_step_and_stops_at_its_range() {
        let mut setting = Setting::number("size", "Size", 31.5, SIZES);
        assert!(setting.step(1));
        assert_eq!(setting.value(), &SettingValue::Number(32.0));
        assert!(!setting.step(1), "already at the largest value");
        assert_eq!(
            Setting::number("size", "Size", 99.0, SIZES).value(),
            &SettingValue::Number(32.0),
            "the default is held to the range"
        );
    }

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

    #[test]
    fn test_parsing_an_edit_refuses_what_does_not_read_as_the_kind() {
        let size = Setting::number("size", "Size", 12.0, SIZES);
        assert_eq!(size.parse(" 4 "), Some(SettingValue::Number(8.0)));
        assert_eq!(size.parse("big"), None);
        assert_eq!(size.parse("NaN"), None);
        let accent = Setting::color("accent", "Accent", Rgb::new(0, 0, 0));
        assert_eq!(
            accent.parse("#89B4FA"),
            Some(SettingValue::Color(Rgb::new(0x89, 0xb4, 0xfa)))
        );
        assert_eq!(accent.parse("#89b4fa80"), None, "not opaque");
        assert_eq!(accent.parse("89b4fa"), None);
        assert_eq!(accent.edit_text(), "#000000");
    }

    #[test]
    fn test_setting_a_value_of_another_kind_or_a_missing_option_is_refused() {
        let mut theme = Setting::choice("theme", "Theme", &["Dark", "Light"], 1);
        assert_eq!(
            theme.set_value(SettingValue::Choice(2)),
            Err(SettingValue::Choice(2))
        );
        assert!(theme.set_value(SettingValue::Toggle(true)).is_err());
        assert_eq!(theme.set_value(SettingValue::Choice(0)), Ok(true));
        assert!(theme.is_modified());
        assert!(theme.reset());
        assert!(!theme.reset(), "already the default");
    }
}

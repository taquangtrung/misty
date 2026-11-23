//! The data of a settings page: [`Setting`]s of a few kinds, each with a
//! current and a default [`SettingValue`], grouped in [`SettingsSection`]s,
//! and the edits a key makes to one value. An action row is a setting
//! too, one with a button and no value to edit.

use super::super::super::base::color::Rgb;
use super::super::super::widgets::number_range::NumberRange;

// ========================================================================
// Constants
// ========================================================================

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

/// The kind of value a setting takes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SettingKind {
    /// A button with this caption, which does something rather than holds
    /// a value.
    Action(String),
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

    /// A row with a button reading `caption` that does something rather
    /// than holds a value (`Clear history`, `Rescan folders`): Enter,
    /// Space, or a click on it answers
    /// [`SettingsPageResponse::Activated`](super::SettingsPageResponse::Activated)
    /// with `id` for the app to do it. Its value is an empty text that
    /// never changes.
    pub fn action(
        id: impl Into<String>,
        label: impl Into<String>,
        caption: impl Into<String>,
    ) -> Self {
        Self::of_kind(
            id,
            label,
            SettingKind::Action(caption.into()),
            SettingValue::Text(String::new()),
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

    /// This setting holding `value`, the app's value now, when it takes
    /// it (the same kind, a choice that exists; a number held to the
    /// range); a value it does not take leaves it as it was.
    pub fn with_value(mut self, value: SettingValue) -> Self {
        let _ = self.set_value(value);
        self
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
                SettingKind::Action(_)
                | SettingKind::Choice(_)
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

    /// The name in the page's sidebar.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The settings, top to bottom.
    pub fn settings(&self) -> &[Setting] {
        &self.settings
    }

    /// Sets the setting `id` to `value` when it takes it (see
    /// [`Setting::with_value`]): an app showing an edit before its own
    /// state comes back with it. Whether the value changed.
    pub fn set_value(&mut self, id: &str, value: SettingValue) -> bool {
        self.settings
            .iter_mut()
            .find(|setting| setting.id() == id)
            .is_some_and(|setting| setting.set_value(value) == Ok(true))
    }
}

// ========================================================================
// Functions
// ========================================================================

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: NumberRange = NumberRange::new(8.0, 32.0, 0.5);

    #[test]
    fn test_a_number_setting_holds_its_value_to_its_range() {
        assert_eq!(
            Setting::number("size", "Size", 99.0, SIZES).value(),
            &SettingValue::Number(32.0),
            "the default is held to the range"
        );
        let size =
            Setting::number("size", "Size", 12.0, SIZES).with_value(SettingValue::Number(4.0));
        assert_eq!(size.value(), &SettingValue::Number(8.0));
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
        assert_eq!(
            theme.set_value(SettingValue::Choice(0)),
            Ok(false),
            "already it"
        );
    }
}

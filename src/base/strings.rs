//! [`Strings`]: the words misty's own widgets show and tell screen readers
//! (a title bar's button names, a tree's loading row, the settings page's
//! hints), carried by the [`Theme`](super::theme::Theme) so an app
//! translates every widget by setting them once.
//!
//! Text the app hands a widget (a dialog's buttons, a list's rows, a
//! checkbox's label) is the app's to translate. Key labels such as
//! `Ctrl+S` take their modifier names from here, and their key names
//! (`S`, `Enter`, `F2`) stay as keyboards print them.

use std::borrow::Cow;

// ========================================================================
// Constants
// ========================================================================

/// What opens a place for a value in a template, as in
/// `"Could not load: {}"` or `"{0} is also bound to {1}"`.
const PLACE_OPEN: char = '{';

/// What closes a place for a value in a template.
const PLACE_CLOSE: char = '}';

// ========================================================================
// Data Structures
// ========================================================================

/// The words misty's widgets show and give screen readers, in one place,
/// so an app in another language translates them all at once:
///
/// ```
/// use std::borrow::Cow;
/// use misty::{Strings, Theme};
///
/// let theme = Theme {
///     strings: Strings {
///         close: Cow::Borrowed("Schließen"),
///         load_failed: Cow::Borrowed("Laden fehlgeschlagen: {}"),
///         ..Strings::english()
///     },
///     ..Theme::dark()
/// };
/// assert_eq!(theme.strings.close, "Schließen");
/// ```
///
/// A template takes a value where it says `{}`, or `{0}`, `{1}`, ... by
/// number when it takes several, so a translation can put each anywhere
/// in its sentence. The default is [`Strings::english`].
///
/// Words are added as misty gains widgets, so build a translation with
/// struct-update syntax from a built-in, as above (`..Strings::default()`
/// or `..Strings::english()`): words not yet translated stay in English,
/// and a translation spelled out field by field stops compiling when a
/// word is added.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Strings {
    /// A breadcrumb's name, for screen readers.
    pub breadcrumb: Cow<'static, str>,
    /// The button that stops capturing a key in the settings page's
    /// keybinding editor.
    pub cancel: Cow<'static, str>,
    /// A title bar's close button, for screen readers.
    pub close: Cow<'static, str>,
    /// The title of the command palette.
    pub command_palette: Cow<'static, str>,
    /// A stepper's minus button, for screen readers.
    pub decrease: Cow<'static, str>,
    /// What screen readers say while hint labels show over targets.
    pub hints: Cow<'static, str>,
    /// A stepper's plus button, for screen readers.
    pub increase: Cow<'static, str>,
    /// The Alt modifier as a key label names it, as in `Alt+X`.
    pub key_alt: Cow<'static, str>,
    /// The settings page's note that the focused command's keys are taken:
    /// a template, `{0}` taking the keys and `{1}` the other commands.
    pub key_conflict: Cow<'static, str>,
    /// The Ctrl modifier as a key label names it, as in `Ctrl+S`.
    pub key_ctrl: Cow<'static, str>,
    /// What the settings page's keybinding editor shows while it waits for
    /// a key.
    pub key_prompt: Cow<'static, str>,
    /// The Shift modifier as a key label names it, as in `Ctrl+Shift+T`.
    pub key_shift: Cow<'static, str>,
    /// The Super (Windows, Command) modifier as a key label names it, as in
    /// `Super+Q`.
    pub key_super: Cow<'static, str>,
    /// What a list's which-key hints say `g g` does.
    pub list_first_row: Cow<'static, str>,
    /// What a list's which-key hints say `g j` does.
    pub list_row_down: Cow<'static, str>,
    /// What a list's which-key hints say `z b` does.
    pub list_row_to_bottom: Cow<'static, str>,
    /// What a list's which-key hints say `z z` does.
    pub list_row_to_middle: Cow<'static, str>,
    /// What a list's which-key hints say `z t` does.
    pub list_row_to_top: Cow<'static, str>,
    /// What a list's which-key hints say `g k` does.
    pub list_row_up: Cow<'static, str>,
    /// A tree row whose children failed to load: a template, `{}` taking
    /// the reason.
    pub load_failed: Cow<'static, str>,
    /// A tree row whose children are still loading.
    pub loading: Cow<'static, str>,
    /// A title bar's maximize button, for screen readers.
    pub maximize: Cow<'static, str>,
    /// A title bar's menu button, for screen readers.
    pub menu: Cow<'static, str>,
    /// A title bar's minimize button, for screen readers.
    pub minimize: Cow<'static, str>,
    /// The button opening a menu of what did not fit (a toolbar's overflow
    /// button, a breadcrumb's ellipsis), for screen readers.
    pub more: Cow<'static, str>,
    /// A tab strip's new-tab button, for screen readers.
    pub new_tab: Cow<'static, str>,
    /// A tab strip's next-tab button, for screen readers.
    pub next_tab: Cow<'static, str>,
    /// What a picker's hints line calls the key that shows a row's actions.
    pub picker_actions: Cow<'static, str>,
    /// What a picker's count ends with once its source stopped at its cap
    /// with rows left: `(capped)`.
    pub picker_capped: Cow<'static, str>,
    /// The one action of a picker that is a list with one thing to do on a
    /// row, such as the command palette's, as its action menu names it.
    pub picker_choose: Cow<'static, str>,
    /// What a picker's hints line calls the key that shows or hides its
    /// side panel.
    pub picker_preview: Cow<'static, str>,
    /// What a picker's hints line calls the key that saves its rows.
    pub picker_save: Cow<'static, str>,
    /// A tab strip's previous-tab button, for screen readers.
    pub prev_tab: Cow<'static, str>,
    /// A message that came more than once, in a toast, the status bar, or
    /// the notice log: a template, `{0}` taking the message and `{1}` how
    /// many times it came.
    pub repeated: Cow<'static, str>,
    /// A title bar's restore button (a maximized window's maximize
    /// button), for screen readers.
    pub restore: Cow<'static, str>,
    /// What the settings page's empty filter line says.
    pub settings_filter_placeholder: Cow<'static, str>,
    /// The settings page's note on a setting of names, naming the ones it
    /// may take that it does not hold: a template, `{0}` taking them.
    pub settings_names_hint: Cow<'static, str>,
    /// What the settings page shows for a switched-off toggle.
    pub settings_off: Cow<'static, str>,
    /// What the settings page shows for a switched-on toggle.
    pub settings_on: Cow<'static, str>,
    /// The button on a settings row whose value a later scope sets, which
    /// drops that value for the one before it.
    pub settings_reset: Cow<'static, str>,
    /// The settings page's note, in its first scope, on a setting a later
    /// scope sets too.
    pub settings_overridden: Cow<'static, str>,
    /// The settings page's note, in a later scope, on a setting the scope
    /// has a value of its own for.
    pub settings_own_value: Cow<'static, str>,
    /// A toolbar button's tooltip naming its command and the keys that run
    /// it: a template, `{0}` taking the command's title and `{1}` its keys.
    pub tooltip_keys: Cow<'static, str>,
    /// The heading over a transient's commands, when it has switches too.
    pub transient_commands: Cow<'static, str>,
    /// The heading over a transient's switches, when it has commands too.
    pub transient_switches: Cow<'static, str>,
    /// What the settings page shows for a command with no key.
    pub unbound: Cow<'static, str>,
    /// What a which-key popup's heading adds when some keys did not fit: a
    /// template, `{}` taking how many.
    pub which_key_hidden: Cow<'static, str>,
}

// ========================================================================
// Strings Implementations
// ========================================================================

impl Strings {
    /// The words in English: the built-in themes' strings.
    pub const fn english() -> Self {
        Self {
            breadcrumb: Cow::Borrowed("Breadcrumb"),
            cancel: Cow::Borrowed("Cancel"),
            close: Cow::Borrowed("Close"),
            command_palette: Cow::Borrowed("Commands"),
            decrease: Cow::Borrowed("Decrease"),
            hints: Cow::Borrowed("Hint mode: type a label to choose, Esc cancels"),
            increase: Cow::Borrowed("Increase"),
            key_alt: Cow::Borrowed("Alt"),
            key_conflict: Cow::Borrowed("{0} is also bound to {1}"),
            key_ctrl: Cow::Borrowed("Ctrl"),
            key_prompt: Cow::Borrowed("press a key"),
            key_shift: Cow::Borrowed("Shift"),
            key_super: Cow::Borrowed("Super"),
            list_first_row: Cow::Borrowed("first row, or the count's"),
            list_row_down: Cow::Borrowed("row down"),
            list_row_to_bottom: Cow::Borrowed("cursor row to the bottom"),
            list_row_to_middle: Cow::Borrowed("cursor row to the middle"),
            list_row_to_top: Cow::Borrowed("cursor row to the top"),
            list_row_up: Cow::Borrowed("row up"),
            load_failed: Cow::Borrowed("Could not load: {}"),
            loading: Cow::Borrowed("Loading…"),
            maximize: Cow::Borrowed("Maximize"),
            menu: Cow::Borrowed("Menu"),
            minimize: Cow::Borrowed("Minimize"),
            more: Cow::Borrowed("More"),
            new_tab: Cow::Borrowed("New Tab"),
            next_tab: Cow::Borrowed("Next Tab"),
            picker_actions: Cow::Borrowed("actions"),
            picker_capped: Cow::Borrowed("(capped)"),
            picker_choose: Cow::Borrowed("Choose"),
            picker_preview: Cow::Borrowed("preview"),
            picker_save: Cow::Borrowed("save"),
            prev_tab: Cow::Borrowed("Previous Tab"),
            repeated: Cow::Borrowed("{0} ({1}x)"),
            restore: Cow::Borrowed("Restore"),
            settings_filter_placeholder: Cow::Borrowed("/ to filter"),
            settings_names_hint: Cow::Borrowed("Also takes: {0}"),
            settings_off: Cow::Borrowed("Off"),
            settings_on: Cow::Borrowed("On"),
            settings_reset: Cow::Borrowed("Reset"),
            settings_overridden: Cow::Borrowed(
                "A later scope sets this too, so this value is not in force there",
            ),
            settings_own_value: Cow::Borrowed(
                "This scope sets its own value, in force over the wider scope's",
            ),
            tooltip_keys: Cow::Borrowed("{0} ({1})"),
            transient_commands: Cow::Borrowed("Commands"),
            transient_switches: Cow::Borrowed("Switches"),
            unbound: Cow::Borrowed("unbound"),
            which_key_hidden: Cow::Borrowed("{} more not shown"),
        }
    }

    /// The string a theme file names `name` (the field's name with dashes
    /// for underscores, as in `load-failed`), to set it.
    #[cfg(feature = "kdl")]
    pub(crate) fn named_mut(&mut self, name: &str) -> Option<&mut Cow<'static, str>> {
        match name {
            "breadcrumb" => Some(&mut self.breadcrumb),
            "cancel" => Some(&mut self.cancel),
            "close" => Some(&mut self.close),
            "command-palette" => Some(&mut self.command_palette),
            "decrease" => Some(&mut self.decrease),
            "hints" => Some(&mut self.hints),
            "increase" => Some(&mut self.increase),
            "key-alt" => Some(&mut self.key_alt),
            "key-conflict" => Some(&mut self.key_conflict),
            "key-ctrl" => Some(&mut self.key_ctrl),
            "key-prompt" => Some(&mut self.key_prompt),
            "key-shift" => Some(&mut self.key_shift),
            "key-super" => Some(&mut self.key_super),
            "list-first-row" => Some(&mut self.list_first_row),
            "list-row-down" => Some(&mut self.list_row_down),
            "list-row-to-bottom" => Some(&mut self.list_row_to_bottom),
            "list-row-to-middle" => Some(&mut self.list_row_to_middle),
            "list-row-to-top" => Some(&mut self.list_row_to_top),
            "list-row-up" => Some(&mut self.list_row_up),
            "load-failed" => Some(&mut self.load_failed),
            "loading" => Some(&mut self.loading),
            "maximize" => Some(&mut self.maximize),
            "menu" => Some(&mut self.menu),
            "minimize" => Some(&mut self.minimize),
            "more" => Some(&mut self.more),
            "new-tab" => Some(&mut self.new_tab),
            "next-tab" => Some(&mut self.next_tab),
            "picker-actions" => Some(&mut self.picker_actions),
            "picker-capped" => Some(&mut self.picker_capped),
            "picker-choose" => Some(&mut self.picker_choose),
            "picker-preview" => Some(&mut self.picker_preview),
            "picker-save" => Some(&mut self.picker_save),
            "prev-tab" => Some(&mut self.prev_tab),
            "repeated" => Some(&mut self.repeated),
            "restore" => Some(&mut self.restore),
            "settings-filter-placeholder" => Some(&mut self.settings_filter_placeholder),
            "settings-names-hint" => Some(&mut self.settings_names_hint),
            "settings-off" => Some(&mut self.settings_off),
            "settings-on" => Some(&mut self.settings_on),
            "settings-reset" => Some(&mut self.settings_reset),
            "settings-overridden" => Some(&mut self.settings_overridden),
            "settings-own-value" => Some(&mut self.settings_own_value),
            "tooltip-keys" => Some(&mut self.tooltip_keys),
            "transient-commands" => Some(&mut self.transient_commands),
            "transient-switches" => Some(&mut self.transient_switches),
            "unbound" => Some(&mut self.unbound),
            "which-key-hidden" => Some(&mut self.which_key_hidden),
            _ => None,
        }
    }
}

impl Default for Strings {
    fn default() -> Self {
        Self::english()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `template` with `values` set in: `{}` takes the next value in turn,
/// and `{0}`, `{1}`, ... the value at that index, so a translation may
/// order them as its language does. A place naming no value is left as
/// written, and a value no place names is not shown.
pub(crate) fn fill(template: &str, values: &[&str]) -> String {
    let mut text = String::new();
    let mut next = 0;
    let mut rest = template;
    while let Some(open) = rest.find(PLACE_OPEN) {
        text.push_str(&rest[..open]);
        let after = &rest[open + PLACE_OPEN.len_utf8()..];
        let Some(close) = after.find(PLACE_CLOSE) else {
            rest = &rest[open..];
            break;
        };
        let inside = &after[..close];
        let index = match inside.is_empty() {
            true => {
                next += 1;
                Some(next - 1)
            }
            false => inside.parse::<usize>().ok(),
        };
        match index.and_then(|index| values.get(index)) {
            Some(value) => text.push_str(value),
            None => text.push_str(&rest[open..open + PLACE_OPEN.len_utf8() + close + 1]),
        }
        rest = &after[close + PLACE_CLOSE.len_utf8()..];
    }
    text.push_str(rest);
    text
}

/// `text`, a message that came `repeats` times, as `strings` write it:
/// unchanged when it came once, else through [`Strings::repeated`].
pub(crate) fn repeated(strings: &Strings, text: &str, repeats: usize) -> String {
    match repeats {
        1 => text.to_owned(),
        repeats => fill(&strings.repeated, &[text, &repeats.to_string()]),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_template_takes_its_value_wherever_it_says_so() {
        let english = Strings::english();
        assert_eq!(
            fill(&english.load_failed, &["permission denied"]),
            "Could not load: permission denied"
        );
        assert_eq!(
            fill("{} konnte nicht laden", &["x"]),
            "x konnte nicht laden"
        );
        assert_eq!(
            fill(&english.key_conflict, &["Ctrl+S", "Save"]),
            "Ctrl+S is also bound to Save"
        );
        assert_eq!(
            fill("{1} nutzt schon {0}", &["Ctrl+S", "Save"]),
            "Save nutzt schon Ctrl+S",
            "numbered places in another order"
        );
        assert_eq!(fill("{} and {}", &["a", "b"]), "a and b", "in turn");
        assert_eq!(
            fill("{} {2} {x} {", &["a"]),
            "a {2} {x} {",
            "places naming no value stay as written"
        );
    }

    #[test]
    fn test_a_message_is_counted_only_when_it_came_again() {
        let english = Strings::english();
        assert_eq!(repeated(&english, "Saved", 1), "Saved");
        assert_eq!(repeated(&english, "Saved", 3), "Saved (3x)");
    }

    #[cfg(feature = "kdl")]
    #[test]
    fn test_every_field_has_a_theme_file_name() {
        let mut strings = Strings::english();
        let names = [
            "breadcrumb",
            "cancel",
            "close",
            "command-palette",
            "decrease",
            "hints",
            "increase",
            "key-alt",
            "key-conflict",
            "key-ctrl",
            "key-prompt",
            "key-shift",
            "key-super",
            "list-first-row",
            "list-row-down",
            "list-row-to-bottom",
            "list-row-to-middle",
            "list-row-to-top",
            "list-row-up",
            "load-failed",
            "loading",
            "maximize",
            "menu",
            "minimize",
            "more",
            "new-tab",
            "next-tab",
            "picker-actions",
            "picker-capped",
            "picker-choose",
            "picker-preview",
            "picker-save",
            "prev-tab",
            "repeated",
            "restore",
            "settings-filter-placeholder",
            "settings-names-hint",
            "settings-off",
            "settings-on",
            "settings-overridden",
            "settings-own-value",
            "settings-reset",
            "tooltip-keys",
            "transient-commands",
            "transient-switches",
            "unbound",
            "which-key-hidden",
        ];
        for name in names {
            *strings.named_mut(name).expect(name) = Cow::Borrowed("x");
        }
        assert!(strings.named_mut("nope").is_none());
        let all_set = Strings {
            breadcrumb: "x".into(),
            cancel: "x".into(),
            close: "x".into(),
            command_palette: "x".into(),
            decrease: "x".into(),
            hints: "x".into(),
            increase: "x".into(),
            key_alt: "x".into(),
            key_conflict: "x".into(),
            key_ctrl: "x".into(),
            key_prompt: "x".into(),
            key_shift: "x".into(),
            key_super: "x".into(),
            list_first_row: "x".into(),
            list_row_down: "x".into(),
            list_row_to_bottom: "x".into(),
            list_row_to_middle: "x".into(),
            list_row_to_top: "x".into(),
            list_row_up: "x".into(),
            load_failed: "x".into(),
            loading: "x".into(),
            maximize: "x".into(),
            menu: "x".into(),
            minimize: "x".into(),
            more: "x".into(),
            new_tab: "x".into(),
            next_tab: "x".into(),
            picker_actions: "x".into(),
            picker_capped: "x".into(),
            picker_choose: "x".into(),
            picker_preview: "x".into(),
            picker_save: "x".into(),
            prev_tab: "x".into(),
            repeated: "x".into(),
            restore: "x".into(),
            settings_filter_placeholder: "x".into(),
            settings_names_hint: "x".into(),
            settings_off: "x".into(),
            settings_on: "x".into(),
            settings_reset: "x".into(),
            settings_overridden: "x".into(),
            settings_own_value: "x".into(),
            tooltip_keys: "x".into(),
            transient_commands: "x".into(),
            transient_switches: "x".into(),
            unbound: "x".into(),
            which_key_hidden: "x".into(),
        };
        assert_eq!(strings, all_set, "each name sets its own field");
    }
}

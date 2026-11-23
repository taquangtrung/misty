//! The form controls in one window, described as a tree: a checkbox and a
//! switch, a radio group that switches the theme, a select that sets the
//! chrome font, a slider driving a progress bar, a stepper, a progress bar
//! of unknown length, and a text area, in a form that scrolls when the
//! window is too short for it. Tab
//! and Shift+Tab move between them, scrolling the one focused into view, a
//! click focuses one, and the line at the bottom says what changed last.
//! The window follows the system's appearance: light or dark, its accent
//! color, and less motion, at start and as they change.
//!
//! Each control takes the keys its module lists: Space toggles a checkbox,
//! the arrows (or `h`, `j`, `k`, `l`) move a radio group's choice and a
//! slider's or stepper's value, Enter or Space opens the select, whose
//! list floats over the form and keeps the keyboard until Esc, a choice,
//! or a click outside closes it, and the text area edits as a text field
//! does, Enter adding a line. Ctrl+Q quits.
//!
//! ```sh
//! cargo run -p misty --example controls
//! ```

use misty::view::{
    checkbox, column, field, form, progress, radio, select, slider, stepper, text, text_area,
};
use misty::{
    App, AppCx, Appearance, ChromeText, Element, Key, KeyCode, Mods, NumberRange, Size, Theme,
    ThemeToken, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty controls";

/// The window's size at start, in logical pixels.
const SIZE: Size = Size::new(720, 560);

/// The blank cell columns at the left and right of the content.
const MARGIN_COLS: f32 = 2.0;

/// The blank rows above and below the form.
const MARGIN_ROWS: f32 = 2.0;

/// The blank rows above and below the status line.
const STATUS_MARGIN_ROWS: f32 = 1.0;

/// The rows the text area takes.
const NOTES_ROWS: usize = 6;

/// The volume slider's range, in percent.
const VOLUME: NumberRange = NumberRange::new(0.0, 100.0, 5.0);

/// The volume at start, in percent.
const VOLUME_START: f64 = 40.0;

/// The tab width stepper's range.
const TAB_WIDTH: NumberRange = NumberRange::new(1.0, 8.0, 1.0);

/// The tab width at start.
const TAB_WIDTH_START: f64 = 4.0;

/// The theme radio group's options: dark when `true`.
const THEMES: [(bool, &str); 2] = [(true, "Dark"), (false, "Light")];

/// The chrome font select's options.
const FONTS: [(ChromeText, &str); 2] = [
    (ChromeText::Proportional, "Proportional"),
    (ChromeText::Cells, "Monospace cells"),
];

/// What the text area holds at start.
const NOTES: &str = "A text area wraps long lines at its width, keeps the caret in view as \
    it scrolls, and edits with the keys a text field takes.\n\nEnter adds a line; Tab moves on.";

// ========================================================================
// Data Structures
// ========================================================================

/// The demo's state: every control's value and what changed last.
struct Demo {
    /// Whether autosave is on.
    autosave: bool,
    /// What the text area holds.
    notes: String,
    /// Changed whenever `notes` is, for the text area.
    notes_version: u64,
    /// What changed last.
    status: String,
    /// The tab width.
    tab_width: f64,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The volume, in percent.
    volume: f64,
    /// Whether long lines wrap.
    wrap: bool,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo at start, in the dark theme.
    fn new() -> Self {
        Self {
            autosave: false,
            notes: NOTES.to_owned(),
            notes_version: 0,
            status: String::from("nothing yet"),
            tab_width: TAB_WIDTH_START,
            theme: Theme::dark(),
            volume: VOLUME_START,
            wrap: true,
        }
    }

    /// Whether the window is drawn in the dark theme.
    fn is_dark(&self) -> bool {
        self.theme.background == Theme::dark().background
    }

    /// Switches to the dark theme when `dark`, or to the light one,
    /// keeping the chrome font chosen.
    fn set_dark(&mut self, dark: bool) {
        let chrome_text = self.theme.chrome_text;
        self.theme = if dark { Theme::dark() } else { Theme::light() };
        self.theme.chrome_text = chrome_text;
        self.status = format!("theme: {}", if dark { "dark" } else { "light" });
    }
}

impl App for Demo {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        let level = (self.volume / VOLUME.max) as f32;
        let status = text("Last: ")
            .muted()
            .then(self.status.as_str())
            .color(ThemeToken::Accent);
        column([
            form([
                field(
                    "Editor",
                    checkbox("Wrap long lines", self.wrap).on_change(|app: &mut Self, on| {
                        app.wrap = on;
                        app.status = format!("wrap: {}", on_off(on));
                    }),
                ),
                field(
                    "",
                    checkbox("Autosave", self.autosave)
                        .switch()
                        .on_change(|app: &mut Self, on| {
                            app.autosave = on;
                            app.status = format!("autosave: {}", on_off(on));
                        }),
                ),
                field(
                    "Theme",
                    radio(&THEMES, &self.is_dark())
                        .on_change(|app: &mut Self, dark| app.set_dark(dark)),
                ),
                field(
                    "Chrome font",
                    select(&FONTS, &self.theme.chrome_text).on_change(
                        |app: &mut Self, chrome_text| {
                            app.theme.chrome_text = chrome_text;
                            app.status = format!("chrome font: {chrome_text:?}");
                        },
                    ),
                ),
                field(
                    "Volume",
                    slider(VOLUME, self.volume).on_change(|app: &mut Self, volume| {
                        app.volume = volume;
                        app.status = format!("volume: {volume}%");
                    }),
                ),
                field("Level", progress(Some(level))),
                field(
                    "Tab width",
                    stepper(TAB_WIDTH, self.tab_width).on_change(|app: &mut Self, width| {
                        app.tab_width = width;
                        app.status = format!("tab width: {width}");
                    }),
                ),
                field("Indexing", progress(None)),
                field(
                    "Notes",
                    text_area(&self.notes, self.notes_version)
                        .rows(NOTES_ROWS)
                        .on_change(|app: &mut Self, notes| {
                            app.status = format!("notes: {} lines", notes.split('\n').count());
                            app.notes = notes;
                            app.notes_version += 1;
                        }),
                ),
            ])
            .padding(MARGIN_COLS, MARGIN_ROWS)
            .scroll(),
            Element::from(status).padding(MARGIN_COLS, STATUS_MARGIN_ROWS),
        ])
    }

    /// Quits on Ctrl+Q.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        if key != Key::new(KeyCode::Char('q'), Mods::CTRL) {
            return false;
        }
        cx.exit();
        true
    }

    /// Follows the system's `appearance`: its light or dark theme in its
    /// accent color, keeping the chrome font chosen, which the theme radio
    /// group follows. The widgets follow its less motion on their own.
    fn follow_appearance(&mut self, appearance: Appearance) {
        self.theme = Theme {
            chrome_text: self.theme.chrome_text,
            ..appearance.theme()
        };
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `on` as the status line says it.
fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn main() -> Result<(), misty::Error> {
    misty::run_app(Demo::new(), WindowOptions::new(TITLE).with_size(SIZE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use misty::AccessAction;
    use misty::accesskit::Role;
    use misty::testing::TreeWindow;

    use super::*;

    /// The window's width in the tests, in physical pixels: the size the
    /// window opens at, at scale factor 1.
    const WIDTH: u32 = 720;

    /// The window's height in the tests.
    const HEIGHT: u32 = 560;

    /// What the hand-driven version of this example (an `Application`
    /// drawing every control itself, before the tree) described to screen
    /// readers at start, captured from it before it was rewritten.
    const HAND_DRIVEN: &str = "\
Window \"misty controls\"
  ScrollView
    CheckBox \"Wrap long lines\" checked focused
    Switch \"Autosave\" unchecked
    RadioGroup
      RadioButton \"Dark\" checked
      RadioButton \"Light\" unchecked
    ComboBox \"Chrome font\" value=\"Proportional\" collapsed
    Slider \"Volume\" value=\"40\" value=40
    SpinButton \"Tab width\" value=\"4\" value=4
      Button \"Decrease\"
      Button \"Increase\"
    MultilineTextInput value=\"A text area wraps long lines at its width, keeps the caret in view as it scrolls, and edits with the keys a text field takes.\\n\\nEnter adds a line; Tab moves on.\"
      TextRun value=\"A text area wraps long lines at its width, keeps the caret in view as it scrolls, and edits with the keys a text field takes.\\n\\nEnter adds a line; Tab moves on.\"
  ProgressIndicator \"Level\" value=40
  ProgressIndicator \"Indexing\"
";

    /// The window showing the demo at start.
    fn window() -> TreeWindow<Demo> {
        TreeWindow::new(Demo::new(), TITLE, WIDTH, HEIGHT)
    }

    /// The line of `window`'s description that has the focus.
    fn focused(window: &TreeWindow<Demo>) -> String {
        let text = window.ui_text();
        let line = text
            .lines()
            .find(|line| line.ends_with(" focused"))
            .unwrap_or_else(|| panic!("nothing focused in\n{text}"));
        line.trim().trim_end_matches(" focused").to_owned()
    }

    #[test]
    fn test_the_tree_describes_what_the_hand_driven_example_did() {
        // The radio group and the text area are now named by their form
        // labels, which the hand-driven version could not do.
        let expected: String = HAND_DRIVEN
            .lines()
            .map(|line| match line.trim() {
                "RadioGroup" => line.replace("RadioGroup", "RadioGroup \"Theme\""),
                trimmed if trimmed.starts_with("MultilineTextInput") => {
                    line.replace("MultilineTextInput", "MultilineTextInput \"Notes\"")
                }
                _ => line.to_owned(),
            })
            .map(|line| line + "\n")
            .collect();
        assert_eq!(window().ui_text(), expected);
    }

    #[test]
    fn test_the_form_draws_each_label_before_its_control_and_the_status_last() {
        let drawn = window().drawn_text();
        let labels = [
            "Editor",
            "Theme",
            "Chrome font",
            "Volume",
            "Level",
            "Tab width",
            "Indexing",
            "Notes",
        ];
        let places: Vec<usize> = labels
            .iter()
            .map(|label| drawn.iter().position(|text| text == label).expect(label))
            .collect();
        assert!(places.is_sorted(), "{drawn:?}");
        assert_eq!(drawn[places[0] + 1], "Wrap long lines");
        assert_eq!(drawn.last().map(String::as_str), Some("Last: nothing yet"));
    }

    #[test]
    fn test_tab_and_shift_tab_visit_the_controls_in_reading_order() {
        let mut window = window();
        let order = [
            "CheckBox \"Wrap long lines\" checked",
            "Switch \"Autosave\" unchecked",
            "RadioButton \"Dark\" checked",
            "ComboBox \"Chrome font\" value=\"Proportional\" collapsed",
            "Slider \"Volume\" value=\"40\" value=40",
            "SpinButton \"Tab width\" value=\"4\" value=4",
        ];
        for line in order {
            assert_eq!(focused(&window), line);
            window.press("<tab>");
        }
        assert!(focused(&window).starts_with("MultilineTextInput \"Notes\""));
        window.press("<tab>");
        assert_eq!(focused(&window), order[0], "Tab wraps to the first control");
        window.press("S-<tab>");
        assert!(focused(&window).starts_with("MultilineTextInput \"Notes\""));
    }

    #[test]
    fn test_typing_in_the_notes_stores_the_text_and_bumps_its_version() {
        let mut window = window();
        window.press("<tab> <tab> <tab> <tab> <tab> <tab>");
        window.type_text("!\nok");
        let demo = window.app();
        assert_eq!(demo.notes, format!("{NOTES}!\nok"));
        assert_eq!(demo.notes_version, 4, "one bump per edit");
        assert_eq!(demo.status, "notes: 4 lines");
        assert!(
            window
                .ui_text()
                .contains("Enter adds a line; Tab moves on.!\\nok\"")
        );
        assert!(
            window
                .drawn_text()
                .contains(&String::from("Last: notes: 4 lines"))
        );
    }

    #[test]
    fn test_space_and_clicks_toggle_the_checkbox_and_the_switch() {
        let mut window = window();
        window.press("<space>");
        assert!(!window.app().wrap);
        assert_eq!(window.app().status, "wrap: off");
        assert!(
            window
                .ui_text()
                .contains("CheckBox \"Wrap long lines\" unchecked focused")
        );
        window.click(Role::Switch, "Autosave");
        assert!(window.app().autosave);
        assert_eq!(
            focused(&window),
            "Switch \"Autosave\" checked",
            "a click focuses"
        );
    }

    #[test]
    fn test_the_radio_group_switches_the_theme_by_key_and_by_click() {
        let mut window = window();
        window.press("<tab> <tab> j");
        assert_eq!(window.app().theme.background, Theme::light().background);
        assert_eq!(window.app().status, "theme: light");
        assert_eq!(focused(&window), "RadioButton \"Light\" checked");
        window.click(Role::RadioButton, "Dark");
        assert_eq!(window.app().theme.background, Theme::dark().background);
    }

    #[test]
    fn test_sliding_the_volume_moves_the_level_bar() {
        let mut window = window();
        window.press("<tab> <tab> <tab> <tab> <right> <right>");
        assert_eq!(window.app().volume, 50.0);
        assert_eq!(window.app().status, "volume: 50%");
        assert!(
            window
                .ui_text()
                .contains("ProgressIndicator \"Level\" value=50\n")
        );
        window.press("<left>");
        assert_eq!(window.app().volume, 45.0);
    }

    #[test]
    fn test_stepping_the_tab_width_by_key_and_by_its_buttons() {
        let mut window = window();
        window.press("<tab> <tab> <tab> <tab> <tab> <up>");
        assert_eq!(window.app().tab_width, 5.0);
        window.click(Role::Button, "Decrease");
        window.click(Role::Button, "Decrease");
        assert_eq!(window.app().tab_width, 3.0);
        assert_eq!(window.app().status, "tab width: 3");
    }

    #[test]
    fn test_screen_reader_actions_reach_the_controls() {
        let mut window = window();
        let wrap = window.find(Role::CheckBox, "Wrap long lines");
        window.act(AccessAction::Click(wrap));
        assert!(!window.app().wrap);
        let volume = window.find(Role::Slider, "Volume");
        window.act(AccessAction::Increment(volume));
        assert_eq!(window.app().volume, 45.0);
        let increase = window.find(Role::Button, "Increase");
        window.act(AccessAction::Click(increase));
        assert_eq!(window.app().tab_width, 5.0);
        let light = window.find(Role::RadioButton, "Light");
        window.act(AccessAction::Click(light));
        assert_eq!(window.app().status, "theme: light");
        let notes = window.find(Role::MultilineTextInput, "Notes");
        window.act(AccessAction::Focus(notes));
        assert!(focused(&window).starts_with("MultilineTextInput \"Notes\""));
    }

    #[test]
    fn test_the_select_opens_over_the_form_chooses_and_gives_the_focus_back() {
        let mut window = window();
        window.press("<tab> <tab> <tab> RET");
        let text = window.ui_text();
        assert!(
            text.contains("ComboBox \"Chrome font\" value=\"Proportional\" expanded"),
            "{text}"
        );
        assert!(text.contains("ListBoxOption \"Monospace cells\""), "{text}");
        window.press("<tab> RET <down> RET");
        assert_eq!(window.app().theme.chrome_text, ChromeText::Cells);
        assert_eq!(window.app().status, "chrome font: Cells");
        assert_eq!(
            focused(&window),
            "ComboBox \"Chrome font\" value=\"Monospace cells\" collapsed",
            "the focus is back on the box"
        );
    }

    #[test]
    fn test_esc_and_a_click_outside_close_the_select_leaving_its_value() {
        let mut window = window();
        window.press("<tab> <tab> <tab> RET <down> <esc>");
        assert_eq!(window.app().theme.chrome_text, ChromeText::Proportional);
        assert!(focused(&window).ends_with("collapsed"));
        window.press("<space>");
        assert!(window.ui_text().contains("expanded"));
        window.click(Role::CheckBox, "Wrap long lines");
        assert!(window.app().wrap, "the click outside only closed the list");
        assert!(
            focused(&window).starts_with("ComboBox"),
            "{}",
            window.ui_text()
        );
        assert!(window.ui_text().contains("collapsed"));
    }

    #[test]
    fn test_a_screen_reader_opens_the_select_and_picks_an_option() {
        let mut window = window();
        let font = window.find(Role::ComboBox, "Chrome font");
        window.act(AccessAction::Click(font));
        let option = window.find(Role::ListBoxOption, "Monospace cells");
        window.act(AccessAction::Click(option));
        assert_eq!(window.app().theme.chrome_text, ChromeText::Cells);
        assert!(window.ui_text().contains("collapsed"));
    }

    #[test]
    fn test_ctrl_q_quits_and_other_unused_keys_do_not() {
        let mut window = window();
        window.press("C-x");
        assert!(!window.is_closed());
        window.press("C-q");
        assert!(window.is_closed());
    }
}

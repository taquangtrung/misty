//! The form controls in one window: a checkbox and a switch, a radio
//! group that switches the theme, a select that sets the chrome font, a
//! slider driving a progress bar, a stepper, a progress bar of unknown
//! length, and a text area, in a form that scrolls when the window is too
//! short for it. Tab and Shift+Tab move between them, scrolling the one
//! focused into view, a click focuses one, and the line at the bottom says
//! what changed last. The window follows the system's appearance: light or
//! dark, its accent color, and less motion, at start and as they change.
//!
//! Each control takes the keys its module lists: Space toggles a checkbox,
//! the arrows (or `h`, `j`, `k`, `l`) move a radio group's choice and a
//! slider's or stepper's value, Enter or Space opens the select, and the
//! text area edits as a text field does, Enter adding a line. Ctrl+Q
//! quits.
//!
//! ```sh
//! cargo run -p misty --example controls
//! ```

use std::time::Instant;

use misty::{
    AccessAction, AppContext, Appearance, Application, Checkbox, CheckboxResponse, ChromeText,
    ColorScheme, Error, EventCx, FocusId, FocusTree, Frame, ImeFocus, Input, Key, KeyCode, Mods,
    NumberRange, Outcome, Point, ProgressBar, RadioGroup, RadioGroupResponse, Rect, Route,
    ScrollView, Select, SelectResponse, Size, Slider, SliderResponse, Span, Stepper,
    StepperResponse, StyledText, TextArea, TextAreaResponse, Theme, ThemeToken, Widget, WindowId,
    WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty controls";

/// The window's size at start, in logical pixels.
const SIZE: Size = Size::new(720, 560);

/// The blank cells around the content, in columns and rows.
const MARGIN: f32 = 2.0;

/// The cells the labels take, left of the controls.
const LABEL_COLS: f32 = 14.0;

/// The rows between two controls.
const ROW_GAP: f32 = 0.5;

/// The rows the text area takes.
const NOTES_ROWS: f32 = 6.0;

/// The rows the status line keeps at the bottom: its line and a blank row
/// above and below.
const STATUS_ROWS: f32 = 3.0;

/// The volume slider's range, in percent.
const VOLUME: NumberRange = NumberRange::new(0.0, 100.0, 5.0);

/// The volume at start, in percent.
const VOLUME_START: f64 = 40.0;

/// The index of the theme radio group's dark option.
const DARK_OPTION: usize = 0;

/// The index of the theme radio group's light option.
const LIGHT_OPTION: usize = 1;

/// The tab width stepper's range.
const TAB_WIDTH: NumberRange = NumberRange::new(1.0, 8.0, 1.0);

/// The tab width at start.
const TAB_WIDTH_START: f64 = 4.0;

/// The volume a full progress bar stands for, in percent.
const FULL_VOLUME: f64 = 100.0;

/// What the text area holds at start.
const NOTES: &str = "A text area wraps long lines at its width, keeps the caret in view as \
    it scrolls, and edits with the keys a text field takes.\n\nEnter adds a line; Tab moves on.";

/// The focus ids of the controls, in tab order.
const WRAP: FocusId = FocusId::new("wrap");
const AUTOSAVE: FocusId = FocusId::new("autosave");
const THEME: FocusId = FocusId::new("theme");
const FONT: FocusId = FocusId::new("font");
const FONT_LIST: FocusId = FocusId::new("font.list");
const VOLUME_ID: FocusId = FocusId::new("volume");
const TABS: FocusId = FocusId::new("tabs");
const NOTES_ID: FocusId = FocusId::new("notes");

/// The id of the scrolled form holding the controls.
const FORM: FocusId = FocusId::new("form");

/// The accessibility ids of the progress bars.
const LEVEL: FocusId = FocusId::new("level");
const INDEXING: FocusId = FocusId::new("indexing");

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the controls, the focus, and what changed last.
struct Demo {
    /// The controls.
    controls: Controls,
    /// Which control has the keyboard.
    focus: FocusTree,
    /// Whether the input method is on: at the text area's caret.
    ime: ImeFocus,
    /// How tall the form was drawn last, for its scroll view.
    form_height: f32,
    /// The control last scrolled into view after the focus moved to it.
    revealed: Option<FocusId>,
    /// What changed last.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// Every control in the window.
struct Controls {
    /// A switch.
    autosave: Checkbox,
    /// The chrome font.
    font: Select<ChromeText>,
    /// The scrolled form the other controls sit in.
    form: ScrollView,
    /// A progress bar of unknown length.
    indexing: ProgressBar,
    /// A progress bar following the volume.
    level: ProgressBar,
    /// A text area.
    notes: TextArea,
    /// The tab width.
    tabs: Stepper,
    /// Dark or light.
    theme: RadioGroup<bool>,
    /// The volume.
    volume: Slider,
    /// A checkbox.
    wrap: Checkbox,
}

/// What a control answered that the app acts on.
enum Change {
    /// The chrome font was chosen.
    Font(ChromeText),
    /// The select's list opened.
    FontOpened,
    /// The select's list closed.
    FontClosed,
    /// The text area's text changed.
    Notes,
    /// The tab width was stepped.
    Tabs(f64),
    /// The theme was chosen: dark when `true`.
    Theme(bool),
    /// A checkbox or switch was turned on or off.
    Toggled(&'static str, bool),
    /// The volume moved.
    Volume(f64),
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo at start, the checkbox focused.
    fn new() -> Self {
        let mut focus = FocusTree::new();
        focus.focus(WRAP);
        let volume = Slider::new(VOLUME, VOLUME_START).with_label("Volume");
        let mut theme = RadioGroup::new()
            .with_option(true, "Dark")
            .with_option(false, "Light");
        theme.select(DARK_OPTION);
        let mut level = ProgressBar::new().with_label("Level");
        level.set_fraction(Some((VOLUME_START / FULL_VOLUME) as f32));
        Self {
            controls: Controls {
                autosave: Checkbox::new("Autosave").switch(),
                font: Select::new()
                    .with_label("Chrome font")
                    .with_option(ChromeText::Cells, "Monospace cells")
                    .with_option(ChromeText::Proportional, "Proportional"),
                form: ScrollView::new(),
                indexing: ProgressBar::new().with_label("Indexing"),
                level,
                notes: TextArea::with_text(NOTES).with_frame(),
                tabs: Stepper::new(TAB_WIDTH, TAB_WIDTH_START).with_label("Tab width"),
                theme,
                volume,
                wrap: Checkbox::new("Wrap long lines").with_checked(true),
            },
            focus,
            ime: ImeFocus::new(),
            form_height: 0.0,
            revealed: None,
            status: String::from("nothing yet"),
            theme: Theme::dark(),
            window: None,
        }
    }

    /// Follows the system's `appearance`: its light or dark theme in its
    /// accent color (keeping the chrome font chosen) and the theme radio
    /// group in step. The widgets follow its less motion on their own.
    fn follow_appearance(&mut self, appearance: Appearance) {
        self.theme = Theme {
            chrome_text: self.theme.chrome_text,
            ..appearance.theme()
        };
        let option = match appearance.color_scheme {
            Some(ColorScheme::Light) => LIGHT_OPTION,
            Some(ColorScheme::Dark) | None => DARK_OPTION,
        };
        self.controls.theme.select(option);
    }

    /// Acts on `change`, keeping the select's focus trap open while its
    /// list is.
    fn settle(&mut self, change: Change) {
        self.status = match change {
            Change::Font(chrome_text) => {
                self.focus.close_trap(FONT_LIST);
                self.theme.chrome_text = chrome_text;
                format!("chrome font: {chrome_text:?}")
            }
            Change::FontOpened => {
                self.focus.open_trap(FONT_LIST);
                return;
            }
            Change::FontClosed => {
                self.focus.close_trap(FONT_LIST);
                return;
            }
            Change::Notes => format!("notes: {} lines", self.controls.notes.line_count()),
            Change::Tabs(width) => format!("tab width: {width}"),
            Change::Theme(dark) => {
                let chrome_text = self.theme.chrome_text;
                self.theme = if dark { Theme::dark() } else { Theme::light() };
                self.theme.chrome_text = chrome_text;
                format!("theme: {}", if dark { "dark" } else { "light" })
            }
            Change::Toggled(name, on) => format!("{name}: {}", if on { "on" } else { "off" }),
            Change::Volume(volume) => {
                let fraction = (volume / FULL_VOLUME) as f32;
                self.controls.level.set_fraction(Some(fraction));
                format!("volume: {volume}%")
            }
        };
    }
}

impl Application for Demo {
    type Message = ();

    fn init(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
        self.follow_appearance(cx.appearance());
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        cx.event_cx(window).request_redraw();
        if let Input::AppearanceChanged(appearance) = input {
            self.follow_appearance(*appearance);
            self.status = String::from("followed the system's appearance");
            return;
        }
        let mut changes = Vec::new();
        let mut controls_cx = cx.event_cx(window);
        let controls = &mut self.controls;
        let route = self.focus.dispatch(input, |id, input| {
            let (used, change) = controls.handle(id, input, &mut controls_cx);
            changes.extend(change);
            used
        });
        for change in changes {
            self.settle(change);
        }
        let quits = input.key() == Some(Key::new(KeyCode::Char('q'), Mods::CTRL));
        if route == Route::Unhandled && (quits || *input == Input::CloseRequested) {
            cx.exit();
        }
    }

    fn access_action(
        &mut self,
        cx: &mut AppContext<'_, ()>,
        window: WindowId,
        action: AccessAction,
    ) {
        cx.event_cx(window).request_redraw();
        let change = match action {
            AccessAction::Focus(id) => {
                self.focus.focus(id);
                None
            }
            _ => self.controls.access_action(action),
        };
        if let Some(change) = change {
            self.settle(change);
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let caret = match self.focus.is_focused(NOTES_ID) {
            true => self.controls.notes.cursor_area(),
            false => None,
        };
        let mut window_cx = cx.event_cx(window);
        self.ime.follow(&mut window_cx, caret);
        let sweep = self.controls.indexing.about_to_wait(&mut window_cx);
        let form = self.controls.form.about_to_wait(&mut window_cx);
        sweep.into_iter().chain(form).min()
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        let theme = &self.theme;
        frame.clear(theme.background.into());
        self.focus.begin_frame();
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let area = frame.bounds();
        let left = area.x + MARGIN * metrics.width;
        // The status line keeps the bottom rows; the form scrolls above it.
        let status_y = (area.y + area.height - STATUS_ROWS * metrics.height).max(area.y);
        let form = Rect::new(area.x, area.y, area.width, status_y - area.y);
        let c = &mut self.controls;
        let focus = &mut self.focus;
        c.form.set_content_size(0.0, self.form_height);
        // Keys a control leaves (PageDown, `j`) bubble up and scroll the form.
        focus.sync_within(FORM, &mut c.form);
        let viewport = c.form.draw(frame, form, theme);
        c.form.access(frame, FORM);
        // The controls register inside the form, so they show only within it.
        focus.enter(FORM, viewport);
        let offset = c.form.offset();
        let control_x = left + LABEL_COLS * metrics.width;
        let width = (viewport.x + viewport.width - MARGIN * metrics.width - control_x).max(0.0);
        let gap = (ROW_GAP * metrics.height).round();
        let origin = viewport.y - offset.y;
        let top = origin + MARGIN * metrics.height;
        let mut y = top;
        let mut focused = None;
        frame.clipped(viewport, |frame| {
            // Each row: its label, then the control's rect at the label's right.
            let mut row = |frame: &mut Frame, label: &str, height: f32| {
                let label_y = (y + (height - metrics.height) / 2.0).round();
                let text = StyledText::plain(label, ThemeToken::Muted.into());
                let clip = Rect::new(left, label_y, control_x - left, metrics.height);
                frame.draw_text(&text, Point::new(left, label_y), clip, theme);
                let rect = Rect::new(control_x, y, width, height);
                y += height + gap;
                rect
            };
            let mut place = |focus: &mut FocusTree, id: FocusId, rect: Rect| {
                focus.register(id, rect);
                if focus.is_focused(id) {
                    focused = Some(rect);
                }
            };

            focus.sync(WRAP, &mut c.wrap);
            let rect = row(frame, "Editor", c.wrap.height(metrics, scale_factor));
            let rect = c.wrap.draw(frame, rect, theme);
            c.wrap.access(frame, WRAP);
            place(focus, WRAP, rect);

            focus.sync(AUTOSAVE, &mut c.autosave);
            let rect = row(frame, "", c.autosave.height(metrics, scale_factor));
            let rect = c.autosave.draw(frame, rect, theme);
            c.autosave.access(frame, AUTOSAVE);
            place(focus, AUTOSAVE, rect);

            focus.sync(THEME, &mut c.theme);
            let rect = row(frame, "Theme", c.theme.height(metrics, scale_factor));
            let rect = c.theme.draw(frame, rect, theme);
            c.theme.access(frame, THEME);
            place(focus, THEME, rect);

            // The select answers to two ids, its own and its open list's,
            // so it is told by hand; a widget with one id is synced.
            c.font
                .set_focused(focus.is_focused(FONT) || focus.is_focused(FONT_LIST));
            let rect = row(frame, "Chrome font", c.font.height(metrics, scale_factor));
            place(focus, FONT, rect);
            // The open list goes to the overlay, unclipped, above every row.
            if let Some(list) = c.font.draw(frame, rect, theme) {
                focus.register(FONT_LIST, list);
            }
            c.font.access(frame, FONT);

            focus.sync(VOLUME_ID, &mut c.volume);
            let rect = row(frame, "Volume", c.volume.height(metrics, scale_factor));
            c.volume.draw(frame, rect, theme);
            c.volume.access(frame, VOLUME_ID);
            place(focus, VOLUME_ID, rect);

            let rect = row(frame, "Level", metrics.height);
            c.level.draw(frame, rect, theme);
            c.level.access(frame, LEVEL);

            focus.sync(TABS, &mut c.tabs);
            let rect = row(frame, "Tab width", c.tabs.height(metrics, scale_factor));
            c.tabs.draw(frame, rect, theme);
            c.tabs.access(frame, TABS);
            place(focus, TABS, rect);

            let rect = row(frame, "Indexing", metrics.height);
            c.indexing.draw(frame, rect, theme);
            c.indexing.access(frame, INDEXING);

            focus.sync(NOTES_ID, &mut c.notes);
            let rect = row(frame, "Notes", (NOTES_ROWS * metrics.height).round());
            c.notes.draw(frame, rect, theme);
            c.notes.access(frame, NOTES_ID);
            place(focus, NOTES_ID, rect);
        });
        focus.leave();
        self.form_height = y - origin + MARGIN * metrics.height;
        // Scroll a control the keyboard just focused into view, once, so the
        // wheel can still scroll away from it.
        if focus.focused() != self.revealed {
            self.revealed = focus.focused();
            if let Some(rect) = focused {
                c.form
                    .reveal(Rect::new(rect.x, rect.y - origin, rect.width, rect.height));
            }
        }

        let status = StyledText::new()
            .with(Span::new("Last: ", ThemeToken::Muted.into()))
            .with(Span::new(self.status.as_str(), ThemeToken::Accent.into()))
            .with_ellipsis();
        let line_y = status_y + (STATUS_ROWS - 1.0) / 2.0 * metrics.height;
        let clip = Rect::new(
            left,
            line_y,
            (area.x + area.width - left).max(0.0),
            metrics.height,
        );
        frame.draw_text(&status, Point::new(left, line_y), clip, theme);
        focus.end_frame();
        focus.draw(frame, theme);
    }
}

// ========================================================================
// Controls Implementations
// ========================================================================

impl Controls {
    /// Hands `input` to the control `id`, through `cx`: whether the
    /// control used it, and what changed.
    fn handle(
        &mut self,
        id: FocusId,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> (bool, Option<Change>) {
        match id {
            WRAP => toggled(self.wrap.handle(input, cx), "wrap"),
            AUTOSAVE => toggled(self.autosave.handle(input, cx), "autosave"),
            THEME => match self.theme.handle(input, cx) {
                RadioGroupResponse::Changed(dark) => (true, Some(Change::Theme(dark))),
                response => (!response.is_ignored(), None),
            },
            FONT | FONT_LIST => font(self.font.handle(input, cx)),
            VOLUME_ID => match self.volume.handle(input, cx) {
                SliderResponse::Changed(volume) => (true, Some(Change::Volume(volume))),
                response => (!response.is_ignored(), None),
            },
            TABS => match self.tabs.handle(input, cx) {
                StepperResponse::Changed(width) => (true, Some(Change::Tabs(width))),
                response => (!response.is_ignored(), None),
            },
            FORM => (!self.form.handle(input, cx).is_ignored(), None),
            NOTES_ID => match self.notes.handle(input, cx) {
                TextAreaResponse::Changed => (true, Some(Change::Notes)),
                response => (!response.is_ignored(), None),
            },
            _ => (false, None),
        }
    }

    /// Does what a screen reader's `action` asks of the control it names:
    /// each control ignores actions on nodes not its own.
    fn access_action(&mut self, action: AccessAction) -> Option<Change> {
        if let CheckboxResponse::Changed(on) = self.wrap.access_action(WRAP, action) {
            return Some(Change::Toggled("wrap", on));
        }
        if let CheckboxResponse::Changed(on) = self.autosave.access_action(AUTOSAVE, action) {
            return Some(Change::Toggled("autosave", on));
        }
        if let RadioGroupResponse::Changed(dark) = self.theme.access_action(THEME, action) {
            return Some(Change::Theme(dark));
        }
        if let (_, Some(change)) = font(self.font.access_action(FONT, action)) {
            return Some(change);
        }
        if let SliderResponse::Changed(volume) = self.volume.access_action(VOLUME_ID, action) {
            return Some(Change::Volume(volume));
        }
        match self.tabs.access_action(TABS, action) {
            StepperResponse::Changed(width) => Some(Change::Tabs(width)),
            StepperResponse::Handled | StepperResponse::Ignored => None,
            _ => None,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What a checkbox named `name` answered, as [`Controls::handle`] returns
/// it.
fn toggled(response: CheckboxResponse, name: &'static str) -> (bool, Option<Change>) {
    match response {
        CheckboxResponse::Changed(on) => (true, Some(Change::Toggled(name, on))),
        response => (!response.is_ignored(), None),
    }
}

/// What the font select answered, as [`Controls::handle`] returns it.
fn font(response: SelectResponse<ChromeText>) -> (bool, Option<Change>) {
    match response {
        SelectResponse::Changed(chrome_text) => (true, Some(Change::Font(chrome_text))),
        SelectResponse::Closed => (true, Some(Change::FontClosed)),
        SelectResponse::Opened => (true, Some(Change::FontOpened)),
        response => (!response.is_ignored(), None),
    }
}

fn main() -> Result<(), Error> {
    misty::run(Demo::new(), WindowOptions::new(TITLE).with_size(SIZE))
}

//! Text fields as a file manager uses them: a rename box opened with the
//! file's name selected up to its extension, an address bar holding a
//! path longer than the field, and a filter box showing a placeholder
//! while empty. Tab and Shift+Tab move between them, a click focuses one.
//!
//! In a field: typing replaces the selection, Shift with the arrows (and
//! Ctrl for words) selects, a drag selects, a double click selects a word
//! and a triple click the whole text; Ctrl+C, Ctrl+X, and Ctrl+V copy,
//! cut, and paste through the system clipboard (Cmd on macOS), Ctrl+Z
//! undoes and Ctrl+Shift+Z redoes, and the Emacs keys (`C-a`, `C-e`,
//! `C-k`, `C-w`, `C-y`, `M-b`, `M-f`, ...) edit. On X11 and Wayland a
//! selection is also the primary selection, which a middle click pastes
//! into any field where it lands. An input method composes in place. Enter submits the focused field (the line at the bottom says
//! what), Esc puts the rename box's name back, and Ctrl+Q quits.
//!
//! ```sh
//! cargo run -p misty --example fields
//! ```

use std::time::Instant;

use misty::accesskit::{Node, Role};
use misty::widgets::TextField;
use misty::{
    AccessAction, AppContext, Application, Control, Error, FocusId, FocusTree, Frame, ImeFocus,
    Input, Key, KeyCode, Mods, Outcome, Point, Rect, Route, Size, Span, StyledText, Theme,
    ThemeToken, Widget, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty fields";

/// The window's size at start, in logical pixels.
const SIZE: Size = Size::new(640, 320);

/// The blank cells around the content, in columns and rows.
const MARGIN: f32 = 2.0;

/// The cells the labels take, left of the fields.
const LABEL_COLS: f32 = 10.0;

/// The rows between two fields.
const FIELD_GAP: f32 = 1.0;

/// The file being renamed.
const FILE_NAME: &str = "quarterly report.final.pdf";

/// The path in the address bar, longer than the field.
const PATH: &str = "/home/user/Documents/projects/2026/clients/northwind/invoices/archive/q3";

/// What the empty filter box says.
const FILTER_PLACEHOLDER: &str = "Filter files by name";

/// The focus and accessibility ids of the fields, in tab order, with
/// their labels.
const FIELDS: [(FocusId, &str); 3] = [
    (FocusId::new("rename"), "Rename"),
    (FocusId::new("address"), "Address"),
    (FocusId::new("filter"), "Filter"),
];

/// The accessibility id of the line saying what was submitted.
const STATUS: FocusId = FocusId::new("status");

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: three labeled fields and a status line.
struct Demo {
    /// The fields, in the order of [`FIELDS`].
    fields: [TextField; 3],
    /// Which field has the keyboard.
    focus: FocusTree,
    /// Whether the input method is on: always, at the focused field.
    ime: ImeFocus,
    /// What the last Enter or Esc did.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo at start: the rename box focused with the name selected.
    fn new() -> Self {
        let mut focus = FocusTree::new();
        focus.focus(FIELDS[0].0);
        Self {
            fields: [
                rename_box(),
                TextField::with_text(PATH).with_frame(),
                TextField::new()
                    .with_placeholder(FILTER_PLACEHOLDER)
                    .with_frame(),
            ],
            focus,
            ime: ImeFocus::new(),
            status: String::from("Enter submits a field, Esc restores the name."),
            theme: Theme::dark(),
            window: None,
        }
    }

    /// The index of the field with focus id `id`.
    fn index_of(id: FocusId) -> Option<usize> {
        FIELDS.iter().position(|(field, _)| *field == id)
    }

    /// Acts on a key the focused field at `index` left: Enter submits it,
    /// Esc restores the rename box. Whether it was used.
    fn settle_key(&mut self, index: usize, key: Key) -> bool {
        if key.mods != Mods::NONE {
            return false;
        }
        let (_, label) = FIELDS[index];
        match key.code {
            KeyCode::Enter => {
                self.status = format!("{label}: {}", self.fields[index].text());
                true
            }
            KeyCode::Esc if index == 0 => {
                self.fields[0] = rename_box();
                self.status = String::from("Rename: restored");
                true
            }
            _ => false,
        }
    }
}

impl Application for Demo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        cx.event_cx(window).request_redraw();
        let mut left = None;
        let mut fields_cx = cx.event_cx(window);
        let fields = &mut self.fields;
        let route = self.focus.dispatch(input, |id, input| {
            let Some(index) = Self::index_of(id) else {
                return false;
            };
            let used = !fields[index].handle(input, &mut fields_cx).is_ignored();
            if !used && input.key().is_some() {
                left = Some(index);
            }
            used
        });
        let settled = match (left, input.key()) {
            (Some(index), Some(key)) => self.settle_key(index, key),
            (None, _) | (_, None) => false,
        };
        let quits = input.key() == Some(Key::new(KeyCode::Char('q'), Mods::CTRL));
        if route == Route::Unhandled && !settled && (quits || *input == Input::CloseRequested) {
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
        if let AccessAction::Focus(id) | AccessAction::Click(id) = action {
            self.focus.focus(id);
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let focused = self.focus.focused().and_then(Self::index_of);
        let caret = focused.and_then(|index| self.fields[index].cursor_area());
        self.ime.follow(&mut cx.event_cx(window), caret);
        None
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        self.focus.begin_frame();
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let area = frame.bounds();
        let left = area.x + MARGIN * metrics.width;
        let field_x = left + LABEL_COLS * metrics.width;
        let field_width = (area.x + area.width - MARGIN * metrics.width - field_x).max(0.0);
        let mut y = area.y + MARGIN * metrics.height;
        for (index, (id, label)) in FIELDS.into_iter().enumerate() {
            let field = &mut self.fields[index];
            let height = field.measure(field_width, metrics, scale_factor);
            let rect = Rect::new(field_x, y, field_width, height);
            let label_y = (y + (height - metrics.height) / 2.0).round();
            let text = StyledText::plain(label, ThemeToken::Muted.into());
            let label_clip = Rect::new(left, label_y, field_x - left, metrics.height);
            frame.draw_text(&text, Point::new(left, label_y), label_clip, &self.theme);
            self.focus.sync(id, field);
            field.draw(frame, rect, &self.theme);
            field.access(frame, id);
            self.focus.register(id, rect);
            y += height + FIELD_GAP * metrics.height;
        }
        let status = StyledText::new()
            .with(Span::new("Last: ", ThemeToken::Muted.into()))
            .with(Span::new(self.status.as_str(), ThemeToken::Accent.into()))
            .with_ellipsis();
        let clip = Rect::new(
            left,
            y,
            (area.width - 2.0 * (left - area.x)).max(0.0),
            metrics.height,
        );
        frame.draw_text(&status, Point::new(left, y), clip, &self.theme);
        let mut line = Node::new(Role::Label);
        line.set_value(self.status.as_str());
        frame.access(STATUS, clip, line);
        self.focus.end_frame();
        self.focus.draw(frame, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The rename box as it opens: the file's name with everything before the
/// last dot selected, so typing replaces the name and keeps the extension.
fn rename_box() -> TextField {
    let mut field = TextField::with_text(FILE_NAME).with_frame();
    let stem = FILE_NAME.rfind('.').unwrap_or(FILE_NAME.len());
    field.select(0..stem);
    field
}

fn main() -> Result<(), Error> {
    misty::run(Demo::new(), WindowOptions::new(TITLE).with_size(SIZE))
}

//! Clickable controls: a toolbar of icon buttons bound to commands, with a
//! view-mode toggle group and a match-case toggle; a breadcrumb of a path;
//! and an "Options" button opening a menu with checked items, radio items,
//! and submenus. Narrow the window: the toolbar's last buttons move into
//! its "more" menu and the breadcrumb's middle crumbs into an ellipsis.
//! Rest the pointer on a toolbar button for its tooltip with its keys.
//!
//! Tab and Shift+Tab move between the three. In the toolbar and the
//! breadcrumb, Left and Right (or `h` and `l`) move between buttons and
//! Enter or Space activates one; Enter or Space opens the menu from the
//! "Options" button, where Up and Down move, Right or `l` opens a submenu
//! and Left or `h` closes it. Ctrl+L edits the breadcrumb as an address
//! (Enter goes there, Esc gives up). The line at the bottom says what
//! happened last. Ctrl+Q quits.
//!
//! ```sh
//! cargo run -p misty --example toolbar
//! ```

use std::time::Instant;

use misty::{
    AccessAction, AppContext, Application, Breadcrumb, BreadcrumbResponse, Button, ButtonResponse,
    Command, CommandRegistry, Crumb, Error, EventCx, FocusId, FocusTree, Frame, IconSet, ImeFocus,
    Input, Key, KeyCode, Menu, MenuEntry, MenuItem, MenuResponse, Mods, Outcome, Point, PopupSide,
    Rect, Route, Size, Span, StyledText, Theme, ThemeToken, ToolButton, Toolbar, ToolbarItem,
    ToolbarResponse, Tooltip, Widget, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty toolbar";

/// The window's size at start, in logical pixels.
const SIZE: Size = Size::new(720, 360);

/// The blank cells around the content, in columns and rows.
const MARGIN: f32 = 2.0;

/// The rows between two controls.
const ROW_GAP: f32 = 1.0;

/// The rows the status line keeps at the bottom: its line and a blank row
/// above and below.
const STATUS_ROWS: f32 = 3.0;

/// The path the breadcrumb shows at start.
const START_PATH: &str = "/home/user/Documents/projects/misty/examples";

/// What separates a path's segments.
const PATH_SEPARATOR: char = '/';

/// The demo's icons: one-color SVGs by name.
const ICONS: &[(&str, &str)] = &[
    (
        "open",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
        <path d="M2 5h8l2 2h10v12H2z" fill="#000"/></svg>"##,
    ),
    (
        "save",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
        <path d="M4 3h13l4 4v14H4zM8 3v6h8V3M7 14h10v7H7z" fill="#000" fill-rule="evenodd"/></svg>"##,
    ),
    (
        "grid",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
        <path d="M3 3h8v8H3zM13 3h8v8h-8zM3 13h8v8H3zM13 13h8v8h-8z" fill="#000"/></svg>"##,
    ),
    (
        "list",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
        <path d="M3 4h18v3H3zM3 10.5h18v3H3zM3 17h18v3H3z" fill="#000"/></svg>"##,
    ),
    (
        "home",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
        <path d="M12 3l9 8h-3v10h-5v-6h-2v6H6V11H3z" fill="#000"/></svg>"##,
    ),
];

/// The commands the toolbar's buttons are bound to.
const COMMANDS: &[Command] = &[
    Command::new("file.open", "Open", "File").with_keys(&["C-o"]),
    Command::new("file.save", "Save", "File").with_keys(&["C-s"]),
    Command::new("view.grid", "Grid View", "View").with_keys(&["C-1"]),
    Command::new("view.list", "List View", "View").with_keys(&["C-2"]),
    Command::new("find.case", "Match Case", "Find").with_keys(&["M-c"]),
    Command::new("file.delete", "Delete", "File").with_keys(&["<del>"]),
];

/// The focus ids of the controls, in tab order, and of their menus.
const TOOLBAR: FocusId = FocusId::new("toolbar");
const TOOLBAR_MENU: FocusId = FocusId::new("toolbar.menu");
const CRUMBS: FocusId = FocusId::new("crumbs");
const CRUMBS_MENU: FocusId = FocusId::new("crumbs.menu");
const OPTIONS: FocusId = FocusId::new("options");
const OPTIONS_MENU: FocusId = FocusId::new("options.menu");

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the controls, the focus, and what happened last.
struct Demo {
    /// The controls.
    controls: Controls,
    /// Which control has the keyboard.
    focus: FocusTree,
    /// The icons, for the breadcrumb's first crumb.
    icons: IconSet,
    /// Whether the input method is on: at the address field's caret.
    ime: ImeFocus,
    /// Where the options button was drawn last, for its menu to open
    /// under.
    options_rect: Rect,
    /// The path's segments.
    path: Vec<String>,
    /// Whether hidden files show, as the options menu sets it.
    show_hidden: bool,
    /// How files are sorted, as the options menu sets it.
    sort: Choice,
    /// What happened last.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The tooltip of the button under the pointer.
    tooltip: Tooltip,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// Every control in the window.
struct Controls {
    /// The path, as crumbs.
    breadcrumb: Breadcrumb,
    /// The options menu.
    menu: Menu<Choice>,
    /// The button opening the options menu.
    options: Button,
    /// The toolbar.
    toolbar: Toolbar<&'static str>,
}

/// An item of the options menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Choice {
    /// Export as HTML.
    ExportHtml,
    /// Export as PDF.
    ExportPdf,
    /// The export submenu.
    ExportMenu,
    /// Show or hide hidden files.
    Hidden,
    /// Refresh.
    Refresh,
    /// Sort by date.
    SortDate,
    /// The sort submenu.
    SortMenu,
    /// Sort by name.
    SortName,
    /// Sort by size.
    SortSize,
}

/// What a control answered that the app acts on.
enum Event {
    /// A breadcrumb crumb was activated.
    Crumb(usize),
    /// The breadcrumb's address was submitted.
    Address(String),
    /// A menu or a popup opened: trap the focus in it.
    Opened(FocusId),
    /// The options menu's item was chosen.
    Chosen(Choice),
    /// The options button was clicked.
    Options,
    /// Something happened worth a line.
    Said(String),
    /// A toolbar button was activated.
    Tool(&'static str),
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo at start, the toolbar focused.
    fn new(icons: IconSet) -> Self {
        let registry = CommandRegistry::new(COMMANDS).expect("the demo's commands parse");
        let icon = |name: &str| icons.get(name).expect("the demo has the icon").clone();
        let mut toolbar = Toolbar::new(vec![
            ToolButton::new("file.open")
                .with_icon(icon("open"))
                .with_command("file.open")
                .into(),
            ToolButton::new("file.save")
                .with_icon(icon("save"))
                .with_command("file.save")
                .into(),
            ToolbarItem::Separator,
            ToolbarItem::Group(vec![
                ToolButton::new("view.grid")
                    .with_icon(icon("grid"))
                    .with_command("view.grid")
                    .toggle(true),
                ToolButton::new("view.list")
                    .with_icon(icon("list"))
                    .with_command("view.list"),
            ]),
            ToolbarItem::Separator,
            ToolButton::new("find.case")
                .with_label("Aa")
                .with_command("find.case")
                .toggle(false)
                .into(),
            ToolbarItem::Spacer,
            ToolButton::new("file.delete")
                .with_command("file.delete")
                .into(),
        ]);
        toolbar.sync_commands(&registry);
        let mut focus = FocusTree::new();
        focus.focus(TOOLBAR);
        let mut demo = Self {
            controls: Controls {
                breadcrumb: Breadcrumb::new(Vec::new()),
                menu: Menu::new(Vec::new()),
                options: Button::new("Options").with_tooltip("View options"),
                toolbar,
            },
            focus,
            icons,
            ime: ImeFocus::new(),
            options_rect: Rect::default(),
            path: Vec::new(),
            show_hidden: false,
            sort: Choice::SortName,
            status: String::from("nothing yet"),
            theme: Theme::dark(),
            tooltip: Tooltip::new(),
            window: None,
        };
        demo.go_to(START_PATH);
        demo
    }

    /// Shows `path` in the breadcrumb, the first crumb with a home icon.
    fn go_to(&mut self, path: &str) {
        self.path = path
            .split(PATH_SEPARATOR)
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        let home = self.icons.get("home").cloned();
        let crumbs = self
            .path
            .iter()
            .enumerate()
            .map(|(index, segment)| match (index, &home) {
                (0, Some(icon)) => Crumb::new(segment.as_str()).with_icon(icon.clone()),
                _ => Crumb::new(segment.as_str()),
            })
            .collect();
        self.controls.breadcrumb.set_segments(crumbs);
        let address = self.address();
        self.controls.breadcrumb.set_address(&address);
    }

    /// The path as one string.
    fn address(&self) -> String {
        self.path
            .iter()
            .map(|segment| format!("{PATH_SEPARATOR}{segment}"))
            .collect()
    }

    /// The options menu's entries, as the demo's state is now.
    fn menu_entries(&self) -> Vec<MenuEntry<Choice>> {
        let sort = |choice: Choice, label: &str| -> MenuEntry<Choice> {
            MenuItem::new(choice, label)
                .with_radio(self.sort == choice)
                .into()
        };
        vec![
            MenuItem::new(Choice::SortMenu, "Sort By")
                .with_submenu(vec![
                    sort(Choice::SortName, "Name"),
                    sort(Choice::SortDate, "Date Modified"),
                    sort(Choice::SortSize, "Size"),
                ])
                .into(),
            MenuItem::new(Choice::Hidden, "Show Hidden Files")
                .with_checked(self.show_hidden)
                .into(),
            MenuEntry::Separator,
            MenuItem::new(Choice::ExportMenu, "Export")
                .with_submenu(vec![
                    MenuItem::new(Choice::ExportPdf, "PDF").into(),
                    MenuItem::new(Choice::ExportHtml, "HTML").into(),
                ])
                .into(),
            MenuItem::new(Choice::Refresh, "Refresh")
                .with_accelerator("F5")
                .into(),
        ]
    }

    /// Acts on `event`.
    fn settle(&mut self, event: Event) {
        self.status = match event {
            Event::Address(address) => {
                self.go_to(&address);
                format!("went to {}", self.address())
            }
            Event::Chosen(choice) => {
                match choice {
                    Choice::Hidden => self.show_hidden = !self.show_hidden,
                    Choice::SortDate | Choice::SortName | Choice::SortSize => self.sort = choice,
                    Choice::ExportHtml
                    | Choice::ExportMenu
                    | Choice::ExportPdf
                    | Choice::Refresh
                    | Choice::SortMenu => {}
                }
                format!("chose {choice:?}")
            }
            Event::Crumb(index) => {
                let path: String = self.path[..=index]
                    .iter()
                    .map(|segment| format!("{PATH_SEPARATOR}{segment}"))
                    .collect();
                self.go_to(&path);
                format!("went to {path}")
            }
            Event::Opened(id) => {
                self.focus.open_trap(id);
                return;
            }
            Event::Options => {
                self.controls.menu = Menu::new(self.menu_entries());
                self.controls.menu.open(self.options_rect, PopupSide::Below);
                self.focus.open_trap(OPTIONS_MENU);
                return;
            }
            Event::Said(text) => text,
            Event::Tool(id) => {
                let state = match self.controls.toolbar.is_active(id) {
                    true => " (on)",
                    false => "",
                };
                format!("{id}{state}")
            }
        };
    }
}

impl Application for Demo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        cx.event_cx(window).request_redraw();
        let key = input.key();
        if key == Some(Key::new(KeyCode::Char('l'), Mods::CTRL)) {
            self.focus.focus(CRUMBS);
            let address = self.address();
            let breadcrumb = &mut self.controls.breadcrumb;
            self.focus.sync(CRUMBS, breadcrumb);
            breadcrumb.set_address(&address);
            breadcrumb.set_editing(true);
            self.status = String::from("editing the address");
            return;
        }
        let mut events = Vec::new();
        let mut window_cx = cx.event_cx(window);
        let controls = &mut self.controls;
        let route = self.focus.dispatch(input, |id, input| {
            let (used, event) = controls.handle(id, input, &mut window_cx);
            events.extend(event);
            used
        });
        for event in events {
            self.settle(event);
        }
        // A menu that closed gives the focus back.
        let c = &self.controls;
        if !c.toolbar.is_menu_open() {
            self.focus.close_trap(TOOLBAR_MENU);
        }
        if !c.breadcrumb.is_menu_open() {
            self.focus.close_trap(CRUMBS_MENU);
        }
        if !c.menu.is_open() {
            self.focus.close_trap(OPTIONS_MENU);
        }
        let hover = c.toolbar.tooltip_hover().or_else(|| {
            c.options
                .tooltip_hover()
                .map(|(rect, text)| (rect, text.to_owned()))
        });
        match hover {
            Some((rect, text)) => self.tooltip.hover(rect, text),
            None => {
                self.tooltip.leave();
            }
        }
        let quits = key == Some(Key::new(KeyCode::Char('q'), Mods::CTRL));
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
        if let AccessAction::Focus(id) = action {
            self.focus.focus(id);
            return;
        }
        let c = &mut self.controls;
        let event = match c.toolbar.access_action(TOOLBAR, action) {
            ToolbarResponse::Activated(id) => Some(Event::Tool(id)),
            ToolbarResponse::Opened => Some(Event::Opened(TOOLBAR_MENU)),
            ToolbarResponse::Closed | ToolbarResponse::Handled | ToolbarResponse::Ignored => None,
            _ => None,
        }
        .or_else(|| crumb(c.breadcrumb.access_action(CRUMBS, action)).1)
        .or_else(|| match c.options.access_action(OPTIONS, action) {
            ButtonResponse::Clicked => Some(Event::Options),
            ButtonResponse::Handled | ButtonResponse::Ignored | ButtonResponse::Toggled(_) => None,
            _ => None,
        })
        .or_else(|| match c.menu.access_action(OPTIONS_MENU, action) {
            MenuResponse::Activated(choice) => Some(Event::Chosen(choice)),
            MenuResponse::Closed | MenuResponse::Handled | MenuResponse::Ignored => None,
            _ => None,
        });
        if let Some(event) = event {
            self.settle(event);
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let caret = match self.focus.is_focused(CRUMBS) {
            true => self.controls.breadcrumb.cursor_area(),
            false => None,
        };
        let mut window_cx = cx.event_cx(window);
        self.ime.follow(&mut window_cx, caret);
        let blink = self.controls.breadcrumb.about_to_wait(&mut window_cx);
        let tooltip = self.tooltip.about_to_wait(&mut window_cx);
        blink.into_iter().chain(tooltip).min()
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        let theme = &self.theme;
        frame.clear(theme.background.into());
        self.focus.begin_frame();
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let area = frame.bounds();
        let left = area.x + MARGIN * metrics.width;
        let width = (area.width - 2.0 * MARGIN * metrics.width).max(0.0);
        let gap = (ROW_GAP * metrics.height).round();
        let mut y = area.y + MARGIN * metrics.height;
        let focus = &mut self.focus;
        let c = &mut self.controls;

        // The bar answers to two ids, its own and its open menu's, so it is
        // told by hand; a widget with one id is synced.
        c.toolbar
            .set_focused(focus.is_focused(TOOLBAR) || focus.is_focused(TOOLBAR_MENU));
        let height = c.toolbar.height(metrics, scale_factor);
        let rect = Rect::new(left, y, width, height);
        focus.register(TOOLBAR, rect);
        let toolbar_menu = c.toolbar.draw(frame, rect, theme);
        c.toolbar.access(frame, TOOLBAR);
        y += height + gap;

        // Two ids as well: the trail and its open menu.
        c.breadcrumb
            .set_focused(focus.is_focused(CRUMBS) || focus.is_focused(CRUMBS_MENU));
        let height = c.breadcrumb.height(metrics, scale_factor);
        let rect = Rect::new(left, y, width, height);
        focus.register(CRUMBS, rect);
        let crumbs_menu = c.breadcrumb.draw(frame, rect, theme);
        c.breadcrumb.access(frame, CRUMBS);
        y += height + gap;

        focus.sync(OPTIONS, &mut c.options);
        let height = c.options.height(metrics, scale_factor);
        let rect = c
            .options
            .draw(frame, Rect::new(left, y, width, height), theme);
        self.options_rect = rect;
        focus.register(OPTIONS, rect);
        c.options.access(frame, OPTIONS);

        // The popups go last: above every control for the pointer.
        if let Some(rect) = toolbar_menu {
            focus.register(TOOLBAR_MENU, rect);
        }
        if let Some(rect) = crumbs_menu {
            focus.register(CRUMBS_MENU, rect);
        }
        if let Some(rect) = c.menu.draw(frame, area, theme) {
            focus.register(OPTIONS_MENU, rect);
        }
        c.menu.access(frame, OPTIONS_MENU);

        let status = StyledText::new()
            .with(Span::new("Last: ", ThemeToken::Muted.into()))
            .with(Span::new(self.status.as_str(), ThemeToken::Accent.into()))
            .with_ellipsis();
        let line_y = area.y + area.height - (STATUS_ROWS + 1.0) / 2.0 * metrics.height;
        let clip = Rect::new(left, line_y, width, metrics.height);
        frame.draw_text(&status, Point::new(left, line_y), clip, theme);
        self.tooltip.draw(frame, area, theme);
        focus.end_frame();
        focus.draw(frame, theme);
    }
}

// ========================================================================
// Controls Implementations
// ========================================================================

impl Controls {
    /// Hands `input` to the control `id`, through `cx`: whether the
    /// control used it, and what happened.
    fn handle(
        &mut self,
        id: FocusId,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> (bool, Option<Event>) {
        match id {
            TOOLBAR | TOOLBAR_MENU => tool(self.toolbar.handle(input, cx)),
            CRUMBS | CRUMBS_MENU => crumb(self.breadcrumb.handle(input, cx)),
            OPTIONS => match self.options.handle(input, cx) {
                ButtonResponse::Clicked => (true, Some(Event::Options)),
                response => (!response.is_ignored(), None),
            },
            OPTIONS_MENU => match self.menu.handle(input, cx) {
                MenuResponse::Activated(choice) => (true, Some(Event::Chosen(choice))),
                response => (!response.is_ignored(), None),
            },
            _ => (false, None),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the toolbar answered, as [`Controls::handle`] returns it.
fn tool(response: ToolbarResponse<&'static str>) -> (bool, Option<Event>) {
    match response {
        ToolbarResponse::Activated(id) => (true, Some(Event::Tool(id))),
        ToolbarResponse::Opened => (true, Some(Event::Opened(TOOLBAR_MENU))),
        response => (!response.is_ignored(), None),
    }
}

/// What the breadcrumb answered, as [`Controls::handle`] returns it.
fn crumb(response: BreadcrumbResponse) -> (bool, Option<Event>) {
    match response {
        BreadcrumbResponse::Activated(index) => (true, Some(Event::Crumb(index))),
        BreadcrumbResponse::Opened => (true, Some(Event::Opened(CRUMBS_MENU))),
        BreadcrumbResponse::Submitted(address) => (true, Some(Event::Address(address))),
        BreadcrumbResponse::Cancelled => (true, Some(Event::Said(String::from("gave up editing")))),
        response => (!response.is_ignored(), None),
    }
}

fn main() -> Result<(), Error> {
    let icons = ICONS
        .iter()
        .try_fold(IconSet::new(), |set, (name, svg)| {
            set.with(*name, svg.as_bytes())
        })
        .expect("the demo's icons parse");
    misty::run(Demo::new(icons), WindowOptions::new(TITLE).with_size(SIZE))
}

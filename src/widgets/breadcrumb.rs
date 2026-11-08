//! [`Breadcrumb`]: where the user is, as a row of crumbs (a location's
//! segments, each a label with an optional icon) between separators, each
//! crumb a flat button that goes there. Segments are the app's strings:
//! splitting a path or a URL into them, and joining them back, is the
//! app's.
//!
//! When the row is narrower than its crumbs, the ones in the middle
//! collapse into an ellipsis, those nearest the start first, always
//! keeping the first and the last crumb; the ellipsis opens a [`Menu`] of
//! the crumbs it hides.
//!
//! [`Breadcrumb::set_editing`] swaps the crumbs for a [`TextField`] holding
//! the address the app gave with [`Breadcrumb::set_address`], all
//! selected, as an address bar: Enter hands back the text typed and Esc
//! gives up, both returning to the crumbs.
//!
//! Keys, while the app says the breadcrumb is focused and it shows its
//! crumbs: Left and Right (or `h` and `l`) move between the crumbs and the
//! ellipsis, wrapping, Home and End go to the first and the last, and
//! Enter or Space activates the one with the focus. A left press on a
//! crumb and its release over it activate it; a press on the ellipsis
//! opens its menu, which then takes every input as an open [`Menu`] does.
//! While editing, the field takes the keys and the pointer, as a
//! [`TextField`] does; call the breadcrumb's [`Widget::about_to_wait`] so
//! the field's caret blinks.

use std::time::Instant;

use accesskit::{Action, HasPopup, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::strings::Strings;
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::image::Svg;
use super::button::{ButtonFace, ButtonVariant, face_height, hover_light};
use super::label::{draw_label, label_width};
use super::menu::{Menu, MenuEntry, MenuItem, MenuResponse};
use super::popup::PopupSide;
use super::text_field::{TextField, TextFieldResponse};

// ========================================================================
// Constants
// ========================================================================

/// What stands between two crumbs.
const SEPARATOR_GLYPH: &str = "\u{203a}";

/// What the ellipsis hiding the middle crumbs shows.
const ELLIPSIS_GLYPH: &str = "\u{2026}";

/// The blank cells on each side of a separator.
const SEPARATOR_PADDING_CELLS: f32 = 0.5;

/// The child index of the ellipsis's accessibility node; the crumbs'
/// count from 0.
const ELLIPSIS_NODE: u64 = 1 << 32;

/// The child index of the address field's accessibility node.
const FIELD_NODE: u64 = 1 << 33;

/// The child index, under the ellipsis's node, of its menu's node.
const MENU_NODE: u64 = 0;

// ========================================================================
// Data Structures
// ========================================================================

/// A row of crumbs showing where the user is. The app owns it, hands it
/// the window's pointer events (and its keys while it has the focus), and
/// draws it each frame; [`BreadcrumbResponse::Activated`] names the crumb
/// the user went to.
#[derive(Clone, Debug)]
pub struct Breadcrumb {
    /// The address field, shown while editing.
    address: TextField,
    /// Whether the address field shows in place of the crumbs.
    editing: bool,
    /// The crumb or ellipsis with the keyboard focus inside the row.
    focus: Option<Stop>,
    /// Whether the app gave it the keyboard.
    focused: bool,
    /// The crumb or ellipsis under the pointer.
    hovered: Option<Stop>,
    /// Where the parts were drawn last, for the pointer.
    layout: Option<BreadcrumbLayout>,
    /// The menu of the crumbs the ellipsis hides.
    menu: Menu<usize>,
    /// The crumb or ellipsis a held left press started on.
    pressed: Option<Stop>,
    /// The segments, first to last.
    segments: Vec<Crumb>,
    /// The words of the theme last drawn in, for screen readers.
    strings: Strings,
}

/// One segment of a [`Breadcrumb`]: a label with an optional icon.
#[derive(Clone, Debug, PartialEq)]
pub struct Crumb {
    /// The icon left of the label, if any.
    icon: Option<Svg>,
    /// What it says.
    label: String,
}

/// What became of an input offered to a [`Breadcrumb`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BreadcrumbResponse {
    /// The user went to the segment at this index, by its crumb or from
    /// the ellipsis's menu (which closed).
    Activated(usize),
    /// The user gave up editing with Esc; the crumbs show again.
    Cancelled,
    /// The address being edited changed: read it with
    /// [`Breadcrumb::address`].
    Changed,
    /// The ellipsis's menu closed without a choice.
    Closed,
    /// The breadcrumb used the input.
    Handled,
    /// The input is not the breadcrumb's.
    Ignored,
    /// The ellipsis's menu opened; while it is, the breadcrumb takes every
    /// pointer input.
    Opened,
    /// The user pressed Enter on this address; the crumbs show again.
    Submitted(String),
}

/// A place the keyboard focus and the pointer stop at.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Stop {
    /// The crumb of the segment at this index.
    Crumb(usize),
    /// The ellipsis.
    Ellipsis,
}

/// Where a breadcrumb's parts were drawn.
#[derive(Clone, Debug, PartialEq)]
struct BreadcrumbLayout {
    /// The whole row.
    area: Rect,
    /// Each crumb shown, with its segment's index, first to last.
    crumbs: Vec<(usize, Rect)>,
    /// The ellipsis, when crumbs collapsed.
    ellipsis: Option<Rect>,
    /// The first and one past the last index of the segments collapsed.
    hidden: (usize, usize),
    /// Each separator's room.
    separators: Vec<Rect>,
}

// ========================================================================
// Breadcrumb Implementations
// ========================================================================

impl Breadcrumb {
    /// A breadcrumb showing `segments`, first to last, with an empty
    /// address.
    pub fn new(segments: Vec<Crumb>) -> Self {
        Self {
            address: TextField::new().with_frame(),
            editing: false,
            focus: None,
            focused: false,
            hovered: None,
            layout: None,
            menu: Menu::new(Vec::new()),
            pressed: None,
            segments,
            strings: Strings::english(),
        }
    }

    /// Shows `segments` in place of the ones shown, first to last, with
    /// the keyboard focus on the last.
    pub fn set_segments(&mut self, segments: Vec<Crumb>) {
        self.segments = segments;
        self.focus = None;
        self.hovered = None;
        self.pressed = None;
        self.layout = None;
        self.menu.close();
    }

    /// Sets the text the address field holds when editing starts (or now,
    /// while editing): the whole location as the user would type it.
    pub fn set_address(&mut self, address: &str) {
        self.address.set_text(address);
    }

    /// The text in the address field.
    pub fn address(&self) -> &str {
        self.address.text()
    }

    /// Shows the address field, all its text selected, in place of the
    /// crumbs (`true`), or the crumbs again.
    pub fn set_editing(&mut self, editing: bool) {
        if editing && !self.editing {
            self.address.select_all();
            self.menu.close();
        }
        self.editing = editing;
        self.address.set_focused(editing && self.focused);
    }

    /// Whether the address field shows in place of the crumbs.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// Where the address field's caret was drawn while editing, for the
    /// input method (see [`ImeFocus`](crate::ImeFocus)); `None` otherwise.
    pub fn cursor_area(&self) -> Option<Rect> {
        self.address.cursor_area().filter(|_| self.editing)
    }

    /// Whether the ellipsis's menu is open.
    pub fn is_menu_open(&self) -> bool {
        self.menu.is_open()
    }

    /// How tall it is drawn with `metrics` on a display scaled by
    /// `scale_factor`: as tall as a button or the address field, whichever
    /// is taller, so editing does not move what is below.
    pub fn height(&self, metrics: CellMetrics, scale_factor: f64) -> f32 {
        face_height(metrics).max(self.address.height(metrics, scale_factor))
    }

    /// Describes the breadcrumb as last drawn to screen readers as `id`:
    /// a navigation landmark named by the theme's `breadcrumb` string
    /// holding a link per crumb shown (the segment at `index` is child
    /// `index` of `id`) and the ellipsis (child `1 << 32`), a button
    /// holding its menu while open (child 0 of its node); or, while
    /// editing, the address field (child `1 << 33`). The focused crumb has
    /// the focus while the breadcrumb does. Nothing before the first
    /// frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = &self.layout else {
            return;
        };
        let mut nav = Node::new(Role::Navigation);
        nav.set_label(self.strings.breadcrumb.as_ref());
        frame.enter_access(id, layout.area, nav);
        if self.editing {
            self.address.access(frame, id.child(FIELD_NODE));
            frame.leave_access();
            return;
        }
        for (index, rect) in &layout.crumbs {
            let mut node = Node::new(Role::Link);
            node.set_label(self.segments[*index].label.as_str());
            node.add_action(Action::Click);
            frame.access(id.child(*index as u64), *rect, node);
        }
        if let Some(rect) = layout.ellipsis {
            let ellipsis = id.child(ELLIPSIS_NODE);
            let mut node = Node::new(Role::Button);
            node.set_label(self.strings.more.as_ref());
            node.set_has_popup(HasPopup::Menu);
            node.set_expanded(self.menu.is_open());
            node.add_action(Action::Click);
            frame.enter_access(ellipsis, rect, node);
            self.menu.access(frame, ellipsis.child(MENU_NODE));
            frame.leave_access();
        }
        frame.leave_access();
        if let Some(stop) = self.focus.filter(|_| !self.menu.is_open()) {
            frame.access.set_inner_focus(id, stop_node(id, stop));
        }
    }

    /// Does what a screen reader's `action` on the breadcrumb described as
    /// `id` asks: a click on a crumb goes there, one on the ellipsis opens
    /// its menu, and one on an item of that menu goes to its segment.
    /// Ignored for other actions and nodes, and while editing.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> BreadcrumbResponse {
        let AccessAction::Click(target) = action else {
            return BreadcrumbResponse::Ignored;
        };
        if self.editing {
            return BreadcrumbResponse::Ignored;
        }
        let ellipsis = id.child(ELLIPSIS_NODE);
        if target == ellipsis {
            return self.activate(Stop::Ellipsis);
        }
        if self.menu.is_open() {
            let response = self.menu.access_action(ellipsis.child(MENU_NODE), action);
            return settle_menu(response);
        }
        let shown = self.layout.as_ref().and_then(|layout| {
            layout
                .crumbs
                .iter()
                .find(|(index, _)| id.child(*index as u64) == target)
        });
        match shown {
            Some((index, _)) => BreadcrumbResponse::Activated(*index),
            None => BreadcrumbResponse::Ignored,
        }
    }

    /// The places the keyboard focus stops at, as last drawn: the crumbs
    /// shown and the ellipsis, left to right.
    fn stops(&self) -> Vec<Stop> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let mut stops: Vec<Stop> = layout
            .crumbs
            .iter()
            .map(|(index, _)| Stop::Crumb(*index))
            .collect();
        if layout.ellipsis.is_some() {
            // The ellipsis sits after the first crumb.
            stops.insert(stops.len().min(1), Stop::Ellipsis);
        }
        stops
    }

    /// The stop under `point`, as last drawn.
    fn stop_at(&self, point: Point) -> Option<Stop> {
        let layout = self.layout.as_ref()?;
        if layout.ellipsis.is_some_and(|rect| rect.contains(point)) {
            return Some(Stop::Ellipsis);
        }
        layout
            .crumbs
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(index, _)| Stop::Crumb(*index))
    }

    /// Activates `stop`: goes to a crumb's segment, or opens the
    /// ellipsis's menu of the crumbs it hides.
    fn activate(&mut self, stop: Stop) -> BreadcrumbResponse {
        let collapsed = match stop {
            Stop::Crumb(index) => return BreadcrumbResponse::Activated(index),
            Stop::Ellipsis => self.layout.as_ref().and_then(|layout| {
                let (start, end) = layout.hidden;
                layout.ellipsis.map(|rect| (rect, start, end))
            }),
        };
        let Some((anchor, start, end)) = collapsed else {
            return BreadcrumbResponse::Ignored;
        };
        let entries = (start..end)
            .map(|index| MenuEntry::Item(MenuItem::new(index, self.segments[index].label.as_str())))
            .collect();
        self.menu = Menu::new(entries);
        self.menu.open(anchor, PopupSide::Below);
        self.focus = Some(Stop::Ellipsis);
        BreadcrumbResponse::Opened
    }

    /// Moves the keyboard focus `delta` stops, wrapping.
    fn move_focus(&mut self, delta: isize) -> BreadcrumbResponse {
        let stops = self.stops();
        if stops.is_empty() {
            return BreadcrumbResponse::Ignored;
        }
        let count = stops.len() as isize;
        let position = self
            .focus
            .and_then(|focus| stops.iter().position(|stop| *stop == focus))
            .map_or(count - 1, |position| {
                (position as isize + delta).rem_euclid(count)
            });
        self.focus = Some(stops[position as usize]);
        BreadcrumbResponse::Handled
    }

    /// Moves the keyboard focus to the first stop, or the last when
    /// `last`.
    fn focus_end(&mut self, last: bool) -> BreadcrumbResponse {
        let stops = self.stops();
        let stop = match last {
            true => stops.last(),
            false => stops.first(),
        };
        match stop {
            Some(stop) => {
                self.focus = Some(*stop);
                BreadcrumbResponse::Handled
            }
            None => BreadcrumbResponse::Ignored,
        }
    }

    /// Reacts to `key` while the crumbs show, as the module documentation
    /// lists, while focused.
    fn handle_key(&mut self, key: Key) -> BreadcrumbResponse {
        if !self.focused || key.mods != Mods::NONE {
            return BreadcrumbResponse::Ignored;
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => self.move_focus(-1),
            KeyCode::Right | KeyCode::Char('l') => self.move_focus(1),
            KeyCode::Home => self.focus_end(false),
            KeyCode::End => self.focus_end(true),
            KeyCode::Char(' ') | KeyCode::Enter => match self.focus {
                Some(stop) => self.activate(stop),
                None => BreadcrumbResponse::Ignored,
            },
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Insert
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Tab
            | KeyCode::Up => BreadcrumbResponse::Ignored,
        }
    }

    /// Reacts to `input` while editing: Enter submits the address and Esc
    /// gives up, both leaving the field; the rest is the field's.
    fn handle_editing(&mut self, input: &Input, cx: &mut EventCx<'_>) -> BreadcrumbResponse {
        let plain = input.key().filter(|key| key.mods == Mods::NONE);
        let response = match plain.map(|key| key.code) {
            Some(KeyCode::Enter) if self.focused => {
                self.set_editing(false);
                BreadcrumbResponse::Submitted(self.address.text().to_owned())
            }
            Some(KeyCode::Esc) if self.focused => {
                self.set_editing(false);
                BreadcrumbResponse::Cancelled
            }
            Some(_) | None => {
                if !self.focused && matches!(input, Input::Key(_) | Input::Ime(_)) {
                    return BreadcrumbResponse::Ignored;
                }
                return match self.address.handle(input, cx) {
                    TextFieldResponse::Changed => BreadcrumbResponse::Changed,
                    TextFieldResponse::Handled => BreadcrumbResponse::Handled,
                    TextFieldResponse::Ignored => BreadcrumbResponse::Ignored,
                };
            }
        };
        cx.request_redraw();
        response
    }

    /// Reacts to a left press or release at `point`: a press on a crumb
    /// holds it (and opens the ellipsis's menu at once), and the release
    /// over the crumb pressed goes there.
    fn press(&mut self, pressed: bool, point: Point) -> BreadcrumbResponse {
        let under = self.stop_at(point);
        match (pressed, under, self.pressed.take()) {
            (true, Some(Stop::Ellipsis), _) => self.activate(Stop::Ellipsis),
            (true, Some(stop), _) => {
                self.pressed = Some(stop);
                self.focus = Some(stop);
                BreadcrumbResponse::Handled
            }
            (false, Some(stop), Some(held)) if stop == held => self.activate(stop),
            (false, _, Some(_)) => BreadcrumbResponse::Handled,
            (true, None, _) | (false, _, None) => BreadcrumbResponse::Ignored,
        }
    }

    /// Follows the pointer to `point` (`None` outside the window), asking a
    /// redraw when it moved onto another crumb; says whether it is over
    /// one.
    fn follow_pointer(&mut self, point: Option<Point>, cx: &mut EventCx<'_>) -> bool {
        let under = point.and_then(|point| self.stop_at(point));
        if under != self.hovered {
            self.hovered = under;
            cx.request_redraw();
        }
        under.is_some()
    }

    /// Lays the crumbs out in `area`, each as wide as `widths` says (by
    /// segment), separators `separator` wide and the ellipsis `ellipsis`
    /// wide, collapsing the middle crumbs, those nearest the start first,
    /// until the row fits or only the first and last are left.
    fn lay_out(
        &self,
        area: Rect,
        widths: &[f32],
        separator: f32,
        ellipsis: f32,
    ) -> BreadcrumbLayout {
        let count = widths.len();
        let row_width = |hidden: (usize, usize)| {
            let (start, end) = hidden;
            let shown = count - (end - start);
            let collapsed = start < end;
            let pieces = shown + usize::from(collapsed);
            let crumbs: f32 = (0..count)
                .filter(|index| !(start..end).contains(index))
                .map(|index| widths[index])
                .sum();
            let marker = if collapsed { ellipsis } else { 0.0 };
            crumbs + marker + separator * pieces.saturating_sub(1) as f32
        };
        let mut hidden = (1, 1);
        while row_width(hidden) > area.width && hidden.1 + 1 < count {
            hidden.1 += 1;
        }
        let mut layout = BreadcrumbLayout {
            area,
            crumbs: Vec::new(),
            ellipsis: None,
            hidden: (0, 0),
            separators: Vec::new(),
        };
        if hidden.0 < hidden.1 {
            layout.hidden = hidden;
        }
        let mut x = area.x;
        let mut place = |width: f32, layout: &mut BreadcrumbLayout| {
            if x > area.x {
                layout
                    .separators
                    .push(Rect::new(x, area.y, separator, area.height));
                x += separator;
            }
            let rect = Rect::new(x, area.y, width, area.height);
            x += width;
            rect
        };
        for (index, width) in widths.iter().enumerate() {
            let (start, end) = layout.hidden;
            if index == start && start < end {
                layout.ellipsis = Some(place(ellipsis, &mut layout));
            }
            if (start..end).contains(&index) {
                continue;
            }
            let rect = place(*width, &mut layout);
            layout.crumbs.push((index, rect));
        }
        layout
    }
}

impl Widget for Breadcrumb {
    type Output = Option<Rect>;
    type Response = BreadcrumbResponse;

    /// Reacts to `input`: while editing, the address field takes it, Enter
    /// and Esc ending the edit; while the ellipsis's menu is open, every
    /// input is the menu's; otherwise keys as the module documentation
    /// lists while focused, a left press and release over a crumb going
    /// there, and the pointer lighting the crumb under it. Asks for a
    /// redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> BreadcrumbResponse {
        if self.editing {
            return self.handle_editing(input, cx);
        }
        if self.menu.is_open() {
            let response = self.menu.handle(input, cx);
            return settle_menu(response);
        }
        let response = match input {
            Input::Key(key) => self.handle_key(*key),
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.follow_pointer(Some(button.position), cx);
                self.press(button.pressed, button.position)
            }
            Input::PointerMoved(position) => match self.follow_pointer(Some(*position), cx) {
                true => BreadcrumbResponse::Handled,
                false => BreadcrumbResponse::Ignored,
            },
            Input::PointerLeft => {
                self.follow_pointer(None, cx);
                BreadcrumbResponse::Ignored
            }
            Input::FocusLost if self.pressed.is_some() => {
                self.pressed = None;
                BreadcrumbResponse::Handled
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => BreadcrumbResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the breadcrumb across the top of `area`, as tall as
    /// [`Breadcrumb::height`]: the address field while editing, else the
    /// crumbs from the left between muted separators, collapsed into an
    /// ellipsis when they do not fit; then the ellipsis's open menu in the
    /// overlay. Returns where the menu was drawn (for a
    /// [`FocusTree`](crate::FocusTree) trap), or `None` while it is closed.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Option<Rect> {
        if self.strings != theme.strings {
            self.strings = theme.strings.clone();
        }
        let metrics = frame.cell_metrics();
        let height = self.height(metrics, frame.scale_factor()).min(area.height);
        let row = Rect::new(area.x, area.y, area.width, height);
        if self.editing {
            self.address.draw(frame, row, theme);
            self.layout = Some(BreadcrumbLayout {
                area: row,
                crumbs: Vec::new(),
                ellipsis: None,
                hidden: (0, 0),
                separators: Vec::new(),
            });
            return None;
        }
        let widths: Vec<f32> = self
            .segments
            .iter()
            .map(|crumb| crumb.face(false, false, false).width(frame, theme))
            .collect();
        let padding = SEPARATOR_PADDING_CELLS * metrics.width;
        let separator = (label_width(frame, theme, SEPARATOR_GLYPH) + 2.0 * padding).ceil();
        let ellipsis = ellipsis_face(false, false).width(frame, theme);
        let layout = self.lay_out(row, &widths, separator, ellipsis);
        let stops = {
            self.layout = Some(layout);
            self.stops()
        };
        if self.focus.is_none_or(|focus| !stops.contains(&focus)) {
            self.focus = stops.last().copied();
        }
        let ring = |stop: Stop| self.focused && self.focus == Some(stop);
        let lit = |stop: Stop| self.hovered == Some(stop);
        let held = |stop: Stop| self.pressed == Some(stop) && self.hovered == Some(stop);
        let Some(layout) = &self.layout else {
            return None;
        };
        let text_y = (row.y + (row.height - metrics.height) / 2.0).round();
        frame.clipped(row, |frame| {
            for rect in &layout.separators {
                let glyph = label_width(frame, theme, SEPARATOR_GLYPH);
                let x = (rect.x + (rect.width - glyph) / 2.0).round();
                draw_label(
                    frame,
                    theme,
                    SEPARATOR_GLYPH,
                    Point::new(x, text_y),
                    theme.muted,
                );
            }
            for (index, rect) in &layout.crumbs {
                let stop = Stop::Crumb(*index);
                let face = self.segments[*index].face(ring(stop), lit(stop), held(stop));
                face.draw(frame, *rect, theme);
            }
            if let Some(rect) = layout.ellipsis {
                let mut face = ellipsis_face(ring(Stop::Ellipsis), lit(Stop::Ellipsis));
                face.active = self.menu.is_open();
                face.draw(frame, rect, theme);
            }
        });
        let window = frame.bounds();
        self.menu.draw(frame, window, theme)
    }

    /// Tells it whether it has the keyboard: it takes keys only while it
    /// does (a [`FocusTree`](crate::FocusTree) draws the focus ring around
    /// it; the breadcrumb marks its focused crumb).
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.address.set_focused(focused && self.editing);
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }

    /// Blinks the address field's caret while editing, returning when it
    /// next shows or hides; `None` otherwise.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.address.about_to_wait(cx)
    }
}

impl Outcome for BreadcrumbResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Crumb Implementations
// ========================================================================

impl Crumb {
    /// A segment saying `label`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            icon: None,
            label: label.into(),
        }
    }

    /// This segment showing `icon`, a one-color SVG, left of its label.
    pub fn with_icon(self, icon: Svg) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// How the crumb is painted: flat, ringed when `ring`, lit when
    /// `hovered`, pressed in when `pressed`.
    fn face(&self, ring: bool, hovered: bool, pressed: bool) -> ButtonFace<'_> {
        ButtonFace {
            active: false,
            enabled: true,
            focus_ring: ring,
            hover: hover_light(hovered),
            icon: self.icon.as_ref(),
            label: &self.label,
            pressed,
            variant: ButtonVariant::Ghost,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How the ellipsis is painted: flat, ringed when `ring`, lit when
/// `hovered`.
fn ellipsis_face(ring: bool, hovered: bool) -> ButtonFace<'static> {
    ButtonFace {
        active: false,
        enabled: true,
        focus_ring: ring,
        hover: hover_light(hovered),
        icon: None,
        label: ELLIPSIS_GLYPH,
        pressed: false,
        variant: ButtonVariant::Ghost,
    }
}

/// What the ellipsis's menu's `response` means for the breadcrumb.
fn settle_menu(response: MenuResponse<usize>) -> BreadcrumbResponse {
    match response {
        MenuResponse::Activated(index) => BreadcrumbResponse::Activated(index),
        MenuResponse::Closed => BreadcrumbResponse::Closed,
        MenuResponse::Handled => BreadcrumbResponse::Handled,
        MenuResponse::Ignored => BreadcrumbResponse::Ignored,
    }
}

/// The accessibility id of `stop` in the breadcrumb described as `id`.
fn stop_node(id: FocusId, stop: Stop) -> FocusId {
    match stop {
        Stop::Crumb(index) => id.child(index as u64),
        Stop::Ellipsis => id.child(ELLIPSIS_NODE),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn left(pressed: bool, point: Point) -> Input {
        let input = match pressed {
            true => ButtonInput::press(PointerButton::Left, point),
            false => ButtonInput::release(PointerButton::Left, point),
        };
        Input::PointerButton(input)
    }

    fn center(rect: Rect) -> Point {
        Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    /// `home`, `trung`, `Workspace`, `apps`, `misty`.
    fn breadcrumb() -> Breadcrumb {
        let segments = ["home", "trung", "Workspace", "apps", "misty"];
        Breadcrumb::new(segments.into_iter().map(Crumb::new).collect())
    }

    /// `breadcrumb` drawn `width` pixels wide in `window`.
    fn draw(window: &TestWindow, breadcrumb: &mut Breadcrumb, width: f32) -> Frame {
        let mut frame = window.frame();
        breadcrumb.draw(&mut frame, Rect::new(0.0, 0.0, width, 40.0), &Theme::dark());
        frame
    }

    fn shown(breadcrumb: &Breadcrumb) -> Vec<usize> {
        let layout = breadcrumb.layout.as_ref().expect("drawn");
        layout.crumbs.iter().map(|(index, _)| *index).collect()
    }

    #[test]
    fn test_narrow_rows_collapse_the_middle_crumbs_keeping_the_first_and_last() {
        let layout = |width: f32| {
            breadcrumb().lay_out(
                Rect::new(0.0, 0.0, width, 30.0),
                &[50.0, 50.0, 50.0, 50.0, 50.0],
                10.0,
                20.0,
            )
        };
        let wide = layout(290.0);
        assert_eq!(wide.crumbs.len(), 5, "five crumbs and four separators fit");
        assert!(wide.ellipsis.is_none());
        let narrow = layout(200.0);
        let indices: Vec<usize> = narrow.crumbs.iter().map(|(index, _)| *index).collect();
        assert_eq!(indices, [0, 3, 4], "those nearest the start go first");
        assert_eq!(narrow.hidden, (1, 3));
        let ellipsis = narrow.ellipsis.expect("an ellipsis");
        assert_eq!(ellipsis.x, 60.0, "after the first crumb and a separator");
        let tiny = layout(10.0);
        let indices: Vec<usize> = tiny.crumbs.iter().map(|(index, _)| *index).collect();
        assert_eq!(indices, [0, 4], "the first and the last always stay");
    }

    #[test]
    fn test_left_right_and_enter_go_to_crumbs_and_the_ellipsis_opens_the_hidden_ones() {
        let mut window = TestWindow::new(800, 400);
        let mut breadcrumb = breadcrumb();
        breadcrumb.set_focused(true);
        draw(&window, &mut breadcrumb, 200.0);
        assert_eq!(shown(&breadcrumb).first(), Some(&0));
        assert_eq!(
            window.feed(&mut breadcrumb, &key("<cr>")),
            BreadcrumbResponse::Activated(4),
            "the focus starts on the last crumb"
        );
        window.feed(&mut breadcrumb, &key("<home>"));
        assert_eq!(
            window.feed(&mut breadcrumb, &key("<cr>")),
            BreadcrumbResponse::Activated(0)
        );
        window.feed(&mut breadcrumb, &key("l"));
        assert_eq!(
            window.feed(&mut breadcrumb, &key("<space>")),
            BreadcrumbResponse::Opened
        );
        let mut frame = draw(&window, &mut breadcrumb, 200.0);
        let id = FocusId::new("path");
        breadcrumb.access(&mut frame, id);
        let text = ui_text(&frame, "t");
        assert!(
            text.contains("Button \"More\" expanded\n")
                && text.contains("MenuItem \"trung\"\n")
                && text.contains("MenuItem \"Workspace\"\n"),
            "{text}"
        );
        window.feed(&mut breadcrumb, &key("<down>"));
        window.feed(&mut breadcrumb, &key("<down>"));
        assert_eq!(
            window.feed(&mut breadcrumb, &key("<cr>")),
            BreadcrumbResponse::Activated(2)
        );
        assert!(!breadcrumb.is_menu_open());
    }

    #[test]
    fn test_a_click_goes_to_the_crumb_released_over() {
        let mut window = TestWindow::new(800, 60);
        let mut breadcrumb = breadcrumb();
        draw(&window, &mut breadcrumb, 800.0);
        let layout = breadcrumb.layout.clone().expect("drawn");
        let (home, apps) = (layout.crumbs[0].1, layout.crumbs[3].1);
        window.feed(&mut breadcrumb, &left(true, center(home)));
        assert_eq!(
            window.feed(&mut breadcrumb, &left(false, center(apps))),
            BreadcrumbResponse::Handled
        );
        window.feed(&mut breadcrumb, &left(true, center(apps)));
        assert_eq!(
            window.feed(&mut breadcrumb, &left(false, center(apps))),
            BreadcrumbResponse::Activated(3)
        );
    }

    #[test]
    fn test_the_address_fields_caret_blinks_only_while_editing() {
        let mut window = TestWindow::new(800, 60);
        let mut breadcrumb = breadcrumb();
        breadcrumb.set_focused(true);
        draw(&window, &mut breadcrumb, 800.0);
        assert_eq!(breadcrumb.about_to_wait(&mut window.cx()), None);
        breadcrumb.set_editing(true);
        draw(&window, &mut breadcrumb, 800.0);
        assert!(breadcrumb.about_to_wait(&mut window.cx()).is_some());
    }

    #[test]
    fn test_editing_shows_the_address_and_enter_or_escape_return_to_the_crumbs() {
        let mut window = TestWindow::new(800, 60);
        let mut breadcrumb = breadcrumb();
        breadcrumb.set_focused(true);
        breadcrumb.set_address("/home/trung");
        breadcrumb.set_editing(true);
        draw(&window, &mut breadcrumb, 800.0);
        assert_eq!(
            window.feed(&mut breadcrumb, &key("x")),
            BreadcrumbResponse::Changed
        );
        assert_eq!(breadcrumb.address(), "x", "the address was all selected");
        assert_eq!(
            window.feed(&mut breadcrumb, &key("<cr>")),
            BreadcrumbResponse::Submitted(String::from("x"))
        );
        assert!(!breadcrumb.is_editing());
        breadcrumb.set_editing(true);
        assert_eq!(
            window.feed(&mut breadcrumb, &key("<esc>")),
            BreadcrumbResponse::Cancelled
        );
        assert!(!breadcrumb.is_editing());
        let mut frame = draw(&window, &mut breadcrumb, 800.0);
        breadcrumb.access(&mut frame, FocusId::new("path"));
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n\
             \x20 Navigation \"Breadcrumb\"\n\
             \x20   Link \"home\"\n\
             \x20   Link \"trung\"\n\
             \x20   Link \"Workspace\"\n\
             \x20   Link \"apps\"\n\
             \x20   Link \"misty\"\n"
        );
    }
}

//! [`RegionBox`]: what the store keeps for an element that takes the
//! pointer, or the keyboard for a press, through properties rather than a
//! widget of its own ([`Element::on_press`](crate::Element::on_press),
//! `on_hover`, `on_pointer_move`, `on_scroll`, a drag source, a drop target, a file drop
//! target, a hover background). The region is registered in the focus
//! tree under [`region_id`] before whatever the element holds, so the
//! controls inside it take the pointer first and what they leave falls
//! through to it, and it is visited by Tab before them.

use std::time::Instant;

use accesskit::{Action, Node as AccessNode, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton, WheelInput};
use super::super::widgets::drag::{Drag, DragResponse, Dropped, FileDropped};
use super::controls::call;
use super::node::Call;
use super::props::{Extras, Payload, PointerDrag, Press};

// ========================================================================
// Constants
// ========================================================================

/// The child index, under an element's id, of the region it takes the
/// pointer in: apart from its children, its popup, its layers, and the
/// region a control takes over everything.
const REGION_NODE: u64 = 1 << 35;

// ========================================================================
// Data Structures
// ========================================================================

/// What lasts of an element's region between rebuilds: where it was
/// drawn, whether the pointer is over it, and whether it has the
/// keyboard.
#[derive(Debug, Default)]
pub(crate) struct RegionBox {
    /// Where, from its top left, the left or middle button was pressed on
    /// it for a pointer drag still under way, and which.
    dragging_from: Option<(Point, PointerButton)>,
    /// Where, from its top left, a press that a drag source asking for
    /// modifiers took landed, and the modifiers held, until it starts a
    /// drag or is released as a click.
    held_click: Option<(Point, Mods)>,
    /// Whether it has the keyboard.
    focused: bool,
    /// Whether the pointer is over it.
    hovered: bool,
    /// Whether `on_pointer_move` last reported the pointer over it, so
    /// its leaving is reported once.
    moving: bool,
    /// Where it was drawn last.
    rect: Rect,
}

/// What a region did with one input.
pub(crate) struct RegionAnswer<A> {
    /// The callback calls it made.
    pub(crate) calls: Vec<Call<A>>,
    /// A drag it carried that was dropped, for the store to hand to the
    /// target under it.
    pub(crate) dropped: Option<Dropped<Payload>>,
    /// Whether it used the input.
    pub(crate) used: bool,
}

/// What a region handling an input may change besides itself.
pub(crate) struct RegionCx<'c, 'w> {
    /// The window's context.
    pub(crate) cx: &'c mut EventCx<'w>,
    /// The window's one drag between elements.
    pub(crate) drag: &'c mut Drag<Payload>,
    /// What the label following the drag says.
    pub(crate) drag_label: &'c mut String,
    /// The scale factor the window was last drawn at.
    pub(crate) scale: f64,
}

// ========================================================================
// RegionBox Implementations
// ========================================================================

impl RegionBox {
    /// Whether the pointer is over it.
    pub(crate) fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Notes that it was drawn at `rect`.
    pub(crate) fn place(&mut self, rect: Rect) {
        self.rect = rect;
    }

    /// Hands `input` to the region of the element with `extras`: a press
    /// runs `on_press` (and arms a drag source's drag), the pointer
    /// coming and going runs `on_hover`, the wheel `on_scroll`, a file
    /// dropped `on_file_drop`, and Enter or Space while it has the
    /// keyboard press it.
    pub(crate) fn handle<A: 'static>(
        &mut self,
        extras: &Extras<'_, A>,
        input: &Input,
        rcx: &mut RegionCx<'_, '_>,
    ) -> RegionAnswer<A> {
        let mut answer = RegionAnswer {
            calls: Vec::new(),
            dropped: None,
            used: false,
        };
        let origin = Point::new(self.rect.x, self.rect.y);
        match input {
            Input::PointerButton(button) if button.pressed => {
                if let Some(drag) = &extras.drag
                    && button.button == PointerButton::Left
                    && drag.mods.is_none_or(|mods| mods == button.mods)
                {
                    rcx.drag
                        .arm(drag.payload.clone(), button.position, rcx.scale);
                    *rcx.drag_label = drag.label.to_string();
                    answer.used = true;
                    // A drag that asked for modifiers takes the press alone;
                    // released without dragging, it is a click after all.
                    if drag.mods.is_some() {
                        self.held_click = Some((local(button.position, origin), button.mods));
                        return answer;
                    }
                }
                if extras.on_pointer_drag.is_some()
                    && matches!(button.button, PointerButton::Left | PointerButton::Middle)
                {
                    self.dragging_from = Some((local(button.position, origin), button.button));
                    answer.used = true;
                }
                let press = Press {
                    button: button.button,
                    by_key: false,
                    clicks: rcx.cx.click_count().max(1),
                    mods: button.mods,
                    point: local(button.position, origin),
                };
                for apply in [&extras.on_pointer_press, &extras.on_press] {
                    if let Some(apply) = call(apply.as_ref(), press) {
                        answer.calls.push(apply);
                        answer.used = true;
                    }
                }
            }
            Input::PointerButton(_) | Input::PointerMoved(_) => {
                if let Input::PointerMoved(point) = input {
                    self.follow_hover(extras, self.rect.contains(*point), &mut answer);
                    self.follow_move(extras, *point, origin, &mut answer);
                }
                let mods = rcx.cx.mods();
                self.follow_pointer_drag(extras, input, origin, mods, &mut answer);
                match rcx.drag.handle_event(input, rcx.cx) {
                    DragResponse::Dropped(dropped) => {
                        self.held_click = None;
                        answer.dropped = Some(dropped);
                        answer.used = true;
                    }
                    DragResponse::Moved | DragResponse::Started => {
                        self.held_click = None;
                        answer.used = true;
                    }
                    DragResponse::Cancelled | DragResponse::Ignored => {}
                }
                self.release_held_click(extras, input, &mut answer);
            }
            Input::PointerLeft => {
                self.follow_hover(extras, false, &mut answer);
                self.report_left(extras, &mut answer);
            }
            Input::Wheel(wheel) => {
                let wheel = WheelInput {
                    position: local(wheel.position, origin),
                    ..*wheel
                };
                if let Some(apply) = call(extras.on_scroll.as_ref(), wheel) {
                    answer.calls.push(apply);
                    answer.used = true;
                }
            }
            Input::FileDropped(file) => {
                let file = FileDropped {
                    path: file.path.clone(),
                    position: file.position.map(|point| local(point, origin)),
                };
                if let Some(apply) = call(extras.file_drop.as_ref(), file) {
                    answer.calls.push(apply);
                    answer.used = true;
                }
            }
            Input::FileHovered(_) => answer.used = extras.file_drop.is_some(),
            Input::Key(key)
                if self.focused
                    && key.mods == Mods::NONE
                    && matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) =>
            {
                if let Some(apply) = call(extras.on_press.as_ref(), self.key_press()) {
                    answer.calls.push(apply);
                    answer.used = true;
                }
            }
            // The window losing the keyboard ends the pointer's capture, and
            // so the drag; no release will come.
            Input::FocusLost => {
                self.dragging_from = None;
                self.held_click = None;
            }
            Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_)
            | Input::Key(_)
            | Input::KeyReleased(_) => {}
        }
        answer
    }

    /// The press a key or a screen reader makes: the left button at its
    /// middle.
    pub(crate) fn key_press(&self) -> Press {
        Press {
            button: PointerButton::Left,
            by_key: true,
            clicks: 1,
            mods: Mods::NONE,
            point: Point::new(self.rect.width / 2.0, self.rect.height / 2.0),
        }
    }

    /// Reports a press a drag source asking for modifiers took, released
    /// with the left button before it dragged, to `on_pointer_drag` into
    /// `answer` as a click: done, where it was pressed, with its
    /// modifiers.
    fn release_held_click<A: 'static>(
        &mut self,
        extras: &Extras<'_, A>,
        input: &Input,
        answer: &mut RegionAnswer<A>,
    ) {
        let Input::PointerButton(button) = input else {
            return;
        };
        if button.pressed || button.button != PointerButton::Left {
            return;
        }
        let Some((point, mods)) = self.held_click.take() else {
            return;
        };
        let click = PointerDrag {
            button: PointerButton::Left,
            done: true,
            mods,
            point,
            start: point,
        };
        answer
            .calls
            .extend(call(extras.on_pointer_drag.as_ref(), click));
        answer.used = true;
    }

    /// Reports a move, or the release, of the pointer drag under way to
    /// `on_pointer_drag` into `answer`, the points from `origin`, `mods`
    /// the modifiers held.
    fn follow_pointer_drag<A: 'static>(
        &mut self,
        extras: &Extras<'_, A>,
        input: &Input,
        origin: Point,
        mods: Mods,
        answer: &mut RegionAnswer<A>,
    ) {
        let Some((start, pressed)) = self.dragging_from else {
            return;
        };
        let (point, mods, done) = match input {
            Input::PointerMoved(point) => (*point, mods, false),
            Input::PointerButton(button) if button.button == pressed => {
                (button.position, button.mods, true)
            }
            _ => return,
        };
        if done {
            self.dragging_from = None;
        }
        let drag = PointerDrag {
            button: pressed,
            done,
            mods,
            point: local(point, origin),
            start,
        };
        answer
            .calls
            .extend(call(extras.on_pointer_drag.as_ref(), drag));
        answer.used = true;
    }

    /// Reports the pointer at `point` to `on_pointer_move` into `answer`,
    /// from `origin`, while it is over the region, and its leaving once.
    fn follow_move<A: 'static>(
        &mut self,
        extras: &Extras<'_, A>,
        point: Point,
        origin: Point,
        answer: &mut RegionAnswer<A>,
    ) {
        if !self.rect.contains(point) {
            self.report_left(extras, answer);
            return;
        }
        self.moving = true;
        let local = local(point, origin);
        answer
            .calls
            .extend(call(extras.on_pointer_move.as_ref(), Some(local)));
    }

    /// Reports to `on_pointer_move` into `answer` that the pointer left
    /// the region, unless it reported that already.
    fn report_left<A: 'static>(&mut self, extras: &Extras<'_, A>, answer: &mut RegionAnswer<A>) {
        if !std::mem::take(&mut self.moving) {
            return;
        }
        answer
            .calls
            .extend(call(extras.on_pointer_move.as_ref(), None));
    }

    /// Notes whether the pointer is `over` the region, running `on_hover`
    /// into `answer` when that changed.
    fn follow_hover<A: 'static>(
        &mut self,
        extras: &Extras<'_, A>,
        over: bool,
        answer: &mut RegionAnswer<A>,
    ) {
        if self.hovered == over {
            return;
        }
        self.hovered = over;
        answer.calls.extend(call(extras.on_hover.as_ref(), over));
        if extras.lights_on_hover() {
            answer.used = true;
        }
    }
}

impl Widget for RegionBox {
    type Output = ();
    type Response = ();

    /// Never called: the store hands the region its element's properties
    /// through [`RegionBox::handle`].
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) {}

    /// Draws nothing: the store draws the element's surface.
    fn draw(&mut self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }

    fn about_to_wait(&mut self, _cx: &mut EventCx<'_>) -> Option<Instant> {
        None
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The id the store registers the region of the element `id` under.
pub(crate) fn region_id(id: FocusId) -> FocusId {
    id.child(REGION_NODE)
}

/// Describes a pressable region at `rect` to screen readers as a button
/// called `name`, under `id`.
pub(crate) fn describe_button(frame: &mut Frame, id: FocusId, rect: Rect, name: &str) {
    let mut node = AccessNode::new(Role::Button);
    node.set_label(name);
    node.add_action(Action::Click);
    node.add_action(Action::Focus);
    frame.access(id, rect, node);
}

/// `point` in the window as a point from `origin`.
fn local(point: Point, origin: Point) -> Point {
    Point::new(point.x - origin.x, point.y - origin.y)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;

    use accesskit::Role;

    use super::*;
    use crate::app::access::AccessAction;
    use crate::base::theme::ThemeToken;
    use crate::input::event::{ButtonInput, FileInput, WheelDelta};
    use crate::render::layout::Justify;
    use crate::testing::{TreeWindow, access_tree};
    use crate::view::controls::button;
    use crate::view::element::{column, row, text};
    use crate::view::props::{Extent, Space};
    use crate::{App, AppCx, Element, Key, WindowId};

    /// An app whose tree each test picks, recording what reaches it.
    struct Probe {
        /// How many times its tree was built.
        builds: Cell<usize>,
        /// The files dropped on its drop zone.
        files: Vec<(PathBuf, Option<Point>)>,
        /// Each hover change of its card.
        hovers: Vec<bool>,
        /// The keys no element took.
        keys: Vec<Key>,
        /// The items of its left list.
        left: Vec<String>,
        /// Where the pointer was over its card, as each move reported.
        moves: Vec<Option<Point>>,
        /// The presses its card took.
        presses: Vec<Press>,
        /// The items of its right list.
        right: Vec<String>,
        /// The wheel turns its card took.
        scrolls: Vec<WheelInput>,
        /// The files dropped where no element took them.
        unclaimed: Vec<PathBuf>,
        /// The colors it is drawn in.
        theme: Theme,
        /// The test's tree.
        view: fn(&Probe) -> Element<'_, Probe>,
    }

    impl App for Probe {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            self.builds.set(self.builds.get() + 1);
            (self.view)(self)
        }

        fn key(&mut self, key: Key, _cx: &mut AppCx<'_>) -> bool {
            self.keys.push(key);
            true
        }

        fn file_dropped(&mut self, _window: WindowId, file: FileDropped, _cx: &mut AppCx<'_>) {
            self.unclaimed.push(file.path);
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window 400 by 300 pixels showing the tree `view`.
    fn window(view: fn(&Probe) -> Element<'_, Probe>) -> TreeWindow<Probe> {
        let probe = Probe {
            builds: Cell::new(0),
            files: Vec::new(),
            hovers: Vec::new(),
            keys: Vec::new(),
            left: vec!["apple".to_owned(), "pear".to_owned()],
            moves: Vec::new(),
            presses: Vec::new(),
            right: Vec::new(),
            scrolls: Vec::new(),
            unclaimed: Vec::new(),
            theme: Theme::dark(),
            view,
        };
        TreeWindow::new(probe, "t", 400, 300)
    }

    /// Where the element with `role` and `name` was described.
    fn bounds(window: &TreeWindow<Probe>, role: Role, name: &str) -> Rect {
        let tree = access_tree(window.frame(), "t");
        let (_, node) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == role && node.label() == Some(name))
            .unwrap_or_else(|| panic!("no {role:?} {name:?}"));
        let rect = node.bounds().expect("described with bounds");
        Rect::new(
            rect.x0 as f32,
            rect.y0 as f32,
            (rect.x1 - rect.x0) as f32,
            (rect.y1 - rect.y0) as f32,
        )
    }

    /// A text pressable as a button called `name`, its region the room
    /// the layout gives it (a button draws itself only as wide as its
    /// label).
    fn pad(name: &'static str) -> Element<'static, Probe> {
        Element::from(text(name).monospace())
            .on_press(|_: &mut Probe, _| {})
            .access_name(name)
    }

    /// A card that records presses, hovers, and wheel turns, beside a
    /// button.
    fn card(_probe: &Probe) -> Element<'_, Probe> {
        column([
            Element::from(text("Card").monospace())
                .height(Extent::Cells(3.0))
                .background(ThemeToken::Surface)
                .hover_background(ThemeToken::Hover)
                .border(ThemeToken::Border)
                .on_press(|probe: &mut Probe, press| probe.presses.push(press))
                .on_hover(|probe: &mut Probe, over| probe.hovers.push(over))
                .on_scroll(|probe: &mut Probe, wheel| probe.scrolls.push(wheel))
                .access_name("Open card"),
            button("Other").on_click(|_: &mut Probe| {}).into(),
        ])
    }

    /// Two lists of names, each dragged to the other, and a drop zone for
    /// files.
    fn lists(probe: &Probe) -> Element<'_, Probe> {
        fn side(items: &[String], to_right: bool) -> Vec<Element<'_, Probe>> {
            items
                .iter()
                .map(|item| {
                    Element::from(text(item.as_str()).monospace())
                        .id(item.clone())
                        .drag_source((item.clone(), to_right), item.as_str())
                        .access_name(item.as_str())
                        .on_press(|_: &mut Probe, _| {})
                })
                .collect()
        }
        row([
            column(side(&probe.left, true))
                .grow(1.0)
                .height(Extent::Fraction(1.0))
                .drop_target(
                    |(_, to_right): &(String, bool)| !*to_right,
                    |probe: &mut Probe, (item, _), _| {
                        probe.right.retain(|kept| *kept != item);
                        probe.left.push(item);
                    },
                ),
            column(side(&probe.right, false))
                .id("right")
                .grow(1.0)
                .height(Extent::Fraction(1.0))
                .drop_target(
                    |(_, to_right): &(String, bool)| *to_right,
                    |probe: &mut Probe, (item, _), _| {
                        probe.left.retain(|kept| *kept != item);
                        probe.right.push(item);
                    },
                )
                .on_file_drop(|probe: &mut Probe, file| {
                    probe.files.push((file.path, file.position));
                }),
        ])
    }

    #[test]
    fn test_sizes_in_cells_pixels_and_fractions_and_growing_share_a_row() {
        fn view(_probe: &Probe) -> Element<'_, Probe> {
            row([
                pad("Px").width(Extent::Px(60.0)),
                pad("Grow").grow(1.0),
                pad("Half").width(Extent::Fraction(0.25)),
            ])
            .gap(Space::Px(10.0))
            .justify(Justify::Start)
        }
        let window = window(view);
        let px = bounds(&window, Role::Button, "Px");
        let grow = bounds(&window, Role::Button, "Grow");
        let half = bounds(&window, Role::Button, "Half");
        assert_eq!(px.width, 60.0);
        assert_eq!(half.width, 100.0);
        assert_eq!(grow.x, px.x + px.width + 10.0);
        assert_eq!(grow.width, 400.0 - 60.0 - 100.0 - 20.0);
    }

    #[test]
    fn test_min_and_max_hold_a_growing_element_and_cells_follow_the_font() {
        fn view(_probe: &Probe) -> Element<'_, Probe> {
            row([
                pad("Max").grow(1.0).max_width(Extent::Cells(10.0)),
                pad("Min")
                    .width(Extent::Px(1.0))
                    .min_width(Extent::Px(50.0))
                    .shrink(0.0),
            ])
        }
        let window = window(view);
        let metrics = window.frame().cell_metrics();
        assert_eq!(
            bounds(&window, Role::Button, "Max").width,
            10.0 * metrics.width
        );
        assert_eq!(bounds(&window, Role::Button, "Min").width, 50.0);
    }

    #[test]
    fn test_a_surface_is_filled_from_the_theme_and_lit_while_hovered() {
        let mut window = window(card);
        let theme = Theme::dark();
        let rect = bounds(&window, Role::Button, "Open card");
        let fill_of = |window: &TreeWindow<Probe>| {
            window
                .frame()
                .draw
                .fills()
                .iter()
                .find(|quad| quad.rect == rect)
                .map(|quad| (quad.color, quad.radius, quad.border.is_some()))
        };
        let radius = f32::from(theme.shape.corner_radius);
        let surface = theme.color(ThemeToken::Surface).into();
        assert_eq!(fill_of(&window), Some((surface, radius, true)));
        window.send(&Input::PointerMoved(Point::new(rect.x + 5.0, rect.y + 5.0)));
        let hover = theme.color(ThemeToken::Hover).into();
        assert_eq!(fill_of(&window), Some((hover, radius, true)));
        assert_eq!(window.app().hovers, [true]);
        window.send(&Input::PointerMoved(Point::new(390.0, 290.0)));
        assert_eq!(fill_of(&window), Some((surface, radius, true)));
        assert_eq!(window.app().hovers, [true, false]);
    }

    /// A pad that lights by default beside one that asked to stay unlit.
    fn pads(_probe: &Probe) -> Element<'_, Probe> {
        column([pad("Lit"), pad("Plain").unlit()])
    }

    #[test]
    fn test_anything_pressable_lights_under_the_pointer_unless_it_asked_not_to() {
        let mut window = window(pads);
        let hover = Theme::dark().color(ThemeToken::Hover).into();
        let lit_at = |window: &mut TreeWindow<Probe>, name: &str| {
            let rect = bounds(window, Role::Button, name);
            window.send(&Input::PointerMoved(Point::new(rect.x + 5.0, rect.y + 5.0)));
            window
                .frame()
                .draw
                .fills()
                .iter()
                .any(|quad| quad.rect == rect && quad.color == hover)
        };
        assert!(
            lit_at(&mut window, "Lit"),
            "the theme's hover light, asked for by no one"
        );
        assert!(!lit_at(&mut window, "Plain"), "it asked to stay unlit");
        assert!(window.unlit_controls().is_empty(), "and that is no fault");
    }

    /// A selected row outlined under the pointer, as a tree's rows are.
    fn outlined(_probe: &Probe) -> Element<'_, Probe> {
        column([pad("Row")
            .background(ThemeToken::SelectedRow)
            .hover_border(ThemeToken::Border)])
    }

    #[test]
    fn test_an_outlined_row_keeps_its_own_fill_under_the_pointer() {
        let mut window = window(outlined);
        let theme = Theme::dark();
        let rect = bounds(&window, Role::Button, "Row");
        let look = |window: &TreeWindow<Probe>| {
            window
                .frame()
                .draw
                .fills()
                .iter()
                .find(|quad| quad.rect == rect)
                .map(|quad| (quad.color, quad.border.map(|border| border.color)))
        };
        let selected = theme.color(ThemeToken::SelectedRow).into();
        assert_eq!(look(&window), Some((selected, None)));
        window.send(&Input::PointerMoved(Point::new(rect.x + 5.0, rect.y + 5.0)));
        let border = theme.color(ThemeToken::Border).into();
        assert_eq!(
            look(&window),
            Some((selected, Some(border))),
            "outlined, still selected"
        );
        assert!(window.unlit_controls().is_empty());
    }

    #[test]
    fn test_a_press_reports_its_point_clicks_and_button_and_the_keyboard_presses_too() {
        let mut window = window(card);
        let rect = bounds(&window, Role::Button, "Open card");
        let at = Point::new(rect.x + 7.0, rect.y + 3.0);
        // Sent at one instant, so the second press makes a double click
        // however long a loaded debug run takes to draw.
        let now = Instant::now();
        window.send_at(&Input::PointerMoved(at), now);
        for pressed in [true, false, true, false] {
            let button = match pressed {
                true => ButtonInput::press(PointerButton::Left, at),
                false => ButtonInput::release(PointerButton::Left, at),
            };
            window.send_at(&Input::PointerButton(button), now);
        }
        let presses = &window.app().presses;
        assert_eq!(presses.len(), 2);
        assert_eq!(presses[0].point, Point::new(7.0, 3.0));
        assert_eq!((presses[0].clicks, presses[1].clicks), (1, 2));
        assert!(!presses[0].by_key);
        // The card is a tab stop before the button, pressed by Enter and
        // Space; other keys go on to the app.
        window.press("<cr> <space> x");
        let presses = &window.app().presses;
        assert_eq!(presses.len(), 4);
        assert!(presses[2].by_key && presses[3].by_key);
        assert_eq!(window.app().keys.len(), 1);
        let id = window.find(Role::Button, "Open card");
        window.act(AccessAction::Click(id));
        assert_eq!(window.app().presses.len(), 5);
        window.send(&Input::Wheel(crate::input::event::WheelInput {
            delta: WheelDelta::Lines(Point::new(0.0, -1.0)),
            mods: Mods::NONE,
            position: at,
        }));
        assert_eq!(window.app().scrolls[0].position, Point::new(7.0, 3.0));
    }

    #[test]
    fn test_moves_over_an_element_report_where_and_leaving_once_with_no_extra_rebuild() {
        fn view(_probe: &Probe) -> Element<'_, Probe> {
            column([
                Element::from(text("Card").monospace())
                    .height(Extent::Cells(3.0))
                    .on_pointer_move(|probe: &mut Probe, at| probe.moves.push(at)),
                Element::from(text("Plain").monospace()).on_hover(|_: &mut Probe, _| {}),
            ])
        }
        let mut window = window(view);
        // The card is three rows tall at the top, the plain text a row
        // under it.
        let row = window.frame().cell_metrics().height;
        let card = Rect::new(0.0, 0.0, 400.0, 3.0 * row);
        let plain = Rect::new(0.0, 3.0 * row, 400.0, row);
        let builds = |window: &TreeWindow<Probe>| window.app().builds.get();
        // A move over an element with no move callback costs what a move
        // over one with it does.
        let before = builds(&window);
        window.send(&Input::PointerMoved(Point::new(
            plain.x + 1.0,
            plain.y + 1.0,
        )));
        let per_move = builds(&window) - before;
        let before = builds(&window);
        for step in [2.0, 5.0] {
            window.send(&Input::PointerMoved(Point::new(
                card.x + step,
                card.y + 1.0,
            )));
        }
        assert_eq!(builds(&window) - before, 2 * per_move);
        // Leaving for another element, and then the window, reports one
        // leave.
        window.send(&Input::PointerMoved(Point::new(
            plain.x + 1.0,
            plain.y + 1.0,
        )));
        window.send(&Input::PointerLeft);
        assert_eq!(
            window.app().moves,
            [Some(Point::new(2.0, 1.0)), Some(Point::new(5.0, 1.0)), None]
        );
        window.send(&Input::PointerMoved(Point::new(card.x + 3.0, card.y + 2.0)));
        window.send(&Input::PointerLeft);
        assert_eq!(window.app().moves[3..], [Some(Point::new(3.0, 2.0)), None]);
    }

    #[test]
    fn test_a_pointer_press_is_no_tab_stop_and_takes_no_keys() {
        fn view(_probe: &Probe) -> Element<'_, Probe> {
            column([
                Element::from(text("Pages").monospace())
                    .height(Extent::Cells(3.0))
                    .on_pointer_press(|probe: &mut Probe, press| probe.presses.push(press))
                    .access_name("Pages"),
                button("Zoom")
                    .on_click(|probe: &mut Probe| probe.keys.clear())
                    .into(),
            ])
        }
        let mut window = window(view);
        assert!(
            !window.ui_text().contains("Pages"),
            "not described as a button"
        );
        // The keyboard starts on the button, the only tab stop.
        window.press("x <tab> y");
        assert!(window.ui_text().contains("Button \"Zoom\" focused"));
        assert!(window.app().presses.is_empty());
        window.click_at(Point::new(10.0, 5.0), PointerButton::Right);
        let presses = &window.app().presses;
        assert_eq!(presses.len(), 1);
        assert_eq!(presses[0].button, PointerButton::Right);
        assert!(!presses[0].by_key);
    }

    #[test]
    fn test_a_press_inside_a_control_stays_the_controls() {
        fn view(_probe: &Probe) -> Element<'_, Probe> {
            column([button("Inner").on_click(|_: &mut Probe| {}).into()])
                .on_press(|probe: &mut Probe, press| probe.presses.push(press))
                .access_name("Outer")
        }
        let mut window = window(view);
        window.click(Role::Button, "Inner");
        assert!(window.app().presses.is_empty());
        let outer = bounds(&window, Role::Button, "Outer");
        window.click_at(
            Point::new(outer.x + 2.0, outer.y + outer.height - 2.0),
            PointerButton::Left,
        );
        assert_eq!(window.app().presses.len(), 1);
    }

    #[test]
    fn test_a_row_dragged_to_the_other_list_moves_there_and_esc_cancels() {
        let mut window = window(lists);
        let apple = window.center(Role::Button, "apple");
        let to = Point::new(300.0, 150.0);
        window.drag(apple, to);
        assert_eq!(window.app().left, ["pear"]);
        assert_eq!(window.app().right, ["apple"]);
        // Pear moved up into apple's place.
        let pear = window.center(Role::Button, "pear");
        // A drop on the list it came from is not taken.
        let back = window.center(Role::Button, "apple");
        window.drag(back, Point::new(back.x + 20.0, back.y + 40.0));
        assert_eq!(window.app().right, ["apple"]);
        // Esc mid-drag cancels it, the key not reaching the app.
        window.send(&Input::PointerMoved(pear));
        window.send(&Input::PointerButton(ButtonInput::press(
            PointerButton::Left,
            pear,
        )));
        window.send(&Input::PointerMoved(Point::new(250.0, 100.0)));
        window.send(&Input::PointerMoved(to));
        window.press("<esc>");
        window.send(&Input::PointerButton(ButtonInput::release(
            PointerButton::Left,
            to,
        )));
        assert_eq!(window.app().left, ["pear"]);
        assert!(window.app().keys.is_empty());
    }

    #[test]
    fn test_a_file_dropped_reaches_the_element_under_the_pointer() {
        let mut window = window(lists);
        let file = |x: f32| FileInput {
            path: PathBuf::from("/tmp/book.epub"),
            position: Some(Point::new(x, 40.0)),
        };
        window.send(&Input::FileHovered(file(300.0)));
        window.send(&Input::FileDropped(file(300.0)));
        let files = &window.app().files;
        assert_eq!(files.len(), 1);
        // From the right list's top left: it starts past the middle.
        let point = files[0].1.expect("dropped with a position");
        assert_eq!(point.y, 40.0);
        assert!(0.0 < point.x && point.x < 100.0, "{point:?}");
        // A drop where no element takes files goes on to the app.
        window.send(&Input::FileDropped(file(50.0)));
        assert_eq!(window.app().files.len(), 1);
        assert_eq!(window.app().unclaimed, [PathBuf::from("/tmp/book.epub")]);
    }
}

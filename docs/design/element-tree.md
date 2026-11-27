# Element Tree Design

A record of how misty's element tree was designed, kept for contributors; to use misty, read [getting-started.md](../getting-started.md) and the API docs.

Status: approved; phases 1 (one control surface), 2 (tree core), 3 (overlays), 4 (lent data widgets), and 5 (chrome and settings) done, phase 6 (documentation) next. The owner's decisions are in section 8; where phases 2 and 3 had to depart from the design, section 9 says how and why, section 10 does for phase 4, and section 11 for phase 5.

This document proposes how misty moves from "the app drives every widget by hand each frame" to "the app describes its window as a tree of elements, and misty drives the widgets". It recommends one design, sketches its API, rewrites the `controls` example in it, and lays out a migration in phases. The owner's decisions on the questions it raised are collected at the end.

## 1. The Problem

The `controls` example (`examples/controls.rs`, 543 lines) shows nine controls and one status line. Most of its code is glue the app writes only because misty has no tree:

- **Per-control frame calls** in `render` (about 130 lines): each control is synced with the focus (`FocusTree::sync`), measured (`height`), laid out in a row, drawn, and described to screen readers (`access`).
- **Routing each input** to the control under a `FocusId` and turning its response into a change, in `Controls::handle`, `toggled`, and `font`.
- **Routing screen reader actions** to every control in turn, in `Application::access_action` and `Controls::access_action`.
- **A `Change` enum** restating each control's response, and `Demo::settle` applying it.
- **Eleven `FocusId` constants** naming what a tree could name.
- **A label column laid out by hand**, row by row, in the `row` closure.

The select's open list needs a second id (`FONT_LIST`), a hand-written `set_focused` covering both ids, a focus trap opened and closed from `settle`, and a draw moved to the end of `render` so the list floats over the rows below it. Every widget hit tests against the rectangle it was drawn in last frame, so the app must draw before it can dispatch, and must remember to.

Each of these is a place an AI agent writing an app can get wrong without the compiler noticing:

1. Forgetting to sync a control with the focus draws it unfocused, and it then ignores keys.

2. Forgetting `access` hides a control from screen readers. Nothing fails.

3. Registering in the wrong order breaks Tab order and pointer stacking.

4. Forgetting to close a trap leaves the window's keyboard stuck inside a list that is no longer drawn. `FocusTree` recovers only because it drops traps that are not registered.

5. Giving the slider `with_label("Volume")` and drawing "Volume" in the label column states the name twice, and the two can drift apart.

The tree should make each of these impossible, or make the compiler or a clear runtime message catch it.

## 2. Constraints

Beyond the rules in `AGENTS.md`, the tree must keep what the code already promises:

- **The `Widget` contract stays.** Every widget keeps `handle`, `draw`, `about_to_wait`, and `set_focused`. The tree is built on top of it, so a widget works with or without a tree, and its tests keep passing.
- **Data the app owns is lent, not copied.** `List`, `Table`, `TreeView`, and `IconGrid` take the app's rows through `Lent` for one call. The tree must lend data the same way.
- **Unused keys go back to the app unchanged**, as `FocusTree::dispatch` returns `Route::Unhandled` today.
- **The same accessibility output**, so the existing `ui_text` snapshots stay comparable.
- **One layout engine:** flexbox through taffy (`misty::layout`), not a second one.

## 3. Recommendation

Adopt **a view function rebuilt every frame, with widget state kept by key**. The app writes `fn view(&self) -> Element<'_, Self>`; misty rebuilds the tree from it each frame and on each input, lays it out, draws it, routes input through it, and applies the changes the app's callbacks make to its state. The stateful widget objects that exist today (`TextField`, `Select`, `ScrollView`) live in a store inside misty, keyed by each element's id, and survive every rebuild.

### Why not the other two

**Immediate mode with ids** (egui style: `if ui.checkbox(&mut wrap, "Wrap").changed() { .. }`) is the closest to today's code, and it would remove less of it. It lays a widget out as it draws it, so a label column or a dialog sized to its content needs a second pass or last frame's sizes. That is the "hit test against last frame's rectangles" problem again. Keys, traps, and accessibility still need an app-written id at each call site, so the `FocusId` constants and the trap bookkeeping stay.

**Signals** (Leptos or Floem style) fight Rust's borrow checker: state lives in `Rc<RefCell<..>>` or arena handles, effects run at times the reader cannot see, and a test must set up a reactive runtime before it can press a key. An agent reading a signals app cannot tell from one function what a click changes. Signals also make it hard to keep `Lent`: a list's rows would have to move into a signal.

**A rebuilt view** keeps the data flow on one page, `view` reading `&self` and callbacks writing `&mut self`, which an agent can follow and a test can drive. Misty already redraws whole frames, so there is no retained render tree to diff: the "diff" is only "which ids are new, which are gone", done in the state store. That keeps the design much smaller than iced's or Xilem's.

### How it fits the constraints

- **Borrow checker:** `view(&self)` borrows the app immutably; callbacks are `Fn(&mut App, Value)` stored in the tree. Misty collects the calls they produce, drops the tree, then runs them on `&mut App`. No `RefCell`, no `Rc`.
- **Widget state:** the store owns one widget object per id, so the caret, scroll offset, open list, and drag state live where they live today. Controls are controlled only: every value comes from the app's state, and the store keeps only view state (caret, scroll, open list, drag). The element carries the props the app owns (options, value, enabled), synced into the stored widget each rebuild.
- **`Lent`:** `Element<'a, App>` borrows `&'a App`, so a list element holds `&'a [Row]` exactly as `Lent` does.
- **Keys:** unhandled keys come back to a new `App::key` hook, the place the app's keymap runs.

## 4. Proposed API

### 4.1 The app

```rust
/// An app described by a view: misty builds, lays out, draws, and
/// dispatches its tree.
pub trait App: Sized + 'static {
    /// The contents of `window`, built from the app's state: one app,
    /// many windows.
    fn view(&self, window: WindowId) -> Element<'_, Self>;

    /// A key no element took: the app's keymap. Return whether it was used.
    fn key(&mut self, _key: Key, _cx: &mut AppCx<'_>) -> bool {
        false
    }

    /// The user's appearance settings, at start and on each change.
    fn follow_appearance(&mut self, _appearance: Appearance) {}

    /// The theme the window is drawn in.
    fn theme(&self) -> &Theme;
}

pub fn run_app(app: impl App, options: WindowOptions) -> Result<(), Error>;
```

`App` is built on the existing `Application`: misty implements `Application` for an internal `TreeRunner<A: App>`, so the event loop, windows, and frames are unchanged. `view` takes the `WindowId` it describes, so one app keeps every window's state, as `Application` allows today. `App` is the one obvious way to build an app; once the tree covers every widget (phases 5 and 6), `Application` moves to a `misty::low_level` namespace as the escape hatch.

### 4.2 Elements and callbacks

```rust
/// One node of a window's tree: a control, a container, or text.
pub struct Element<'a, A> { /* kind, id, style, children, callbacks */ }

// Controls, each built from a free function and `with_` methods.
pub fn checkbox<'a, A>(label: &'a str, checked: bool) -> Checkbox<'a, A>;
pub fn select<'a, A, T: Clone + PartialEq>(
    options: &'a [(T, &'a str)],
    chosen: &T,
) -> Select<'a, A, T>;
pub fn slider<'a, A>(range: NumberRange, value: f64) -> Slider<'a, A>;
pub fn text_area<'a, A>(text: &'a str, version: u64) -> TextArea<'a, A>;
pub fn list<'a, A, S: ListSource>(source: &'a S) -> List<'a, A, S>;

// Containers.
pub fn column<'a, A>(
    children: impl IntoIterator<Item = Element<'a, A>>,
) -> Element<'a, A>;
pub fn row<'a, A>(children: impl IntoIterator<Item = Element<'a, A>>) -> Element<'a, A>;
pub fn form<'a, A>(fields: impl IntoIterator<Item = Field<'a, A>>) -> Element<'a, A>;
pub fn field<'a, A>(label: &'a str, control: impl Into<Element<'a, A>>) -> Field<'a, A>;

impl<A> Number<A> {
    /// What runs on the app's state when the user moves the slider.
    pub fn on_change(self, apply: impl Fn(&mut A, f64) + 'static) -> Self;
}
```

A control is **controlled**, with no uncontrolled mode: its value comes from the app's state in `view`, and the only way it changes is through its `on_change`, a closure on `&mut App` (no message enum). That removes the `Change` enum and `settle`: the callback is the settle. A control with no `on_change` is read-only and says so to screen readers.

`text_area` takes the text and a version counter the app supplies, changed whenever the text changes: a rebuild compares the number, and the whole string only when the number moved.

### 4.3 Ids and the state store

Each element has an id. By default it is the element's path in the tree (`form/2/control`), which is stable as long as the tree's shape is. A control whose position can change (a row in a filtered list, a tab) gets an explicit id with `.id("volume")`; ids reuse `FocusId`'s hashing.

The store keeps, per id, the widget object (`misty::Slider`, `misty::Select<T>`, and so on), its last rectangle, and its accessibility subtree id. After each rebuild, entries whose id was not seen are dropped, so a closed dialog forgets its focus and a removed row its caret.

Two elements built with the same explicit id panic in debug builds with a message that says how to fix it:

```text
misty: two elements have the id "volume" (column/0/form/3/slider and column/0/form/5/slider).
Give one of them another id with `.id(..)`, or build it once.
```

A stateful control inside a dynamic container (a list, a conditional) without an explicit id panics the same way in debug builds, naming the container, since a path id there would shift and lose the widget's state. As built, a container is dynamic when its number of children differs from the last rebuild's, and every control inside it whose id is a path (through containers without ids of their own) is flagged, since misty cannot tell which child moved:

```text
misty: the column at column went from 1 to 2 elements, and the checkbox at column/0/checkbox inside it has no id of its own, so it may take the state of the element that was in its place.
Give it an id with `.id(..)` naming what it shows.
```

### 4.4 One input, step by step

1. An input arrives: a key, a pointer event, or a screen reader action.
2. `view(&app)` builds the tree.
3. The store syncs each element's props into its id's widget, creating widgets for new ids and dropping those for missing ids.
4. `FocusTree` dispatches the input against the rectangles the last layout stored, and the widget answers.
5. A changed value queues its `on_change` call; an unused key goes to `App::key`.
6. The tree is dropped, the queued calls run on `&mut app`, and the window redraws: rebuild, lay out, draw, describe.

Rebuilding on every input and every frame is affordable because elements are small borrowed structs; the heavy state stays in the store, and big data stays lent. The benchmark harness (`make bench`) gets a tree case to keep this honest.

### 4.5 Layout

Each element carries a `layout::Style` and becomes one taffy node. Every control reports its size through one trait, which replaces each widget's own `height(metrics, scale_factor)` and adds the width a row needs. As built in phase 1 (`src/app/control.rs`):

```rust
/// A form control: a widget the app measures, hands its state each
/// frame, and reads the values its responses report.
pub trait Control: Widget {
    /// What the app's state says the control shows.
    type Props<'a>
    where
        Self: 'a;
    /// What a change reports to `on_change`.
    type Value;

    /// Copies `props` into the widget, keeping its own state (caret, scroll).
    fn sync(&mut self, props: Self::Props<'_>);
    /// Its height for `width` (`f32::INFINITY`: any width), in the frame's
    /// metrics.
    fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32;
    /// The value `response` reports as changed, or `None`.
    fn changed(&self, response: Self::Response) -> Option<Self::Value>;
}
```

`changed` is the step that turns a widget's response into the element's `on_change` call; it takes `&self` so a text control reports its text, which its response does not carry. Each control's props are a small struct next to it:

| Control | `Props` | `Value` |
|---|---|---|
| `Button` | `ButtonProps { active, enabled, label }` | `()`: clicked or flipped |
| `Checkbox` | `CheckboxProps { checked, enabled, label }` | `bool` |
| `RadioGroup<T>` | `RadioGroupProps { chosen, enabled, label, options }` | `T` |
| `Select<T>` | `SelectProps { chosen, enabled, label, options }` | `T` |
| `Slider` | `NumberProps { enabled, label, range, value }` | `f64` |
| `Stepper` | `NumberProps { enabled, label, range, value }` | `f64` |
| `ProgressBar` | `ProgressProps { fraction, label }` | `Infallible` |
| `TextField` | `TextFieldProps { label, placeholder, text }` | `String` |
| `TextArea` | `TextAreaProps { label, placeholder, text, version }` | `String` |

A text area measures the rows its text wraps into at the width, so an area grows with its text; a tree holding it to a few rows (`.rows(6)`) makes it that tall and lets it scroll.

`form` lays out a two-column grid, the label column as wide as its widest label. The label is drawn by the form and also becomes the control's accessible name, so `with_label` disappears from the tree API (the widget keeps it for apps without a tree).

### 4.6 Focus, accessibility, and animation

The tree registers every focusable element in `FocusTree` in tree order, so Tab order is reading order; `.order(n)` maps to `FocusTree::set_order` for the rare exception. Containers map to `FocusTree::enter` and `leave`, so keys bubble up the tree. The store syncs each widget's focus, so no app code calls `sync` or `set_focused`. The ring is drawn by `FocusTree::draw` after the tree, as now.

After drawing, the store calls each control's existing `access(frame, id)`. A screen reader action names a node id; the store finds the element that owns it (the id or a child of it, as `FocusId::child` already encodes), calls that widget's `access_action`, and turns a change into the element's `on_change` call. Tree apps write no `access_action`.

The store also calls every stored widget's `Widget::about_to_wait` and returns the earliest instant, so carets, glides, and progress sweeps animate without the app forwarding them.

### 4.7 Overlays

```rust
impl<'a, A> Element<'a, A> {
    /// Draws `layer` over everything at the end of the frame, anchored to
    /// this element; `modal` traps the focus inside it while it shows.
    pub fn overlay(self, layer: Overlay<'a, A>) -> Self;
}
```

The select's open list, a menu, a tooltip, and toasts are overlays the control declares itself, from its stored state: the `select` adapter adds a modal overlay while its widget `is_open()`. A modal overlay opens a trap the frame it first appears and closes it the frame it disappears. A dialog is an overlay the app declares from its own state:

```rust
column(..).overlay_if(self.confirming, dialog("Unsaved changes", body)
    .button("Discard", |app: &mut Self| app.close(false))
    .button("Save", |app: &mut Self| app.close(true)))
```

Overlays draw in paint order after the base layer, so "draw the select last" is no longer the app's job.

### 4.8 Fitting into misty

- **Existing widgets** keep their files and `Widget` impls, and each gains an `impl Control` and one builder. A widget without an adapter yet goes in a tree through `custom(id, make, respond)`, which lays it out and focuses it while the app handles its responses, so an app can move to `App` before every widget has an adapter, and a tree app embeds hand-driven widgets without dropping to `misty::low_level::Application`.
- **Testing:** a `testing::TreeWindow`, built on `TestWindow`, owns the app, finds elements by role and accessible name (panicking with the names it did find when there is no match), sends keys and clicks, and reads `ui_text`, with no display.
- **Layers:** the `Control` trait goes in `app`; its impls sit next to each widget in `widgets` and `chrome`; `App`, `AppCx`, `Element`, the builders, and the store go in a new `view` layer between `chrome` and `runtime`, free of winit; `run_app` goes in `runtime`, and `TreeWindow` in `testing` (see section 9).

## 5. The Controls Example in the Proposed API

```rust
//! The form controls in one window (see the module doc of the current
//! example for the keys).

use misty::view::{
    checkbox, column, field, form, progress, radio, select, slider, stepper, text,
    text_area,
};
use misty::{App, ChromeText, Element, NumberRange, Theme, WindowId, WindowOptions};

const VOLUME: NumberRange = NumberRange::new(0.0, 100.0, 5.0);
const TAB_WIDTH: NumberRange = NumberRange::new(1.0, 8.0, 1.0);
const FONTS: [(ChromeText, &str); 2] = [
    (ChromeText::Cells, "Monospace cells"),
    (ChromeText::Proportional, "Proportional"),
];
const THEMES: [(bool, &str); 2] = [(true, "Dark"), (false, "Light")];

struct Demo {
    autosave: bool,
    notes: String,
    notes_version: u64,
    status: String,
    tab_width: f64,
    theme: Theme,
    volume: f64,
    wrap: bool,
}

impl App for Demo {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        let dark = self.theme.background == Theme::dark().background;
        column([
            form([
                field("Editor", checkbox("Wrap long lines", self.wrap)
                    .on_change(|app: &mut Self, on| {
                        app.set_status("wrap", on, |a| a.wrap = on)
                    })),
                field("", checkbox("Autosave", self.autosave).switch()
                    .on_change(|app: &mut Self, on| app.autosave = on)),
                field("Theme", radio(&THEMES, &dark)
                    .on_change(|app: &mut Self, dark| app.set_theme(dark))),
                field("Chrome font", select(&FONTS, &self.theme.chrome_text)
                    .on_change(|app: &mut Self, font| app.theme.chrome_text = font)),
                field("Volume", slider(VOLUME, self.volume)
                    .on_change(|app: &mut Self, volume| app.volume = volume)),
                field("Level", progress(Some((self.volume / VOLUME.max) as f32))),
                field("Tab width", stepper(TAB_WIDTH, self.tab_width)
                    .on_change(|app: &mut Self, width| app.tab_width = width)),
                field("Indexing", progress(None)),
                field("Notes", text_area(&self.notes, self.notes_version).rows(6)
                    .on_change(|app: &mut Self, notes| {
                        app.notes = notes;
                        app.notes_version += 1;
                    })),
            ]),
            text(&self.status).muted(),
        ])
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

fn main() -> Result<(), misty::Error> {
    misty::run_app(Demo::default(), WindowOptions::new("misty controls"))
}
```

What went away: the eleven `FocusId` constants, the `Controls` struct, the `Change` enum and `settle`, `input`, `access_action`, `about_to_wait`, the whole of `render`, and the trap calls. Ctrl+Q moves into `App::key`. The example should land near 120 lines, down from 543, and every remaining line is about the app.

## 6. Migration Plan

Each phase ships on its own and leaves the low-level `Application` path working.

1. **One control surface.** Add the `Control` trait in `app` and implement it for the nine form controls (`Button`, `Checkbox`, `RadioGroup`, `Select`, `Slider`, `Stepper`, `ProgressBar`, `TextField`, `TextArea`), with `measure` taking over from `height`. Ships: nothing visible to apps without a tree; the controls' inherent `height` is gone, replaced by `Control::measure`. Verified: `make check`, `make test`; every widget's existing tests untouched.

2. **Tree core.** The `view` layer with `Element`, `column`, `row`, `form`, `field`, `text`, the store, layout through `misty::layout`, automatic focus, accessibility, and animation, `App` and `run_app`, and `TreeWindow`. Ships: the controls example rewritten, without the select. Verified: the rewritten example's `ui_text` matches the old example's output line for line (a snapshot captured before the rewrite), plus `TreeWindow` tests for each interaction in the example.

3. **Overlays.** `overlay`, automatic traps, and adapters for `Select`, `Menu`, `Tooltip`, `Toasts`, and `Dialog`. Ships: the full controls example and the dialog example as trees. Verified: tests that a trap opens and closes with the overlay and that Esc and outside clicks behave as in the widgets' own tests.

4. **Lent data widgets.** Adapters for `List`, `Table`, `TreeView`, `IconGrid`, `QuickPick`, and `ScrollView` holding arbitrary children. Ships: the list, table, tree, and grid examples as trees. Verified: `make bench` shows no regression on the heaviest views; snapshot equality as in phase 2.

5. **Chrome and settings.** `TitleBar`, `TabStrip`, `SplitView`, `StatusBar` as tree elements, and the settings page rebuilt from the page kit (`view::settings_page`). Ships: a whole frameless window described by one `view`. Verified: a frameless chrome example (title bar with its menu, tab strip, context menu, which-key) ported to one `view`.

6. **Documentation.** The README and crate docs lead with `App`; `Application` and its frame-by-frame path move to `misty::low_level`, documented as the escape hatch.

## 7. Risks

- **Rebuild cost.** A tree of thousands of elements rebuilt per input could cost more than today's hand-written frame. Lent data keeps large lists as one element each; the bench harness gates every phase from 2 on.
- **Implicit ids.** A path id changes when an element is inserted before it, and the widget silently loses its state (a caret jumps). The debug-build panic in section 4.3 catches the common case.
- **Generic callbacks.** `Fn(&mut A, V)` boxed per element adds an allocation per callback per rebuild. Acceptable at form sizes; revisit with an arena if the bench says so.

## 8. Decisions

The owner answered the questions this proposal raised; the sections above follow the answers.

1. **Callbacks, not messages.** Controls take closures on `&mut App` (`on_change(|app, v| ..)`); there is no `Msg` enum and no `update`, which would bring back the `Change` enum the example wants gone.

2. **`Application` becomes the escape hatch.** Once the tree covers everything (phases 5 and 6), `Application` moves to a `misty::low_level` namespace; `App` is the one obvious way to build an app, and `custom` embeds hand-driven widgets in a tree.

3. **Controlled only.** Every control value comes from the app's state; misty's store keeps only view state (caret, scroll, open list, drag). There is no uncontrolled mode.

4. **Text area version counter.** `text_area` takes the text plus a version counter the app supplies, so a rebuild compares a number, not the whole string; the text is compared only when the number changed (`TextAreaProps::version`).

5. **One app, many windows.** `App::view` takes a `WindowId`, so one app describes each of its windows.

## 9. Phases 2 and 3 as Built

Phase 2 follows sections 3 to 5 except where the borrow checker or the existing code asked for the closest design that keeps their goals:

1. **Callbacks are `'static`.** The queued calls run on `&mut App` after the tree is dropped, but a callback borrowing `'a` would keep the app borrowed as long as it lives. Callbacks are `Rc<dyn Fn(&mut A, V)>`, cloned into a boxed `FnOnce(&mut A)` when a change is reported; one that needs a value from `view` moves an owned copy in.

2. **`App` and `AppCx` live in `view`, `run_app` in `runtime`.** The input step (rebuild, dispatch, run the calls, hand an unused key to `App::key`) is shared by the runtime's runner and `TreeWindow`, so it sits in the store, which needs the trait. `AppCx` holds the window's `EventCx` and records `exit` and `close_window`, so `TreeWindow` builds one without an event loop.

3. **`TreeWindow` is in `testing`, not `view`.** It builds on `TestWindow`, and a layer may not import the top modules; `src/testing.rs` became `src/testing/mod.rs` with `tree.rs` beside it.

4. **`App::follow_appearance`.** The controls example follows the system's light or dark appearance; the hook, called when the first window opens and on each change, keeps that without an `Application`.

5. **`Element::scroll` and `Element::padding`.** The hand-driven example scrolls its form and keeps cell margins, and its accessibility output starts with that `ScrollView`, so a container can scroll (a stored `ScrollView` entered in the focus tree, revealing the control Tab focuses) and any element takes padding in cell columns and rows. Scrolling containers take the room their parent leaves.

6. **Only scrolling containers enter the focus tree.** Entering every column and row would add each as a focusable group to the accessibility tree and change `ui_text`; a plain container has no widget to take a bubbled key anyway.

7. **Builder names.** `slider` and `stepper` both return `view::Number<A>` (they share their props); `radio` returns `view::Radio<'a, A, T>` and clones `chosen`. `text` returns a `view::Text` (colors with `muted`, `color`, `then`), made an `Element` with `.into()` where an array mixes it with other elements; `Text::proportional` sets it in the proportional font, named as `view::List::proportional` is in the view layer (the widgets' `with_proportional_font`), the store measuring its shaped width with the frame before layout so a row gives it exactly that room, and drawing it as a proportional label cut with an ellipsis. Text is not described to screen readers, as the hand-driven status line was not.

8. **A control without a callback is disabled**, the closest state the widgets have to read-only.

9. **Accessible names.** `RadioGroup`, `TextField`, and `TextArea` gained a `label` (props and `with_label`), so a form label names them. Their `ui_text` lines are the only change from the hand-driven example's snapshot besides the select's line, which waits for phase 3: the controls example's tests compare the tree's output against that snapshot line for line with exactly those edits.

The cost of rebuilding, measured by `make bench` (`bench_tree_form_rebuilt_on_every_frame_and_input`, release, software GL): a form of 9 fields rebuilds, lays out, and draws without the GPU in about 0.09 ms and handles a key in about 0.002 ms; a form of 300 fields takes about 2.3 ms and 0.04 ms; whole frames with the GPU average 0.7 ms and 3.6 ms.

Phase 3 follows section 4.7 the same way, with these departures:

10. **Layers cover the window.** `Element::overlay` takes anything that becomes an `Overlay`: a layer of elements made with `overlay(content)` (`.modal()` to trap), a `dialog`, or `toasts`; `overlay_if(show, layer)` keeps the slot of a hidden layer, so the layers after it keep their ids. Each layer is flattened after the base tree, laid out over the whole window as a root, and drawn in the frame's overlay after the base layer, in the order given. The element a layer is given to names its id (its id with a child index from `1 << 33`) and its lifetime, not its place: a layer anchored beside an element would need the anchor's rectangle before the layer is laid out, and the popups that do open beside their control place themselves (next point).

11. **A control's popup is not an element layer.** The select's list and a menu are drawn by their widgets, which already place them beside their box and draw them in the frame's overlay. A control declares its popup through `Spec::has_layer` (the select's `is_open`), and its `Spec::draw` returns the popup's rectangle beside its own (`Drawn`); the store registers every popup after the base tree and the layers, under the control's id with child `1 << 32`, which is the node the select describes its list as, so the trap and the list are one `ListBox` node for screen readers. `set_focused` tells the control it has the keyboard while either id has it, and the dispatcher maps the popup's id to the control.

12. **Traps follow the ids seen.** Each rebuild collects the trap ids of the modal layers and the open popups, in tree order; it closes the open traps no longer seen (innermost first), which gives the focus back to the element that had it, and opens the new ones (outermost first). A modal layer whose root is a control (a dialog) is the trap itself, so no group node is added; a modal layer of elements is entered as a group over the window and drawn over `draw_backdrop`. While a trap is open a screen reader's actions reach only the elements of the innermost modal layer, or the control whose popup is open, as the hand-driven dialog example did by hand.

13. **The dialog wraps `Dialog` without its content.** `dialog(title, body)` with `button`, `primary`, `on_close` (Esc), and `on_link` keeps a `Dialog<usize>` in the store, made again only when its title, body, or buttons change. `Dialog::with_content` is not wrapped yet: elements inside the dialog would share the focus between the tree's controls and the dialog's own buttons, which the dialog moves itself; a modal `overlay` of elements covers a form in a modal until phase 4 brings scrolling containers of arbitrary children.

14. **The menu is a menu button.** `menu(label, entries)` is a button that opens a `Menu` of `MenuEntry` values under itself, its first item highlighted when a key opened it, and runs `on_choose` with the item's id. Context menus, which open at the pointer, wait for phase 5's chrome.

15. **A tooltip is a property of any element.** `Element::tooltip(help)` rather than an adapter element: the store keeps one `Tooltip` per window, notes where each element with help was drawn (cut to its scrolling container), follows the pointer over them, draws the tooltip over everything, and describes it as a `Tooltip` node while it shows.

16. **The toasts change the app's notifier through calls.** `toasts(&notifier, |app| &mut app.notifier)` draws from the lent notifier; holding a toast under the pointer, dismissing one, and the callbacks `on_open` and `on_cancel` run as queued calls on the notifier the accessor reaches. Expiry needs the app too, so `Store::about_to_wait` takes the app and, only while a ticking element is built, rebuilds the tree to run what `Spec::tick` says is due. The cards are not described to screen readers, as the widget describes none.

17. **`App::should_exit`.** Callbacks reach only the app's state, so a dialog's Quit button cannot call `AppCx::exit`; the store quits after an input or screen reader action once `should_exit` says so. The tree has no hook for the window's close request, which closes the window, so the dialog example's close button no longer asks first.

18. **The label column.** With the chrome font select back, the form's widest label is "Chrome font", and the label column (widest label and a gap of 3 cells) is 14 cells, the hand-driven example's `LABEL_COLS`; the controls example's `ui_text` matches the hand-driven snapshot line for line, the select's line included, but for the two names phase 2 added.

## 10. Phase 4 as Built

Phase 4 follows sections 2, 4, and 6 item 4 with these departures, each the option closest to the principles above where the design left the choice open:

1. **Cursor and selection are the app's.** `list(rows, cursor)`, `table(rows, columns, cursor)`, `tree_view(items, cursor)`, and `icon_grid(items, cursor, thumbnails)` take the cursor from the app's state, and `.selected(..)` the selection; they change only through `on_cursor_move` and `on_select`, as a select's chosen option does (decision 3). A list without `on_cursor_move` keeps the app's cursor, one without `on_select` selects nothing. The alternative, keeping them in the store as view state, would leave an app no way to move the cursor after it sorts or drops rows, which the table and list examples do. What only the view knows stays in the store: the scroll and its glide, the count or prefix typed (reported by `on_keys`, for a status line), drags, rubber bands, a tree's expanded nodes, a table's dragged widths, the thumbnails' cache, and a quick pick's session.

2. **Callbacks are named after the widgets' responses:** `on_activate`, `on_drop` (a list without it cannot drag a row), `on_sort`, `on_resize` (the column and every width), `on_header_menu`, `on_expand`, `on_collapse`, and `on_load`. One input can move the cursor, the selection, and the keys at once, so a list-like element compares its widget's state before and after the input and runs every callback that applies, in that order, then the response's.

3. **`Spec` lends the data.** A lent widget handles, draws, and describes itself only with the app's rows, so `Spec` gained `handle` (by default the widget's own), `describe` and `act` take the element (`&self`), and `grows` says an element takes the height its column leaves, laid out as a scrolling container is (a list inside a scrolling container gets no height; it scrolls itself). The store keeps each lent widget in a small wrapper (`ListBox`, `TableBox`, `TreeBox`, `GridBox`), whose own `Widget::handle` is never called; stepping a table's or a grid's glide, which needs no rows, lends them an empty source.

4. **The tree copies its items when they change.** `TreeView` keeps the items it was given and rebuilds its rows from them, so lending would mean rewriting it. The app keeps a `TreeItems` (roots, loaded children, failures, `reload`), whose every change takes a version no collection had before; the element copies the items into its `TreeView` only when the version moved, as a text area compares its version counter (decision 4). An expanded node the items lack shows a loading row. The expanded nodes stay the tree's; an app that needs them (the tree example's F5) follows `on_expand`, `on_collapse`, and `on_load`.

5. **A table takes the app's columns where they changed.** The app keeps its `TableColumn`s (hiding one with the new `TableColumn::with_hidden`) and the sort it put its rows in (`.sort(..)`); the element takes the columns when they differ from those it took last, keeping a width the user dragged unless the app changed that column's width itself, so a drag is never undone mid-way and `on_resize` reports the widths to save.

6. **The icon grid's thumbnails are view state.** `IconGrid` is lent its `Thumbnails` mutably, which a tree borrowing the app cannot do, so the cache lives in the store, made the first time the element is built by the closure it was given (which takes the app's waker), and misty takes in what its workers load. The zoom stays the grid's. The icons example keeps its hand-driven `Application`: phase 4's grid example is `examples/grid.rs`, the cell grid.

7. **A cells element for the grid example.** `examples/grid.rs` draws a `CellBuffer`, not an `IconGrid`, so `view::cells(source)` lends a `CellSource`, drawn from the element's top left on the cell grid and as tall as its rows; the store blinks its cursor at the theme's caret rate, starting over when it moves or changes shape, which the hand-driven example did by hand on Tab.

8. **Work on other threads reaches the app as callbacks.** The tree and grid examples read folders and count seconds on worker threads, and `App` had no messages. `App::start` hands the app a `Waker<Update<Self>>`, where `Update<A>` is a boxed `FnOnce(&mut A)`: a thread sends a closure, and misty runs it on the app's state and redraws every window, so there is still no message enum (decision 1). `TreeWindow::waker` and `TreeWindow::deliver` do the same in tests. `App::on_panic` carries the grid example's "carried on from" line, and `App::reduced_motion` the list example's `--reduced-motion` flag.

9. **Tab in a window with nothing to focus is the app's.** The grid example has no focusable element and cycles its cursor shape on Tab, which the focus tree used to swallow; with no tab stops, Tab now reaches `App::key`.

10. **The quick pick is a modal layer around a session the view keeps.** `quick_pick(open)` is shown with `overlay_if` from the app's state; `open` makes the `Picker` the first time the layer shows (again after it went, so a closed picker forgets its query), and `on_run` and `on_close` report what the user did. Ticks that hand the app work back (`QuickPickTick`: a source saving its rows, a preview for the side panel) are not wrapped yet, since `Spec::tick` has no widget to take them from; nor is the picker described to screen readers, as the widget describes nothing. A picker matching on a worker is opened with the app's waker.

11. **Scrolling containers of arbitrary children.** `Element::scroll` (section 9 item 5) already held any elements; phase 4 makes it work as a modal layer's content. A modal layer whose root was itself a scrolling container was entered in the focus tree twice under one id, which looped; it is now entered once, as its trap. A form taller than the window scrolls inside a modal layer, Tab revealing each control, which is the way to put a form in a modal until `Dialog::with_content` is wrapped.

12. **Snapshot equality.** Each rewritten example's tests compare its `ui_text` and drawn text, at start and after the interactions it supports, with captures of the hand-driven example made before the rewrite, through `TestApp`. The grid's match exactly. The table's and the tree's match line for line but for one edit: the keyboard is on the element, so the cursor's row is `focused` rather than the window. The hand-driven list example never called `List::access`, so it described only its window; the tree describes the list and its rows in sight (problem 2 of section 1), and draws the same rows, its status line one run of text rather than cells word by word.

The cost of lending, measured by `make bench` (`bench_tree_list_and_table_lent_their_rows`, release, software GL, 1280 by 800): a list of 100,000 rows scrolled a row a frame draws without the GPU in 0.13 ms by hand (drawn and described) and 0.16 ms as a tree element; a table of 100,000 rows in 0.32 ms and 0.33 ms; a key costs the tree under 0.001 ms. Whole frames with the GPU average 3.6 ms for the hand-driven list and 3.3 ms for the tree's, 2.8 ms for the hand-driven table and 2.9 ms for the tree's, within the run-to-run noise.

## 11. Phase 5 as Built

Phase 5 follows section 6 item 5 with these departures, each the option closest to the principles above where the design left the choice open:

1. **The chrome wraps the existing widgets.** `title_bar`, `tab_strip`, `split`, and `status_bar` keep a `TitleBar`, a `TabStrip`, a `SplitView`, and a `StatusBar` in the store, so they draw, hit test, and describe themselves exactly as the hand-driven widgets do; a crate test draws the bar, the tabs, and the status line by hand and through the tree and compares their `ui_text` line for line, the one difference being the focus line (the tree's menu button is a tab stop). No chrome widget file changed.

2. **Menus are the app's state, opened through one `MenuRequest`.** The title bar's menu button and menubar titles, a tab's right click, and `Element::on_context_menu` each report a `MenuRequest` (the anchor to open beside and whether a key asked), and the app shows `context_menu(entries, request)` with `overlay_if` while it keeps the request. The alternative, the bar holding a `Menu` as phase 3's menu button does, would make the title bar generic over the menu's id type even without a menu, and would give the app no way to open the menu from its keymap; one request type serves every menu. The open menu's highlight and submenus stay in the store. While a menubar menu is open the pointer moving onto another title does not switch menus, since the open menu's trap takes the pointer; the app reopens on the next press.

3. **A control may take the pointer over everything (`Drawn::over`).** The title bar's resize border runs along the window's edges, over the content, so the bar registers the area from its row to the window's bottom for the pointer after every other element, under its id with child `1 << 34`; input it declines (anything off its parts) falls through to the elements under it, as the hand-driven apps offered the bar every input first. The bar is a tab stop only when it has a menu button (Enter or Space asks for the menu), registered as the bar's own id, so a screen reader hears the bar focused rather than its Menu button; it is visited after its siblings (`order(1)`), so a window's first tab stop is its content.

4. **The close button closes the window unless the app says otherwise.** Callbacks reach only the app's state, so `Spec::asks_close` lets an element ask the store to close the window, as `Input::CloseRequested` does; the title bar asks when it has no `on_close`.

5. **Tabs are the app's list, synced by key.** `tab_strip(tabs, active)` takes `tab(key, title)` values built from the app's state; the store maps each key to the strip's `TabId`, removes, inserts, moves, retitles, and marks tabs to match on every rebuild, and reports activation, closing, moves (the new index), the new-tab button, and right clicks by key. A strip without `on_activate` or `on_move` snaps back to the app's state. Dragging a tab off the strip (`Dragging`, `DroppedOutside`) and tabs inside the title bar (`TitleBar::with_tabs`) are not wrapped: an app puts a `tab_strip` under the bar. A strip with tabs is a tab stop visited after its siblings (`order(1)`), as the title bar is, so `AppCx::focus` can move a sidebar's keyboard onto it: while it has the keyboard Left and Right move between its tabs, wrapping, outlining the one the keyboard is on unless it is the active tab, and Enter or Space activates that tab as a click does. The keys live in the store's `TabsBox` rather than the hand-driven `TabStrip`, which takes no keys; a tab scrolled out of sight is not scrolled to until it is activated, since the strip scrolls only to its active tab.

6. **A split is a container, not a lent `SplitTree`.** `split(axis, ratio, first, second)` holds two arbitrary elements, nested for more panes, so the tree's nesting is the layout and the app keeps only ratios (`on_resize` on each move of a drag); a `SplitView` over a one-split `SplitTree` draws the divider and its hover and drag. It takes the room its column leaves, as a scrolling container does, lays each child out as a root in its side, and registers its whole area for the pointer after its children, so the divider's grab zone either side of the line wins while a press in a side falls through. Pane focus and `focus_toward` are not wrapped: the focus tree's Tab order and focus ring take their place.

7. **Context menus follow a right click after the element under it.** The store notes where each element with `on_context_menu` was drawn (cut to its scrolling container, and out of reach under a modal layer, as tooltips are) and, after dispatching a right press, asks the innermost one under the pointer. Shift+F10 that no control took asks the innermost one around the focused control, opening under it with its first item highlighted: the platforms' context menu key, a convention like Tab rather than an app command, so misty takes it as it takes Tab. The menu itself is a non-modal layer whose open `Menu` is the control's popup, so it traps the focus under `layer_id` as the select's list does.

8. **Which-key is a layer fed the app's prefix.** `which_key(prefix, entries)` keeps a `WhichKey` popup, told the prefix only when it changed; the store's `about_to_wait` shows it once the prefix has waited. The keys still reach the app's keymap; a click on an entry runs `on_activate` with its index and one outside `on_close`. It describes nothing to screen readers, as the widget describes nothing; neither does the status bar.

9. **The settings page is elements of the page kit.** The `SettingsPage` widget, which edited its own copy of the values and was lent the app's `CommandRegistry`, is gone. `view::settings_page` composes the page kit from `settings_section` (each `setting_row` the control its kind takes), `keybindings_section` (`key_capture` per command), a filter field, and scope tabs, all controlled: the app holds the values and the registry, and hears each edit as a `SettingChange` or a `KeyChange`, applied with `CommandRegistry::edit_keys`.

10. **No hand-driven example covered this chrome.** None of the examples drew a title bar, a tab strip, or a status bar, and the panes example draws cells it does not describe, so there was no example snapshot to capture; the crate test of item 1 compares against the widgets drawn by hand instead. The chrome example (`examples/chrome.rs`) is new, with a `TreeWindow` test for each interaction: the menu button by pointer, keyboard, and screen reader, opening files, activating, adding, closing, and dragging tabs, a tab's menu, dragging the divider, the file list's menu by right click and Shift+F10, the which-key prefix, the settings layer, and closing the window.

11. **`TreeWindow` gained `right_click`, `click_at`, `drag`, and `center`**, the pointer input the chrome's tests need.


The cost of the new store paths (the context menu regions, the focusable controls' rectangles kept for Shift+F10, the regions taken over everything, and the split arms), measured by `make bench` against the phase 4 commit on the same machine (release, software GL, best of two runs): a form of 9 fields rebuilds, lays out, and draws without the GPU in 0.15 ms, up from 0.11 ms, a fixed cost of about 0.04 ms a frame; a form of 300 fields takes 2.32 ms against 2.31 ms, so the cost does not grow with the tree; a key costs 0.002 ms and 0.05 ms, as before. A list of 100,000 rows draws as a tree element in 0.155 ms against 0.139 ms, and a table's 0.33 ms to 0.44 ms is within its run-to-run noise (the base commit's ran 0.31 ms).

## 12. Lifecycle, Embedding, and Element Properties as Built

A large desktop app (a reader with a library manager) written as one tree `App` needed three things phases 1 to 5 left to `Application`: the app's own timers, windows, and window events; a way to put a hand-driven widget or a painted area in the tree; and layout, surface, and pointer properties on any element. Each was built as the option closest to the principles above where the design left the choice open:

1. **Timers are `App::about_to_wait`, not a deadline API.** `App::about_to_wait(now, cx)` is called each time the event loop is about to sleep and returns when it next needs calling, as `Application::about_to_wait` does, so a debounce is a field holding a due time that a callback sets and the hook checks (`Deadlines` keeps several). It is also where state a callback set becomes a request only `AppCx` can make, since callbacks reach only the app's state (decision 1): a callback sets `details_wanted`, the hook opens the window. `TreeWindow::about_to_wait_at(now)` tells the app the time is `now`, so a test steps a timer past its deadline without sleeping.

2. **One app, many windows, through `AppCx`.** `AppCx::open_window(options)` opens a window whose contents are `App::view(id)` with the id it returns, and `AppCx::close(window)` closes any of them; the app quits with its last window. Headless, a `TreeWindow` sets up the windows the app opens, and `TreeWindow::windows` and `switch_to` pick the one the inputs go to, each with its own store.

3. **Window events are hooks with defaults.** `App::close_requested` returns whether the window closes, so an app with unsaved work refuses and asks, closing it later with `AppCx::close`; `window_closed`, `focus_changed`, `resized` (size and scale factor together, as winit reports them), and `file_dropped` (a file no element took, with where the pointer was) follow. The appearance hook already existed (`App::follow_appearance`). `Application` gained `resized` too, with a default, since the runtime had no other way to tell a hand-driven app.

4. **`custom` embeds a widget with closures, not a response enum.** `custom(create)` keeps the `W: Widget` that `create` makes the first time the element is seen, as the store keeps a control's widget, handing it the element's inputs, stepping its animation, and drawing it clipped to its area. `.sync(|w| ..)` copies the app's state in at every rebuild (the controlled half, which compares before it sets so the widget's own state lasts), `.on_response(read, apply)` brings answers back: `read` looks at the widget after the input and its response and returns a value, `apply` stores it on the app's state once the tree is dropped. Section 4.8's `custom(id, make, respond)` left the response to the app's `Application`; reading the widget is what an app needs (a code view's response says the cursor moved, not where), and a value keeps the app's state the one source of truth. The id is the element's own (`Element::id`). It takes the room its column leaves, as a list does, unless `.measure(|w, width, metrics, scale| ..)` or `Element::height` says otherwise; it is a tab stop unless `.unfocusable()`; screen readers hear what `.access(|w, frame, id| ..)` describes (most widgets have an `access(frame, id)`), or a group named by `.name`. A widget that needs data lent at each call (a `FlowList` and its source) is embedded through a small wrapper widget that owns its data, as phase 4's boxes do; a widget's caret for the input method and its screen reader actions are not forwarded yet.

5. **`canvas` paints with a closure that captures the app.** `canvas(|frame, area, theme| ..)` reads the app's state through what the closure borrows from `view`, rather than being handed `&A`, since the tree already borrows the app for its lifetime; drawing is clipped to the area. It takes the room left unless measured or sized, and takes the keyboard only with `.on_key(accepts, apply)`: `accepts` decides at once whether a key is the canvas's (a key it does not take goes on to the containers and the app), `apply` runs once the tree is dropped, the split every callback needs because the app is borrowed while keys are routed. Its pointer input is the element properties of item 7, so a canvas and any other element take the pointer the same way. It is described as a `Canvas` named by `.name`.

6. **Both are controls to the store.** `Custom` and `Canvas` implement `Spec`, so nothing in the store's paths changed for them. A widget's answer that only the element can turn into a call (the element holds the app's closures) is worked out in `Spec::handle` and kept in the widget's box until the store takes it through `Control::changed`; `Spec` gained a defaulted `measure_widget` for an element that, rather than its widget, knows its height. Implementing `Node` directly would have been simpler, but the blanket `Node` impl for every `Spec` makes it conflict under coherence.

7. **Element properties live in one boxed `Extras`.** `Element::width`, `height`, `min_width`, `max_width`, `min_height`, `max_height`, `grow`, `shrink`, `gap`, `align`, `justify`, and `padding_px` map onto `misty::layout` (min and max sizes and flex shrink were added to `layout::Style`); what the app sets wins over what an element's place gives it. Sizes are `Extent::Cells` (the default unit, which keeps an element in step with the text beside it whatever the font size), `Extent::Px` (logical pixels, scaled by the display, for what is drawn rather than written), `Extent::Fraction` of the container, or `Extent::Auto`; spaces are `Space::Cells` or `Space::Px`. `Element::padding` stays in cells and `padding_px` adds pixels on top. An element given none of these keeps a `None` and lays out exactly as before, which `make bench` confirms.

8. **Surfaces come from theme tokens and the theme's shape.** `Element::background`, `border`, and `hover_background` take `ThemeToken`s, never colors, and `Element::corners` picks `Corners::Box` (the default), `Control`, or `Square` from the theme's `Shape`; a border is as thick as the theme's box borders, and at least a device pixel, so a border asked for shows in a theme whose boxes have none. The hover fill needs no app state.

9. **The pointer on any element goes through a region under the element's id.** An element with pointer properties gets a region (child `1 << 35` of its id) registered before whatever it holds, so the controls inside take the pointer first and what they leave falls through. `on_press` reports a `Press`: the button, the click count from `EventCx::click_count`, the modifiers, and the point from the element's top left. An element with `on_press` that is not a control already becomes a tab stop in reading order with a focus ring, pressed by Enter, Space, or a screen reader's click (described as a button named by `access_name`, its text, or its tooltip), so whatever the pointer can do the keyboard can. `on_hover` reports the pointer coming and going, `on_scroll` a wheel turn nothing inside used, and `on_pointer_drag` the moves and the release of a press, the region keeping the pointer until the release (the canvas's strokes). `TreeWindow::send_at` gives an input a time, so a double click is tested without racing the clock.

10. **Drag and drop between elements is one `Drag` per window.** `drag_source(payload, label)` arms the store's `Drag<Rc<dyn Any>>` on a left press, which starts after the usual few pixels so a click stays a click; `drop_target(accepts, on_drop)` takes payloads of its type that `accepts` allows, the innermost such target under the pointer is marked while the drag is over it and gets the payload and the point from its top left, and Esc or the window losing the keyboard cancels. Payloads are typed by the app (`P: Clone + 'static`), matched by downcasting, so two kinds of drag never land on the wrong target. Misty has no keyboard convention for dragging between arbitrary elements (a list moves its rows with its own keys), so the keyboard way is the app's: the workbench example selects a book with Enter (its card's `on_press`) and moves it with `m`.

11. **External files reach the element under the pointer.** `on_file_drop` takes a file dropped from another app on the innermost element that has it, with the point from its top left, the element marked while files are held over it; a file no element takes goes to `App::file_dropped`.

12. **The workbench example joins them.** `examples/workbench.rs` has two shelves of styled cards (pressed to select, dragged between shelves, moved with `m`, taking dropped files), a filter applied once typing settles, the selected book's notes in an embedded `CodeView`, a canvas taking strokes and cleared by `c`, and a second window, each tested through `TreeWindow`. No widget file changed.

The cost, measured by `make bench`'s tree benchmarks against the phase 5 commit on the same machine (release, software GL, two runs each): a form of 9 fields rebuilds, lays out, and draws without the GPU in 0.116 to 0.119 ms against 0.119 to 0.121 ms; a form of 300 fields in 2.25 to 2.28 ms against 2.23 to 2.25 ms; a list of 100,000 rows as a tree element in 0.138 to 0.140 ms against 0.137 ms; a table in 0.312 to 0.315 ms against 0.307 to 0.314 ms; keys unchanged. Elements without properties pay one `None` check each.

## 13. What a Workbench Shell Needed

A workbench app's shell (a frameless window whose title bar carries the app's panel buttons, a hamburger menu of sections, spatial pane navigation, chords on symbol keys, and headless tests of its timers) found gaps the earlier phases left. Each is closed additively; existing apps draw and behave as before.

1. **App buttons in the title bar are one tab stop with the menu button.** `TitleBar::app_button(button, apply)` adds a `TitleBarButton` (an icon, a label, or both, with a tooltip) at the end of the room beside the menu button, or beside the window buttons when those sit at the right; the hand-driven `TitleBar` takes them with `with_app_buttons` and answers `TitleBarResponse::AppButton(index)`. The bar stays a single tab stop, as a toolbar is one, rather than one stop per button: Left and Right move between its buttons left to right, wrapping, Enter or Space presses the one the keyboard is on, and the focus ring follows it (`Drawn::hit` is that button). Screen readers see each button under the bar's id from child `1 << 16`, named by its label or its tooltip. A part of a control that shows a tooltip of its own reports it through `Spec::tips`, which the store adds to the element tooltips, so the help shows where the pointer rests and hides under a modal layer as any tooltip does.

2. **Menu headings are entries, not disabled items.** `MenuEntry::Heading(label)` is a row of its own rather than a flag on `MenuItem`, since a heading has no id to hand back and a disabled item would still read as a command. The highlight's steps, the pointer, and a screen reader's click skip it as they skip separators, and it is described with the heading role inside the menu. Adding the variant breaks an exhaustive `match` over `MenuEntry`; the downstream apps have none.

3. **Stressed chrome words take their weight from the theme.** The chrome made spans bold in a dozen places; `Theme::emphasis` (`Emphasis::Bold` by default, so nothing changes, or `Regular`) now decides, through a crate-internal `Span::emphasized`. It is a choice between the font's two weights rather than a number, because the renderer draws a span at the window font's regular or bold weight and nothing between. Content keeps its own weights: a document's headings, an info card's keywords, and a span an app made bold. The quick pick's match highlights and the public `fuzzy_highlight` helper, which has no theme to read, still bold their matches.

4. **The app moves the focus through `AppCx`, by element id.** `AppCx::focus(id)` asks for the focus to go to the element the app named with `Element::id`, so spatial pane moves (Alt+H, J, K, L) move the real focus ring rather than a pane field the app keeps beside it. The request is the window's store's until a frame resolves it: the next rebuild's sync gives it to the element if it takes the keyboard, else to the first tab stop inside it in tab order (a control, a pressable region of `Element::on_press`, or a stop a widget registers inside its own), unless the keyboard is inside it already (a pane keeps the control it had); an element new in that frame takes it once the frame has registered its stops. It goes through `AppCx` rather than a property honored on rebuild because a property states where the focus is, which would fight the user's Tab, while a request says when it moves; callbacks, which reach only the app's state, ask through a field `App::about_to_wait` reads, as they open windows.

5. **Key places ride beside keys, not inside them.** `Key` drops Shift on a symbol because the character already says what Shift did, and the apps' keymaps, tests, and struct literals depend on that equality, so keeping Shift there, or adding a field to `Key`, would break them. The runtime instead reads where each key sits (winit's physical key code, named by the US layout's unshifted character or key name) into the window's state, beside the key repeat, and `EventCx::physical_key` reports it, Shift kept, for chords written by key place; `App::key` reaches it through `AppCx::event_cx`.

6. **The test clock is the thread's, held by `TreeWindow::advance`.** Widgets, the store, and the input reader read the time through one crate-internal `clock::now`, which is the system's clock unless a test holds it on its own thread: `TreeWindow::advance(by)` holds it at the time it is the first time, moves it on, and runs `about_to_wait` at the new time, so a tooltip's delay, a which-key popup's pause, a caret's blink, and an app's debounce are stepped without sleeping, while tests that never advance it see the system's clock as before. A thread-local rather than a clock handed to every widget keeps the `Widget` contract and every widget's signature unchanged; tests run one per thread, so they do not share it, and dropping the window lets it go. The event loop keeps the system's clock, and so do the waits that poll a background source (`Picker::settle`). The quick pick still reads the system's clock, so its query pause is not stepped yet.

## 14. What a Document Viewer Needed

A PDF viewer in a tree app (pages painted on a canvas driven by the app's keymap, a toolbar of buttons, a find field, pages in split panes, and a scroll that speeds up while a key is held) found gaps sections 12 and 13 left. Each is closed additively, as the option closest to the principles above; apps that use none of it draw and behave as before, but for item 6.

1. **The pointer's place is a property callback, not a field to poll.** `Element::on_pointer_move(apply)` reports the pointer from the element's top left on each move over it (`Some(point)`), whatever is inside it, and `None` once when it leaves, as `on_hover` reports coming and going; the region of section 12 item 9 already saw every move, so it is the same region. Reading `EventCx::pointer` in `App::about_to_wait` worked but put view state back in the app's loop. Every input already rebuilds the tree once and redraws once, and the callback runs after the tree is dropped like any other, so a move costs no extra rebuild; a test counts the builds of a move over an element with the callback and one without, and they match. Setting the pointer's shape still goes through `AppCx::event_cx`.

2. **A text field's Enter and Esc are callbacks the field takes.** `TextField::on_submit` and `on_cancel` run on plain Enter and Esc, and the field then takes the key, so it reaches neither another element nor `App::key`; without them the keys go on as before. They take only the app (`Fn(&mut A)`), since the text is the app's already through `on_change` (decision 3). The field widget answers both keys `Ignored` and its response enum is matched exhaustively downstream, so rather than a new response, `Spec` gained a defaulted `unused(widget, input)`: what an input the widget left runs when the element takes it itself.

3. **Tab order across containers is `order` on a container.** Plain containers are not in the focus tree (section 9 item 6), so a control's `order` sorted it among every tab stop of its scrolling container, and `order` on a container did nothing. Now a container (a column, a row, a form, a split) given an order registers its tab stops inside a focus group, visited as one block that takes the order among the elements beside it in the tree; the container holding such a block becomes a group too, so the order is among element siblings and the rest of the window keeps its place. A scrolling container, entered already, takes the order itself. Groups are crate-internal (`FocusTree::enter_group`): they take neither the keyboard nor the pointer, cut nothing inside them, and are passed over in the accessibility tree, so `ui_text` and pointer routing are unchanged. A window-wide tab index (HTML's positive `tabindex`) would fight reading order and break as elements come and go; a separate scope element would be a second way to say what `order` says.

4. **A pressed canvas takes the keyboard; a key-less one asks with `focusable`.** Pressing a canvas that takes keys already focused it, as a press focuses any control; the viewer's press landed elsewhere because of item 6, which left its canvas no height. What the viewer did need was a canvas that is a tab stop and focused when pressed yet takes no keys, so the keyboard rests on the pages and every key reaches the app's keymap rather than a toolbar button taking Enter. It used `on_key(|_| false, ..)` for that; `Canvas::focusable()` says it.

5. **A pointer press without a tab stop is its own callback.** `on_press` makes an element a tab stop pressed by Enter, Space, and screen readers (section 12 item 9), right for a card and wrong for pages that have their own keyboard path. `Element::on_pointer_press` reports the same `Press` for pointer buttons only and registers the region for the pointer alone. A separate callback rather than a flag on `on_press` keeps one meaning per name, as `on_pointer_drag` beside `on_press` does.

6. **A root's `grow` no longer shrinks it.** A split lays each side out as a root, which fills its area; but `grow` replaced a root's height with a flex share of a parent it does not have, which resolved to its content's height, so a canvas growing inside a growing side got none. A root has no siblings to share room with, so `grow` there now changes nothing and the root fills its area, window and layer roots alike. This is the one behavior change: misty's examples grow no root, and the apps built on misty then either built no tree app or set `.height(Extent::Fraction(1.0))` beside its `grow` only to undo the old behavior.

7. **Key releases reach a tree app through their own hook.** `App::key_released(key, cx)`, with a default that does nothing, gets each release no element took, so a held-key ramp ends on the release rather than on repeats stopping. `App::key` keeps seeing presses only: keymaps (an app's own among them) treat every key they get as a press, so releases never come through it. A release goes to the app even while a modal layer keeps the keys, since a release ends what a press began; none comes for a key held as the window loses the keyboard, because the event reader drops winit's synthetic releases, so `focus_changed` is the app's cue to let every key go. A hand-driven `Application` already gets `Input::KeyReleased`, and nothing changes for it, so it gained no hook. winit reports releases on X11 (with detectable autorepeat, so a held key repeats without releases between) and on Wayland. `TreeWindow::release(keys)` lets keys go in tests.

The cost, measured by `make bench`'s tree benchmarks against misty's main before these changes (`1ed4eec`) on the same machine (release, software GL, two runs each, alternating): a form of 9 fields rebuilds, lays out, and draws without the GPU in 0.161 to 0.164 ms against 0.176 to 0.192 ms; a form of 300 fields in 2.63 to 3.08 ms against 2.83 to 3.28 ms; a list of 100,000 rows as a tree element in 0.190 to 0.228 ms against 0.180 to 0.183 ms; a table in 0.325 to 0.512 ms against 0.321 to 0.400 ms, its hand-drawn twin moving as much in the same runs; keys unchanged. The spread is the machine's run-to-run noise; the new paths are one check per container for an ordered child, and the region's checks only for elements with the new properties.

## 15. What a Book Reader Needed

A reader of EPUB books and Markdown previews in a tree app (chapters in a `RichText` embedded through `view::custom`, a spread mode of two pages side by side, loose lines, centered and justified paragraphs, headings at one weight, and a Vim-style editor beside it taking text from an input method) found gaps the earlier sections left. Each is closed additively, as the option closest to the principles above; apps that use none of it draw and behave as before, but for item 6 under a theme of `Emphasis::Regular`.

1. **Pages are a mode of `RichText`, not a widget of their own.** The reader cut pages by moving the app caret through a laid out column and reading its rectangle, binary searching for the line that crossed each page's foot, then drew two `RichText`s scrolled to the page tops and clipped. `RichText::set_paged(Some(Paged::new(columns)))` does it inside the view instead: the text is laid out once a column wide (the area less the gaps between pages, shared out), and cut into pages as tall as the area, each ending at the top of the first line, image, or custom block that would cross its foot (one taller than a page is cut through), the next starting at the first line or block below, so a page never opens on a blank gap. The view keeps what it showed as panes, the one viewport when scrolled and a page each when paged, and every place that mapped between the window and the content (hit tests, marks, the caret, custom blocks, screen readers) goes through the pane showing that content, so selecting, find, links, and marks work on both pages of a spread with no second code path. A separate paged widget, or a `Pages` element holding several `RichText`s, would have had to share the layout, the selection across pages, and the marks between views, which is what the reader's workaround had to do by hand; a mode keeps one obvious way and one model of places (`RichTextPos`). Turning is by spreads (`turn_pages`, `set_page`, the page shown always the left of its spread) and answers whether the page changed, as `scroll_by` does, so the app knows when to open the next chapter; the wheel is the app's when paged, since how fast a wheel turns pages (the reader waits 450 ms between turns) is a reader's choice. The first place on the page shown is kept, so a resize or a new line height finds the same place on the new pages. `place_rect(pos)` gives where a place shows on any page, which the reader's link hints and visible-place checks probed the caret for.

2. **Scrolling by an amount answers whether it moved.** `RichText::scroll_by(ScrollAmount)` scrolls by pixels or by lines (the line Up and Down scroll by), gliding as keys do, and returns `false` when the view was already at the edge it went toward, which the reader found by comparing scroll positions before and after. `content_height` gives the laid out height beside `scroll_position`. Before a frame, an amount is added to a scroll position still waiting, as the reader's requests do.

3. **Hiding the scrollbar is a switch on `RichText`, kept crate-internal on `ScrollView`.** `set_scrollbar_hidden(true)` lays the text out across the whole width and draws no thumb; keys and the wheel still scroll. Paged views show none either way.

4. **Line height belongs to the paragraph, and the view sets it on its text.** The renderer shaped every paragraph at the window font's line height, so a looser line had to be carried with the text: `Paragraph::with_line_height(multiple)` travels in the paragraph's wrapping, which measuring, the shape cache's key, the renderer, and the transformed text raster all read, so layout, carets, hit tests, and pixels cannot disagree. Each run's line scales from it, so a larger span makes its line taller in step, as CSS's unitless `line-height` does. `RichText::set_line_height` sets it on a document's every paragraph and a plain paragraph without its own; the gaps between blocks stay in lines of the font, as CSS margins are not scaled by `line-height`.

5. **Alignment is a block that holds blocks.** `Block::Aligned(align, blocks)` sets the paragraphs and headings inside it, through lists, quotes, boxes, and table cells, as CSS's inherited `text-align` does, the innermost winning; code keeps to the start. A field on `Container` or `Heading` would break struct literals in apps (those types are not `#[non_exhaustive]`), and `Block` is, so a variant is the additive choice, and it says what an EPUB's `<div style="text-align: center">` says.

6. **Headings take their weight from the theme.** Section 13 item 3 left a document's headings bold as content; a reader drawn at one weight then had to fake headings as scaled paragraphs, losing their heading role and anchors. Headings now follow `Theme::emphasis` (bold by default, so nothing changes under the built-in themes), a span the app made bold staying bold. A `RichText` that draws under a theme of another emphasis rebuilds its document's paragraphs once and lays them out again.

7. **An embedded widget's caret reaches the input method through `ime_caret`.** The store already handed a focused custom widget its `Input::Ime` events (they route to the focused element, as keys do), but it enabled the input method only for a focused element whose `Spec::caret` gave a rectangle, and `custom` gave none, so the window disabled it and no composition ever came. `Custom::ime_caret(|widget| ..)` reads the widget's caret after each frame, as the text field's and text area's elements read theirs (`Spec::caret` now takes the element, so a closure can answer); while it gives one the input method is enabled and its candidate window kept beside the caret, and `None` (a Vim Normal mode) disables it so keys reach the widget as keys. Preedit and commit are the widget's to show and type, through `Preedit` as misty's widgets do. `TreeWindow::ime_area` gives the rectangle the input method follows, for tests that send `Input::Ime`.

8. **`press` keeps a Shift the notation folds, beside the key.** `Key::new` drops Shift on a character because the character says what Shift did (section 13 item 5), so `press("S-<space>")` sent a plain space and a keymap reading key places saw no Shift; the reader's test set the place by hand. Changing the notation would change what `S-g` means in every keymap and test, so `TreeWindow::press` and `TestApp::press` instead send such a key with its place (`EventCx::physical_key`) at the key written with Shift held, as the runtime reports a keyboard's Shift+Space; the key sent is the same, and a place the test set wins. Tests pressing keys with no folded Shift see no place, as before.

The cost, measured by `make bench`'s rich text benchmarks against misty's main before these changes (`d2df17a`, main with the new chapter benchmark) on the same machine (release, software GL, two runs each, alternating): a chapter of 30 headings over 300 wrapped paragraphs laid out anew and drawn without the GPU in 6.01 to 6.23 ms against 6.14 to 6.86 ms, and scrolled in 1.80 to 1.87 ms a frame against 1.89 to 1.92 ms; the dialog of wrapped paragraphs 1.70 to 1.81 ms against 1.79 to 1.80 ms. The spread is the machine's run-to-run noise: a paragraph without a line height shapes and draws as before, and scrolling adds one pane to walk. Paged, the same chapter is laid out, cut into pages, and drawn in 5.79 to 6.37 ms (a column narrower, so more lines but shorter), and turned a spread a frame in 1.89 to 2.29 ms.

## 16. What a Library Page Needed

A reference library in a tree app (tables of papers, a sidebar tree of venues, lists of tags and reading items, and a graph of notes on a canvas) found gaps the earlier sections left. Each is closed additively, as the option closest to the principles above; apps that use none of it draw and behave as before.

1. **Tables and trees take the proportional font as lists do.** The hand-driven `Table` and `TreeView` had `with_proportional_font`, but the `table` and `tree_view` elements made their widgets without it, so a table's cells stayed on the cell grid. `Table::proportional()` and `Tree::proportional()` pass it through when the element is first built, as `List::proportional()` does; a table's header keeps the theme's chrome font and a tree's arrows and indents stay on the cell grid, so they line up by depth.
2. **A row takes the app's keys after the widget's own.** The library's reading list marks the item under the cursor read with `x` and Discover imports a paper with `i`, which the app routed through its own key handler for the active tab, reading the cursor back out of its state. `List::on_row_key(accepts, apply)`, and the same on `Table` and `Tree`, hands the app a key with the cursor's row (a row index, or a tree item's key) in the style of `Canvas::on_key`: `accepts` decides at once, while the key is routed, and `apply` runs once the tree is dropped. The element asks only about a key its widget answered as ignored, so the widget's motions, counts, `g` and `z` prefixes (and the key a prefix swallows), Space and Enter stay its own whatever `accepts` says. The key rides in the element's answer beside what moved, not through `Spec::unused`, so a count the key dropped (`5x`) still reaches `on_keys` in the same input. A key `accepts` declines, or any key with no row under the cursor (an empty list, a tree's loading row), leaves the answer ignored and goes on to the containers around the element and `App::key`, as before.
3. **A canvas draws UI-font labels with the frame's paragraphs; no new API.** The notes graph drew its node names with `Frame::draw_plain_text`, which sets them on the cell grid. The frame a canvas's closure is handed already sets text in the proportional font whatever the theme: a `Paragraph` drawn by `Frame::draw_paragraph` in a rectangle one line tall (`Frame::cell_metrics().height`) and as wide as `Frame::paragraph_width` measures it, a pixel to spare so rounding never wraps it, is a label at a point, centered by moving the rectangle half its width. `draw_label` would follow the theme's chrome font instead, and `Frame::with_font` changes the cell grid's font, not the proportional one. The `canvas` docs show it with an example; a helper beside `draw_label` would only rename those two calls.

## 17. What a Reader's Polish Needed

A book reader and a PDF toolbar in a tree app found four gaps the earlier sections left: headings set at the reader's own sizes and line heights, toolbar buttons that show an icon, and a toolbar that fits its buttons to the width it was laid out at. Each is closed additively, as the option closest to the principles above; apps that use none of it draw and behave as before.

1. **A block's line height rides on a wrapper block, as alignment does.** Section 15 item 4 put line height on the paragraph and let the view set it on all of a document's text, so a reader setting its body at 1.7 set its headings at 1.7 too, where the reader it replaces sets them at 1.25 to 1.4. `Heading` is a struct of public fields that apps build as literals, so a field on it would break them; `Block` is `#[non_exhaustive]`, so a variant does not. `Block::Styled(BlockStyle, blocks)` sets the blocks inside it as `Block::Aligned` sets their alignment: the innermost style that sets a line height wins, and it wins over the view's, as a paragraph's own `Paragraph::with_line_height` does. `BlockStyle` has private fields and `with_` methods, so it can take more settings without breaking anyone. A text block is still at least a body line tall, as before.
2. **A heading's scale is a style setting, and the table is public.** A `Block::Heading` set its spans by private factors, so a reader wanting its own sizes kept a copy of them and divided by it, which would go wrong the day misty's changed. `Heading::SCALES` and `Heading::level_scale` make the factors public, and `BlockStyle::with_heading_scale(scale)` replaces them for the headings inside the style, so `1.0` sets a heading's spans at exactly the sizes the app gave them while the heading keeps its level, role, anchors, and weight. It is a style setting rather than a field of `Heading` for the reason item 1 gives, and a scale rather than a size because a heading's spans may each have a size of their own, which a scale keeps in proportion.
3. **An icon on a tree button is a builder method, and an icon alone takes its name.** The hand-driven `Button` already drew an icon beside or in place of its label, but `view::button` made it from a label only, so the toolbar painted its icons on pressable canvases, sized by hand, with a tooltip and an accessible name given separately. `Button::icon(svg)` adds the icon to a labelled button, and `icon_button(svg, name)` takes the name with the icon, so the compiler, not a review, keeps an icon-only button named: the name becomes the widget's tooltip, which already names a label-less button to screen readers, and the element registers it as a tooltip region over the button, as a title bar's buttons register theirs (`Spec::tips`). The icon and name are copied into the widget at each frame in `Spec::draw` rather than through `ButtonProps`, whose public fields apps build as literals. The tree's icons are drawn in the theme's text color in every state but disabled, through a crate-internal flag on the shared `ButtonFace` that every other face leaves off, so the hand-driven buttons, toolbars, dialogs, and breadcrumbs keep their sizes and colors (a test pins the face widths, height, and a toolbar's natural width as formulas over the font's metrics).
4. **The laid-out size is a property callback, reported after the frame and only on change.** The toolbar decided which buttons fit from a width it computed from the app's split tree and sidebars, which drifts from the layout as soon as a gap or a border changes. Three designs were weighed. Reading a size by id (`AppCx::size_of`) would put a query of view state back in the app's loop, as reading the pointer in `App::about_to_wait` did before section 14 item 1, and the app would have to remember to ask. Measuring during the build (a closure handed the width, as a `measure` is) would make `view` depend on layout, which runs after it, and break the one-way flow of section 3. `Element::on_size_change(apply)` follows section 14 item 1 instead: a property callback that runs on the app's state like any other, after the tree is dropped. The paint pass, which already has every element's rectangle, records the size of each element with the callback; `Store::about_to_wait`, which already rebuilds the tree to run what is due (section 9 item 16), compares them with the sizes last reported and, only when one differs, rebuilds once to collect the callbacks, runs them, and asks for a frame. So the value always arrives for the next rebuild, never the one under way, and a window whose sizes hold still pays a map comparison and no rebuild, which a test counts. A loop is possible only when an element's own size follows what its callback stores; the docs say to size it by its place instead. The size is reported in physical pixels, as the pointer's places are, with the scale factor and logical accessors for comparing with `Extent::Px`. It is named `on_size_change` rather than `on_resize`, which the split, list, and table elements already use for a divider's ratio and column widths.

## 18. What a PDF Toolbar's Buttons Needed

The PDF toolbar of section 17 drew its icon buttons through `view::icon_button`, but they came filled, where a toolbar's are flat, and each took a share of its row, so the app gave every button a fixed slot. Both are closed in the element tree; the hand-driven `Button`, its `ButtonFace` sizes, and `face_height` are unchanged.

1. **A look is a builder method copied in at each frame.** The hand-driven `Button` already drew `ButtonVariant::Ghost` (flat at rest, a subtle fill under the pointer and while pressed, the selection fill for a toggle pressed in), so `view::Button::variant(variant)` hands the variant through rather than adding a look of its own. A method taking the enum rather than a `ghost()` shortcut gives one way to pick any of the four looks, and the enum is already public. The variant is copied into the widget in `Spec::draw`, as the icon and name are (section 17 item 3), not through `ButtonProps`, whose public fields apps build as literals. The ghost's hover fades as a color, through the button's existing `HoverFade`, with no shadow or movement; the focus ring is the focus tree's, drawn around every control whatever its look. A pixel test draws a row of ghost buttons, labelled and icon-only, beside a default one through a `TreeWindow`, and checks the ghosts show the background at rest and a fill under the pointer.
2. **A control may report a natural width, and a row packs to it.** A control measured only its height, so in a row it took `Length::Flex(1.0)` unless the app gave it a width, and a row of icon buttons spread across the toolbar. The layout closure has no frame to measure text with, so the store measures before layout, in the pass that already measures proportional texts: `Spec::natural_width(frame, theme)`, `None` by default, is the button's face width through the same `ButtonFace::width` the hand-driven `Button::width` uses, so the slot and the face never disagree. In a row a control with a natural width gets it as a fixed width, still shrinking when the row is short, and a padded one's box hugs it; `Element::grow` or `Element::width` wins, so stretching stays one call away, and in a column nothing changes. Only buttons report one for now: a checkbox, a select, or a slider still takes a share, since their natural widths are less settled (a slider has none) and no app has asked. This moves existing buttons in rows, which is listed under Changed; the dialog and workbench examples and the crate's tests were checked, and none depended on the stretching, as a button's face was always drawn at its natural width at the left of its share. `make bench`'s tree form of 300 rows, which holds no buttons, rebuilt, laid out, and drew in 2.45 ms against 2.32 ms before (3.76 ms against 3.61 ms a frame on the GPU), a single run each, the extra cost being the measuring pass now looking at every control's kind.

## 19. What an Office Viewer Needed

A spreadsheet viewer, a print page, a Settings modal, and a dictionary card in a tree app found gaps the earlier sections left. Each is closed additively, as the option closest to the principles above; apps that use none of it draw and behave as before.

1. **A spreadsheet's cells are the source's to style and the table's to lay out.** A sheet viewer drew fills only behind a cell's glyphs (a span's background), had no borders, merges, frozen panes, per-cell alignment, cell cursor, or zoom of its text, and put its charts in a strip under the table because nothing drew over the cells. What a cell looks like is per cell and changes with the data, so it comes from the lent source, asked only for cells on screen as their content is: `TableSource::cell_style` (fill, borders, alignment) and `TableSource::merged_range`, both with defaults that ask for nothing, so a source that leaves them alone costs a call that returns at once (`make bench`'s table of 100000 rows stayed within the noise of a loaded machine). How the table is laid out (frozen rows and columns, zoom, the cell cursor) is the table's state, as its sort is, set by `with_` and `set_` methods on the hand-driven `Table` and builder methods on the element. Frozen rows are the list's: `ListState` keeps a count of pinned leading rows, so the cursor moving onto a row the pinned ones hide scrolls it out from under them, and the list draws them in a band at the top over the scrolling rows, hit testing and `row_rect` following; nothing changes with none pinned. Frozen columns reuse the sideways scroll, the columns after them clipped to the room they leave, and the first scrolled column starts where they end, as the first column starts at the table's left today. Fills are drawn with the row, every fill of a row before every content so they batch; merged ranges and borders are drawn after all the rows, by part of the table (frozen or scrolling, each way), so the next row's fill never covers a border and a range is drawn once whatever row it is met on. A border is centered on its grid line, so the right edge of one cell and the left edge of the next coincide instead of doubling. Zoom scales the cells' text through a crate-internal `Frame::with_text_scale`, the cells' metrics scaled rather than measured again, and the columns' widths through the scale they are drawn at, so the widths an app gives, saves, and a drag reports stay unzoomed; the header keeps the chrome's size. The cell cursor turns Left and Right (`h`, `l`), `0`, and `$` from scrolling sideways into moving between shown columns, which is what a spreadsheet's arrows do, and scrolls its column into sight only when it or the row cursor moves, so the wheel can still look away. `TableSource::draw_over` hands the source a `TableCells` that places any cell or range, on screen or not, clipped to the part of the table that scrolls both ways, for charts anchored to cells.
2. **A modal layer may hand its leftover keys to the app.** A modal `overlay` sends a key no element inside took nowhere (`Route::Trapped`), which is right for a dialog asking a question, but a full page shown as a modal layer (a print preview, a Settings page) wants the app's keymap: Esc to close it and the app's chords. `Overlay::keys_to_app()` keeps everything else the trap guarantees (Tab cycles inside it, the focus cannot leave it, the elements under it get no input, the focus goes back when it goes) and only changes where a trapped key ends: in `App::key`, as an unhandled key does without a layer. It is checked against the innermost trap only, so a select's list or a menu opened inside the layer still keeps the keys while it is open. A per-layer `on_key` was weighed and left out: the app already has one key handler, `App::key`, and a second for the same keys would be two ways to do one thing; an app that wants the layer's keys apart reads its own state there.
3. **A dialog takes a text field as its content; other elements stay a modal layer.** Section 9 item 13 left `Dialog::with_content` unwrapped, because tree elements inside the dialog would share the focus between the focus tree and the dialog's own buttons, which the dialog moves itself. A lookup card needs only a word typed into it, and a text field need not be a tree element to do that: `Dialog::field(text, apply)` keeps a hand-driven `TextField` beside the hand-driven `Dialog` in the dialog's widget, in the room `with_content(1)` keeps, so the focus tree sees one control and the dialog alone moves the keyboard between its field and its buttons, as its module documents for an app's content. The widget hands keys to the field first while it has the dialog's keyboard and the rest to the dialog, gives Enter the field leaves to the first primary button, draws the field focused only while the dialog has the keyboard, names it to screen readers under the dialog's node, and reports its caret through `Spec::caret`, so the input method follows it as it follows a `text_field`. The field's text is the app's, synced each rebuild as a `text_field`'s is, so an app that does not store the text sees the field keep the app's. Arbitrary elements in a dialog are still a modal `overlay` of elements, which section 10 item 11 made scroll and which `Overlay::keys_to_app` (item 2) now lets the app close with its own Esc; wrapping `with_content` for any element would mean a control hosting a laid-out subtree, which nothing else in the store does, for no case a field and a modal layer do not already cover.
4. **A masked field is the field's own mode, drawn from a mirror line.** API keys shown in clear once loaded were the gap. Masking is a property of the hand-driven `TextField` (`with_mask`, `set_masked`), so every field that embeds one could use it, and `view::TextField::masked()` copies it in at each frame, as section 18 copies a button's variant, because `TextFieldProps` has public fields apps build as literals. Drawing and hit testing never see the text: they run over a mirror `LineEdit` holding a dot per character, its caret and selection on the same characters, and a press maps back from the mirror to the text, so the cell-grid and proportional layouts needed no second code path. What could leak the text out is refused at its source: the copy and cut chords are taken and do nothing, the primary selection is never written, the input method's composition is not shown (committed text is still typed), a double click selects everything rather than revealing a word's bounds, and screen readers get `Role::PasswordInput` with the dots as its value, as AccessKit asks. The kill ring stays, since it never leaves the field.
5. **Form labels as chrome are an option, since the default would move every form.** A form's labels are chrome by the library rules, yet they were set on the cell grid, which a Settings modal in the UI font showed up. Following `ChromeText` by default would change the label column's width in every existing form (misty's chrome and controls examples and an app's forms), so `Element::chrome_labels()` asks for it per form, as `proportional()` does on lists, tables, and texts. The layout pass has no frame to measure text with, so the widest label is measured in the pass before layout that already measures proportional texts and buttons (section 18 item 2), stored by the form's index, and the label column takes it and the gap; the labels are drawn through `widgets/label.rs`, as all chrome is, cut to the column.
6. **Indents shape the first line and the rest apart.** Slide bullets were drawn as a paragraph of their own beside the text, because a `Paragraph` wraps every line at one width from one left edge, and cosmic-text, which shapes it, has no indent and wraps a buffer at one width. Moving lines after shaping cannot change where they wrap, so an indent changes the width a line wraps at: the paragraph is shaped whole at the width the first-line indent leaves and only its first line kept, then the text after that line is shaped on its own as a paragraph of its own spans at the width the hanging indent leaves; the two shapes are put together, each line moved in by its indent, the second's text offsets and span numbers moved on to the whole paragraph's, so `ParagraphLayout` and its carets, ranges, hit tests, links, and boxes need no change. Drawing emits two text segments, the first cut to the first line (its other lines are shaped but cut away) so a justified first line stays justified, and rendering to an image does the same with two buffers. Both shapes go through the shared shape cache under their own keys, which hold no indent since a shape does not depend on one. A paragraph with no indent takes the old path untouched, so documents, rich text, dialogs, and labels are shaped and drawn exactly as before. The indents are from the left, as the slides that asked for them are set; a right-to-left paragraph is moved the same way.

## 20. What a Folder Pane Needed

A folder tab and a file browser in a tree app wanted the path they show as crumbs to go back up by. The `Breadcrumb` widget had everything but a way into the tree, so the gap is closed additively as the option closest to the principles above.

1. **A breadcrumb is a pane's control, not the picker's.** `view::breadcrumb(segments).on_navigate(apply)` wraps the existing widget as a controlled element: the segments are the app's (it splits its path or its page's parents into labels each rebuild), and going to one runs `on_navigate` with its index, by a click or by Left, Right and Enter while it has the keyboard; the menu of the crumbs that do not fit is the widget's own layer, as a select's list is. Putting it in the quick pick instead would tie a location to one widget; a pane, a page, or a picker's header can each place the same element. The address bar mode (`Breadcrumb::set_editing`) stays on the hand-driven widget until a tree app needs it.

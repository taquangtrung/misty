# Element Tree Design

Status: proposal for approval. Nothing here is built yet.

This document proposes how misty moves from "the app drives every widget by hand each frame" to "the app describes its window as a tree of elements, and misty drives the widgets". It recommends one design, sketches its API, rewrites the `controls` example in it, and lays out a migration in phases. Open questions for the owner are collected at the end.

## 1. The Problem

The `controls` example (`examples/controls.rs`, about 470 lines before its form moved into a scroll view) shows nine controls and one status line. About 270 of its lines are glue the app writes only because misty has no tree:

- **Per-control frame calls** (`set_focused`, `height`, `draw`, `access`, `focus.register`) in `render`: about 95 lines.
- **Routing each input** to the control under a `FocusId` and mapping each response to a change, in `Controls::handle`, `toggled`, and `font`: about 50 lines.
- **Routing screen reader actions** to every control in turn, in `Controls::access_action`: about 25 lines.
- **A `Change` enum** restating each control's response, and `Demo::settle` applying it: about 70 lines.
- **Ten `FocusId` constants** naming what the tree could name: about 15 lines.
- **A label column laid out by hand**, row by row, in the `row` closure: about 15 lines.

Counted call by call, one frame makes 7 `set_focused`, 9 `access`, 9 `register`, and 9 `height` calls. The select's open list needs a second id (`FONT_LIST`), a focus trap opened on `Opened` and closed on `Changed` and `Closed` (three call sites), and a draw moved to the end of `render` so the list floats over the rows below it. Every widget hit tests against the rectangle it was drawn in last frame, so the app must draw before it can dispatch, and must remember to.

Each of these is a place an AI agent writing an app can get wrong without the compiler noticing:

1. Forgetting `set_focused` draws a focused control as unfocused, and it then ignores keys.

2. Forgetting `access` hides a control from screen readers. Nothing fails.

3. Registering in the wrong order breaks Tab order and pointer stacking.

4. Forgetting to close a trap leaves the window's keyboard stuck inside a list that is no longer drawn. `FocusTree` recovers only because it drops traps that are not registered.

5. Giving the slider `with_label("Volume")` and drawing "Volume" in the label column states the name twice, and the two can drift apart.

The tree should make each of these impossible, or make the compiler or a clear runtime message catch it.

## 2. Constraints

These come from `AGENTS.md` and the code as it is.

- **Keyboard first, and misty resolves no keys.** Widgets take keys meant for them; anything they leave goes back to the app's keymap unchanged, exactly as `FocusTree::dispatch` returns `Route::Unhandled` today.
- **The `Widget` contract stays.** Every widget keeps `handle(&Input, &mut EventCx) -> Response` and `draw(&mut Frame, Rect, &Theme) -> Output`. The tree is built on top of it, so a widget works with or without a tree, and its tests keep passing.
- **Data the app owns is lent, not copied.** `List`, `Table`, `TreeView`, and `IconGrid` take the app's rows through `Lent` for one call. The tree must lend data the same way, and must never ask the app to clone a large model into the tree.
- **Accessibility is rebuilt every frame** through `Frame::access`, and `FocusTree` supplies the focus. The tree should produce the same AccessKit output, so the existing `ui_text` snapshots stay comparable.
- **Layout is flexbox through taffy** (`render/layout.rs`: `Element`, `Style`, `Layout::rect`, measured leaves). The tree reuses it rather than adding a second layout engine.
- **Layers** (`make layers`): a module imports only from its own layer and those below. `widgets` and `chrome` never see winit.
- **Headless testing** through `misty::testing` must cover a whole tree: find a control by role and name, send keys and clicks, read `ui_text`, and never need a display.
- **AI-written apps** need one obvious way to do each thing, a small public API, and panics or errors that say how to fix the cause.

## 3. Recommendation

Adopt **a view function rebuilt every frame, with widget state kept by key** (option (b), simplified). The app writes `fn view(&self) -> Element<'_, Self>`; misty rebuilds the tree from it each frame and on each input, lays it out, draws it, routes input through it, and applies the changes the app's callbacks make to its state. The stateful widget objects that exist today (`TextField`, `Select`, `ScrollView`) live in a store inside misty, keyed by each element's id, and survive every rebuild.

### Why not the other two

**(a) Immediate mode with ids** (egui style: `if ui.checkbox(&mut wrap, "Wrap").changed() { .. }`) is the closest to today's code, and it would remove less of it. It lays a widget out as it draws it, so a label column or a dialog sized to its content needs a second pass or last frame's sizes. That is the "hit test against last frame's rectangles" problem again. Keys, traps, and accessibility still need an app-written id at each call site, so the `FocusId` constants and the trap bookkeeping stay.

**(c) Signals** (Leptos or Floem style) fight Rust's borrow checker: state lives in `Rc<RefCell<..>>` or arena handles, effects run at times the reader cannot see, and a test must set up a reactive runtime before it can press a key. An agent reading a signals app cannot tell from one function what a click changes. Signals also make it hard to keep `Lent`: a list's rows would have to move into a signal.

**(b) as proposed** keeps the data flow on one page, `view` reading `&self` and callbacks writing `&mut self`, which an agent can follow and a test can drive. Misty already redraws whole frames, so there is no retained render tree to diff: the "diff" is only "which ids are new, which are gone", done in the state store. That keeps the design much smaller than iced's or Xilem's.

### How it fits the constraints

- **Borrow checker:** `view(&self)` borrows the app immutably; callbacks are `Fn(&mut App, Value)` stored in the tree. Misty collects the calls they produce, drops the tree, then runs them on `&mut App`. No `RefCell`, no `Rc`.
- **Widget state:** the store owns one widget object per id: the caret, scroll offset, open list, and drag state live where they live today. The element only carries the props the app owns (options, value, enabled), synced into the stored widget each rebuild.
- **`Lent`:** `Element<'a, App>` borrows `&'a App`, so a list element holds `&'a [Row]` exactly as `Lent` does.
- **Testability:** a tree is a value. `testing::TreeWindow` builds it, routes inputs, and reads `ui_text` without a display.
- **Keys:** unhandled keys come back to a new `App::key` hook, the place the app's keymap runs.
- **Migration:** each widget gains one small adapter; the low-level `Application` trait stays for apps that draw by hand.

## 4. Proposed API

### 4.1 The app

```rust
/// An app described by a view: misty builds, lays out, draws, and
/// dispatches its tree.
pub trait App: Sized + 'static {
    /// The window's contents, built from the app's state.
    fn view(&self) -> Element<'_, Self>;

    /// A key no element took: the app's keymap. Return whether it was used.
    fn key(&mut self, _key: Key, _cx: &mut AppCx<'_>) -> bool {
        false
    }

    /// The theme the window is drawn in.
    fn theme(&self) -> &Theme;
}

pub fn run_app(app: impl App, options: WindowOptions) -> Result<(), Error>;
```

`App` is built on the existing `Application`: misty implements `Application` for an internal `TreeRunner<A: App>`, so the event loop, windows, and frames are unchanged.

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
pub fn text_area<'a, A>(text: &'a str) -> TextArea<'a, A>;
pub fn list<'a, A, S: ListSource>(source: &'a S) -> List<'a, A, S>;

// Containers.
pub fn column<'a, A>(
    children: impl IntoIterator<Item = Element<'a, A>>,
) -> Element<'a, A>;
pub fn row<'a, A>(children: impl IntoIterator<Item = Element<'a, A>>) -> Element<'a, A>;
pub fn form<'a, A>(fields: impl IntoIterator<Item = Field<'a, A>>) -> Element<'a, A>;
pub fn field<'a, A>(label: &'a str, control: impl Into<Element<'a, A>>) -> Field<'a, A>;

impl<'a, A> Slider<'a, A> {
    /// What runs on the app's state when the user moves the slider.
    pub fn on_change(self, apply: impl Fn(&mut A, f64) + 'a) -> Self;
}
```

A control is **controlled**: its value comes from the app's state in `view`, and the only way it changes is through its `on_change`. That removes the `Change` enum and `settle`: the callback is the settle. A control with no `on_change` is read-only and says so to screen readers.

### 4.3 Ids and the state store

Each element has an id. By default it is the element's path in the tree (`form/2/control`), which is stable as long as the tree's shape is. A control whose position can change (a row in a filtered list, a tab) gets an explicit id with `.id("volume")`; ids reuse `FocusId`'s hashing.

The store keeps, per id, the widget object (`misty::Slider`, `misty::Select<T>`, and so on), its last rectangle, and its accessibility subtree id. After each rebuild, entries whose id was not seen are dropped, so a closed dialog forgets its focus and a removed row its caret.

Two elements built with the same explicit id panic in debug builds with a message that says how to fix it:

```text
misty: two elements have the id "volume" (form/3/control and form/5/control).
Give one of them another id with `.id(..)`, or build it once.
```

### 4.4 One rebuild, step by step

```text
┌───────────────────────────────────────────────┐
│ input arrives (key, pointer, screen reader)   │
└───────────────────────┬───────────────────────┘
                        ▼
┌───────────────────────────────────────────────┐
│ view(&app) builds the tree                    │
└───────────────────────┬───────────────────────┘
                        ▼
┌───────────────────────────────────────────────┐
│ store syncs props into each id's widget       │
│ (new ids get a widget, missing ids drop it)   │
└───────────────────────┬───────────────────────┘
                        ▼
┌───────────────────────────────────────────────┐
│ FocusTree dispatches against the rectangles   │
│ the last layout stored; the widget answers    │
└───────────────────────┬───────────────────────┘
                        ▼
┌───────────────────────────────────────────────┐
│ a changed value queues its on_change call;    │
│ an unused key goes to App::key                │
└───────────────────────┬───────────────────────┘
                        ▼
┌───────────────────────────────────────────────┐
│ tree dropped; queued calls run on &mut app    │
└───────────────────────┬───────────────────────┘
                        ▼
┌───────────────────────────────────────────────┐
│ redraw: rebuild, lay out, draw, describe      │
└───────────────────────────────────────────────┘
```

Rebuilding on every input and every frame is affordable because elements are small borrowed structs; the heavy state stays in the store, and big data stays lent. The benchmark harness (`make bench`) gets a tree case to keep this honest.

### 4.5 Layout

Each element carries a `layout::Style` and becomes one taffy node, through the `render/layout.rs` engine that exists. Every control reports its size through one method that replaces today's mismatched `height(metrics)` and `height(metrics, scale_factor)`:

```rust
/// What a control needs to sit in a tree. Implemented by each widget,
/// in its own file.
pub trait Control {
    /// The props an element hands the stored widget each rebuild.
    type Props<'a>;
    /// What a change reports to `on_change`.
    type Value;

    /// Copies `props` into the widget, keeping its own state (caret, scroll).
    fn sync(&mut self, props: Self::Props<'_>);
    /// Its height for `width`, in the frame's metrics.
    fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32;
}
```

`form` lays out a two-column grid, the label column as wide as its widest label. The label is drawn by the form and also becomes the control's accessible name, so `with_label` disappears from the tree API (the widget keeps it for apps without a tree).

### 4.6 Focus, Tab order, traps, the ring

The tree registers every focusable element in `FocusTree` in tree order, so Tab order is reading order. `.order(n)` maps to `FocusTree::set_order` for the rare exception. Containers map to `FocusTree::enter` and `leave`, so keys bubble up the tree. The focused element's widget gets `set_focused(true)` from the store; no app code calls it. The ring is drawn by `FocusTree::draw` after the tree, as now.

An overlay (section 4.8) that declares itself modal opens a trap the frame it first appears and closes it the frame it disappears. The trap calls in the example go away.

### 4.7 Accessibility

After drawing, the store calls each control's existing `access(frame, id)`. A screen reader action names a node id; the store finds the element that owns it (the id or a child of it, as `FocusId::child` already encodes), calls that widget's `access_action`, and turns a `Changed` response into the element's `on_change` call. `Application::access_action` is not written by tree apps at all.

Elements without a widget describe themselves from their kind: `text` as a label, `form` as a group, `field` as labelling its control.

### 4.8 Overlays

```rust
impl<'a, A> Element<'a, A> {
    /// Draws `layer` over everything at the end of the frame, anchored to
    /// this element; `modal` traps the focus inside it while it shows.
    pub fn overlay(self, layer: Overlay<'a, A>) -> Self;
}
```

The select's open list, a menu, a tooltip, and toasts are overlays the control declares itself, from its stored state: the `select` adapter adds a modal overlay while its widget `is_open()`. A dialog is an overlay the app declares from its own state:

```rust
column(..).overlay_if(self.confirming, dialog("Unsaved changes", body)
    .button("Discard", |app: &mut Self| app.close(false))
    .button("Save", |app: &mut Self| app.close(true)))
```

Overlays draw in paint order after the base layer, so "draw the select last" is no longer the app's job.

### 4.9 Wrapping existing widgets

Each widget keeps its file and its `Widget` impl, and gains an `impl Control` plus one builder in the `view` module. The builder wraps `Widget::handle`, `Widget::draw`, `access`, and `access_action`, and maps the widget's response to the value `on_change` gets. A widget that does not have an adapter yet can still be placed in a tree with an escape hatch:

```rust
/// A widget the tree lays out and focuses but the app drives: misty hands
/// it inputs and a rect, and gives its responses to `respond`.
pub fn custom<'a, A, W: Widget + 'static>(
    id: &'static str,
    make: impl FnOnce() -> W,
    respond: impl Fn(&mut A, W::Response) + 'a,
) -> Element<'a, A>;
```

That makes migration incremental: an app can move to `App` before every widget has an adapter.

### 4.10 Testing a tree

```rust
let mut window = TreeWindow::new(Demo::default(), 720, 560);
window.focus(Role::Slider, "Volume");
window.press("<right> <right>");
assert_eq!(window.app().volume, 50.0);
window.click(Role::ComboBox, "Chrome font");
window.press("<down> <cr>");
assert_eq!(window.app().theme.chrome_text, ChromeText::Proportional);
assert_eq!(window.ui_text(), include_str!("controls.ui.txt"));
```

`TreeWindow` builds on `TestWindow`: it owns the app, rebuilds and lays out on each call, finds elements by role and accessible name (panicking with the list of names it did find when there is no match), and reads `ui_text` from the frame it drew. Every interaction in the example becomes a few lines like these, with no display.

### 4.11 Layers

- **`Control` trait:** in `app`, since widgets implement it and they sit above `app`.
- **`impl Control for Slider` and the rest:** in `widgets` and `chrome`, next to each widget, as `access` is today.
- **`Element`, builders, store, `TreeWindow`:** in a new `view` layer above `chrome` and below `runtime`, since it names every widget.
- **`App`, `run_app`:** in `runtime`, since it drives the event loop.

`make layers` gains the `view` folder; `view` stays free of winit like `widgets` and `chrome`.

## 5. The Controls Example in the Proposed API

```rust
//! The form controls in one window (see the module doc of the current
//! example for the keys).

use misty::view::{
    checkbox, column, field, form, progress, radio, select, slider, stepper, text,
    text_area,
};
use misty::{App, ChromeText, Element, NumberRange, Theme, WindowOptions};

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
    status: String,
    tab_width: f64,
    theme: Theme,
    volume: f64,
    wrap: bool,
}

impl App for Demo {
    fn view(&self) -> Element<'_, Self> {
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
                field("Notes", text_area(&self.notes).rows(6)
                    .on_change(|app: &mut Self, notes| app.notes = notes)),
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

What went away: the ten `FocusId` constants, the `Controls` struct, the `Change` enum and `settle`, `input`, `access_action`, `about_to_wait` (the tree follows the text area's caret for the input method and asks for the progress bar's next frame itself), the whole of `render`, and the trap calls. Ctrl+Q moves into `App::key`. The example should land near 120 lines, from 467, and every remaining line is about the app.

## 6. Migration Plan

Each phase ships on its own and leaves the low-level `Application` path working.

1. **One control surface.** Add the `Control` trait in `app` and implement it for the nine form controls, replacing the `height` variants with `measure`. Ships: nothing visible to apps without a tree; the `height` inconsistency is gone. Verified: `make check`, `make test`; every widget's existing tests untouched.

2. **Tree core.** The `view` layer with `Element`, `column`, `row`, `form`, `field`, `text`, the store, layout through `render/layout.rs`, automatic focus and accessibility, `App` and `run_app`, and `TreeWindow`. Ships: the controls example rewritten, without the select. Verified: the rewritten example's `ui_text` matches the old example's output line for line (a snapshot captured before the rewrite), plus `TreeWindow` tests for each interaction in the example.

3. **Overlays.** `overlay`, automatic traps, and adapters for `Select`, `Menu`, `Tooltip`, `Toasts`, and `Dialog`. Ships: the full controls example and the dialog example as trees. Verified: tests that a trap opens and closes with the overlay and that Esc and outside clicks behave as in the widgets' own tests.

4. **Lent data widgets.** Adapters for `List`, `Table`, `TreeView`, `IconGrid`, `QuickPick`, and `ScrollView` holding arbitrary children. Ships: the list, table, tree, and grid examples as trees. Verified: `make bench` shows no regression on the heaviest views; snapshot equality as in phase 2.

5. **Chrome and settings.** `TitleBar`, `TabStrip`, `SplitView`, `StatusBar` as tree elements, and `SettingsPage` rebuilt from form elements. Ships: a whole frameless window described by one `view`. Verified: a frameless chrome example (title bar with its menu, tab strip, context menu, which-key) ported to one `view`.

6. **Documentation.** The README and crate docs lead with `App`; the low-level path is documented as the escape hatch. Feeds the planned `llms.txt`.

## 7. Risks

- **Rebuild cost.** A tree of thousands of elements rebuilt per input could cost more than today's hand-written frame. Lent data keeps large lists as one element each; the bench harness gates every phase from 2 on.
- **Implicit ids.** A path id changes when an element is inserted before it, and the widget silently loses its state (a caret jumps). Mitigation: stateful controls inside a dynamic container (a list, a conditional) panic in debug builds without an explicit id, with a message naming the container.
- **Two ways to build an app.** `Application` and `App` coexist, which strains "one obvious way". Mitigation: docs present `App` first and `Application` only for custom drawing, and `custom` lets a tree app embed hand-driven widgets instead of dropping to `Application`.
- **Generic callbacks.** `Fn(&mut A, V)` boxed per element adds an allocation per callback per rebuild. Acceptable at form sizes; revisit with an arena if the bench says so.

## 8. Open Questions for the Owner

1. **Callbacks or messages?** This proposal uses closures on `&mut App` (`on_change(|app, v| ..)`). An Elm-style `Msg` enum with `update(&mut self, msg)` is more testable in isolation, but it brings back exactly the `Change` enum the example wants gone. Recommended: closures.

2. **Should `Application` stay public** once phase 5 lands, or move behind a feature or a `misty::low_level` module to keep one obvious way?

3. **Controlled only?** The proposal makes every control's value come from app state. An uncontrolled mode (the widget owns its value, the app reads it on demand) is simpler for scratch fields but doubles each adapter's surface. Recommended: controlled only, with the store keeping view state (caret, scroll) as now.

4. **Text area value type.** Syncing a large `String` into the stored `TextArea` every rebuild is a full compare. Should `text_area` take a version counter or a rope from the app instead of `&str`?

5. **Multiple windows.** `App::view` describes one window. Should `view` take a `WindowId` (one app, many windows, as `Application` allows today), or should each window be its own `App`?

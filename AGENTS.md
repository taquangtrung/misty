# Misty: Agent Guide

## Library rules

- Misty is a generic GUI framework for any Rust desktop app, designed keyboard-first. A widget belongs here only when it is generic; anything that names a terminal, a file, or one app's command belongs in the app that uses misty.
- Design for apps written by AI agents: one obvious way to do each thing, error messages that say how to fix the cause, and every widget testable headlessly through `misty::testing`.
- Misty owns its key values (`Key`, `KeyCode`, `Mods`, and the key notation read by `parse_key`) and resolves no key presses: turning keys into commands (prefixes, timeouts, layers) is the app's, with the keymap library of its choice. `misty` depends on no keymap library or editor; an app adapts its own to misty.
- `unsafe_code` is denied workspace-wide; an exception needs a narrow `#[allow]` with a `// SAFETY:` comment.
- Colors and box shapes come from the theme only (its color tokens and its `Shape`), never literals in widget code.
- Words misty itself shows or names for screen readers (button names, hints, placeholders) come from the theme's `Strings` (`theme.strings`), never literals in widget code, so an app translates them in one place.
- Chrome labels (titles, tab names, status segments, button and menu captions, headers, tooltips, toasts) are measured and drawn through `widgets/label.rs` (`label_width` and `draw_label` for plain text, `styled_label_width`, `styled_label_cells`, `fit_label`, and `draw_styled_label` for `StyledText`; whatever is drawn through it is measured through it too), so they follow the theme's `ChromeText`: buttons, checkboxes, radios, selects, steppers, breadcrumbs, menus, floating panels, tooltips, toasts, dialog titles, table column headers, the settings page's and quick pick's chrome, and the tab strip, title bar, and status bar do. Widgets whose text must line up on the cell grid stay on cells (`Frame::draw_plain_text`, `Frame::draw_text`): transients, which-key popups, hints, completion and drop lists, info cards, typed text and its prompt or placeholder (a caret is placed by cells), and content views (list rows, code, hex, grid), because a proportional font would break their columns.
- Library crates use `thiserror` for errors; classify errors by variant, never by message text.
- Every public item is documented (`missing_docs` is denied); docs are written for someone who has never seen misty or an app built on it.
- Keep the public API small: an item is public only when a consumer needs it.

## Modules

`misty`'s source is grouped in layers, lowest first, each a folder of `src`: `base` (value types that need no GPU or window: colors, box styles, cells, themes, geometry, keys with their notation and labels, split panes, list state, text measured in cells and styled text, fuzzy matching, scroll easing, syntax colors, the OS appearance as a value, pointer shapes, the theme's built-in words), `platform` (clipboard, errors, resize edges, the waker, redraw schedule, window ids, reading the OS appearance), `render` (GPU, surfaces, the draw list, fonts, text and paragraph layout, pictures, flexbox layout, the renderer), `input` (the `Input` events and keys), `app` (`Application`, `AppContext`, `Frame`, `EventCx`, the `Widget` contract, the focus tree and dispatch, accessibility, IME focus, window options and geometry, file dialogs), `widgets`, `chrome` (title bar, tab strip, split view, status bar), and `runtime` (the event loop). A module imports only from its own layer and those below; `make layers` (part of `make check`) checks it, keeps `winit` out of `widgets` and `chrome`, and keeps the GPU, window, and other GUI crates out of `base`. The public API is re-exported flat from `lib.rs`, each item at one path: only `picker`, `layout`, and `testing` stay public namespaces.

## Checks

Decide what to run by the risk of the change:

- Cosmetic edit (docs, comments, renames inside one function): run nothing.
- Structural edit (moving modules, changing signatures): `cargo check` right away.
- Behavioral edit: `cargo test -p <crate>` for the crate touched.
- Before calling a task done or committing: `make check` and `make test`.

## Commits

Semantic one-line messages: `<type>(<scope>): <description>` (`feat`, `fix`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`).

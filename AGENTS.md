# Misty: agent guide

Misty is a GPU-rendered, keyboard-first GUI framework for Rust desktop apps. Keep it generic, predictable, and testable.

## Scope

- Add only generic widgets. Anything naming a terminal, an editor, or one app's command belongs in that app.
- Put desktop utilities (file dialogs, file pickers) behind an opt-in feature, built on the public API.
- Own key values (`Key`, `KeyCode`, `Mods`, `parse_key`) but resolve no key presses: keymaps are the app's.

## API

- One obvious way per task; make an item public only when an app needs it.
- Re-export every public item flat from `lib.rs`. Only `picker`, `layout`, `testing`, and `view` are namespaces.
- Document every public item for a newcomer (`missing_docs` is denied).
- Errors: `thiserror`, matched by variant, never by text; each message says how to fix its cause.
- No `unsafe`, except a narrow `#[allow]` with a `// SAFETY:` comment.

## Theme and text

- Colors, shapes, and emphasis come from the theme only (`Shape`, `Emphasis`, `Span::emphasized`); no color literals, no `Span::bold` in widget code.
- Words misty shows or names for screen readers come from `theme.strings`.
- Measure and draw the labels of controls and bars through `widgets/label.rs`, so they follow `LabelFont`.
- Text that lines up in columns (list rows, code, hex, grids, typed text) stays on the cell grid. With `with_proportional_font`, place carets and selections through `ParagraphLayout`, never cell math.

## Interaction

- Every widget works from the keyboard and is described to screen readers.
- Light the part under the pointer (`hover` color or `ButtonFace` tint); redraw only when it changes.
- Pointer shapes as the desktop: I-beam over editable text, resize over handles, hand over links, arrow elsewhere.
- Test each widget headlessly through `misty::testing`.

## Layers

Lowest first: `base`, `platform`, `render`, `input`, `app`, `widgets`, `view`, `runtime`. Import only from your layer or below; no `winit` in `widgets` or `view`; no GPU or window crates in `base`. `make layers` checks it.

## Checks

- Cosmetic edit: nothing. Structural: `cargo check`. Behavioral: `cargo test -p misty`.
- Before done or a commit: `make check` and `make test`.

## Changelog and commits

- Leave `CHANGELOG.md` alone before 0.1.0. After, one line per user-visible change; breaking changes say what to use instead.
- Commit messages: one line, `<type>(<scope>): <description>` (`feat`, `fix`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`).

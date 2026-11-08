# Changelog

All notable changes to misty are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

While misty is at `0.0.x`, any release may change the public API; each such change is listed here under **Changed** or **Removed** with what to use instead. A major version bump of `wgpu` or `accesskit`, which misty re-exports, is a misty breaking change.

## [Unreleased]

## [0.0.1]

First public release.

### Added

- Window runtime: several windows, frameless windows with misty's own title bar, tabs, split panes, and a status bar.
- The `Widget` contract with headless testing through `misty::testing`, focus through `FocusTree`, and animations through `Widget::about_to_wait`.
- Widgets: lists, tables, trees, text fields and areas, menus, dialogs, a command palette, which-key hints, toasts, a settings page, and form controls.
- Themes with KDL theme files, and every built-in word translatable through `Strings`.
- Optional features: `dialogs`, `global-hotkey`, `highlight`, `html`, `icon-theme`, `kdl`, `markdown`, `notify`, `serde`, `single-instance`, `testing`, and `tray`.

[Unreleased]: https://github.com/taquangtrung/misty/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/taquangtrung/misty/releases/tag/v0.0.1

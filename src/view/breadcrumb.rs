//! [`breadcrumb`]: where a view is, as a row of crumbs (a location's
//! segments) each going there, as an element over the [`Breadcrumb`]
//! widget, for the header of a pane or a page.
//!
//! [`Breadcrumb`]: crate::Breadcrumb

use std::rc::Rc;

use super::super::app::access::AccessAction;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::super::widgets::breadcrumb::{self, BreadcrumbProps, BreadcrumbResponse};
use super::controls::{Callback, call};
use super::element::{Element, Kind};
use super::node::{Call, Drawn, Spec};

// ========================================================================
// Data Structures
// ========================================================================

/// Where a view is, as a row of crumbs, as an element. Made by
/// [`breadcrumb`].
pub struct Breadcrumb<A> {
    /// What going to a segment runs, with its index.
    on_navigate: Option<Callback<A, usize>>,
    /// The segments' labels, first to last.
    segments: Vec<String>,
}

// ========================================================================
// Breadcrumb Implementations
// ========================================================================

impl<A> Breadcrumb<A> {
    /// What runs on the app's state with a segment's index (0 the first)
    /// when the user goes to it, by its crumb or from the menu of the
    /// crumbs that do not fit.
    pub fn on_navigate(self, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        Self {
            on_navigate: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for Breadcrumb<A> {
    type Widget = breadcrumb::Breadcrumb;

    const KIND: &'static str = "breadcrumb";

    fn create(&self) -> breadcrumb::Breadcrumb {
        breadcrumb::Breadcrumb::new(Vec::new())
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> BreadcrumbProps<'b> {
        BreadcrumbProps {
            segments: &self.segments,
        }
    }

    fn call(&self, index: usize) -> Option<Call<A>> {
        call(self.on_navigate.as_ref(), index)
    }

    /// Draws the crumbs in `rect` and, while open, the menu of the crumbs
    /// that do not fit in the frame's overlay: the popup the store traps
    /// the focus in.
    fn draw(
        &self,
        widget: &mut breadcrumb::Breadcrumb,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        let layer = widget.draw(frame, rect, theme);
        Drawn {
            hit: rect,
            layer,
            over: None,
        }
    }

    fn describe(&self, widget: &breadcrumb::Breadcrumb, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut breadcrumb::Breadcrumb,
        id: FocusId,
        action: AccessAction,
    ) -> Option<BreadcrumbResponse> {
        Some(widget.access_action(id, action))
    }

    fn has_layer(widget: &breadcrumb::Breadcrumb) -> bool {
        widget.is_menu_open()
    }
}

impl<'a, A: 'static> From<Breadcrumb<A>> for Element<'a, A> {
    fn from(breadcrumb: Breadcrumb<A>) -> Self {
        Element::new(Kind::Control(Box::new(breadcrumb)))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where a view is, as a row of `segments` (a folder's path split into its
/// folders, a page's parents), first to last, each a flat crumb going
/// there: a click, or Left and Right (`h`, `l`) then Enter while it has
/// the keyboard, runs [`Breadcrumb::on_navigate`] with its index. Crumbs
/// that do not fit collapse into an ellipsis opening a menu of them,
/// keeping the first and the last. Splitting a location into segments and
/// going there is the app's.
///
/// ```
/// use misty::view::{breadcrumb, column, text};
/// use misty::{App, Element, Theme, WindowId};
///
/// struct Files {
///     folder: Vec<String>,
///     theme: Theme,
/// }
///
/// impl App for Files {
///     fn view(&self, _window: WindowId) -> Element<'_, Self> {
///         column([
///             breadcrumb(self.folder.clone())
///                 .on_navigate(|app: &mut Self, index| app.folder.truncate(index + 1))
///                 .into(),
///             text("the folder's files").into(),
///         ])
///     }
///
///     fn theme(&self) -> &Theme {
///         &self.theme
///     }
/// }
/// ```
pub fn breadcrumb<A>(segments: impl IntoIterator<Item = impl Into<String>>) -> Breadcrumb<A> {
    Breadcrumb {
        on_navigate: None,
        segments: segments.into_iter().map(Into::into).collect(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use accesskit::Role;

    use super::*;
    use crate::platform::window_id::WindowId;
    use crate::testing::TreeWindow;
    use crate::view::app::App;
    use crate::view::element::column;

    struct Folders {
        folder: Vec<String>,
        theme: Theme,
    }

    impl App for Folders {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            column([breadcrumb(self.folder.clone())
                .on_navigate(|app: &mut Self, index| app.folder.truncate(index + 1))
                .into()])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    fn window(folder: &[&str]) -> TreeWindow<Folders> {
        let app = Folders {
            folder: folder.iter().map(|name| (*name).to_owned()).collect(),
            theme: Theme::default(),
        };
        TreeWindow::new(app, "Folders", 800, 200)
    }

    #[test]
    fn test_a_crumb_clicked_runs_on_navigate_with_its_index() {
        let mut window = window(&["home", "me", "papers"]);
        window.click(Role::Link, "me");
        assert_eq!(window.app().folder, ["home", "me"]);
        assert!(
            !window.ui_text().contains("papers"),
            "the crumbs follow the app"
        );
    }

    #[test]
    fn test_the_keyboard_moves_between_crumbs_and_enter_goes_there() {
        let mut window = window(&["home", "me", "papers"]);
        window.press("<tab>");
        window.press("<left> <left> RET");
        assert_eq!(window.app().folder, ["home"]);
    }
}

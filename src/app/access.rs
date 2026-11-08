//! Accessibility: the tree of what a window shows that screen readers
//! and other assistive technologies read, through AccessKit (see
//! [`AccessAction`] for how it is built).

use std::collections::HashSet;

use accesskit::{
    Action, ActionRequest, Node, NodeId, Rect as AccessRect, Role, TreeId, TreeInfo, TreeUpdate,
};

use super::super::base::geometry::Rect;
use super::focus::FocusId;

// ========================================================================
// Constants
// ========================================================================

/// The id of each window's root node.
const WINDOW_NODE: FocusId = FocusId::new("misty.window");

/// The toolkit name the tree reports.
const TOOLKIT_NAME: &str = "misty";

/// The toolkit version the tree reports.
const TOOLKIT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ========================================================================
// Data Structures
// ========================================================================

/// What an assistive technology asks of a widget.
///
/// # The accessibility tree
///
/// Screen readers and other assistive technologies read what a window
/// shows through AccessKit, as a tree. The tree is built frame by frame, as the focus tree is. While
/// rendering, widgets describe themselves into the frame: each widget
/// with an `access` method (the title bar, tab strip, list, table, tree
/// view, icon grid, menu, dialog, text field, and editor view) adds its node and its parts' nodes under the
/// [`FocusId`] the app gives it, and the app adds any other node with
/// [`Frame::access`](crate::Frame::access) (a paragraph of text, say),
/// nesting nodes with
/// [`Frame::enter_access`](crate::Frame::enter_access).
/// [`FocusTree::draw`](crate::FocusTree::draw) adds the widgets that
/// take the focus and says which has it; a
/// focusable widget that described itself no further is a plain group.
/// The runtime sends the tree to the window's AccessKit adapter after
/// each frame while an assistive technology listens, only when it
/// changed.
///
/// What a screen reader asks of the window comes back to the app as an
/// [`AccessAction`] through
/// [`Application::access_action`](crate::Application::access_action):
/// focus a widget, or click (do the default action of) one. Widgets with
/// parts answer clicks on them with their own `access_action` methods.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum AccessAction {
    /// Do the default action of the node with this id: press a button,
    /// choose a menu item, select a tab.
    Click(FocusId),
    /// Lower the value of the node with this id by one step: a slider, a
    /// number field.
    Decrement(FocusId),
    /// Give the keyboard to the widget with this id: the focusable widget
    /// nearest the node asked for, itself or a widget around it.
    Focus(FocusId),
    /// Raise the value of the node with this id by one step.
    Increment(FocusId),
}

/// The accessibility tree of one frame, as widgets and the app describe
/// it while rendering.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AccessTree {
    /// The nodes described, in the order described.
    entries: Vec<AccessEntry>,
    /// The widgets that take the focus, from the focus tree.
    focusables: Vec<Focusable>,
    /// The widget with the keyboard, from the focus tree.
    focused: Option<FocusId>,
    /// The part of a widget that has the focus while the widget does (a
    /// dialog's focused button), by widget.
    inner_focus: Vec<(FocusId, FocusId)>,
    /// The nodes open while describing, innermost last.
    stack: Vec<FocusId>,
}

/// One node described.
#[derive(Clone, Debug, PartialEq)]
struct AccessEntry {
    /// The node's id.
    id: FocusId,
    /// The node, without its bounds and children, which the tree sets.
    node: Node,
    /// The node it was described in, if any.
    parent: Option<FocusId>,
    /// Where it was drawn, in physical pixels.
    rect: Rect,
}

/// A widget that takes the focus, as the focus tree knows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Focusable {
    /// Its id.
    pub(crate) id: FocusId,
    /// The focus container it was registered in, if any.
    pub(crate) parent: Option<FocusId>,
    /// Where it was drawn, in physical pixels.
    pub(crate) rect: Rect,
}

// ========================================================================
// AccessTree Implementations
// ========================================================================

impl AccessTree {
    /// Adds `node` as `id`, drawn in `rect`, in the node open now.
    pub(crate) fn add(&mut self, id: FocusId, rect: Rect, node: Node) {
        let parent = self.stack.last().copied();
        self.entries.push(AccessEntry {
            id,
            node,
            parent,
            rect,
        });
    }

    /// Adds `node` as `id`, drawn in `rect`, and opens it: the nodes added
    /// until [`AccessTree::leave`] are its children.
    pub(crate) fn enter(&mut self, id: FocusId, rect: Rect, node: Node) {
        self.add(id, rect, node);
        self.stack.push(id);
    }

    /// Closes the node [`AccessTree::enter`] opened last.
    pub(crate) fn leave(&mut self) {
        self.stack.pop();
    }

    /// Takes the widgets that take the focus and the one that has it.
    pub(crate) fn set_focus(&mut self, focusables: Vec<Focusable>, focused: Option<FocusId>) {
        self.focusables = focusables;
        self.focused = focused;
    }

    /// Says `part` has the focus whenever the widget `widget` does: the
    /// focused button of a dialog, the highlighted item of a menu.
    pub(crate) fn set_inner_focus(&mut self, widget: FocusId, part: FocusId) {
        self.inner_focus.push((widget, part));
    }

    /// The focusable widget nearest the node `id`: itself, or the first
    /// around it that takes the focus.
    pub(crate) fn focus_target(&self, id: FocusId) -> Option<FocusId> {
        let mut current = Some(id);
        while let Some(id) = current {
            if self.focusables.iter().any(|focusable| focusable.id == id) {
                return Some(id);
            }
            current = self
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.parent)
                .or_else(|| {
                    self.focusables
                        .iter()
                        .find(|focusable| focusable.id == id)
                        .and_then(|focusable| focusable.parent)
                });
        }
        None
    }

    /// The whole tree as one AccessKit update, under a window node named
    /// `title`: each node with its bounds and children, a focusable one
    /// able to take the focus, a widget described at the top moved into
    /// its focus container, a node whose parent is gone moved to the
    /// window, and the focus on the focused widget (on the window when
    /// none is).
    pub(crate) fn tree_update(&self, title: &str) -> TreeUpdate {
        let mut seen = HashSet::new();
        let mut flat: Vec<(FocusId, Option<FocusId>, Node, Rect)> = Vec::new();
        let focus_parent = |id: FocusId| {
            self.focusables
                .iter()
                .find(|focusable| focusable.id == id)
                .and_then(|focusable| focusable.parent)
        };
        for entry in &self.entries {
            if seen.insert(entry.id) {
                // A widget described at the top sits in its focus container.
                let parent = entry.parent.or_else(|| focus_parent(entry.id));
                flat.push((entry.id, parent, entry.node.clone(), entry.rect));
            }
        }
        for focusable in &self.focusables {
            if seen.insert(focusable.id) {
                let node = Node::new(Role::Group);
                flat.push((focusable.id, focusable.parent, node, focusable.rect));
            }
        }
        let focusable: HashSet<FocusId> = self.focusables.iter().map(|item| item.id).collect();
        let parent_of = |parent: Option<FocusId>| parent.filter(|parent| seen.contains(parent));
        let children_of = |id: Option<FocusId>| -> Vec<NodeId> {
            flat.iter()
                .filter(|(child, parent, _, _)| parent_of(*parent) == id && Some(*child) != id)
                .map(|(child, _, _, _)| node_id(*child))
                .collect()
        };
        let mut nodes: Vec<(NodeId, Node)> = flat
            .iter()
            .map(|(id, _, node, rect)| {
                let mut node = node.clone();
                node.set_bounds(access_rect(*rect));
                let children = children_of(Some(*id));
                if !children.is_empty() {
                    node.set_children(children);
                }
                if focusable.contains(id) {
                    node.add_action(Action::Focus);
                }
                (node_id(*id), node)
            })
            .collect();
        let mut window = Node::new(Role::Window);
        window.set_label(title);
        window.set_children(children_of(None));
        nodes.push((node_id(WINDOW_NODE), window));
        let inner = |focused: FocusId| {
            self.inner_focus
                .iter()
                .find(|(widget, part)| *widget == focused && seen.contains(part))
                .map_or(focused, |(_, part)| *part)
        };
        let focus = self
            .focused
            .filter(|focused| seen.contains(focused))
            .map_or(WINDOW_NODE, inner);
        TreeUpdate {
            focus: node_id(focus),
            nodes,
            tree: Some(TreeInfo {
                root: node_id(WINDOW_NODE),
                toolkit_name: Some(TOOLKIT_NAME.to_owned()),
                toolkit_version: Some(TOOLKIT_VERSION.to_owned()),
            }),
            tree_id: TreeId::ROOT,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What `request` asks of the app, for the actions misty passes on
/// (click, focus, increment, and decrement); `None` for the rest. A focus request goes to the
/// focusable widget nearest the node in `tree`.
pub(crate) fn access_action(request: &ActionRequest, tree: &AccessTree) -> Option<AccessAction> {
    let id = FocusId::from_raw(request.target_node.0);
    match request.action {
        Action::Click => Some(AccessAction::Click(id)),
        Action::Decrement => Some(AccessAction::Decrement(id)),
        Action::Focus => tree.focus_target(id).map(AccessAction::Focus),
        Action::Increment => Some(AccessAction::Increment(id)),
        _ => None,
    }
}

/// The AccessKit id of the node `id`.
pub(crate) fn node_id(id: FocusId) -> NodeId {
    NodeId(id.get())
}

/// The node `id` of `update`, if it has one.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn find_node(update: &TreeUpdate, id: FocusId) -> Option<&Node> {
    let id = node_id(id);
    update
        .nodes
        .iter()
        .find(|(node, _)| *node == id)
        .map(|(_, node)| node)
}

/// `rect` as AccessKit bounds.
fn access_rect(rect: Rect) -> AccessRect {
    AccessRect {
        x0: f64::from(rect.x),
        x1: f64::from(rect.x + rect.width),
        y0: f64::from(rect.y),
        y1: f64::from(rect.y + rect.height),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const TABS: FocusId = FocusId::new("tabs");
    const EDITOR: FocusId = FocusId::new("editor");
    const PANEL: FocusId = FocusId::new("panel");

    /// The node `id` of `update`.
    fn node(update: &TreeUpdate, id: FocusId) -> &Node {
        find_node(update, id).expect("the node is in the tree")
    }

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 10.0, 10.0)
    }

    /// A tree describing a tab list with one tab, an undescribed panel
    /// holding a described editor, both focusable, the editor focused.
    fn tree() -> AccessTree {
        let mut tree = AccessTree::default();
        tree.enter(TABS, rect(), Node::new(Role::TabList));
        let mut tab = Node::new(Role::Tab);
        tab.set_label("one");
        tree.add(TABS.child(0), rect(), tab);
        tree.leave();
        tree.add(EDITOR, rect(), Node::new(Role::MultilineTextInput));
        let focusable = |id, parent| Focusable {
            id,
            parent,
            rect: rect(),
        };
        tree.set_focus(
            vec![
                focusable(TABS, None),
                focusable(PANEL, None),
                focusable(EDITOR, Some(PANEL)),
            ],
            Some(EDITOR),
        );
        tree
    }

    #[test]
    fn test_described_nodes_keep_roles_and_names_under_the_window() {
        let update = tree().tree_update("demo");
        let window = node(&update, WINDOW_NODE);
        assert_eq!(window.role(), Role::Window);
        assert_eq!(window.label(), Some("demo"));
        assert_eq!(
            window.children(),
            [node_id(TABS), node_id(PANEL)],
            "the editor moved into its focus container"
        );
        assert_eq!(node(&update, TABS).children(), [node_id(TABS.child(0))]);
        assert_eq!(node(&update, TABS.child(0)).label(), Some("one"));
        assert_eq!(node(&update, PANEL).role(), Role::Group, "undescribed");
        assert_eq!(node(&update, PANEL).children(), [node_id(EDITOR)]);
        assert_eq!(
            update.tree.map(|tree| tree.root),
            Some(node_id(WINDOW_NODE))
        );
    }

    #[test]
    fn test_focusable_nodes_take_focus_and_the_focused_one_is_reported() {
        let update = tree().tree_update("demo");
        assert_eq!(update.focus, node_id(EDITOR));
        assert!(node(&update, EDITOR).supports_action(Action::Focus));
        let mut inner = tree();
        inner.set_inner_focus(TABS, TABS.child(0));
        inner.set_focus(Vec::new(), Some(TABS));
        assert_eq!(
            inner.tree_update("demo").focus,
            node_id(TABS.child(0)),
            "a widget's focused part takes its focus"
        );
        assert!(!node(&update, TABS.child(0)).supports_action(Action::Focus));
        let mut unfocused = tree();
        unfocused.set_focus(Vec::new(), Some(FocusId::new("gone")));
        assert_eq!(
            unfocused.tree_update("demo").focus,
            node_id(WINDOW_NODE),
            "a focus on no node falls back to the window"
        );
    }

    #[test]
    fn test_a_focus_request_on_a_part_goes_to_its_focusable_widget() {
        let tree = tree();
        assert_eq!(tree.focus_target(TABS.child(0)), Some(TABS));
        assert_eq!(tree.focus_target(EDITOR), Some(EDITOR));
        assert_eq!(tree.focus_target(FocusId::new("nowhere")), None);
        let request = ActionRequest {
            action: Action::Focus,
            data: None,
            target_node: node_id(TABS.child(0)),
            target_tree: TreeId::ROOT,
        };
        assert_eq!(
            access_action(&request, &tree),
            Some(AccessAction::Focus(TABS))
        );
        let click = ActionRequest {
            action: Action::Click,
            ..request
        };
        assert_eq!(
            access_action(&click, &tree),
            Some(AccessAction::Click(TABS.child(0)))
        );
    }
}

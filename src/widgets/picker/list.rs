//! The one-call way to build a picker that is a fixed list with one thing to
//! do on a row: the open windows, Open With, a question with a few answers.

use super::row::{MatchOn, Row, RowKey};
use super::source::{Action, ActionId, Feed, Matching, Resume, Source};

// ========================================================================
// Constants
// ========================================================================

/// The one action of a list, run by `Enter`.
const CHOOSE: ActionId = ActionId("choose");

// ========================================================================
// Data Structures
// ========================================================================

/// A [`Source`] of a fixed list of rows that each do one thing.
pub struct FixedSource<O> {
    /// What choosing a row does.
    choose: Box<dyn FnMut(RowKey) -> Option<O> + Send>,
    /// What the action menu calls the one action.
    choose_title: String,
    /// The picker's name.
    id: &'static str,
    /// The fields the query meets.
    match_on: MatchOn,
    /// The rows, until they are fed.
    rows: Option<Vec<Row>>,
    /// What the picker is headed by.
    title: String,
}

// ========================================================================
// FixedSource Implementations
// ========================================================================

impl<O> FixedSource<O> {
    /// A list named `id`, headed by `title`, of `rows`, where choosing a row
    /// does what `choose` returns for its key; `choose_title` names that
    /// one action, as the action menu shows it (`Open`, `Choose`).
    pub fn new(
        id: &'static str,
        title: impl Into<String>,
        rows: Vec<Row>,
        choose_title: impl Into<String>,
        choose: impl FnMut(RowKey) -> Option<O> + Send + 'static,
    ) -> Self {
        Self {
            choose: Box::new(choose),
            choose_title: choose_title.into(),
            id,
            match_on: MatchOn::LABEL,
            rows: Some(rows),
            title: title.into(),
        }
    }

    /// The same list, its query matched against the fields `on` names.
    pub fn match_on(self, on: MatchOn) -> Self {
        Self {
            match_on: on,
            ..self
        }
    }
}

impl<O, P> Source<O, P> for FixedSource<O> {
    fn id(&self) -> &'static str {
        self.id
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn matching(&self) -> Matching {
        Matching::Words(self.match_on)
    }

    fn resume(&self) -> Resume {
        Resume::Never
    }

    fn feed(&mut self, _query: &str) -> Feed {
        Feed::Fixed(self.rows.take().unwrap_or_default())
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        vec![Action {
            button: None,
            id: CHOOSE,
            title: self.choose_title.clone(),
        }]
    }

    fn run(&mut self, _action: ActionId, row: RowKey, _query: &str) -> Option<O> {
        (self.choose)(row)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_list_source_runs_its_one_action_on_the_chosen_row() {
        let rows = vec![Row::new(RowKey(1), "one"), Row::new(RowKey(2), "two")];
        let mut list = FixedSource::new("panes", "Panes", rows, "Go", |key| Some(key.0 * 10));
        let source: &mut dyn Source<u64> = &mut list;
        let actions = source.actions(RowKey(2));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, "Go", "titled as the app named it");
        assert_eq!(source.run(actions[0].id, RowKey(2), ""), Some(20));
    }
}

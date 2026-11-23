//! Misty's notification engine: what an app reports, how long each notice
//! is shown and where, what is kept afterwards, and the long tasks under
//! way: plain data, needing no window, so notices are made and posted
//! anywhere in an app.
//!
//! One [`Notifier`] owns the whole lifecycle. A [`Notice`] posted to it goes
//! two ways: onto the [`ToastStack`], shown for a few seconds, and (unless
//! it says otherwise) into the [`NoticeLog`], the session's history an
//! unread count is taken from. Where a toast shows is the notifier's
//! [`Routing`]: as a card in a corner (misty's `Toasts` draws them), or as a
//! message in place of the status bar's text (misty's `StatusBar` draws the
//! newest). A [`Progress`] task started on it runs until finished: an item
//! on the status bar, or a card with a bar filling as it goes and, when it
//! may be stopped, a Cancel button.
//!
//! Everything here is plain data driven by a clock the caller passes in (or
//! [`Notifier::notify`] reads), so it is tested without a window; the
//! widgets only draw it and hand it the pointer.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::clock;
use super::color::Rgb;
use super::strings::{self, Strings};
use super::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// How many notifications the log keeps before the oldest go.
const LOG_LIMIT: usize = 200;

/// How many toasts are shown at once; a new one pushes out the oldest.
const TOAST_LIMIT: usize = 3;

/// How long a confirmation is shown.
const INFO_DURATION: Duration = Duration::from_secs(3);

/// How long a refusal is shown: a little longer than a confirmation, since
/// it says why a key did nothing.
const REFUSAL_DURATION: Duration = Duration::from_secs(4);

/// How long a warning is shown: long enough to be read in full.
const WARNING_DURATION: Duration = Duration::from_secs(6);

/// How long an error is shown: the longest, since it is the one that matters.
const ERROR_DURATION: Duration = Duration::from_secs(8);

/// The severity from which a notice is a card by default.
const DEFAULT_CARDS_FROM: Severity = Severity::Warning;

// ========================================================================
// Data Structures
// ========================================================================

/// How much a notice matters, which decides its color, how long it is shown,
/// and whether it counts on the bell until it is looked at.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// A confirmation of something done: `pushed`, `copied`.
    Info,
    /// A command that had nothing to act on: `nothing to undo`, `not found`.
    /// Not a failure: said in passing, never kept, never a dialog.
    Refusal,
    /// Something worth knowing that is not a failure.
    Warning,
    /// Something that failed.
    Error,
}

/// Something to tell the user, as the app or an editor posts it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notice {
    /// What the user can do about it, each a button on the dialog an error
    /// opens, handed back to whoever posted it (see [`NoticeAction`]).
    pub actions: Vec<NoticeAction>,
    /// The whole of what went wrong, under the one-line [`Self::text`]: what
    /// a failed command printed, say. Shown in the dialog and kept in the log.
    pub detail: Option<String>,
    /// Whether it is kept in the log once its toast is gone. Off for what the
    /// log says about itself, which would only fill it.
    pub logged: bool,
    /// How much it matters.
    pub severity: Severity,
    /// Who is saying it (`Git`, `Dir`), shown above the text; `None` for the
    /// window itself.
    pub source: Option<String>,
    /// What is said, in a line: the dialog's message, a toast's text.
    pub text: String,
}

/// Something the user can do about a notice: a button on its dialog. Pressed,
/// its `id` goes back to the editor that posted the notice, which knows what
/// it means (`abort_rebase`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoticeAction {
    /// What the poster is told was chosen.
    pub id: &'static str,
    /// What the button says (`Abort Rebase`).
    pub label: String,
}

/// A notice as the log keeps it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggedNotice {
    /// When it last came, in seconds since the Unix epoch.
    pub(crate) at: i64,
    /// How many times in a row it came, so a repeat is counted, not listed.
    pub(crate) repeats: usize,
    /// The whole of what went wrong, as the notice gave it.
    pub(crate) detail: Option<String>,
    /// How much it matters.
    pub(crate) severity: Severity,
    /// Who said it, as the notice named it.
    pub(crate) source: Option<String>,
    /// What was said.
    pub(crate) text: String,
    /// Whether it still counts on the bell: an error or a warning not yet
    /// looked at. A confirmation is never unread.
    pub(crate) unread: bool,
}

/// Every notification of the session, newest first.
#[derive(Debug, Default)]
pub struct NoticeLog {
    entries: VecDeque<LoggedNotice>,
}

/// A notice on screen as a toast.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Toast {
    /// When it goes away on its own.
    pub(crate) expires: Instant,
    /// Names it for the whole time it is shown, so a click on a card dismisses
    /// that card even if others came or went since it was drawn.
    pub(crate) id: ToastId,
    /// How many times in a row it came.
    pub(crate) repeats: usize,
    /// How much it matters, which colors the card.
    pub(crate) severity: Severity,
    /// Who said it, shown above the text.
    pub(crate) source: Option<String>,
    /// What was said.
    pub(crate) text: String,
}

/// Names one toast for as long as it is shown.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ToastId(u64);

/// The toasts on screen, newest first.
#[derive(Debug, Default)]
pub struct ToastStack {
    /// The toast the pointer rests on, which does not expire while it does.
    held: Option<ToastId>,
    /// The number the next toast's id takes.
    next_id: u64,
    /// The toasts, newest first.
    toasts: Vec<Toast>,
}

/// Where a toast shows: as a card, or on the status bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Routing {
    /// The least severity shown as a card, or `None` for no cards while
    /// the status bar is shown.
    pub cards_from: Option<Severity>,
    /// Whether the window shows a status bar; without one, every toast is
    /// a card, having nowhere else to go.
    pub status_bar: bool,
}

/// A long task to show while it runs, as the app starts it.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// Whether it may be stopped, which gives its card a Cancel button.
    cancellable: bool,
    /// Whether it shows as a card from its start rather than only on the
    /// status bar.
    card: bool,
    /// Who runs it (`cargo`, `rust-analyzer`).
    source: Option<String>,
    /// What it is doing, in a line (`Building`, `Indexing`).
    title: String,
}

/// Names one task for as long as it runs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProgressId(u64);

/// A task under way.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// Whether it may be stopped.
    pub(crate) cancellable: bool,
    /// Whether it shows as a card.
    pub(crate) card: bool,
    /// How far it has come, 0 to 1, or `None` while that is not known.
    pub(crate) fraction: Option<f32>,
    /// Its name.
    pub(crate) id: ProgressId,
    /// Who runs it.
    pub(crate) source: Option<String>,
    /// When it started, which its spinner turns from.
    pub(crate) started: Instant,
    /// The step it is on (`3/10 files`), or `None`.
    pub(crate) step: Option<String>,
    /// What it is doing.
    pub(crate) title: String,
}

/// The single owner of what is shown and what is kept.
#[derive(Debug)]
pub struct Notifier {
    /// What is kept for the session.
    log: NoticeLog,
    /// The number the next task's id takes.
    next_task: u64,
    /// Where each toast shows.
    routing: Routing,
    /// The tasks under way, oldest first.
    tasks: Vec<Task>,
    /// What is on screen now.
    toasts: ToastStack,
}

// ========================================================================
// Severity
// ========================================================================

impl Severity {
    /// How long a toast of this severity is shown.
    pub fn duration(self) -> Duration {
        match self {
            Severity::Error => ERROR_DURATION,
            Severity::Info => INFO_DURATION,
            Severity::Refusal => REFUSAL_DURATION,
            Severity::Warning => WARNING_DURATION,
        }
    }

    /// The glyph that marks this severity before a message: an information
    /// sign, a crossed circle, a warning sign, a cross.
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Severity::Error => "\u{2716}",
            Severity::Info => "\u{2139}",
            Severity::Refusal => "\u{2298}",
            Severity::Warning => "\u{26a0}",
        }
    }

    /// The color that marks this severity, from `theme`.
    pub(crate) fn color(self, theme: &Theme) -> Rgb {
        match self {
            Severity::Error => theme.danger,
            Severity::Info => theme.accent,
            Severity::Refusal | Severity::Warning => theme.warning,
        }
    }

    /// Whether a notification of this severity counts on the bell until it
    /// is looked at.
    fn needs_attention(self) -> bool {
        self >= Severity::Warning
    }
}

// ========================================================================
// Notice
// ========================================================================

impl Notice {
    fn new(severity: Severity, text: impl Into<String>) -> Self {
        Self {
            actions: Vec::new(),
            detail: None,
            logged: true,
            severity,
            source: None,
            text: text.into(),
        }
    }

    /// Something that failed.
    pub fn error(text: impl Into<String>) -> Self {
        Self::new(Severity::Error, text)
    }

    /// A confirmation of something done.
    pub fn info(text: impl Into<String>) -> Self {
        Self::new(Severity::Info, text)
    }

    /// Something worth knowing that is not a failure.
    pub fn warning(text: impl Into<String>) -> Self {
        Self::new(Severity::Warning, text)
    }

    /// A command that had nothing to act on, said in passing and not kept:
    /// a key that did nothing is no news worth a history.
    pub fn refusal(text: impl Into<String>) -> Self {
        Self::new(Severity::Refusal, text).unlogged()
    }

    /// The same notice, said by `source`. A source already given is kept:
    /// the one closest to the notice knows best who is speaking.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source.get_or_insert_with(|| source.into());
        self
    }

    /// The same notice, `detail` under its line (see [`Self::detail`]).
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The same notice, offering `action` as a button on its dialog.
    pub fn with_action(mut self, id: &'static str, label: impl Into<String>) -> Self {
        self.actions.push(NoticeAction {
            id,
            label: label.into(),
        });
        self
    }

    /// The same notice, shown but not kept in the log.
    pub fn unlogged(mut self) -> Self {
        self.logged = false;
        self
    }

    /// Whether `self` says what an earlier notice said, so it counts as a
    /// repeat of it rather than as news.
    fn repeats(&self, severity: Severity, source: &Option<String>, text: &str) -> bool {
        self.severity == severity && &self.source == source && self.text == text
    }
}

// ========================================================================
// NoticeLog
// ========================================================================

impl LoggedNotice {
    /// When it last came, in seconds since the Unix epoch.
    pub fn at(&self) -> i64 {
        self.at
    }

    /// The whole of what went wrong, as the notice gave it.
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// How many times in a row it came, so a repeat is counted, not listed.
    pub fn repeats(&self) -> usize {
        self.repeats
    }

    /// How much it matters.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// Who said it, as the notice named it.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// What was said.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether it still counts on the bell: an error or a warning not yet
    /// looked at.
    pub fn is_unread(&self) -> bool {
        self.unread
    }

    /// What was said, as a list shows it: who said it ahead of it, and how
    /// many times when it came more than once, in `strings`' words
    /// ([`Strings::repeated`]); pass the theme's, `&theme.strings`.
    pub fn message(&self, strings: &Strings) -> String {
        let text = match &self.source {
            Some(source) => format!("{source}: {}", self.text),
            None => self.text.clone(),
        };
        strings::repeated(strings, &text, self.repeats)
    }
}

impl NoticeLog {
    /// Keep `notice`, which came at `at`. The same notification again only
    /// bumps the newest one's count, and makes it news again.
    pub fn record(&mut self, notice: &Notice, at: i64) {
        let unread = notice.severity.needs_attention();
        if let Some(newest) = self.entries.front_mut()
            && notice.repeats(newest.severity, &newest.source, &newest.text)
        {
            newest.at = at;
            newest.repeats += 1;
            newest.unread |= unread;
            return;
        }
        self.entries.push_front(LoggedNotice {
            at,
            detail: notice.detail.clone(),
            repeats: 1,
            severity: notice.severity,
            source: notice.source.clone(),
            text: notice.text.clone(),
            unread,
        });
        self.entries.truncate(LOG_LIMIT);
    }

    /// How many errors and warnings are unread.
    pub fn unread(&self) -> usize {
        self.entries.iter().filter(|entry| entry.unread).count()
    }

    /// Count every notification as looked at.
    pub fn mark_read(&mut self) {
        self.entries
            .iter_mut()
            .for_each(|entry| entry.unread = false);
    }

    /// Forget every notification.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Forget the notification `index` places from the newest, as a log
    /// page's Delete does; the one forgotten, if there was one.
    pub fn remove(&mut self, index: usize) -> Option<LoggedNotice> {
        self.entries.remove(index)
    }

    /// The notification `index` places from the newest.
    pub fn get(&self, index: usize) -> Option<&LoggedNotice> {
        self.entries.get(index)
    }

    /// Every notification, newest first.
    pub fn iter(&self) -> impl Iterator<Item = &LoggedNotice> {
        self.entries.iter()
    }
}

// ========================================================================
// Toast
// ========================================================================

impl Toast {
    /// When it goes away on its own.
    pub fn expires(&self) -> Instant {
        self.expires
    }

    /// Names it for the whole time it is shown, so a click on a card
    /// dismisses that card even if others came or went since it was drawn.
    pub fn id(&self) -> ToastId {
        self.id
    }

    /// How many times in a row it came.
    pub fn repeats(&self) -> usize {
        self.repeats
    }

    /// How much it matters, which colors the card.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// Who said it, shown above the text.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// What was said.
    pub fn text(&self) -> &str {
        &self.text
    }
}

// ========================================================================
// ToastStack
// ========================================================================

impl ToastStack {
    /// Show `notice` from `now`. A repeat of the newest toast refreshes it
    /// in place rather than stacking a copy. Past [`TOAST_LIMIT`] the oldest
    /// confirmation or refusal goes first, then the oldest of the rest, so a
    /// burst of either never pushes a problem off screen.
    fn show(&mut self, notice: &Notice, now: Instant) {
        let expires = now + notice.severity.duration();
        if let Some(newest) = self.toasts.first_mut()
            && notice.repeats(newest.severity, &newest.source, &newest.text)
        {
            newest.expires = expires;
            newest.repeats += 1;
            return;
        }
        self.next_id += 1;
        self.toasts.insert(
            0,
            Toast {
                expires,
                id: ToastId(self.next_id),
                repeats: 1,
                severity: notice.severity,
                source: notice.source.clone(),
                text: notice.text.clone(),
            },
        );
        while self.toasts.len() > TOAST_LIMIT {
            let oldest_info = self
                .toasts
                .iter()
                .rposition(|toast| toast.severity < Severity::Warning);
            let evicted = oldest_info.unwrap_or(self.toasts.len() - 1);
            self.toasts.remove(evicted);
        }
    }

    /// The toasts on screen, newest first.
    pub fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter()
    }

    /// The toast the pointer rests on, if any.
    pub fn held(&self) -> Option<ToastId> {
        self.held
    }

    /// The newest toast, if any is on screen.
    pub fn newest(&self) -> Option<&Toast> {
        self.toasts.first()
    }

    /// Whether no toast is on screen.
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    /// Hold the toast `id` on screen while the pointer rests on it, or none.
    /// Letting go gives the toast its full time again from `now`, so it is
    /// not snatched away the moment the pointer leaves.
    pub fn hold(&mut self, id: Option<ToastId>, now: Instant) -> bool {
        if self.held == id {
            return false;
        }
        if let Some(released) = self.held.and_then(|held| self.find_mut(held)) {
            released.expires = now + released.severity.duration();
        }
        self.held = id;
        true
    }

    /// Take the toast `id` off screen at once. Whether it was there.
    pub fn dismiss(&mut self, id: ToastId) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.id != id);
        self.toasts.len() != before
    }

    /// Take every toast off screen.
    pub fn dismiss_all(&mut self) {
        self.toasts.clear();
    }

    /// Drop the toasts whose time is up at `now`, all but the held one.
    /// Whether any went.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.toasts.len();
        let held = self.held;
        self.toasts
            .retain(|toast| Some(toast.id) == held || toast.expires > now);
        self.toasts.len() != before
    }

    /// When the next toast is due to go, for the event loop to wake at.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.toasts
            .iter()
            .filter(|toast| Some(toast.id) != self.held)
            .map(|toast| toast.expires)
            .min()
    }

    /// The toast `id`, to change it.
    fn find_mut(&mut self, id: ToastId) -> Option<&mut Toast> {
        self.toasts.iter_mut().find(|toast| toast.id == id)
    }
}

// ========================================================================
// Notifier
// ========================================================================

impl Notifier {
    /// A notifier with nothing to show, cards from warnings up.
    pub fn new() -> Self {
        Self {
            log: NoticeLog::default(),
            next_task: 0,
            routing: Routing::default(),
            tasks: Vec::new(),
            toasts: ToastStack::default(),
        }
    }

    // --------------------------------------------------------------------
    // Notices
    // --------------------------------------------------------------------

    /// Show `notice` from now and keep it in the log, as [`Notifier::post`]
    /// does with the clocks read. Ask for a redraw after.
    pub fn notify(&mut self, notice: Notice) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);
        self.post(&notice, clock::now(), at);
    }

    /// Show `notice` from `now` and, unless it is unlogged, keep it in the
    /// log as having come at `at` (seconds since the Unix epoch).
    pub fn post(&mut self, notice: &Notice, now: Instant, at: i64) {
        self.toasts.show(notice, now);
        if notice.logged {
            self.log.record(notice, at);
        }
    }

    /// What is kept for the session.
    pub fn log(&self) -> &NoticeLog {
        &self.log
    }

    /// What is kept for the session, to mark it read or clear it.
    pub fn log_mut(&mut self) -> &mut NoticeLog {
        &mut self.log
    }

    /// What is on screen now.
    pub fn toasts(&self) -> &ToastStack {
        &self.toasts
    }

    /// What is on screen now, to hold or dismiss a toast.
    pub fn toasts_mut(&mut self) -> &mut ToastStack {
        &mut self.toasts
    }

    /// Shows each toast where `routing` says from now on.
    pub fn set_routing(&mut self, routing: Routing) {
        self.routing = routing;
    }

    /// Where each toast shows.
    pub fn routing(&self) -> Routing {
        self.routing
    }

    /// The toasts shown as cards, newest first.
    pub fn cards(&self) -> impl Iterator<Item = &Toast> {
        let routing = self.routing;
        self.toasts
            .iter()
            .filter(move |toast| routing.is_card(toast.severity))
    }

    /// The newest toast shown on the status bar, if any.
    pub fn status_message(&self) -> Option<&Toast> {
        let routing = self.routing;
        self.toasts
            .iter()
            .find(|toast| !routing.is_card(toast.severity))
    }

    // --------------------------------------------------------------------
    // Progress
    // --------------------------------------------------------------------

    /// Starts showing `progress` from `now` until [`Notifier::finish`];
    /// what names it meanwhile.
    pub fn start(&mut self, progress: Progress, now: Instant) -> ProgressId {
        let id = ProgressId(self.next_task);
        self.next_task += 1;
        self.tasks.push(Task {
            cancellable: progress.cancellable,
            card: progress.card,
            fraction: None,
            id,
            source: progress.source,
            started: now,
            step: None,
            title: progress.title,
        });
        id
    }

    /// Says task `id` has come `fraction` of the way (held to 0 to 1), or
    /// that how far is not known; `false` when it is not running.
    pub fn set_fraction(&mut self, id: ProgressId, fraction: Option<f32>) -> bool {
        self.update_task(id, |task| {
            task.fraction = fraction.map(|f| f.clamp(0.0, 1.0))
        })
    }

    /// Says task `id` is on `step` (`3/10 files`), or none; `false` when it
    /// is not running.
    pub fn set_step(&mut self, id: ProgressId, step: Option<String>) -> bool {
        self.update_task(id, |task| task.step = step)
    }

    /// Shows every task under way as a card: what a click on the status
    /// bar's progress item asks.
    pub fn show_task_cards(&mut self) {
        self.tasks.iter_mut().for_each(|task| task.card = true);
    }

    /// Ends task `id`, posting `notice` (a `finished` or a `failed`) at
    /// `now` and `at` when given; `false` when it was not running.
    pub fn finish(
        &mut self,
        id: ProgressId,
        notice: Option<Notice>,
        now: Instant,
        at: i64,
    ) -> bool {
        let before = self.tasks.len();
        self.tasks.retain(|task| task.id != id);
        let ended = self.tasks.len() != before;
        if let (true, Some(notice)) = (ended, notice) {
            self.post(&notice, now, at);
        }
        ended
    }

    /// The tasks under way, oldest first.
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// Runs `change` on task `id`; whether it is running.
    fn update_task(&mut self, id: ProgressId, change: impl FnOnce(&mut Task)) -> bool {
        match self.tasks.iter_mut().find(|task| task.id == id) {
            Some(task) => {
                change(task);
                true
            }
            None => false,
        }
    }
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Task
// ========================================================================

impl Task {
    /// Whether it may be stopped.
    pub fn is_cancellable(&self) -> bool {
        self.cancellable
    }

    /// Whether it shows as a card.
    pub fn is_card(&self) -> bool {
        self.card
    }

    /// How far it has come, 0 to 1, or `None` while that is not known.
    pub fn fraction(&self) -> Option<f32> {
        self.fraction
    }

    /// Its name.
    pub fn id(&self) -> ProgressId {
        self.id
    }

    /// Who runs it.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// When it started, which its spinner turns from.
    pub fn started(&self) -> Instant {
        self.started
    }

    /// The step it is on (`3/10 files`), or `None`.
    pub fn step(&self) -> Option<&str> {
        self.step.as_deref()
    }

    /// What it is doing.
    pub fn title(&self) -> &str {
        &self.title
    }
}

// ========================================================================
// Routing
// ========================================================================

impl Routing {
    /// Whether a toast of `severity` is a card rather than a status bar
    /// message.
    pub fn is_card(self, severity: Severity) -> bool {
        !self.status_bar || self.cards_from.is_some_and(|least| severity >= least)
    }
}

impl Default for Routing {
    /// Warnings and errors as cards, the rest on the status bar.
    fn default() -> Self {
        Self {
            cards_from: Some(DEFAULT_CARDS_FROM),
            status_bar: true,
        }
    }
}

// ========================================================================
// Progress
// ========================================================================

impl Progress {
    /// A task doing `title`, shown on the status bar, which cannot be
    /// stopped.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            cancellable: false,
            card: false,
            source: None,
            title: title.into(),
        }
    }

    /// This task run by `source`.
    pub fn from_source(self, source: impl Into<String>) -> Self {
        Self {
            source: Some(source.into()),
            ..self
        }
    }

    /// This task with a Cancel button on its card.
    pub fn cancellable(self) -> Self {
        Self {
            cancellable: true,
            ..self
        }
    }

    /// This task shown as a card from its start.
    pub fn as_card(self) -> Self {
        Self { card: true, ..self }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// `notice` posted to `notifier` at `now`.
    fn post(notifier: &mut Notifier, notice: Notice, now: Instant) {
        notifier.post(&notice, now, 0);
    }

    #[test]
    fn test_only_errors_and_warnings_count_until_read() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::info("copied"), now);
        assert_eq!(notifier.log.unread(), 0);
        post(&mut notifier, Notice::warning("slow fetch"), now);
        assert_eq!(notifier.log.unread(), 1);
        post(&mut notifier, Notice::error("push failed"), now);
        post(&mut notifier, Notice::warning("slow again"), now);
        assert_eq!(notifier.log.unread(), 3);

        notifier.log.mark_read();
        assert_eq!(notifier.log.unread(), 0);
        assert_eq!(notifier.log.iter().count(), 4, "read is not forgotten");
    }

    #[test]
    fn test_one_notification_is_forgotten_by_its_place_from_the_newest() {
        let mut notifier = Notifier::new();
        for text in ["first", "second", "third"] {
            notifier.notify(Notice::error(text));
        }
        let removed = notifier.log_mut().remove(1).expect("the second newest");
        assert_eq!(removed.text(), "second");
        let left: Vec<&str> = notifier.log().iter().map(LoggedNotice::text).collect();
        assert_eq!(left, ["third", "first"]);
        assert_eq!(notifier.log().unread(), 2);
        assert!(notifier.log_mut().remove(5).is_none());
    }

    #[test]
    fn test_a_repeat_merges_into_the_newest_in_both_halves() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::error("push failed"), now);
        notifier.log.mark_read();
        let later = now + Duration::from_secs(5);
        post(&mut notifier, Notice::error("push failed"), later);
        assert_eq!(notifier.log.iter().count(), 1);
        assert_eq!(notifier.log.get(0).map(|entry| entry.repeats), Some(2));
        assert_eq!(notifier.log.unread(), 1, "the repeat is news again");
        let toast = notifier.toasts.newest().expect("the toast");
        assert_eq!((toast.repeats, toast.expires), (2, later + ERROR_DURATION));

        post(
            &mut notifier,
            Notice::error("push failed").from_source("Git"),
            later,
        );
        assert_eq!(notifier.log.iter().count(), 2, "another source is news");
    }

    #[test]
    fn test_an_unlogged_notice_is_shown_but_not_kept() {
        let mut notifier = Notifier::default();
        post(
            &mut notifier,
            Notice::info("cleared").unlogged(),
            Instant::now(),
        );
        assert!(!notifier.toasts.is_empty());
        assert_eq!(notifier.log.iter().count(), 0);
    }

    #[test]
    fn test_the_log_and_the_stack_keep_only_the_newest() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        for at in 0..LOG_LIMIT + 5 {
            notifier.post(&Notice::info(at.to_string()), now, at as i64);
        }
        assert_eq!(notifier.log.iter().count(), LOG_LIMIT);
        assert_eq!(
            notifier.log.get(0).map(|entry| entry.at),
            Some(LOG_LIMIT as i64 + 4)
        );
        let shown: Vec<_> = notifier
            .toasts
            .iter()
            .map(|toast| toast.text.clone())
            .collect();
        assert_eq!(shown.len(), TOAST_LIMIT);
        assert_eq!(shown[0], (LOG_LIMIT + 4).to_string(), "newest first");
    }

    #[test]
    fn test_confirmations_give_way_before_problems() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::error("push failed"), now);
        for text in ["a", "b", "c", "d"] {
            post(&mut notifier, Notice::info(text), now);
        }
        let shown: Vec<_> = notifier
            .toasts
            .iter()
            .map(|toast| toast.text.as_str())
            .collect();
        assert_eq!(shown, ["d", "c", "push failed"]);
    }

    #[test]
    fn test_toasts_expire_by_severity_except_the_held_one() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::info("copied"), now);
        post(&mut notifier, Notice::error("push failed"), now);
        assert_eq!(notifier.toasts.next_expiry(), Some(now + INFO_DURATION));

        assert!(notifier.toasts.expire(now + INFO_DURATION));
        let left: Vec<_> = notifier.toasts.iter().map(|toast| toast.severity).collect();
        assert_eq!(left, [Severity::Error], "an error outlasts a confirmation");

        let error = notifier.toasts.newest().map(|toast| toast.id);
        notifier.toasts.hold(error, now);
        assert_eq!(notifier.toasts.next_expiry(), None, "a held toast waits");
        assert!(!notifier.toasts.expire(now + ERROR_DURATION * 2));

        let released = now + ERROR_DURATION * 2;
        notifier.toasts.hold(None, released);
        assert_eq!(
            notifier.toasts.next_expiry(),
            Some(released + ERROR_DURATION),
            "let go, it gets its full time again"
        );
    }

    #[test]
    fn test_dismissing_takes_only_the_named_toast() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::info("a"), now);
        post(&mut notifier, Notice::info("b"), now);
        let first = notifier
            .toasts
            .iter()
            .last()
            .map(|toast| toast.id)
            .expect("a");
        assert!(notifier.toasts.dismiss(first));
        assert!(!notifier.toasts.dismiss(first), "already gone");
        let left: Vec<_> = notifier
            .toasts
            .iter()
            .map(|toast| toast.text.as_str())
            .collect();
        assert_eq!(left, ["b"]);
    }

    #[test]
    fn test_the_nearest_source_wins() {
        let notice = Notice::error("x").from_source("Git").from_source("window");
        assert_eq!(notice.source.as_deref(), Some("Git"));
    }

    #[test]
    fn test_routing_puts_problems_on_cards_and_everything_there_without_a_status_bar() {
        let mut notifier = Notifier::new();
        let now = Instant::now();
        post(&mut notifier, Notice::info("copied"), now);
        post(&mut notifier, Notice::error("push failed"), now);
        let cards: Vec<&str> = notifier.cards().map(|toast| toast.text.as_str()).collect();
        assert_eq!(cards, ["push failed"]);
        assert_eq!(
            notifier.status_message().map(|toast| toast.text.as_str()),
            Some("copied")
        );
        notifier.set_routing(Routing {
            cards_from: None,
            status_bar: false,
        });
        assert_eq!(notifier.cards().count(), 2, "nowhere else to go");
        assert_eq!(notifier.status_message(), None);
        notifier.set_routing(Routing {
            cards_from: None,
            status_bar: true,
        });
        assert_eq!(notifier.cards().count(), 0);
    }

    #[test]
    fn test_a_task_runs_until_finished_and_may_leave_a_notice() {
        let mut notifier = Notifier::new();
        let now = Instant::now();
        let build = notifier.start(
            Progress::new("Building").from_source("cargo").cancellable(),
            now,
        );
        let index = notifier.start(Progress::new("Indexing"), now);
        assert!(notifier.set_fraction(build, Some(1.5)));
        assert!(notifier.set_step(build, Some("3/10".into())));
        let task = &notifier.tasks()[0];
        assert_eq!(
            (task.fraction, task.step.as_deref(), task.cancellable),
            (Some(1.0), Some("3/10"), true)
        );
        assert!(!notifier.tasks()[1].card);
        notifier.show_task_cards();
        assert!(notifier.tasks().iter().all(|task| task.card));
        assert!(notifier.finish(build, Some(Notice::info("built")), now, 0));
        assert!(!notifier.finish(build, None, now, 0), "already done");
        assert_eq!(notifier.tasks().len(), 1);
        assert_eq!(
            notifier.status_message().map(|toast| toast.text.as_str()),
            Some("built")
        );
        assert!(!notifier.set_step(build, None));
        assert!(notifier.finish(index, None, now, 0));
    }
}

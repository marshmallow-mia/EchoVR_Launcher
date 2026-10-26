//! The status bar's notices: what just happened, one at a time, with a button when there
//! is something to do about it ("Undo", "Update", "Try again"). A notice stays while the
//! window is in front: its time runs only while the window has focus, so one shown while
//! the player was in the game is still there when they come back. An error stays until
//! it is closed.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::core::launcher::goback::WayBack;

/// How long a notice is shown (window in front).
const PLAIN: Duration = Duration::from_secs(8);
/// A notice with a button: long enough to get to it.
const WITH_ACTION: Duration = Duration::from_secs(20);
/// At most this many wait; the oldest plain one goes first.
const MAX: usize = 6;
/// What [`Notices::offer`] adds its button to: said this recently.
const FRESH: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Info,
    /// Red, and stays until it is closed.
    Error,
}

/// What a notice's button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Action {
    /// Asks to undo this update (`goback::ask`).
    Undo(WayBack),
    /// Opens "Undo an update".
    UndoCard,
    /// Installs the newest version of a part whose update was undone (its hold's key).
    UseNewest(String),
    /// Updates installed version `id`'s game files again.
    UpdateVersion(String),
    /// Updates the game on the Quest.
    UpdateQuest,
    /// Points Echo VR's entry in SteamVR's library at the version picked now.
    SteamVrLibrary,
    /// Installs what the update check found.
    Update(Vec<crate::core::updates::Finding>),
    /// Opens a page.
    Page(super::Page),
    /// Picks installed version `id` and plays it (Play's next step, when there is one
    /// first).
    Play(String),
    /// Echo VR's entry in Steam starts this launcher again.
    FixSteamEntry,
    /// Joins the match of an invite (`servers`).
    JoinInvite(String),
    /// Opens a web page.
    OpenUrl(String),
    /// Closes the Echo VR PLAY started and starts it again (a change that loads at the
    /// next start).
    RestartGame,
}

#[derive(Debug, Clone)]
pub(super) struct Notice {
    pub text: String,
    pub kind: Kind,
    /// The button: its label, and what it does.
    pub action: Option<(String, Action)>,
    /// Time left while the window is in front (an error's isn't counted).
    left: Duration,
    /// When it was added.
    at: Instant,
}

impl Notice {
    pub fn info(text: &str) -> Notice {
        Notice {
            text: text.to_string(),
            kind: Kind::Info,
            action: None,
            left: PLAIN,
            at: Instant::now(),
        }
    }

    pub fn error(text: &str) -> Notice {
        Notice {
            kind: Kind::Error,
            ..Notice::info(text)
        }
    }

    /// With a button.
    pub fn with(mut self, label: &str, action: Action) -> Notice {
        self.action = Some((label.to_string(), action));
        self.left = self.left.max(WITH_ACTION);
        self
    }
}

#[derive(Default)]
pub(super) struct Notices {
    queue: VecDeque<Notice>,
    /// When time was last counted, and whether the window was in front then (`None`: the
    /// one shown is new). The time up to the next count is the window's state at the last:
    /// a change of focus draws a frame, so it is counted.
    counted: Option<(Instant, bool)>,
}

impl Notices {
    /// Adds one: after those waiting (the same text again replaces the one waiting).
    pub fn push(&mut self, n: Notice) {
        if let Some(i) = self.queue.iter().position(|q| q.text == n.text) {
            self.queue.remove(i);
            if i == 0 {
                self.counted = None;
            }
        }
        self.queue.push_back(n);
        while self.queue.len() > MAX {
            // A plain one goes first, else the oldest (not the one shown).
            let i = self
                .queue
                .iter()
                .skip(1)
                .position(|q| q.kind == Kind::Info && q.action.is_none())
                .map_or(1, |i| i + 1);
            self.queue.remove(i);
        }
    }

    /// Adds a button to what was just said (a job's "… is updated"), or says it on its
    /// own (as `text`) when that was a while ago or has a button already.
    pub fn offer(&mut self, text: &str, label: &str, action: Action) {
        match self.queue.back_mut() {
            Some(n) if n.action.is_none() && n.kind == Kind::Info && n.at.elapsed() < FRESH => {
                n.action = Some((label.to_string(), action));
                n.left = n.left.max(WITH_ACTION);
            }
            _ => self.push(Notice::info(text).with(label, action)),
        }
    }

    /// Counts the time shown since the last call (only while `focused`) and lets the one
    /// shown go once its time is up; returns how long until that happens, for a repaint.
    pub fn tick(&mut self, now: Instant, focused: bool) -> Option<Duration> {
        let last = self.counted.replace((now, focused));
        let front = self.queue.front_mut()?;
        if front.kind == Kind::Error {
            return None;
        }
        if let Some((at, true)) = last {
            front.left = front.left.saturating_sub(now.saturating_duration_since(at));
        }
        if front.left.is_zero() {
            self.next();
            return self.queue.front().map(|n| n.left);
        }
        Some(front.left)
    }

    /// The one shown, and how many wait behind it.
    pub fn current(&self) -> Option<(&Notice, usize)> {
        self.queue
            .front()
            .map(|n| (n, self.queue.len().saturating_sub(1)))
    }

    /// Closes the one shown.
    pub fn next(&mut self) {
        self.queue.pop_front();
        self.counted = None;
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.counted = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_runs_only_while_the_window_is_in_front() {
        let mut q = Notices::default();
        q.push(Notice::info("a"));
        let t0 = Instant::now();
        q.tick(t0, true);
        // Away after a second, for a minute: still there.
        q.tick(t0 + Duration::from_secs(1), false);
        q.tick(t0 + Duration::from_secs(61), true);
        assert_eq!(q.current().unwrap().0.text, "a");
        // Back: 1 + 6 seconds in front so far, then 8.
        q.tick(t0 + Duration::from_secs(67), true);
        assert!(q.current().is_some());
        q.tick(t0 + Duration::from_secs(69), true);
        assert!(q.current().is_none());
    }

    #[test]
    fn one_after_the_other_and_errors_stay() {
        let mut q = Notices::default();
        q.push(Notice::error("broke"));
        q.push(Notice::info("b"));
        let t0 = Instant::now();
        q.tick(t0, true);
        q.tick(t0 + Duration::from_secs(600), true);
        let (n, more) = q.current().unwrap();
        assert_eq!((n.text.as_str(), more), ("broke", 1));
        q.next();
        q.tick(t0 + Duration::from_secs(601), true);
        assert_eq!(q.current().unwrap().0.text, "b");
        q.tick(t0 + Duration::from_secs(610), true);
        assert!(q.current().is_none());
    }

    #[test]
    fn a_button_gives_more_time_and_the_same_text_isnt_repeated() {
        let mut q = Notices::default();
        q.push(Notice::info("u").with("Update", Action::UndoCard));
        q.push(Notice::info("u"));
        assert_eq!(q.current().unwrap().1, 0);
        // Pushed again plain: the plain one replaced it.
        let mut q = Notices::default();
        q.push(Notice::info("u").with("Update", Action::UndoCard));
        let t0 = Instant::now();
        q.tick(t0, true);
        q.tick(t0 + Duration::from_secs(10), true);
        assert!(q.current().is_some());
        q.tick(t0 + Duration::from_secs(21), true);
        assert!(q.current().is_none());
    }

    #[test]
    fn plain_ones_make_room_first() {
        let mut q = Notices::default();
        q.push(Notice::error("shown"));
        q.push(Notice::info("act").with("Undo", Action::UndoCard));
        for i in 0..10 {
            q.push(Notice::info(&format!("plain {i}")));
        }
        assert_eq!(q.queue.len(), MAX);
        assert_eq!(q.queue[0].text, "shown");
        assert_eq!(q.queue[1].text, "act");
        assert_eq!(q.queue.back().unwrap().text, "plain 9");
    }

    #[test]
    fn an_offer_goes_on_what_was_just_said() {
        let mut q = Notices::default();
        q.push(Notice::info("Plugin updated"));
        q.offer("x is updated", "Undo", Action::UndoCard);
        assert_eq!(q.queue.len(), 1);
        assert_eq!(q.queue[0].action.as_ref().unwrap().0, "Undo");
        // Said with a button already: on its own.
        q.offer("x is updated", "Undo", Action::UndoCard);
        assert_eq!(q.queue.len(), 2);
        assert_eq!(q.queue[1].text, "x is updated");
    }
}

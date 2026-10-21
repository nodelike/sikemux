//! What an agent just did, told once to both the person's unread marks and
//! the phones' notifications, for chats and agents in terminals alike.

use super::follow::Happened;
use super::notify::Signal;
use super::Core;

pub(crate) enum Activity<'a> {
    /// It began a turn of work.
    Started,
    /// It is still at the work it began.
    Working,
    /// Its turn ended. One the person stopped is nothing new to them.
    Finished { cancelled: bool },
    /// It waits on the person.
    NeedsYou,
    /// It ran into a problem, and said this about it.
    Problem(&'a str),
}

impl Core {
    pub(crate) fn note_activity(&self, agent_id: &str, activity: Activity<'_>) {
        let happened = match activity {
            Activity::Started => Some(Happened::Working),
            Activity::Working => None,
            Activity::Finished { cancelled: false } => Some(Happened::Done),
            Activity::Finished { cancelled: true } => Some(Happened::Stopped),
            Activity::NeedsYou => Some(Happened::NeedsYou),
            Activity::Problem(_) => Some(Happened::Failed),
        };
        if let Some(happened) = happened {
            self.journal_agent(agent_id, happened);
        }
        if matches!(activity, Activity::Started | Activity::Working) {
            self.edits.working(agent_id);
        }
        let agent_id = agent_id.to_owned();
        let came_to_rest = matches!(activity, Activity::Finished { .. } | Activity::Problem(_));
        match &activity {
            Activity::Started => {
                if let Some(place) = super::activity_log::place(self, &agent_id) {
                    self.activity_log.turn_started(&agent_id, place.cwd);
                }
            }
            Activity::Finished { .. } | Activity::Problem(_) => {
                if let Some(place) = super::activity_log::place(self, &agent_id) {
                    self.activity_log.turn_ended(&agent_id, place);
                }
            }
            _ => {}
        }
        match activity {
            Activity::Started => {
                self.seen.started(&agent_id);
                self.inbox.started(&agent_id);
                self.notify.send(Signal::TurnStarted { agent_id });
            }
            Activity::Working => self.seen.working(&agent_id),
            Activity::Finished { cancelled } => {
                super::questions::turn_ended(self, &agent_id);
                if cancelled {
                    self.seen.active(&agent_id);
                    self.inbox.stopped(&agent_id);
                } else {
                    self.seen.wants_a_look(&agent_id);
                    self.inbox.ended(&agent_id, None);
                }
                self.notify.send(Signal::TurnCompleted {
                    agent_id,
                    cancelled,
                });
            }
            Activity::NeedsYou => self.seen.wants_a_look(&agent_id),
            Activity::Problem(message) => {
                self.seen.wants_a_look(&agent_id);
                self.inbox.ended(&agent_id, Some(message));
                self.notify.send(Signal::Problem {
                    agent_id,
                    message: message.to_owned(),
                });
            }
        }
        if came_to_rest {
            self.agents_changed();
        }
    }

    /// The agent stopped running, whether it ended by itself or was closed.
    pub(crate) fn note_agent_gone(&self, agent_id: &str) {
        self.journal_agent(agent_id, Happened::Gone);
        self.agents_changed();
    }

    /// Something about the agents changed that the messages waiting for them,
    /// their questions and the board depend on: one came to rest, started,
    /// went away, or the window opened or closed.
    pub(crate) fn agents_changed(&self) {
        self.harness.mailbox.wake.now();
        self.harness.questions.wake.now();
        self.boards.wake.now();
        self.view_changed();
    }

    /// Something a device's view of this host shows changed; the view goes
    /// out again shortly, once with any changes that follow it.
    pub(crate) fn view_changed(&self) {
        self.view_stale.notify_one();
    }

    fn journal_agent(&self, agent_id: &str, happened: Happened) {
        if let Some(named) = self.names.get(agent_id) {
            self.harness
                .note_agent(&named.project, happened.kind(), agent_id);
        }
        super::hosts::route::happened(self, agent_id, happened);
    }
}

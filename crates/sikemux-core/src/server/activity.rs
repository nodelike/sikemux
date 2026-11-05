//! What an agent just did, told once to both the person's unread marks and
//! the phones' notifications, for chats and agents in terminals alike.

use super::follow::Happened;
use super::notify::Signal;
use super::Core;
use crate::server::changed::What;

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
        if came_to_rest {
            self.ship.turn_ended(&agent_id);
        }
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
                super::board::chat_goal::turn_started(self, &agent_id);
                self.ship.turn_started(&agent_id);
                self.seen.started(&agent_id);
                self.inbox.started(&agent_id);
                self.boards.wake.now();
                self.harness.questions.turn_started(&agent_id);
                self.notify.send(Signal::TurnStarted { agent_id });
            }
            Activity::Working => self.seen.working(&agent_id),
            Activity::Finished { cancelled } => {
                super::board::chat_goal::turn_ended(self, &agent_id, !cancelled);
                super::questions::turn_ended(self, &agent_id);
                if cancelled {
                    self.seen.active(&agent_id);
                    self.inbox.stopped(&agent_id);
                } else {
                    if self.reports_to_open_lead(&agent_id) {
                        self.seen.active(&agent_id);
                    } else {
                        self.seen.wants_a_look(&agent_id);
                    }
                    self.inbox.ended(&agent_id, None);
                }
                self.notify.send(Signal::TurnCompleted {
                    agent_id,
                    cancelled,
                });
            }
            Activity::NeedsYou => self.seen.wants_a_look(&agent_id),
            Activity::Problem(message) => {
                super::board::chat_goal::turn_ended(self, &agent_id, false);
                self.seen.wants_a_look(&agent_id);
                self.inbox.ended(&agent_id, Some(message));
                self.notify.send(Signal::Problem {
                    agent_id,
                    message: message.to_owned(),
                });
            }
        }
        if came_to_rest {
            self.changed(What::Agents);
        }
    }

    /// The agent stopped running, whether it ended by itself or was closed.
    pub(crate) fn note_agent_gone(&self, agent_id: &str) {
        self.journal_agent(agent_id, Happened::Gone);
        self.changed(What::Agents);
    }

    /// Whether the agent's turns are told to a lead still here, who reads them
    /// in the person's place.
    fn reports_to_open_lead(&self, agent_id: &str) -> bool {
        self.names
            .reports_to(agent_id)
            .is_some_and(|lead| self.open_agents().contains(&lead))
    }

    fn journal_agent(&self, agent_id: &str, happened: Happened) {
        if let Some(named) = self.names.get(agent_id) {
            self.harness
                .note_agent(&named.project, happened.kind(), agent_id);
        }
        super::hosts::route::happened(self, agent_id, happened);
    }
}

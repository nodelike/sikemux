//! What an agent just did, told once to both the person's unread marks and
//! the phones' notifications, for chats and agents in terminals alike.

use std::sync::Arc;

use super::follow::Happened;
use super::notify::Signal;
use super::spawn::Ending;
use super::Core;
use crate::server::changed::What;

pub(crate) enum Activity<'a> {
    Started,
    Working,
    /// Its turn ended. One the person stopped is nothing new to them.
    Finished {
        cancelled: bool,
    },
    NeedsYou,
    /// It ran into a problem, and said this about it.
    Problem(&'a str),
}

impl Core {
    /// Every consumer of what an agent does hears of it here, in this order.
    pub(crate) fn note_activity(&self, agent_id: &str, activity: Activity<'_>) {
        let moves_turn = !matches!(activity, Activity::Working);
        match activity {
            Activity::Started => {
                self.journal_agent(agent_id, Happened::Working);
                self.edits.working(agent_id);
                if let Some(place) = super::activity_log::place(self, agent_id) {
                    self.activity_log.turn_started(agent_id, place.cwd);
                }
                super::board::chat_goal::turn_started(self, agent_id);
                super::why::turn_started(agent_id);
                self.ship.turn_started(agent_id);
                super::worktree::build_output::turn_started(self, agent_id);
                self.seen.started(agent_id);
                self.inbox.started(agent_id);
                super::pins::turn_started(self, agent_id);
                self.boards.wake.now();
                let open = self.person_calls.open_questions();
                self.harness.questions.turn_started(agent_id, &open);
                self.notify.send(Signal::TurnStarted {
                    agent_id: agent_id.to_owned(),
                });
            }
            Activity::Working => {
                self.edits.working(agent_id);
                self.seen.working(agent_id);
            }
            Activity::Finished { cancelled } => {
                self.journal_agent(
                    agent_id,
                    if cancelled {
                        Happened::Stopped
                    } else {
                        Happened::Done
                    },
                );
                self.came_to_rest(agent_id);
                super::board::chat_goal::turn_ended(self, agent_id, !cancelled);
                super::questions::turn_ended(self, agent_id);
                super::pins::turn_ended(self, agent_id, !cancelled);
                super::why::turn_ended(self, agent_id);
                if cancelled {
                    self.seen.active(agent_id);
                    self.inbox.stopped(agent_id);
                } else {
                    if self.reports_to_open_lead(agent_id) {
                        self.seen.active(agent_id);
                    } else {
                        self.seen.wants_a_look(agent_id);
                    }
                    let left = super::turn_state::left_open(self, agent_id);
                    self.inbox.ended(agent_id, None, left);
                    super::turn_state::read_work(self, agent_id);
                }
                self.notify.send(Signal::TurnCompleted {
                    agent_id: agent_id.to_owned(),
                    cancelled,
                });
                self.changed(What::Agents);
            }
            Activity::NeedsYou => {
                self.journal_agent(agent_id, Happened::NeedsYou);
                super::settling::needs_person(self, agent_id);
                self.seen.wants_a_look(agent_id);
            }
            Activity::Problem(message) => {
                self.journal_agent(agent_id, Happened::Failed);
                super::settling::needs_person(self, agent_id);
                self.came_to_rest(agent_id);
                super::board::chat_goal::turn_ended(self, agent_id, false);
                super::pins::turn_ended(self, agent_id, false);
                self.seen.wants_a_look(agent_id);
                self.inbox.ended(agent_id, Some(message), None);
                self.notify.send(Signal::Problem {
                    agent_id: agent_id.to_owned(),
                    message: message.to_owned(),
                });
                self.changed(What::Agents);
            }
        }
        if moves_turn {
            super::hosts::route::rows_moved(self);
        }
    }

    fn came_to_rest(&self, agent_id: &str) {
        self.ship.turn_ended(agent_id);
        super::worktree::build_output::turn_ended(self, agent_id);
        if let Some(place) = super::activity_log::place(self, agent_id) {
            self.activity_log.turn_ended(agent_id, place);
        }
    }

    /// How a turn, or the agent, ended: Auto moves its card on, and a worker's lead is sent how
    /// it ended.
    pub(crate) fn note_ending(self: &Arc<Self>, agent_id: &str, ending: Ending) {
        super::board::auto::turn_ended(self, agent_id, &ending);
        let ignored = super::board::offers::offer_turn_ignored(self, agent_id);
        super::spawn::report_to_lead(self, agent_id, ending, ignored);
    }

    /// A lead on a linked host ended a turn, so what it was handed goes on to the person.
    pub(crate) fn note_linked_turn_ended(&self, agent_id: &str) {
        super::questions::turn_ended(self, agent_id);
    }

    /// The agent stopped running, whether it ended by itself or was closed.
    pub(crate) fn note_agent_gone(&self, agent_id: &str) {
        self.journal_agent(agent_id, Happened::Gone);
        super::pictures::tidy(self, true);
        self.changed(What::Agents);
    }

    /// Whether the agent's turns are told to a lead still here, who reads them
    /// in the person's place.
    fn reports_to_open_lead(&self, agent_id: &str) -> bool {
        self.names
            .reports_to(agent_id)
            .is_some_and(|lead| self.open_agents().contains(&lead))
    }

    /// The agent's chat went to sleep. Here it stopped running as for
    /// [`Self::note_agent_gone`]; teammates on other hosts hear it sleeps, and wakes for them.
    pub(crate) fn note_agent_asleep(&self, agent_id: &str) {
        self.journal_here(agent_id, Happened::Gone);
        super::hosts::route::asleep(self, agent_id);
        self.changed(What::Agents);
    }

    fn journal_agent(&self, agent_id: &str, happened: Happened) {
        self.journal_here(agent_id, happened);
        super::hosts::route::happened(self, agent_id, happened);
    }

    fn journal_here(&self, agent_id: &str, happened: Happened) {
        if let Some(named) = self.names.get(agent_id) {
            self.harness
                .note_agent(&named.project, happened.kind(), agent_id);
        }
    }
}

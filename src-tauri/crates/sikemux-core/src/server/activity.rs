//! What an agent just did, told once to both the person's unread marks and
//! the phones' notifications, for chats and agents in terminals alike.

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
        let agent_id = agent_id.to_owned();
        match activity {
            Activity::Started => {
                self.seen.started(&agent_id);
                self.notify.send(Signal::TurnStarted { agent_id });
            }
            Activity::Working => self.seen.working(&agent_id),
            Activity::Finished { cancelled } => {
                if cancelled {
                    self.seen.active(&agent_id);
                } else {
                    self.seen.wants_a_look(&agent_id);
                }
                self.notify.send(Signal::TurnCompleted {
                    agent_id,
                    cancelled,
                });
            }
            Activity::NeedsYou => self.seen.wants_a_look(&agent_id),
            Activity::Problem(message) => {
                self.seen.wants_a_look(&agent_id);
                self.notify.send(Signal::Problem {
                    agent_id,
                    message: message.to_owned(),
                });
            }
        }
    }
}

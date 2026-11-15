//! Where each agent sits in the lists: when it started, which orders them, and whether it is
//! snoozed or settled out of the main list.

use super::*;

/// Kept with the agent across restarts. Empty for an agent the person has never written to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPlace {
    /// When the person last sent it a message, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze: Option<Snooze>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<Settled>,
}

impl ListPlace {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snooze {
    pub until: u64,
    /// When the person snoozed it: only what it asks after this wakes it early.
    pub at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settled {
    pub at: u64,
}

impl Workspaces {
    /// Every kept agent's place, with whether the person pinned its chat.
    pub fn places(&self) -> HashMap<String, (ListPlace, bool)> {
        let published = self.published.locked();
        let saved = &published.saved;
        (saved.agents.iter())
            .map(|kept| {
                let pinned = saved.pinned.iter().any(|pin| {
                    pin.provider == kept.agent.provider
                        && kept.agent.resume_id.as_deref() == Some(pin.session_id.as_str())
                });
                (kept.agent.id.clone(), (kept.place.clone(), pinned))
            })
            .collect()
    }

    /// When each kept agent was started, for those that say.
    pub fn started_at(&self) -> HashMap<String, u64> {
        let published = self.published.locked();
        (published.saved.agents.iter())
            .filter(|kept| kept.agent.created_at > 0)
            .map(|kept| (kept.agent.id.clone(), kept.agent.created_at))
            .collect()
    }

    /// Changes the places of `agent_ids` the workspace keeps, answering whether any changed.
    pub fn move_places(&self, agent_ids: &[String], mut edit: impl FnMut(&mut ListPlace)) -> bool {
        if agent_ids.is_empty() {
            return false;
        }
        self.change(|saved| {
            let mut changed = false;
            for agent_id in agent_ids {
                if let Some(kept) = saved.kept_mut(agent_id) {
                    let before = kept.place.clone();
                    edit(&mut kept.place);
                    changed |= kept.place != before;
                }
            }
            Ok(changed)
        })
        .unwrap_or(false)
    }

    /// The person wrote to the agent, which brings it back to the main list.
    pub fn person_said(&self, agent_id: &str) {
        let now = epoch_ms();
        self.move_places(&[agent_id.to_owned()], |place| {
            place.said_at = Some(now);
            place.snooze = None;
            place.settled = None;
        });
    }
}

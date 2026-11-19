//! What a card keeps when the person asked for it: what they sent and to which agent, the agents
//! that took it on, and how far their work got, from the project's branch to the build they run.
//! It is listed once an agent took it on as work; until then the turn it came in may still answer
//! it.

use serde::{Deserialize, Serialize};
use sikemux_view::goals::DeskPage;
use sikemux_view::now::AskSaid;
use sikemux_view::turn::WorkAt;

use super::{Board, Card, CardStatus};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asked {
    /// The agent the person sent it to.
    pub to: String,
    /// What the first agent it went to with a task called it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// What the person sent under it, the ask itself first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub said: Vec<AskSaid>,
    /// Still in the turn of `to` it came in, whose starts and messages hand it on as work.
    #[serde(default)]
    pub in_turn: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<Holder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nobody_since: Option<u64>,
    /// The `nobody_since` its project's chief was last told of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told_since: Option<u64>,
    /// The commit its holder made for it, on the project's branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_build_at: Option<u64>,
    /// What opened on the desk of `to` while it answered this or the person had it open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<DeskPage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub agent: String,
    /// Where its work stood as it took the ask, so only work done since counts for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Stood>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stood {
    pub changes: WorkAt,
    pub head: Option<String>,
}

impl Asked {
    fn waits_on_its_turn(&self) -> bool {
        self.in_turn && self.owners.is_empty()
    }

    fn status(&self) -> CardStatus {
        match (&self.commit, self.owners.is_empty() && !self.in_turn) {
            (Some(_), _) => CardStatus::Done,
            (None, true) => CardStatus::Open,
            (None, false) => CardStatus::Progress,
        }
    }
}

impl Card {
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn at(&self) -> u64 {
        self.at
    }

    pub fn asked(&self) -> Option<&Asked> {
        self.asked.as_ref()
    }

    /// When it was done or dropped, else when it last changed.
    pub fn ended_at(&self) -> u64 {
        (self.dropped.as_ref()).map_or(self.updated_at, |dropped| dropped.at)
    }

    /// Whether lists show it: a card the person asked for shows once an agent took it on.
    pub fn listed(&self) -> bool {
        self.asked
            .as_ref()
            .is_none_or(|asked| !asked.waits_on_its_turn())
    }

    /// Changes what it keeps of the person's ask, and moves its status to where that work stands.
    pub fn change_asked<T>(&mut self, at: u64, change: impl FnOnce(&mut Asked) -> T) -> Option<T> {
        let asked = self.asked.as_mut()?;
        let changed = change(asked);
        let status = asked.status();
        if status != self.status {
            self.status = status;
            self.updated_at = at;
        }
        Some(changed)
    }
}

impl Board {
    /// The folder of its project on this host.
    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// Adds a card for what the person asked, titled `title`; its number.
    pub fn ask(&mut self, title: String, at: u64, asked: Asked) -> u64 {
        let id = self.next.max(1);
        self.next = id + 1;
        let status = asked.status();
        self.cards.push(Card {
            id,
            title,
            details: String::new(),
            acceptance: String::new(),
            status,
            owner: None,
            parent: None,
            depends_on: Vec::new(),
            created_by: None,
            team: None,
            result: None,
            ticket: None,
            trigger: None,
            fired_on: None,
            ci: Vec::new(),
            arrivals: Vec::new(),
            recordings: Vec::new(),
            goal: super::goal::Goal::default(),
            at,
            updated_at: at,
            acted_at: at,
            dropped: None,
            opened_at: None,
            checked: None,
            commits: Vec::new(),
            pulls: Vec::new(),
            asked: Some(asked),
        });
        self.places.take();
        id
    }

    /// Takes off the cards the person asked for that `answered` picks, which needed no work.
    pub fn forget_answered(&mut self, mut answered: impl FnMut(&Card) -> bool) -> bool {
        let before = self.cards.len();
        self.cards
            .retain(|card| card.asked.is_none() || !answered(card));
        self.places.take();
        self.cards.len() != before
    }
}

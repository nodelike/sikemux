//! Tells the person a goal moved: slices landed, it finished, or its ETA
//! slipped. The board says so once for each change, so this never repeats and
//! never runs on a timer. What moves close together is gathered into one
//! notification, kept off a phone with Sikemux in front; the window decides for
//! itself whether the person is looking.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::time::Duration;

use sikemux_process::sync::Locked;
use sikemux_wire::protocol::NoticeGoal;
use sikemux_wire::slice::Slip;

use super::{
    alert_phones, project_name, Draft, NotificationKind, Notifier, MAX_BODY_CHARS,
    MAX_DETAIL_CHARS, MAX_TITLE_CHARS,
};
use crate::protocol::Event;
use crate::server::Core;
use crate::text::clipped_line;

/// How long after a goal first moves the person is told, so whatever moves with
/// it comes along.
pub(super) const GATHER: Duration = Duration::from_secs(20);
const GOAL_LIFE_MS: u64 = 30 * 60_000;

/// How a goal moved.
pub(crate) struct GoalMoved {
    pub project: String,
    pub goal: u64,
    pub title: String,
    /// The titles of the slices that landed.
    pub landed: Vec<String>,
    pub finished: bool,
    pub slip: Option<Slip>,
}

impl GoalMoved {
    fn gather(&mut self, more: Self) {
        self.landed.extend(more.landed);
        self.finished |= more.finished;
        self.slip = more.slip.or(self.slip);
    }

    /// In a few words. A finished goal says only that.
    fn words(&self) -> String {
        if self.finished {
            return "Goal finished".to_owned();
        }
        let landed = match self.landed.as_slice() {
            [] => None,
            [slice] => Some(format!("Landed {slice}")),
            slices => Some(format!(
                "{} slices landed: {}",
                slices.len(),
                slices.join(", ")
            )),
        };
        let slip = self
            .slip
            .map(|slip| format!("ETA slipped: {}", slip.words()));
        [landed, slip]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(". ")
    }
}

pub(super) type Gathered = BTreeMap<(String, u64), GoalMoved>;

impl Notifier {
    /// Goals moved, which the person hears of shortly, with whatever else moves.
    pub(crate) fn goals_moved(&self, moved: Vec<GoalMoved>) {
        let mut state = self.state.locked();
        for goal in moved {
            match state.goals.entry((goal.project.clone(), goal.goal)) {
                Entry::Occupied(mut gathered) => gathered.get_mut().gather(goal),
                Entry::Vacant(gathered) => {
                    gathered.insert(goal);
                }
            }
        }
        drop(state);
        self.goals_moved.notify_one();
    }
}

/// Tells of what was gathered, as one notification, unless the person turned
/// these off.
pub(super) fn tell(core: &Core, now: u64) {
    let gathered = std::mem::take(&mut core.notify.state.locked().goals);
    if !core.preferences.get().goal_notifications {
        return;
    }
    let Some(((project, card), _)) = gathered.first_key_value() else {
        return;
    };
    let goal = NoticeGoal {
        project: project.clone(),
        card: *card,
    };
    let draft = draft(core, &gathered, now);
    core.window.tell(Event::Notice {
        title: draft.title.clone(),
        body: draft.body.clone(),
        agent_id: String::new(),
        goal: Some(goal),
    });
    alert_phones(core, &draft, now, |phone| {
        !core
            .clients()
            .iter()
            .any(|client| client.peer.is_device(phone) && client.in_front())
    });
}

/// What the person is told of `gathered`: one goal by its name, several by how
/// many. Its card stays the same for a goal, so a phone shows its latest move.
fn draft(core: &Core, gathered: &Gathered, now: u64) -> Draft {
    let mut goals = gathered.iter();
    let (title, body, card, detail) = match (goals.next(), goals.next()) {
        (Some(((project, id), goal)), None) => (
            goal.title.clone(),
            goal.words(),
            format!("goal|{project}|{id}"),
            Some(project_name(core, project)),
        ),
        _ => (
            format!("{} goals moved", gathered.len()),
            (gathered.values())
                .map(|goal| format!("{}: {}", goal.title, goal.words()))
                .collect::<Vec<_>>()
                .join(". "),
            "goals".to_owned(),
            None,
        ),
    };
    Draft {
        kind: NotificationKind::Finished,
        card,
        agent_id: String::new(),
        chat: false,
        provider: String::new(),
        chat_title: None,
        title: clipped_line(&title, MAX_TITLE_CHARS),
        body: clipped_line(&body, MAX_BODY_CHARS),
        detail: detail.map(|detail| clipped_line(&detail, MAX_DETAIL_CHARS)),
        request_id: None,
        allow_option_id: None,
        reject_option_id: None,
        at: now,
        expires_at: now + GOAL_LIFE_MS,
    }
}

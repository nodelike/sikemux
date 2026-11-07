//! The one look the person takes at a goal whose work changes what people see: due once every
//! slice that shows something is Built, with every picture those slices hold.

use sikemux_view::inbox::{InboxLook, LookPicture, LookSlice};

use super::*;

/// What the person answered when they last looked at what a goal's work shows.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Looked {
    pub at: u64,
    /// The slices it covered, by card.
    pub covered: Vec<u64>,
    /// What the person wrote it needs; none once it looked right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_work: Option<String>,
}

/// Where the look at a goal stands.
pub(crate) enum Look<'a> {
    /// Nothing in it changes what people see.
    Unneeded,
    /// Some of what shows something is not Built yet.
    Ahead,
    /// The person is to look at what these slices show now.
    Due(Vec<&'a Card>),
    /// The person said it needs work, and nothing they looked at was built again since.
    SentBack,
    Passed,
}

impl Look<'_> {
    /// Nothing about it keeps the goal from being done.
    pub(crate) fn settled(&self) -> bool {
        matches!(self, Look::Unneeded | Look::Passed)
    }

    /// The person is still to look.
    pub(crate) fn coming(&self) -> bool {
        matches!(self, Look::Ahead | Look::Due(_))
    }
}

impl Slice {
    /// Its work changes what people see: its change touches the project's pages, or it holds a
    /// picture or recording.
    fn shows(&self) -> bool {
        self.pages
            || (self.evidence.iter())
                .any(|evidence| matches!(evidence.proof, Proof::Picture { .. }))
    }

    fn built_at(&self) -> Option<u64> {
        self.pass_of(&Step::Gate(Gate::Built)).map(|pass| pass.at)
    }

    /// Its pictures worth looking at, as slice `card`'s: from before its first commit, and on
    /// its latest.
    fn to_look_at(&self, card: u64) -> impl Iterator<Item = LookPicture> + '_ {
        let head = self.head();
        (self.evidence.iter())
            .filter(|evidence| matches!(evidence.proof, Proof::Picture { .. }))
            .filter_map(move |evidence| {
                let before = head.is_some() && evidence.commit.is_none();
                (before || evidence.commit.as_deref() == head).then(|| LookPicture {
                    card,
                    before,
                    evidence: evidence.clone(),
                })
            })
    }
}

/// Where the look at `goal` stands. A slice built again since the person last looked, or one
/// they did not see, makes it due again.
pub(crate) fn look<'a>(board: &'a Board, goal: &'a Card) -> Look<'a> {
    let shown: Vec<&Card> = (board.held_by_goal(goal).into_iter())
        .filter(|card| card.slice.as_ref().is_some_and(Slice::shows))
        .collect();
    let built = |card: &Card| card.slice.as_ref().and_then(Slice::built_at);
    if shown.is_empty() {
        return Look::Unneeded;
    }
    if shown.iter().any(|card| built(card).is_none()) {
        return Look::Ahead;
    }
    let Some(looked) = &goal.looked else {
        return Look::Due(shown);
    };
    let fresh = |card: &&Card| !looked.covered.contains(&card.id) || built(card) > Some(looked.at);
    match (shown.iter().any(fresh), &looked.needs_work) {
        (true, _) => Look::Due(shown),
        (false, Some(_)) => Look::SentBack,
        (false, None) => Look::Passed,
    }
}

/// What the person looks at for the slices `shown`.
pub(crate) fn look_view(core: &Core, shown: &[&Card]) -> InboxLook {
    let held = || (shown.iter()).filter_map(|card| Some((*card, card.slice.as_ref()?)));
    let mut pictures: Vec<LookPicture> = held()
        .flat_map(|(card, slice)| slice.to_look_at(card.id))
        .collect();
    pictures.sort_by_key(|picture| picture.evidence.at);
    InboxLook {
        slices: held()
            .map(|(card, slice)| LookSlice {
                card: card.id,
                title: card.title.clone(),
                promise: slice.promise.clone(),
                owner: card.owner_id().map(str::to_owned),
                owner_name: card.owner_name(core),
            })
            .collect(),
        pictures,
    }
}

/// The person answers the look the goal above card `id` waits for: it looks right, or it needs
/// work as `needs_work` says. The goal's card, and the slices the answer covers.
fn answer(
    board: &mut Board,
    id: u64,
    needs_work: Option<String>,
) -> Result<(u64, Vec<u64>), String> {
    let goal = board.goal_above(board.card(id)?);
    let Look::Due(shown) = look(board, goal) else {
        return Err(format!(
            "{} has nothing waiting for you to look at",
            goal.label()
        ));
    };
    let covered: Vec<u64> = shown.iter().map(|card| card.id).collect();
    let goal = goal.id;
    board.card_mut(goal)?.looked = Some(Looked {
        at: epoch_ms(),
        covered: covered.clone(),
        needs_work,
    });
    Ok((goal, covered))
}

/// Whether the person writing on card `id` answers the look its goal waits for: the look is
/// due and covers it, or `id` is the goal's own card.
pub(crate) fn answers_look(board: &Board, id: u64) -> bool {
    let Ok(card) = board.card(id) else {
        return false;
    };
    let goal = board.goal_above(card);
    match look(board, goal) {
        Look::Due(shown) => goal.id == id || shown.iter().any(|card| card.id == id),
        _ => false,
    }
}

/// The person looked at what the goal above card `id` shows, and it looks right. Its lead hears.
pub(crate) fn looks_right(board: &mut Board, id: u64) -> Result<Option<Notice>, String> {
    let (goal, _) = answer(board, id, None)?;
    let goal = board.card(goal)?;
    Ok(goal.owner_id().map(|lead| Notice {
        to: lead.to_owned(),
        text: format!(
            "The person looked at what {} shows, and it looks right.",
            goal.label()
        ),
    }))
}

/// The person looked at what the goal above card `id` shows, and it needs work, as `text` says,
/// about the piece `evidence` of that card's evidence when given. It goes to the goal's lead as
/// new work; a goal with no lead of its own sends the slices it covers back to their owners, or
/// only the one the note is pinned to. Who hears it, and what.
pub(crate) fn needs_work(
    board: &mut Board,
    id: u64,
    text: &str,
    evidence: Option<u64>,
) -> Result<Vec<(String, String)>, String> {
    let card = board.card_mut(id)?;
    let label = card.label();
    let pinned = match (card.slice.as_mut(), evidence) {
        (Some(slice), _) => {
            let (agent, about) = slice
                .note(text.to_owned(), evidence)
                .map_err(|error| format!("{label}: {error}"))?;
            evidence.map(|_| (agent, format!(" The note is about {about} on {label}.")))
        }
        (None, Some(_)) => return Err(format!("{label} holds no evidence of its own")),
        (None, None) => None,
    };
    let (goal, covered) = answer(board, id, Some(text.to_owned()))?;
    let goal_card = board.card(goal)?;
    let goal_label = goal_card.label();
    let about = pinned.as_ref().map_or("", |(_, about)| about.as_str());
    if let (Some(lead), None) = (goal_card.owner_id(), &goal_card.slice) {
        let message = format!(
            "The person looked at what goal {goal_label} shows, and it needs work.{about}\n\n{text}\n\nTake it up as new work under the goal: add a slice for it, or have a slice's owner reopen it. The person looks again once it is Built."
        );
        return Ok(vec![(lead.to_owned(), message)]);
    }
    let back: Vec<u64> = match &pinned {
        Some(_) => vec![id],
        None => covered,
    };
    let mut told: Vec<(String, String)> = Vec::new();
    for slice_id in back {
        let card = board.card_mut(slice_id)?;
        let Some(slice) = card.slice.as_mut() else {
            continue;
        };
        slice.back_to_work();
        if card.status == CardStatus::Done {
            card.status = CardStatus::Progress;
            card.auto = None;
        }
        card.updated_at = epoch_ms();
        let to = (pinned.as_ref())
            .and_then(|(agent, _)| agent.clone())
            .or_else(|| card.owner_id().map(str::to_owned));
        let Some(to) = to.filter(|to| !told.iter().any(|(told, _)| told == to)) else {
            continue;
        };
        let which = match slice_id == goal {
            true => "it".to_owned(),
            false => card.label(),
        };
        let message = format!(
            "The person looked at what {goal_label} shows, and {which} needs work.{about}\n\n{text}\n\nThe card is back with you: commit what the note asks, or answer it, then mark the card done again."
        );
        told.push((to, message));
    }
    Ok(told)
}

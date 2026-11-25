use super::*;

/// A card added as `adding` asks, by `maker`, or the person or Sikemux when none.
pub(super) fn new_card(core: &Core, id: u64, adding: Adding, maker: Option<String>) -> Card {
    let now = epoch_ms();
    Card {
        id,
        title: adding.title,
        details: adding.details,
        acceptance: adding.acceptance,
        status: CardStatus::Open,
        owner: None,
        parent: adding.parent,
        depends_on: adding.depends_on,
        created_by: maker.map(|agent| Maker {
            name: Some(name_of(core, &agent)),
            id: agent,
        }),
        team: None,
        result: None,
        ticket: adding.ticket,
        trigger: adding.trigger,
        fired_on: adding.fired_on,
        about: adding.about,
        ci: adding.ci.map(|broke| broke.failures).unwrap_or_default(),
        arrivals: Vec::new(),
        recordings: Vec::new(),
        goal: goal::Goal::default(),
        at: now,
        updated_at: now,
        acted_at: now,
        dropped: None,
        opened_at: None,
        checked: None,
        commits: Vec::new(),
        pulls: Vec::new(),
        asked: None,
    }
}

pub fn add_card(
    core: &Arc<Core>,
    project: &str,
    by: &By,
    adding: Adding,
    holder: Option<&str>,
) -> Result<(u64, Value), String> {
    let boards = core.boards();
    let mut state = boards.state.locked();
    archive::make_room(boards, &mut state, project);
    let board = state.made(project);
    if board.cards.len() >= MAX_CARDS {
        return Err(format!(
            "This project's board already holds {MAX_CARDS} cards, the most it may, and none of its finished ones can leave it yet; finish or remove some before adding more"
        ));
    }
    if let Some(id) = arrival::Key::of(&adding).and_then(|key| arrival::holder(board, &key)) {
        let notice = arrival::arrived_again(board.card_mut(id)?, &adding);
        let mut view = agent_view(core, board, board.card(id)?, true);
        boards.save(&mut state, project);
        send_notices(core, notice.into_iter().collect());
        if let Some(fields) = view.as_object_mut() {
            fields.insert(
                "note".into(),
                "It was on the board already, so it went to the card that holds it.".into(),
            );
        }
        return Ok((id, view));
    }
    let goal = adding.parent.is_none();
    board.check_parent(None, adding.parent)?;
    board.check_depends(None, &adding.depends_on)?;
    let id = board.next.max(1);
    board.next = id + 1;
    let maker = match by {
        By::Person | By::Auto(_) => None,
        By::Agent(agent) => Some(agent.clone()),
    };
    let mut card = new_card(core, id, adding, maker);
    let mut note = None;
    if let Some(agent) = holder {
        if board.waits_on(&card).is_empty() {
            take(core, &mut card, agent);
        } else {
            note = Some(
                "It waits on cards not done yet, so it stays open; you will hear when you can claim it.",
            );
        }
    }
    board.cards.push(card);
    remember_team(core, board, id);
    let card = board.card(id)?;
    let mut view = agent_view(core, board, card, true);
    boards.save(&mut state, project);
    drop(state);
    let creator = match by {
        By::Agent(agent) => Some(agent.as_str()),
        By::Person | By::Auto(_) => holder,
    };
    if let Some(creator) = creator.filter(|_| goal) {
        (core.board_above().goal_name_new)(core, project, id, creator);
    }
    if let (Some(note), Some(fields)) = (note, view.as_object_mut()) {
        fields.insert("note".into(), note.into());
    }
    Ok((id, view))
}

/// What a trigger's firing is about, such as one pull request: every firing about it goes to the
/// card the first one added, and to the agent that holds it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    pub about: String,
    /// The commit it brought, which is no news to the card's agent when that agent made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// Where a trigger's firing went on the board.
#[derive(Debug, PartialEq)]
pub enum TriggerCard {
    /// To a card that holds it already, whose agent the board tells itself, or needs nobody told.
    Kept,
    /// To a card nobody holds, for an agent of its own to take up with `hand_off::carry`.
    TakeUp { card: u64, task: String },
    /// To the card of its subject, whose agent has closed or finished: `hand_off::carry` reopens
    /// that agent and sends it `text`, or starts one on `task` when it cannot.
    Owner {
        card: u64,
        agent: String,
        text: String,
        task: String,
    },
}

/// Adds a card for trigger `trigger` of `host` that came due, as the person's own; to the card it
/// added before instead, while that is still open, or, for a firing about a subject, to the card
/// of that subject however long ago it finished.
pub fn add_for_trigger(
    core: &Arc<Core>,
    project: &str,
    (trigger, host): (u64, &str),
    title: &str,
    details: &str,
    subject: Option<Subject>,
) -> Result<TriggerCard, String> {
    let write = own_writes::OwnWrite::TriggerCard {
        trigger,
        host: host.to_owned(),
        title: title.to_owned(),
        details: details.to_owned(),
        subject: subject.clone(),
    };
    if own_writes::sent_home(core, project, write) {
        return Ok(TriggerCard::Kept);
    }
    let adding = Adding {
        title: title.to_owned(),
        details: details.to_owned(),
        trigger: Some(trigger),
        fired_on: Some(host.to_owned()),
        about: subject.as_ref().map(|subject| subject.about.clone()),
        ..Adding::default()
    };
    if let Some(went) =
        (subject.as_ref()).and_then(|subject| to_owner(core, project, &adding, subject))
    {
        return Ok(went);
    }
    let (id, _) = add_card(core, project, &By::Auto("Trigger"), adding, None)?;
    let mut state = core.boards().state.locked();
    let board = state.of(project).ok_or("The board is gone")?;
    let card = board.card(id)?;
    let free = card.status != CardStatus::Done
        && holder(core, card).is_none()
        && board.waits_on(card).is_empty()
        && !state
            .work_of(project, id)
            .is_some_and(|work| work.taking_up);
    if !free {
        return Ok(TriggerCard::Kept);
    }
    let task = format!("{}\n\n{}", card.title, card.details);
    state.change_work(project, id, |work| work.taking_up = true);
    Ok(TriggerCard::TakeUp { card: id, task })
}

/// Keeps a firing about `subject` on the newest card of it an agent ever held, reopened when it
/// finished, and answers what that agent should hear. Its own push it is not told of, nor
/// anything on a card the person dropped.
pub(super) fn to_owner(
    core: &Arc<Core>,
    project: &str,
    adding: &Adding,
    subject: &Subject,
) -> Option<TriggerCard> {
    let boards = core.boards();
    let mut state = boards.state.locked();
    let board = state.of_mut(project)?;
    let key = arrival::Key::of(adding)?;
    let id = board.cards.iter().rev().find(|card| key.held_by(card))?.id;
    let card = board.card_mut(id).ok()?;
    arrival::keep(card, adding);
    let agent = card.owner_id()?.to_owned();
    let own_push = (subject.head.as_ref())
        .is_some_and(|head| card.commits.iter().any(|commit| commit.sha == *head));
    if own_push || card.dropped.is_some() {
        boards.save(&mut state, project);
        return Some(TriggerCard::Kept);
    }
    let reopened = card.status == CardStatus::Done;
    card.status = CardStatus::Progress;
    card.checked = None;
    let text = format!(
        "{} came again about {}, which {} holds{}. What came:\n\n{}",
        adding.title,
        subject.about,
        card.label(),
        if reopened { " and is open again" } else { "" },
        adding.details
    );
    let task = format!("{}\n\n{}", card.title, adding.details);
    let starting = state
        .work_of(project, id)
        .is_some_and(|work| work.taking_up);
    if !starting && !core.agent_now(&agent).open() {
        state.change_work(project, id, |work| work.taking_up = true);
    }
    boards.save(&mut state, project);
    Some(match starting {
        true => TriggerCard::Kept,
        false => TriggerCard::Owner {
            card: id,
            agent,
            text,
            task,
        },
    })
}

/// The agent being started or reopened on trigger card `card` has started, or could not.
pub fn taken_up(core: &Core, project: &str, card: u64) {
    (core.boards().state.locked()).change_work(project, card, |work| work.taking_up = false);
}

pub fn triggers_changed(core: &Core, project: &str) {
    core.changed(What::ReadIn(Topic::Boards, Some(project), None));
}

/// The project's check passed on `sha` of its branch, which holds commits of `agents`: the card
/// each of them works now keeps it, to close by itself once its owner leaves it.
pub fn check_passed(core: &Arc<Core>, project: &str, agents: &[String], sha: &str) {
    let write = own_writes::OwnWrite::CheckPassed {
        agents: agents.to_vec(),
        sha: sha.to_owned(),
    };
    if own_writes::sent_home(core, project, write) {
        return;
    }
    let mut state = core.boards().state.locked();
    let Some(board) = state.of_mut(project) else {
        return;
    };
    let working: Vec<u64> = (agents.iter())
        .filter_map(|agent| {
            (board.cards.iter())
                .filter(|card| {
                    card.status == CardStatus::Progress
                        && card.owner_id() == Some(agent.as_str())
                        && !board.is_goal(card)
                })
                .max_by_key(|card| card.acted_at)
                .map(|card| card.id)
        })
        .collect();
    if working.is_empty() {
        return;
    }
    for card in (board.cards.iter_mut()).filter(|card| working.contains(&card.id)) {
        card.checked = Some(sha.to_owned());
    }
    core.boards().save(&mut state, project);
    drop(state);
    core.boards().wake.now();
}

const MAX_COMMITS: usize = 100;

/// `agent` made `commits`, each a sha and subject, oldest first: the card it works now keeps
/// them. Landed ones are made again on the project's branch, so they replace those of the same
/// subject that had not landed.
pub fn committed(
    core: &Core,
    project: &str,
    agent: &str,
    commits: &[(String, String)],
    landed: bool,
) {
    let write = own_writes::OwnWrite::Committed {
        agent: agent.to_owned(),
        commits: commits.to_vec(),
        landed,
    };
    if own_writes::sent_home(core, project, write) {
        return;
    }
    let mut state = core.boards().state.locked();
    let Some(board) = state.of_mut(project) else {
        return;
    };
    let Some(id) = board.open_working_card(agent).map(|card| card.id) else {
        return;
    };
    let Ok(card) = board.card_mut(id) else {
        return;
    };
    if landed {
        card.commits.retain(|known| {
            known.landed || !commits.iter().any(|(_, subject)| *subject == known.subject)
        });
    }
    let at = epoch_ms();
    for (sha, subject) in commits {
        if !card.commits.iter().any(|known| known.sha == *sha) {
            card.commits.push(CardCommit {
                sha: sha.clone(),
                subject: subject.clone(),
                at,
                landed,
            });
        }
    }
    let over = card.commits.len().saturating_sub(MAX_COMMITS);
    card.commits.drain(..over);
    core.boards().save(&mut state, project);
}

/// What the code host says of `pull`, which card `id`'s commits went up in.
pub fn pull_seen(core: &Core, project: &str, id: u64, pull: CardPull) {
    let write = own_writes::OwnWrite::PullSeen {
        card: id,
        pull: pull.clone(),
    };
    if own_writes::sent_home(core, project, write) {
        return;
    }
    let mut state = core.boards().state.locked();
    let Some(card) = (state.of_mut(project)).and_then(|board| board.card_mut(id).ok()) else {
        return;
    };
    match card
        .pulls
        .iter_mut()
        .find(|known| known.number == pull.number)
    {
        Some(known) if *known == pull => return,
        Some(known) => *known = pull,
        None => card.pulls.push(pull),
    }
    core.boards().save(&mut state, project);
}

/// Whether card `id` is one the board closed by itself, which the person's reopening puts back.
pub(super) fn closed_by_check(core: &Core, project: &str, id: u64) -> bool {
    let state = core.boards().state.locked();
    (state.of(project))
        .and_then(|board| board.card(id).ok())
        .is_some_and(|card| card.status == CardStatus::Done && card.checked.is_some())
}

pub fn ci_card(core: &Core, project: &str) -> Option<CiCard> {
    let state = core.boards().state.locked();
    let card = (state.of(project)?.cards.iter())
        .find(|card| card.status != CardStatus::Done && !card.ci.is_empty())?;
    Some(CiCard {
        label: card.label(),
        owner: card.owner_id().map(str::to_owned),
    })
}

pub(super) fn take(core: &Core, card: &mut Card, agent: &str) {
    card.hold(Owner::of(core, agent));
}

pub fn claim_card(core: &Core, project: &str, agent: &str, id: u64) -> Result<Value, String> {
    let boards = core.boards();
    let mut state = boards.state.locked();
    let board = state
        .of_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let card = board.card(id)?;
    if card.owner_id() == Some(agent) && card.status == CardStatus::Progress {
        let card = board.card_mut(id)?;
        card.acted_at = epoch_ms();
        let view = agent_view(core, board, board.card(id)?, true);
        boards.save(&mut state, project);
        return Ok(view);
    }
    if card.status == CardStatus::Done {
        return Err(format!("#{id} is done already"));
    }
    if let Some(owner) = holder(core, card).filter(|owner| *owner != agent) {
        return Err(format!(
            "{} has #{id} already; pick another card, or message {} about it",
            name_of(core, owner),
            name_of(core, owner)
        ));
    }
    let waits = board.waits_on(card);
    if !waits.is_empty() {
        let list: Vec<String> = waits.iter().map(|id| format!("#{id}")).collect();
        return Err(format!(
            "#{id} waits on {} not done yet; you will hear when you can claim it",
            list.join(", ")
        ));
    }
    take(core, board.card_mut(id)?, agent);
    remember_team(core, board, id);
    let view = agent_view(core, board, board.card(id)?, true);
    boards.save(&mut state, project);
    Ok(view)
}

/// Changes a card and says who should hear of it.
pub fn change_card(
    core: &Core,
    project: &str,
    by: &By,
    id: u64,
    changes: Changes,
) -> Result<(Value, Vec<Notice>), String> {
    if changes.is_empty() {
        return Err("Say what changes: title, details, status, result, dependsOn or parent".into());
    }
    let mut state = core.boards().state.locked();
    let changed = change_in(core, &mut state, project, by, id, changes);
    core.boards().save(&mut state, project);
    changed
}

/// Changes a card on the boards held locked: the one way a card's status moves. Says who
/// should hear of it.
pub(super) fn change_in(
    core: &Core,
    state: &mut State,
    project: &str,
    by: &By,
    id: u64,
    changes: Changes,
) -> Result<(Value, Vec<Notice>), String> {
    let board = state
        .of_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let before = board.card(id)?.clone();
    if let By::Agent(agent) = by {
        if !may_change(core, agent, holder(core, &before)) {
            let owner = before.owner_name(core).unwrap_or_default();
            let shown = before.owner_at_name(core).unwrap_or_default();
            let called = match before.owner_id() {
                Some(owner_id) => crate::server::names::called(&owner, owner_id),
                None => shown.clone(),
            };
            return Err(format!(
                "#{id} is {shown}'s; message {called} with agent_send to change it"
            ));
        }
        if changes.status == Some(CardStatus::Progress) && before.status != CardStatus::Progress {
            return Err(format!("Claim #{id} with task_claim to start on it"));
        }
    }
    if let Some(depends_on) = &changes.depends_on {
        board.check_depends(Some(id), depends_on)?;
    }
    if let Some(Dropping::Drop {
        instead: Some(instead),
        ..
    }) = &changes.drop
    {
        if *instead == id {
            return Err(format!("#{id} cannot replace itself"));
        }
        board.card(*instead)?;
    }
    if changes.opened && before.status != CardStatus::Done {
        return Err(format!("#{id} is not done yet"));
    }
    if let Some(parent) = changes.parent {
        board.check_parent(Some(id), parent)?;
        let keeps = !before.goal.is_none() || !board.decided.on_goal(id).is_empty();
        if parent.is_some() && keeps {
            return Err(format!(
                "#{id} is a goal that keeps decisions or rules of its own, so it stays at the top"
            ));
        }
    }
    let blocked_before = board.blocked();
    let only_opened = changes.opened
        && Changes {
            opened: false,
            ..changes.clone()
        }
        .is_empty();
    let card = board.card_mut(id)?;
    if changes
        .details
        .as_deref()
        .is_some_and(|details| crate::cut::cut_of(details, &card.details))
    {
        return Err(format!(
            "Nothing changed: those details are card #{id}'s cut short, as a list shows them. Read them whole with task_list task {id} first."
        ));
    }
    if let Some(title) = changes.title {
        card.title = title;
    }
    if let Some(details) = changes.details {
        card.details = details;
    }
    if let Some(acceptance) = changes.acceptance {
        card.acceptance = acceptance;
    }
    if let Some(result) = changes.result {
        card.result = (!result.is_empty()).then_some(result);
    }
    if let Some(depends_on) = changes.depends_on {
        card.depends_on = depends_on;
    }
    if let Some(parent) = changes.parent {
        card.parent = parent;
    }
    if changes
        .status
        .is_some_and(|status| status != CardStatus::Done)
    {
        card.checked = None;
    }
    match changes.status {
        Some(CardStatus::Open) => {
            card.status = CardStatus::Open;
            card.owner = None;
            card.team = None;
        }
        Some(CardStatus::Done) => {
            card.status = CardStatus::Done;
            if let (None, By::Agent(agent)) = (holder(core, card), by) {
                card.owner = Some(Owner::of(core, agent));
            }
        }
        Some(CardStatus::Progress) => card.status = CardStatus::Progress,
        None => {}
    }
    let now = epoch_ms();
    match changes.drop {
        Some(Dropping::Drop { why, instead }) => {
            card.dropped = Some(CardDropped {
                at: now,
                by: dropper(core, by),
                why,
                instead,
            });
        }
        Some(Dropping::Undrop) => card.dropped = None,
        None if changes.status.is_some() => card.dropped = None,
        None => {}
    }
    if changes.opened {
        card.opened_at = Some(now);
    }
    if !only_opened {
        card.updated_at = now;
    }
    remember_team(core, board, id);
    let after = board.card(id)?.clone();
    if after.status == CardStatus::Done {
        board.ci_failing.forget(&after.ci);
    }
    let changer = match by {
        By::Person => Some("The person".to_owned()),
        By::Auto(who) => Some((*who).to_owned()),
        By::Agent(agent) if before.owner_id() != Some(agent.as_str()) => Some(name_of(core, agent)),
        By::Agent(_) => None,
    };
    let mut notices = Vec::new();
    if let (Some(changer), Some(owner), CardStatus::Progress) =
        (changer, before.owner_id(), before.status)
    {
        let text = if after.owner.is_none() {
            format!(
                "{changer} moved your card {} back to Open, so it is no longer yours. Stop working on it unless they tell you otherwise.",
                before.label()
            )
        } else if after.status == CardStatus::Done {
            format!("{changer} marked your card {} done.", after.label())
        } else {
            format!(
                "{changer} changed your card {}; task_list with task {id} shows it as it is now.",
                after.label()
            )
        };
        notices.push(Notice {
            to: owner.to_owned(),
            text,
        });
    }
    notices.extend(unblocked(board, &blocked_before, by));
    let view = agent_view(core, board, &after, false);
    let closed_now = (before.status != CardStatus::Done && before.dropped.is_none())
        && (after.status == CardStatus::Done || after.dropped.is_some());
    if closed_now {
        if let Some(owner) = before.owner_id().or_else(|| after.owner_id()) {
            if !has_open_ask(state, owner) {
                let by_self = matches!(by, By::Agent(agent) if agent == owner);
                (core.board_above().task_closed)(core, owner, by_self);
            }
        }
    }
    Ok((view, notices))
}

pub(super) fn has_open_ask(state: &State, agent: &str) -> bool {
    state.boards.values().any(|board| {
        board.cards.iter().filter_map(Card::asked).any(|asked| {
            asked.owners.iter().any(|holder| holder.agent == agent)
                || asked.in_build_at.is_none() && asked.landed_by.as_deref() == Some(agent)
        })
    })
}

pub(super) fn dropper(core: &Core, by: &By) -> DroppedBy {
    match by {
        By::Person => DroppedBy::Person,
        By::Auto(_) => DroppedBy::Board,
        By::Agent(agent) => DroppedBy::Agent {
            id: agent.clone(),
            name: name_of(core, agent),
        },
    }
}

pub(super) fn remove_card(core: &Core, project: &str, id: u64) -> Result<Vec<Notice>, String> {
    let boards = core.boards();
    let mut state = boards.state.locked();
    let board = state
        .of_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let removed = board.card(id)?.clone();
    let blocked_before = board.blocked();
    board.cards.retain(|card| card.id != id);
    board.ci_failing.forget(&removed.ci);
    for card in &mut board.cards {
        card.depends_on.retain(|other| *other != id);
        if card.parent == Some(id) {
            card.parent = removed.parent;
        }
    }
    let mut notices = unblocked(board, &blocked_before, &By::Person);
    state.work.remove(&(project.to_owned(), id));
    let closed_owner = (removed.status != CardStatus::Done && removed.dropped.is_none())
        .then(|| removed.owner_id().map(str::to_owned))
        .flatten()
        .filter(|owner| !has_open_ask(&state, owner));
    boards.save(&mut state, project);
    drop(state);
    if let Some(owner) = closed_owner {
        (core.board_above().task_closed)(core, &owner, false);
    }
    if let (Some(owner), CardStatus::Progress) =
        (removed.owner_id().map(str::to_owned), removed.status)
    {
        notices.push(Notice {
            to: owner,
            text: format!(
                "The person took your card {} off the board. Stop working on it unless they tell you otherwise.",
                removed.label()
            ),
        });
    }
    Ok(notices)
}

/// Tells the owner of each card in progress that waited on others before a change and no
/// longer does, and the maker of each such card nobody holds, else its goal's lead, that it
/// can claim it.
pub(super) fn unblocked(board: &Board, blocked_before: &[u64], by: &By) -> Vec<Notice> {
    let freed = (board.cards.iter())
        .filter(|card| blocked_before.contains(&card.id) && board.waits_on(card).is_empty());
    let mut notices: Vec<Notice> = Vec::new();
    let mut claimable: Vec<(String, Vec<String>)> = Vec::new();
    for card in freed {
        let to = match card.status {
            CardStatus::Progress => card.owner_id(),
            CardStatus::Open if card.owner.is_none() => card
                .maker_id()
                .or_else(|| board.goal_above(card).owner_id()),
            _ => None,
        };
        let Some(to) = to.filter(|to| !matches!(by, By::Agent(agent) if agent == to)) else {
            continue;
        };
        if card.status == CardStatus::Progress {
            notices.push(Notice {
                to: to.to_owned(),
                text: format!(
                    "Every card your card {} waits on is done, so it can go ahead.",
                    card.label()
                ),
            });
            continue;
        }
        match claimable.iter_mut().find(|(agent, _)| agent == to) {
            Some((_, labels)) => labels.push(card.label()),
            None => claimable.push((to.to_owned(), vec![card.label()])),
        }
    }
    notices.extend(claimable.into_iter().map(|(to, labels)| Notice {
        to,
        text: format!(
            "Nothing holds up {} any more: what it waits on is done. Claim it with task_claim if it is yours to do.",
            labels.join(", ")
        ),
    }));
    notices
}

pub(super) fn send_notices(core: &Arc<Core>, notices: Vec<Notice>) {
    if notices.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        for notice in notices {
            let _ = (core.board_above().messages_from_board)(
                &core,
                notice.to,
                notice.text,
                "Sent by Sikemux's board about a card you hold, not by another agent.".to_owned(),
            )
            .await;
        }
    });
}

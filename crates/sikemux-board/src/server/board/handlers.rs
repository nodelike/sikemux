use super::*;

pub async fn add(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "task_add")?;
    let params = &request.params;
    if ["ticket", "key", "url"]
        .iter()
        .any(|key| params.get(*key).is_some_and(|value| !value.is_null()))
    {
        return Err(
            "Only the person puts a Jira or Linear ticket on the board; add the card without one"
                .into(),
        );
    }
    let adding = Adding::from_params(params)?;
    let claim = match params.get("claim") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(claim)) => *claim,
        Some(_) => return Err("claim must be a boolean".into()),
    };
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = crate::server::caller::project_of(&core, &request);
        let holder = claim.then_some(agent.as_str());
        add_card(&core, &project, &By::Agent(agent.clone()), adding, holder)
            .map(|(_, view)| view)
            .map_err(Into::into)
    })
    .await
    .map_err(|error| error.to_string())
}

pub async fn claim(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "task_claim")?;
    let id = card_id(request.params.get("task").unwrap_or(&Value::Null), "task")?;
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = crate::server::caller::project_of(&core, &request);
        claim_card(&core, &project, &agent, id).map_err(Into::into)
    })
    .await
    .map_err(|error| error.to_string())
}

pub async fn update(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "task_update")?;
    let id = card_id(request.params.get("task").unwrap_or(&Value::Null), "task")?;
    let changes = Changes::from_params(&request.params)?;
    let (view, notices) = {
        let (core, request) = (core.clone(), request.clone());
        blocking(move || {
            let project = crate::server::caller::project_of(&core, &request);
            change_card(&core, &project, &By::Agent(agent), id, changes).map_err(Into::into)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    send_notices(core, notices);
    Ok(view)
}

pub async fn list(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "task_list")?;
    let one = match request.params.get("task") {
        None | Some(Value::Null) => None,
        Some(value) => Some(card_id(value, "task")?),
    };
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = crate::server::caller::project_of(&core, &request);
        let state = core.boards().state.locked();
        let Some(board) = state.of(&project) else {
            if let Some(id) = one {
                return Err(format!("There is no card #{id} on this project's board").into());
            }
            return Ok(json!({ "project": project, "tasks": [], "note": "The project has no tasks; add one with task_add." }));
        };
        if let Some(id) = one {
            return Ok(agent_view(&core, board, board.card(id)?, true));
        }
        let team = core.names().team_of(&agent);
        let rank = |card: &Card| {
            let mine = card.owner_id() == Some(agent.as_str());
            let ours = team_of(&core, board, card).is_some_and(|ours| ours.lead == team);
            let place = if mine {
                0
            } else if ours {
                1
            } else {
                2
            };
            let status = match card.status {
                CardStatus::Progress => 0,
                CardStatus::Open => 1,
                CardStatus::Done => 2,
            };
            (place, status, card.id)
        };
        let all_done = request.params.get("done") == Some(&Value::Bool(true));
        let mut done: Vec<&Card> = board.cards.iter().filter(|card| card.status == CardStatus::Done).collect();
        done.sort_by_key(|card| std::cmp::Reverse(card.id));
        let hidden_done = if all_done { 0 } else { done.len().saturating_sub(RECENT_DONE) };
        let shown_done: HashSet<u64> = done.iter().take(done.len() - hidden_done).map(|card| card.id).collect();
        let mut cards: Vec<&Card> = board
            .cards
            .iter()
            .filter(|card| card.listed())
            .filter(|card| card.status != CardStatus::Done || shown_done.contains(&card.id))
            .collect();
        cards.sort_by_key(|card| rank(card));
        board.place_cards();
        let listed: Vec<Value> = cards
            .iter()
            .map(|card| agent_view(&core, board, card, false))
            .collect();
        board.forget_places();
        let mut note = "Your tasks come first, then your team's. Pass task for one task in full.".to_string();
        if hidden_done > 0 {
            note.push_str(&format!(" {hidden_done} older done tasks are left out; pass done to list them."));
        }
        Ok(json!({
            "project": project,
            "tasks": listed,
            "note": note,
        }))
    })
    .await
    .map_err(|error| error.to_string())
}

pub fn boards(core: &Core) -> Vec<ProjectBoard> {
    let state = core.boards().state.locked();
    state
        .boards
        .values()
        .map(|board| board_view(core, board))
        .collect()
}

pub fn project_view(core: &Core, project: &str) -> ProjectBoard {
    let state = core.boards().state.locked();
    match state.of(project) {
        Some(board) => board_view(core, board),
        None => board_view(core, &Board::empty(&state.key(project), project)),
    }
}

/// How `project`'s board is set up.
pub fn settings_of(core: &Core, project: &str) -> BoardSettings {
    (core.boards().state.locked().of(project))
        .map(|board| board.settings.clone())
        .unwrap_or_default()
}

pub fn project_at(core: &Core, path: &str) -> Result<String, String> {
    if Path::new(path).is_absolute() && path.len() <= 4096 {
        Ok(core.workspaces().project_of(Path::new(path)))
    } else {
        Err("project must be the project's absolute path".into())
    }
}

trait BoardAddParamsExt: Sized {
    fn adding(self) -> Result<Adding, String>;
}

impl BoardAddParamsExt for BoardAddParams {
    fn adding(self) -> Result<Adding, String> {
        Ok(Adding {
            title: title_of(&self.title)?,
            details: given_text("details", self.details)?.unwrap_or_default(),
            acceptance: given_text("acceptance", self.acceptance)?.unwrap_or_default(),
            parent: self.parent.filter(|id| *id > 0),
            depends_on: distinct_cards(self.depends_on, "dependsOn")?,
            ticket: self.ticket.map(BoardTicketParams::ticket).transpose()?,
            ..Adding::default()
        })
    }
}

trait BoardUpdateParamsExt: Sized {
    fn changes(self) -> Result<Changes, String>;
}

impl BoardUpdateParamsExt for BoardUpdateParams {
    fn changes(self) -> Result<Changes, String> {
        Ok(Changes {
            title: self.title.as_deref().map(title_of).transpose()?,
            details: given_text("details", self.details)?,
            acceptance: given_text("acceptance", self.acceptance)?,
            status: self.status,
            result: given_text("result", self.result)?,
            depends_on: self
                .depends_on
                .map(|ids| distinct_cards(ids, "dependsOn"))
                .transpose()?,
            parent: self.parent.map(|id| (id > 0).then_some(id)),
            drop: self.undrop.then_some(Dropping::Undrop),
            opened: self.opened,
        })
    }
}

/// One project's board, empty when it has no cards yet.
pub async fn board(core: &Arc<Core>, params: BoardParams) -> Result<ProjectBoard, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

pub async fn person_add(core: &Arc<Core>, params: BoardAddParams) -> Result<ProjectBoard, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let owner = params.owner.clone();
        if let Some(owner) = &owner {
            let here = core
                .names()
                .get(owner)
                .is_some_and(|named| named.project == project);
            if !here || !core.open_agents().contains(owner) {
                return Err(format!("There is no open agent {owner} in this project").into());
            }
        }
        add_card(
            &core,
            &project,
            &By::Person,
            params.adding()?,
            owner.as_deref(),
        )?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

pub async fn person_update(
    core: &Arc<Core>,
    params: BoardUpdateParams,
) -> Result<ProjectBoard, String> {
    let (view, notices) = {
        let core = core.clone();
        blocking(move || {
            let project = project_at(&core, &params.project)?;
            let id = card_number(Some(params.id), "id")?;
            let changes = params.changes()?;
            let reopening = changes
                .status
                .is_some_and(|status| status != CardStatus::Done)
                && closed_by_check(&core, &project, id);
            let (_, notices) = change_card(&core, &project, &By::Person, id, changes)?;
            if reopening {
                (core.board_above().chief_card_reopened)(&core, &project, id);
            }
            Ok((project_view(&core, &project), notices))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    send_notices(core, notices);
    Ok(view)
}

/// Whether `params` lets a project spend further before the person hears of it.
pub fn raises_spend_ceiling(core: &Core, params: &BoardSettingsParams) -> bool {
    let (Some(ceiling), Ok(project)) = (params.spend_ceiling, project_at(core, &params.project))
    else {
        return false;
    };
    let now = settings_of(core, &project).spend_ceiling;
    ceiling.unwrap_or(spend::STEP_USD) > now.unwrap_or(spend::STEP_USD)
}

/// The person sets who lands the project's work, what counts as landed and the spend they hear
/// of. Answers with the board.
pub async fn person_settings(
    core: &Arc<Core>,
    params: BoardSettingsParams,
) -> Result<ProjectBoard, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        if params.spend_ceiling == Some(Some(0)) {
            return Err("spendCeiling must be at least 1 dollar".into());
        }
        if params.landing.is_some() || params.landed.is_some() || params.spend_ceiling.is_some() {
            let mut state = core.boards().state.locked();
            let board = state.made(&project);
            if let Some(landing) = params.landing {
                board.settings.landing = landing;
            }
            if let Some(landed) = params.landed {
                board.settings.landed = landed;
            }
            if let Some(spend_ceiling) = params.spend_ceiling {
                board.settings.spend_ceiling = spend_ceiling;
            }
            core.boards().save(&mut state, &project);
        }
        core.changed(What::Read(crate::server::changed::Topic::Playbook));
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// Shares the project's board with the org `org`, or with nobody. Answers the org it was shared
/// with before.
pub async fn person_share(
    core: &Arc<Core>,
    project: String,
    org: Option<String>,
) -> Result<Option<String>, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &project)?;
        let mut state = core.boards().state.locked();
        let board = state.made(&project);
        let before = std::mem::replace(&mut board.settings.shared_with, org);
        if board.settings.shared_with != before {
            core.boards().save(&mut state, &project);
        }
        Ok(before)
    })
    .await
    .map_err(|error| error.to_string())
}

/// The board's share takes `now`, what the account lists, unless it changed from `was` meanwhile.
pub async fn share_follows_account(
    core: &Arc<Core>,
    project: String,
    was: Option<String>,
    now: Option<String>,
) -> Result<(), String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &project)?;
        let mut state = core.boards().state.locked();
        let board = state.made(&project);
        if board.settings.shared_with == was {
            board.settings.shared_with = now;
            core.boards().save(&mut state, &project);
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())
}

pub async fn person_answer_proposal(
    core: &Arc<Core>,
    params: BoardProposalParams,
) -> Result<ProjectBoard, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let accepted = {
            let mut state = core.boards().state.locked();
            let accepted = steps::answer_proposal(&mut state, &project, params.accept)?;
            core.boards().save(&mut state, &project);
            accepted
        };
        if !accepted.is_empty() {
            crate::server::places::accepted(&core, &project, accepted)?;
        }
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// An agent proposes places for the project, given as environments, for the person to
/// accept; it can't set them.
pub fn propose_environments(
    core: &Core,
    project: &str,
    agent: &str,
    environments: Vec<Value>,
    why: &str,
) -> Result<Value, String> {
    let project = project_at(core, project)?;
    let by = name_of(core, agent);
    let mut state = core.boards().state.locked();
    let board = state.made(&project);
    steps::propose(board, environments, why, by)?;
    core.boards().save(&mut state, &project);
    Ok(
        json!({ "note": "The person sees this in Needs you. Nothing changes until they accept it." }),
    )
}

pub fn proposals(core: &Core) -> Vec<(String, PlacesProposal)> {
    (core.boards().state.locked().boards.values())
        .filter_map(|board| Some((board.folder.clone(), board.places_proposal.clone()?)))
        .collect()
}

/// Who passes what lands in `project`: Sikemux once it is on the branch, or the person.
pub fn landing(core: &Core, project: &str) -> Landing {
    let state = core.boards().state.locked();
    (state.of(project)).map_or_else(Default::default, |board| board.settings.landing)
}

/// What the person set to count as landed in `project`, when they did.
pub fn landed_set(core: &Core, project: &str) -> Option<sikemux_view::turn::Landed> {
    let state = core.boards().state.locked();
    state.of(project)?.settings.landed
}

/// Whether the place called `name` is one of `project`'s production places.
pub fn is_production(core: &Core, project: &str, name: &str) -> bool {
    let places = core.places().of(project).places;
    (places.iter()).any(|place| place.shown.production && place.shown.name == name)
}

/// Whether `project` deploys somewhere that isn't production, and whether it has a
/// production place, by its places.
pub fn deploys_to(core: &Core, project: &str) -> (bool, bool) {
    let places = core.places().of(project).places;
    let production = places.iter().any(|place| place.shown.production);
    let elsewhere = places.iter().any(|place| !place.shown.production);
    (elsewhere, production)
}

/// Keeps `project`'s places with its board, where clients read them.
pub fn mirror_places(
    core: &Core,
    project: &str,
    places: Vec<crate::server::places::Place>,
    confirmed: Option<u64>,
) {
    let mut state = core.boards().state.locked();
    let Some(board) = state.of_mut(project) else {
        return;
    };
    if board.settings.places == places && board.settings.places_confirmed == confirmed {
        return;
    }
    board.settings.places = places;
    board.settings.places_confirmed = confirmed;
    core.boards().save(&mut state, project);
}

/// A released client removes an environment a folder held. Folders hold none now: each
/// project keeps what it inherited as its own places.
pub async fn person_forget_inherited(
    core: &Arc<Core>,
    params: ForgetInheritedParams,
) -> Result<ProjectBoard, String> {
    let project = project_at(core, &params.project)?;
    let core = core.clone();
    blocking(move || Ok(project_view(&core, &project)))
        .await
        .map_err(|error| error.to_string())
}

/// The address a deploy to the place called `name` in `project` is read healthy at.
pub fn health_of(core: &Core, project: &str, name: &str) -> Option<String> {
    let state = core.boards().state.locked();
    state.of(project)?.settings.health(name)
}

/// What a released client is told when it asks for a part of the board that no longer exists:
/// Auto, slices and their gates, goal reports and assertions.
pub fn retired<P>(_: P) -> Result<ProjectBoard, String> {
    Err("Sikemux's board no longer has this; update the app".into())
}

/// A goal's reports, which no goal keeps any more.
pub fn retired_reports(
    _: sikemux_calls::board::GoalReportsParams,
) -> Vec<sikemux_view::slice::GoalReport> {
    Vec::new()
}

pub async fn person_remove(
    core: &Arc<Core>,
    params: BoardCardParams,
) -> Result<ProjectBoard, String> {
    let (view, notices) = {
        let core = core.clone();
        blocking(move || {
            let project = project_at(&core, &params.project)?;
            let id = card_number(Some(params.id), "id")?;
            let notices = remove_card(&core, &project, id)?;
            Ok((project_view(&core, &project), notices))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    send_notices(core, notices);
    Ok(view)
}

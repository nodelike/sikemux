use super::*;

pub(super) fn chars(text: &str) -> usize {
    text.chars().count()
}

pub(super) fn title_of(value: &str) -> Result<String, String> {
    let title = value.trim();
    if title.is_empty() || chars(title) > MAX_TITLE_CHARS || title.contains('\n') {
        return Err(format!(
            "title must be one line of at most {MAX_TITLE_CHARS} characters"
        ));
    }
    Ok(title.to_owned())
}

pub(super) fn long_text(key: &str, value: &str) -> Result<String, String> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "{key} is {} bytes, more than the {MAX_TEXT_BYTES} it may hold; put long material in a file and name its path",
            value.len()
        ));
    }
    Ok(value.trim().to_owned())
}

pub(super) fn card_number(id: Option<u64>, key: &str) -> Result<u64, String> {
    id.filter(|id| *id > 0)
        .ok_or_else(|| format!("{key} must be a task number, such as 3"))
}

pub(super) fn card_id(value: &Value, key: &str) -> Result<u64, String> {
    card_number(
        match value {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.trim().trim_start_matches('#').parse().ok(),
            _ => None,
        },
        key,
    )
}

pub(super) fn card_ids(value: &Value, key: &str) -> Result<Vec<u64>, String> {
    let Value::Array(list) = value else {
        return Err(format!("{key} must be a list of card numbers"));
    };
    let ids = list
        .iter()
        .map(|item| card_id(item, key))
        .collect::<Result<_, _>>()?;
    distinct_cards(ids, key)
}

/// Each of `ids` once, in their order, every one a card number.
pub(super) fn distinct_cards(ids: Vec<u64>, key: &str) -> Result<Vec<u64>, String> {
    let mut kept = Vec::new();
    for id in ids {
        let id = card_number(Some(id), key)?;
        if !kept.contains(&id) {
            kept.push(id);
        }
    }
    Ok(kept)
}

/// Text the person gave for a card's `key`, checked as a card holds it.
pub(super) fn given_text(key: &str, text: Option<String>) -> Result<Option<String>, String> {
    text.map(|text| long_text(key, &text)).transpose()
}

pub(super) fn optional_id(value: Option<&Value>, key: &str) -> Result<Option<Option<u64>>, String> {
    match value {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) if value.as_u64() == Some(0) => Ok(Some(None)),
        Some(value) => card_id(value, key).map(|id| Some(Some(id))),
    }
}

pub(super) fn optional_text(params: &Value, key: &str) -> Result<Option<String>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => long_text(key, text).map(Some),
        Some(_) => Err(format!("{key} must be text")),
    }
}

/// A new card, as the person or an agent asked for it.
#[derive(Default)]
pub struct Adding {
    pub title: String,
    pub details: String,
    pub acceptance: String,
    pub parent: Option<u64>,
    pub depends_on: Vec<u64>,
    pub ticket: Option<CardTicket>,
    /// The trigger whose firing it is.
    pub trigger: Option<u64>,
    /// The host whose trigger it is.
    pub fired_on: Option<String>,
    /// What the trigger's firing is about.
    pub about: Option<String>,
    /// What CI kept failing on, on the project's branch.
    pub ci: Option<failing::Broke>,
}

pub trait AddingExt: Sized {
    fn from_params(params: &Value) -> Result<Self, String>;
}

impl AddingExt for Adding {
    fn from_params(params: &Value) -> Result<Self, String> {
        let title = match params.get("title") {
            Some(Value::String(title)) => title_of(title)?,
            _ => return Err("title must be the card's one-line title".into()),
        };
        Ok(Self {
            title,
            details: optional_text(params, "details")?.unwrap_or_default(),
            acceptance: optional_text(params, "acceptance")?.unwrap_or_default(),
            parent: optional_id(params.get("parent"), "parent")?.flatten(),
            depends_on: match params.get("dependsOn") {
                None | Some(Value::Null) => Vec::new(),
                Some(value) => card_ids(value, "dependsOn")?,
            },
            ..Self::default()
        })
    }
}

#[derive(Clone, Default)]
pub struct Changes {
    pub title: Option<String>,
    pub details: Option<String>,
    pub acceptance: Option<String>,
    pub status: Option<CardStatus>,
    pub result: Option<String>,
    pub depends_on: Option<Vec<u64>>,
    pub parent: Option<Option<u64>>,
    pub drop: Option<Dropping>,
    pub opened: bool,
}

#[derive(Clone)]
pub enum Dropping {
    Drop { why: String, instead: Option<u64> },
    Undrop,
}

impl Changes {
    pub(super) fn from_params(params: &Value) -> Result<Self, String> {
        Self {
            title: match params.get("title") {
                None | Some(Value::Null) => None,
                Some(Value::String(title)) => Some(title_of(title)?),
                Some(_) => return Err("title must be text".into()),
            },
            details: optional_text(params, "details")?,
            acceptance: optional_text(params, "acceptance")?,
            status: match params.get("status") {
                None | Some(Value::Null) => None,
                Some(Value::String(status)) if status.trim() == "dropped" => None,
                Some(Value::String(status)) => Some(CardStatus::parse(status)?),
                Some(_) => return Err("status must be open, progress, done or dropped".into()),
            },
            result: optional_text(params, "result")?,
            depends_on: match params.get("dependsOn") {
                None | Some(Value::Null) => None,
                Some(value) => Some(card_ids(value, "dependsOn")?),
            },
            parent: optional_id(params.get("parent"), "parent")?,
            drop: None,
            opened: false,
        }
        .dropping(params)
    }

    fn dropping(mut self, params: &Value) -> Result<Self, String> {
        let dropped =
            params.get("status").and_then(Value::as_str).map(str::trim) == Some("dropped");
        let instead = optional_id(params.get("replacedBy"), "replacedBy")?.flatten();
        if !dropped {
            return match instead {
                Some(_) => Err("replacedBy goes with status dropped".into()),
                None => Ok(self),
            };
        }
        let why = (self.result.take())
            .filter(|why| !why.trim().is_empty())
            .ok_or("Say in result, in a few words, why it is dropped")?;
        self.drop = Some(Dropping::Drop { why, instead });
        Ok(self)
    }

    pub(super) fn is_empty(&self) -> bool {
        let Self {
            title,
            details,
            acceptance,
            status,
            result,
            depends_on,
            parent,
            drop,
            opened,
        } = self;
        title.is_none()
            && details.is_none()
            && acceptance.is_none()
            && status.is_none()
            && result.is_none()
            && depends_on.is_none()
            && parent.is_none()
            && drop.is_none()
            && !opened
    }
}

pub trait BoardTicketParamsExt: Sized {
    fn ticket(self) -> Result<CardTicket, String>;
}

impl BoardTicketParamsExt for BoardTicketParams {
    fn ticket(self) -> Result<CardTicket, String> {
        let field = |name: &str, text: String, max: usize| -> Result<String, String> {
            let trimmed = text.trim();
            if trimmed.is_empty() || chars(&text) > max {
                return Err(format!(
                    "the ticket's {name} must be text of at most {max} characters"
                ));
            }
            Ok(trimmed.to_owned())
        };
        let key = field("key", self.key, MAX_KEY_CHARS)?;
        if key.contains(char::is_whitespace) {
            return Err("a ticket's key is one word, such as PROJ-12".into());
        }
        let url = self
            .url
            .map(|url| field("url", url, MAX_URL_CHARS))
            .transpose()?;
        if url
            .as_deref()
            .is_some_and(|url| !url.starts_with("https://") && !url.starts_with("http://"))
        {
            return Err("a ticket's url must be a web address".into());
        }
        Ok(CardTicket {
            key,
            url,
            source: self
                .source
                .map(|source| field("source", source, 40))
                .transpose()?,
            provider: self.provider.unwrap_or(Tracker::Jira),
            synced: None,
            sync_note: None,
            done_unlanded: false,
        })
    }
}

/// The provider paying for the agent's turns, by name, when not its own account.
pub(super) fn paid_by(core: &Core, agent_id: &str) -> Option<String> {
    let chat = core.chats().get(agent_id)?;
    let provider = core
        .workspaces()
        .providers()?
        .find(chat.launch.model_provider.as_deref()?)?;
    Some(provider.label.clone())
}

pub(super) fn name_of(core: &Core, agent_id: &str) -> String {
    core.names().name_or_id(agent_id)
}

/// The agent that answers for a card: its owner while that agent is open, here
/// or on a linked host. An owner that closed answers for nothing, so its card
/// counts as nobody's and can't be left stuck.
pub(super) fn holder<'a>(core: &Core, card: &'a Card) -> Option<&'a str> {
    card.owner_id().filter(|owner| core.agent_now(owner).open())
}

/// Whether `agent_id` may change a card `holder` answers for: its own, one held by
/// an agent it started, or one nobody answers for.
pub(super) fn may_change(core: &Core, agent_id: &str, holder: Option<&str>) -> bool {
    match holder {
        None => true,
        Some(owner) => owner == agent_id || core.names().leads(agent_id, owner),
    }
}

/// The agent on a card, with what it was called and which CLI it ran when it
/// claimed the card, for after it has gone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Owner {
    #[serde(rename = "owner")]
    pub(super) id: String,
    #[serde(rename = "ownerName", default)]
    pub(super) name: Option<String>,
    #[serde(rename = "ownerCli", default)]
    pub(super) cli: Option<String>,
}

impl Owner {
    pub(super) fn of(core: &Core, agent: &str) -> Self {
        Self {
            id: agent.to_owned(),
            name: Some(name_of(core, agent)),
            cli: core.agent_now(agent).cli(),
        }
    }
}

/// The agent that added a card, with what it was called then.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Maker {
    #[serde(rename = "createdBy")]
    pub(super) id: String,
    #[serde(rename = "createdByName", default)]
    pub(super) name: Option<String>,
}

/// The most words a goal's title has and still serves as its short name.
const MAX_NAME_WORDS: usize = 7;
/// The most of the first clause of what the person asked a goal shows while it waits for a name.
const MAX_CLAUSE_CHARS: usize = 60;

/// Whether a goal titled `title` wants a short name: its title is too long to read at a glance,
/// or is only the start of what its details say, as one made from a long first message is.
pub fn wants_name(title: &str, details: &str) -> bool {
    title.split_whitespace().count() > MAX_NAME_WORDS
        || (details.len() > title.len() && details.starts_with(title.trim_end_matches('…')))
}

/// What the person asked for goal `card` of `project`, when its title is too long to go by.
pub fn asked_for_name(core: &Core, project: &str, card: u64) -> Option<String> {
    let state = core.boards().state.locked();
    let goal = state.of(project)?.card(card).ok()?;
    wants_name(&goal.title, &goal.details).then(|| match goal.details.as_str() {
        "" => goal.title.clone(),
        details => details.to_owned(),
    })
}

/// The open goals still waiting for a short name, by project, card and the agent holding each.
pub fn unnamed_goals(core: &Core) -> Vec<(String, u64, String)> {
    let state = core.boards().state.locked();
    state.boards.values().for_each(Board::place_cards);
    let unnamed = (state.boards.values())
        .flat_map(|board| board.cards.iter().map(move |card| (board, card)))
        .filter(|(board, card)| card.status != CardStatus::Done && board.is_goal(card))
        .filter(|(_, card)| wants_name(&card.title, &card.details))
        .filter(|(board, card)| {
            (core.board_above().goal_title)(core, &board.folder, card.id).is_none()
        })
        .filter_map(|(board, card)| {
            Some((board.folder.clone(), card.id, card.owner_id()?.to_owned()))
        })
        .collect();
    state.boards.values().for_each(Board::forget_places);
    unnamed
}

/// The goals `agent` holds, by project and card, whose titles are too long to go by and which
/// `title` is short enough to name.
pub fn goals_to_name(core: &Core, agent: &str, title: &str) -> Vec<(String, u64)> {
    let state = core.boards().state.locked();
    (state.boards.values())
        .flat_map(|board| board.cards.iter().map(move |card| (board, card)))
        .filter(|(_, card)| card.parent.is_none() && card.owner_id() == Some(agent))
        .filter(|(_, card)| wants_name(&card.title, &card.details))
        .filter(|(_, card)| !wants_name(title, &card.details))
        .map(|(board, card)| (board.folder.clone(), card.id))
        .collect()
}

/// The first clause of what the person asked, shown until the goal has its short name.
pub(super) fn first_clause(card: &Card) -> String {
    let asked = if card.details.is_empty() {
        &card.title
    } else {
        &card.details
    };
    let line = (asked.lines())
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let end = [": ", "; ", ". ", "? ", "! ", " - ", " — "]
        .iter()
        .filter_map(|mark| line.find(mark))
        .min()
        .unwrap_or(line.len());
    clipped(
        line.get(..end)
            .unwrap_or(line)
            .trim_end_matches(['.', '?', '!', ':']),
        MAX_CLAUSE_CHARS,
    )
}

impl Card {
    /// `owner` holds it from now, at work on it: its status and times go together.
    pub(super) fn hold(&mut self, owner: Owner) {
        self.owner = Some(owner);
        self.status = CardStatus::Progress;
        self.dropped = None;
        self.updated_at = epoch_ms();
        self.acted_at = self.updated_at;
    }

    /// Which of an agent's cards it works now: the highest of these.
    pub(super) fn work_rank(&self) -> (bool, u64) {
        (self.status == CardStatus::Progress, self.acted_at)
    }

    pub fn owner_id(&self) -> Option<&str> {
        self.owner.as_ref().map(|owner| owner.id.as_str())
    }

    pub(super) fn maker_id(&self) -> Option<&str> {
        self.created_by.as_ref().map(|maker| maker.id.as_str())
    }

    pub(super) fn owner_name(&self, core: &Core) -> Option<String> {
        let owner = self.owner.as_ref()?;
        core.names()
            .name(&owner.id)
            .or_else(|| owner.name.clone())
            .or_else(|| Some(owner.id.clone()))
    }

    pub(super) fn owner_at_name(&self, core: &Core) -> Option<String> {
        let owner = self.owner.as_ref()?;
        let name = core
            .names()
            .name(&owner.id)
            .or_else(|| owner.name.clone())
            .filter(|name| name != &owner.id);
        Some(match name {
            Some(name) => crate::server::names::at_name(&name),
            None => owner.id.clone(),
        })
    }

    pub(super) fn creator_name(&self, core: &Core) -> String {
        match &self.created_by {
            None => "the person".to_owned(),
            Some(maker) => core
                .names()
                .name(&maker.id)
                .or_else(|| maker.name.clone())
                .unwrap_or_else(|| maker.id.clone()),
        }
    }

    pub(super) fn creator_at_name(&self, core: &Core) -> String {
        match &self.created_by {
            None => "the person".to_owned(),
            Some(maker) => {
                let name = core
                    .names()
                    .name(&maker.id)
                    .or_else(|| maker.name.clone())
                    .filter(|name| name != &maker.id);
                match name {
                    Some(name) => crate::server::names::at_name(&name),
                    None => maker.id.clone(),
                }
            }
        }
    }

    pub(super) fn label(&self) -> String {
        format!("#{} {}", self.id, self.title)
    }
}

/// The team a card belongs to: the agent at the top of its owner's chain of
/// leads, else its maker's, else its parent card's. None for a card only the
/// person has touched.
pub(super) fn team_of(core: &Core, board: &Board, card: &Card) -> Option<Team> {
    let card = board
        .up_from(card)
        .find(|card| card.owner_id().or(card.maker_id()).is_some())?;
    let agent = card.owner_id().or(card.maker_id())?;
    if core.names().get(agent).is_none() {
        return card.team.clone().or_else(|| {
            Some(Team {
                lead: agent.to_owned(),
                name: name_of(core, agent),
            })
        });
    }
    let lead = core.names().team_of(agent);
    let name = core
        .names()
        .name(&lead)
        .or_else(|| {
            card.team
                .as_ref()
                .filter(|team| team.lead == lead)
                .map(|team| team.name.clone())
        })
        .unwrap_or_else(|| lead.clone());
    Some(Team { lead, name })
}

pub(super) fn remember_team(core: &Core, board: &mut Board, id: u64) {
    let Ok(card) = board.card(id) else {
        return;
    };
    let team = team_of(core, board, card);
    if let Ok(card) = board.card_mut(id) {
        card.team = team;
    }
}

/// Whether `card` waits on the person: its owner asked them something it is blocked on.
pub(super) fn waits_on_person(core: &Core, card: &Card) -> bool {
    card.status == CardStatus::Progress
        && card
            .owner_id()
            .is_some_and(|owner| (core.board_above().questions_blocks_on_person)(core, owner))
}

/// The column a card shows in: done once done, else waiting on the person
/// while its owner does.
pub(super) fn column_of(core: &Core, card: &Card) -> BoardColumn {
    match card.status {
        _ if waits_on_person(core, card) => BoardColumn::Needs,
        CardStatus::Done => BoardColumn::Done,
        CardStatus::Open => BoardColumn::Open,
        CardStatus::Progress => BoardColumn::Progress,
    }
}

pub(super) fn ticket_above(board: &Board, card: &Card) -> Option<String> {
    (board.up_from(card).skip(1)).find_map(|card| Some(card.ticket.as_ref()?.key.clone()))
}

/// How a card stands, as its column does.
pub(super) fn state_of(core: &Core, card: &Card) -> SliceState {
    match column_of(core, card) {
        BoardColumn::Done => SliceState::Done,
        BoardColumn::Needs => SliceState::NeedsYou,
        BoardColumn::Progress => SliceState::Moving,
        BoardColumn::Open => SliceState::Waiting,
    }
}

pub(super) fn card_view(core: &Core, board: &Board, card: &Card) -> BoardCard {
    let column = column_of(core, card);
    let team = team_of(core, board, card);
    let is_goal = board.is_goal(card);
    BoardCard {
        id: card.id,
        title: card.title.clone(),
        details: card.details.clone(),
        acceptance: card.acceptance.clone(),
        status: card.status,
        owner: card.owner_id().map(str::to_owned),
        owner_name: card.owner_name(core),
        owner_cli: card.owner.as_ref().and_then(|owner| {
            core.agent_now(&owner.id)
                .cli()
                .or_else(|| owner.cli.clone())
        }),
        paid_by: card.owner_id().and_then(|owner| paid_by(core, owner)),
        team: team.as_ref().map(|team| team.lead.clone()),
        team_name: team.map(|team| team.name),
        parent: card.parent,
        depends_on: card.depends_on.clone(),
        waits_on: board.waits_on(card),
        created_by: card.maker_id().map(str::to_owned),
        created_by_name: card.creator_name(core),
        result: card.result.clone(),
        ticket: card.ticket.clone(),
        ticket_above: ticket_above(board, card),
        column,
        question: (column == BoardColumn::Needs)
            .then(|| (core.board_above().questions_blocking)(core, card.owner_id()?))
            .flatten(),
        recordings: card.recordings.clone(),
        is_goal,
        display_title: display_title(core, board, card),
        goal_state: is_goal.then(|| state_of(core, card)),
        goal_stalled: chat_goal::plain_owner_closed(core, card),
        decisions: board.decided.on_goal(card.id),
        rules: card.goal.rules(),
        arrivals: card.arrivals.clone(),
        at: card.at,
        updated_at: card.updated_at,
        dropped: card.dropped.clone(),
        commits: card.commits.clone(),
        pulls: card.pulls.clone(),
    }
}

/// What a goal whose title is too long to go by is called: its short name, or the first clause
/// of what the person asked until that arrives.
pub(super) fn display_title(core: &Core, board: &Board, card: &Card) -> Option<String> {
    (card.parent.is_none() && wants_name(&card.title, &card.details)).then(|| {
        (core.board_above().goal_title)(core, &board.folder, card.id)
            .unwrap_or_else(|| first_clause(card))
    })
}

pub(super) fn board_view(core: &Core, board: &Board) -> ProjectBoard {
    board.place_cards();
    let view = ProjectBoard {
        project: board.folder.clone(),
        cards: board
            .cards
            .iter()
            .filter(|card| card.listed())
            .map(|card| card_view(core, board, card))
            .collect(),
        settings: board.settings.clone(),
        places_proposal: board.places_proposal.clone(),
        problems: Vec::new(),
        triggers: (core.board_above().triggers_on_board)(core, &board.folder),
        audit: audit::view(core, board),
    };
    board.forget_places();
    view
}

/// The card as `task_list` tells an agent of it.
pub(super) fn agent_view(core: &Core, board: &Board, card: &Card, full: bool) -> Value {
    let text = |text: &str| {
        if full {
            text.to_owned()
        } else {
            clipped(text, LISTED_TEXT_CHARS)
        }
    };
    let mut fields = serde_json::Map::new();
    fields.insert("task".into(), json!(card.id));
    fields.insert("title".into(), json!(card.title));
    fields.insert("status".into(), json!(card.status.words()));
    fields.insert("owner".into(), json!(card.owner_at_name(core)));
    fields.insert("createdBy".into(), json!(card.creator_at_name(core)));
    if let Some(owner) = card.owner_id() {
        fields.insert("ownerId".into(), owner.into());
    }
    if let Some(maker) = card.maker_id() {
        fields.insert("createdById".into(), maker.into());
    }
    if !card.details.is_empty() {
        fields.insert("details".into(), text(&card.details).into());
    }
    if !card.acceptance.is_empty() {
        fields.insert("acceptance".into(), text(&card.acceptance).into());
    }
    if let Some(parent) = card.parent {
        fields.insert("parent".into(), parent.into());
    }
    let (done, under) = board.subcards(card.id);
    if under > 0 {
        fields.insert("subtasks".into(), format!("{done}/{under} done").into());
    }
    if !card.depends_on.is_empty() {
        fields.insert("dependsOn".into(), json!(card.depends_on));
        let waits = board.waits_on(card);
        if !waits.is_empty() {
            fields.insert("waitsOn".into(), json!(waits));
        }
    }
    if let Some(ticket) = &card.ticket {
        let mut label = ticket.key.clone();
        if let Some(source) = &ticket.source {
            label.push_str(&format!(" ({source})"));
        }
        fields.insert("ticket".into(), label.into());
    }
    if let Some(result) = &card.result {
        fields.insert("result".into(), text(result).into());
    }
    if waits_on_person(core, card) {
        fields.insert("waitingOnThePerson".into(), true.into());
    }
    if let Some(dropped) = &card.dropped {
        fields.insert("dropped".into(), dropped.why.clone().into());
        if let Some(instead) = dropped.instead {
            fields.insert("replacedBy".into(), instead.into());
        }
    }
    if full {
        let under: Vec<Value> = board
            .cards
            .iter()
            .filter(|other| other.parent == Some(card.id))
            .map(|other| {
                json!({
                    "task": other.id,
                    "title": other.title,
                    "status": other.status.words(),
                    "owner": other.owner_at_name(core),
                    "ownerId": other.owner_id(),
                })
            })
            .collect();
        if !under.is_empty() {
            fields.insert("tasks".into(), under.into());
        }
        if !card.arrivals.is_empty() {
            fields.insert("arrivals".into(), json!(card.arrivals));
        }
        if !card.commits.is_empty() {
            fields.insert("commits".into(), json!(card.commits));
        }
        if !card.pulls.is_empty() {
            fields.insert("pulls".into(), json!(card.pulls));
        }
    }
    Value::Object(fields)
}

/// What a teammate's agent asking about card `id` is told of it: the card as `task_list` gives it,
/// with the decisions standing on it or a goal above it, and the agent that holds or held it.
pub fn told_to_teammate(
    core: &Core,
    project: &str,
    id: u64,
) -> Result<(Value, Option<String>), String> {
    let state = core.boards().state.locked();
    let board = (state.of(project)).ok_or_else(|| format!("There is no task #{id} here"))?;
    let card = board.card(id)?;
    let mut facts = agent_view(core, board, card, true);
    let mut above = vec![card.id];
    let mut parent = card.parent;
    while let Some(id) = parent.filter(|id| !above.contains(id)) {
        above.push(id);
        parent = board.card(id).ok().and_then(|card| card.parent);
    }
    let decisions: Vec<Value> = (board.decided.standing())
        .filter(|decision| decision.goal.is_some_and(|goal| above.contains(&goal)))
        .map(|decision| {
            json!({
                "decided": decision.says(),
                "why": decision.why,
                "by": decision
                    .learned
                    .name
                    .as_deref()
                    .map(crate::server::names::at_name),
            })
        })
        .collect();
    if let Some(fields) = facts.as_object_mut().filter(|_| !decisions.is_empty()) {
        fields.insert("decisions".into(), decisions.into());
    }
    Ok((facts, card.owner_id().map(str::to_owned)))
}

/// The titles of `project`'s tasks `ids`, of those it still holds.
pub fn titles(core: &Core, project: &str, ids: &[u64]) -> HashMap<u64, String> {
    let state = core.boards().state.locked();
    let Some(board) = state.of(project) else {
        return HashMap::new();
    };
    (ids.iter())
        .filter_map(|id| Some((*id, board.card(*id).ok()?.title.clone())))
        .collect()
}

/// The card the agent works, on whichever board, else the one it touched last.
pub(super) fn held_card<'a>(
    state: &'a mut State,
    agent_id: &str,
) -> Option<(&'a String, &'a mut Card)> {
    state
        .boards
        .values_mut()
        .flat_map(|Board { folder, cards, .. }| {
            let folder = &*folder;
            cards.iter_mut().map(move |card| (folder, card))
        })
        .filter(|(_, card)| card.owner_id() == Some(agent_id))
        .max_by_key(|(_, card)| card.work_rank())
}

/// A line naming the agent's card, for the report its lead gets when its
/// turn ends.
pub fn card_line(core: &Core, agent_id: &str) -> Option<String> {
    let mut state = core.boards().state.locked();
    let (_, card) = held_card(&mut state, agent_id)?;
    Some(format!(
        "Its card on the board: {}, {}.",
        card.label(),
        card.status.words()
    ))
}

/// The open card an agent works now, on whichever board, and the board: the highest of them, as
/// [`held_card`] takes, so its report, ticket and goal all name one card.
pub(super) fn open_work<'a>(
    state: &'a State,
    agent_id: &str,
) -> Option<(&'a String, &'a Board, &'a Card)> {
    (state.boards.values())
        .filter_map(|board| Some((&board.folder, board, board.open_working_card(agent_id)?)))
        .max_by_key(|(_, _, card)| card.work_rank())
}

/// The ticket of the card an agent holds, or of the card it sits under.
pub fn ticket_of_agent(core: &Core, agent_id: &str) -> Option<String> {
    let state = core.boards().state.locked();
    let (_, board, card) = open_work(&state, agent_id)?;
    let ticket = board
        .up_from(card)
        .find_map(|card| Some(card.ticket.as_ref()?.key.clone()));
    ticket
}

pub struct Summary {
    pub project: String,
    pub id: u64,
    /// The goal card above it, the one at the top; none when it is at the top.
    pub goal: Option<u64>,
    pub title: String,
    pub done: bool,
    pub owner: Option<String>,
    pub owner_name: Option<String>,
    pub result: Option<String>,
    pub recordings: Vec<InboxRecording>,
    pub updated_at: u64,
}

impl Card {
    pub(super) fn summary(&self, core: &Core, board: &Board) -> Summary {
        Summary {
            project: board.folder.clone(),
            id: self.id,
            goal: Some(board.goal_above(self).id).filter(|goal| *goal != self.id),
            title: display_title(core, board, self).unwrap_or_else(|| self.title.clone()),
            done: self.status == CardStatus::Done,
            owner: self.owner_id().map(str::to_owned),
            owner_name: self.owner_name(core),
            result: self.result.clone(),
            recordings: self.recordings.clone(),
            updated_at: self.updated_at,
        }
    }
}

/// The card the agent works.
pub fn agent_card(core: &Core, agent_id: &str) -> Option<Summary> {
    let mut state = core.boards().state.locked();
    let (project, id) =
        held_card(&mut state, agent_id).map(|(project, card)| (project.clone(), card.id))?;
    let board = state.of(&project)?;
    Some(board.card(id).ok()?.summary(core, board))
}

/// The cards done since `since`.
pub fn cards_done_since(core: &Core, since: u64) -> Vec<Summary> {
    let state = core.boards().state.locked();
    state.boards.values().for_each(Board::place_cards);
    let cards = (state.boards.values())
        .flat_map(|board| board.cards.iter().map(move |card| (board, card)))
        .filter(|(_, card)| card.status == CardStatus::Done && card.updated_at >= since)
        .map(|(board, card)| card.summary(core, board))
        .collect();
    state.boards.values().for_each(Board::forget_places);
    cards
}

/// Keeps a browser recording among the recordings of the card the agent works.
pub fn attach_recording(core: &Arc<Core>, agent_id: &str, recording: InboxRecording) {
    let held_in =
        held_card(&mut core.boards().state.locked(), agent_id).map(|(project, _)| project.clone());
    let Some(project) = held_in else {
        return;
    };
    let write = own_writes::OwnWrite::Recording {
        agent: agent_id.to_owned(),
        recording: recording.clone(),
    };
    if own_writes::sent_home(core, &project, write) {
        return;
    }
    let mut state = core.boards().state.locked();
    let Some((project, card)) = held_card(&mut state, agent_id) else {
        return;
    };
    let project = project.clone();
    if card.recordings.len() >= MAX_RECORDINGS {
        card.recordings.remove(0);
    }
    card.recordings.push(recording);
    core.boards().save(&mut state, &project);
}

/// The goal a card sits under: the top card above it, or itself.
pub struct GoalRef {
    pub project: String,
    pub card: u64,
    /// The title and details of the agent's own card, the work it does now.
    pub live: String,
}

/// The goal the card `agent_id` works sits under, in its own project: its
/// card not done, walked up to the top card above it, done or not.
pub fn goal_of(core: &Core, agent_id: &str) -> Option<GoalRef> {
    let state = core.boards().state.locked();
    let (project, board, live) = open_work(&state, agent_id)?;
    let project = project.clone();
    let goal = board.goal_above(live);
    let text = [live.title.as_str(), live.details.as_str()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(GoalRef {
        project,
        card: goal.id,
        live: text,
    })
}

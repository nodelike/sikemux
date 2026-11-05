//! The moments of a goal, bounded and in time order: what the person typed and what its agents
//! answered, from the session files their CLIs wrote, and the commits, checks, evidence, gates,
//! decisions and held actions the core recorded itself. Text a transcript holds is untrusted, so
//! it is cut down and stripped of pastes and anything that may be a secret here, before any
//! model sees it. Another vendor's transcript never goes to the one asked.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use sikemux_view::slice::{
    Evidence, GateState, LeftOut, LeftOutWhy, Passer, Proof, SavedChat, SourceKind, SourceLink,
    SummarySource, Verdict,
};

use crate::agent_catalog::AgentKind;
use crate::server::board::{self, BoardView, CardView};
use crate::server::chat::feed::Said;
use crate::server::edits::footer;
use crate::server::edits::git;
use crate::server::{saved, transcript, Core};
use crate::sessions::asked::{may_hold_secret, person_prose, prose_of};
use crate::text::clipped;
use crate::trailers;

/// The most agents of one goal read.
const MAX_AGENTS: usize = 8;
const MAX_MOMENTS: usize = 90;
/// What all moments may hold together, about ten thousand tokens.
const MAX_BYTES: usize = 40 * 1024;
const MAX_PERSON_CHARS: usize = 500;
const MAX_ANSWER_CHARS: usize = 900;
const MAX_FACT_CHARS: usize = 300;
/// How much of a session's end is read.
const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;
/// How long reading one goal's sessions may take.
const READ_BUDGET: Duration = Duration::from_secs(10);
const MAX_RAN_PER_AGENT: usize = 12;
const MAX_COMMITS: usize = 30;
const MAX_LOG_COMMITS: &str = "--max-count=400";
/// A prompt written just before its card was added belongs to the card.
const START_SLACK_MS: u64 = 5 * 60 * 1000;
/// Only these agents' one-shot answer keeps no session, uses no tools and runs no hooks.
pub(super) const ASKED: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];
/// Commands that check work, among what an agent ran.
const CHECKS: [&str; 15] = [
    "cargo test",
    "cargo clippy",
    "cargo check",
    "pnpm check",
    "pnpm test",
    "npm test",
    "pytest",
    "go test",
    "swift test",
    "make test",
    "vitest",
    "jest",
    "gh run",
    "gh pr checks",
    "xcodebuild test",
];
/// Where a person's or an answer's line sits in a live chat, by how it begins.
const PLACE_CHARS: usize = 160;

pub(super) struct Moment {
    pub source: SummarySource,
    /// How to find it in the agent's live chat.
    place: Option<String>,
}

/// Something not proven or still failing, which a summary states even if the model did not.
pub(super) struct Gap {
    pub moment: usize,
    pub sentence: String,
}

impl Gathered {
    /// Puts `moment` where its time falls, with what a summary must say of it if the model
    /// does not.
    pub(super) fn insert(&mut self, moment: Moment, gap: Option<String>) {
        let at = moment.source.at;
        let place = self.moments.partition_point(|known| known.source.at <= at);
        self.moments.insert(place, moment);
        for gap in self.gaps.iter_mut().filter(|gap| gap.moment >= place) {
            gap.moment += 1;
        }
        self.gaps.extend(gap.map(|sentence| Gap {
            moment: place,
            sentence,
        }));
    }
}

pub(super) struct Gathered {
    pub card: u64,
    pub title: String,
    pub title_is_chopped: bool,
    pub moments: Vec<Moment>,
    pub gaps: Vec<Gap>,
    pub recorded: Vec<String>,
    pub left_out: Vec<LeftOut>,
    /// The pull requests the goal's cards hold, with when each card last moved.
    pub pulls: Vec<(u64, u64)>,
    /// The agent to ask and the account its goal runs on; none when no agent of the goal's
    /// vendor can be asked.
    pub asked: Option<(AgentKind, Option<String>)>,
    pub lead_label: String,
}

/// One agent of a goal and when it worked on it.
struct Worker {
    id: String,
    name: Option<String>,
    since: u64,
    until: Option<u64>,
}

fn source(kind: SourceKind, at: u64, who: Option<String>, text: String) -> SummarySource {
    SummarySource {
        kind,
        at,
        who,
        text,
        open: None,
    }
}

pub(super) fn fact(
    kind: SourceKind,
    at: u64,
    who: Option<String>,
    text: &str,
    open: SourceLink,
) -> Moment {
    let mut source = source(kind, at, who, clipped(&one_line(text), MAX_FACT_CHARS));
    source.open = Some(open);
    Moment {
        source,
        place: None,
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn top_of<'a>(cards: &'a [CardView], card: &'a CardView) -> &'a CardView {
    let mut top = card;
    for _ in 0..cards.len() {
        let Some(above) = (top.parent).and_then(|id| cards.iter().find(|card| card.id == id))
        else {
            break;
        };
        top = above;
    }
    top
}

/// The cards of goal `id`: its own, and those under it at any depth.
fn family(cards: &[CardView], id: u64) -> Vec<&CardView> {
    (cards.iter())
        .filter(|card| top_of(cards, card).id == id)
        .collect()
}

/// The agents of a goal, the lead first: who holds its cards, who added it, then those the lead
/// started. An agent counts until it took up another goal.
fn workers(core: &Core, board: &BoardView, goal: &CardView, cards: &[&CardView]) -> Vec<Worker> {
    let mut workers: Vec<Worker> = Vec::new();
    let mut add = |id: &str, name: Option<String>, since: u64| {
        if let Some(worker) = workers.iter_mut().find(|worker| worker.id == id) {
            worker.since = worker.since.min(since);
        } else if workers.len() < MAX_AGENTS {
            workers.push(Worker {
                id: id.to_owned(),
                name: name.or_else(|| core.names.name(id)),
                since,
                until: None,
            });
        }
    };
    for card in cards {
        if let Some(owner) = &card.owner {
            add(owner, card.owner_name.clone(), card.at);
        }
    }
    if let Some(maker) = &goal.created_by {
        add(maker, None, goal.at);
    }
    if let Some(lead) = &goal.owner {
        for teammate in core.names.team(lead) {
            add(&teammate, None, goal.at);
        }
    }
    let inside: HashSet<u64> = cards.iter().map(|card| card.id).collect();
    for worker in &mut workers {
        worker.until = (board.cards.iter())
            .filter(|card| !inside.contains(&card.id))
            .filter(|card| card.owner.as_deref() == Some(worker.id.as_str()))
            .filter(|card| card.parent.is_none() && card.at > worker.since)
            .map(|card| card.at)
            .min();
    }
    workers
}

/// What the person's message `text` says, without the tag an answer to an agent's question
/// comes in.
pub(crate) fn person_said(text: &str) -> Option<String> {
    let text = text.trim();
    if !text.starts_with("<question-answer") {
        return person_prose(text).map(|line| clipped(&line, MAX_PERSON_CHARS));
    }
    let question = text
        .split_once("question=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(question, _)| question.replace("&quot;", "\"").replace("&amp;", "&"))?;
    let answer = text.split_once('>')?.1.split("</question-answer>").next()?;
    let line = format!("Asked \"{question}\", you answered: {}", answer.trim());
    (!may_hold_secret(&line)).then(|| clipped(&one_line(&line), MAX_PERSON_CHARS))
}

/// The start and end of a long answer, which are where it says what it did and what is left.
pub(super) fn answer_text(text: &str) -> String {
    let line = one_line(&sikemux_markdown::words(&prose_of(text)));
    let count = line.chars().count();
    if count <= MAX_ANSWER_CHARS {
        return line;
    }
    let tail = MAX_ANSWER_CHARS / 3;
    let start: String = line.chars().take(MAX_ANSWER_CHARS - tail).collect();
    let end: String = line.chars().skip(count - tail).collect();
    format!("{start} […] {end}")
}

fn is_check(title: &str) -> bool {
    CHECKS.iter().any(|check| title.contains(check))
}

fn place_of(text: &str) -> String {
    one_line(text).chars().take(PLACE_CHARS).collect()
}

/// What one agent's turns add, from what its session file said.
struct Turns {
    /// Where its chat opens.
    link: SourceLink,
    who: Option<String>,
    moments: Vec<Moment>,
    /// What it wrote last in the turn, with nothing after it so far.
    answer: Option<(u64, String)>,
    /// The checks it ran, the newest few, each with its tool call.
    ran: Vec<(String, Moment)>,
}

impl Turns {
    fn end(&mut self) {
        let Some((at, text)) = self.answer.take() else {
            return;
        };
        let answer = answer_text(&text);
        if answer.is_empty() {
            return;
        }
        let mut source = source(SourceKind::Answer, at, self.who.clone(), answer);
        source.open = Some(self.link.clone());
        self.moments.push(Moment {
            source,
            place: Some(place_of(&text)),
        });
    }

    fn take(&mut self, at: u64, said: Said) {
        match said {
            Said::Asked(text) => {
                self.end();
                if let Some(line) = person_said(&text) {
                    let mut source = source(SourceKind::Person, at, None, line);
                    source.open = Some(self.link.clone());
                    self.moments.push(Moment {
                        source,
                        place: Some(place_of(&text)),
                    });
                }
            }
            Said::Message(text) => self.answer = Some((at, text)),
            Said::Tool { id, title, failed } => {
                self.answer = None;
                self.tool(at, id, title, failed);
            }
            Said::TurnStarted => {}
            Said::TurnEnded { .. } => self.end(),
            Said::Failed(text) => {
                self.end();
                self.moments.push(fact(
                    SourceKind::TurnFailed,
                    at,
                    self.who.clone(),
                    &prose_of(&text),
                    self.link.clone(),
                ));
            }
        }
    }

    fn tool(&mut self, at: u64, id: String, title: Option<String>, failed: bool) {
        if let Some((_, known)) = self.ran.iter_mut().rev().find(|(known, _)| *known == id) {
            if failed {
                known.source.text = format!("{} (failed)", known.source.text);
            }
            return;
        }
        let Some(title) = title.filter(|title| is_check(title)) else {
            return;
        };
        let command = title.strip_prefix("Bash ").unwrap_or(&title);
        let command = prose_of(command.lines().next().unwrap_or_default());
        if command.is_empty() {
            return;
        }
        if self.ran.len() >= MAX_RAN_PER_AGENT {
            self.ran.remove(0);
        }
        let moment = fact(
            SourceKind::Ran,
            at,
            self.who.clone(),
            &command,
            self.link.clone(),
        );
        self.ran.push((id, moment));
    }
}

fn chat_link(core: &Core, agent: &str, feed: Option<String>, index: Option<u64>) -> SourceLink {
    let saved = core.names.ran_session(agent).map(|ran| SavedChat {
        provider: ran.provider,
        session: ran.session_id,
    });
    SourceLink::Chat {
        agent: agent.to_owned(),
        feed,
        index,
        saved,
    }
}

/// When the line at `offset` was written, or the nearest before it.
fn time_at(times: &[(u64, u64)], offset: u64) -> Option<u64> {
    let after = times.partition_point(|(at, _)| *at <= offset);
    after.checked_sub(1).map(|at| times[at].1)
}

/// What `worker` said in the end of its session file while it worked on the goal.
fn read_session(
    kind: AgentKind,
    path: &Path,
    worker: &Worker,
    link: SourceLink,
    working: bool,
    deadline: Instant,
) -> Vec<Moment> {
    let mut turns = Turns {
        link,
        who: worker.name.clone(),
        moments: Vec::new(),
        answer: None,
        ran: Vec::new(),
    };
    let len = std::fs::metadata(path).map_or(0, |meta| meta.len());
    let mut from = len.saturating_sub(MAX_SESSION_BYTES);
    let since = worker.since.saturating_sub(START_SLACK_MS);
    let until = worker.until.unwrap_or(u64::MAX);
    let mut at = since;
    while let Ok(excerpt) = transcript::read(kind, path, Some(from), 0) {
        for (offset, _, said) in excerpt.said {
            at = time_at(&excerpt.times, offset).unwrap_or(at);
            if at >= since && at < until {
                turns.take(at, said);
            } else {
                turns.answer = None;
            }
        }
        if excerpt.whole || excerpt.next <= from || Instant::now() >= deadline {
            break;
        }
        from = excerpt.next;
    }
    if !working {
        turns.end();
    }
    let ran = turns.ran.into_iter().map(|(_, moment)| moment);
    turns.moments.into_iter().chain(ran).collect()
}

/// Places in `agent`'s live chat: where each thing it was sent or wrote at length starts.
fn places(core: &Core, agent: &str) -> Option<(String, HashMap<String, u64>)> {
    let chat = core.chats.get(agent)?;
    let excerpt = chat
        .feed
        .excerpt(Some(0), usize::MAX, MAX_SESSION_BYTES as usize);
    let mut places = HashMap::new();
    let mut message: Option<(u64, String)> = None;
    let mut settle = |message: &mut Option<(u64, String)>| {
        if let Some((at, text)) = message.take() {
            places.insert(place_of(&text), at);
        }
    };
    let mut asked = Vec::new();
    for (at, _, said) in excerpt.said {
        match said {
            Said::Message(text) => match &mut message {
                Some((_, whole)) => whole.push_str(&text),
                None => message = Some((at, text)),
            },
            Said::Asked(text) => {
                settle(&mut message);
                asked.push((place_of(&text), at));
            }
            _ => settle(&mut message),
        }
    }
    settle(&mut message);
    places.extend(asked);
    Some((excerpt.feed, places))
}

fn place_moments(core: &Core, agent: &str, moments: &mut [Moment]) {
    let Some((feed, places)) = places(core, agent) else {
        return;
    };
    for moment in moments {
        let index = moment.place.as_ref().and_then(|place| places.get(place));
        if let Some(index) = index {
            moment.source.open = Some(chat_link(core, agent, Some(feed.clone()), Some(*index)));
        }
    }
}

/// The commits the goal's agents made since `since`, as signed footers name them: each once,
/// and saying whether it reached the project's branch.
fn commits(core: &Core, project: &str, workers: &[Worker], since: u64) -> Vec<Moment> {
    let dir = Path::new(project);
    let since_arg = format!("--since={}", since / 1000);
    let log = |what: &[&str]| {
        let mut args = vec!["log", since_arg.as_str(), MAX_LOG_COMMITS];
        args.extend_from_slice(what);
        git(dir, &args).unwrap_or_default()
    };
    let here: HashSet<String> = log(&["HEAD", "--format=%H"])
        .lines()
        .map(str::to_owned)
        .collect();
    let log = log(&[
        "--all",
        "--grep=^Agent-Signature: ",
        "--format=%H%x1f%ct%x1f%s%x1f%B%x1e",
    ]);
    let sessions = workers
        .iter()
        .filter_map(|worker| core.names.ran_session(&worker.id))
        .map(|ran| ran.session_id);
    let keys: HashSet<String> = workers
        .iter()
        .map(|worker| worker.id.clone())
        .chain(sessions)
        .collect();
    let holds = |agent: &trailers::Agent| keys.contains(&agent.chat) || keys.contains(agent.key());
    let mut seen: HashSet<String> = HashSet::new();
    let mut found = Vec::new();
    for commit in log.split('\x1e') {
        let parts: Vec<&str> = commit.trim().splitn(4, '\x1f').collect();
        let [sha, at, subject, message] = parts[..] else {
            continue;
        };
        let named = trailers::agents(message);
        if !named.iter().any(holds) {
            continue;
        }
        let landed = here.contains(sha);
        if !seen.insert(subject.to_owned()) && !landed {
            continue;
        }
        if found.len() >= MAX_COMMITS {
            break;
        }
        let verified = footer::verified(core, dir, sha).unwrap_or_default();
        let Some(agent) = verified.iter().find(|agent| holds(agent)) else {
            continue;
        };
        let at = at.parse::<u64>().unwrap_or(0) * 1000;
        let short: String = sha.chars().take(9).collect();
        let place = match landed {
            true => "on the project's branch",
            false => "not on the project's branch",
        };
        let subject = one_line(&prose_of(subject));
        found.push(fact(
            SourceKind::Commit,
            at,
            Some(agent.name.clone()),
            &format!("{short} ({place}): {subject}"),
            SourceLink::Commit {
                sha: sha.to_owned(),
            },
        ));
    }
    found
}

fn evidence_words(evidence: &Evidence, title: &str) -> String {
    let verdict = match evidence.verdict {
        Some(Verdict::Passed) => " passed",
        Some(Verdict::Failed) => " failed",
        None => "",
    };
    match &evidence.proof {
        Proof::Run { command, sha, .. } => {
            let on = sha.as_deref().map(|sha| {
                let short: String = sha.chars().take(9).collect();
                format!(" on {short}")
            });
            format!(
                "On \"{title}\", Sikemux ran `{command}`{}:{verdict}",
                on.unwrap_or_default()
            )
        }
        Proof::Diff {
            head,
            files,
            added,
            removed,
            ..
        } => {
            let short: String = head.chars().take(9).collect();
            let files = files.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            format!("On \"{title}\", the change up to {short}: +{added} -{removed} in {files}")
        }
        Proof::Picture {
            seconds, opened, ..
        } => match (seconds, opened) {
            (Some(seconds), _) => format!("On \"{title}\", a recording of {seconds:.0} s was kept"),
            (None, Some(page)) => format!("On \"{title}\", Sikemux took a screenshot of {page}"),
            (None, None) => format!("On \"{title}\", a screenshot was kept"),
        },
        Proof::Numbers { measures } => {
            let measures: Vec<String> = (measures.iter())
                .map(|measure| match &measure.before {
                    Some(before) => format!("{} {before} → {}", measure.label, measure.after),
                    None => format!("{} {}", measure.label, measure.after),
                })
                .collect();
            format!("On \"{title}\", measured: {}", measures.join(", "))
        }
        Proof::Timeline { moments } => {
            let last = moments.last().map_or("", |moment| moment.text.as_str());
            format!("On \"{title}\", events ending: {last}")
        }
    }
}

fn evidence_key(evidence: &Evidence) -> String {
    match &evidence.proof {
        Proof::Run { command, .. } => format!("run {command}"),
        other => format!("{other:?}"),
    }
}

/// What the core recorded of the goal's cards: evidence, gates passed, notes and results, with
/// any evidence still failing as a gap.
fn card_facts(cards: &[&CardView], moments: &mut Vec<Moment>, gaps: &mut Vec<(usize, String)>) {
    for card in cards {
        if let Some(result) = &card.result {
            moments.push(fact(
                SourceKind::Answer,
                card.updated_at,
                card.owner_name.clone(),
                &format!("Result put on \"{}\": {}", card.title, prose_of(result)),
                SourceLink::Card { card: card.id },
            ));
        }
        let Some(slice) = &card.slice else {
            continue;
        };
        for gate in &slice.gates {
            let (GateState::Passed, Some(at)) = (gate.state, gate.at) else {
                continue;
            };
            let by = match gate.passer {
                Passer::Person => "you".to_owned(),
                Passer::Core => "Sikemux".to_owned(),
                Passer::Owner => gate.agent_name.clone().unwrap_or("its owner".to_owned()),
            };
            moments.push(fact(
                SourceKind::Gate,
                at,
                None,
                &match &gate.environment {
                    Some(environment) => {
                        format!("\"{}\" passed {environment}, by {by}", card.title)
                    }
                    None => format!("\"{}\" passed {:?}, by {by}", card.title, gate.gate),
                },
                SourceLink::Card { card: card.id },
            ));
        }
        for evidence in &slice.evidence {
            let failing = evidence.verdict == Some(Verdict::Failed)
                && !(slice.evidence.iter()).any(|later| {
                    later.at > evidence.at
                        && later.verdict == Some(Verdict::Passed)
                        && evidence_key(later) == evidence_key(evidence)
                });
            let words = evidence_words(evidence, &card.title);
            if failing {
                let what = match &evidence.proof {
                    Proof::Run { command, .. } => format!("`{command}`"),
                    _ => "Its evidence".to_owned(),
                };
                let sentence = format!("{what} still fails on \"{}\".", card.title);
                gaps.push((moments.len(), sentence));
            }
            moments.push(fact(
                SourceKind::Evidence,
                evidence.at,
                None,
                &words,
                SourceLink::Evidence {
                    card: card.id,
                    evidence: evidence.id,
                },
            ));
        }
        for note in slice.notes.iter().filter(|note| note.by.is_none()) {
            if may_hold_secret(&note.text) {
                continue;
            }
            moments.push(fact(
                SourceKind::Note,
                note.at,
                None,
                &format!("On \"{}\" you wrote: {}", card.title, note.text),
                SourceLink::Card { card: card.id },
            ));
        }
    }
}

/// What the goal keeps: its decisions, and its assertions with how far their evidence got. An
/// assertion some slice has not met is a gap.
fn goal_facts(
    core: &Core,
    goal: &CardView,
    cards: &[&CardView],
    moments: &mut Vec<Moment>,
    gaps: &mut Vec<(usize, String)>,
) {
    for decision in goal.decisions.iter().filter(|kept| kept.retired.is_none()) {
        let link = match &decision.learned.agent {
            Some(agent) => chat_link(
                core,
                agent,
                decision.learned.feed.clone(),
                decision.learned.turn,
            ),
            None => SourceLink::Card { card: goal.id },
        };
        moments.push(fact(
            SourceKind::Decision,
            decision.learned.at,
            decision.learned.name.clone(),
            &decision.text,
            link,
        ));
    }
    for assertion in goal.assertions.iter().filter(|kept| kept.retired.is_none()) {
        let asserted: Vec<bool> = (cards.iter())
            .filter_map(|card| card.slice.as_ref())
            .flat_map(|slice| slice.asserts.iter())
            .filter(|asserted| asserted.assertion == assertion.id)
            .map(|asserted| asserted.met)
            .collect();
        let met = asserted.iter().filter(|met| **met).count();
        let words = match asserted.len() {
            0 => "no slice fulfils it yet".to_owned(),
            all => format!("its evidence is in on {met} of {all} slices"),
        };
        if asserted.is_empty() || met < asserted.len() {
            gaps.push((moments.len(), format!("Not proven yet: {}", assertion.text)));
        }
        moments.push(fact(
            SourceKind::Assertion,
            assertion.learned.at,
            None,
            &format!("Must hold: {}; {words}", assertion.text),
            SourceLink::Card { card: goal.id },
        ));
    }
}

/// What the person was asked about the goal's agents' actions that leave this machine.
fn held(core: &Core, workers: &[Worker], moments: &mut Vec<Moment>) {
    let name = |id: &str| {
        (workers.iter())
            .find(|worker| worker.id == id)
            .map(|worker| worker.name.clone())
    };
    for waiting in core.ship.waiting() {
        if let Some(who) = name(&waiting.agent_id) {
            moments.push(fact(
                SourceKind::Held,
                waiting.at,
                who,
                &format!("Held for you, still waiting: {}", waiting.hold.what),
                chat_link(core, &waiting.agent_id, None, None),
            ));
        }
    }
    for answered in core.ship.answered() {
        if let Some(who) = name(&answered.agent_id) {
            let verb = if answered.allowed {
                "You allowed once"
            } else {
                "You refused"
            };
            moments.push(fact(
                SourceKind::Held,
                answered.at,
                who,
                &format!("{verb}: {}", answered.what),
                chat_link(core, &answered.agent_id, None, None),
            ));
        }
    }
}

/// The moments that fit, ahead of everything what the core recorded, then the person's words,
/// then the agents' answers, newest first within each, all back in time order.
fn bounded(moments: Vec<Moment>, gaps: Vec<(usize, String)>) -> (Vec<Moment>, Vec<Gap>) {
    let rank = |kind: SourceKind| match kind {
        SourceKind::Person => 1,
        SourceKind::Answer => 2,
        SourceKind::Ran | SourceKind::TurnFailed => 3,
        _ => 0,
    };
    let mut order: Vec<usize> = (0..moments.len()).collect();
    order.sort_by_key(|at| {
        let source = &moments[*at].source;
        (rank(source.kind), std::cmp::Reverse(source.at))
    });
    let mut bytes = 0;
    let mut kept: Vec<usize> = Vec::new();
    for at in order {
        let size = moments[at].source.text.len() + 40;
        if kept.len() < MAX_MOMENTS && bytes + size <= MAX_BYTES {
            bytes += size;
            kept.push(at);
        }
    }
    kept.sort_by_key(|at| (moments[*at].source.at, *at));
    let number: HashMap<usize, usize> = (kept.iter().enumerate())
        .map(|(new, old)| (*old, new))
        .collect();
    let gaps = (gaps.into_iter())
        .filter_map(|(old, sentence)| {
            Some(Gap {
                moment: *number.get(&old)?,
                sentence,
            })
        })
        .collect();
    let mut slots: Vec<Option<Moment>> = moments.into_iter().map(Some).collect();
    let kept = (kept.into_iter())
        .filter_map(|at| slots[at].take())
        .collect();
    (kept, gaps)
}

/// Whether a card's title is only the start of its details, as one made from a prompt is.
pub(crate) fn title_is_chopped(title: &str, details: &str) -> bool {
    details.len() > title.len() && details.starts_with(title.trim_end_matches('…'))
}

/// The moments of goal `card` of `project`, or none when there is no such goal or no agent on it.
pub(super) fn gather(core: &Core, project: &str, card: u64) -> Option<Gathered> {
    let board = board::project_view(core, project);
    let goal = board.cards.iter().find(|known| known.id == card)?;
    if goal.parent.is_some() {
        return None;
    }
    let cards = family(&board.cards, goal.id);
    let workers = workers(core, &board, goal, &cards);
    let lead = workers.first()?;
    let lead_ran = core.names.ran_session(&lead.id);
    let lead_kind = (goal.owner_cli.as_deref())
        .or_else(|| cards.iter().find_map(|card| card.owner_cli.as_deref()))
        .map(str::to_owned)
        .or_else(|| lead_ran.as_ref().map(|ran| ran.provider.clone()))
        .and_then(|cli| AgentKind::from_name(&cli));
    let lead_label = lead_kind
        .map_or("This goal's agent", |kind| kind.entry().label)
        .to_owned();
    let asked_kind = lead_kind.filter(|kind| ASKED.contains(kind));
    let deadline = Instant::now() + READ_BUDGET;
    let (mut moments, mut gaps) = (Vec::new(), Vec::new());
    let mut left_out = Vec::new();
    let mut account = None;
    for (at, worker) in workers.iter().enumerate() {
        let name = worker.name.clone().unwrap_or_else(|| worker.id.clone());
        let ran = core.names.ran_session(&worker.id);
        let kind = ran
            .as_ref()
            .and_then(|ran| AgentKind::from_name(&ran.provider));
        if kind.is_some() && kind != asked_kind {
            left_out.push(LeftOut {
                name,
                why: LeftOutWhy::OtherVendor,
            });
            continue;
        }
        let file = ran
            .as_ref()
            .zip(asked_kind)
            .and_then(|(ran, kind)| saved::session_file(core, kind, &ran.session_id, &ran.project));
        let (Some((path, config)), Some(kind)) = (file, asked_kind) else {
            left_out.push(LeftOut {
                name,
                why: LeftOutWhy::NoSession,
            });
            continue;
        };
        if at == 0 {
            account = config;
        }
        let working = (core.chats.get(&worker.id)).is_some_and(|chat| chat.feed.in_turn());
        let link = chat_link(core, &worker.id, None, None);
        let mut read = read_session(kind, &path, worker, link, working, deadline);
        place_moments(core, &worker.id, &mut read);
        moments.append(&mut read);
    }
    let since = goal.at.saturating_sub(START_SLACK_MS);
    moments.append(&mut commits(core, project, &workers, since));
    if !goal.details.is_empty() {
        if let Some(line) = person_said(&goal.details) {
            moments.push(fact(
                SourceKind::Person,
                goal.at,
                None,
                &line,
                SourceLink::Card { card: goal.id },
            ));
        }
    }
    card_facts(&cards, &mut moments, &mut gaps);
    goal_facts(core, goal, &cards, &mut moments, &mut gaps);
    held(core, &workers, &mut moments);
    let (moments, gaps) = bounded(moments, gaps);
    let pulls = (cards.iter())
        .filter_map(|card| Some((card.auto.as_ref()?.pull.as_ref()?.number, card.updated_at)))
        .collect();
    Some(Gathered {
        card: goal.id,
        title: goal.title.clone(),
        title_is_chopped: title_is_chopped(&goal.title, &goal.details),
        moments,
        gaps,
        recorded: (goal.decisions.iter())
            .filter(|kept| kept.retired.is_none())
            .map(|kept| kept.text.clone())
            .collect(),
        left_out,
        pulls,
        asked: asked_kind.map(|kind| (kind, account)),
        lead_label,
    })
}

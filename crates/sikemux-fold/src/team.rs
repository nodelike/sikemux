//! Messages between agents, folded so they do not bury what the person asked and what they were
//! answered. Everything in a turn another agent started folds but its last message, and a
//! message sent into the person's own turn folds alone; anything left in view ends a run.

use std::collections::HashMap;

use sikemux_view::chat::{
    ChatPart, ChatRole, LetterKind, SubagentState, TeamAgent, TeamFold, TeamLine, TeamNeed,
    TeamNeedKind, TeamSaid,
};

use super::turns::{is_reply, reply_at};
use super::{Fold, Row};

/// The senders a line shows before it only counts them.
const MOST_MARKS: usize = 4;
/// How many messages a fold opens on, with the rest behind one more click.
const OPENS_ON: u64 = 5;
/// A run is read again whole each time it grows, so one this long ends and the next begins.
const MOST_ROWS: usize = 400;
const MAX_LINE_CHARS: usize = 240;
/// A run this quick says nothing by how long it lasted.
const MIN_TOOK_MS: u64 = 1000;

/// What an agent needs of the person, as the core knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNeed {
    pub agent_id: String,
    pub kind: TeamNeedKind,
    pub text: Option<String>,
}

#[derive(Default)]
pub(super) struct Needs {
    of: Vec<AgentNeed>,
    /// What each run's line says of them, by the run: only the latest run an agent wrote in.
    placed: HashMap<String, TeamNeed>,
    moved: bool,
}

/// A message from another agent, rather than something the person sent or a trigger fired.
fn from_an_agent(row: &Row) -> bool {
    row.msg.role == ChatRole::User
        && !row.msg.letters.is_empty()
        && (row.msg.letters.iter()).all(|letter| letter.kind == LetterKind::Message)
}

/// Whether the row at `at` starts a turn. The first answer to a message is marked as opening a
/// turn too, but only one that follows no message is the agent starting one by itself.
fn starts_turn(rows: &[Row], at: usize) -> bool {
    let row = &rows[at];
    match row.msg.role {
        ChatRole::User => !(from_an_agent(row) && row.steered),
        ChatRole::Assistant => {
            row.msg.opens_turn.is_some()
                && at
                    .checked_sub(1)
                    .is_none_or(|before| rows[before].msg.role != ChatRole::User)
        }
    }
}

fn continues_answer(rows: &[Row], at: usize) -> bool {
    rows[at].msg.role == ChatRole::Assistant && rows[at].msg.opens_turn.is_none()
}

/// Where a change at `from` starts to matter: in another agent's turn, the start of that turn
/// or of the run that reaches into it; in the person's, only the rows around the change.
fn read_from(rows: &[Row], from: usize) -> usize {
    let from = from.min(rows.len() - 1);
    let mut at = from;
    while at > 0 && !starts_turn(rows, at) {
        at -= 1;
    }
    if !from_an_agent(&rows[at]) {
        at = at.max(from.saturating_sub(1));
    }
    let Some(before) = at.checked_sub(1) else {
        return at;
    };
    match &rows[before].msg.team {
        Some(team) if team.shown.is_empty() => rows[..=before]
            .iter()
            .rposition(|row| row.msg.id == team.id)
            .unwrap_or(at),
        _ => at,
    }
}

/// Whether another agent started the turn the row at `at` falls in.
fn agent_turn_before(rows: &[Row], at: usize) -> bool {
    (0..at)
        .rev()
        .find(|before| starts_turn(rows, *before))
        .is_some_and(|before| from_an_agent(&rows[before]))
}

/// A page, a subagent at work and where a goal began stay in view when a turn folds. Unlike a
/// finished turn's own fold, a GIF does not: agents send them in passing, and each would split the run.
fn stays(part: &ChatPart) -> bool {
    match part {
        ChatPart::Tool { page, .. } => page.is_some(),
        ChatPart::Subagent { subagent, .. } => subagent.state == SubagentState::Running,
        ChatPart::AgentNotice { notice, .. } => notice.goal.is_some(),
        _ => false,
    }
}

/// The first thing said in a text, without what marks it up.
fn one_line(text: &str) -> String {
    let line = (text.lines().map(str::trim))
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .trim_start_matches(['#', '>', ' ']);
    let line = (line.strip_prefix("- "))
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
        .replace("**", "")
        .replace('`', "");
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}

/// When a row last says something happened.
fn time_of(row: &Row) -> Option<u64> {
    row.msg.ended_at.max(row.msg.sent_at)
}

/// A run as it is read, row by row.
#[derive(Default)]
struct Run {
    /// Each row in it and the parts it keeps in view.
    rows: Vec<(usize, Vec<u64>)>,
    /// The rows that say something, and how much.
    said: Vec<(usize, u64)>,
    agents: Vec<TeamAgent>,
    latest: Option<TeamSaid>,
}

impl Run {
    fn add(&mut self, rows: &mut [Row], at: usize, kept: Vec<u64>, out: &mut Reading) {
        if self.rows.len() >= MOST_ROWS {
            self.close(rows, out);
        }
        let row = &rows[at];
        for letter in &row.msg.letters {
            let sender = TeamAgent {
                agent_id: letter.agent_id.clone(),
                cli: letter.cli.clone(),
            };
            if !self.agents.contains(&sender) {
                self.agents.push(sender);
            }
            self.latest = Some(TeamSaid {
                name: letter.name.clone(),
                text: one_line(&letter.text),
            });
            out.needs.moved |= out
                .needs
                .of
                .iter()
                .any(|need| need.agent_id == letter.agent_id);
        }
        let writes = (row.msg.parts.iter().enumerate()).any(|(index, part)| {
            matches!(part, ChatPart::Text { .. } | ChatPart::Content { .. })
                && !kept.contains(&(index as u64))
        });
        let says = (row.msg.letters.len() as u64).max(writes as u64);
        if says > 0 {
            self.said.push((at, says));
        }
        self.rows.push((at, kept));
    }

    /// Ends the run, writing what it came to on its rows.
    fn close(&mut self, rows: &mut [Row], out: &mut Reading) {
        let members = std::mem::take(&mut self.rows);
        let said = std::mem::take(&mut self.said);
        let agents = std::mem::take(&mut self.agents);
        let latest = self.latest.take();
        let (Some((first, _)), Some((last, _))) = (members.first(), members.last()) else {
            return;
        };
        let (first, last) = (*first, *last);
        let id = rows[first].msg.id.clone();
        let messages: u64 = said.iter().map(|(_, says)| says).sum();
        let (mut shown, mut from) = (0, first);
        for (at, says) in said.iter().rev() {
            shown += says;
            from = *at;
            if shown >= OPENS_ON {
                break;
            }
        }
        if shown == messages {
            from = first;
        }
        let times = || members.iter().filter_map(|(at, _)| time_of(&rows[*at]));
        let took = (times().min())
            .zip(times().max())
            .map(|(began, ended)| ended - began)
            .filter(|took| *took >= MIN_TOOK_MS);
        let line = TeamLine {
            more: agents.len().saturating_sub(MOST_MARKS) as u64,
            agents: agents.into_iter().take(MOST_MARKS).collect(),
            messages,
            from: rows[from].msg.id.clone(),
            earlier: messages - shown,
            latest,
            need: out.needs.placed.get(&id).cloned(),
            took,
        };
        let v = members.iter().map(|(at, _)| rows[*at].v).max().unwrap_or(0);
        let mut line = Some(line);
        for (at, kept) in members {
            let team = TeamFold {
                id: id.clone(),
                shown: kept,
                line: if at == last { line.take() } else { None },
            };
            put(&mut rows[at], Some(team), v, at, &mut out.touched);
        }
    }
}

fn put(row: &mut Row, team: Option<TeamFold>, v: u64, at: usize, touched: &mut Vec<usize>) {
    if row.msg.team != team {
        row.msg.team = team;
        row.set("team");
        row.v = row.v.max(v);
        touched.push(at);
    }
}

/// What reading the rows gave and took.
struct Reading<'a> {
    touched: Vec<usize>,
    needs: &'a mut Needs,
}

/// The parts of a row in an agent's turn that stay in view while the run around it is closed.
fn kept_parts(row: &Row, index: usize, reply: Option<(usize, usize)>, live: bool) -> Vec<u64> {
    let parts = row.msg.parts.iter().enumerate();
    parts
        .filter(|(at, part)| {
            let replying = reply.is_some_and(|reply| (index, *at) >= reply && is_reply(part));
            live || replying || stays(part)
        })
        .map(|(at, _)| at as u64)
        .collect()
}

fn read(rows: &mut [Row], from: usize, running: bool, needs: &mut Needs) -> Vec<usize> {
    let mut out = Reading {
        touched: Vec::new(),
        needs,
    };
    let mut at = read_from(rows, from);
    let mut agent_turn = agent_turn_before(rows, at);
    let mut run = Run::default();
    while at < rows.len() {
        if rows[at].msg.role == ChatRole::User {
            if from_an_agent(&rows[at]) {
                agent_turn |= !rows[at].steered;
                run.add(rows, at, Vec::new(), &mut out);
            } else {
                agent_turn = false;
                run.close(rows, &mut out);
                put(&mut rows[at], None, 0, at, &mut out.touched);
            }
            at += 1;
            continue;
        }
        agent_turn &= !starts_turn(rows, at);
        let mut end = at;
        while end + 1 < rows.len() && continues_answer(rows, end + 1) {
            end += 1;
        }
        if !agent_turn {
            run.close(rows, &mut out);
            for (index, row) in rows.iter_mut().enumerate().take(end + 1).skip(at) {
                put(row, None, 0, index, &mut out.touched);
            }
            at = end + 1;
            continue;
        }
        let mut first = at;
        while first > 0
            && rows[first - 1].msg.role == ChatRole::Assistant
            && continues_answer(rows, first)
        {
            first -= 1;
        }
        // The answer being written stays in view until the next one comes, and only the turn's
        // last answer has a reply to keep.
        let writing = running && end + 1 == rows.len();
        let last_of_turn = rows
            .get(end + 1)
            .is_none_or(|next| !next.steered || !from_an_agent(next));
        let reply = (last_of_turn && !writing)
            .then(|| reply_at(rows, first, end))
            .flatten();
        for index in at..=end {
            let kept = kept_parts(&rows[index], index, reply, writing && index == end);
            if !kept.is_empty() && kept.len() == rows[index].msg.parts.len() {
                run.close(rows, &mut out);
                put(&mut rows[index], None, 0, index, &mut out.touched);
                continue;
            }
            let ends = !kept.is_empty();
            run.add(rows, index, kept, &mut out);
            if ends {
                run.close(rows, &mut out);
            }
        }
        at = end + 1;
    }
    run.close(rows, &mut out);
    out.touched
}

/// The folds of rows read on their own, such as an earlier page.
pub(super) fn settle_page(rows: &mut [Row]) {
    if !rows.is_empty() {
        read(rows, 0, false, &mut Needs::default());
    }
}

impl Fold {
    /// What other agents need of the person now, which the core works out across its chats.
    pub fn set_needs(&mut self, needs: Vec<AgentNeed>) {
        if self.needs.of != needs {
            self.needs.of = needs;
            self.needs.moved = true;
            self.settle_last_run();
        }
    }

    pub(super) fn needs_moved(&mut self) {
        self.needs.moved = !self.needs.of.is_empty();
    }

    /// Works out the runs again from the first main row that changed.
    pub(super) fn settle_team(&mut self, from: usize) {
        if self.main.rows.is_empty() {
            return;
        }
        let running = self.head.running;
        for at in read(&mut self.main.rows, from, running, &mut self.needs) {
            self.main.queue(at);
        }
        if std::mem::take(&mut self.needs.moved) {
            self.place_needs();
        }
    }

    /// Says each need on the latest run its agent wrote in. Needs change rarely, so this reads
    /// the whole chat rather than keeping track of where each agent last wrote.
    fn place_needs(&mut self) {
        let mut left: Vec<&AgentNeed> = self.needs.of.iter().collect();
        let mut placed = HashMap::new();
        for row in self.main.rows.iter().rev() {
            let Some(team) = row.msg.team.as_ref().filter(|_| !left.is_empty()) else {
                continue;
            };
            left.retain(|need| {
                let letter =
                    (row.msg.letters.iter()).find(|letter| letter.agent_id == need.agent_id);
                let Some(letter) = letter else {
                    return true;
                };
                placed.entry(team.id.clone()).or_insert_with(|| TeamNeed {
                    kind: need.kind,
                    name: letter.name.clone(),
                    text: need.text.clone(),
                });
                false
            });
        }
        let mut changed = Vec::new();
        for (at, row) in self.main.rows.iter_mut().enumerate() {
            let Some(team) = row.msg.team.as_mut() else {
                continue;
            };
            let wanted = placed.get(&team.id);
            let Some(line) = team
                .line
                .as_mut()
                .filter(|line| line.need.as_ref() != wanted)
            else {
                continue;
            };
            line.need = wanted.cloned();
            row.set("team");
            changed.push(at);
        }
        self.needs.placed = placed;
        for at in changed {
            self.main.touch(at);
        }
    }
}

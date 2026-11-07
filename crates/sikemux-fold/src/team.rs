//! Messages between agents, folded so they do not bury what the person asked and what they were
//! answered. Messages from other agents in a row fold into one run; anything else ends it.

use std::collections::HashMap;

use sikemux_view::chat::{
    ChatRole, LetterKind, TeamAgent, TeamFold, TeamLine, TeamNeed, TeamNeedKind, TeamSaid,
};

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

/// Where a change at `from` starts to matter: the start of the run it continues.
fn read_from(rows: &[Row], from: usize) -> usize {
    let at = from.min(rows.len() - 1);
    let Some(before) = at.checked_sub(1) else {
        return at;
    };
    match &rows[before].msg.team {
        Some(team) => rows[..=before]
            .iter()
            .rposition(|row| row.msg.id == team.id)
            .unwrap_or(at),
        None => at,
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
    rows: Vec<usize>,
    /// The rows that say something, and how much.
    said: Vec<(usize, u64)>,
    agents: Vec<TeamAgent>,
    latest: Option<TeamSaid>,
}

impl Run {
    fn add(&mut self, rows: &mut [Row], at: usize, out: &mut Reading) {
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
        self.said.push((at, row.msg.letters.len() as u64));
        self.rows.push(at);
    }

    /// Ends the run, writing what it came to on its rows.
    fn close(&mut self, rows: &mut [Row], out: &mut Reading) {
        let members = std::mem::take(&mut self.rows);
        let said = std::mem::take(&mut self.said);
        let agents = std::mem::take(&mut self.agents);
        let latest = self.latest.take();
        let (Some(first), Some(last)) = (members.first(), members.last()) else {
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
        let times = || members.iter().filter_map(|at| time_of(&rows[*at]));
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
        let v = members.iter().map(|at| rows[*at].v).max().unwrap_or(0);
        let mut line = Some(line);
        for at in members {
            let team = TeamFold {
                id: id.clone(),
                shown: Vec::new(),
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

fn read(rows: &mut [Row], from: usize, needs: &mut Needs) -> Vec<usize> {
    let mut out = Reading {
        touched: Vec::new(),
        needs,
    };
    let mut run = Run::default();
    for at in read_from(rows, from)..rows.len() {
        if from_an_agent(&rows[at]) {
            run.add(rows, at, &mut out);
        } else {
            run.close(rows, &mut out);
            put(&mut rows[at], None, 0, at, &mut out.touched);
        }
    }
    run.close(rows, &mut out);
    out.touched
}

/// The folds of rows read on their own, such as an earlier page.
pub(super) fn settle_page(rows: &mut [Row]) {
    if !rows.is_empty() {
        read(rows, 0, &mut Needs::default());
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
        for at in read(&mut self.main.rows, from, &mut self.needs) {
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

//! Messages between agents, folded so they do not bury what the person asked and what they were
//! answered. Messages from other agents in a row fold into one run; anything else ends it.

use sikemux_view::chat::{ChatRole, LetterKind, TeamAgent, TeamFold, TeamLine, TeamSaid};

use super::{Fold, Row};

/// The senders a line shows before it only counts them.
const MOST_MARKS: usize = 4;
/// How many messages a fold opens on, with the rest behind one more click.
const OPENS_ON: u64 = 5;
/// A run is read again whole each time it grows, so one this long ends and the next begins.
const MOST_ROWS: usize = 400;
const MAX_LINE_CHARS: usize = 240;
/// How a worker's report opens when its turn ended with an answer, which follows.
pub const REPORT_OPENING: &str = "Finished its turn. Its final answer:";
/// A run this quick says nothing by how long it lasted.
const MIN_TOOK_MS: u64 = 1000;

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
    match rows.get(before).and_then(|row| row.msg.team.as_ref()) {
        Some(team) => (rows.iter().take(before + 1))
            .rposition(|row| row.msg.id == team.id)
            .unwrap_or(at),
        None => at,
    }
}

/// The first words of a message, without what marks them up. A report opens on words of
/// Sikemux's own, so its first words are the answer's.
pub fn first_words(text: &str) -> String {
    let text = text.strip_prefix(REPORT_OPENING).unwrap_or(text);
    let words = sikemux_markdown::words(text);
    let line = (words.lines().map(str::trim))
        .chain(text.lines().map(str::trim))
        .find(|line| !line.is_empty() && !line.starts_with("```"))
        .unwrap_or_default();
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => format!("{}…", line.get(..at).unwrap_or(line)),
        None => line.to_owned(),
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
}

impl Run {
    fn add(&mut self, rows: &mut [Row], at: usize, touched: &mut Vec<usize>) {
        if self.rows.len() >= MOST_ROWS {
            self.close(rows, touched);
        }
        let Some(row) = rows.get(at) else {
            return;
        };
        for letter in &row.msg.letters {
            let sender = TeamAgent {
                agent_id: letter.agent_id.clone(),
                cli: letter.cli.clone(),
            };
            if !self.agents.contains(&sender) {
                self.agents.push(sender);
            }
        }
        self.said.push((at, row.msg.letters.len() as u64));
        self.rows.push(at);
    }

    /// Ends the run, writing what it came to on its rows.
    #[expect(
        clippy::indexing_slicing,
        reason = "a run holds only the places of rows `read` walked, all within `rows`"
    )]
    fn close(&mut self, rows: &mut [Row], touched: &mut Vec<usize>) {
        let members = std::mem::take(&mut self.rows);
        let said = std::mem::take(&mut self.said);
        let agents = std::mem::take(&mut self.agents);
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
        let latest = rows[last].msg.letters.last().map(|letter| TeamSaid {
            agent_id: letter.agent_id.clone(),
            name: letter.name.clone(),
            text: first_words(&letter.text),
        });
        let line = TeamLine {
            more: agents.len().saturating_sub(MOST_MARKS) as u64,
            agents: agents.into_iter().take(MOST_MARKS).collect(),
            messages,
            from: rows[from].msg.id.clone(),
            earlier: messages - shown,
            latest,
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
            put(&mut rows[at], Some(team), v, at, touched);
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

/// Works out the runs from the one a change at `from` falls in, and gives the rows that moved.
fn read(rows: &mut [Row], from: usize) -> Vec<usize> {
    let mut touched = Vec::new();
    let mut run = Run::default();
    for at in read_from(rows, from)..rows.len() {
        if rows.get(at).is_some_and(from_an_agent) {
            run.add(rows, at, &mut touched);
        } else {
            run.close(rows, &mut touched);
            if let Some(row) = rows.get_mut(at) {
                put(row, None, 0, at, &mut touched);
            }
        }
    }
    run.close(rows, &mut touched);
    touched
}

/// The folds of rows read on their own, such as an earlier page.
pub(super) fn settle_page(rows: &mut [Row]) {
    if !rows.is_empty() {
        read(rows, 0);
    }
}

impl Fold {
    /// Works out the runs again from the first main row that changed.
    pub(super) fn settle_team(&mut self, from: usize) {
        if self.main.rows.is_empty() {
            return;
        }
        for at in read(&mut self.main.rows, from) {
            self.main.queue(at);
        }
    }
}

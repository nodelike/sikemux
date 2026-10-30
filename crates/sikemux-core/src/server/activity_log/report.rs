//! A whole UTC day of this host's activity in totals, for the anonymous usage
//! report. Never which projects, prompts or commits: only how much.

use std::collections::HashMap;

use rusqlite::{params, Connection};

use crate::launch::model_feed::Feed;

use super::pricing::Prices;
use super::{ActivityLog, DAY_MS};

#[derive(Default, Debug, PartialEq)]
pub(crate) struct ReportedDay {
    /// UTC days counted from the Unix epoch.
    pub day: i64,
    pub sessions: i64,
    pub resumed: i64,
    pub chat_sessions: i64,
    pub terminal_sessions: i64,
    pub turns: i64,
    pub agent_ms: i64,
    /// The most agents at work at once, from the turns that began that day.
    pub peak_agents: i64,
    pub commits: i64,
    pub agent_commits: i64,
    pub tokens: Vec<ReportedTokens>,
    /// How often each feature was used, by name and who asked for it.
    pub features: Vec<(String, String, i64)>,
}

#[derive(Default, Debug, PartialEq)]
pub(crate) struct ReportedTokens {
    pub agent: String,
    pub model: String,
    pub speed: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    /// What the tokens would cost at API prices, when the model has a known price.
    pub cost_usd: Option<f64>,
}

impl ActivityLog {
    /// Each UTC day from `first` up to but not including `end` that had any activity.
    pub(crate) fn reported_days(&self, first: i64, end: i64) -> Vec<ReportedDay> {
        let feed = self.feed.as_ref().map(Feed::current).unwrap_or_default();
        let prices = Prices::new(&feed, &self.price_overrides());
        self.write_uses();
        self.with_database(|db| days_between(db, first, end, &prices))
            .unwrap_or_default()
    }
}

fn days_between(
    db: &Connection,
    first: i64,
    end: i64,
    prices: &Prices,
) -> rusqlite::Result<Vec<ReportedDay>> {
    let (from_ms, to_ms) = (first * DAY_MS, end * DAY_MS);
    let mut days: HashMap<i64, ReportedDay> = HashMap::new();

    let mut launches = db.prepare(
        "SELECT at_ms, transport, resumed FROM launches WHERE at_ms >= ?1 AND at_ms < ?2",
    )?;
    let mut rows = launches.query(params![from_ms, to_ms])?;
    while let Some(row) = rows.next()? {
        let day = day_at(&mut days, row.get(0)?);
        day.sessions += 1;
        day.resumed += row.get::<_, i64>(2)?;
        match row.get::<_, String>(1)?.as_str() {
            "chat" => day.chat_sessions += 1,
            _ => day.terminal_sessions += 1,
        }
    }

    let mut turns: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    let mut statement = db.prepare(
        "SELECT started_ms, ended_ms FROM turns WHERE started_ms >= ?1 AND started_ms < ?2",
    )?;
    let mut rows = statement.query(params![from_ms, to_ms])?;
    while let Some(row) = rows.next()? {
        let (started, ended): (i64, i64) = (row.get(0)?, row.get(1)?);
        let day = day_at(&mut days, started);
        day.turns += 1;
        day.agent_ms += (ended - started).max(0);
        turns.entry(day.day).or_default().push((started, ended));
    }

    let mut statement = db
        .prepare("SELECT at_ms, agent IS NOT NULL FROM commits WHERE at_ms >= ?1 AND at_ms < ?2")?;
    let mut rows = statement.query(params![from_ms, to_ms])?;
    while let Some(row) = rows.next()? {
        let day = day_at(&mut days, row.get(0)?);
        day.commits += 1;
        day.agent_commits += row.get::<_, i64>(1)?;
    }

    let mut statement = db.prepare(
        "SELECT at_ms, agent, model, speed, input, output, cache_read, cache_write FROM tokens
         WHERE at_ms >= ?1 AND at_ms < ?2",
    )?;
    let mut rows = statement.query(params![from_ms, to_ms])?;
    while let Some(row) = rows.next()? {
        let day = day_at(&mut days, row.get(0)?);
        let (agent, model, speed): (String, String, String) =
            (row.get(1)?, row.get(2)?, row.get(3)?);
        let entry =
            match day.tokens.iter().position(|entry| {
                entry.agent == agent && entry.model == model && entry.speed == speed
            }) {
                Some(index) => &mut day.tokens[index],
                None => {
                    day.tokens.push(ReportedTokens {
                        agent,
                        model,
                        speed,
                        ..ReportedTokens::default()
                    });
                    day.tokens.last_mut().expect("just pushed")
                }
            };
        let [input, output, cache_read, cache_write]: [i64; 4] =
            [row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?];
        entry.input += input;
        entry.output += output;
        entry.cache_read += cache_read;
        entry.cache_write += cache_write;
    }

    let mut statement =
        db.prepare("SELECT day, feature, via, count FROM features WHERE day >= ?1 AND day < ?2")?;
    let mut rows = statement.query(params![first, end])?;
    while let Some(row) = rows.next()? {
        let day = day_at(&mut days, row.get::<_, i64>(0)? * DAY_MS);
        day.features.push((row.get(1)?, row.get(2)?, row.get(3)?));
    }

    let mut days: Vec<ReportedDay> = days.into_values().collect();
    for day in &mut days {
        day.peak_agents = turns.get(&day.day).map_or(0, |turns| most_at_once(turns));
        for entry in &mut day.tokens {
            entry.cost_usd = prices.lookup(&entry.model).map(|rate| {
                let rates = rate.at(&entry.speed);
                entry.input as f64 * rates.input
                    + entry.output as f64 * rates.output
                    + entry.cache_read as f64 * rates.cache_read
                    + entry.cache_write as f64 * rates.cache_write
            });
        }
    }
    days.sort_by_key(|day| day.day);
    Ok(days)
}

fn day_at(days: &mut HashMap<i64, ReportedDay>, at_ms: i64) -> &mut ReportedDay {
    let day = at_ms.div_euclid(DAY_MS);
    days.entry(day).or_insert_with(|| ReportedDay {
        day,
        ..ReportedDay::default()
    })
}

/// The most spans open at one moment. One that ends as another starts does not overlap it.
fn most_at_once(spans: &[(i64, i64)]) -> i64 {
    let mut edges: Vec<(i64, i64)> = spans
        .iter()
        .flat_map(|&(start, end)| [(start, 1), (end.max(start), -1)])
        .collect();
    edges.sort_unstable();
    let (mut open, mut most) = (0, 0);
    for (_, change) in edges {
        open += change;
        most = most.max(open);
    }
    most
}

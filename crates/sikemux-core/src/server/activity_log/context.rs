//! What each model call of a chat's session carried, and each compaction of it, kept call by
//! call for the day they happen in and then added up into one row per chat and UTC day: how big
//! its calls were, how often the cache was rewritten cold and how often it compacted.

use std::collections::BTreeMap;
use std::sync::Arc;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;

use super::{in_background, ActivityLog, DAY_MS};

/// A call this big that read less than half of it from the cache wrote the cache again cold.
const COLD_CONTEXT: u64 = 100_000;

/// One call to the model, with how much context it carried and how much of that the cache held.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelCall {
    pub at_ms: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl ModelCall {
    pub(crate) fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    fn cold(&self) -> bool {
        self.context() > COLD_CONTEXT && self.cache_read * 2 < self.context()
    }
}

/// The session was compacted: by its agent as it filled, by the person, or by the core while idle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Compaction {
    pub at_ms: u64,
    pub trigger: String,
    pub before: u64,
    pub after: u64,
    pub ms: u64,
    /// What writing the summary cost, when nothing else called the model beside it.
    pub cost: Option<ModelCall>,
}

pub(super) const TABLES: &str = "CREATE TABLE model_calls (
         at_ms INTEGER NOT NULL,
         chat TEXT NOT NULL,
         project TEXT NOT NULL,
         input INTEGER NOT NULL,
         cache_read INTEGER NOT NULL,
         cache_write INTEGER NOT NULL,
         output INTEGER NOT NULL
     );
     CREATE INDEX model_calls_by_time ON model_calls (at_ms);
     CREATE TABLE compactions (
         at_ms INTEGER NOT NULL,
         chat TEXT NOT NULL,
         project TEXT NOT NULL,
         trigger TEXT NOT NULL,
         before INTEGER NOT NULL,
         after INTEGER NOT NULL,
         ms INTEGER NOT NULL,
         input INTEGER,
         cache_read INTEGER,
         cache_write INTEGER,
         output INTEGER
     );
     CREATE INDEX compactions_by_time ON compactions (at_ms);
     CREATE TABLE session_days (
         day INTEGER NOT NULL,
         chat TEXT NOT NULL,
         project TEXT NOT NULL,
         calls INTEGER NOT NULL,
         context_median INTEGER NOT NULL,
         context_p90 INTEGER NOT NULL,
         context_max INTEGER NOT NULL,
         cold_rewrites INTEGER NOT NULL,
         cold_tokens INTEGER NOT NULL,
         compacted_by_agent INTEGER NOT NULL,
         compacted_by_person INTEGER NOT NULL,
         compacted_idle INTEGER NOT NULL,
         compacted_tokens INTEGER NOT NULL,
         PRIMARY KEY (day, chat)
     );";

/// One chat's model calls and compactions over a UTC day.
#[derive(Default)]
struct Day {
    project: String,
    contexts: Vec<u64>,
    cold_rewrites: u64,
    cold_tokens: u64,
    by_agent: u64,
    by_person: u64,
    idle: u64,
    compacted_tokens: u64,
}

/// A day's model calls and compactions over every chat, for the usage report.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct ReportedModelUse {
    pub calls: i64,
    pub cold_rewrites: i64,
    pub compacted_by_agent: i64,
    pub compacted_by_person: i64,
    pub compacted_idle: i64,
}

impl ActivityLog {
    /// Keeps the calls and compactions a chat's session made since it was last asked to.
    pub(crate) fn session_used(
        self: &Arc<Self>,
        chat: &str,
        project: &str,
        calls: Vec<ModelCall>,
        compactions: Vec<Compaction>,
    ) {
        if calls.is_empty() && compactions.is_empty() {
            return;
        }
        let (log, chat, project) = (self.clone(), chat.to_owned(), project.to_owned());
        in_background(move || {
            log.with_database(|db| {
                let saving = db.unchecked_transaction()?;
                for call in &calls {
                    saving.execute(
                        "INSERT INTO model_calls (at_ms, chat, project, input, cache_read, cache_write, output)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            call.at_ms as i64,
                            chat,
                            project,
                            call.input as i64,
                            call.cache_read as i64,
                            call.cache_write as i64,
                            call.output as i64
                        ],
                    )?;
                }
                for compaction in &compactions {
                    let cost = compaction.cost.as_ref();
                    let part = |part: fn(&ModelCall) -> u64| cost.map(|cost| part(cost) as i64);
                    saving.execute(
                        "INSERT INTO compactions (at_ms, chat, project, trigger, before, after, ms, input, cache_read, cache_write, output)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            compaction.at_ms as i64,
                            chat,
                            project,
                            compaction.trigger,
                            compaction.before as i64,
                            compaction.after as i64,
                            compaction.ms as i64,
                            part(|cost| cost.input),
                            part(|cost| cost.cache_read),
                            part(|cost| cost.cache_write),
                            part(|cost| cost.output)
                        ],
                    )?;
                }
                roll_up(&saving)?;
                saving.commit()
            });
        });
    }
}

/// Adds up every whole UTC day still kept call by call into its chats' day rows, and lets go of
/// the calls and compactions it added up.
pub(super) fn roll_up(db: &Connection) -> rusqlite::Result<()> {
    let today = epoch_ms() as i64 / DAY_MS * DAY_MS;
    let oldest: Option<i64> = db.query_row(
        "SELECT MIN(at_ms) FROM (SELECT at_ms FROM model_calls UNION ALL SELECT at_ms FROM compactions)",
        [],
        |row| row.get(0),
    )?;
    if oldest.is_none_or(|oldest| oldest >= today) {
        return Ok(());
    }
    let mut days: BTreeMap<(i64, String), Day> = BTreeMap::new();
    let mut statement = db.prepare(
        "SELECT at_ms, chat, project, input, cache_read, cache_write FROM model_calls WHERE at_ms < ?1",
    )?;
    let mut rows = statement.query(params![today])?;
    while let Some(row) = rows.next()? {
        let call = ModelCall {
            at_ms: row.get::<_, i64>(0)? as u64,
            input: row.get::<_, i64>(3)? as u64,
            cache_read: row.get::<_, i64>(4)? as u64,
            cache_write: row.get::<_, i64>(5)? as u64,
            output: 0,
        };
        let day = days
            .entry((call.at_ms as i64 / DAY_MS, row.get(1)?))
            .or_default();
        day.project = row.get(2)?;
        day.contexts.push(call.context());
        if call.cold() {
            day.cold_rewrites += 1;
            day.cold_tokens += call.context();
        }
    }
    let mut statement = db.prepare(
        "SELECT at_ms, chat, project, trigger, before, after FROM compactions WHERE at_ms < ?1",
    )?;
    let mut rows = statement.query(params![today])?;
    while let Some(row) = rows.next()? {
        let day = days
            .entry((row.get::<_, i64>(0)? / DAY_MS, row.get(1)?))
            .or_default();
        day.project = row.get(2)?;
        match row.get::<_, String>(3)?.as_str() {
            "idle" => day.idle += 1,
            "manual" => day.by_person += 1,
            _ => day.by_agent += 1,
        }
        day.compacted_tokens += (row.get::<_, i64>(4)? - row.get::<_, i64>(5)?).max(0) as u64;
    }
    for ((day, chat), mut counted) in days {
        counted.contexts.sort_unstable();
        let at = |share: usize| {
            let last = counted.contexts.len().saturating_sub(1);
            counted
                .contexts
                .get(last * share / 100)
                .copied()
                .unwrap_or(0) as i64
        };
        db.execute(
            "INSERT INTO session_days (day, chat, project, calls, context_median, context_p90, context_max,
                 cold_rewrites, cold_tokens, compacted_by_agent, compacted_by_person, compacted_idle, compacted_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(day, chat) DO UPDATE SET
                 context_median = CASE WHEN excluded.calls > calls THEN excluded.context_median ELSE context_median END,
                 context_p90 = CASE WHEN excluded.calls > calls THEN excluded.context_p90 ELSE context_p90 END,
                 calls = calls + excluded.calls,
                 context_max = MAX(context_max, excluded.context_max),
                 cold_rewrites = cold_rewrites + excluded.cold_rewrites,
                 cold_tokens = cold_tokens + excluded.cold_tokens,
                 compacted_by_agent = compacted_by_agent + excluded.compacted_by_agent,
                 compacted_by_person = compacted_by_person + excluded.compacted_by_person,
                 compacted_idle = compacted_idle + excluded.compacted_idle,
                 compacted_tokens = compacted_tokens + excluded.compacted_tokens",
            params![
                day,
                chat,
                counted.project,
                counted.contexts.len() as i64,
                at(50),
                at(90),
                at(100),
                counted.cold_rewrites as i64,
                counted.cold_tokens as i64,
                counted.by_agent as i64,
                counted.by_person as i64,
                counted.idle as i64,
                counted.compacted_tokens as i64
            ],
        )?;
    }
    db.execute("DELETE FROM model_calls WHERE at_ms < ?1", params![today])?;
    db.execute("DELETE FROM compactions WHERE at_ms < ?1", params![today])?;
    Ok(())
}

/// Each day's calls and compactions over every chat, by UTC day.
pub(super) fn reported(
    db: &Connection,
    first: i64,
    end: i64,
) -> rusqlite::Result<Vec<(i64, ReportedModelUse)>> {
    let mut statement = db.prepare(
        "SELECT day, SUM(calls), SUM(cold_rewrites), SUM(compacted_by_agent), SUM(compacted_by_person),
             SUM(compacted_idle)
         FROM session_days WHERE day >= ?1 AND day < ?2 GROUP BY day",
    )?;
    let rows = statement.query_map(params![first, end], |row| {
        Ok((
            row.get(0)?,
            ReportedModelUse {
                calls: row.get(1)?,
                cold_rewrites: row.get(2)?,
                compacted_by_agent: row.get(3)?,
                compacted_by_person: row.get(4)?,
                compacted_idle: row.get(5)?,
            },
        ))
    })?;
    rows.collect()
}

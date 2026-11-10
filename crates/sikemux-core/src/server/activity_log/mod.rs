//! What agents did on this host, for the Activity page: the sessions they
//! opened, the turns they ran, the commits made and the tokens their
//! transcripts or answers report, kept in one database in the core's data folder and
//! priced from the model feed and the person's own prices.

mod features;
mod pricing;
mod report;

pub(crate) use features::Counted;

use super::changed::Topic;
use super::connection::blocking;
use super::CoreResult;
use crate::server::changed::What;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::agent_catalog::AgentKind;
use crate::launch::model_feed::Feed;
use crate::launch::named_account;
use pricing::Prices;
pub use pricing::{PriceOverride, PricingInfo};
pub(crate) use report::{ReportedDay, ReportedTokens};

use super::Core;
use crate::sessions::tokens::{ReadState, TokenCount};
use crate::sessions::transcript;

const DAY_MS: i64 = 86_400_000;
const HEATMAP_DAYS: i64 = 371;
const MAX_TRANSCRIPT_READ: u64 = 64 * 1024 * 1024;
const MAX_TURN_COMMITS: usize = 500;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const DATABASE: &str = "activity.sqlite3";
const PRICES: &str = "activity-prices.json";

/// Where an agent works and the transcript its CLI writes, as the core knows
/// them when a turn begins or ends.
pub(crate) struct Place {
    pub agent: String,
    pub cwd: String,
    pub transcript: Option<Transcript>,
    /// The chat, and the account it runs on, that the tokens counted here are charged to.
    pub chat: String,
    pub account: String,
}

pub(crate) struct Transcript {
    pub kind: AgentKind,
    pub session: String,
    pub path: PathBuf,
}

/// Where the agent `agent_id` works and the transcript its CLI writes, for a
/// chat or an agent in a terminal.
pub(crate) fn place(core: &Core, agent_id: &str) -> Option<Place> {
    if let Some(chat) = core.chats.get(agent_id) {
        let launch = chat.current_launch();
        let home = named_account(&launch.provider, &launch.env).map(PathBuf::from);
        let session = chat.feed.start().map(|start| start.session_id);
        return Some(Place {
            transcript: transcript_of(&launch.provider, session, home.as_deref()),
            cwd: launch.cwd.to_string_lossy().into_owned(),
            agent: launch.provider,
            chat: agent_id.to_owned(),
            account: launch.account.map(|account| account.id).unwrap_or_default(),
        });
    }
    let session = core.agent_session(agent_id)?;
    let owner = session.owner();
    let agent = owner.agent_type.clone()?;
    Some(Place {
        transcript: transcript_of(
            &agent,
            core.names.session_of(agent_id),
            owner.account_home.as_deref(),
        ),
        cwd: owner.project.clone()?,
        agent,
        chat: agent_id.to_owned(),
        account: (core.workspaces.agent(agent_id))
            .and_then(|agent| agent.profile_id)
            .unwrap_or_default(),
    })
}

pub(crate) fn transcript_of(
    agent: &str,
    session: Option<String>,
    home: Option<&Path>,
) -> Option<Transcript> {
    let session = session?;
    let kind = AgentKind::from_name(agent)?;
    let path = transcript::locate(kind, home, &session)?;
    Some(Transcript {
        kind,
        session,
        path,
    })
}

pub(crate) fn chat_launched(core: &Core, launch: &sikemux_wire::protocol::ChatLaunch) {
    let home = named_account(&launch.provider, &launch.env).map(PathBuf::from);
    let place = Place {
        agent: launch.provider.clone(),
        cwd: launch.cwd.to_string_lossy().into_owned(),
        transcript: transcript_of(&launch.provider, launch.resume_id.clone(), home.as_deref()),
        chat: launch.agent_id.clone(),
        account: (launch.account.as_ref())
            .map(|account| account.id.clone())
            .unwrap_or_default(),
    };
    core.activity_log
        .launched(place, "chat", launch.resume_id.is_some());
}

/// For an agent a terminal starts, the session its command resumes, if any.
pub(crate) fn resumed_by(spawn: &sikemux_wire::protocol::TerminalSpawn) -> Option<Option<String>> {
    let command = spawn.direct_command.as_ref()?;
    let agent = spawn.context.as_ref()?.agent_type.as_deref()?;
    Some(crate::launch::terminal::resumed_session(agent, &command.args).map(str::to_owned))
}

pub(crate) fn terminal_launched(
    core: &Core,
    owner: &super::prepare::Owner,
    resumed: Option<String>,
) {
    let (Some(agent), Some(project)) = (&owner.agent_type, &owner.project) else {
        return;
    };
    let place = Place {
        agent: agent.clone(),
        cwd: project.clone(),
        transcript: transcript_of(agent, resumed.clone(), owner.account_home.as_deref()),
        chat: owner.agent_id.clone().unwrap_or_default(),
        account: String::new(),
    };
    core.activity_log
        .launched(place, "terminal", resumed.is_some());
}

/// Work on the database and git, off the threads that serve clients.
fn in_background(work: impl FnOnce() + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn_blocking(work);
    }
}

struct OpenTurn {
    started_ms: i64,
    head: Option<String>,
}

pub(crate) struct ActivityLog {
    path: Option<PathBuf>,
    prices_path: Option<PathBuf>,
    database: Mutex<Option<Connection>>,
    turns: Mutex<HashMap<String, OpenTurn>>,
    uses: Mutex<features::Uses>,
    feed: Option<Feed>,
}

impl ActivityLog {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|dir| dir.join(DATABASE));
        Self {
            path,
            prices_path: data_dir.map(|dir| dir.join(PRICES)),
            database: Mutex::new(None),
            turns: Mutex::new(HashMap::new()),
            uses: Mutex::default(),
            feed: data_dir.map(Feed::new),
        }
    }

    /// Runs `work` against the database. Activity is a record, never a
    /// reason for the action it describes to fail, so errors are dropped here.
    fn with_database<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Option<T> {
        let path = self.path.as_ref()?;
        let mut cache = self.database.locked();
        if cache.is_none() {
            *cache = Some(open(path).ok()?);
        }
        let result = work(cache.as_ref()?);
        if result.is_err() {
            *cache = None;
        }
        result.ok()
    }

    /// An agent opened a session, `resumed` when it loaded one. A loaded
    /// session's transcript already holds turns from before it came here, so
    /// only what is written from now on is counted.
    pub(crate) fn launched(self: &Arc<Self>, place: Place, transport: &'static str, resumed: bool) {
        let log = self.clone();
        let at_ms = epoch_ms() as i64;
        in_background(move || {
            log.with_database(|db| {
                db.execute(
                    "INSERT INTO launches (at_ms, agent, project, transport, resumed) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![at_ms, place.agent, place.cwd, transport, resumed],
                )
            });
            let Some(transcript) = place.transcript.as_ref().filter(|_| resumed) else {
                return;
            };
            let Ok(len) = fs::metadata(&transcript.path).map(|meta| meta.len()) else {
                return;
            };
            let key = transcript_key(transcript.kind, &transcript.session);
            log.with_database(|db| {
                db.execute(
                    "INSERT INTO transcripts (session, path, offset, mark) VALUES (?1, ?2, ?3, '')
                     ON CONFLICT(session) DO UPDATE SET path = excluded.path, offset = excluded.offset, mark = '',
                         model = '', speed = 'standard'",
                    params![key, transcript.path.to_string_lossy(), len as i64],
                )
            });
        });
    }

    /// The agent began a turn; the commit it began on says which commits it made.
    pub(crate) fn turn_started(self: &Arc<Self>, agent_id: &str, cwd: String) {
        let started_ms = epoch_ms() as i64;
        {
            let mut turns = self.turns.locked();
            if turns.contains_key(agent_id) {
                return;
            }
            turns.insert(
                agent_id.to_owned(),
                OpenTurn {
                    started_ms,
                    head: None,
                },
            );
        }
        let log = self.clone();
        let agent_id = agent_id.to_owned();
        in_background(move || {
            let head = crate::git::head(&cwd);
            if let Some(turn) = log
                .turns
                .locked()
                .get_mut(&agent_id)
                .filter(|turn| turn.started_ms == started_ms)
            {
                turn.head = head;
            }
        });
    }

    pub(crate) fn turn_ended(self: &Arc<Self>, agent_id: &str, place: Place) {
        let Some(turn) = self.turns.locked().remove(agent_id) else {
            return;
        };
        let ended_ms = epoch_ms() as i64;
        let log = self.clone();
        in_background(move || {
            log.with_database(|db| {
                db.execute(
                    "INSERT INTO turns (started_ms, ended_ms, agent, project) VALUES (?1, ?2, ?3, ?4)",
                    params![turn.started_ms, ended_ms, place.agent, place.cwd],
                )
            });
            if let Some(start) = turn.head.as_deref() {
                log.record_commits_since(&place.cwd, start, Some(&place.agent));
            }
            log.record_new_tokens(&place, ended_ms);
        });
    }

    fn record_commits_since(&self, repo: &str, start: &str, agent: Option<&str>) {
        let range = format!("{start}..HEAD");
        let Some(log) = git(repo, &["log", "--format=%H %ct", &range]) else {
            return;
        };
        let commits: Vec<(&str, i64)> = log
            .lines()
            .take(MAX_TURN_COMMITS)
            .filter_map(|line| {
                let (sha, seconds) = line.split_once(' ')?;
                Some((sha, seconds.trim().parse::<i64>().ok()? * 1000))
            })
            .collect();
        self.insert_commits(repo, &commits, agent);
    }

    fn insert_commits(&self, repo: &str, commits: &[(&str, i64)], agent: Option<&str>) {
        if commits.is_empty() {
            return;
        }
        self.with_database(|db| {
            let mut insert = db.prepare_cached(
                "INSERT OR IGNORE INTO commits (sha, at_ms, project, agent) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (sha, at_ms) in commits {
                insert.execute(params![sha, at_ms, repo, agent])?;
            }
            Ok(())
        });
    }

    /// A commit the person made in Sikemux.
    pub(crate) fn record_commit(&self, repo: &str) {
        if let Some(sha) = crate::git::head(repo) {
            self.insert_commits(repo, &[(&sha, epoch_ms() as i64)], None);
        }
    }

    fn price_overrides(&self) -> HashMap<String, PriceOverride> {
        self.prices_path
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Prices `model` at what the person entered, or at the published price
    /// again with none.
    pub(crate) fn set_price(
        &self,
        model: &str,
        price: Option<PriceOverride>,
    ) -> Result<(), String> {
        let path = self
            .prices_path
            .as_ref()
            .ok_or("Sikemux keeps no prices here")?;
        let mut prices = self.price_overrides();
        match price {
            Some(price) => prices.insert(model.to_owned(), price),
            None => prices.remove(model),
        };
        let bytes = serde_json::to_vec(&prices).map_err(|error| error.to_string())?;
        sikemux_process::files::write_atomic(path, &bytes, false).map_err(|error| error.to_string())
    }

    /// `offset_ms` places each event on the person's own calendar day.
    pub(crate) fn summary(&self, offset_ms: i64, project: Option<&str>) -> ActivitySummary {
        let overrides = self.price_overrides();
        let feed = self.feed.as_ref().map(Feed::current).unwrap_or_default();
        let prices = Prices::new(&feed, &overrides);
        let mut summary = self
            .with_database(|db| {
                summarize(
                    db,
                    offset_ms.clamp(-DAY_MS, DAY_MS),
                    epoch_ms() as i64,
                    project,
                    &prices,
                )
            })
            .unwrap_or_default();
        summary.pricing.overrides = overrides;
        summary
    }
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(3))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS launches (
             at_ms INTEGER NOT NULL,
             agent TEXT NOT NULL,
             project TEXT NOT NULL,
             transport TEXT NOT NULL,
             resumed INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS turns (
             started_ms INTEGER NOT NULL,
             ended_ms INTEGER NOT NULL,
             agent TEXT NOT NULL,
             project TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS commits (
             sha TEXT PRIMARY KEY,
             at_ms INTEGER NOT NULL,
             project TEXT NOT NULL,
             agent TEXT
         );
         CREATE TABLE IF NOT EXISTS tokens (
             at_ms INTEGER NOT NULL,
             agent TEXT NOT NULL,
             project TEXT NOT NULL,
             input INTEGER NOT NULL,
             output INTEGER NOT NULL,
             cache_read INTEGER NOT NULL,
             cache_write INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS transcripts (
             session TEXT PRIMARY KEY,
             path TEXT NOT NULL,
             offset INTEGER NOT NULL,
             mark TEXT NOT NULL
         );",
    )?;
    migrate(&connection)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(connection)
}

/// Each step runs once, in order, and `user_version` records how many have run.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE tokens ADD COLUMN model TEXT NOT NULL DEFAULT '';
     ALTER TABLE tokens ADD COLUMN speed TEXT NOT NULL DEFAULT 'standard';
     ALTER TABLE transcripts ADD COLUMN model TEXT NOT NULL DEFAULT '';
     ALTER TABLE transcripts ADD COLUMN speed TEXT NOT NULL DEFAULT 'standard';",
    "CREATE TABLE features (
         day INTEGER NOT NULL,
         feature TEXT NOT NULL,
         via TEXT NOT NULL,
         count INTEGER NOT NULL,
         PRIMARY KEY (day, feature, via)
     );",
    "ALTER TABLE tokens ADD COLUMN account TEXT NOT NULL DEFAULT '';
     ALTER TABLE tokens ADD COLUMN chat TEXT NOT NULL DEFAULT '';
     CREATE INDEX tokens_by_account ON tokens (account, at_ms);
     CREATE INDEX tokens_by_project ON tokens (project, at_ms);",
];

fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    let done: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (index, step) in MIGRATIONS.iter().enumerate().skip(done.max(0) as usize) {
        connection.execute_batch(&format!(
            "BEGIN; {step} PRAGMA user_version = {}; COMMIT;",
            index + 1
        ))?;
    }
    Ok(())
}

fn git(repo: &str, args: &[&str]) -> Option<String> {
    crate::git::Git::at(repo)
        .timeout(GIT_TIMEOUT)
        .limit(1024 * 1024)
        .text(args)
        .ok()
}

// ---- tokens ------------------------------------------------------------

fn transcript_key(kind: AgentKind, session: &str) -> String {
    format!("{}:{session}", kind.as_str())
}

impl ActivityLog {
    fn record_new_tokens(&self, place: &Place, at_ms: i64) {
        let Some(transcript) = &place.transcript else {
            return;
        };
        let key = transcript_key(transcript.kind, &transcript.session);
        let known = self
            .with_database(|db| {
                db.query_row(
                    "SELECT path, offset, mark, model, speed FROM transcripts WHERE session = ?1",
                    params![key],
                    |row| {
                        Ok((
                            PathBuf::from(row.get::<_, String>(0)?),
                            row.get::<_, i64>(1)? as u64,
                            ReadState {
                                mark: row.get(2)?,
                                model: row.get(3)?,
                                speed: row.get(4)?,
                            },
                        ))
                    },
                )
                .optional()
            })
            .flatten()
            .filter(|(path, _, _)| path.is_file());
        let (path, offset, state) =
            known.unwrap_or_else(|| (transcript.path.clone(), 0, ReadState::default()));
        let Some((text, next_offset)) = read_complete_lines(&path, offset) else {
            return;
        };
        let Some(count) = transcript.kind.ops().transcript_tokens() else {
            return;
        };
        let (tally, next) = count(&text, state);
        self.with_database(|db| {
            db.execute(
                "INSERT INTO transcripts (session, path, offset, mark, model, speed) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(session) DO UPDATE SET path = excluded.path, offset = excluded.offset,
                     mark = excluded.mark, model = excluded.model, speed = excluded.speed",
                params![
                    key,
                    path.to_string_lossy(),
                    next_offset as i64,
                    next.mark,
                    next.model,
                    next.speed
                ],
            )?;
            for entry in tally.0.iter().filter(|entry| !entry.count.is_empty()) {
                db.execute(
                    "INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write, model, speed, account, chat)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        at_ms,
                        transcript.kind.as_str(),
                        place.cwd,
                        entry.count.input as i64,
                        entry.count.output as i64,
                        entry.count.cache_read as i64,
                        entry.count.cache_write as i64,
                        entry.model,
                        entry.speed,
                        place.account,
                        place.chat
                    ],
                )?;
            }
            Ok(())
        });
    }
}

/// Some agents report a turn's tokens only in the transcript their CLI writes.
/// Every other chat agent says them in its answer to the prompt.
fn counts_from_transcript(kind: AgentKind) -> bool {
    kind.ops().transcript_tokens().is_some()
}

impl ActivityLog {
    /// Counts the tokens a chat's agent said its turn used, in the ACP shape
    /// every chat driver answers a prompt with.
    pub(crate) fn turn_answered(
        self: &Arc<Self>,
        launch: &sikemux_wire::protocol::ChatLaunch,
        answer: &Value,
    ) {
        let kind = AgentKind::from_name(&launch.provider);
        if kind.is_some_and(counts_from_transcript) {
            return;
        }
        let Some(usage) = answer.get("usage").filter(|usage| usage.is_object()) else {
            return;
        };
        let field = |key: &str| usage[key].as_u64().unwrap_or(0);
        let count = TokenCount {
            input: field("inputTokens"),
            output: field("outputTokens"),
            cache_read: field("cachedReadTokens"),
            cache_write: field("cachedWriteTokens"),
        };
        if count.is_empty() {
            return;
        }
        let agent = kind.map_or_else(|| launch.provider.clone(), |kind| kind.as_str().to_owned());
        let project = launch.cwd.to_string_lossy().into_owned();
        let model = launch.model.clone().unwrap_or_default();
        let account = (launch.account.as_ref())
            .map(|account| account.id.clone())
            .unwrap_or_default();
        let chat = launch.agent_id.clone();
        let at_ms = epoch_ms() as i64;
        let log = self.clone();
        in_background(move || {
            log.with_database(|db| {
                db.execute(
                    "INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write, model, speed, account, chat)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'standard', ?9, ?10)",
                    params![
                        at_ms,
                        agent,
                        project,
                        count.input as i64,
                        count.output as i64,
                        count.cache_read as i64,
                        count.cache_write as i64,
                        model,
                        account,
                        chat
                    ],
                )
            });
        });
    }

    /// The tokens chats on `account` spent between `from_ms` and `to_ms`, cached reads aside
    /// as plans count them for little. Blocks on the database.
    pub(crate) fn tokens_on(&self, account: &str, from_ms: u64, to_ms: u64) -> u64 {
        self.with_database(|db| {
            db.query_row(
                "SELECT COALESCE(SUM(input + output + cache_write), 0) FROM tokens
                 WHERE account = ?1 AND at_ms > ?2 AND at_ms <= ?3",
                params![account, from_ms as i64, to_ms as i64],
                |row| row.get::<_, i64>(0),
            )
        })
        .map_or(0, |tokens| tokens.max(0) as u64)
    }

    /// The tokens a typical chat in the project at `cwd` spent over the last month: the middle
    /// of its chats', once enough chats have run there to say. Blocks on the database.
    pub(crate) fn typical_chat_tokens(&self, cwd: &str) -> Option<u64> {
        const ENOUGH_CHATS: usize = 5;
        const MONTH_MS: u64 = 30 * 24 * 60 * 60 * 1000;
        let since = epoch_ms().saturating_sub(MONTH_MS) as i64;
        let mut chats = self.with_database(|db| {
            let mut query = db.prepare_cached(
                "SELECT SUM(input + output + cache_write) FROM tokens
                 WHERE project = ?1 AND chat != '' AND at_ms > ?2 GROUP BY chat",
            )?;
            let rows = query.query_map(params![cwd, since], |row| row.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()
        })?;
        if chats.len() < ENOUGH_CHATS {
            return None;
        }
        chats.sort_unstable();
        chats
            .get(chats.len() / 2)
            .map(|tokens| (*tokens).max(0) as u64)
    }
}

/// Everything after `offset` up to the last full line, and where that ends.
/// A file that shrank was rewritten, so reading restarts past its new end.
fn read_complete_lines(path: &Path, offset: u64) -> Option<(String, u64)> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < offset {
        return Some((String::new(), len));
    }
    let start = offset.max(len.saturating_sub(MAX_TRANSCRIPT_READ));
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.take(len - start).read_to_end(&mut bytes).ok()?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    bytes.truncate(complete);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some((text, start + complete as u64))
}

// ---- summary -----------------------------------------------------------

#[derive(Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTotals {
    pub sessions: i64,
    pub resumed: i64,
    pub turns: i64,
    pub agent_ms: i64,
    pub commits: i64,
    pub agent_commits: i64,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub first_at_ms: Option<i64>,
    /// What the tokens would cost at API prices. Subscription plans bill separately.
    pub cost_usd: f64,
    /// What reading from the cache saved against sending the same input fresh.
    pub cache_savings_usd: f64,
    /// Tokens with no price: no model was recorded, or no price is known for it.
    pub unpriced_tokens: i64,
}

#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDay {
    pub day: i64,
    pub sessions: i64,
    pub agent_ms: i64,
    pub commits: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    /// How full the calendar draws the day for each measure.
    pub levels: ActivityLevels,
}

/// From 0 for nothing to 4, by which quarter of the days with anything the
/// day falls in, so one huge day does not wash out the rest.
#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLevels {
    pub sessions: u8,
    pub agent_ms: u8,
    pub commits: u8,
    pub tokens: u8,
    pub cost_usd: u8,
}

/// Days in a row with any activity. A streak that reached yesterday still
/// counts as current until today is over.
#[derive(Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStreaks {
    pub active_days: usize,
    pub longest: usize,
    pub current: usize,
}

#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityShare {
    pub name: String,
    pub sessions: i64,
    pub agent_ms: i64,
    pub commits: i64,
    pub tokens: i64,
    pub cost_usd: f64,
}

#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityModel {
    pub model: String,
    pub agent: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost_usd: f64,
    pub fast_cost_usd: f64,
    pub priced: bool,
}

#[derive(Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySummary {
    pub totals: ActivityTotals,
    /// Local calendar days, counted from the Unix epoch, with anything on them.
    pub days: Vec<ActivityDay>,
    pub streaks: ActivityStreaks,
    pub agents: Vec<ActivityShare>,
    pub projects: Vec<ActivityShare>,
    /// Models by cost, then by tokens.
    pub models: Vec<ActivityModel>,
    pub pricing: PricingInfo,
}

/// A query yielding `(at_ms, agent, project, amount)` rows, and where each amount adds up.
type Tally = (
    &'static str,
    fn(&mut ActivityDay, i64),
    fn(&mut ActivityShare, i64),
);

fn summarize(
    db: &Connection,
    offset_ms: i64,
    now_ms: i64,
    project: Option<&str>,
    prices: &Prices,
) -> rusqlite::Result<ActivitySummary> {
    let mut totals = db.query_row(
        "SELECT COUNT(*), COALESCE(SUM(resumed), 0), MIN(at_ms) FROM launches
         WHERE ?1 IS NULL OR project = ?1",
        [project],
        |row| {
            Ok(ActivityTotals {
                sessions: row.get(0)?,
                resumed: row.get(1)?,
                first_at_ms: row.get(2)?,
                ..ActivityTotals::default()
            })
        },
    )?;
    (totals.turns, totals.agent_ms) = db.query_row(
        "SELECT COUNT(*), COALESCE(SUM(ended_ms - started_ms), 0) FROM turns
         WHERE ?1 IS NULL OR project = ?1",
        [project],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    (totals.commits, totals.agent_commits) = db.query_row(
        "SELECT COUNT(*), COUNT(agent) FROM commits WHERE ?1 IS NULL OR project = ?1",
        [project],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    (
        totals.input,
        totals.output,
        totals.cache_read,
        totals.cache_write,
    ) = db.query_row(
        "SELECT COALESCE(SUM(input), 0), COALESCE(SUM(output), 0),
                COALESCE(SUM(cache_read), 0), COALESCE(SUM(cache_write), 0) FROM tokens
         WHERE ?1 IS NULL OR project = ?1",
        [project],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;

    let first_day = (now_ms + offset_ms).div_euclid(DAY_MS) - HEATMAP_DAYS;
    let mut days: HashMap<i64, ActivityDay> = HashMap::new();
    let mut agents: HashMap<String, ActivityShare> = HashMap::new();
    let mut projects: HashMap<String, ActivityShare> = HashMap::new();
    let queries: [Tally; 4] = [
        (
            "SELECT at_ms, agent, project, 1 FROM launches WHERE ?1 IS NULL OR project = ?1",
            |day, n| day.sessions += n,
            |share, n| share.sessions += n,
        ),
        (
            "SELECT started_ms, agent, project, ended_ms - started_ms FROM turns
             WHERE ?1 IS NULL OR project = ?1",
            |day, n| day.agent_ms += n,
            |share, n| share.agent_ms += n,
        ),
        (
            "SELECT at_ms, agent, project, 1 FROM commits WHERE ?1 IS NULL OR project = ?1",
            |day, n| day.commits += n,
            |share, n| share.commits += n,
        ),
        (
            "SELECT at_ms, agent, project, input + output + cache_write FROM tokens
             WHERE ?1 IS NULL OR project = ?1",
            |day, n| day.tokens += n,
            |share, n| share.tokens += n,
        ),
    ];
    for (sql, add_day, add_share) in queries {
        let mut statement = db.prepare(sql)?;
        let mut rows = statement.query([project])?;
        while let Some(row) = rows.next()? {
            let at_ms: i64 = row.get(0)?;
            let agent: Option<String> = row.get(1)?;
            let project: String = row.get(2)?;
            let amount: i64 = row.get(3)?;
            let day = (at_ms + offset_ms).div_euclid(DAY_MS);
            if day > first_day {
                add_day(
                    days.entry(day).or_insert_with(|| ActivityDay {
                        day,
                        ..ActivityDay::default()
                    }),
                    amount,
                );
            }
            if let Some(agent) = agent {
                add_share(
                    agents
                        .entry(agent.clone())
                        .or_insert_with(|| ActivityShare {
                            name: agent,
                            ..ActivityShare::default()
                        }),
                    amount,
                );
            }
            add_share(
                projects
                    .entry(project.clone())
                    .or_insert_with(|| ActivityShare {
                        name: project,
                        ..ActivityShare::default()
                    }),
                amount,
            );
        }
    }
    let mut models: HashMap<String, ActivityModel> = HashMap::new();
    let mut statement = db.prepare(
        "SELECT at_ms, agent, project, model, speed, input, output, cache_read, cache_write FROM tokens
         WHERE ?1 IS NULL OR project = ?1",
    )?;
    let mut rows = statement.query([project])?;
    while let Some(row) = rows.next()? {
        let at_ms: i64 = row.get(0)?;
        let agent: String = row.get(1)?;
        let project: String = row.get(2)?;
        let model: String = row.get(3)?;
        let speed: String = row.get(4)?;
        let [input, output, cache_read, cache_write]: [i64; 4] =
            [row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?];
        let tokens = input + output + cache_write;
        let rate = prices.lookup(&model);
        if model.is_empty() {
            totals.unpriced_tokens += tokens;
            continue;
        }
        let entry = models
            .entry(model.clone())
            .or_insert_with(|| ActivityModel {
                model,
                agent: agent.clone(),
                ..ActivityModel::default()
            });
        entry.input += input;
        entry.output += output;
        entry.cache_read += cache_read;
        entry.cache_write += cache_write;
        let Some(rate) = rate else {
            totals.unpriced_tokens += tokens;
            continue;
        };
        entry.priced = true;
        let rates = rate.at(&speed);
        let cost = input as f64 * rates.input
            + output as f64 * rates.output
            + cache_read as f64 * rates.cache_read
            + cache_write as f64 * rates.cache_write;
        entry.cost_usd += cost;
        if speed != "standard" {
            entry.fast_cost_usd += cost;
        }
        totals.cost_usd += cost;
        totals.cache_savings_usd += cache_read as f64 * (rates.input - rates.cache_read).max(0.0);
        let day = (at_ms + offset_ms).div_euclid(DAY_MS);
        if day > first_day {
            days.entry(day)
                .or_insert_with(|| ActivityDay {
                    day,
                    ..ActivityDay::default()
                })
                .cost_usd += cost;
        }
        if let Some(share) = agents.get_mut(&agent) {
            share.cost_usd += cost;
        }
        if let Some(share) = projects.get_mut(&project) {
            share.cost_usd += cost;
        }
    }
    drop(rows);
    drop(statement);
    let mut models: Vec<ActivityModel> = models.into_values().collect();
    models.sort_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then_with(|| {
                (b.input + b.output + b.cache_write).cmp(&(a.input + a.output + a.cache_write))
            })
            .then_with(|| a.model.cmp(&b.model))
    });

    let mut days: Vec<ActivityDay> = days.into_values().collect();
    days.sort_by_key(|day| day.day);
    level(
        &mut days,
        |day| day.sessions as f64,
        |levels| &mut levels.sessions,
    );
    level(
        &mut days,
        |day| day.agent_ms as f64,
        |levels| &mut levels.agent_ms,
    );
    level(
        &mut days,
        |day| day.commits as f64,
        |levels| &mut levels.commits,
    );
    level(
        &mut days,
        |day| day.tokens as f64,
        |levels| &mut levels.tokens,
    );
    level(&mut days, |day| day.cost_usd, |levels| &mut levels.cost_usd);
    let streaks = streaks(&days, (now_ms + offset_ms).div_euclid(DAY_MS));
    let ranked = |shares: HashMap<String, ActivityShare>| {
        let mut shares: Vec<ActivityShare> = shares.into_values().collect();
        shares.sort_by(|a, b| {
            (b.agent_ms, b.sessions, b.commits)
                .cmp(&(a.agent_ms, a.sessions, a.commits))
                .then_with(|| a.name.cmp(&b.name))
        });
        shares
    };
    Ok(ActivitySummary {
        totals,
        days,
        streaks,
        agents: ranked(agents),
        projects: ranked(projects),
        models,
        pricing: prices.info.clone(),
    })
}

fn level(
    days: &mut [ActivityDay],
    value: impl Fn(&ActivityDay) -> f64,
    slot: impl Fn(&mut ActivityLevels) -> &mut u8,
) {
    let mut sorted: Vec<f64> = days
        .iter()
        .map(&value)
        .filter(|value| *value > 0.)
        .collect();
    sorted.sort_by(f64::total_cmp);
    let quartiles: Vec<f64> = [0.25, 0.5, 0.75]
        .iter()
        .filter_map(|quantile| {
            let at = ((quantile * sorted.len() as f64) as usize).min(sorted.len().checked_sub(1)?);
            Some(sorted[at])
        })
        .collect();
    for day in days {
        let value = value(day);
        let above = quartiles
            .iter()
            .filter(|quartile| value > **quartile)
            .count();
        *slot(&mut day.levels) = if value > 0. { 1 + above as u8 } else { 0 };
    }
}

fn streaks(days: &[ActivityDay], today: i64) -> ActivityStreaks {
    let active: Vec<i64> = days
        .iter()
        .filter(|day| day.sessions + day.agent_ms + day.commits + day.tokens > 0)
        .map(|day| day.day)
        .collect();
    let (mut longest, mut run) = (0, 0);
    for (index, day) in active.iter().enumerate() {
        run = if index > 0 && active[index - 1] == day - 1 {
            run + 1
        } else {
            1
        };
        longest = longest.max(run);
    }
    let last = if active.last() == Some(&today) {
        today
    } else {
        today - 1
    };
    let current = active
        .iter()
        .rev()
        .zip((0..).map(|back| last - back))
        .take_while(|(day, expected)| **day == *expected)
        .count();
    ActivityStreaks {
        active_days: active.len(),
        longest,
        current,
    }
}

#[derive(Serialize, Deserialize)]
pub struct CommandUsed {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySummaryParams {
    /// Minutes the person's clock is ahead of UTC, which places each event on their own day.
    pub utc_offset_minutes: i64,
    pub project: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct ActivityPriceParams {
    pub model: String,
    pub price: Option<PriceOverride>,
}

pub(crate) async fn summary_for(
    core: &Arc<Core>,
    params: ActivitySummaryParams,
) -> CoreResult<ActivitySummary> {
    let core = core.clone();
    blocking(move || {
        Ok(core.activity_log.summary(
            params.utc_offset_minutes * 60_000,
            params.project.as_deref(),
        ))
    })
    .await
}

pub(crate) fn set_price_for(core: &Core, params: ActivityPriceParams) -> CoreResult<()> {
    core.activity_log.set_price(&params.model, params.price)?;
    core.changed(What::Read(Topic::Activity));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_lines_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        fs::write(&path, "one\ntwo\nthr").unwrap();
        let (text, next) = read_complete_lines(&path, 4).unwrap();
        assert_eq!(text, "two\n");
        assert_eq!(next, 8);
    }

    #[test]
    fn a_resumed_session_is_found_in_its_launch_arguments() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            crate::launch::terminal::resumed_session(
                "claude",
                &args(&["--model", "opus", "--resume", "abc"])
            ),
            Some("abc")
        );
        assert_eq!(
            crate::launch::terminal::resumed_session(
                "codex",
                &args(&["resume", "--sandbox", "x", "abc"])
            ),
            Some("abc")
        );
        assert_eq!(
            crate::launch::terminal::resumed_session("pi", &args(&["--session", "s1"])),
            Some("s1")
        );
        assert_eq!(
            crate::launch::terminal::resumed_session("claude", &args(&["--model", "opus"])),
            None
        );
    }

    #[test]
    fn the_summary_buckets_events_on_local_days_and_ranks_by_agent_time() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("a.sqlite3")).unwrap();
        let day = 20_000 * DAY_MS;
        db.execute_batch(&format!(
            "INSERT INTO launches VALUES ({day}, 'claude', '/p/a', 'chat', 0);
             INSERT INTO launches VALUES ({day}, 'codex', '/p/b', 'terminal', 1);
             INSERT INTO turns VALUES ({day}, {}, 'codex', '/p/b');
             INSERT INTO commits VALUES ('s1', {day}, '/p/b', 'codex');
             INSERT INTO commits VALUES ('s2', {day}, '/p/b', NULL);
             INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write)
                 VALUES ({day}, 'claude', '/p/a', 1, 2, 50, 3);",
            day + 60_000
        ))
        .unwrap();
        let prices = Prices::new(&Default::default(), &HashMap::new());
        let summary = summarize(&db, -60_000, day + DAY_MS, None, &prices).unwrap();
        assert_eq!(summary.totals.sessions, 2);
        assert_eq!(summary.totals.resumed, 1);
        assert_eq!(summary.totals.agent_ms, 60_000);
        assert_eq!(summary.totals.commits, 2);
        assert_eq!(summary.totals.agent_commits, 1);
        assert_eq!(summary.days.len(), 1);
        assert_eq!(summary.days[0].day, 19_999);
        assert_eq!(summary.days[0].tokens, 6);
        assert_eq!(summary.agents[0].name, "codex");
        assert_eq!(summary.projects[0].name, "/p/b");
        assert_eq!(summary.projects[0].commits, 2);

        let only_a = summarize(&db, -60_000, day + DAY_MS, Some("/p/a"), &prices).unwrap();
        assert_eq!(only_a.totals.sessions, 1);
        assert_eq!(only_a.totals.commits, 0);
        assert_eq!(only_a.days[0].tokens, 6);
        assert_eq!(only_a.days[0].commits, 0);
        assert_eq!(only_a.projects.len(), 1);
    }

    fn active(days: &[i64]) -> Vec<ActivityDay> {
        days.iter()
            .map(|&day| ActivityDay {
                day,
                sessions: 1,
                ..ActivityDay::default()
            })
            .collect()
    }

    #[test]
    fn days_are_graded_by_the_quartiles_of_active_days() {
        let mut days: Vec<ActivityDay> = [0, 1, 2, 3, 4, 100]
            .into_iter()
            .enumerate()
            .map(|(day, tokens)| ActivityDay {
                day: day as i64,
                tokens,
                ..ActivityDay::default()
            })
            .collect();
        level(
            &mut days,
            |day| day.tokens as f64,
            |levels| &mut levels.tokens,
        );
        let levels: Vec<u8> = days.iter().map(|day| day.levels.tokens).collect();
        assert_eq!(levels[0], 0);
        assert_eq!(levels[1], 1);
        assert_eq!(levels[5], 4);
    }

    #[test]
    fn a_streak_stays_alive_until_today_is_over() {
        let run = streaks(&active(&[90, 91, 92, 98, 99]), 100);
        assert_eq!((run.active_days, run.current, run.longest), (5, 2, 3));
        assert_eq!(streaks(&active(&[97, 98]), 100).current, 0);
    }
}

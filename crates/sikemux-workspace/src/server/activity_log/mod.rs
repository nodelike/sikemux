//! What agents did on this host, for the Activity page: the sessions they
//! opened, the turns they ran, the commits made and the tokens their
//! transcripts or answers report, kept in one database in the core's data folder and
//! priced from the model feed and the person's own prices.

mod context;
mod features;
mod pricing;
mod report;
mod waits;

pub use context::{Compaction, ModelCall};
pub use features::Counted;

use super::changed::Topic;
use super::connection::blocking;
use super::CoreResult;
use crate::server::activity_log::pricing::ModelRateExt;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::activity_log::CommandUsed;
pub use sikemux_calls::activity_log::{
    ActivityDay, ActivityLevels, ActivityModel, ActivityPriceParams, ActivityShare,
    ActivityStreaks, ActivitySummary, ActivitySummaryParams, ActivityTotals, TokenCosts,
};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::agent_catalog::AgentKind;
use crate::launch::model_feed::Feed;
use crate::launch::named_account;
pub use pricing::{PriceOverride, Prices, PricingInfo};
pub use report::{ReportedAgent, ReportedAnswers, ReportedDay, ReportedTokens};

use super::Core;
use crate::sessions::tokens::{ReadState, TokenCount};
use crate::sessions::transcript;

const DAY_MS: i64 = 86_400_000;
const HEATMAP_DAYS: i64 = 371;
const MAX_TRANSCRIPT_READ: u64 = 64 * 1024 * 1024;
const MAX_TURN_COMMITS: usize = 500;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const BUSY_TIMEOUT: Duration = Duration::from_secs(3);
const CHECKPOINT_EVERY_WRITES: usize = 256;
pub const DATABASE: &str = "activity.sqlite3";
const PRICES: &str = "activity-prices.json";

/// Where an agent works and the transcript its CLI writes, as the core knows
/// them when a turn begins or ends.
pub struct Place {
    pub agent: String,
    pub cwd: String,
    pub transcript: Option<Transcript>,
    /// The chat, and the account it runs on, that the tokens counted here are charged to.
    pub chat: String,
    pub account: String,
}

pub struct Transcript {
    pub kind: AgentKind,
    pub session: String,
    pub path: PathBuf,
}

/// Where the agent `agent_id` works and the transcript its CLI writes, for a
/// chat or an agent in a terminal.
pub fn place(core: &Core, agent_id: &str) -> Option<Place> {
    if let Some(place) = (core.workspace_above().chat_place)(core, agent_id) {
        return Some(place);
    }
    let session = core.agent_session(agent_id)?;
    let owner = session.owner();
    let agent = owner.agent_type.clone()?;
    Some(Place {
        transcript: transcript_of(
            &agent,
            core.names().session_of(agent_id),
            owner.account_home.as_deref(),
        ),
        cwd: owner.project.clone()?,
        agent,
        chat: agent_id.to_owned(),
        account: (core.workspaces().agent(agent_id))
            .and_then(|agent| agent.profile_id)
            .unwrap_or_default(),
    })
}

pub fn transcript_of(
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

pub fn chat_launched(core: &Core, launch: &sikemux_wire::protocol::ChatLaunch) {
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
    core.activity_log()
        .launched(place, "chat", launch.resume_id.is_some());
}

/// For an agent a terminal starts, the session its command resumes, if any.
pub fn resumed_by(spawn: &sikemux_wire::protocol::TerminalSpawn) -> Option<Option<String>> {
    let command = spawn.direct_command.as_ref()?;
    let agent = spawn.context.as_ref()?.agent_type.as_deref()?;
    Some(crate::launch::terminal::resumed_session(agent, &command.args).map(str::to_owned))
}

pub fn terminal_launched(core: &Core, owner: &super::prepare::Owner, resumed: Option<String>) {
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
    core.activity_log()
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

pub struct ActivityLog {
    path: Option<PathBuf>,
    prices_path: Option<PathBuf>,
    database: Mutex<Option<Connection>>,
    turns: Mutex<HashMap<String, OpenTurn>>,
    uses: Mutex<features::Uses>,
    feed: Option<Feed>,
    /// The last UTC day whose chats' model calls were added up.
    rolled_day: AtomicI64,
    writes_since_checkpoint: AtomicUsize,
}

impl ActivityLog {
    pub fn new(data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|dir| dir.join(DATABASE));
        Self {
            path,
            prices_path: data_dir.map(|dir| dir.join(PRICES)),
            database: Mutex::new(None),
            turns: Mutex::new(HashMap::new()),
            uses: Mutex::default(),
            feed: data_dir.map(Feed::new),
            rolled_day: AtomicI64::new(0),
            writes_since_checkpoint: AtomicUsize::new(0),
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

    fn with_database_write<T>(
        &self,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        let path = self.path.as_ref()?;
        let mut cache = self.database.locked();
        if cache.is_none() {
            *cache = Some(open(path).ok()?);
        }
        let result = work(cache.as_ref()?);
        match result {
            Ok(value) => {
                if self.writes_since_checkpoint.fetch_add(1, Ordering::Relaxed) + 1
                    >= CHECKPOINT_EVERY_WRITES
                {
                    self.writes_since_checkpoint.store(0, Ordering::Relaxed);
                    if let Some(db) = cache.as_ref() {
                        checkpoint_truncate(db);
                    }
                }
                Some(value)
            }
            Err(_) => {
                *cache = None;
                None
            }
        }
    }

    pub fn checkpoint(&self) {
        let cache = self.database.locked();
        if let Some(db) = cache.as_ref() {
            self.writes_since_checkpoint.store(0, Ordering::Relaxed);
            checkpoint_truncate(db);
        }
    }

    /// An agent opened a session, `resumed` when it loaded one. A loaded
    /// session's transcript already holds turns from before it came here, so
    /// only what is written from now on is counted.
    pub fn launched(self: &Arc<Self>, place: Place, transport: &'static str, resumed: bool) {
        let log = self.clone();
        let at_ms = epoch_ms() as i64;
        in_background(move || {
            log.record_launched(&place, transport, resumed, at_ms);
        });
    }

    fn record_launched(&self, place: &Place, transport: &'static str, resumed: bool, at_ms: i64) {
        self.with_database_write(|db| {
            db.execute(
                "INSERT INTO launches (at_ms, agent, project, transport, resumed) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![at_ms, place.agent, project_at(&place.cwd), transport, resumed],
            )
        });
        let Some(transcript) = place.transcript.as_ref().filter(|_| resumed) else {
            return;
        };
        let Ok(len) = fs::metadata(&transcript.path).map(|meta| meta.len()) else {
            return;
        };
        let key = transcript_key(transcript.kind, &transcript.session);
        let (model, speed) = match transcript
            .kind
            .ops()
            .transcript_state_before(&transcript.path, len)
        {
            Some(state) => (state.model, state.speed),
            None => (String::new(), String::new()),
        };
        self.with_database_write(|db| {
            db.execute(
                "INSERT INTO transcripts (session, path, offset, mark, model, speed)
                 VALUES (?1, ?2, ?3, '', ?4, CASE WHEN ?5 = '' THEN 'standard' ELSE ?5 END)
                 ON CONFLICT(session) DO UPDATE SET path = excluded.path, offset = excluded.offset, mark = '',
                     model = CASE WHEN excluded.model = '' THEN transcripts.model ELSE excluded.model END,
                     speed = CASE WHEN ?5 = '' THEN transcripts.speed ELSE excluded.speed END",
                params![key, transcript.path.to_string_lossy(), len as i64, model, speed],
            )
        });
    }

    /// The agent began a turn; the commit it began on says which commits it made.
    pub fn turn_started(self: &Arc<Self>, agent_id: &str, cwd: String) {
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

    pub fn turn_ended(self: &Arc<Self>, agent_id: &str, place: Place) {
        let Some(turn) = self.turns.locked().remove(agent_id) else {
            return;
        };
        let ended_ms = epoch_ms() as i64;
        let log = self.clone();
        in_background(move || {
            let project = project_at(&place.cwd);
            log.with_database_write(|db| {
                db.execute(
                    "INSERT INTO turns (started_ms, ended_ms, agent, project) VALUES (?1, ?2, ?3, ?4)",
                    params![turn.started_ms, ended_ms, place.agent, project],
                )
            });
            if let Some(start) = turn.head.as_deref() {
                log.record_commits_since(&place.cwd, &project, start, Some(&place.agent));
            }
            log.record_new_tokens(&place, &project, ended_ms);
        });
    }

    fn record_commits_since(&self, repo: &str, project: &str, start: &str, agent: Option<&str>) {
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
        self.insert_commits(project, &commits, agent);
    }

    fn insert_commits(&self, project: &str, commits: &[(&str, i64)], agent: Option<&str>) {
        if commits.is_empty() {
            return;
        }
        self.with_database_write(|db| {
            let mut insert = db.prepare_cached(
                "INSERT OR IGNORE INTO commits (sha, at_ms, project, agent) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (sha, at_ms) in commits {
                insert.execute(params![sha, at_ms, project, agent])?;
            }
            Ok(())
        });
    }

    /// A commit the person made in Sikemux.
    pub fn record_commit(&self, repo: &str) {
        if let Some(sha) = crate::git::head(repo) {
            self.insert_commits(&project_at(repo), &[(&sha, epoch_ms() as i64)], None);
        }
    }

    fn price_overrides(&self) -> HashMap<String, PriceOverride> {
        self.prices_path
            .as_ref()
            .and_then(|path| {
                sikemux_process::files::read_or_set_aside(path, |bytes| {
                    serde_json::from_slice(bytes)
                })
            })
            .unwrap_or_default()
    }

    /// What models cost: the prices the person entered over the published ones.
    pub fn prices(&self) -> Prices {
        let feed = self.feed.as_ref().map(Feed::current).unwrap_or_default();
        Prices::new(&feed, &self.price_overrides())
    }

    /// Prices `model` at what the person entered, or at the published price
    /// again with none.
    pub fn set_price(&self, model: &str, price: Option<PriceOverride>) -> Result<(), String> {
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
    pub fn summary(
        &self,
        offset_ms: i64,
        project: Option<&str>,
        since_ms: Option<i64>,
    ) -> ActivitySummary {
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
                    since_ms,
                    &prices,
                )
            })
            .unwrap_or_default();
        summary.pricing.overrides = overrides;
        summary
    }
}

fn checkpoint_truncate(db: &Connection) {
    let _ = db.busy_timeout(Duration::ZERO);
    let busy = db
        .query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(1);
    let _ = db.busy_timeout(BUSY_TIMEOUT);
    if busy != 0 {
        let _ = db.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
    }
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    let _ = sikemux_process::files::private_database(path);
    let connection = Connection::open(path)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA journal_size_limit = 16777216;
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
    checkpoint_truncate(&connection);
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
    "CREATE TABLE answers (
         day INTEGER NOT NULL,
         kind TEXT NOT NULL,
         via TEXT NOT NULL,
         count INTEGER NOT NULL,
         ms INTEGER NOT NULL,
         PRIMARY KEY (day, kind, via)
     );
     CREATE TABLE waited (day INTEGER PRIMARY KEY, ms INTEGER NOT NULL);",
    context::TABLES,
    "UPDATE launches SET project = substr(project, 1, instr(project, '.worktrees/') - 1) WHERE instr(project, '.worktrees/') > 0;
     UPDATE turns SET project = substr(project, 1, instr(project, '.worktrees/') - 1) WHERE instr(project, '.worktrees/') > 0;
     UPDATE commits SET project = substr(project, 1, instr(project, '.worktrees/') - 1) WHERE instr(project, '.worktrees/') > 0;
     UPDATE tokens SET project = substr(project, 1, instr(project, '.worktrees/') - 1) WHERE instr(project, '.worktrees/') > 0;",
    "UPDATE tokens SET model = substr(model, instr(model, '/') + 1)
     WHERE model GLOB 'sikemux-*/*' AND instr(model, '/') > 9 AND length(model) > instr(model, '/');",
    "CREATE INDEX IF NOT EXISTS launches_by_time ON launches (at_ms);
     CREATE INDEX IF NOT EXISTS launches_by_project ON launches (project, at_ms);
     CREATE INDEX IF NOT EXISTS turns_by_time ON turns (started_ms);
     CREATE INDEX IF NOT EXISTS turns_by_project ON turns (project, started_ms);
     CREATE INDEX IF NOT EXISTS commits_by_time ON commits (at_ms);
     CREATE INDEX IF NOT EXISTS commits_by_project ON commits (project, at_ms);
     CREATE INDEX IF NOT EXISTS tokens_by_time ON tokens (at_ms);",
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

/// The project work in `cwd` is counted under: a teammate's worktree counts as its main checkout.
fn project_at(cwd: &str) -> String {
    super::workspace::project_of(Path::new(cwd), &[])
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
    fn record_new_tokens(&self, place: &Place, project: &str, at_ms: i64) {
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
        let (path, offset, mut state) =
            known.unwrap_or_else(|| (transcript.path.clone(), 0, ReadState::default()));
        let Some((text, next_offset)) = read_complete_lines(&path, offset) else {
            return;
        };
        let Some(count) = transcript.kind.ops().transcript_tokens() else {
            return;
        };
        if !text.is_empty() && offset > 0 && state.model.is_empty() {
            if let Some(seeded) = transcript.kind.ops().transcript_state_before(&path, offset) {
                state.model = seeded.model;
                if state.speed.is_empty() || state.speed == "standard" {
                    state.speed = seeded.speed;
                }
            }
        }
        let (tally, next) = count(&text, state);
        self.with_database_write(|db| {
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
                        project,
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
    pub fn turn_answered(
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
        let cwd = launch.cwd.clone();
        let model = launch
            .model
            .as_deref()
            .map(crate::launch::providers::unrouted_model)
            .unwrap_or_default()
            .to_owned();
        let account = (launch.account.as_ref())
            .map(|account| account.id.clone())
            .unwrap_or_default();
        let chat = launch.agent_id.clone();
        let at_ms = epoch_ms() as i64;
        let log = self.clone();
        in_background(move || {
            let project = project_at(&cwd.to_string_lossy());
            log.with_database_write(|db| {
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

fn filter_clause(project: Option<&str>, since_ms: Option<i64>, time_col: &str) -> String {
    match (project, since_ms) {
        (Some(_), Some(_)) => format!(" WHERE project = ?1 AND {time_col} >= ?2"),
        (Some(_), None) => " WHERE project = ?1".to_owned(),
        (None, Some(_)) => format!(" WHERE {time_col} >= ?1"),
        (None, None) => String::new(),
    }
}

fn query_filtered(
    db: &Connection,
    select: &str,
    time_col: &str,
    project: Option<&str>,
    since_ms: Option<i64>,
    mut visit: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    let sql = format!("{select}{}", filter_clause(project, since_ms, time_col));
    let mut statement = db.prepare(&sql)?;
    let mut rows = match (project, since_ms) {
        (Some(project), Some(since)) => statement.query(params![project, since])?,
        (Some(project), None) => statement.query(params![project])?,
        (None, Some(since)) => statement.query(params![since])?,
        (None, None) => statement.query([])?,
    };
    while let Some(row) = rows.next()? {
        visit(row)?;
    }
    Ok(())
}

fn summarize(
    db: &Connection,
    offset_ms: i64,
    now_ms: i64,
    project: Option<&str>,
    since_ms: Option<i64>,
    prices: &Prices,
) -> rusqlite::Result<ActivitySummary> {
    let mut totals = ActivityTotals::default();
    let first_day = (now_ms + offset_ms).div_euclid(DAY_MS) - HEATMAP_DAYS;
    let mut days: HashMap<i64, ActivityDay> = HashMap::new();
    let mut agents: HashMap<String, ActivityShare> = HashMap::new();
    let mut projects: HashMap<String, ActivityShare> = HashMap::new();

    let bucket = |days: &mut HashMap<i64, ActivityDay>,
                  agents: &mut HashMap<String, ActivityShare>,
                  projects: &mut HashMap<String, ActivityShare>,
                  at_ms: i64,
                  agent: Option<&str>,
                  project: &str,
                  amount: i64,
                  add_day: fn(&mut ActivityDay, i64),
                  add_share: fn(&mut ActivityShare, i64)| {
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
                    .entry(agent.to_owned())
                    .or_insert_with(|| ActivityShare {
                        name: agent.to_owned(),
                        ..ActivityShare::default()
                    }),
                amount,
            );
        }
        add_share(
            projects
                .entry(project.to_owned())
                .or_insert_with(|| ActivityShare {
                    name: project.to_owned(),
                    ..ActivityShare::default()
                }),
            amount,
        );
    };

    query_filtered(
        db,
        "SELECT at_ms, agent, project, resumed FROM launches",
        "at_ms",
        project,
        since_ms,
        |row| {
            let at_ms: i64 = row.get(0)?;
            let agent: String = row.get(1)?;
            let project: String = row.get(2)?;
            let resumed: i64 = row.get(3)?;
            totals.sessions += 1;
            totals.resumed += resumed;
            totals.first_at_ms = Some(totals.first_at_ms.map_or(at_ms, |first| first.min(at_ms)));
            bucket(
                &mut days,
                &mut agents,
                &mut projects,
                at_ms,
                Some(&agent),
                &project,
                1,
                |day, n| day.sessions += n,
                |share, n| share.sessions += n,
            );
            Ok(())
        },
    )?;

    query_filtered(
        db,
        "SELECT started_ms, agent, project, ended_ms - started_ms FROM turns",
        "started_ms",
        project,
        since_ms,
        |row| {
            let started_ms: i64 = row.get(0)?;
            let agent: String = row.get(1)?;
            let project: String = row.get(2)?;
            let elapsed: i64 = row.get(3)?;
            totals.turns += 1;
            totals.agent_ms += elapsed;
            bucket(
                &mut days,
                &mut agents,
                &mut projects,
                started_ms,
                Some(&agent),
                &project,
                elapsed,
                |day, n| day.agent_ms += n,
                |share, n| share.agent_ms += n,
            );
            Ok(())
        },
    )?;

    query_filtered(
        db,
        "SELECT at_ms, agent, project FROM commits",
        "at_ms",
        project,
        since_ms,
        |row| {
            let at_ms: i64 = row.get(0)?;
            let agent: Option<String> = row.get(1)?;
            let project: String = row.get(2)?;
            totals.commits += 1;
            if agent.is_some() {
                totals.agent_commits += 1;
            }
            bucket(
                &mut days,
                &mut agents,
                &mut projects,
                at_ms,
                agent.as_deref(),
                &project,
                1,
                |day, n| day.commits += n,
                |share, n| share.commits += n,
            );
            Ok(())
        },
    )?;

    let mut models: HashMap<String, ActivityModel> = HashMap::new();
    query_filtered(
        db,
        "SELECT at_ms, agent, project, model, speed, input, output, cache_read, cache_write FROM tokens",
        "at_ms",
        project,
        since_ms,
        |row| {
            let at_ms: i64 = row.get(0)?;
            let agent: String = row.get(1)?;
            let project: String = row.get(2)?;
            let model: String = row.get(3)?;
            let speed: String = row.get(4)?;
            let [input, output, cache_read, cache_write]: [i64; 4] =
                [row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?];
            totals.input += input;
            totals.output += output;
            totals.cache_read += cache_read;
            totals.cache_write += cache_write;
            let tokens = input + output + cache_write;
            bucket(
                &mut days,
                &mut agents,
                &mut projects,
                at_ms,
                Some(&agent),
                &project,
                tokens,
                |day, n| day.tokens += n,
                |share, n| share.tokens += n,
            );
            let rate = prices.lookup(&model);
            if model.is_empty() {
                totals.unpriced_tokens += tokens;
                return Ok(());
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
                return Ok(());
            };
            entry.priced = true;
            let rates = rate.at(&speed);
            let costs = TokenCosts {
                input: input as f64 * rates.input,
                output: output as f64 * rates.output,
                cache_read: cache_read as f64 * rates.cache_read,
                cache_write: cache_write as f64 * rates.cache_write,
            };
            let cost = costs.total();
            entry.cost_usd += cost;
            entry.costs.add(&costs);
            totals.costs.add(&costs);
            if speed != "standard" {
                entry.fast_cost_usd += cost;
            }
            totals.cost_usd += cost;
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
            Ok(())
        },
    )?;
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
            sorted.get(at).copied()
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
        let before = index.checked_sub(1).and_then(|before| active.get(before));
        run = if before == Some(&(day - 1)) {
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

pub async fn summary_for(
    core: &Arc<Core>,
    params: ActivitySummaryParams,
) -> CoreResult<ActivitySummary> {
    let core = core.clone();
    blocking(move || {
        Ok(core.activity_log().summary(
            params.utc_offset_minutes * 60_000,
            params.project.as_deref(),
            params.since_ms,
        ))
    })
    .await
}

pub fn set_price_for(core: &Core, params: ActivityPriceParams) -> CoreResult<()> {
    core.activity_log().set_price(&params.model, params.price)?;
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
        let summary = summarize(&db, -60_000, day + DAY_MS, None, None, &prices).unwrap();
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

        let only_a = summarize(&db, -60_000, day + DAY_MS, Some("/p/a"), None, &prices).unwrap();
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

    #[test]
    fn migration_strips_sikemux_routing_prefix_from_existing_token_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(DATABASE);
        let db = open(&db_path).unwrap();
        let day = 20_000 * DAY_MS;
        db.execute_batch(&format!(
            "PRAGMA user_version = 6;
             INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write, model)
                 VALUES ({day}, 'omp', '/repo', 10, 5, 0, 0, 'sikemux-local-3425/workbuddy/hy4flash-free'),
                        ({day}, 'omp', '/repo', 20, 10, 0, 0, 'workbuddy/hy4flash-free'),
                        ({day}, 'omp', '/repo', 30, 15, 0, 0, 'anthropic/claude-sonnet-4');"
        ))
        .unwrap();
        drop(db);

        let log = ActivityLog::new(Some(dir.path()));
        let days = log.reported_days(20_000, 20_001);
        assert_eq!(days.len(), 1);
        let rows: Vec<(&str, i64, i64)> = days[0]
            .tokens
            .iter()
            .map(|entry| (entry.model.as_str(), entry.input, entry.output))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("workbuddy/hy4flash-free", 30, 15),
                ("anthropic/claude-sonnet-4", 30, 15),
            ]
        );
    }

    #[test]
    fn codex_resume_keeps_and_seeds_transcript_model_before_next_turn_context() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-codex.jsonl");
        let turn_context =
            r#"{"type":"turn_context","payload":{"model":"gpt-5.4","service_tier":"priority"}}"#;
        let token_count = |total: u64, input: u64, output: u64| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":{output}}}}}}}}}"#
            )
        };
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{turn_context}").unwrap();
        writeln!(file, "{}", token_count(100, 80, 20)).unwrap();
        file.flush().unwrap();

        let log = ActivityLog::new(Some(dir.path()));
        let place = Place {
            agent: "codex".into(),
            cwd: "/repo".into(),
            transcript: Some(Transcript {
                kind: AgentKind::Codex,
                session: "s-codex".into(),
                path: path.clone(),
            }),
            chat: "chat-1".into(),
            account: String::new(),
        };
        let day = 20_000 * DAY_MS;
        log.record_new_tokens(&place, "/repo", day);

        // Resume on a transcript whose tail lacks turn_context must keep the stored model/speed.
        let empty_path = dir.path().join("rollout-empty.jsonl");
        fs::write(&empty_path, "{\"type\":\"session_meta\"}\n").unwrap();
        let resumed_empty = Place {
            agent: "codex".into(),
            cwd: "/repo".into(),
            transcript: Some(Transcript {
                kind: AgentKind::Codex,
                session: "s-codex".into(),
                path: empty_path,
            }),
            chat: "chat-1".into(),
            account: String::new(),
        };
        log.record_launched(&resumed_empty, "chat", true, day + 1);
        let kept: (String, String) = log
            .with_database(|db| {
                db.query_row(
                    "SELECT model, speed FROM transcripts WHERE session = 'codex:s-codex'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(kept, ("gpt-5.4".into(), "fast".into()));

        // Resuming an untracked Codex transcript seeds model/speed from prior turn_context
        // so a token_count emitted before the next turn_context does not log model = "".
        let resumed = Place {
            agent: "codex".into(),
            cwd: "/repo".into(),
            transcript: Some(Transcript {
                kind: AgentKind::Codex,
                session: "s-resumed".into(),
                path: path.clone(),
            }),
            chat: "chat-2".into(),
            account: String::new(),
        };
        log.record_launched(&resumed, "chat", true, day + 2);
        let mut append = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(append, "{}", token_count(250, 120, 30)).unwrap();
        append.flush().unwrap();
        log.record_new_tokens(&resumed, "/repo", day + 3);

        let days = log.reported_days(20_000, 20_001);
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].tokens.len(), 1);
        assert_eq!(days[0].tokens[0].agent, "codex");
        assert_eq!(days[0].tokens[0].model, "gpt-5.4");
        assert_eq!(days[0].tokens[0].speed, "fast");
        assert_eq!(days[0].tokens[0].input, 200);
        assert_eq!(days[0].tokens[0].output, 50);

        // A tracked Codex row already at a non-zero offset with blank model also heals on read.
        let offset = fs::metadata(&path).unwrap().len() as i64;
        log.with_database(|db| {
            db.execute(
                "UPDATE transcripts SET offset = ?1, model = '', speed = 'standard' WHERE session = 'codex:s-resumed'",
                params![offset],
            )
        });
        writeln!(append, "{}", token_count(300, 40, 10)).unwrap();
        append.flush().unwrap();
        log.record_new_tokens(&resumed, "/repo", day + 4);

        let days = log.reported_days(20_000, 20_001);
        assert_eq!(days[0].tokens.len(), 1);
        assert_eq!(days[0].tokens[0].model, "gpt-5.4");
        assert_eq!(days[0].tokens[0].speed, "fast");
        assert_eq!(days[0].tokens[0].input, 240);
        assert_eq!(days[0].tokens[0].output, 60);
    }

    #[test]
    fn activity_queries_use_indexes_and_checkpoint_truncates_wal() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(DATABASE);
        let db = open(&db_path).unwrap();
        let day = 20_000 * DAY_MS;
        db.execute_batch(&format!(
            "INSERT INTO launches VALUES ({day}, 'claude', '/p/a', 'chat', 0);
             INSERT INTO turns VALUES ({day}, {}, 'claude', '/p/a');
             INSERT INTO commits VALUES ('sha-1', {day}, '/p/a', 'claude');
             INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write)
                 VALUES ({day}, 'claude', '/p/a', 10, 5, 0, 0);",
            day + 1_000
        ))
        .unwrap();

        let plan_of = |sql: &str, bound: &[&dyn rusqlite::ToSql]| -> Vec<String> {
            let mut stmt = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            stmt.query_map(bound, |row| row.get::<_, String>(3))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };

        let tables = [
            ("SELECT at_ms, agent, project, resumed FROM launches", "at_ms"),
            (
                "SELECT started_ms, agent, project, ended_ms - started_ms FROM turns",
                "started_ms",
            ),
            ("SELECT at_ms, agent, project FROM commits", "at_ms"),
            (
                "SELECT at_ms, agent, project, model, speed, input, output, cache_read, cache_write FROM tokens",
                "at_ms",
            ),
        ];
        let project_name = "/p/a";
        for (select, time_col) in tables {
            for (project, since) in [
                (Some(project_name), None),
                (Some(project_name), Some(day)),
                (None, Some(day)),
            ] {
                let sql = format!("{select}{}", filter_clause(project, since, time_col));
                let plan = match (project, since) {
                    (Some(project), Some(since)) => plan_of(&sql, &[&project, &since]),
                    (Some(project), None) => plan_of(&sql, &[&project]),
                    (None, Some(since)) => plan_of(&sql, &[&since]),
                    (None, None) => plan_of(&sql, &[]),
                };
                assert!(
                    plan.iter().any(|line| line.contains("USING INDEX")
                        || line.contains("USING COVERING INDEX")),
                    "expected index usage for `{sql}`, got {plan:?}"
                );
                assert!(
                    !plan.iter().any(|line| line.starts_with("SCAN ")),
                    "unexpected full table scan for `{sql}`, got {plan:?}"
                );
            }
        }

        let end_day = day + DAY_MS;
        for sql in [
            "SELECT at_ms, transport, resumed, agent FROM launches WHERE at_ms >= ?1 AND at_ms < ?2",
            "SELECT started_ms, ended_ms, agent FROM turns WHERE started_ms >= ?1 AND started_ms < ?2",
            "SELECT at_ms, agent IS NOT NULL FROM commits WHERE at_ms >= ?1 AND at_ms < ?2",
            "SELECT at_ms, agent, model, speed, input, output, cache_read, cache_write FROM tokens WHERE at_ms >= ?1 AND at_ms < ?2",
        ] {
            let plan = plan_of(sql, &[&day, &end_day]);
            assert!(
                plan.iter()
                    .any(|line| line.contains("USING INDEX") || line.contains("USING COVERING INDEX")),
                "expected index usage for `{sql}`, got {plan:?}"
            );
            assert!(
                !plan.iter().any(|line| line.starts_with("SCAN ")),
                "unexpected full table scan for `{sql}`, got {plan:?}"
            );
        }

        let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
        assert!(
            fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0) > 0,
            "expected non-empty WAL after inserts before checkpoint"
        );
        checkpoint_truncate(&db);
        assert_eq!(
            fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0),
            0,
            "expected checkpoint_truncate to shrink WAL to 0 bytes"
        );
    }
}

// What agents see: the saved databases by name, their tables, and SQL run on
// a connection of the agent's own that stays read-only unless the person
// allowed changes for that database. Agents may also propose a database,
// which is saved only once the person approves it.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::connections::Pool;
use crate::engines::Session;
use crate::error::{DatabaseError, DatabaseResult};
use crate::form::{self, FromUrl, ProfileDraft, UrlAsk};
use crate::profiles::{self, Profile, SaveRequest, Target};
use crate::queries::{self, QueryRequest};
use crate::schema::{Table, TableInfo};
use crate::values::{self, QueryOutcome};

/// Rows an agent gets when it does not ask for a number; enough to see the shape without flooding its context.
const AGENT_ROWS: usize = 100;
const AGENT_MAX_ROWS: usize = 1_000;

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentDatabase {
    pub name: String,
    pub engine: &'static str,
    pub address: String,
    /// Whether the agent may run statements that change data here.
    pub writable: bool,
    /// Whether it is production, where each statement an agent may change data with waits
    /// for the person to allow it.
    pub production: bool,
}

#[derive(Deserialize, Debug)]
pub struct AgentSchemaRequest {
    pub database: String,
    #[serde(default)]
    pub schema: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct DatabaseRef {
    pub database: String,
}

/// A switch on a saved database: whether agents may change its data, or whether it is production.
#[derive(Deserialize, Debug)]
pub struct WritesRequest {
    pub database: String,
    pub on: bool,
}

#[derive(Deserialize, Debug)]
pub struct AgentTableRequest {
    pub database: String,
    pub table: String,
    #[serde(default)]
    pub schema: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct AgentQueryRequest {
    pub database: String,
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Deserialize, Debug)]
pub struct AgentAddRequest {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AgentOutcome {
    #[serde(flatten)]
    pub outcome: QueryOutcome,
    /// Present when the reply was cut to fit, saying how to ask for less.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AgentTables {
    pub schemas: Vec<String>,
    pub schema: String,
    pub tables: Vec<Table>,
}

fn address(target: &Target) -> String {
    match target {
        Target::Postgres(server) => server.address(profiles::POSTGRES_PORT),
        Target::Mysql(server) => server.address(crate::engines::mysql::MYSQL_PORT),
        Target::Sqlite { path } => path.clone(),
    }
}

fn engine_name(target: &Target) -> &'static str {
    match target {
        Target::Postgres(_) => "PostgreSQL",
        Target::Mysql(_) => "MySQL",
        Target::Sqlite { .. } => "SQLite",
    }
}

fn describe_profile(profile: &Profile) -> AgentDatabase {
    let engine = match &profile.target {
        Target::Postgres(_) => "postgres",
        Target::Mysql(_) => "mysql",
        Target::Sqlite { .. } => "sqlite",
    };
    AgentDatabase {
        name: profile.name.clone(),
        engine,
        address: address(&profile.target),
        writable: profile.agent_writes && !profile.read_only,
        production: profile.production,
    }
}

/// Whether a statement an agent runs on the database `database` names may change production
/// data: the database is production, and agents may change its data.
pub fn writes_production(data_dir: &Path, database: DatabaseRef) -> DatabaseResult<bool> {
    let saved = describe_profile(profiles::load(data_dir)?.find(&database.database)?);
    Ok(saved.production && saved.writable)
}

pub fn databases(data_dir: &Path) -> DatabaseResult<Vec<AgentDatabase>> {
    Ok(profiles::load(data_dir)?
        .profiles
        .iter()
        .map(describe_profile)
        .collect())
}

/// The saved database an agent names, or `<server>/<database>` for one database on a saved server
/// that names none.
fn id_of(data_dir: &Path, database: &str) -> DatabaseResult<String> {
    let saved = profiles::load(data_dir)?;
    let found = saved.find(database);
    let (Err(_), Some((server, name))) = (&found, database.split_once('/')) else {
        return Ok(found?.id.clone());
    };
    let id = format!("{}/{name}", saved.find(server)?.id);
    saved.open(&id)?;
    Ok(id)
}

pub async fn tables(
    pool: &Pool,
    data_dir: &Path,
    request: AgentSchemaRequest,
) -> DatabaseResult<AgentTables> {
    let id = id_of(data_dir, &request.database)?;
    let session = pool.session(data_dir, &id).await?;
    let schema = request.schema.unwrap_or_else(|| session.default_schema());
    Ok(AgentTables {
        schemas: session.schemas().await?,
        tables: session.tables(Some(schema.clone())).await?,
        schema,
    })
}

pub async fn describe(
    pool: &Pool,
    data_dir: &Path,
    request: AgentTableRequest,
) -> DatabaseResult<TableInfo> {
    let id = id_of(data_dir, &request.database)?;
    let session = pool.session(data_dir, &id).await?;
    session.describe(request.schema, request.table).await
}

pub async fn query(
    pool: &Pool,
    data_dir: &Path,
    request: AgentQueryRequest,
) -> DatabaseResult<AgentOutcome> {
    let id = id_of(data_dir, &request.database)?;
    let limit = request.limit.unwrap_or(AGENT_ROWS).min(AGENT_MAX_ROWS);
    let request = QueryRequest {
        id,
        sql: request.sql,
        limit: Some(limit),
    };
    let mut outcome = queries::run(pool, data_dir, request).await?;
    let cut = values::fit(
        &mut outcome.results,
        values::AGENT_CELL_CHARACTERS,
        values::AGENT_REPLY_BYTES,
    );
    let note = cut.then(|| {
        format!(
            "Cut to fit: values longer than {} characters end in …, and rows past {} KB were left out. \
             Select fewer columns, add a where, or aggregate rather than asking for more rows.",
            values::AGENT_CELL_CHARACTERS,
            values::AGENT_REPLY_BYTES / 1024
        )
    });
    Ok(AgentOutcome { outcome, note })
}

/// The database an agent proposes, read from its URL as the form reads one, kept from changing
/// data and never named as one already saved.
fn proposed(data_dir: &Path, request: AgentAddRequest) -> DatabaseResult<FromUrl> {
    let profile = ProfileDraft {
        name: request.name.unwrap_or_default().trim().to_owned(),
        read_only: true,
        agent_writes: false,
        ..form::blank()
    };
    let read = form::from_url(UrlAsk {
        url: request.url,
        profile,
    })?;
    let name = read.profile.name.trim();
    if profiles::load(data_dir)?
        .profiles
        .iter()
        .any(|kept| kept.name.eq_ignore_ascii_case(name))
    {
        return Err(DatabaseError::BadArg(format!(
            "a database named {name} is already saved; propose this one under another name"
        )));
    }
    Ok(read)
}

/// What the person is asked about a proposed database, once it is tried. A server is only
/// tried, and only proposed, with its password in the address.
pub async fn add_check(data_dir: &Path, request: AgentAddRequest) -> DatabaseResult<Value> {
    let FromUrl { profile, password } = proposed(data_dir, request)?;
    let draft = form::profile(profile)?;
    if password.is_none() && !matches!(draft.target, Target::Sqlite { .. }) {
        return Err(DatabaseError::BadArg(
            "a server database needs its password in the address, such as postgres://user:password@host/db; the person sees and approves it before it is saved".into(),
        ));
    }
    let named = format!(
        "{} · {}\n{}",
        draft.name,
        engine_name(&draft.target),
        address(&draft.target)
    );
    let session = Session::open(&draft.target, password.as_deref(), true).await?;
    let tried = match session.tables(None).await {
        Ok(tables) => format!("Connected · {} tables", tables.len()),
        Err(_) => "Connected".to_owned(),
    };
    Ok(json!({
        "header": "New database",
        "text": format!("{named}\n{tried}. Agents may read it but not change data."),
        "choices": [
            { "label": "Add", "does": "approve" },
            { "label": "Don't add", "does": "decline" },
        ],
    }))
}

/// Saves the database the person approved, as the agent proposed it.
pub fn add(data_dir: &Path, request: AgentAddRequest) -> DatabaseResult<String> {
    let FromUrl { profile, password } = proposed(data_dir, request)?;
    let saved = profiles::save(
        data_dir,
        SaveRequest {
            profile: form::profile(profile)?,
            password,
        },
    )?;
    Ok(format!(
        "{} is saved; use it by that name. Agents may read it but not change data.",
        saved.name
    ))
}

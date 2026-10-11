// What agents see: the saved databases by name, their tables, and SQL run on
// a connection of the agent's own that stays read-only unless the person
// allowed changes for that database.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::connections::{Access, Pool};
use crate::error::DatabaseResult;
use crate::history::Source;
use crate::profiles::{self, Profile, Target};
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
}

#[derive(Deserialize, Debug)]
pub struct AgentSchemaRequest {
    pub database: String,
    #[serde(default)]
    pub schema: Option<String>,
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

fn describe_profile(profile: &Profile) -> AgentDatabase {
    let (engine, address) = match &profile.target {
        Target::Postgres(server) => ("postgres", server.address(profiles::POSTGRES_PORT)),
        Target::Mysql(server) => ("mysql", server.address(crate::engines::mysql::MYSQL_PORT)),
        Target::Sqlite { path } => ("sqlite", path.clone()),
    };
    AgentDatabase {
        name: profile.name.clone(),
        engine,
        address,
        writable: profile.agent_writes && !profile.read_only,
    }
}

pub fn databases(data_dir: &Path) -> DatabaseResult<Vec<AgentDatabase>> {
    Ok(profiles::load(data_dir)?
        .profiles
        .iter()
        .map(describe_profile)
        .collect())
}

fn id_of(data_dir: &Path, database: &str) -> DatabaseResult<String> {
    Ok(profiles::load(data_dir)?.find(database)?.id.clone())
}

pub async fn tables(
    pool: &Pool,
    data_dir: &Path,
    request: AgentSchemaRequest,
) -> DatabaseResult<AgentTables> {
    let id = id_of(data_dir, &request.database)?;
    let session = pool.session(data_dir, &id, Access::Agent).await?;
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
    let session = pool.session(data_dir, &id, Access::Agent).await?;
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
        run: None,
    };
    let mut outcome = queries::run(pool, data_dir, request, Source::Agent).await?;
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

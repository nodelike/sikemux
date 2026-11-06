// Running an agent's SQL against a saved database.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::connections::Pool;
use crate::engines::Session;
use crate::error::{DatabaseError, DatabaseResult};
use crate::values::{self, QueryOutcome, ResultSet};

/// How long an agent's statement may run before it is stopped, so a runaway query cannot hold the database.
const AGENT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a stopped query has to wind down and roll back.
const STOP_GRACE: Duration = Duration::from_secs(10);

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub id: String,
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// The queries on one connection, run one at a time so a stop reaches only the one it was meant for.
#[derive(Default)]
pub struct Lane {
    turn: tokio::sync::Mutex<()>,
}

pub async fn run(
    pool: &Pool,
    data_dir: &Path,
    request: QueryRequest,
) -> DatabaseResult<QueryOutcome> {
    let sql = request.sql.trim().to_string();
    if sql.is_empty() {
        return Err(DatabaseError::BadArg("there is no SQL to run".into()));
    }
    let (session, read_only) = pool.session_and_mode(data_dir, &request.id).await?;
    let lane = pool.lane(&request.id);
    let started = Instant::now();
    let limit = values::row_limit(request.limit);
    let results = agent_run(lane, session, sql, limit, read_only).await?;
    Ok(QueryOutcome {
        results,
        millis: values::elapsed_millis(started),
    })
}

/// Runs an agent's SQL on a task of its own, so a call dropped halfway, as when the agent's turn is interrupted,
/// still finishes and rolls back. A query past the time limit is stopped, and its connection is not used again.
async fn agent_run(
    lane: Arc<Lane>,
    session: Session,
    sql: String,
    limit: usize,
    read_only: bool,
) -> DatabaseResult<Vec<ResultSet>> {
    let running = tokio::spawn(async move {
        let _turn = lane.turn.lock().await;
        let query = agent_query(&session, &sql, limit, read_only);
        let mut query = std::pin::pin!(query);
        tokio::select! {
            ran = &mut query => ran,
            () = tokio::time::sleep(AGENT_TIMEOUT) => {
                let _ = session.cancel().await;
                let _ = tokio::time::timeout(STOP_GRACE, query).await;
                session.retire();
                Err(DatabaseError::Query(format!(
                    "stopped after {}s; agents' queries are limited to that",
                    AGENT_TIMEOUT.as_secs()
                )))
            }
        }
    });
    running
        .await
        .map_err(|error| DatabaseError::Query(error.to_string()))?
}

/// An agent on a read-only connection gets the guarded path, so no statement can switch read-only off for another.
async fn agent_query(
    session: &Session,
    sql: &str,
    limit: usize,
    read_only: bool,
) -> DatabaseResult<Vec<ResultSet>> {
    if read_only {
        session.query_guarded(sql, limit).await
    } else {
        session.query(sql, limit).await
    }
}

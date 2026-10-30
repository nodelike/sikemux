// Running SQL against a saved database, and keeping each run in its history.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::connections::{Access, Pool};
use crate::engines::Session;
use crate::error::{DatabaseError, DatabaseResult};
use crate::history::{self, Entry, Source};
use crate::profiles;
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
    /// Names this run, so stopping it never stops another console's query on the same connection.
    #[serde(default)]
    pub run: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct CancelRequest {
    pub id: String,
    pub run: String,
}

/// The queries on one connection, run one at a time so a stop reaches only the one it was meant for.
#[derive(Default)]
pub struct Lane {
    turn: tokio::sync::Mutex<()>,
    runs: tokio::sync::Mutex<Runs>,
}

#[derive(Default)]
struct Runs {
    current: Option<String>,
    /// Runs waiting their turn, each with a way to end it without running.
    waiting: HashMap<String, tokio::sync::oneshot::Sender<()>>,
}

fn stopped_before_it_ran() -> DatabaseError {
    DatabaseError::Query("stopped before it ran".into())
}

/// Runs the SQL and adds it to the database's history, failed or not.
pub async fn run(
    pool: &Pool,
    data_dir: &Path,
    request: QueryRequest,
    source: Source,
) -> DatabaseResult<QueryOutcome> {
    let sql = request.sql.trim().to_string();
    if sql.is_empty() {
        return Err(DatabaseError::BadArg("there is no SQL to run".into()));
    }
    let access = match source {
        Source::Person => Access::Person,
        Source::Agent => Access::Agent,
    };
    let (session, read_only) = pool.session_and_mode(data_dir, &request.id, access).await?;
    let lane = pool.lane(&request.id, access);
    let started = Instant::now();
    let limit = values::row_limit(request.limit);
    let ran = match access {
        Access::Agent => agent_run(lane, session, sql.clone(), limit, read_only).await,
        Access::Person | Access::Browse => {
            person_run(&lane, request.run.as_deref(), &session, &sql, limit).await
        }
    };
    let outcome = ran.map(|results| QueryOutcome {
        results,
        millis: values::elapsed_millis(started),
    });
    let entry = Entry {
        sql,
        at: history::now_millis(),
        millis: values::elapsed_millis(started),
        ok: outcome.is_ok(),
        rows: outcome.as_ref().ok().and_then(rows_of_last),
        error: outcome.as_ref().err().map(ToString::to_string),
        source,
    };
    let dir = data_dir.to_path_buf();
    let id = request.id;
    let _ = profiles::blocking(move || history::record(&dir, &id, entry)).await;
    outcome
}

async fn person_run(
    lane: &Lane,
    run: Option<&str>,
    session: &Session,
    sql: &str,
    limit: usize,
) -> DatabaseResult<Vec<ResultSet>> {
    let _turn = match run {
        Some(run) => {
            let (stop, mut stopped) = tokio::sync::oneshot::channel();
            lane.runs.lock().await.waiting.insert(run.to_string(), stop);
            tokio::select! {
                turn = lane.turn.lock() => {
                    let mut runs = lane.runs.lock().await;
                    if runs.waiting.remove(run).is_none() {
                        return Err(stopped_before_it_ran());
                    }
                    runs.current = Some(run.to_string());
                    turn
                }
                _ = &mut stopped => return Err(stopped_before_it_ran()),
            }
        }
        None => {
            let turn = lane.turn.lock().await;
            lane.runs.lock().await.current = None;
            turn
        }
    };
    let ran = session.query(sql, limit).await;
    lane.runs.lock().await.current = None;
    ran
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

fn rows_of_last(outcome: &QueryOutcome) -> Option<u64> {
    let last = outcome.results.last()?;
    last.affected
        .or_else(|| u64::try_from(last.rows.len()).ok())
}

/// Stops one run: on the server when it is the one running, or before it starts when it is waiting its turn.
/// A run that already ended has nothing to stop.
pub async fn cancel(pool: &Pool, request: CancelRequest) -> DatabaseResult<()> {
    let lane = pool.lane(&request.id, Access::Person);
    let mut runs = lane.runs.lock().await;
    if runs.current.as_deref() == Some(request.run.as_str()) {
        if let Some(session) = pool.running(&request.id).await {
            return session.cancel().await;
        }
    } else if let Some(stop) = runs.waiting.remove(&request.run) {
        let _ = stop.send(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::SaveRequest;

    const SLOW: &str =
        "with recursive c(x) as (select 1 union all select x + 1 from c where x < 1000000000) \
                        select count(*) from c";

    fn request(id: &str, sql: &str, run: &str) -> QueryRequest {
        QueryRequest {
            id: id.into(),
            sql: sql.into(),
            limit: None,
            run: Some(run.into()),
        }
    }

    #[tokio::test]
    async fn a_stop_reaches_only_its_own_run_and_browsing_is_not_held_up() {
        let dir =
            std::env::temp_dir().join(format!("sikemux-database-runs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = crate::engines::sqlite::tests::fixture("runs");
        let saved: SaveRequest = serde_json::from_value(serde_json::json!({
            "profile": { "name": "Local", "engine": "sqlite", "path": file }
        }))
        .unwrap();
        let id = profiles::save(&dir, saved).unwrap().id;
        let pool = Arc::new(Pool::default());

        let spawn_run = |sql: &'static str, run: &'static str| {
            let (pool, dir, id) = (Arc::clone(&pool), dir.clone(), id.clone());
            tokio::spawn(async move { run_query(&pool, &dir, request(&id, sql, run)).await })
        };
        let slow = spawn_run(SLOW, "slow");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let queued = spawn_run("select 1", "queued");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let schemas = tokio::time::timeout(
            Duration::from_secs(2),
            pool.session(&dir, &id, Access::Browse),
        )
        .await
        .unwrap()
        .unwrap()
        .schemas()
        .await
        .unwrap();
        assert_eq!(schemas, vec!["main".to_string()]);

        let stop = |run: &str| CancelRequest {
            id: id.clone(),
            run: run.into(),
        };
        cancel(&pool, stop("someone-else")).await.unwrap();
        cancel(&pool, stop("queued")).await.unwrap();
        let queued = tokio::time::timeout(Duration::from_secs(2), queued)
            .await
            .unwrap()
            .unwrap();
        let Err(DatabaseError::Query(message)) = queued else {
            panic!("expected the queued run to be stopped")
        };
        assert!(message.contains("before it ran"), "{message}");
        assert!(!slow.is_finished());

        cancel(&pool, stop("slow")).await.unwrap();
        let slow = tokio::time::timeout(Duration::from_secs(5), slow)
            .await
            .unwrap()
            .unwrap();
        let Err(DatabaseError::Query(message)) = slow else {
            panic!("expected the slow run to be stopped")
        };
        assert!(message.contains("interrupt"), "{message}");
    }

    async fn run_query(
        pool: &Pool,
        dir: &Path,
        request: QueryRequest,
    ) -> DatabaseResult<QueryOutcome> {
        run(pool, dir, request, Source::Person).await
    }
}

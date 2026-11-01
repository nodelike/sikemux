// Reaching a database: keeping one connection open per saved database, so an
// agent's calls do not sign in each time.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, PoisonError};

use crate::engines::Session;
use crate::error::DatabaseResult;
use crate::profiles;
use crate::queries::Lane;

#[derive(Clone)]
struct Open {
    session: Session,
    read_only: bool,
}

/// A saved database's id.
type Key = String;

#[derive(Default)]
pub struct Pool {
    open: tokio::sync::Mutex<HashMap<Key, Open>>,
    /// One lock per connection, so calls that arrive together sign in once rather than each on their own.
    opening: std::sync::Mutex<HashMap<Key, Arc<tokio::sync::Mutex<()>>>>,
    lanes: std::sync::Mutex<HashMap<Key, Arc<Lane>>>,
}

impl Pool {
    async fn kept(&self, key: &Key) -> Option<Open> {
        let mut open = self.open.lock().await;
        match open.get(key) {
            Some(kept) if kept.session.is_alive() => Some(kept.clone()),
            Some(_) => {
                open.remove(key);
                None
            }
            None => None,
        }
    }

    /// The open connection to a saved database, signing in first when there is none or it has dropped.
    async fn open(&self, data_dir: &Path, id: &str) -> DatabaseResult<Open> {
        let key = id.to_string();
        if let Some(kept) = self.kept(&key).await {
            return Ok(kept);
        }
        let opening = Arc::clone(
            self.opening
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(key.clone())
                .or_default(),
        );
        let _one_at_a_time = opening.lock().await;
        if let Some(kept) = self.kept(&key).await {
            return Ok(kept);
        }
        let dir = data_dir.to_path_buf();
        let wanted = id.to_string();
        let (profile, password) = profiles::blocking(move || {
            let profile = profiles::load(&dir)?.open(&wanted)?;
            let password = profiles::saved_password(&profile)?;
            Ok((profile, password))
        })
        .await?;
        let read_only = profile.read_only || !profile.agent_writes;
        let session = Session::open(&profile.target, password.as_deref(), read_only).await?;
        let opened = Open { session, read_only };
        self.open.lock().await.insert(key, opened.clone());
        Ok(opened)
    }

    /// Where the queries on one connection wait their turn.
    pub fn lane(&self, id: &str) -> Arc<Lane> {
        Arc::clone(
            self.lanes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(id.to_string())
                .or_default(),
        )
    }

    pub async fn session(&self, data_dir: &Path, id: &str) -> DatabaseResult<Session> {
        Ok(self.open(data_dir, id).await?.session)
    }

    /// The connection and whether it was opened read-only.
    pub async fn session_and_mode(
        &self,
        data_dir: &Path,
        id: &str,
    ) -> DatabaseResult<(Session, bool)> {
        let opened = self.open(data_dir, id).await?;
        Ok((opened.session, opened.read_only))
    }

    /// Closes the database's connection, and those to each database on it, as after it is removed.
    pub async fn forget(&self, id: &str) {
        let other = |kept: &String| kept != id && profiles::split_id(kept).0 != id;
        self.open.lock().await.retain(|kept, _| other(kept));
        self.lanes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|kept, _| other(kept));
    }
}

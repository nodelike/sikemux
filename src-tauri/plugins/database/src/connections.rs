// Reaching a database: trying a connection before it is saved, and keeping
// one open per saved database so browsing and queries do not sign in each time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::engines::Session;
use crate::error::DatabaseResult;
use crate::profiles::{self, Draft};
use crate::queries::Lane;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    pub profile: Draft,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Tested {
    pub version: String,
    pub millis: u64,
}

/// Tries the profile as typed. Editing a saved one without retyping its password uses the saved password,
/// unless the server or user changed.
pub async fn test(data_dir: PathBuf, request: TestRequest) -> DatabaseResult<Tested> {
    let started = Instant::now();
    let TestRequest { profile, password } = request;
    let draft = profile.clone();
    let password =
        profiles::blocking(move || profiles::password_for(&data_dir, &draft, password)).await?;
    let session = Session::open(&profile.target, password.as_deref(), profile.read_only).await?;
    let version = session.version().await?;
    Ok(Tested {
        version,
        millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    pub id: String,
    pub version: String,
}

#[derive(Clone)]
struct Open {
    session: Session,
    version: String,
    read_only: bool,
}

/// Who a connection is for. Agents get one of their own, so it can be read-only while the person's is not.
/// Browsing tables has its own read-only one, so a long query does not hold up the list of tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    Person,
    Browse,
    Agent,
}

type Key = (String, Access);

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
    async fn open(&self, data_dir: &Path, id: &str, access: Access) -> DatabaseResult<Open> {
        let key = (id.to_string(), access);
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
            let profile = profiles::load(&dir)?.get(&wanted)?.clone();
            let password = profiles::saved_password(&profile)?;
            Ok((profile, password))
        })
        .await?;
        let read_only = match access {
            Access::Person => profile.read_only,
            Access::Browse => true,
            Access::Agent => profile.read_only || !profile.agent_writes,
        };
        let session = Session::open(&profile.target, password.as_deref(), read_only).await?;
        let version = session.version().await?;
        let opened = Open {
            session,
            version,
            read_only,
        };
        self.open.lock().await.insert(key, opened.clone());
        Ok(opened)
    }

    /// Where the queries on one connection wait their turn.
    pub fn lane(&self, id: &str, access: Access) -> Arc<Lane> {
        Arc::clone(
            self.lanes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry((id.to_string(), access))
                .or_default(),
        )
    }

    /// The person's open connection, without signing in when there is none.
    pub async fn running(&self, id: &str) -> Option<Session> {
        self.kept(&(id.to_string(), Access::Person))
            .await
            .map(|kept| kept.session)
    }

    pub async fn session(
        &self,
        data_dir: &Path,
        id: &str,
        access: Access,
    ) -> DatabaseResult<Session> {
        Ok(self.open(data_dir, id, access).await?.session)
    }

    /// The connection and whether it was opened read-only.
    pub async fn session_and_mode(
        &self,
        data_dir: &Path,
        id: &str,
        access: Access,
    ) -> DatabaseResult<(Session, bool)> {
        let opened = self.open(data_dir, id, access).await?;
        Ok((opened.session, opened.read_only))
    }

    pub async fn connect(&self, data_dir: &Path, id: &str) -> DatabaseResult<Connected> {
        let opened = self.open(data_dir, id, Access::Person).await?;
        Ok(Connected {
            id: id.to_string(),
            version: opened.version,
        })
    }

    /// Closes the person's and the agents' connections, as after the profile is edited or removed.
    pub async fn forget(&self, id: &str) {
        self.open.lock().await.retain(|(kept, _), _| kept != id);
        self.lanes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(kept, _), _| kept != id);
    }

    pub async fn connected(&self) -> Vec<Connected> {
        let mut connected: Vec<Connected> = self
            .open
            .lock()
            .await
            .iter()
            .filter(|((_, access), kept)| *access != Access::Agent && kept.session.is_alive())
            .map(|((id, _), kept)| Connected {
                id: id.clone(),
                version: kept.version.clone(),
            })
            .collect();
        connected.sort_by(|a, b| a.id.cmp(&b.id));
        connected.dedup_by(|a, b| a.id == b.id);
        connected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::SaveRequest;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sikemux-database-pool-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn save_sqlite(dir: &Path, name: &str, path: &Path) -> String {
        let request: SaveRequest = serde_json::from_value(serde_json::json!({
            "profile": { "name": name, "engine": "sqlite", "path": path }
        }))
        .unwrap();
        profiles::save(dir, request).unwrap().id
    }

    #[tokio::test]
    async fn a_connection_is_opened_once_and_forgotten_on_request() {
        let dir = scratch("once");
        let file = crate::engines::sqlite::tests::fixture("pool-once");
        let id = save_sqlite(&dir, "Local", &file);
        let pool = Pool::default();
        assert!(pool.connected().await.is_empty());
        let connected = pool.connect(&dir, &id).await.unwrap();
        assert!(connected.version.starts_with("SQLite"));
        assert_eq!(pool.connected().await, vec![connected]);
        pool.forget(&id).await;
        assert!(pool.connected().await.is_empty());
    }

    #[tokio::test]
    async fn agents_read_only_until_the_person_lets_them_change_data() {
        let dir = scratch("agent");
        let file = crate::engines::sqlite::tests::fixture("pool-agent");
        let id = save_sqlite(&dir, "Local", &file);
        let pool = Pool::default();
        let delete = "delete from orders";
        let agent = pool.session(&dir, &id, Access::Agent).await.unwrap();
        assert!(agent.query(delete, 1).await.is_err());
        let person = pool.session(&dir, &id, Access::Person).await.unwrap();
        assert!(person.query(delete, 1).await.is_ok());
        assert_eq!(pool.connected().await.len(), 1);

        let request: SaveRequest = serde_json::from_value(serde_json::json!({
            "profile": { "id": id, "name": "Local", "engine": "sqlite", "path": file, "agentWrites": true }
        }))
        .unwrap();
        profiles::save(&dir, request).unwrap();
        pool.forget(&id).await;
        let agent = pool.session(&dir, &id, Access::Agent).await.unwrap();
        assert!(agent
            .query("delete from customers where id = 2", 1)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn an_unknown_profile_or_a_failed_sign_in_leaves_nothing_open() {
        let dir = scratch("failed");
        let pool = Pool::default();
        assert!(pool.connect(&dir, "nope").await.is_err());
        let id = save_sqlite(&dir, "Gone", Path::new("/nope/gone.db"));
        assert!(pool.connect(&dir, &id).await.is_err());
        assert!(pool.connected().await.is_empty());
    }
}

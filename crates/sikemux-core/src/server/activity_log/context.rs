//! What each model call of a chat's session carried, and each compaction of it, so a day can
//! say how big its calls were, how often the cache was rewritten cold and how often it compacted.

use std::sync::Arc;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{in_background, ActivityLog};

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
    /// What writing the summary cost, once the turn's answer says.
    pub cost: ModelCall,
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
     CREATE INDEX model_calls_by_chat ON model_calls (chat, at_ms);
     CREATE TABLE compactions (
         at_ms INTEGER NOT NULL,
         chat TEXT NOT NULL,
         project TEXT NOT NULL,
         trigger TEXT NOT NULL,
         before INTEGER NOT NULL,
         after INTEGER NOT NULL,
         ms INTEGER NOT NULL,
         input INTEGER NOT NULL,
         cache_read INTEGER NOT NULL,
         cache_write INTEGER NOT NULL,
         output INTEGER NOT NULL
     );
     CREATE INDEX compactions_by_chat ON compactions (chat, at_ms);";

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
                    let cost = &compaction.cost;
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
                            cost.input as i64,
                            cost.cache_read as i64,
                            cost.cache_write as i64,
                            cost.output as i64
                        ],
                    )?;
                }
                saving.commit()
            });
        });
    }
}

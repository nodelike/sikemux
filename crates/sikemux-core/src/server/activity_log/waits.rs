//! How long agents stood waiting on the person each UTC day, and how long each kind of call
//! took to be answered from each client, for the usage report.

use std::sync::Arc;
use std::time::Duration;

use rusqlite::params;
use sikemux_process::clock::epoch_ms;

use super::super::access::Peer;
use super::features::via;
use super::{in_background, ActivityLog, DAY_MS};

impl ActivityLog {
    /// A call on the person of kind `kind` closed after `waited`: answered from `answered_by`'s
    /// client, or with none when it went away unanswered, as when its agent closed.
    pub(crate) fn call_closed(
        self: &Arc<Self>,
        kind: &str,
        answered_by: Option<&Peer>,
        waited: Duration,
    ) {
        self.closed(kind, answered_by.and_then(via), waited);
    }

    /// A call of kind `kind` the person answered by writing in its asker's chat, after `waited`.
    pub(crate) fn call_answered_in_chat(self: &Arc<Self>, kind: &str, waited: Duration) {
        self.closed(kind, Some("chat"), waited);
    }

    fn closed(self: &Arc<Self>, kind: &str, via: Option<&'static str>, waited: Duration) {
        let day = (epoch_ms() as i64).div_euclid(DAY_MS);
        let ms = waited.as_millis().min(i64::MAX as u128) as i64;
        let answer = via.map(|via| (kind.to_owned(), via));
        let log = self.clone();
        in_background(move || {
            log.with_database(|db| {
                db.prepare_cached(
                    "INSERT INTO waited (day, ms) VALUES (?1, ?2)
                     ON CONFLICT(day) DO UPDATE SET ms = ms + excluded.ms",
                )?
                .execute(params![day, ms])?;
                if let Some((kind, via)) = &answer {
                    db.prepare_cached(
                        "INSERT INTO answers (day, kind, via, count, ms) VALUES (?1, ?2, ?3, 1, ?4)
                         ON CONFLICT(day, kind, via)
                         DO UPDATE SET count = count + 1, ms = ms + excluded.ms",
                    )?
                    .execute(params![day, kind, via, ms])?;
                }
                Ok(())
            });
        });
    }
}

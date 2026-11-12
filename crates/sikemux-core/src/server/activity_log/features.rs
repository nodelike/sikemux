//! Which features were used each UTC day, for the usage report: the core's
//! calls whose row says so and the window's commands. Each counts by name, with
//! who asked: the app, a program on this host such as an agent's tools or the
//! CLI, or a paired device.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::params;
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::super::access::Peer;
use super::{in_background, ActivityLog, DAY_MS};

const WRITE_EVERY: Duration = Duration::from_secs(10 * 60);

/// The window's commands that never reach the core as a call of their own.
const COMMANDS: &[&str] = &[
    "palette.commands",
    "palette.files",
    "search.global",
    "settings.toggle",
    "view.focusMode",
    "desk.toggle",
    "desk.alone",
    "theme.switch",
    "agent.permissions",
    "session.lastUsed",
    "window.files",
    "window.terminal",
    "window.git",
    "window.agents",
    "window.search",
    "window.board",
    "window.goals",
    "window.hosts",
    "window.playbook",
    "chat.openWorker",
    "browser.tabNew",
    "browser.snip",
    "browser.width",
    "browser.widthAll",
    "text.sizeIncrease",
];

/// Whether a call's use goes in the report, and under what name.
#[derive(Clone, Copy)]
pub(crate) enum Counted {
    No,
    Yes,
    /// Under the name the call's params give in this field, as a plugin's.
    ByField(&'static str),
}

#[derive(Default)]
pub(super) struct Uses {
    counts: HashMap<(i64, String, &'static str), i64>,
    written: Option<Instant>,
}

/// Who asked, as the report names them; a shared chat's viewers never count.
pub(super) fn via(peer: &Peer) -> Option<&'static str> {
    match peer {
        Peer::App => Some("app"),
        Peer::Local => Some("local"),
        Peer::Device { .. } => Some("device"),
        Peer::Share { .. } => None,
    }
}

/// A name a counted call's params may give, as the plugins compiled in are named.
fn plain(name: &str) -> bool {
    name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

impl ActivityLog {
    pub(crate) fn call_used(
        self: &Arc<Self>,
        counted: Counted,
        method: &str,
        params: &Value,
        peer: &Peer,
    ) {
        let Some(via) = via(peer) else {
            return;
        };
        let feature = match counted {
            Counted::No => None,
            Counted::Yes => Some(method.to_owned()),
            Counted::ByField(field) => params[field]
                .as_str()
                .filter(|name| plain(name))
                .map(|name| format!("{field}.{name}")),
        };
        if let Some(feature) = feature {
            self.used(feature, via);
        }
    }

    /// The person sent a message to a chat: a project's chief, or any other.
    pub(crate) fn person_wrote(self: &Arc<Self>, to_chief: bool) {
        let feature = if to_chief {
            "message.chief"
        } else {
            "message.chat"
        };
        self.used(feature.to_owned(), "person");
    }

    pub(crate) fn command_used(self: &Arc<Self>, id: &str) {
        if COMMANDS.contains(&id) {
            self.used(id.to_owned(), "app");
        }
    }

    fn used(self: &Arc<Self>, feature: String, via: &'static str) {
        let day = (epoch_ms() as i64).div_euclid(DAY_MS);
        let due = {
            let mut uses = self.uses.locked();
            *uses.counts.entry((day, feature, via)).or_default() += 1;
            uses.written.is_none_or(|at| at.elapsed() >= WRITE_EVERY)
        };
        if due {
            let log = self.clone();
            in_background(move || log.write_uses());
        }
    }

    pub(super) fn write_uses(&self) {
        let counts = {
            let mut uses = self.uses.locked();
            uses.written = Some(Instant::now());
            std::mem::take(&mut uses.counts)
        };
        if counts.is_empty() {
            return;
        }
        self.with_database(|db| {
            let mut add = db.prepare_cached(
                "INSERT INTO features (day, feature, via, count) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(day, feature, via) DO UPDATE SET count = count + excluded.count",
            )?;
            for ((day, feature, via), count) in &counts {
                add.execute(params![day, feature, via, count])?;
            }
            Ok(())
        });
    }
}

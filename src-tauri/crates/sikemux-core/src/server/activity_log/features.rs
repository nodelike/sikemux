//! Which features were used each UTC day, for the usage report: the core's
//! calls and the window's commands that do something, never those that only
//! read or watch. Each counts by name, with who asked: the app, a program on
//! this host such as an agent's tools or the CLI, or a paired device.

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

const CALLS: &[&str] = &[
    "startChat",
    "startTerminalAgent",
    "createAgent",
    "resumeSession",
    "steerQueued",
    "chatFind",
    "sleepAgent",
    "renameAgent",
    "moveAgentToProject",
    "createAgentWorktree",
    "agentReview",
    "removeAgentWorktree",
    "answerQuestion",
    "answerQuestions",
    "sendSession",
    "shareChat",
    "pinChat",
    "setSpaces",
    "splitPane",
    "separatePane",
    "duplicateTab",
    "addAgentAccount",
    "selectAgentAccount",
    "setAccountAutoSwitch",
    "agentSignIn",
    "modelProviderConnect",
    "createSkill",
    "installSkill",
    "adoptSkill",
    "boardAdd",
    "boardAddTicket",
    "boardUpdate",
    "commitStaged",
    "commitLines",
    "git.commit",
    "git.aiMessage",
    "git.push",
    "git.pull",
    "git.branchCreate",
    "git.checkoutSmart",
    "git.stashPush",
    "git.blame",
    "git.merge",
    "git.compare",
    "git.stagePaths",
    "git.discardFiles",
    "searchStream",
    "searchReplace",
    "lsp.locations",
    "codeHostPull",
    "codeHostNewPull",
    "codeHostCheckout",
    "codeHostWorkOn",
    "codeHostRun",
    "taskStart",
    "taskRerunFailed",
    "startPreview",
    "runCustomCommand",
    "openSshTerminal",
    "linkHost",
    "openOnHost",
    "answerPairing",
    "stashPrompt",
    "stashFile",
    "stashPicture",
    "attachStash",
    "themeFromPixels",
    "setFaceStyle",
    "setAgentNameTheme",
    "updateInstall",
    "installCli",
];

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
    "browser.tabNew",
    "text.sizeIncrease",
];

#[derive(Default)]
pub(super) struct Uses {
    counts: HashMap<(i64, String, &'static str), i64>,
    written: Option<Instant>,
}

/// A plugin's own name, as the plugins compiled in are named.
fn plain(name: &str) -> bool {
    name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

impl ActivityLog {
    pub(crate) fn call_used(self: &Arc<Self>, method: &str, params: &Value, peer: &Peer) {
        let via = match peer {
            Peer::App => "app",
            Peer::Local => "local",
            Peer::Device { .. } => "device",
            Peer::Share { .. } => return,
        };
        let feature = match method {
            "plugin" => params["plugin"]
                .as_str()
                .filter(|name| plain(name))
                .map(|name| format!("plugin.{name}")),
            method => CALLS.contains(&method).then(|| method.to_owned()),
        };
        if let Some(feature) = feature {
            self.used(feature, via);
        }
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

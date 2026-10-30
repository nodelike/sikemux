//! ⌘⇧N, the agent palette: a new chat with any agent the host has, the agents
//! open in the project, and its pinned and recent chats to take up again.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Context, SharedString, Window};
use serde_json::json;
use sikemux_wire::protocol::{AgentStatus, LauncherInfo, PermissionMode};

use crate::icons::Icon;
use crate::palette::{Choice, Mark, Row, Spec};
use crate::workbench::Workbench;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "agent.choose")]
pub struct ChooseAgent;

const NEW: &str = "new:";
const SIGN_IN: &str = "signin:";
const INSTALL: &str = "install:";
const OPEN: &str = "open:";
const RESUME: &str = "resume:";

/// Agents whose accounts the core signs in to itself; the rest sign in with their own
/// command, in the sheet Settings › Installed shows.
const SIGNS_IN: [&str; 2] = ["claude", "codex"];
const YOLO: &str = "bypass";

impl Workbench {
    pub fn toggle_agent_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project().map(|project| project.id.clone()) else {
            return;
        };
        let rows = self.agent_palette_rows(&project);
        let yolo = self
            .view
            .as_ref()
            .is_some_and(|view| view.workspace.permission_mode == YOLO);
        let mode = Rc::new(Cell::new(usize::from(yolo)));
        let choice = Choice {
            options: vec![(Icon::Lock, "safe".into()), (Icon::BoltFill, "yolo".into())],
            chosen: mode.clone(),
            armed: Some(1),
        };
        let spec = Spec::new(
            "agent.choose",
            "search agents",
            rows,
            move |row, bench, window, cx| {
                let key: &str = &row.key;
                if let Some(launcher) = key.strip_prefix(NEW) {
                    let provider = launcher_of(bench, launcher)
                        .map(|known| known.provider.clone())
                        .unwrap_or_default();
                    let can_skip = bench
                        .reads
                        .catalog
                        .kind(&provider)
                        .is_some_and(|kind| kind.bypass);
                    let launch_mode = if mode.get() == 1 && can_skip {
                        PermissionMode::Bypass
                    } else {
                        PermissionMode::WorkspaceWrite
                    };
                    bench.target().start_chat(
                        launcher.to_owned(),
                        project.clone(),
                        Some(launch_mode),
                        None,
                    );
                } else if let Some(launcher) = key.strip_prefix(SIGN_IN) {
                    sign_in(bench, launcher, window, cx);
                } else if let Some(launcher) = key.strip_prefix(INSTALL) {
                    install(bench, launcher, window, cx);
                } else if let Some(agent) = key.strip_prefix(OPEN) {
                    bench.open_agent(agent);
                } else if let Some(id) = key.strip_prefix(RESUME) {
                    if let Some(recent) = bench.saved_chat(id).cloned() {
                        bench.resume_chat(&recent, cx);
                    }
                }
            },
        )
        .empty("no agent matches")
        .choice(choice)
        .on_stage()
        .ruled();
        self.toggle_palette(spec, window, cx);
        if self.overlays.palette.is_some() {
            self.look_for_older(true, crate::recent::PALETTE_PAGE, cx);
        }
    }

    /// Offers the open ⌘⇧N the saved chats that came in since it opened.
    pub fn refill_agent_palette(&mut self, cx: &mut Context<Self>) {
        let open = self
            .overlays
            .palette
            .as_ref()
            .is_some_and(|palette| palette.id() == "agent.choose");
        let Some(project) = self
            .project()
            .map(|project| project.id.clone())
            .filter(|_| open)
        else {
            return;
        };
        let rows = self.agent_palette_rows(&project);
        if let Some(palette) = self.overlays.palette.as_mut() {
            palette.refill(rows, cx);
        }
    }

    fn agent_palette_rows(&self, project: &str) -> Vec<Row> {
        let kind_of = |provider: &str| self.reads.catalog.kind(provider);
        let mut rows = Vec::new();
        let mut sign_ins = Vec::new();
        for launcher in self.launchers().iter().chain(self.terminal_launchers()) {
            let kind = kind_of(&launcher.provider);
            let signs_in = SIGNS_IN.contains(&launcher.provider.as_str())
                || kind.is_some_and(|kind| kind.login.is_some());
            let bypasses = kind.is_some_and(|kind| kind.bypass);
            let new = || {
                Row::new(
                    format!("{NEW}{}", launcher.id),
                    format!("+ new {}", launcher.label),
                )
                .unarmed(!bypasses)
            };
            let row = match launcher.status.as_deref() {
                Some("missing") if kind.is_some_and(|kind| kind.install.is_some()) => Row::new(
                    format!("{INSTALL}{}", launcher.id),
                    format!("Install {}", launcher.label),
                )
                .meta(kind.map(|kind| kind.about.clone()).unwrap_or_default()),
                Some("missing" | "broken") => {
                    new().meta("Agent executable is unavailable").disabled(true)
                }
                Some("signedOut") => new().meta("signed out"),
                _ => new(),
            };
            if launcher.status.as_deref() == Some("signedOut") && signs_in {
                sign_ins.push(
                    Row::new(
                        format!("{SIGN_IN}{}", launcher.id),
                        format!("Sign in to {}", launcher.label),
                    )
                    .meta("Signed out")
                    .mark(Mark::Agent(launcher.provider.clone()))
                    .group(NEW)
                    .also(&launcher.provider),
                );
            }
            rows.push(
                row.mark(Mark::Agent(launcher.provider.clone()))
                    .group(NEW)
                    .also(&launcher.provider),
            );
        }
        rows.extend(sign_ins);
        let unarmed = |provider: &str| kind_of(provider).is_none_or(|kind| !kind.bypass);
        let mut taken_up = Vec::new();
        for agent in self.agents_of(project) {
            let handle = agent.name.as_ref().map(|name| format!("@{name}"));
            let pinned = self.chat_of(&agent.id).is_some_and(|chat| chat.pinned);
            let row = Row::new(format!("{OPEN}{}", agent.id), agent.title.clone())
                .handle(handle.clone().map(SharedString::from))
                .state(agent.status)
                .unarmed(unarmed(&agent.provider))
                .mark(Mark::Open(Box::new(agent.clone())))
                .group(RESUME)
                .also(handle.as_deref().unwrap_or_default())
                .also(&agent.provider);
            taken_up.push((pinned, agent.active_at, row));
        }
        let saved = self
            .saved_chats(project)
            .into_iter()
            .filter(|recent| recent.agent_id.is_none());
        for chat in saved {
            let row = Row::new(format!("{RESUME}{}", chat.id), chat.title.clone())
                .meta(if chat.active_at == 0 {
                    String::new()
                } else {
                    crate::ui::time::age(chat.active_at)
                })
                .unarmed(unarmed(&chat.provider))
                .mark(Mark::Agent(chat.provider.clone()))
                .group(RESUME)
                .also(&chat.provider);
            taken_up.push((chat.pinned, chat.active_at, row));
        }
        taken_up.sort_by_key(|(pinned, active_at, _)| {
            (std::cmp::Reverse(*pinned), std::cmp::Reverse(*active_at))
        });
        rows.extend(taken_up.into_iter().map(|(_, _, row)| row));
        rows
    }
}

/// The launcher `id` names, of a chat agent or one that runs only in its terminal.
fn launcher_of<'a>(bench: &'a Workbench, id: &str) -> Option<&'a LauncherInfo> {
    bench
        .launchers()
        .iter()
        .chain(bench.terminal_launchers())
        .find(|known| known.id == id)
}

/// Signs the launcher's agent in: the core does it for the agents whose accounts it keeps,
/// and the agent's own command runs in Settings › Installed for the rest.
fn sign_in(
    bench: &mut Workbench,
    launcher: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(provider) = launcher_of(bench, launcher).map(|known| known.provider.clone()) else {
        return;
    };
    if SIGNS_IN.contains(&provider.as_str()) {
        bench.call("signInLauncher", json!({ "launcher": launcher }));
        return;
    }
    bench.open_settings(crate::settings::Page::Installed, window, cx);
    let folder = bench
        .accounts
        .current(&provider)
        .and_then(|profile| profile.config_path.clone());
    let sheet = bench
        .agent_setup
        .found
        .iter()
        .flatten()
        .find(|found| found.kind == provider)
        .and_then(|found| crate::agents::setup::signing_in(found, folder.as_deref()));
    if let Some(sheet) = sheet {
        crate::agents::setup::run(bench, sheet, cx);
    }
}

/// Runs the agent's installer in Settings › Installed, as the agent's own sign-in runs there.
fn install(
    bench: &mut Workbench,
    launcher: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(launcher) = launcher_of(bench, launcher).cloned() else {
        return;
    };
    let command = bench
        .reads
        .catalog
        .kind(&launcher.provider)
        .and_then(|kind| kind.install.clone());
    let Some(command) = command else {
        return;
    };
    let sheet = crate::agents::setup::installer(&launcher.provider, &launcher.label, command);
    bench.open_settings(crate::settings::Page::Installed, window, cx);
    crate::agents::setup::run(bench, sheet, cx);
}

pub(crate) fn status_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Starting => "starting",
        AgentStatus::Blocked => "needs you",
        AgentStatus::Done => "done",
        AgentStatus::Idle => "idle",
        AgentStatus::Asleep => "asleep",
        AgentStatus::Stopped => "stopped",
    }
}

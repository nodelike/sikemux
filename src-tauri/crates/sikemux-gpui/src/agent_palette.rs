//! ⌘⇧N, the agent palette: a new chat with any agent the host has, the agents
//! open in the project, and its pinned and recent chats to take up again.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Context, SharedString, Window};
use serde_json::{json, Value};
use sikemux_wire::protocol::AgentStatus;

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
/// command, in the sheet Settings › Agents shows.
const SIGNS_IN: [&str; 2] = ["claude", "codex"];
const SAFE: &str = "workspace-write";
const YOLO: &str = "bypass";

impl Workbench {
    pub fn toggle_agent_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project().map(|project| project.id.clone()) else {
            return;
        };
        let catalog = self.read("catalog").and_then(Value::as_array).cloned();
        let kind_of = move |provider: &str| {
            catalog
                .iter()
                .flatten()
                .find(|kind| kind["type"] == provider)
                .cloned()
        };
        let mut rows = Vec::new();
        let mut sign_ins = Vec::new();
        for launcher in self.launchers() {
            let kind = kind_of(&launcher.provider);
            let field = |name: &str| {
                kind.as_ref()
                    .and_then(|kind| kind[name].as_str())
                    .map(str::to_owned)
            };
            let signs_in = SIGNS_IN.contains(&launcher.provider.as_str())
                || kind.as_ref().is_some_and(|kind| kind["login"].is_array());
            let bypasses = kind.as_ref().is_some_and(|kind| kind["bypass"] == true);
            let new = || {
                Row::new(
                    format!("{NEW}{}", launcher.id),
                    format!("+ new {}", launcher.label),
                )
                .unarmed(!bypasses)
            };
            let row = match launcher.status.as_deref() {
                Some("missing") if field("install").is_some() => Row::new(
                    format!("{INSTALL}{}", launcher.id),
                    format!("Install {}", launcher.label),
                )
                .meta(field("about").unwrap_or_default()),
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
        let unarmed = |provider: &str| kind_of(provider).is_none_or(|kind| kind["bypass"] != true);
        let mut pinned_open = Vec::new();
        for agent in self.agents_of(&project) {
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
            match pinned {
                true => pinned_open.push(row),
                false => rows.push(row),
            }
        }
        rows.extend(pinned_open);
        let recent = self
            .view
            .as_ref()
            .map(|view| view.recent.as_slice())
            .unwrap_or_default();
        let saved = recent
            .iter()
            .filter(|recent| recent.project == project && recent.agent_id.is_none());
        let (pinned, rest): (Vec<_>, Vec<_>) = saved.partition(|recent| recent.pinned);
        for chat in pinned.into_iter().chain(rest) {
            rows.push(
                Row::new(format!("{RESUME}{}", chat.id), chat.title.clone())
                    .meta(if chat.active_at == 0 {
                        String::new()
                    } else {
                        crate::agents::age(chat.active_at)
                    })
                    .unarmed(unarmed(&chat.provider))
                    .mark(Mark::Agent(chat.provider.clone()))
                    .group(RESUME)
                    .also(&chat.provider),
            );
        }
        let yolo = self
            .view
            .as_ref()
            .is_some_and(|view| view.workspace.permission_mode == YOLO);
        let mode = Rc::new(Cell::new(usize::from(yolo)));
        let choice = Choice {
            options: vec![
                (Icon::Shield, "safe".into()),
                (Icon::ShieldBolt, "yolo".into()),
            ],
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
                    let provider = bench
                        .launchers()
                        .iter()
                        .find(|known| known.id == launcher)
                        .map(|known| known.provider.clone())
                        .unwrap_or_default();
                    let can_skip = kind_of(&provider).is_some_and(|kind| kind["bypass"] == true);
                    let launch_mode = if mode.get() == 1 && can_skip {
                        YOLO
                    } else {
                        SAFE
                    };
                    bench.target().start_chat(
                        launcher.to_owned(),
                        project.clone(),
                        Some(launch_mode.to_owned()),
                        None,
                    );
                } else if let Some(launcher) = key.strip_prefix(SIGN_IN) {
                    sign_in(bench, launcher, window, cx);
                } else if let Some(launcher) = key.strip_prefix(INSTALL) {
                    install(bench, launcher, window, cx);
                } else if let Some(agent) = key.strip_prefix(OPEN) {
                    bench.open_agent(agent);
                } else if let Some(id) = key.strip_prefix(RESUME) {
                    let recent = bench
                        .view
                        .as_ref()
                        .and_then(|view| view.recent.iter().find(|recent| recent.id == id))
                        .cloned();
                    if let Some(recent) = recent {
                        bench.resume_chat(&recent);
                    }
                }
            },
        )
        .empty("no agent matches")
        .choice(choice)
        .on_stage()
        .ruled();
        self.toggle_palette(spec, window, cx);
    }
}

/// Signs the launcher's agent in: the core does it for the agents whose accounts it keeps,
/// and the agent's own command runs in Settings › Agents for the rest.
fn sign_in(
    bench: &mut Workbench,
    launcher: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(provider) = bench
        .launchers()
        .iter()
        .find(|known| known.id == launcher)
        .map(|known| known.provider.clone())
    else {
        return;
    };
    if SIGNS_IN.contains(&provider.as_str()) {
        bench.call("signInLauncher", json!({ "launcher": launcher }));
        return;
    }
    bench.open_settings(crate::settings::Page::Agents, window, cx);
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
        .and_then(|found| crate::agent_setup::signing_in(found, folder.as_deref()));
    if let Some(sheet) = sheet {
        crate::agent_setup::run(bench, sheet, cx);
    }
}

/// Runs the agent's installer in Settings › Agents, as the agent's own sign-in runs there.
fn install(
    bench: &mut Workbench,
    launcher: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(launcher) = bench
        .launchers()
        .iter()
        .find(|known| known.id == launcher)
        .cloned()
    else {
        return;
    };
    let command = bench
        .read("catalog")
        .and_then(Value::as_array)
        .and_then(|kinds| kinds.iter().find(|kind| kind["type"] == launcher.provider))
        .and_then(|kind| kind["install"].as_str())
        .map(str::to_owned);
    let Some(command) = command else {
        return;
    };
    let sheet = crate::agent_setup::installer(&launcher.provider, &launcher.label, command);
    bench.open_settings(crate::settings::Page::Agents, window, cx);
    crate::agent_setup::run(bench, sheet, cx);
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

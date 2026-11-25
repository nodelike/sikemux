//! ⌘⇧N, the agent palette: a new chat with any agent the host has, the agents
//! open in the project, and its pinned and recent chats to take up again.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Context, SharedString, Window};
use sikemux_agents::agent_accounts::by_hand;
use sikemux_calls::agent_sign_in::LauncherParams;
use sikemux_calls::call;
use sikemux_view::device::FoundBy;
use sikemux_view::workspace::LauncherInfo;
use sikemux_wire::protocol::{AgentStatus, PermissionMode};

use crate::icons::Icon;
use crate::palette::{Choice, Mark, Row, Spec};
use crate::workbench::Workbench;

const NEW: &str = "new:";
const SIGN_IN: &str = "signin:";
const INSTALL: &str = "install:";
const OPEN: &str = "open:";
const MODES: [PermissionMode; 3] = [
    PermissionMode::WorkspaceWrite,
    PermissionMode::Auto,
    PermissionMode::Bypass,
];
const RESUME: &str = "resume:";
/// The group of the chats found by what was said in them and all after, ruled off below
/// those found by name or title.
const SAID: &str = "said:";
pub const AGENT_PALETTE: &str = "agent.choose";

impl Workbench {
    pub fn toggle_agent_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project().map(|project| project.id.clone()) else {
            return;
        };
        let rows = self.agent_palette_rows(&project, "");
        let current = self
            .this_view()
            .map_or(PermissionMode::WorkspaceWrite, |view| {
                view.workspace.permission_mode
            });
        let mode = Rc::new(Cell::new(
            MODES.iter().position(|mode| *mode == current).unwrap_or(0),
        ));
        let choice = Choice {
            options: vec![
                (Icon::Lock, t!("safe").into()),
                (Icon::ShieldBolt, t!("auto").into()),
                (Icon::BoltFill, t!("yolo").into()),
            ],
            chosen: mode.clone(),
            armed: Some(2),
        };
        let mut spec = Spec::new(
            AGENT_PALETTE,
            t!("search agents"),
            rows,
            move |row, bench, window, cx| {
                let key: &str = &row.key;
                if let Some(launcher) = key.strip_prefix(NEW) {
                    let provider = launcher_of(bench, launcher)
                        .map(|known| known.provider.clone())
                        .unwrap_or_default();
                    let kind = bench.reads.catalog.kind(&provider);
                    let launch_mode = match MODES.get(mode.get()) {
                        Some(PermissionMode::Bypass) if kind.is_some_and(|kind| kind.bypass) => {
                            PermissionMode::Bypass
                        }
                        Some(PermissionMode::Auto) if kind.is_some_and(|kind| kind.auto) => {
                            PermissionMode::Auto
                        }
                        _ => PermissionMode::WorkspaceWrite,
                    };
                    bench.target().open_new_chat(
                        launcher.to_owned(),
                        project.clone(),
                        Some(launch_mode),
                    );
                } else if let Some(launcher) = key.strip_prefix(SIGN_IN) {
                    sign_in(bench, launcher, window, cx);
                } else if let Some(launcher) = key.strip_prefix(INSTALL) {
                    install(bench, launcher, cx);
                } else if let Some(agent) = key.strip_prefix(OPEN) {
                    bench.open_found_agent(agent, cx);
                } else if let Some(id) = key.strip_prefix(RESUME) {
                    if let Some(recent) = bench.saved_chat(id).cloned() {
                        bench.take_up_chat(&recent, cx);
                    }
                }
            },
        )
        .empty(t!("no agent matches"))
        .choice(choice)
        .on_stage()
        .ruled();
        spec.typing = Some(Rc::new(|bench, _, cx| {
            let limit = match bench.overlays.palette.is_some() {
                true => crate::pickers::recent::PALETTE_PAGE,
                false => crate::pickers::recent::PAGE,
            };
            bench.look_for_older(false, limit, cx);
        }));
        self.toggle_palette(spec, window, cx);
        if self.overlays.palette.is_some() {
            self.look_for_older(true, crate::pickers::recent::PALETTE_PAGE, cx);
        }
    }

    /// Offers the open ⌘⇧N the saved chats that came in since it opened.
    pub fn refill_agent_palette(&mut self, cx: &mut Context<Self>) {
        let open = self
            .overlays
            .palette
            .as_ref()
            .is_some_and(|palette| palette.id() == AGENT_PALETTE);
        let Some(project) = self
            .project()
            .map(|project| project.id.clone())
            .filter(|_| open)
        else {
            return;
        };
        let query = (self.overlays.palette.as_ref())
            .map(|palette| palette.query(cx).trim().to_lowercase())
            .unwrap_or_default();
        let rows = self.agent_palette_rows(&project, &query);
        if let Some(palette) = self.overlays.palette.as_mut() {
            palette.refill(rows, cx);
        }
    }

    /// The palette's rows, those found by what was said in them with where they said `query`.
    fn agent_palette_rows(&self, project: &str, query: &str) -> Vec<Row> {
        let said = |id: Option<&str>, row: Row| match id.and_then(|id| self.said_for(query, id)) {
            Some(said) => row
                .meta(
                    said.runs
                        .iter()
                        .map(|run| run.text.as_str())
                        .collect::<String>(),
                )
                .also(query),
            None => row,
        };
        let kind_of = |provider: &str| self.reads.catalog.kind(provider);
        let mut rows = Vec::new();
        let mut sign_ins = Vec::new();
        let mut one_per_agent: Vec<&LauncherInfo> = Vec::new();
        for launcher in self.launchers() {
            if one_per_agent
                .iter()
                .all(|seen| seen.provider != launcher.provider)
            {
                one_per_agent.push(
                    self.launcher_of_kind(&launcher.provider)
                        .unwrap_or(launcher),
                );
            }
        }
        for launcher in one_per_agent.into_iter().chain(self.terminal_launchers()) {
            let kind = kind_of(&launcher.provider);
            let signs_in = kind.is_some_and(|kind| kind.accounts || kind.login.is_some());
            let bypasses = kind.is_some_and(|kind| kind.bypass);
            let new = || {
                Row::new(
                    format!("{NEW}{}", launcher.id),
                    t!("+ new {agent}", agent = self.agent_name(&launcher.provider)),
                )
                .unarmed(!bypasses)
            };
            let row = match launcher.status.as_deref() {
                Some("missing") if kind.is_some_and(|kind| kind.install.is_some()) => Row::new(
                    format!("{INSTALL}{}", launcher.id),
                    t!("Install {agent}", agent = launcher.label),
                )
                .meta(kind.map(|kind| kind.about.clone()).unwrap_or_default()),
                Some("missing" | "broken") => new()
                    .meta(t!("Agent executable is unavailable"))
                    .disabled(true),
                Some("signedOut") => new().meta(t!("signed out")),
                _ => new(),
            };
            if launcher.status.as_deref() == Some("signedOut") && signs_in {
                sign_ins.push(
                    Row::new(
                        format!("{SIGN_IN}{}", launcher.id),
                        t!("Sign in to {agent}", agent = launcher.label),
                    )
                    .meta(t!("Signed out"))
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
        let unarmed = |provider: &str, terminal: bool| {
            kind_of(provider).is_none_or(|kind| !kind.can_skip_asking(terminal))
        };
        let mut taken_up = Vec::new();
        for agent in self.agents_of(project) {
            let handle = agent.beside();
            let pinned = self.chat_of(&agent.id).is_some_and(|chat| chat.pinned);
            let chat = self.chat_of(&agent.id).map(|chat| chat.id.as_str());
            let Some((place, by)) = self.found_place(query, Some(&agent.id), chat, &agent.title)
            else {
                continue;
            };
            let row = Row::new(format!("{OPEN}{}", agent.id), agent.title.clone())
                .handle(handle.clone().map(SharedString::from))
                .state(agent.turn.clone())
                .unarmed(unarmed(&agent.provider, self.in_terminal(&agent.id)))
                .mark(Mark::Open(Box::new(agent.clone())))
                .also(handle.as_deref().unwrap_or_default())
                .also(&agent.provider);
            let row = said(chat, row);
            taken_up.push((
                (place, !pinned, std::cmp::Reverse(agent.active_at)),
                by,
                row,
            ));
        }
        let saved = self
            .saved_chats(project)
            .into_iter()
            .filter(|recent| recent.agent_id.is_none());
        for chat in saved {
            let Some((place, by)) = self.found_place(query, None, Some(&chat.id), &chat.title)
            else {
                continue;
            };
            let row = Row::new(format!("{RESUME}{}", chat.id), chat.title.clone())
                .meta(if chat.active_at == 0 {
                    String::new()
                } else {
                    crate::ui::time::age(chat.active_at)
                })
                .unarmed(unarmed(&chat.provider, false))
                .mark(Mark::Agent(chat.provider.clone()))
                .also(&chat.provider);
            let row = said(Some(&chat.id), row);
            taken_up.push((
                (place, !chat.pinned, std::cmp::Reverse(chat.active_at)),
                by,
                row,
            ));
        }
        taken_up.sort_by_key(|(key, _, _)| *key);
        let mut said_yet = false;
        rows.extend(taken_up.into_iter().map(|(_, by, row)| {
            said_yet |= by == FoundBy::Said;
            row.group(if said_yet { SAID } else { RESUME })
        }));
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
/// and the agent's own command runs in Settings › Agents for the rest.
fn sign_in(
    bench: &mut Workbench,
    launcher: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(provider) = launcher_of(bench, launcher).map(|known| known.provider.clone()) else {
        return;
    };
    if by_hand(&provider) {
        let account = launcher.split_once(':').map(|(_, id)| id);
        return crate::accounts::sign_in_agent(bench, &provider, account, window, cx);
    }
    // An agent with accounts is signed in by the core; the rest sign in with their own command,
    // in the box over the window.
    if bench
        .reads
        .catalog
        .kind(&provider)
        .is_some_and(|kind| kind.accounts)
    {
        bench
            .target()
            .call_typed::<call::SignInLauncher>(LauncherParams {
                launcher: launcher.to_owned(),
            });
        return;
    }
    let folder = bench
        .accounts
        .current(&provider)
        .and_then(|profile| profile.config_path.clone());
    crate::agents::setup::sign_in(bench, &provider, folder, cx);
}

/// Runs the agent's installer in the box over the window.
fn install(bench: &mut Workbench, launcher: &str, cx: &mut Context<Workbench>) {
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
    crate::agents::setup::run(bench, sheet, cx);
}

pub(crate) fn status_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => t!("working"),
        AgentStatus::Starting => t!("starting"),
        AgentStatus::Blocked => t!("needs you"),
        AgentStatus::Done => t!("done"),
        AgentStatus::Idle => t!("idle"),
        AgentStatus::Asleep => t!("asleep"),
        AgentStatus::Stopped => t!("stopped"),
    }
}

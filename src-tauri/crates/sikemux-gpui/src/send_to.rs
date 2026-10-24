//! Text handed to an agent from elsewhere in the window, such as a terminal's selection or an
//! editor's problem: to one open in the project, or a new chat started for it.

use std::future::Future;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{AppContext, Context, SharedString, Task, WeakEntity, Window};
use sikemux_wire::protocol::{LauncherInfo, PaneKind};
use tokio::sync::oneshot;

use crate::icons::Icon;
use crate::workbench::{shows, Strip, Workbench};

/// An agent text can go to.
#[derive(Clone)]
pub struct Choice {
    pub label: SharedString,
    pub provider: String,
    pub to: Recipient,
}

#[derive(Clone)]
pub enum Recipient {
    Agent(String),
    NewChat { launcher: String, project: String },
}

/// The project's agents, the one in front first and the rest in tab order, then a new chat with
/// each agent whose CLI is installed.
pub fn choices(bench: &Workbench) -> Vec<Choice> {
    let Some(project) = bench.project() else {
        return Vec::new();
    };
    let strip = Strip::Project(project.id.clone());
    let front = bench
        .front_tab(&strip)
        .and_then(|tab| shows(tab, PaneKind::Agent));
    let tabbed = bench
        .tabs(&strip)
        .iter()
        .filter_map(|tab| shows(tab, PaneKind::Agent))
        .filter(|id| Some(*id) != front);
    let mut agents = bench.agents_of(&project.id);
    let open = front.into_iter().chain(tabbed).filter_map(|id| {
        let at = agents.iter().position(|agent| agent.id == *id)?;
        let agent = agents.swap_remove(at);
        Some(Choice {
            label: agent.title.into(),
            provider: agent.provider,
            to: Recipient::Agent(agent.id),
        })
    });
    let installed = |launcher: &&LauncherInfo| {
        !matches!(launcher.status.as_deref(), Some("missing" | "broken"))
    };
    let fresh = bench
        .launchers()
        .iter()
        .filter(installed)
        .map(|launcher| Choice {
            label: format!("New {} chat", launcher.label).into(),
            provider: launcher.provider.clone(),
            to: Recipient::NewChat {
                launcher: launcher.id.clone(),
                project: project.id.clone(),
            },
        });
    open.chain(fresh).collect()
}

/// Puts `text` in the composer of the agent `to` names, starting its chat first when it is a
/// new one, and brings the agent forward. Nothing goes when `text` comes to nothing.
pub fn send(
    bench: &mut Workbench,
    to: Recipient,
    text: impl Future<Output = Option<String>> + 'static,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let agent = match to {
        Recipient::Agent(agent) => Task::ready(Some(agent)),
        Recipient::NewChat { launcher, project } => {
            let (started, agent) = oneshot::channel();
            bench
                .target()
                .start_chat(launcher, project, None, Some(started));
            cx.background_spawn(async move { agent.await.ok() })
        }
    };
    cx.spawn_in(window, async move |bench, cx| {
        let (Some(text), Some(agent)) = (text.await, agent.await) else {
            return;
        };
        let _ = bench.update_in(cx, |bench, window, cx| {
            bench.deliver(&agent, text, window, cx)
        });
    })
    .detach();
}

/// The "Send to Agent…" submenu under `label`: each of the project's choices hands who it names
/// to `chosen`, or one row says there is no agent to take it.
pub fn submenu(
    popup: PopupMenu,
    label: impl Into<SharedString>,
    bench: WeakEntity<Workbench>,
    chosen: impl Fn(&mut Workbench, Recipient, &mut Window, &mut Context<Workbench>) + Clone + 'static,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let choices = bench
        .upgrade()
        .map(|bench| choices(bench.read(cx)))
        .unwrap_or_default();
    popup.submenu_with_icon(
        Some(Icon::Message.into()),
        label,
        window,
        cx,
        move |mut menu, _, _| {
            if choices.is_empty() {
                return menu.item(PopupMenuItem::new("No agents available").disabled(true));
            }
            for choice in &choices {
                let (bench, chosen, to) = (bench.clone(), chosen.clone(), choice.to.clone());
                menu = menu.item(PopupMenuItem::new(choice.label.clone()).on_click(
                    move |_, window, cx| {
                        let _ = bench.update(cx, |bench, cx| chosen(bench, to.clone(), window, cx));
                    },
                ));
            }
            menu
        },
    )
}

/// [`submenu`] for text known as the menu opens, which the chosen agent is sent.
pub fn submenu_with_text(
    popup: PopupMenu,
    label: impl Into<SharedString>,
    bench: WeakEntity<Workbench>,
    text: String,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let chosen =
        move |bench: &mut Workbench, to, window: &mut Window, cx: &mut Context<Workbench>| {
            send(
                bench,
                to,
                std::future::ready(Some(text.clone())),
                window,
                cx,
            )
        };
    submenu(popup, label, bench, chosen, window, cx)
}

/// `text` in a fenced code block whose fence no run of backticks in it can close.
pub fn code_fence(text: &str) -> String {
    let longest = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}\n{}\n{fence}", text.trim_end_matches('\n'))
}

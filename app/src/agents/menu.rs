//! What right-clicking an agent offers, wherever it is shown: its tab, its rail row or its header.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::{App, ClickEvent, ClipboardItem, Context, WeakEntity, Window};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use sikemux_view::workspace::{PinnedChat, WorkspaceAgent};
use sikemux_wire::protocol::PermissionMode;

use crate::agents::rows::AgentRow;
use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::toast;
use crate::workbench::Workbench;
use sikemux_core::server::call;
use sikemux_core::server::calls::AgentParams;
use sikemux_core::server::names::RenameAgentParams;
use sikemux_core::server::workspace::{AgentPatch, PinnedParams, UpdateAgentParams};

/// What a link's parts escape, as a browser's encodeURIComponent does.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// The agent's menu as it was when it was drawn.
pub struct AgentMenu {
    bench: WeakEntity<Workbench>,
    id: String,
    name: Option<String>,
    title: String,
    own_tags: Vec<String>,
    asleep: bool,
    /// The host whose core its calls go to.
    place: Place,
    /// Its record on this host. An agent on another host has none, and its menu leaves out
    /// what works on one: its buddy, pin, project, worktree, permissions, share and link.
    record: Option<WorkspaceAgent>,
    /// Its row, for an agent on another host, which that host closes.
    elsewhere: Option<AgentRow>,
    /// The other agents of its project, which Close Others closes.
    others: Vec<String>,
    /// Whether it can move to another project now, as the Mac's canMoveAgentToProject says.
    can_move: bool,
    pinned: Vec<PinnedChat>,
    can_skip_permissions: bool,
    /// Whether it runs as a chat, which can be shared read-only.
    can_share: bool,
    link: Option<String>,
    /// Its header is on screen, where its title can be edited in place.
    in_front: bool,
    /// Agents wear faces, which it can have one of its own of.
    faces_on: bool,
    snoozed: bool,
    settled: bool,
    /// Every tag on this host's chats, in order.
    tags: Vec<String>,
}

impl AgentMenu {
    pub fn new(bench: &Workbench, agent_id: &str, cx: &Context<Workbench>) -> Option<Self> {
        let workspace = &bench.this_view()?.workspace;
        let agent = workspace
            .agents
            .iter()
            .find(|agent| agent.id == agent_id)?
            .clone();
        let catalog = bench.reads.catalog.kind(&agent.provider);
        let row = bench.agent_by_id(agent_id);
        let project = workspace
            .projects
            .iter()
            .find(|project| project.id == agent.project);
        let link = agent
            .resume_id
            .as_ref()
            .zip(project)
            .map(|(resume, project)| {
                format!(
                    "sikemux://agent/{}/{}?project={}",
                    agent.provider,
                    utf8_percent_encode(resume, COMPONENT),
                    utf8_percent_encode(&project.path.to_string_lossy(), COMPONENT)
                )
            });
        Some(Self {
            bench: cx.entity().downgrade(),
            others: workspace
                .agents
                .iter()
                .filter(|other| other.project == agent.project && other.id != agent.id)
                .map(|other| other.id.clone())
                .collect(),
            can_move: agent.worktree.is_none()
                && (agent.resume_id.is_none()
                    || catalog.is_some_and(|kind| kind.relocatable_sessions)
                        && (agent.asleep
                            || row.as_ref().is_none_or(|row| {
                                !matches!(
                                    row.status,
                                    sikemux_wire::protocol::AgentStatus::Working
                                        | sikemux_wire::protocol::AgentStatus::Blocked
                                ) && row.tasks + row.subagents == 0
                            }))),
            pinned: workspace.pinned.clone(),
            can_skip_permissions: catalog
                .is_some_and(|kind| kind.can_skip_asking(bench.in_terminal(&agent.id))),
            can_share: catalog.is_some_and(|kind| kind.chat.is_some()),
            link,
            in_front: bench.agent().is_some_and(|front| front.id == agent.id),
            faces_on: bench.shown().is_some_and(|view| view.face_style.on),
            snoozed: row.as_ref().is_some_and(|row| row.snoozed_until.is_some()),
            settled: row.as_ref().is_some_and(|row| row.settled_at.is_some()),
            tags: (workspace.agents.iter())
                .flat_map(|agent| agent.tags.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            id: agent.id.clone(),
            name: agent.name.clone(),
            title: agent.title.clone(),
            own_tags: agent.tags.clone(),
            asleep: agent.asleep,
            place: Place::This,
            record: Some(agent),
            elsewhere: None,
        })
    }

    /// The menu of `row`, wherever its agent runs: the whole menu for one on the host in
    /// front, and for one on another host what that host does to it from its row.
    pub fn of_row(bench: &Workbench, row: &AgentRow, cx: &Context<Workbench>) -> Option<Self> {
        if row.place == bench.place() {
            return Self::new(bench, &row.id, cx);
        }
        let place = bench.asked_at(row);
        let in_use = (bench.view_at(&place).into_iter())
            .flat_map(|view| &view.workspace.agents)
            .flat_map(|agent| agent.tags.iter().cloned());
        Some(Self {
            bench: cx.entity().downgrade(),
            id: row.id.clone(),
            name: row.name.clone(),
            title: row.handle().map_or_else(String::new, |_| row.title.clone()),
            own_tags: row.tags.clone(),
            asleep: row.status == sikemux_wire::protocol::AgentStatus::Asleep,
            others: Vec::new(),
            can_move: false,
            pinned: Vec::new(),
            can_skip_permissions: false,
            can_share: false,
            link: None,
            in_front: false,
            faces_on: false,
            snoozed: row.snoozed_until.is_some(),
            settled: row.settled_at.is_some(),
            tags: in_use
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            place,
            record: None,
            elsewhere: Some(row.clone()),
        })
    }

    pub fn build(
        &self,
        mut popup: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let id = self.id.clone();
        let record = self.record.as_ref();
        let asked = self.place.clone();
        let (bench, renamed, name, place) = (
            self.bench.clone(),
            id.clone(),
            self.name.clone().unwrap_or_default(),
            asked.clone(),
        );
        popup = popup.item(
            PopupMenuItem::new(t!("Rename…"))
                .icon(Icon::Pencil)
                .on_click(move |_, _, cx| {
                    let (renamed, name, place) = (renamed.clone(), name.clone(), place.clone());
                    let _ = bench.update(cx, |bench, cx| {
                        bench.prompt(
                            t!("Rename agent"),
                            name.clone(),
                            move |bench, typed, _, _| {
                                if !typed.is_empty() && typed != name {
                                    Target::new(&bench.asks, place.clone())
                                        .call_typed::<call::RenameAgent>(RenameAgentParams {
                                            agent_id: renamed.clone(),
                                            name: typed.to_owned(),
                                        });
                                }
                            },
                            cx,
                        )
                    });
                }),
        );
        let (bench, retitled, title, in_front, place) = (
            self.bench.clone(),
            id.clone(),
            self.title.clone(),
            self.in_front,
            asked.clone(),
        );
        popup = popup.item(
            PopupMenuItem::new(t!("Edit Title…"))
                .icon(Icon::Pencil)
                .on_click(move |_, window, cx| {
                    let (bench, retitled, title) = (bench.clone(), retitled.clone(), title.clone());
                    let place = place.clone();
                    if in_front {
                        window.defer(cx, move |window, cx| {
                            let _ = bench.update(cx, |bench, cx| {
                                bench.retitle(&retitled, false, window, cx)
                            });
                        });
                        return;
                    }
                    let _ = bench.update(cx, |bench, cx| {
                        bench.prompt(
                            t!("Edit title"),
                            title.clone(),
                            move |bench, typed, _, _| {
                                if !typed.is_empty() && typed != title {
                                    Target::new(&bench.asks, place.clone())
                                        .call_typed::<call::UpdateAgent>(UpdateAgentParams::new(
                                            retitled.clone(),
                                            AgentPatch {
                                                title: Some(typed.to_owned()),
                                                renamed: Some(true),
                                                ..AgentPatch::default()
                                            },
                                        ));
                                }
                            },
                            cx,
                        )
                    });
                }),
        );
        if self.faces_on && record.is_some() {
            let (bench, picked) = (self.bench.clone(), id.clone());
            popup = popup.item(
                PopupMenuItem::new(t!("Change Buddy…"))
                    .icon(Icon::User)
                    .on_click(move |_, _, cx| {
                        let _ = bench.update(cx, |bench, cx| bench.open_buddy_picker(&picked, cx));
                    }),
            );
        }
        let resumed = record.and_then(|agent| Some((agent, agent.resume_id.as_ref()?)));
        if let Some((agent, resume)) = resumed {
            let pin = PinnedChat {
                provider: agent.provider.clone(),
                session_id: resume.clone(),
                title: agent.title.clone(),
                project: agent.cwd.clone(),
            };
            let is_pin = |kept: &PinnedChat| {
                kept.provider == pin.provider && kept.session_id == pin.session_id
            };
            let pinned = self.pinned.iter().any(is_pin);
            let mut chats: Vec<PinnedChat> = self
                .pinned
                .iter()
                .filter(|kept| !is_pin(kept))
                .cloned()
                .collect();
            if !pinned {
                chats.push(pin.clone());
            }
            popup = popup.item(
                PopupMenuItem::new(if pinned { t!("Unpin") } else { t!("Pin") })
                    .icon(Icon::Pin)
                    .on_click(self.on_bench(move |bench, _| {
                        Target::here(&bench.asks).call_typed::<call::SetPinnedChats>(PinnedParams {
                            chats: chats.clone(),
                        })
                    })),
            );
        }
        popup = crate::agents::tags::menu_item(
            popup,
            self.bench.clone(),
            asked.clone(),
            &id,
            self.own_tags.clone(),
            self.tags.clone(),
            window,
            cx,
        );
        popup = crate::agents::settled::menu_items(
            popup,
            self.bench.clone(),
            asked.clone(),
            &id,
            self.snoozed,
            self.settled,
            window,
            cx,
        );
        if self.asleep {
            let (row, place, local) = (id.clone(), asked.clone(), record.is_some());
            popup = popup.item(
                PopupMenuItem::new(t!("Resume"))
                    .icon(Icon::Resume)
                    .on_click(self.on_bench(move |bench, cx| match local {
                        true => bench.open_agent(&row, cx),
                        false => {
                            Target::new(&bench.asks, place.clone()).call_typed::<call::ReopenAgent>(
                                AgentParams {
                                    agent_id: row.clone(),
                                },
                            )
                        }
                    })),
            );
        } else if record.is_none_or(|agent| agent.resume_id.is_some()) {
            popup = popup.item(PopupMenuItem::new(t!("Sleep")).icon(Icon::Power).on_click(
                self.on_bench({
                    let (id, place) = (id.clone(), asked.clone());
                    move |bench, _| {
                        Target::new(&bench.asks, place.clone()).call_typed::<call::SleepAgent>(
                            AgentParams {
                                agent_id: id.clone(),
                            },
                        )
                    }
                }),
            ));
        }
        if let Some(agent) = record.filter(|agent| !agent.asleep && agent.resume_id.is_some()) {
            popup = popup.item(
                PopupMenuItem::new(if agent.keep_alive {
                    t!("Allow Auto-Sleep")
                } else {
                    t!("Keep Alive")
                })
                .icon(Icon::Pin)
                .on_click({
                    let (id, keep_alive) = (id.clone(), !agent.keep_alive);
                    self.on_bench(move |bench, _| {
                        Target::here(&bench.asks).call_typed::<call::UpdateAgent>(
                            UpdateAgentParams::new(
                                id.clone(),
                                AgentPatch {
                                    keep_alive: Some(keep_alive),
                                    ..AgentPatch::default()
                                },
                            ),
                        )
                    })
                }),
            );
        }
        if record.is_some() {
            let (bench, moved) = (self.bench.clone(), id.clone());
            popup = popup.item(
                PopupMenuItem::new(t!("Move to Project…"))
                    .icon(Icon::Folder)
                    .disabled(!self.can_move)
                    .on_click(move |_, window, cx| {
                        let (bench, moved) = (bench.clone(), moved.clone());
                        window.defer(cx, move |window, cx| {
                            let _ = bench
                                .update(cx, |bench, cx| bench.pick_project_for(&moved, window, cx));
                        });
                    }),
            );
        }
        if record.is_some_and(|agent| agent.worktree.is_some()) {
            let (bench, removed) = (self.bench.clone(), id.clone());
            popup = popup.separator().item(
                PopupMenuItem::new(t!("Remove worktree…"))
                    .icon(Icon::Trash)
                    .on_click(move |_, window, cx| {
                        let _ = bench.update(cx, |bench, cx| {
                            bench.remove_worktree(&removed, false, window, cx)
                        });
                    }),
            );
        }
        let Some(agent) = record else {
            let row = self.elsewhere.clone();
            return popup.separator().item(
                PopupMenuItem::new(t!("Close"))
                    .icon(Icon::Close)
                    .on_click(self.on_bench(move |bench, cx| {
                        if let Some(row) = &row {
                            bench.close_elsewhere(row, cx);
                        }
                    })),
            );
        };
        popup = popup
            .separator()
            .item(
                PopupMenuItem::new(t!("Close"))
                    .icon(Icon::Close)
                    .action(Box::new(crate::workbench::actions::CloseTab))
                    .on_click(self.on_bench({
                        let id = id.clone();
                        move |bench, cx| bench.close_agents(vec![id.clone()], cx)
                    })),
            )
            .item(
                PopupMenuItem::new(t!("Close Others"))
                    .icon(Icon::Close)
                    .disabled(self.others.is_empty())
                    .on_click(self.on_bench({
                        let others = self.others.clone();
                        move |bench, cx| bench.close_agents(others.clone(), cx)
                    })),
            );
        if self.can_skip_permissions {
            let yolo = agent.permission_mode == PermissionMode::Bypass;
            let mode = if yolo {
                PermissionMode::WorkspaceWrite
            } else {
                PermissionMode::Bypass
            };
            popup = popup.separator().item(
                PopupMenuItem::new(if yolo {
                    t!("Disable YOLO Mode")
                } else {
                    t!("Enable YOLO Mode")
                })
                .icon(if yolo { Icon::Lock } else { Icon::BoltFill })
                .on_click(self.on_bench({
                    let id = id.clone();
                    move |bench, _| bench.switch_mode(&id, mode)
                })),
            );
        }
        popup = popup.separator();
        if self.can_share {
            let (bench, shared) = (self.bench.clone(), id.clone());
            popup = popup.item(
                PopupMenuItem::new(t!("Share Read-Only…"))
                    .icon(Icon::Share)
                    .on_click(move |_, window, cx| {
                        let (bench, shared) = (bench.clone(), shared.clone());
                        window.defer(cx, move |window, cx| {
                            let _ =
                                bench.update(cx, |bench, cx| bench.open_share(&shared, window, cx));
                        });
                    }),
            );
        }
        let link = self.link.clone();
        popup.item(
            PopupMenuItem::new(t!("Copy Link"))
                .icon(Icon::Copy)
                .disabled(link.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(link) = &link {
                        cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                        window.push_notification(toast::success(t!("Copied link")), cx);
                    }
                }),
        )
    }

    /// A menu item's click that works on the bench.
    fn on_bench(
        &self,
        work: impl Fn(&mut Workbench, &mut Context<Workbench>) + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let bench = self.bench.clone();
        move |_, _, cx| {
            let _ = bench.update(cx, |bench, cx| work(bench, cx));
        }
    }
}

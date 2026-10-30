//! What right-clicking an agent offers, wherever it is shown: its tab, its rail row or its header.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::{App, ClickEvent, ClipboardItem, Context, WeakEntity, Window};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use serde_json::{json, Value};
use sikemux_wire::protocol::{PermissionMode, PinnedChat, WorkspaceAgent};

use crate::icons::Icon;
use crate::toast;
use crate::workbench::Workbench;

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
    agent: WorkspaceAgent,
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
}

impl AgentMenu {
    pub fn new(bench: &Workbench, agent_id: &str, cx: &Context<Workbench>) -> Option<Self> {
        let workspace = &bench.view.as_ref()?.workspace;
        let agent = workspace
            .agents
            .iter()
            .find(|agent| agent.id == agent_id)?
            .clone();
        let catalog = bench.reads.catalog.kind(&agent.provider);
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
                            || bench.agent_by_id(&agent.id).is_none_or(|row| {
                                !matches!(
                                    row.status,
                                    sikemux_wire::protocol::AgentStatus::Working
                                        | sikemux_wire::protocol::AgentStatus::Blocked
                                ) && row.tasks + row.subagents == 0
                            }))),
            pinned: workspace.pinned.clone(),
            can_skip_permissions: catalog.is_some_and(|kind| kind.bypass),
            can_share: catalog.is_some_and(|kind| kind.chat.is_some()),
            link,
            in_front: bench.agent().is_some_and(|front| front.id == agent.id),
            faces_on: bench.shown().is_some_and(|view| view.face_style.on),
            agent,
        })
    }

    pub fn build(&self, mut popup: PopupMenu) -> PopupMenu {
        let agent = &self.agent;
        let id = agent.id.clone();
        let (bench, renamed, name) = (
            self.bench.clone(),
            id.clone(),
            agent.name.clone().unwrap_or_default(),
        );
        popup = popup.item(PopupMenuItem::new("Rename…").icon(Icon::Pencil).on_click(
            move |_, _, cx| {
                let (renamed, name) = (renamed.clone(), name.clone());
                let _ = bench.update(cx, |bench, cx| {
                    bench.prompt(
                        "Rename agent",
                        name.clone(),
                        move |bench, typed, _, _| {
                            if !typed.is_empty() && typed != name {
                                bench.call(
                                    "renameAgent",
                                    json!({ "agentId": renamed, "name": typed }),
                                );
                            }
                        },
                        cx,
                    )
                });
            },
        ));
        let (bench, retitled, title, in_front) = (
            self.bench.clone(),
            id.clone(),
            agent.title.clone(),
            self.in_front,
        );
        popup = popup
            .item(
                PopupMenuItem::new("Edit Title…")
                    .icon(Icon::Pencil)
                    .on_click(move |_, window, cx| {
                        let (bench, retitled, title) =
                            (bench.clone(), retitled.clone(), title.clone());
                        if in_front {
                            window.defer(cx, move |window, cx| {
                                let _ = bench
                                    .update(cx, |bench, cx| bench.retitle(&retitled, false, window, cx));
                            });
                            return;
                        }
                        let _ = bench.update(cx, |bench, cx| {
                            bench.prompt(
                                "Edit title",
                                title.clone(),
                                move |bench, typed, _, _| {
                                    if !typed.is_empty() && typed != title {
                                        bench.call(
                                            "updateAgent",
                                            json!({ "agentId": retitled, "patch": { "title": typed, "renamed": true } }),
                                        );
                                    }
                                },
                                cx,
                            )
                        });
                    }),
            );
        if self.faces_on {
            let (bench, picked) = (self.bench.clone(), id.clone());
            popup = popup.item(
                PopupMenuItem::new("Change Buddy…")
                    .icon(Icon::User)
                    .on_click(move |_, _, cx| {
                        let _ = bench.update(cx, |bench, cx| bench.open_buddy_picker(&picked, cx));
                    }),
            );
        }
        if let Some(resume) = &agent.resume_id {
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
                PopupMenuItem::new(if pinned { "Unpin" } else { "Pin" })
                    .icon(Icon::Pin)
                    .on_click(self.call("setPinnedChats", json!({ "chats": chats }))),
            );
        }
        if agent.asleep {
            let (bench, row) = (self.bench.clone(), id.clone());
            popup = popup.item(PopupMenuItem::new("Resume").icon(Icon::Rotate).on_click(
                move |_, _, cx| {
                    let _ = bench.update(cx, |bench, _| bench.open_agent(&row));
                },
            ));
        } else if agent.resume_id.is_some() {
            popup = popup
                .item(
                    PopupMenuItem::new("Sleep")
                        .icon(Icon::Power)
                        .on_click(self.call("sleepAgent", json!({ "agentId": id }))),
                )
                .item(
                    PopupMenuItem::new(if agent.keep_alive {
                        "Allow Auto-Sleep"
                    } else {
                        "Keep Alive"
                    })
                    .icon(Icon::Pin)
                    .on_click(self.call(
                        "updateAgent",
                        json!({ "agentId": id, "patch": { "keepAlive": !agent.keep_alive } }),
                    )),
                );
        }
        let (bench, moved) = (self.bench.clone(), id.clone());
        popup = popup.item(
            PopupMenuItem::new("Move to Project…")
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
        if agent.worktree.is_some() {
            let (bench, removed) = (self.bench.clone(), id.clone());
            popup = popup.separator().item(
                PopupMenuItem::new("Remove worktree…")
                    .icon(Icon::Trash)
                    .on_click(move |_, window, cx| {
                        let _ = bench.update(cx, |bench, cx| {
                            bench.remove_worktree(&removed, false, window, cx)
                        });
                    }),
            );
        }
        popup = popup
            .separator()
            .item(
                PopupMenuItem::new("Close")
                    .icon(Icon::Close)
                    .action(Box::new(crate::keymap::CloseTab))
                    .on_click(self.on_bench({
                        let id = id.clone();
                        move |bench, cx| bench.close_agents(vec![id.clone()], cx)
                    })),
            )
            .item(
                PopupMenuItem::new("Close Others")
                    .icon(Icon::Close)
                    .disabled(self.others.is_empty())
                    .on_click(self.on_bench({
                        let others = self.others.clone();
                        move |bench, cx| bench.close_agents(others.clone(), cx)
                    })),
            );
        if self.can_skip_permissions {
            let yolo = agent.permission_mode == "bypass";
            let mode = if yolo {
                PermissionMode::WorkspaceWrite
            } else {
                PermissionMode::Bypass
            };
            popup = popup.separator().item(
                PopupMenuItem::new(if yolo {
                    "Disable YOLO Mode"
                } else {
                    "Enable YOLO Mode"
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
                PopupMenuItem::new("Share Read-Only…")
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
            PopupMenuItem::new("Copy Link")
                .icon(Icon::Copy)
                .disabled(link.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(link) = &link {
                        cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                        window.push_notification(toast::success("Copied link"), cx);
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

    /// A menu item's click that makes a core call.
    fn call(
        &self,
        method: &'static str,
        params: Value,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static {
        let bench = self.bench.clone();
        move |_, _, cx| {
            let params = params.clone();
            let _ = bench.update(cx, |bench, _| bench.call(method, params));
        }
    }
}

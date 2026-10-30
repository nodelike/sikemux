//! The host the person is on (ADR 0018): this one, another host on the account, or an SSH host
//! without Sikemux. The rail, the top bar and the tab list follow the one chosen.

use std::path::Path;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::{App, Context, SharedString};
use serde_json::{json, Value};
use sikemux_core::server::{call, ApprovalSetting, NoParams};

use crate::core;
use crate::icons::Icon;
use crate::toast;
use crate::workbench::{Strip, Workbench};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum HostChoice {
    #[default]
    This,
    /// Another host on the account, by its core's key.
    Account(String),
    /// An alias from ~/.ssh/config that no host on the account was installed on.
    Ssh(String),
}

impl HostChoice {
    /// The same across launches: "this", "host:<key>" or "ssh:<alias>".
    pub fn key(&self) -> SharedString {
        match self {
            HostChoice::This => "this".into(),
            HostChoice::Account(key) => format!("host:{key}").into(),
            HostChoice::Ssh(alias) => format!("ssh:{alias}").into(),
        }
    }
}

/// Whose core a pane talks to: this host's, or another host's on the account. An SSH host
/// without Sikemux has no core, so its strip is this host's.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum Place {
    #[default]
    This,
    Account(String),
}

/// The account's other hosts as this app knows them.
#[derive(Default)]
pub struct Hosts {
    /// The account's other hosts this app reaches, by key: each one's view, as its core sends it.
    pub(crate) views: std::collections::HashMap<String, sikemux_wire::protocol::DeviceView>,
    /// The account's other hosts this app is connected to now, by key.
    pub(crate) reached: std::collections::HashSet<String>,
    /// The home folders of the account's other hosts, by the host's key.
    pub(crate) homes: std::collections::HashMap<String, String>,
    /// Whether each host asks the person before a new device joins, by key, none for this one.
    pub(crate) approvals: std::collections::HashMap<Option<String>, bool>,
    /// Why this app reaches none of the account's other hosts, when it cannot.
    pub(crate) unavailable: Option<String>,
}

impl Hosts {
    /// Forgets what it knew of the hosts no longer linked to the account, as `listed` says.
    pub fn keep_linked(&mut self, listed: &Value) {
        let linked = |id: &String| {
            listed
                .as_array()
                .into_iter()
                .flatten()
                .any(|host| host["id"] == id.as_str())
        };
        self.views.retain(|id, _| linked(id));
        self.reached.retain(linked);
        self.homes.retain(|id, _| linked(id));
        self.approvals
            .retain(|id, _| id.as_ref().is_none_or(linked));
    }
}

pub struct HostOption {
    pub choice: HostChoice,
    pub label: SharedString,
}

impl Workbench {
    /// This host, then the account's hosts in the order they were linked, then the SSH hosts
    /// no account host was installed on, as the core lists them.
    pub fn host_choices(&self) -> Vec<HostOption> {
        let listed = self
            .reads
            .hosts
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|host| {
                let choice = match (host["id"].as_str(), host["ssh"].as_str()) {
                    (Some(key), _) => HostChoice::Account(key.to_owned()),
                    (None, Some(alias)) => HostChoice::Ssh(alias.to_owned()),
                    (None, None) => return None,
                };
                let label = host["name"].as_str().unwrap_or_default().to_owned();
                Some(HostOption {
                    choice,
                    label: label.into(),
                })
            });
        std::iter::once(HostOption {
            choice: HostChoice::This,
            label: "This host".into(),
        })
        .chain(listed)
        .collect()
    }

    /// The name of the account host `host`, as the core lists it.
    pub fn host_name(&self, host: &str) -> SharedString {
        self.host_choices()
            .into_iter()
            .find(|option| option.choice == HostChoice::Account(host.to_owned()))
            .map_or_else(|| "another host".into(), |option| option.label)
    }

    /// The host in front, as the core's layout says.
    pub fn chosen_host(&self) -> HostChoice {
        if let Some(key) = self
            .view
            .as_ref()
            .and_then(|view| view.layout.chosen_host.clone())
        {
            return HostChoice::Account(key);
        }
        match self.strip() {
            Some(Strip::Host(alias)) => HostChoice::Ssh(alias),
            _ => HostChoice::This,
        }
    }

    /// The home folder of the host at `place`, once its core has said it.
    pub fn home_at(&self, place: &Place) -> Option<String> {
        match place {
            Place::This => crate::ui::path::this_home(),
            Place::Account(host) => self.hosts.homes.get(host).cloned(),
        }
    }

    /// Whose core the panes in front talk to.
    pub fn place(&self) -> Place {
        match self.chosen_host() {
            HostChoice::Account(key) => Place::Account(key),
            _ => Place::This,
        }
    }

    /// Opens the project at `path` on the account host `host`, once the person agrees: that
    /// host clones it from its remote or takes it from this host as a bundle, then the person is
    /// on that host, in that project. Only committed work goes.
    pub fn open_on(
        &mut self,
        path: &Path,
        host: &str,
        label: SharedString,
        after: HostChoice,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (path, host) = (path.to_owned(), host.to_owned());
        self.confirm(
            format!("Open {name} on {label}?"),
            format!("{label} clones it from its remote, or takes it from where it is when it cannot reach the remote. Only what is committed goes over; uncommitted changes stay there."),
            "Open",
            false,
            move |bench, yes, _, cx| {
                if yes {
                    bench.open_on_now(&path, &host, &name, label.clone(), after.clone(), cx);
                }
            },
            cx,
        );
    }

    fn open_on_now(
        &mut self,
        path: &Path,
        host: &str,
        name: &str,
        label: SharedString,
        after: HostChoice,
        cx: &mut Context<Self>,
    ) {
        let params = json!({ "path": path, "host": host });
        // The project's own host sends it, so the call goes to the host in front.
        let opening = self.target().request("openOnHost", params);
        self.notify(toast::info(format!("Opening {name} on {label}…")), cx);
        cx.spawn(async move |bench, cx| {
            let opened = opening.await;
            let _ = bench.update(cx, |bench, cx| match opened {
                Ok(opened) => {
                    let place = match &after {
                        HostChoice::Account(key) => Place::Account(key.clone()),
                        _ => Place::This,
                    };
                    bench.choose_host(after, cx);
                    if let Some(project) = opened["project"].as_str() {
                        core::Target::new(&bench.asks, place)
                            .call("activateProject", json!({ "project": project }));
                    }
                }
                Err(error) => bench.notify(
                    toast::error(format!("It did not open on {label}: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Sends the project `project` of the host in front to `host`, with its chats and tabs and,
    /// when asked, its uncommitted changes. A move then removes the chats here, once that host
    /// has every one. The person is then on that host, in that project.
    pub fn send_session(&mut self, sending: Sending, cx: &mut Context<Self>) {
        let params = json!({
            "project": sending.project,
            "host": sending.host,
            "move": sending.moving,
            "uncommitted": sending.uncommitted,
        });
        // The project's own host sends it, so the call goes to the host in front.
        let asked = self.target().request("sendSession", params);
        let verb = if sending.moving { "Moving" } else { "Copying" };
        crate::settings::session_sent(self, None, cx);
        self.notify(
            toast::info(format!("{verb} {} to {}…", sending.name, sending.label)),
            cx,
        );
        cx.spawn(async move |bench, cx| {
            let answer = asked.await;
            let _ = bench.update(cx, |bench, cx| {
                let said = match &answer {
                    Ok(sent) => sent_summary(sent, &sending),
                    Err(error) => format!("It did not go to {}: {error}", sending.label),
                };
                match &answer {
                    Ok(_) => bench.notify(toast::success(said.clone()), cx),
                    Err(_) => bench.notify(toast::error(said.clone()), cx),
                }
                crate::settings::session_sent(bench, Some(said.into()), cx);
                let Ok(sent) = answer else {
                    return;
                };
                let place = match &sending.after {
                    HostChoice::Account(key) => Place::Account(key.clone()),
                    _ => Place::This,
                };
                bench.choose_host(sending.after.clone(), cx);
                if let Some(project) = sent["project"].as_str() {
                    core::Target::new(&bench.asks, place)
                        .call("activateProject", json!({ "project": project }));
                }
            });
        })
        .detach();
    }

    /// Sets Sikemux up on the SSH host `alias` as `set_up` says, over the person's own ssh in a
    /// terminal of its strip, signing it in to the account where that takes a code; its row
    /// says how it goes.
    pub fn set_up_host(
        &mut self,
        alias: &str,
        set_up: crate::install_pane::SetUp,
        cx: &mut Context<Self>,
    ) {
        let (asks, alias) = (self.asks.clone(), alias.to_owned());
        let params = json!({ "alias": alias, "work": set_up.work() });
        cx.spawn(async move |bench, cx| {
            if let Err(error) = core::request(&asks, "installHost", params).await {
                let _ = bench.update(cx, |bench, cx| {
                    bench.notify(
                        toast::error(format!("{}: {error}", set_up.offer(&alias))),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// Reads whether `host`, this one when none, asks the person before a new device joins.
    pub fn read_approval(&self, host: Option<String>, cx: &mut Context<Self>) {
        let place = host.clone().map_or(Place::This, Place::Account);
        let asked = core::Target::new(&self.asks, place)
            .request_typed::<call::RequireApproval>(NoParams {});
        cx.spawn(async move |bench, cx| {
            let Ok(read) = asked.await else {
                return;
            };
            let _ = bench.update(cx, |bench, cx| {
                bench.hosts.approvals.insert(host, read.require_approval);
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks the person before a new device joins `host`, this one when none, or stops asking.
    pub fn flip_approval(&self, host: Option<String>, cx: &mut Context<Self>) {
        let Some(on) = self.hosts.approvals.get(&host).copied() else {
            return;
        };
        let place = host.clone().map_or(Place::This, Place::Account);
        let set = core::Target::new(&self.asks, place).request_typed::<call::SetRequireApproval>(
            ApprovalSetting {
                require_approval: !on,
            },
        );
        cx.spawn(async move |bench, cx| {
            let set = set.await;
            let _ = bench.update(cx, |bench, cx| match set {
                Ok(_) => bench.read_approval(host, cx),
                Err(error) => bench.notify(
                    toast::error(format!("Require approval for new devices: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Turns the account host `host`'s own updates on or off; its view then says which.
    pub fn set_host_updates(&self, host: String, on: bool, cx: &mut Context<Self>) {
        let set = core::Target::new(&self.asks, Place::Account(host))
            .request("setHostUpdates", json!({ "on": on }));
        cx.spawn(async move |bench, cx| {
            if let Err(error) = set.await {
                let _ = bench.update(cx, |bench, cx| {
                    bench.notify(toast::error(format!("Update automatically: {error}")), cx)
                });
            }
        })
        .detach();
    }

    /// A new chat on the account host `host`, in the project in front there, with the agent the
    /// rail is set to, and the person moved there to see it.
    pub fn new_chat_on(&mut self, host: &str, cx: &mut Context<Self>) {
        let Some(view) = self.hosts.views.get(host) else {
            return;
        };
        let projects = &view.workspace.projects;
        let Some(project) = view
            .layout
            .active_project
            .as_deref()
            .and_then(|id| projects.iter().find(|project| project.id == id))
            .or(projects.first())
        else {
            return;
        };
        let provider = self.launcher().map(|launcher| launcher.provider.clone());
        let launchers = &view.workspace.launchers;
        let Some(launcher) = launchers
            .iter()
            .find(|launcher| Some(&launcher.provider) == provider.as_ref())
            .or(launchers.first())
        else {
            return;
        };
        let (launcher, project) = (launcher.id.clone(), project.id.clone());
        core::Target::new(&self.asks, Place::Account(host.to_owned()))
            .start_chat(launcher, project, None, None);
        self.choose_host(HostChoice::Account(host.to_owned()), cx);
    }

    /// A terminal of no project on the account host `host`, and the person moved there.
    pub fn new_terminal_on(&mut self, host: &str, cx: &mut Context<Self>) {
        self.take_keys_when_shown(crate::workbench::TakesKeys::Terminal, cx);
        core::Target::new(&self.asks, Place::Account(host.to_owned())).call(
            "newTerminal",
            crate::workbench::Strip::Terminals.with(json!({})),
        );
        self.choose_host(HostChoice::Account(host.to_owned()), cx);
    }

    /// Renames the account host `host` for every device and host on the account.
    pub fn rename_host(&mut self, host: &str, name: &str, cx: &mut Context<Self>) {
        let key = host.to_owned();
        self.prompt(
            format!("Rename {name}"),
            name.to_owned(),
            move |bench, typed, _, _| {
                if !typed.trim().is_empty() {
                    bench.call(
                        "renameAccountDevice",
                        json!({ "key": key, "name": typed.trim() }),
                    );
                }
            },
            cx,
        );
    }

    /// Takes the account host `host` off the account, once the person agrees.
    pub fn remove_host(&mut self, host: &str, name: &str, cx: &mut Context<Self>) {
        let key = host.to_owned();
        self.confirm(
            format!("Remove {name} from your account?"),
            "Every device and host on your account stops reaching it at once. To add it again, sign it in again.",
            "Remove",
            true,
            move |bench, yes, _, _| {
                if yes {
                    bench.call("removeAccountDevice", json!({ "key": key }));
                }
            },
            cx,
        );
    }

    /// Opens ~/.ssh/config on this host in the editor, at the `Host` line of `alias` when given.
    pub fn edit_ssh_config(&mut self, alias: Option<String>, cx: &mut Context<Self>) {
        let file = core::request(&self.asks, "sshConfigFile", json!({}));
        let hosts = alias
            .is_some()
            .then(|| core::request(&self.asks, "sshHosts", json!({})));
        cx.spawn(async move |bench, cx| {
            let file = file.await;
            let line = match hosts {
                Some(hosts) => hosts.await.ok().and_then(|hosts| {
                    let listed = hosts
                        .as_array()?
                        .iter()
                        .find(|host| host["alias"].as_str() == alias.as_deref())?;
                    u32::try_from(listed["line"].as_u64()?).ok()
                }),
                None => None,
            };
            let _ = bench.update(cx, |bench, cx| match file {
                Ok(path) => {
                    bench.choose_host(HostChoice::This, cx);
                    let at = crate::editor::OpenAt {
                        path: path.as_str().unwrap_or_default().into(),
                        line: line.unwrap_or(0),
                        column: None,
                    };
                    crate::editor::go_in(bench, at, &Strip::Terminals, "SSH config", cx);
                }
                Err(error) => bench.notify(
                    toast::error(format!("~/.ssh/config did not open: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Says a device or host joined the account, with a way to take it off again (ADR 0018).
    pub fn new_account_device(&mut self, key: String, name: String, cx: &mut Context<Self>) {
        let name = if name.is_empty() {
            "a device".to_owned()
        } else {
            name
        };
        let asks = self.asks.clone();
        let note =
            toast::info(format!("New device on your account: {name}")).action(move |_, _, cx| {
                let (note, asks, key, name) =
                    (cx.entity(), asks.clone(), key.clone(), name.clone());
                gpui_kit::component::button::Button::new("remove-account-device")
                    .label("Remove")
                    .on_click(move |_, window, cx| {
                        let removed =
                            core::request(&asks, "removeAccountDevice", json!({ "key": key }));
                        let name = name.clone();
                        window
                            .spawn(cx, async move |cx| {
                                if let Err(error) = removed.await {
                                    let _ = cx.update(|window, cx| {
                                        window.push_notification(
                                            toast::error(format!("Remove {name}: {error}")),
                                            cx,
                                        )
                                    });
                                }
                            })
                            .detach();
                        note.update(cx, |note, cx| note.dismiss(window, cx));
                    })
            });
        self.notify(note, cx);
    }

    /// Every other Sikemux host a project in front could go to, this one while another is in
    /// front: its key, its label, whether this app reaches it, and the choice that puts the
    /// person there.
    pub fn other_hosts(&self) -> Vec<(String, SharedString, bool, HostChoice)> {
        let shown = self.chosen_host();
        let own_key = self.remote.as_ref().map(|remote| remote.core_id.clone());
        self.host_choices()
            .into_iter()
            .filter(|option| option.choice != shown)
            .filter_map(|option| match option.choice {
                HostChoice::This => Some((own_key.clone()?, option.label, true, HostChoice::This)),
                HostChoice::Account(key) => Some((
                    key.clone(),
                    option.label,
                    self.hosts.reached.contains(&key),
                    HostChoice::Account(key),
                )),
                HostChoice::Ssh(_) => None,
            })
            .collect()
    }

    /// Puts the person on `choice`, which the rail and the tab list then follow.
    pub fn choose_host(&mut self, choice: HostChoice, cx: &mut Context<Self>) {
        self.call("chooseHost", json!({ "host": choice.key() }));
        cx.notify();
    }
}

/// How busy a host is, as its row shows it, from what its core sends in its view.
pub fn load_label(load: &sikemux_wire::protocol::HostLoad) -> String {
    match load.memory_used {
        Some(bytes) => format!("{}% · {:.1} GB", load.cpu, bytes as f64 / 1024f64.powi(3)),
        None => format!("{}%", load.cpu),
    }
}

/// A project's session on its way to another host, as the person asked for it.
pub struct Sending {
    pub project: String,
    pub name: String,
    pub host: String,
    pub label: SharedString,
    pub after: HostChoice,
    pub moving: bool,
    pub uncommitted: bool,
}

/// One line on what went, what stayed here and how the uncommitted changes went.
fn sent_summary(sent: &Value, sending: &Sending) -> String {
    let carried = sent["carried"].as_array().map_or(0, Vec::len);
    let chats = match carried {
        1 => "1 chat".to_owned(),
        count => format!("{count} chats"),
    };
    let done = match (sending.moving, sent["moved"] == true) {
        (true, true) => "Moved",
        _ => "Copied",
    };
    let mut said = format!(
        "{done} {} to {} with {chats} and its tabs.",
        sending.name, sending.label
    );
    if sending.moving && sent["moved"] != true {
        said.push_str(" Nothing was removed here, since not everything arrived whole.");
    }
    let left: Vec<String> = sent["left"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|left| {
            format!(
                "{} ({})",
                left["title"].as_str().unwrap_or_default(),
                left["why"].as_str().unwrap_or_default()
            )
        })
        .collect();
    if !left.is_empty() {
        said.push_str(&format!(" Stayed here: {}.", left.join(", ")));
    }
    match sent["uncommitted"].as_str() {
        Some("applied") => said.push_str(" Uncommitted changes went too."),
        Some(error) => said.push_str(&format!(" Uncommitted changes stayed here: {error}.")),
        None => {}
    }
    said
}

/// A host's update status, from its view, when it updates itself.
pub fn host_updates(view: &sikemux_wire::protocol::DeviceView) -> Option<&Value> {
    view.updates
        .as_ref()
        .filter(|updates| updates["automatic"].is_boolean())
}

/// How a host's row sums up its updates: under way, failed, else the version it runs.
pub fn updates_label(updates: &Value) -> Option<String> {
    Some(match updates["state"].as_str() {
        Some("failed") => "update failed".to_owned(),
        Some("downloading" | "installing" | "waiting" | "ready") => "updating".to_owned(),
        _ => updates["current"].as_str()?.to_owned(),
    })
}

/// "Update automatically" on a host that updates itself, ticked while it does, and why its
/// last update failed when it did. `set` turns it on or off.
pub fn updates_items(
    popup: PopupMenu,
    updates: &Value,
    set: impl Fn(bool, &mut App) + 'static,
) -> PopupMenu {
    let on = updates["automatic"] == Value::Bool(true);
    let popup = popup.item(
        PopupMenuItem::new("Update automatically")
            .icon(Icon::Refresh)
            .checked(on)
            .on_click(move |_, _, cx| set(!on, cx)),
    );
    match (updates["state"].as_str(), updates["error"].as_str()) {
        (Some("failed"), Some(error)) => popup.item(PopupMenuItem::label(format!(
            "The last update failed: {error}"
        ))),
        _ => popup,
    }
}

//! The host the person is on (ADR 0018): this one, another host on the account, or an SSH host
//! without Sikemux. The rail, the top bar and the tab list follow the one chosen.

pub mod install_pane;
pub mod page;
pub(crate) mod top_place;

use std::collections::HashMap;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::{App, Context, SharedString};
use sikemux_core::git::log::GitOverview;
use sikemux_core::git::status::ProjectRepo;
use sikemux_core::server::activity_log::ActivitySummary;
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::hosts::{HostAliasParams, HostReach, HostRow};
use sikemux_core::server::mods::ModList;
use sikemux_core::server::ports::ProjectPort;
use sikemux_core::server::remote::ApprovalSetting;
use sikemux_core::server::tasks::TrustQuestion;
use sikemux_core::server::tasks::{RunView, TaskList};
use sikemux_plugin_api::codehost::PullList;
use sikemux_view::device::DeviceView;
use sikemux_view::host_stats::{AlertLevel, HostAlert, HostAlertKind};

use crate::chat::Question;
use crate::copies::CopyState;
use crate::core;
use crate::core::Target;
use crate::hosts::install_pane::Words;
use crate::icons::Icon;
use crate::reads::Drafts;
use crate::toast;
use crate::workbench::{Strip, Workbench};
use sikemux_core::server::account::devices::DeviceParams;
use sikemux_core::server::hosts::install::InstallParams;
use sikemux_core::server::hosts::open::OpenOnParams;
use sikemux_core::server::hosts::transfer::{SendParams, SessionSent};
use sikemux_core::server::layout::{ActivateProjectParams, HostParams};
use sikemux_core::server::spare::NewTerminal;
use sikemux_core::server::updates::HostUpdatesParams;
use sikemux_view::updates::{UpdateOutcomeState, UpdateState, UpdateStatus};

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

/// What one core has told this window. An account host's trust, drafts and questions are kept
/// while it is in front, as only then are they watched, and so is what the project in front
/// holds, on either kind of core.
#[derive(Default)]
pub struct Feed {
    pub view: Option<DeviceView>,
    pub trust: Vec<TrustQuestion>,
    pub drafts: Option<Drafts>,
    pub questions: Vec<Question>,
    pub tasks: Option<TaskList>,
    pub runs: Vec<RunView>,
    pub ports: Vec<ProjectPort>,
    pub activity: Option<ActivitySummary>,
    pub mods: Option<ModList>,
    pub git_project_repo: Option<ProjectRepo>,
    /// The repository's status, branches and log, or why they did not read.
    pub git_overview: Option<Result<GitOverview, String>>,
    pub git_draft: Option<Drafts>,
    /// The worktree in front's pull requests, and whether its branch is merged.
    pub worktree_pulls: Option<PullList>,
    pub worktree_merged: Option<bool>,
    /// Whether it asks the person before a new device joins, once it has said.
    pub approval: Option<bool>,
    /// Whether this app is connected to it now, for an account host.
    pub reached: bool,
    /// Its home folder, once an account host has said.
    pub home: Option<String>,
    /// How busy it is, while the Hosts page watches it, and what was last read once it is not.
    pub stats: Option<sikemux_view::host_stats::HostStats>,
    /// When this window last received `stats`, in Unix milliseconds to show, and by the
    /// steady clock to judge whether they are still current.
    pub stats_at: Option<u64>,
    pub stats_seen: Option<std::time::Instant>,
    /// Why it is out of reach, or for this host whether it can be reached, as this host's
    /// core last found, and when this window last asked.
    pub diagnosis: Option<sikemux_view::host_stats::HostDiagnosis>,
    pub diagnosis_asked: Option<std::time::Instant>,
    /// When this window saw an account host go out of reach, while it stays out of reach.
    pub away_since: Option<u64>,
}

/// How long numbers a host sent last count as now; it sends them every few seconds while watched.
pub const STATS_FRESH: std::time::Duration = std::time::Duration::from_secs(30);

impl Feed {
    fn is_call(&self, id: &str) -> bool {
        (self.view.as_ref()).is_some_and(|view| view.calls.iter().any(|call| call.id == id))
    }

    /// The host's numbers while they are recent, from a host this window reaches.
    pub fn fresh_stats(&self) -> Option<&sikemux_view::host_stats::HostStats> {
        let recent = self.stats_seen?.elapsed() < STATS_FRESH;
        self.stats.as_ref().filter(|_| recent)
    }

    /// Lets go of what the project in front held, as this core stops being in front.
    pub fn leave(&mut self) {
        self.tasks = None;
        self.runs.clear();
        self.ports.clear();
        self.activity = None;
        self.mods = None;
        self.git_project_repo = None;
        self.git_overview = None;
        self.git_draft = None;
        self.worktree_pulls = None;
        self.worktree_merged = None;
    }
}

/// What each core has told this window: this host's and those of the account's other hosts.
#[derive(Default)]
pub struct Feeds {
    pub this: Feed,
    hosts: HashMap<String, Feed>,
    /// The account host in front, whose trust, drafts and questions are watched.
    front: Option<String>,
    /// Why this app reaches none of the account's other hosts, when it cannot.
    pub unavailable: Option<String>,
    /// An SSH host just set up, whose terminals follow it onto its linked host once it is one.
    following: Option<String>,
}

impl Feeds {
    pub fn host(&self, key: &str) -> Option<&Feed> {
        self.hosts.get(key)
    }

    pub fn host_mut(&mut self, key: &str) -> &mut Feed {
        self.hosts.entry(key.to_owned()).or_default()
    }

    pub fn at(&self, place: &Place) -> Option<&Feed> {
        match place {
            Place::This => Some(&self.this),
            Place::Account(host) => self.host(host),
        }
    }

    pub fn at_mut(&mut self, place: &Place) -> &mut Feed {
        match place {
            Place::This => &mut self.this,
            Place::Account(host) => self.host_mut(host),
        }
    }

    pub fn hosts(&self) -> impl Iterator<Item = (&String, &Feed)> {
        self.hosts.iter()
    }

    pub fn front(&self) -> Option<&String> {
        self.front.as_ref()
    }

    pub fn front_mut(&mut self) -> Option<&mut Feed> {
        let front = self.front.as_ref()?;
        self.hosts.get_mut(front)
    }

    /// The feed of the core in front: the account host chosen, else this host's.
    pub fn in_front(&self) -> &Feed {
        (self.front.as_ref())
            .and_then(|front| self.hosts.get(front))
            .unwrap_or(&self.this)
    }

    /// Makes `host` the account host in front, letting go of what the last one's core answered.
    pub fn set_front(&mut self, host: Option<String>) {
        if let Some(gone) = self.front_mut() {
            gone.trust.clear();
            gone.drafts = None;
            gone.questions.clear();
        }
        if let Some(host) = &host {
            self.host_mut(host);
        }
        self.front = host;
    }

    /// What agents asked the person and the assumptions they went on: this host's, then those of
    /// the account host in front.
    pub fn questions(&self) -> impl Iterator<Item = &Question> {
        let front = self.front.as_ref().and_then(|front| self.hosts.get(front));
        (self.this.questions.iter()).chain(front.into_iter().flat_map(|feed| &feed.questions))
    }

    /// The questions a chat answers outside a call: an assumption an agent went on that nobody
    /// raised, which the person may still correct. Every other question reaches them as a call.
    pub fn questions_outside_calls(&self) -> impl Iterator<Item = &Question> {
        let front = self.front.as_ref().and_then(|front| self.hosts.get(front));
        std::iter::once(&self.this).chain(front).flat_map(|feed| {
            (feed.questions.iter()).filter(move |question| !feed.is_call(&question.id))
        })
    }

    /// Forgets the hosts no longer linked to the account, as `listed` says.
    pub fn keep_linked(&mut self, listed: &[HostRow]) {
        let linked = |id: &String| listed.iter().any(|host| host.id.as_ref() == Some(id));
        self.hosts
            .retain(|id, _| linked(id) || self.front.as_ref() == Some(id));
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
        let listed = self.reads.hosts.iter().filter_map(|host| {
            let choice = match (&host.id, &host.ssh) {
                (Some(key), _) => HostChoice::Account(key.clone()),
                (None, Some(alias)) => HostChoice::Ssh(alias.clone()),
                (None, None) => return None,
            };
            Some(HostOption {
                choice,
                label: host.name.clone().into(),
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
            .this_view()
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
            Place::Account(host) => self.feeds.host(host)?.home.clone(),
        }
    }

    /// Whose core the panes in front talk to.
    pub fn place(&self) -> Place {
        match self.chosen_host() {
            HostChoice::Account(key) => Place::Account(key),
            _ => Place::This,
        }
    }

    /// Makes the copy `copying` asks for, once the person agrees: that host clones the project
    /// from its remote, or takes it from the copy's host as a bundle, then the new copy is in
    /// front.
    pub fn copy_to(&mut self, copying: Copying, cx: &mut Context<Self>) {
        let (from, label) = (&copying.from.host, &copying.label);
        let said = match copying.uncommitted {
            true => format!("{label} makes a new checkout of it from its remote, or from {from} when it cannot reach the remote, with the uncommitted changes there as a commit on top."),
            false => format!("{label} makes a new checkout of it from its remote, or from {from} when it cannot reach the remote. Only what is committed goes over; uncommitted changes stay there."),
        };
        self.confirm(
            format!("Copy {} to {label}?", copying.name()),
            said,
            "Copy",
            false,
            move |bench, yes, _, cx| {
                if yes {
                    bench.copy_now(&copying, cx);
                }
            },
            cx,
        );
    }

    fn copy_now(&mut self, copying: &Copying, cx: &mut Context<Self>) {
        let params = OpenOnParams {
            path: copying.from.path.clone(),
            host: copying.host.clone(),
            uncommitted: copying.uncommitted,
        };
        // The copy's own host sends it.
        let opening = core::Target::new(&self.asks, copying.from.place.clone())
            .request_typed::<call::OpenOnHost>(params);
        let (name, label) = (copying.name(), copying.label.clone());
        let identity = copying.identity.clone();
        self.note_copy(
            identity.as_deref(),
            label.clone(),
            CopyState::Going,
            format!("Copying {name} to {label}…"),
            cx,
        );
        let (place, chat) = (self.place_of(&copying.host), copying.chat.clone());
        cx.spawn(async move |bench, cx| {
            let opened = opening.await;
            let _ = bench.update(cx, |bench, cx| {
                let opened = opened.and_then(|opened| match opened["project"].as_str() {
                    Some(_) => Ok(opened),
                    None => Err(format!("{label} did not add the project")),
                });
                let opened = match opened {
                    Ok(opened) => opened,
                    Err(error) => {
                        let said = format!("{name} did not copy to {label}: {error}");
                        bench.note_copy(
                            identity.as_deref(),
                            label,
                            CopyState::Failed,
                            said.clone(),
                            cx,
                        );
                        bench.notify(toast::error(said), cx);
                        return;
                    }
                };
                let said = match opened["uncommitted"].as_str() {
                    Some("applied") => format!(
                        "Copied {name} to {label}, with the uncommitted changes as a commit on top."
                    ),
                    Some(error) => format!(
                        "Copied {name} to {label}. Its uncommitted changes did not go: {error}."
                    ),
                    None => format!("Copied {name} to {label}."),
                };
                bench.note_copy(
                    identity.as_deref(),
                    label.clone(),
                    CopyState::Done,
                    said.clone(),
                    cx,
                );
                bench.notify(toast::success(said), cx);
                let copy = crate::copies::HostCopy {
                    place,
                    project: opened["project"].as_str().unwrap_or_default().to_owned(),
                    host: label.clone(),
                    path: opened["path"].as_str().unwrap_or_default().into(),
                    git: None,
                    away: false,
                };
                match &chat {
                    Some(chat) => bench.move_chat_to_copy(chat, &copy, identity.as_deref(), cx),
                    None => bench.go_to_copy(&copy, identity.as_deref(), cx),
                }
            });
        })
        .detach();
    }

    /// Moves the project `sending` names after the person agrees, as a move removes its chats
    /// where it was.
    pub fn move_session_asking(&mut self, sending: Sending, cx: &mut Context<Self>) {
        let title = format!("Move {} to {}?", sending.name, sending.label);
        let held = std::cell::RefCell::new(Some(sending));
        self.confirm(
            title,
            "Each chat the other host takes is closed here and its saved conversation removed. If one goes on here while the move runs, none closes here and the other host drops the ones it took.",
            "Move",
            true,
            move |bench, yes, _, cx| {
                if let Some(sending) = held.borrow_mut().take().filter(|_| yes) {
                    bench.send_session(sending, cx);
                }
            },
            cx,
        );
    }

    /// Sends the project `project` of the host at `from` to `host`, with its chats and tabs and,
    /// when asked, its uncommitted changes. A move then removes the chats there, once that host
    /// has every one. The person is then on that host, in that project, when `after` says so.
    pub fn send_session(&mut self, sending: Sending, cx: &mut Context<Self>) {
        self.send_session_into(sending, false, cx);
    }

    /// Sends the session as `send_session` does, into a worktree at its commit on the other
    /// host when `worktree`, rather than into that host's checkout.
    fn send_session_into(&mut self, sending: Sending, worktree: bool, cx: &mut Context<Self>) {
        let params = SendParams {
            project: sending.project.clone(),
            host: sending.host.clone(),
            moving: sending.moving,
            uncommitted: sending.uncommitted,
            worktree,
            there: self.copy_in_front_on(&sending.from, &sending.project, &sending.host),
        };
        // The project's own host sends it.
        let asked = core::Target::new(&self.asks, sending.from.clone())
            .request_typed::<call::SendSession>(params);
        let verb = if sending.moving { "Moving" } else { "Copying" };
        crate::settings::session_sent(self, None, cx);
        self.notify(
            toast::info(format!("{verb} {} to {}…", sending.name, sending.label)),
            cx,
        );
        cx.spawn(async move |bench, cx| {
            let answer = asked.await;
            let _ = bench.update(cx, |bench, cx| {
                if let Ok(SessionSent {
                    differs: Some(why), ..
                }) = &answer
                {
                    bench.offer_worktree(sending, why, cx);
                    return;
                }
                let said = match &answer {
                    Ok(sent) => sent_summary(sent, &sending),
                    Err(error) => format!("It did not go to {}: {error}", sending.label),
                };
                match &answer {
                    Ok(sent) if sent.left.is_empty() => {
                        bench.notify(toast::success(said.clone()), cx)
                    }
                    Ok(_) => bench.notify(toast::info(said.clone()), cx),
                    Err(_) => bench.notify(toast::error(said.clone()), cx),
                }
                crate::settings::session_sent(bench, Some(said.into()), cx);
                let (Ok(sent), Some(after)) = (answer, sending.after.clone()) else {
                    return;
                };
                let place = match &after {
                    HostChoice::Account(key) => Place::Account(key.clone()),
                    _ => Place::This,
                };
                bench.choose_host(after, cx);
                core::Target::new(&bench.asks, place).call_typed::<call::ActivateProject>(
                    ActivateProjectParams {
                        project: sent.project,
                    },
                );
            });
        })
        .detach();
    }

    /// Says the session did not go because the other host's checkout stands elsewhere, with
    /// a way to send it into a worktree there at its commit instead.
    fn offer_worktree(&mut self, sending: Sending, why: &str, cx: &mut Context<Self>) {
        let said = format!(
            "{} did not go to {}: {why}, and laying it there would mix the two.",
            sending.name, sending.label
        );
        crate::settings::session_sent(self, Some(said.clone().into()), cx);
        let bench = cx.weak_entity();
        let note = toast::error(said).action(move |_, _, cx| {
            let (note, bench, sending) = (cx.entity(), bench.clone(), sending.clone());
            gpui_kit::component::button::Button::new("send-into-worktree")
                .label("Into a worktree")
                .on_click(move |_, window, cx| {
                    let sending = sending.clone();
                    let _ =
                        bench.update(cx, |bench, cx| bench.send_session_into(sending, true, cx));
                    note.update(cx, |note, cx| note.dismiss(window, cx));
                })
        });
        self.notify(note, cx);
    }

    /// Sets Sikemux up on the SSH host `alias` as `set_up` says, over the person's own ssh;
    /// its row says how it goes. When it ends short a toast says why, and when it brings the
    /// alias onto a linked host with its terminals in front, the window moves to that host.
    pub fn set_up_host(
        &mut self,
        alias: &str,
        set_up: crate::hosts::install_pane::SetUp,
        cx: &mut Context<Self>,
    ) {
        let (asks, alias) = (self.asks.clone(), alias.to_owned());
        let params = InstallParams {
            alias: alias.clone(),
            work: set_up,
        };
        cx.spawn(async move |bench, cx| {
            let here = core::Target::here(&asks);
            let ended = match here.request_typed::<call::InstallHost>(params).await {
                Err(error) => Err(error.to_string()),
                Ok(_) => here
                    .request_typed::<call::HostSetup>(HostAliasParams {
                        alias: alias.clone(),
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|ended| match ended.problem {
                        Some(problem) => Err(problem),
                        None => Ok(ended.added),
                    }),
            };
            let _ = bench.update(cx, |bench, cx| match ended {
                Err(problem) => bench.notify(
                    toast::error(format!("{}: {problem}", set_up.offer(&alias))),
                    cx,
                ),
                Ok(added) => {
                    if let Some(name) = added {
                        bench.notify(
                            toast::success(format!("{alias} was added to your account as {name}")),
                            cx,
                        );
                    }
                    bench.feeds.following = Some(alias.clone());
                    bench.follow_folded(cx);
                }
            });
        })
        .detach();
    }

    /// Moves the window to the linked host the SSH host just set up became, once the core lists
    /// it as one, when that SSH host's terminals are in front.
    pub(crate) fn follow_folded(&mut self, cx: &mut Context<Self>) {
        let Some(alias) = self.feeds.following.clone() else {
            return;
        };
        if self.strip() != Some(Strip::Host(alias.clone())) {
            self.feeds.following = None;
            return;
        }
        let linked = (self.reads.hosts.iter())
            .find(|host| host.id.is_some() && host.ssh.as_deref() == Some(alias.as_str()))
            .and_then(|host| host.id.clone());
        let Some(key) = linked else {
            return;
        };
        self.feeds.following = None;
        self.choose_host(HostChoice::Account(key), cx);
    }

    /// Takes a set-up that stopped short off `alias`'s row.
    pub fn dismiss_set_up(&self, alias: &str) {
        Target::here(&self.asks).call_typed::<call::DismissHostSetup>(HostAliasParams {
            alias: alias.to_owned(),
        });
    }

    /// Asks this host's core why `host` is out of reach, or, with none, whether this host can be
    /// reached, at most every so often.
    pub fn read_diagnosis(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        const ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(10);
        let place = host.clone().map_or(Place::This, Place::Account);
        let feed = self.feeds.at_mut(&place);
        if feed
            .diagnosis_asked
            .is_some_and(|asked| asked.elapsed() < ASK_EVERY)
        {
            return;
        }
        feed.diagnosis_asked = Some(std::time::Instant::now());
        let asked = Target::here(&self.asks).request_typed::<call::HostDiagnosis>(
            sikemux_core::server::diagnosis::DiagnosisParams { host },
        );
        cx.spawn(async move |bench, cx| {
            let Ok(diagnosis) = asked.await else {
                return;
            };
            let _ = bench.update(cx, |bench, cx| {
                bench.feeds.at_mut(&place).diagnosis = Some(diagnosis);
                if bench.hosts_page_in_front() {
                    bench.parts.redraw_stage(cx);
                }
            });
            cx.background_executor().timer(ASK_EVERY).await;
            let _ = bench.update(cx, |bench, cx| {
                if bench.hosts_page_in_front() {
                    bench.parts.redraw_stage(cx);
                }
            });
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
                let place = host.map_or(Place::This, Place::Account);
                bench.feeds.at_mut(&place).approval = Some(read.require_approval);
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks the person before a new device joins `host`, this one when none, or stops asking.
    pub fn flip_approval(&self, host: Option<String>, cx: &mut Context<Self>) {
        let place = host.clone().map_or(Place::This, Place::Account);
        let Some(on) = self.feeds.at(&place).and_then(|feed| feed.approval) else {
            return;
        };
        let place = host.clone().map_or(Place::This, Place::Account);
        let set = core::Target::new(&self.asks, place).request_typed::<call::SetRequireApproval>(
            ApprovalSetting {
                require_approval: !on,
            },
        );
        self.report(
            set,
            "Require approval for new devices",
            move |bench, _, cx| bench.read_approval(host, cx),
            cx,
        );
    }

    /// Turns the account host `host`'s own updates on or off; its view then says which.
    pub fn set_host_updates(&self, host: String, on: bool, cx: &mut Context<Self>) {
        let set = core::Target::new(&self.asks, Place::Account(host))
            .request_typed::<call::SetHostUpdates>(HostUpdatesParams { on });
        self.report(set, "Update automatically", |_, _, _| {}, cx);
    }

    /// A new chat on the account host `host`, in the project in front there, with the agent the
    /// rail is set to, and the person moved there to see it.
    pub fn new_chat_on(&mut self, host: &str, cx: &mut Context<Self>) {
        self.show_workbench(cx);
        let Some(view) = self.feeds.host(host).and_then(|feed| feed.view.as_ref()) else {
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
        self.show_workbench(cx);
        self.take_keys_when_shown(crate::keyboard::TakesKeys::Terminal, cx);
        core::Target::new(&self.asks, Place::Account(host.to_owned()))
            .call_typed::<call::NewTerminal>(NewTerminal::loose());
        self.choose_host(HostChoice::Account(host.to_owned()), cx);
    }

    /// Renames the device or host `key` for every device and host on the account.
    pub fn rename_on_account(&mut self, key: &str, name: &str, cx: &mut Context<Self>) {
        let key = key.to_owned();
        self.prompt(
            format!("Rename {name}"),
            name.to_owned(),
            move |bench, typed, _, _| {
                if !typed.trim().is_empty() {
                    Target::here(&bench.asks).call_typed::<call::RenameAccountDevice>(
                        DeviceParams {
                            key: key.clone(),
                            name: Some(typed.trim().to_owned()),
                        },
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
                    Target::here(&bench.asks).call_typed::<call::RemoveAccountDevice>(DeviceParams {
                        key: key.clone(),
                        name: None,
                    });
                }
            },
            cx,
        );
    }

    /// Opens ~/.ssh/config on this host in the editor, at the `Host` line of `alias` when given.
    pub fn edit_ssh_config(&mut self, alias: Option<String>, cx: &mut Context<Self>) {
        self.show_workbench(cx);
        let here = core::Target::here(&self.asks);
        let file = here.request_typed::<call::SshConfigFile>(NoParams {});
        let hosts = alias
            .is_some()
            .then(|| here.request_typed::<call::SshHosts>(NoParams {}));
        cx.spawn(async move |bench, cx| {
            let file = file.await;
            let line = match hosts {
                Some(hosts) => hosts.await.ok().and_then(|hosts| {
                    let listed = hosts
                        .iter()
                        .find(|host| Some(&host.alias) == alias.as_ref())?;
                    Some((u32::try_from(listed.line).ok()?, listed.file.clone()))
                }),
                None => None,
            };
            let (line, included) = line.map_or((None, None), |(line, file)| (Some(line), file));
            let file = file.map(|main| included.unwrap_or(main));
            let _ = bench.update(cx, |bench, cx| match file {
                Ok(path) => {
                    bench.choose_host(HostChoice::This, cx);
                    let at = crate::editor::OpenAt {
                        path: path.into(),
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
                        let removed = core::Target::here(&asks)
                            .request_typed::<call::RemoveAccountDevice>(DeviceParams {
                                key: key.clone(),
                                name: None,
                            });
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
                    self.feeds.host(&key).is_some_and(|feed| feed.reached),
                    HostChoice::Account(key),
                )),
                HostChoice::Ssh(_) => None,
            })
            .collect()
    }

    /// Puts the person on `choice`, which the rail and the tab list then follow.
    pub fn choose_host(&mut self, choice: HostChoice, cx: &mut Context<Self>) {
        self.show_workbench(cx);
        Target::here(&self.asks).call_typed::<call::ChooseHost>(HostParams {
            host: choice.key().to_string(),
        });
        cx.notify();
    }
}

/// How a host is reached, as the rail and Devices both show it. A linked host counts as
/// connected only once this app has its own connection to it, which its stats come over.
pub fn reach(bench: &Workbench, row: &HostRow) -> HostReach {
    let app_reached =
        (row.id.as_ref()).is_none_or(|id| bench.feeds.host(id).is_some_and(|feed| feed.reached));
    match row.reach {
        HostReach::Connected if !app_reached => HostReach::Away,
        reach => reach,
    }
}

/// A new copy of a project, as the person asked for it.
pub struct Copying {
    /// The copy it is made from.
    pub from: crate::copies::HostCopy,
    /// What the project is known by on every host.
    pub identity: Option<String>,
    /// The account host it goes to, by its key, and its name.
    pub host: String,
    pub label: SharedString,
    /// What is not committed goes too, as a commit on top.
    pub uncommitted: bool,
    /// The new chat that moves onto the copy once it exists.
    pub chat: Option<String>,
}

impl Copying {
    fn name(&self) -> String {
        (self.from.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// What a host in trouble says of `alert` on its Hosts page card:
/// "Memory 98%", "Disk 94%", "CPU 97%" or "Out of reach 14 min".
pub fn alert_words(alert: &HostAlert) -> String {
    match alert.kind {
        HostAlertKind::Offline => format!("Out of reach {}", lasted(alert.since)),
        HostAlertKind::Memory if alert.pressure => "Memory short".to_owned(),
        kind => format!("{} {}%", alert_kind_word(kind), alert.percent),
    }
}

pub fn alert_kind_word(kind: HostAlertKind) -> &'static str {
    match kind {
        HostAlertKind::Memory => "Memory",
        HostAlertKind::Disk => "Disk",
        HostAlertKind::Cpu => "CPU",
        HostAlertKind::Offline => "Out of reach",
        HostAlertKind::Unknown => "Load",
    }
}

/// How long since `since` (Unix milliseconds): "just now", "14 min", "3 h" or "2 d".
pub fn lasted(since: u64) -> String {
    let minutes = crate::ui::time::now_ms().saturating_sub(since) / 60_000;
    match minutes {
        0 => "just now".to_owned(),
        1..60 => format!("{minutes} min"),
        60..1440 => format!("{} h", minutes / 60),
        _ => format!("{} d", minutes / 1440),
    }
}

pub fn alert_tint(level: AlertLevel, t: &crate::theme::Tokens) -> gpui_kit::Hsla {
    match level {
        AlertLevel::Warn => t.warn,
        AlertLevel::Danger => t.danger,
    }
}

/// The worst of `alerts`, serious ones first, then the longest standing.
pub fn worst_alert<'a>(alerts: impl IntoIterator<Item = &'a HostAlert>) -> Option<&'a HostAlert> {
    alerts.into_iter().max_by_key(|alert| {
        (
            alert.level == AlertLevel::Danger,
            std::cmp::Reverse(alert.since),
        )
    })
}

impl Workbench {
    /// The worst alert about the account host `key` names, this one when none: what this
    /// host's core says, such as that host being out of reach, and, while it is `connected`,
    /// what that host's own core says of itself.
    pub fn host_alert(&self, key: Option<&str>, connected: bool) -> Option<&HostAlert> {
        let here = (self.this_view().into_iter())
            .flat_map(|view| &view.alerts)
            .filter(|alerted| alerted.host.as_deref() == key);
        let own = (key.filter(|_| connected))
            .and_then(|key| self.feeds.host(key)?.view.as_ref())
            .into_iter()
            .flat_map(|view| &view.alerts)
            .filter(|alerted| alerted.host.is_none());
        worst_alert(here.chain(own).map(|alerted| &alerted.alert))
    }
}

/// A project's session on its way to another host, as the person asked for it.
#[derive(Clone)]
pub struct Sending {
    /// Whose core holds the project now.
    pub from: Place,
    pub project: String,
    pub name: String,
    pub host: String,
    pub label: SharedString,
    /// Where the person goes once it arrives, when they go with it.
    pub after: Option<HostChoice>,
    pub moving: bool,
    pub uncommitted: bool,
}

/// One line on what went, what stayed here and how the uncommitted changes went.
fn sent_summary(sent: &SessionSent, sending: &Sending) -> String {
    let chats = |count: usize| match count {
        1 => "1 chat".to_owned(),
        count => format!("{count} chats"),
    };
    let (name, label) = (&sending.name, &sending.label);
    let mut said = match sending.moving {
        true if sent.closed.is_empty() => format!("{name} did not move to {label}."),
        true => format!(
            "Moved {name} to {label} with {} and its tabs.",
            chats(sent.closed.len())
        ),
        false => format!(
            "Copied {name} to {label} with {} and its tabs.",
            chats(sent.carried.len())
        ),
    };
    let left: Vec<String> = (sent.left.iter())
        .map(|left| format!("{} ({})", left.title, left.why))
        .collect();
    if !left.is_empty() {
        said.push_str(&format!(" Stayed here: {}.", left.join(", ")));
    }
    match sent.uncommitted.as_deref() {
        Some("applied") => said.push_str(" Uncommitted changes went too."),
        Some(error) => said.push_str(&format!(" Uncommitted changes stayed here: {error}.")),
        None => {}
    }
    said
}

/// A host's update status, from its view, when it updates itself.
pub fn host_updates(view: &sikemux_view::device::DeviceView) -> Option<UpdateStatus> {
    (view.updates.clone()).filter(|updates| updates.automatic.is_some())
}

/// How a host's last update ended, once it ended badly: a written outcome
/// outlives the core that wrote it, so a failure nobody was watching still shows.
pub(crate) fn failed_update(updates: &UpdateStatus) -> Option<(UpdateOutcomeState, Option<&str>)> {
    if updates.state == UpdateState::Failed {
        return Some((UpdateOutcomeState::Failed, updates.error.as_deref()));
    }
    let outcome = updates.outcome.as_ref()?;
    let settled = updates.state == UpdateState::Idle || updates.id.as_ref() == Some(&outcome.id);
    let bad = matches!(
        outcome.state,
        UpdateOutcomeState::Failed | UpdateOutcomeState::RolledBack
    );
    (settled && bad && outcome.to.as_deref() != Some(updates.current.as_str()))
        .then_some((outcome.state, outcome.reason.as_deref()))
}

/// What a host's row says of its updates while one is under way or has failed.
pub fn updates_label(updates: &UpdateStatus) -> Option<String> {
    match failed_update(updates) {
        Some((UpdateOutcomeState::RolledBack, _)) => return Some("update rolled back".to_owned()),
        Some(_) => return Some("update failed".to_owned()),
        None => {}
    }
    match updates.state {
        UpdateState::Downloading
        | UpdateState::Installing
        | UpdateState::Waiting
        | UpdateState::Ready => Some("updating".to_owned()),
        UpdateState::Idle | UpdateState::Failed => None,
    }
}

/// "Update automatically" on a host that updates itself, ticked while it does, and why its
/// last update failed when it did. `set` turns it on or off.
pub fn updates_items(
    popup: PopupMenu,
    updates: &UpdateStatus,
    set: impl Fn(bool, &mut App) + 'static,
) -> PopupMenu {
    let on = updates.automatic == Some(true);
    let popup = popup.item(
        PopupMenuItem::new("Update automatically")
            .icon(Icon::Refresh)
            .checked(on)
            .on_click(move |_, _, cx| set(!on, cx)),
    );
    let restarting = updates.forced
        && !updates.blockers.is_empty()
        && matches!(updates.state, UpdateState::Ready | UpdateState::Installing);
    if restarting {
        let count = updates.blockers.len();
        let chats = if count == 1 { "chat" } else { "chats" };
        return popup.item(PopupMenuItem::label(format!(
            "Restarting into the update now: {count} {chats} stop and pick up again after"
        )));
    }
    match failed_update(updates) {
        Some((UpdateOutcomeState::RolledBack, reason)) => {
            popup.item(PopupMenuItem::label(format!(
                "The last update was undone: {}",
                reason.unwrap_or("the new core did not start well")
            )))
        }
        Some((_, Some(reason))) => popup.item(PopupMenuItem::label(format!(
            "The last update failed: {reason}"
        ))),
        _ => popup,
    }
}

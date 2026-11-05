//! What the window waits on from the core, each read named once: what answers it and moves it,
//! the field it is kept in, what follows an answer, and what clears it. Adding one is a row here,
//! and the compiler asks for the rest.

use std::collections::BTreeMap;

use gpui_kit::Context;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sikemux_core::git::log::GitOverview;
use sikemux_core::git::status::ProjectRepo;
use sikemux_core::server::{
    AccountUsage, ActivitySummary, Draft, HostRow, ModList, ProjectEntry, ProjectPort, RunView,
    ShareView, ShownStash, SshHost, TaskList, UpdateState, UpdateStatus,
};
use sikemux_plugin_api::codehost::PullList;
use sikemux_view::catalog::CatalogAgent;
use sikemux_view::plugins::{Listed, TokenForm};
use sikemux_view::prefs::Preferences;
use sikemux_view::questions::AgentQuestion;
use sikemux_view::theme::ThemeSummary;
use sikemux_wire::changed::Topic;

use crate::chat::Question;
use crate::core::Ask;
use crate::hosts::{Feed, Feeds};
use crate::theme;
use crate::workbench::Workbench;

/// One thing the window asks a core and acts on the answer of, kept up by a `Target::watch` or
/// asked once by a `Target::answer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Read {
    Theme,
    Plugins,
    Hosts,
    Update,
    Trust,
    Questions,
    Shares,
    Stash,
    Catalog,
    Usage,
    SkillsAdded,
    Preferences,
    WhatsNew,
    Picture,
    Themes,
    /// Activity across every project, for Settings.
    ActivityEverywhere,
    /// What the project in front holds.
    Activity,
    Mods,
    Tasks,
    Runs,
    ProjectConfig,
    Ports,
    GitProjectRepo,
    GitOverview,
    GitDraft,
    /// The composer drafts kept on this host, and on the account host in front.
    Drafts,
    HostDrafts,
    /// The trust and agent questions of the account host in front.
    HostTrust,
    HostQuestions,
    /// The worktree in front's pull requests, and whether its branch is merged.
    WorktreePulls,
    WorktreeMerged,
    SignIn,
    SignInFinished,
    ProjectFiles,
    FilesList,
    PickerFolders,
    SshHosts,
}

impl Read {
    /// What the window watches from the moment it opens.
    pub const FROM_THE_START: [Read; 13] = [
        Read::Theme,
        Read::Plugins,
        Read::Hosts,
        Read::Update,
        Read::Trust,
        Read::Questions,
        Read::Shares,
        Read::Stash,
        Read::Catalog,
        Read::Usage,
        Read::SkillsAdded,
        Read::Preferences,
        Read::WhatsNew,
    ];

    /// The core's method that answers it, and the topics whose changing reads it again.
    const fn source(self) -> (&'static str, &'static [Topic]) {
        match self {
            Read::Theme => ("theme", &[Topic::Preferences]),
            Read::Plugins => ("plugins", &[Topic::Plugins]),
            Read::Hosts => ("hosts", &[Topic::Hosts]),
            Read::Update => ("updateStatus", &[Topic::Update]),
            Read::Trust | Read::HostTrust => ("trustAsks", &[Topic::Trust]),
            Read::Questions | Read::HostQuestions => ("questions", &[Topic::Questions]),
            Read::Shares => ("shares", &[Topic::Shares]),
            Read::Stash => ("stash", &[Topic::Stash, Topic::Account]),
            Read::Catalog => ("agentCatalog", &[]),
            Read::Usage => ("agentUsages", &[Topic::Usage]),
            Read::SkillsAdded => ("skillsAdded", &[Topic::Skills]),
            Read::Preferences => ("preferences", &[Topic::Preferences]),
            Read::WhatsNew => ("whatsNew", &[Topic::Preferences]),
            Read::Picture => ("accountPicture", &[]),
            Read::Themes => ("themes", &[Topic::Preferences]),
            Read::Activity | Read::ActivityEverywhere => ("activitySummary", &[Topic::Activity]),
            Read::Mods => ("mods", &[Topic::Mods]),
            Read::Tasks => ("tasks", &[Topic::Tasks, Topic::Runs]),
            Read::Runs => ("taskRuns", &[Topic::Runs]),
            Read::ProjectConfig => ("projectConfig", &[Topic::Tasks]),
            Read::Ports => ("ports", &[Topic::Ports]),
            Read::GitProjectRepo => ("git.projectRepo", &[Topic::Git, Topic::Preferences]),
            Read::GitOverview => ("git.overview", &[Topic::Git]),
            Read::GitDraft | Read::Drafts | Read::HostDrafts => ("drafts", &[Topic::Drafts]),
            Read::WorktreePulls => ("codeHostPulls", &[Topic::CodeHost]),
            Read::WorktreeMerged => ("worktreeMerged", &[Topic::Git]),
            Read::SignIn => ("accountSignIn", &[]),
            Read::SignInFinished => ("accountSignInFinish", &[]),
            Read::ProjectFiles => ("projectFiles", &[]),
            Read::FilesList => ("files.list", &[]),
            Read::PickerFolders => ("projectRoots", &[]),
            Read::SshHosts => ("sshHosts", &[]),
        }
    }

    pub const fn method(self) -> &'static str {
        self.source().0
    }

    pub const fn topics(self) -> &'static [Topic] {
        self.source().1
    }
}

/// The agents the core can run, as its catalog lists them.
#[derive(Default, PartialEq)]
pub struct Catalog(Vec<CatalogAgent>);

impl Catalog {
    pub fn agents(&self) -> impl Iterator<Item = &CatalogAgent> {
        self.0.iter()
    }

    /// The catalog's entry for the agent kind `provider`, such as `claude`.
    pub fn kind(&self, provider: &str) -> Option<&CatalogAgent> {
        self.0.iter().find(|agent| agent.kind == provider)
    }

    /// What the agent kind is called, such as "Claude", or its id until the catalog is read.
    pub fn label<'a>(&'a self, provider: &'a str) -> &'a str {
        self.kind(provider).map_or(provider, |agent| &agent.label)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The plugins built in, as the core last listed them.
#[derive(Default, PartialEq)]
pub struct Plugins(Vec<Listed>);

impl Plugins {
    pub fn all(&self) -> impl Iterator<Item = &Listed> {
        self.0.iter()
    }

    /// Those switched on.
    pub fn on(&self) -> impl Iterator<Item = &Listed> {
        self.0.iter().filter(|plugin| plugin.enabled)
    }

    /// Whether the person is signed in to `id`; none for a plugin with no sign-in.
    pub fn signed_in(&self, id: &str) -> Option<bool> {
        self.0.iter().find(|plugin| plugin.id == id)?.signed_in
    }

    /// The form `id` is signed in to with a token, when it has one.
    pub fn token_form(&self, id: &str) -> Option<&TokenForm> {
        self.0
            .iter()
            .find(|plugin| plugin.id == id)?
            .token_form
            .as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Drafts the core keeps, by the key each is kept under.
pub type Drafts = BTreeMap<String, Draft>;

#[derive(Default)]
pub struct Reads {
    pub catalog: Catalog,
    pub plugins: Plugins,
    /// The account's other hosts, then the SSH hosts a terminal is open to.
    pub hosts: Vec<HostRow>,
    /// How updating this app stands, once the core has said.
    pub update: Option<UpdateStatus>,
    /// How much of its plan each account has used, as the core last read it.
    pub usage: Vec<AccountUsage>,
    /// The chats this host shares read-only.
    pub shares: Vec<ShareView>,
    /// The mods of the host in front, once its core has listed them.
    pub mods: Option<ModList>,
    /// The account's Stash, as this host last listed it.
    pub stash: Option<ShownStash>,
    pub themes: Vec<ThemeSummary>,
    /// What the project in front holds: its tasks, their runs, what listens on its ports,
    /// its activity, and the repository its git pane shows.
    pub tasks: Option<TaskList>,
    pub runs: Vec<RunView>,
    pub ports: Vec<ProjectPort>,
    pub activity: Option<ActivitySummary>,
    pub git_project_repo: Option<ProjectRepo>,
    /// The repository's status, branches and log, or why they did not read.
    pub git_overview: Option<Result<GitOverview, String>>,
    pub git_draft: Option<Drafts>,
    /// Activity across every project, for Settings; `Some(None)` when the core refused it.
    pub activity_everywhere: Option<Option<ActivitySummary>>,
    /// The worktree in front's pull requests, and whether its branch is merged.
    pub worktree_pulls: Option<PullList>,
    pub worktree_merged: Option<bool>,
    /// What the project and host pickers offer: folders under the project roots, and SSH hosts.
    pub picker_folders: Vec<ProjectEntry>,
    pub ssh_hosts: Vec<SshHost>,
}

impl Reads {
    /// Keeps the stash the core listed, and says whether it differs from the one kept. When
    /// this window counted the stash seen later than the answer says, its time stands.
    fn keep_stash(&mut self, value: Value) -> bool {
        let Some(mut told) = decoded::<ShownStash>(value, "stash") else {
            return false;
        };
        let seen = |stash: &ShownStash| {
            let at = stash.seen_at.as_deref()?;
            chrono::DateTime::parse_from_rfc3339(at).ok()
        };
        if let Some(kept) = self.stash.as_ref() {
            if seen(kept).is_some_and(|at| seen(&told).is_none_or(|told| told < at)) {
                told.seen_at = kept.seen_at.clone();
            }
        }
        keep(&mut self.stash, Some(told))
    }

    /// Forgets what `read` answered, so nothing shows the answer of what it watched before.
    pub fn forget(&mut self, read: Read) {
        match read {
            Read::Tasks => self.tasks = None,
            Read::GitDraft => self.git_draft = None,
            Read::Runs => self.runs.clear(),
            Read::Ports => self.ports.clear(),
            Read::Activity => self.activity = None,
            Read::Mods => self.mods = None,
            Read::GitProjectRepo => self.git_project_repo = None,
            Read::WorktreePulls => self.worktree_pulls = None,
            Read::WorktreeMerged => self.worktree_merged = None,
            Read::Theme
            | Read::Plugins
            | Read::Hosts
            | Read::Update
            | Read::Trust
            | Read::HostTrust
            | Read::Questions
            | Read::Shares
            | Read::Stash
            | Read::Catalog
            | Read::Usage
            | Read::SkillsAdded
            | Read::Preferences
            | Read::WhatsNew
            | Read::Picture
            | Read::Themes
            | Read::ActivityEverywhere
            | Read::ProjectConfig
            | Read::GitOverview
            | Read::Drafts
            | Read::HostDrafts
            | Read::HostQuestions
            | Read::SignIn
            | Read::SignInFinished
            | Read::ProjectFiles
            | Read::FilesList
            | Read::PickerFolders
            | Read::SshHosts => {}
        }
    }
}

impl Feeds {
    /// Where the answer to a read kept per core goes.
    fn feed_of(&mut self, read: Read) -> Option<&mut Feed> {
        match read {
            Read::Trust | Read::Questions | Read::Drafts => Some(&mut self.this),
            _ => self.front_mut(),
        }
    }
}

impl Workbench {
    /// Takes in what the core answered `read`, then does what follows from it.
    pub(crate) fn read_answered(&mut self, read: Read, value: Value, cx: &mut Context<Self>) {
        match read {
            Read::Hosts => {
                let changed = keep_decoded(&mut self.reads.hosts, value, "hosts");
                if self.feeds.this.approval.is_none() {
                    self.read_approval(None, cx);
                }
                if !changed {
                    return;
                }
                self.feeds.keep_linked(&self.reads.hosts);
            }
            Read::SkillsAdded => return self.offer_skills(&value, cx),
            Read::Plugins => {
                if !keep_decoded(&mut self.reads.plugins.0, value, "plugins") {
                    return;
                }
                crate::settings::plugins_changed(self, cx);
            }
            Read::Update => {
                let ready = |status: &UpdateStatus| {
                    status.state == UpdateState::Ready
                        && status
                            .available
                            .as_ref()
                            .map(|release| release.version.as_str())
                            != Some(env!("CARGO_PKG_VERSION"))
                };
                let already = self.reads.update.as_ref().is_some_and(ready);
                if !keep_decoded(&mut self.reads.update, value, "update status") {
                    return;
                }
                let now = self.reads.update.as_ref().is_some_and(ready);
                if self.over_shown() {
                    self.parts.redraw(&self.parts.over, cx);
                }
                if now && !already {
                    // The core put the update in place of this app; opening the app again starts it.
                    // An AppImage runs from a mount that goes with this process, so it starts again
                    // from its file.
                    if let Some(appimage) = std::env::var_os("APPIMAGE") {
                        cx.set_restart_path(appimage.into());
                    }
                    if self
                        .reads
                        .update
                        .as_ref()
                        .is_some_and(|update| update.forced)
                    {
                        crate::core::force_next_core_update();
                    }
                    // Restarting runs the restart hooks, which update this workbench.
                    cx.defer(|cx| cx.restart());
                }
            }
            Read::Questions | Read::HostQuestions => {
                let Ok(asked) =
                    serde_json::from_value::<Vec<AgentQuestion>>(value).inspect_err(|error| {
                        eprintln!("sikemux: the core's questions do not read: {error}")
                    })
                else {
                    return cx.notify();
                };
                if let Some(feed) = self.feeds.feed_of(read) {
                    feed.questions = (asked.into_iter())
                        .map(|asked| Question { asked })
                        .collect();
                }
            }
            Read::Theme => {
                if let Ok(palette) = serde_json::from_value(value) {
                    theme::apply(&palette, cx);
                    crate::syntax::restyle(cx);
                    for pane in self.mod_panes.values() {
                        pane.update(cx, |pane, cx| pane.retheme(cx));
                    }
                    cx.refresh_windows();
                }
            }
            Read::Picture => self.picture.read(&value),
            Read::SignIn => match value["url"].as_str() {
                Some(url) => {
                    cx.open_url(url);
                    let _ = self.asks.send(Ask::Answer {
                        read: Read::SignInFinished,
                        params: json!({}),
                    });
                }
                None => self.signing_in = false,
            },
            Read::SignInFinished => {
                self.signing_in = false;
                crate::pairing::offer_account_phones(self, cx);
            }
            Read::FilesList => {
                self.files_retry.answered();
                self.files.listed(&value, &self.target());
                return self.parts.redraw(&self.parts.tabs, cx);
            }
            Read::GitProjectRepo => {
                let was = self.git_repo();
                if let Some(found) = decoded(value, "project repository") {
                    self.reads.git_project_repo = Some(found);
                }
                return self.follow_git_repo(was, cx);
            }
            Read::GitOverview => {
                // An overview that does not read is kept too, which the Git pane says.
                self.reads.git_overview =
                    Some(serde_json::from_value(value).map_err(|error| error.to_string()));
                let overview = self.reads.git_overview.as_ref();
                if let Some(Ok(overview)) = overview {
                    self.files.mark(overview);
                }
                let in_front = self.git_in_front();
                if let Some(overview) = overview {
                    self.git.overview(overview, in_front, cx);
                }
                self.parts.redraw(&self.parts.top, cx);
                return self.parts.redraw(&self.parts.tabs, cx);
            }
            Read::ProjectFiles => {
                if self.project_files.answered(&value) {
                    self.rank(cx);
                    self.files_listed(cx);
                }
                return;
            }
            Read::Preferences => {
                if let Ok(preferences) = serde_json::from_value::<Preferences>(value) {
                    if self.preferences.as_ref() == Some(&preferences) {
                        return;
                    }
                    crate::keymap::bind(&preferences.keybinding_overrides, cx);
                    if self.rails.keeping.is_none() {
                        self.follow_rails(&preferences);
                    }
                    self.sims
                        .read(cx)
                        .offer_to_agents(preferences.ios_simulator);
                    let on = preferences.voice_dictation;
                    self.voice.update(cx, |voice, cx| voice.follow(on, cx));
                    let scale = preferences.chat_text_scale as f32;
                    for chat in self.chats.open.values_mut() {
                        chat.rescale(scale, cx);
                    }
                    crate::ui::grain::set(&preferences, cx);
                    crate::theme::set_text_scale(preferences.ui_text_scale as f32, cx);
                    crate::see_through::set(&preferences, cx);
                    crate::experience::preferences_read(self, &preferences, cx);
                    self.preferences = Some(preferences);
                }
                self.terminals.redraw_all(cx);
                crate::notifications::introduce(self, cx);
                crate::notifications::kept_running(self, cx);
            }
            Read::GitDraft => {
                if !keep_decoded(&mut self.reads.git_draft, value, "drafts") {
                    return;
                }
                if let Some(drafts) = &self.reads.git_draft {
                    self.git.drafts(drafts, cx);
                }
            }
            Read::Stash => {
                if !self.reads.keep_stash(value) {
                    return;
                }
                let unseen = !crate::stash::Stash::of(self).new_items().is_empty();
                if unseen && self.stash_in_front() {
                    self.see_stash();
                }
            }
            Read::Drafts | Read::HostDrafts => {
                let elsewhere = read == Read::HostDrafts;
                let Some(feed) = self.feeds.feed_of(read) else {
                    return;
                };
                if !keep_decoded(&mut feed.drafts, value, "drafts") && !elsewhere {
                    return;
                }
                if let Some(drafts) = &feed.drafts {
                    crate::chat::draft::take(&mut self.chats, drafts, elsewhere, cx);
                }
            }
            Read::Usage => {
                if !keep_decoded(&mut self.reads.usage, value, "plan usage") {
                    return;
                }
            }
            Read::Shares => {
                if !keep_decoded(&mut self.reads.shares, value, "shares") {
                    return;
                }
            }
            Read::Mods => {
                let read = decoded(value, "mods");
                if !read.is_some_and(|read| keep(&mut self.reads.mods, Some(read))) {
                    return;
                }
                self.parts.redraw(&self.parts.rail, cx);
            }
            Read::Themes => {
                if !keep_decoded(&mut self.reads.themes, value, "themes") {
                    return;
                }
            }
            Read::Catalog => {
                if !keep_decoded(&mut self.reads.catalog.0, value, "agent catalog") {
                    return;
                }
                self.git.catalog(&self.reads.catalog, cx);
            }
            Read::Trust | Read::HostTrust => {
                let Some(feed) = self.feeds.feed_of(read) else {
                    return;
                };
                if !keep_decoded(&mut feed.trust, value, "trust questions") && read == Read::Trust {
                    return;
                }
                self.ask_trust(cx);
            }
            Read::Tasks => {
                if !keep_decoded(&mut self.reads.tasks, value, "tasks") {
                    return;
                }
            }
            Read::ProjectConfig => {
                crate::keymap::bind_project_actions(&crate::commands::action_keys(&value), cx);
            }
            Read::Runs => {
                if !keep_decoded(&mut self.reads.runs, value, "process runs") {
                    return;
                }
                self.place_agent_runs(cx);
                self.show_started_runs();
                self.time_run(cx);
            }
            Read::Activity => {
                let Some(summary) = decoded(value, "activity") else {
                    return;
                };
                if !keep(&mut self.reads.activity, Some(summary)) {
                    return;
                }
            }
            Read::ActivityEverywhere => {
                let Some(summary) = decoded(value, "activity") else {
                    return;
                };
                if !keep(&mut self.reads.activity_everywhere, Some(Some(summary))) {
                    return;
                }
            }
            Read::Ports => {
                if !keep_decoded(&mut self.reads.ports, value, "ports") {
                    return;
                }
                self.time_run(cx);
            }
            Read::WhatsNew => crate::experience::whats_new_read(self, &value, cx),
            Read::PickerFolders => {
                keep_decoded(&mut self.reads.picker_folders, value, "project folders");
                self.picker_answered(cx);
            }
            Read::SshHosts => {
                keep_decoded(&mut self.reads.ssh_hosts, value, "SSH hosts");
                self.picker_answered(cx);
            }
            Read::WorktreePulls => {
                self.reads.worktree_pulls = crate::core::outcome(&value)
                    .inspect_err(|error| eprintln!("sikemux: the worktree's pulls: {error}"))
                    .ok();
            }
            Read::WorktreeMerged => {
                self.reads.worktree_merged = decoded(value, "worktree merged");
            }
        }
        cx.notify();
    }

    /// The core turned `read` down.
    pub(crate) fn read_refused(&mut self, read: Read, error: String, cx: &mut Context<Self>) {
        eprintln!("sikemux: the core refused {read:?}: {error}");
        match read {
            Read::SignIn | Read::SignInFinished => self.signing_in = false,
            Read::FilesList => self.files_retry.refused(
                cx,
                |bench| &mut bench.files_retry,
                |bench, _| {
                    if bench.out_of_reach().is_none() {
                        bench.files.relist_shown(&bench.target());
                    }
                },
            ),
            // Both draw empty rather than waiting on an answer that won't come.
            Read::Activity => self.reads.activity = Some(ActivitySummary::default()),
            Read::ActivityEverywhere => {
                self.reads.activity_everywhere = Some(None);
                self.parts.redraw_stage(cx);
                self.parts.activity.update(cx, |_, cx| cx.notify());
            }
            _ => {}
        }
    }
}

/// Keeps `value` decoded in `slot`, and says whether it differs from what was kept. An answer
/// that does not read leaves the slot as it was.
fn keep_decoded<T: DeserializeOwned + PartialEq>(slot: &mut T, value: Value, what: &str) -> bool {
    decoded(value, what).is_some_and(|read| keep(slot, read))
}

/// Puts `read` in `slot`, and says whether it differs from what was there.
fn keep<T: PartialEq>(slot: &mut T, read: T) -> bool {
    let changed = *slot != read;
    *slot = read;
    changed
}

/// `value` read as a `T`, or none when it does not read as one, which is logged.
fn decoded<T: DeserializeOwned>(value: Value, what: &str) -> Option<T> {
    serde_json::from_value(value)
        .inspect_err(|error| eprintln!("sikemux: the core's {what} did not read: {error}"))
        .ok()
}

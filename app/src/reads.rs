//! What the window waits on from the core, each read named once: what answers it and moves it,
//! the field it is kept in, what follows an answer, and what clears it. For a new one the compiler
//! asks for its source and what follows its answer, not for its field or the site that watches it.

use crate::core::Target;
use sikemux_core::server::about::WhatsNew;
use sikemux_core::server::account::SignInPage;
use sikemux_core::server::calls::NoParams;
use std::collections::BTreeMap;

use gpui_kit::Context;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_core::server::activity_log::ActivitySummary;
use sikemux_core::server::agent_accounts::AccountUsage;
use sikemux_core::server::drafts::Draft;
use sikemux_core::server::hosts::HostRow;
use sikemux_core::server::outside::OutsideView;
use sikemux_core::server::plugins::PluginOutcome;
use sikemux_core::server::project_roots::RootEntry;
use sikemux_core::server::share::ShareView;
use sikemux_core::server::ssh::SshHost;
use sikemux_core::server::stash::ShownStash;
use sikemux_view::catalog::CatalogAgent;
use sikemux_view::changed::Topic;
use sikemux_view::plugins::{Listed, TokenForm};
use sikemux_view::prefs::Preferences;
use sikemux_view::questions::AgentQuestion;
use sikemux_view::theme::ThemeSummary;
use sikemux_view::updates::UpdateState;
use sikemux_view::updates::UpdateStatus;

use crate::chat::Question;
use crate::hosts::Place;
use crate::theme;
use crate::workbench::Workbench;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::git::call as git_call;

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
    /// The composer drafts kept on a core.
    Drafts,
    /// The worktree in front's pull requests, and whether its branch is merged.
    WorktreePulls,
    WorktreeMerged,
    SignIn,
    SignInFinished,
    ProjectFiles,
    FilesList,
    PickerFolders,
    SshHosts,
    /// How busy a host is, for the Hosts page while it is in front.
    HostStats,
    /// The programs outside Sikemux this host lets in, and those waiting to be.
    Outside,
}

impl Read {
    /// What the window watches from the moment it opens.
    pub const FROM_THE_START: [Read; 14] = [
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
        Read::Outside,
    ];

    /// What the core in front is watched for while its project is in front. A feed's `leave`
    /// lets go of what they answered.
    pub const OF_THE_PROJECT_IN_FRONT: [Read; 11] = [
        Read::Activity,
        Read::Mods,
        Read::Tasks,
        Read::Runs,
        Read::Ports,
        Read::ProjectConfig,
        Read::GitProjectRepo,
        Read::GitOverview,
        Read::GitDraft,
        Read::WorktreePulls,
        Read::WorktreeMerged,
    ];

    /// What an account host is watched for only while it is in front; this host's are always.
    pub const OF_THE_HOST_IN_FRONT: [Read; 3] = [Read::Trust, Read::Questions, Read::Drafts];

    /// The core's method that answers it, and the topics whose changing reads it again.
    const fn source(self) -> (&'static str, &'static [Topic]) {
        match self {
            Read::Theme => (call::Theme::METHOD, &[Topic::Preferences]),
            Read::Plugins => (call::Plugins::METHOD, &[Topic::Plugins]),
            Read::Hosts => (call::Hosts::METHOD, &[Topic::Hosts]),
            Read::Update => (call::UpdateStatus::METHOD, &[Topic::Update]),
            Read::Trust => (call::TrustAsks::METHOD, &[Topic::Trust]),
            Read::Questions => (call::Questions::METHOD, &[Topic::Questions]),
            Read::Shares => (call::Shares::METHOD, &[Topic::Shares]),
            Read::Stash => (call::Stash::METHOD, &[Topic::Stash, Topic::Account]),
            Read::Catalog => (call::AgentCatalog::METHOD, &[]),
            Read::Usage => (call::AgentUsages::METHOD, &[Topic::Usage]),
            Read::SkillsAdded => (call::SkillsAdded::METHOD, &[Topic::Skills]),
            Read::Preferences => (call::Preferences::METHOD, &[Topic::Preferences]),
            Read::WhatsNew => (call::WhatsNew::METHOD, &[Topic::Preferences]),
            Read::Picture => (call::AccountPicture::METHOD, &[]),
            Read::Themes => (call::Themes::METHOD, &[Topic::Preferences]),
            Read::Activity | Read::ActivityEverywhere => {
                (call::ActivitySummary::METHOD, &[Topic::Activity])
            }
            Read::Mods => (call::Mods::METHOD, &[Topic::Mods]),
            Read::Tasks => (call::Tasks::METHOD, &[Topic::Tasks, Topic::Runs]),
            Read::Runs => (call::TaskRuns::METHOD, &[Topic::Runs]),
            Read::ProjectConfig => (call::ProjectConfig::METHOD, &[Topic::Tasks]),
            Read::Ports => (call::Ports::METHOD, &[Topic::Ports]),
            Read::GitProjectRepo => (
                git_call::ProjectRepo::METHOD,
                &[Topic::Git, Topic::Preferences],
            ),
            Read::GitOverview => (git_call::Overview::METHOD, &[Topic::Git]),
            Read::GitDraft | Read::Drafts => (call::Drafts::METHOD, &[Topic::Drafts]),
            Read::WorktreePulls => (call::CodeHostPulls::METHOD, &[Topic::CodeHost]),
            Read::WorktreeMerged => (call::WorktreeMerged::METHOD, &[Topic::Git]),
            Read::SignIn => (call::AccountSignIn::METHOD, &[]),
            Read::SignInFinished => (call::AccountSignInFinish::METHOD, &[]),
            Read::ProjectFiles => (call::ProjectFiles::METHOD, &[]),
            Read::FilesList => (call::FilesList::METHOD, &[]),
            Read::PickerFolders => (call::ProjectRoots::METHOD, &[]),
            Read::SshHosts => (call::SshHosts::METHOD, &[]),
            Read::HostStats => (call::HostStats::METHOD, &[Topic::HostStats]),
            Read::Outside => (call::OutsideAccess::METHOD, &[Topic::Outside]),
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
    /// The account's Stash, as this host last listed it.
    pub stash: Option<ShownStash>,
    pub themes: Vec<ThemeSummary>,
    /// Activity across every project, for Settings; `Some(None)` when the core refused it.
    pub activity_everywhere: Option<Option<ActivitySummary>>,
    /// What the project and host pickers offer: folders under the project roots, and SSH hosts.
    pub picker_folders: Vec<RootEntry>,
    pub ssh_hosts: Vec<SshHost>,
    /// The programs outside Sikemux this host lets in, and those waiting to be.
    pub outside: Option<OutsideView>,
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
}

impl Workbench {
    /// Takes in what the core answered `read`, then does what follows from it.
    pub(crate) fn read_answered(
        &mut self,
        place: Place,
        read: Read,
        value: Value,
        cx: &mut Context<Self>,
    ) {
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
                self.follow_folded(cx);
                self.watch_hosts_page();
            }
            Read::SkillsAdded => {
                if let Some(added) = decoded(value, "skills added") {
                    self.offer_skills(added, cx);
                }
                return;
            }
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
                    // An AppImage runs from a mount that goes with this process, and an unpacked one
                    // from a folder the update replaced, so each starts again from its own launcher.
                    if let Some(launcher) = appimage_launcher() {
                        cx.set_restart_path(launcher);
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
            Read::Questions => {
                let Some(asked) = decoded::<Vec<AgentQuestion>>(value, "questions") else {
                    return cx.notify();
                };
                self.feeds.at_mut(&place).questions = (asked.into_iter())
                    .map(|asked| Question { asked })
                    .collect();
            }
            Read::Theme => {
                if let Some(palette) = decoded(value, "theme") {
                    theme::apply(&palette, cx);
                    crate::syntax::restyle(cx);
                    for pane in self.mod_panes.values() {
                        pane.update(cx, |pane, cx| pane.retheme(cx));
                    }
                    cx.refresh_windows();
                }
            }
            Read::Picture => self
                .picture
                .read(decoded(value, "account picture").flatten()),
            Read::SignIn => match decoded::<SignInPage>(value, "sign-in page") {
                Some(page) => {
                    cx.open_url(&page.url);
                    Target::here(&self.asks)
                        .answer::<call::AccountSignInFinish>(Read::SignInFinished, NoParams {});
                }
                None => self.signing_in = false,
            },
            Read::SignInFinished => {
                self.signing_in = false;
                crate::pairing::offer_account_phones(self, cx);
            }
            Read::FilesList => {
                self.files_retry.answered();
                let listed = decoded(value, "file list").unwrap_or_default();
                self.files.listed(listed, &self.target());
                return self.parts.redraw(&self.parts.tabs, cx);
            }
            Read::GitProjectRepo => {
                let was = self.git_repo();
                if let Some(found) = decoded(value, "project repository") {
                    self.feeds.at_mut(&place).git_project_repo = Some(found);
                }
                return self.follow_git_repo(was, cx);
            }
            Read::GitOverview => {
                // An overview that does not read is kept too, which the Git pane says.
                let in_front = self.git_in_front();
                let feed = self.feeds.at_mut(&place);
                feed.git_overview =
                    Some(serde_json::from_value(value).map_err(|error| error.to_string()));
                let overview = feed.git_overview.as_ref();
                if let Some(Ok(overview)) = overview {
                    self.files.mark(overview);
                }
                if let Some(overview) = overview {
                    self.git.overview(overview, in_front, cx);
                }
                self.parts.redraw(&self.parts.top, cx);
                return self.parts.redraw(&self.parts.tabs, cx);
            }
            Read::ProjectFiles => {
                let answered = decoded(value, "project files")
                    .is_some_and(|answer| self.project_files.answered(answer));
                if answered {
                    self.rank(cx);
                    self.files_listed(cx);
                }
                return;
            }
            Read::Preferences => {
                if let Some(preferences) = decoded::<Preferences>(value, "preferences") {
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
                    if crate::chat::column::set(preferences.chat_width, cx) {
                        for chat in self.chats.open.values() {
                            chat.widen(cx);
                        }
                    }
                    crate::ui::grain::set(&preferences, cx);
                    crate::theme::set_text_scale(preferences.ui_text_scale as f32, cx);
                    if crate::i18n::speak(preferences.language.as_deref()) {
                        cx.refresh_windows();
                    }
                    crate::see_through::set(&preferences, cx);
                    crate::screens::experience::preferences_read(self, &preferences, cx);
                    self.preferences = Some(preferences);
                }
                self.terminals.redraw_all(cx);
                crate::notifications::introduce(self, cx);
                crate::notifications::kept_running(self, cx);
            }
            Read::GitDraft => {
                let feed = self.feeds.at_mut(&place);
                if !keep_decoded(&mut feed.git_draft, value, "drafts") {
                    return;
                }
                if let Some(drafts) = &feed.git_draft {
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
            Read::Drafts => {
                let elsewhere = place != Place::This;
                let feed = self.feeds.at_mut(&place);
                if !keep_decoded(&mut feed.drafts, value, "drafts") && !elsewhere {
                    return;
                }
                if let Some(drafts) = &feed.drafts {
                    crate::chat::draft::take(&mut self.chats, drafts, elsewhere, cx);
                }
            }
            Read::Usage => {
                crate::agents::limits::usage_read(self, cx);
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
                let kept = &mut self.feeds.at_mut(&place).mods;
                if !read.is_some_and(|read| keep(kept, Some(read))) {
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
            Read::Trust => {
                let kept = &mut self.feeds.at_mut(&place).trust;
                if !keep_decoded(kept, value, "trust questions") && place == Place::This {
                    return;
                }
                self.ask_trust(cx);
            }
            Read::Tasks => {
                if !keep_decoded(&mut self.feeds.at_mut(&place).tasks, value, "tasks") {
                    return;
                }
            }
            Read::ProjectConfig => {
                if let Some(config) = decoded(value, "project config") {
                    crate::keymap::bind_project_actions(&crate::commands::action_keys(&config), cx);
                }
            }
            Read::Runs => {
                if !keep_decoded(&mut self.feeds.at_mut(&place).runs, value, "process runs") {
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
                if !keep(&mut self.feeds.at_mut(&place).activity, Some(summary)) {
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
                if !keep_decoded(&mut self.feeds.at_mut(&place).ports, value, "ports") {
                    return;
                }
                self.time_run(cx);
            }
            Read::WhatsNew => {
                let news = decoded::<WhatsNew>(value, "release notes");
                let unseen = news.is_some_and(|news| news.unseen);
                crate::screens::experience::whats_new_read(self, unseen, cx);
            }
            Read::PickerFolders => {
                keep_decoded(&mut self.reads.picker_folders, value, "project folders");
                self.picker_answered(cx);
            }
            Read::SshHosts => {
                keep_decoded(&mut self.reads.ssh_hosts, value, "SSH hosts");
                self.picker_answered(cx);
            }
            Read::WorktreePulls => {
                self.feeds.at_mut(&place).worktree_pulls =
                    decoded::<PluginOutcome>(value, "worktree's pulls").and_then(|told| {
                        told.read()
                            .inspect_err(|error| {
                                eprintln!("sikemux: the worktree's pulls: {error}")
                            })
                            .ok()
                    });
            }
            Read::WorktreeMerged => {
                self.feeds.at_mut(&place).worktree_merged = decoded(value, "worktree merged");
            }
            Read::Outside => {
                if !keep_decoded(&mut self.reads.outside, value, "outside access") {
                    return;
                }
                self.outside_read(cx);
            }
            Read::HostStats => {
                if let Some(stats) = decoded(value, "host stats") {
                    self.host_stats_read(place, stats, cx);
                }
                return;
            }
        }
        cx.notify();
    }

    pub(crate) fn read_refused(
        &mut self,
        place: Place,
        read: Read,
        error: String,
        cx: &mut Context<Self>,
    ) {
        eprintln!("sikemux: the core refused {read:?}: {error}");
        match read {
            Read::SignIn | Read::SignInFinished => {
                if std::mem::take(&mut self.signing_in) {
                    self.notify(
                        crate::toast::error(format!("Sikemux could not sign in: {error}"))
                            .autohide(false),
                        cx,
                    );
                }
            }
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
            Read::Activity => {
                self.feeds.at_mut(&place).activity = Some(ActivitySummary::default());
            }
            Read::ActivityEverywhere => {
                self.reads.activity_everywhere = Some(None);
                self.parts.redraw_stage(cx);
                self.parts.redraw_activity(cx);
            }
            _ => {}
        }
    }
}

fn appimage_launcher() -> Option<std::path::PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Some(appimage.into());
    }
    std::env::var_os("APPDIR").map(|folder| std::path::Path::new(&folder).join("AppRun"))
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

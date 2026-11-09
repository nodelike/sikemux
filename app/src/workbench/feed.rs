//! What a core sent: one path per update, whichever core it came from, and what follows it.

use sikemux_core::server::activity_log::ActivitySummaryParams;
use sikemux_core::server::call;
use sikemux_core::server::drafts::DraftsParams;
use sikemux_core::server::git::call as git_call;
use sikemux_core::server::ports::PortsParams;
use sikemux_core::server::tasks::ProjectParams;
use std::path::PathBuf;

use gpui_kit::Context;
use sikemux_view::changed::Topic;
use sikemux_view::device::DeviceView;

use crate::chat::{Chat, START};
use crate::core::{utc_offset_minutes, ChatAsk, ChatNews, Target, Update};
use crate::hosts::Place;
use crate::reads::Read;
use crate::toast;
use crate::workbench::Workbench;
use sikemux_core::server::layout::HostParams;

/// What the watches that follow the front were last made for.
#[derive(Default)]
pub(crate) struct Watched {
    /// The project the per-project watches were made for, by path.
    pub project: Option<String>,
    /// The host whose core those watches went to.
    pub place: Place,
    /// The agent in front whose worktree its header reads, by id and folder, and where it is.
    pub worktree: Option<(String, String, Place)>,
}

impl Workbench {
    pub(crate) fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        #[cfg(feature = "drive")]
        if let Update::Read(_, key, _) = &update {
            crate::drive::trace(format_args!("main read {key:?}"));
        }
        match update {
            Update::Connected(version) => {
                // The app opens on this host, as the Mac's does, whichever host was last in front.
                if self.connection.connected(version) {
                    Target::here(&self.asks).call_typed::<call::ChooseHost>(HostParams {
                        host: "this".to_owned(),
                    });
                    self.usage_wanted = true;
                }
                self.on_screen = None;
                self.desk_news.sent = None;
                self.desks_moved(cx);
                for (agent, chat) in self.chats.open.iter().filter(|(_, chat)| !chat.resting) {
                    self.ask_chat(ChatAsk::Attach {
                        agent: agent.clone(),
                        since: chat.mark.clone(),
                    });
                }
                self.terminals.reattach(&self.asks);
                self.files.relist_shown(&self.target());
                crate::editor::reconnected(self, cx);
                self.boards_moved(cx);
            }
            Update::View(place, view) => return self.view_arrived(place, *view, cx),
            Update::HostReach(host, reached) => {
                if reached {
                    self.feeds.host_mut(&host).reached = true;
                    crate::editor::host_reached(self, &Place::Account(host.clone()), cx);
                    self.read_approval(Some(host.clone()), cx);
                } else {
                    self.feeds.host_mut(&host).reached = false;
                }
                self.parts.redraw_stage(cx);
            }
            Update::HostsUnavailable(reason) => self.feeds.unavailable = Some(reason),
            Update::HostHome(host, home) => {
                self.feeds.host_mut(&host).home = Some(home);
            }
            Update::Chat(agent, news) => {
                match news {
                    ChatNews::Found { asked, found } => self.found(&agent, asked, found, cx),
                    news => match self.chats.open.get_mut(&agent) {
                        Some(chat) => chat.hear(&agent, news, cx),
                        None => return,
                    },
                }
                let restage = self
                    .chats
                    .open
                    .get_mut(&agent)
                    .is_some_and(Chat::take_head_moved);
                if restage && self.agent().is_some_and(|front| front.id == agent) {
                    self.parts.redraw_stage(cx);
                }
                return;
            }
            Update::Failed(error) => {
                return self.notify(toast::error(error).autohide(false), cx);
            }
            Update::Read(place, read, value) => return self.read_answered(place, read, value, cx),
            Update::Refused(place, read, error) => self.read_refused(place, read, error, cx),
            Update::Terminal(place, session, screenful) => {
                let key = self.terminals.key(&place, session);
                self.terminals.apply(key, screenful, cx);
                return self.terminals.redraw(key, cx);
            }
            Update::Lost(reason) => self.connection.lose(reason),
            Update::Stopped => self.connection.stop(),
            Update::Stuck(pid) => self.connection.hang(pid),
            Update::ShutDown => return cx.quit(),
            Update::Notice {
                title,
                body,
                agent,
                goal,
            } => {
                return crate::notifications::notice(self, title, body, agent, goal, cx);
            }
            Update::FocusAgent(agent) => {
                cx.activate(true);
                return self.open_agent(&agent, cx);
            }
            Update::AccountDevice { key, name } => return self.new_account_device(key, name, cx),
            Update::Network(change) => {
                return self
                    .voice
                    .update(cx, |voice, cx| voice.network_changed(change, cx));
            }
            Update::TerminalExited(session, code) => {
                crate::agents::setup::exited(self, session, code, cx);
            }
            Update::Remote(status) => {
                let was_required = self.remote.as_ref().map(|remote| &remote.update_required);
                if was_required != Some(&status.update_required) {
                    cx.notify();
                }
                self.remote = Some(*status);
                crate::settings::remote_moved(self, cx);
                crate::pairing::remote_moved(self, cx);
                return self.parts.redraw_stage(cx);
            }
            Update::WindowCall(call_id, call) => self.answer_window_call(call_id, call, cx),
            Update::Changed {
                place,
                topic,
                project,
                paths,
            } => return self.core_changed(place, topic, project, paths, cx),
        }
        cx.notify();
    }

    /// Takes in the view of the core at `place`.
    pub(crate) fn view_arrived(&mut self, place: Place, view: DeviceView, cx: &mut Context<Self>) {
        let here = place == Place::This;
        if !here && self.view_at(&place) == Some(&view) {
            return;
        }
        self.mods_follow(&place, crate::mod_pane::Moved::View, cx);
        let settled_before = crate::agents::settled::auto_settled(self.view_at(&place));
        match &place {
            Place::This => {
                crate::ui::moving_face::set_on(view.face_style.motion, cx);
                let account_moved =
                    self.this_view().map(|shown| &shown.account) != Some(&view.account);
                self.feeds.this.view = Some(view);
                if account_moved {
                    self.parts.redraw(&self.parts.rail, cx);
                }
                self.forget_departed(cx);
            }
            Place::Account(host) => {
                self.feeds.host_mut(host).view = Some(view);
            }
        }
        crate::agents::settled::tell(self, &place, &settled_before, cx);
        self.settle_closing();
        self.keep_terminals();
        crate::editor::keep(self, cx);
        if here {
            // The core finds its agents a moment after it starts, so the plan limits are
            // read once it lists them.
            if self.usage_wanted && !self.launchers().is_empty() {
                self.usage_wanted = false;
                self.read_usage();
            }
            crate::agents::setup::settle(self, cx);
            self.settle_open_waits();
            crate::notifications::badge(self);
            crate::notifications::kept_running(self, cx);
            self.note_place();
            self.watch_picture();
        }
        // This host's view says which host is in front, so it can move what the front shows.
        if here || self.place() == place {
            self.watch_project();
            self.watch_worktree();
            self.close_chats(cx);
            self.open_chat(cx);
            self.rest_chats_later(cx);
            self.look_for_older(false, crate::pickers::recent::PAGE, cx);
            self.front_moved(cx);
            self.files.reveal(self.front_file(), &self.target());
            crate::settings::front_moved(self, cx);
        }
        self.publish_on_screen();
        cx.notify();
    }

    /// Ends the terminals their core no longer lists.
    pub(crate) fn keep_terminals(&mut self) {
        let feeds = &self.feeds;
        let sessions = (feeds.this.view.as_ref()).map_or(&[][..], |view| view.sessions.as_slice());
        let setup = &self.agent_setup;
        self.terminals.keep(
            |place, id| match place {
                Place::This => sessions.iter().any(|session| session.id == id) || setup.shows(id),
                Place::Account(host) => feeds
                    .host(host)
                    .and_then(|feed| feed.view.as_ref())
                    .is_none_or(|view| view.sessions.iter().any(|session| session.id == id)),
            },
            &self.asks,
        );
    }

    /// Tells what follows it that a topic moved on the core at `place`.
    pub(crate) fn core_changed(
        &mut self,
        place: Place,
        topic: Topic,
        project: Option<String>,
        paths: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if topic == Topic::Files {
            crate::editor::changed(self, &place, paths.as_deref().unwrap_or_default(), cx);
        }
        self.project_changed(&place, topic, project.as_deref(), paths.as_deref(), cx);
        if topic == Topic::Mods {
            self.mods_moved(&place, paths.as_deref(), cx);
        }
        self.mods_follow(&place, crate::mod_pane::Moved::Topic(topic), cx);
        if topic == Topic::Git {
            crate::editor::git_changed(self, &place, project.as_deref(), paths.as_deref(), cx);
        }
        if place != Place::This {
            return;
        }
        if topic == Topic::Boards {
            self.boards_moved(cx);
        }
        if topic == Topic::Memory {
            if let Some(goals) = &self.goals.pane {
                goals.update(cx, |goals, cx| goals.changed(cx));
            }
        }
        if topic == Topic::GoalSummary {
            if let Some(goals) = &self.goals.pane {
                goals.update(cx, |goals, cx| goals.summary_moved(cx));
            }
        }
        if topic == Topic::Goals {
            self.goals_moved(cx);
        }
        if topic == Topic::Lsp {
            crate::editor::diagnostics_changed(self, paths.as_deref().unwrap_or_default(), cx);
        }
        if topic == Topic::AgentSignIn {
            crate::accounts::pages_moved(self, cx);
        }
        if topic == Topic::AgentAccounts {
            crate::accounts::book_moved(self, cx);
        }
        if topic == Topic::AgentStatus {
            crate::accounts::statuses_moved(self, cx);
        }
    }

    /// Has the board and the goals read what the core holds again.
    fn boards_moved(&mut self, cx: &mut Context<Self>) {
        if let Some(board) = &self.board.pane {
            board.update(cx, |board, cx| board.changed(cx));
        }
        if let Some(goals) = &self.goals.pane {
            goals.update(cx, |goals, cx| goals.changed(cx));
        }
        self.goals_moved(cx);
    }

    fn goals_moved(&mut self, cx: &mut Context<Self>) {
        if let Some(goals) = &self.goals.pane {
            let on_screen = self.showing_goals();
            let focused = self.rails.goal_focus.is_some();
            goals.update(cx, |goals, cx| goals.goals_moved(on_screen, focused, cx));
        }
    }

    /// Reads the file tree, the finder's list and search again when `topic` moved on
    /// `place`, while the project in front is on that host.
    pub(crate) fn project_changed(
        &mut self,
        place: &Place,
        topic: Topic,
        project: Option<&str>,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        if self.place() != *place {
            return;
        }
        if topic == Topic::Git {
            match (&self.git.pane, self.git_in_front()) {
                (Some(pane), true) => {
                    pane.update(cx, |pane, cx| pane.git_moved(project, paths, cx))
                }
                (Some(_), false) => self.git.behind = true,
                (None, _) => {}
            }
        }
        if topic == Topic::Git {
            let root = (self.files.root.as_ref()).map(|root| root.to_string_lossy().into_owned());
            if project.is_none() || project == root.as_deref() {
                if self.files.changed(paths) {
                    self.parts.redraw(&self.parts.tabs, cx);
                }
                if self.overlays.finder.is_some() {
                    self.project_files.read(root.as_deref(), &self.target());
                }
            }
        }
        if topic == Topic::Files {
            if let Some(search) = &self.search {
                search.update(cx, |search, cx| search.changed(cx));
            }
        }
    }

    /// Points the watches that follow the project in front at it.
    pub(crate) fn watch_project(&mut self) {
        let path = self
            .project()
            .map(|project| project.path.to_string_lossy().into_owned());
        let place = self.place();
        if path == self.watched.project && place == self.watched.place {
            return;
        }
        if place != self.watched.place {
            self.leave_place(&place);
        }
        let target = Target::new(&self.asks, place.clone());
        target.watch::<call::ActivitySummary>(
            Read::Activity,
            ActivitySummaryParams {
                utc_offset_minutes: i64::from(utc_offset_minutes()),
                project: path.clone(),
            },
        );
        target.watch_plain(Read::Mods);
        let feed = self.feeds.at_mut(&place);
        feed.git_project_repo = None;
        feed.git_draft = None;
        self.git.listing = false;
        match &path {
            Some(path) => {
                let of_project = || ProjectParams {
                    project: path.clone(),
                };
                target.watch::<call::Tasks>(Read::Tasks, of_project());
                target.watch_plain(Read::Runs);
                target.watch::<call::ProjectConfig>(Read::ProjectConfig, of_project());
                target.watch::<call::Ports>(
                    Read::Ports,
                    PortsParams {
                        project: Some(path.clone()),
                    },
                );
                target.watch_folder::<git_call::ProjectRepo>(
                    Read::GitProjectRepo,
                    git_call::ProjectRepo {
                        project: path.clone(),
                    },
                    path.clone(),
                );
            }
            None => {
                for read in [
                    Read::Tasks,
                    Read::Runs,
                    Read::ProjectConfig,
                    Read::Ports,
                    Read::GitProjectRepo,
                ] {
                    target.unwatch(read);
                }
                let feed = self.feeds.at_mut(&place);
                feed.tasks = None;
                feed.runs.clear();
                feed.ports.clear();
            }
        }
        self.files.show(path.clone().map(PathBuf::from), &target);
        self.watched.project = path;
        self.watched.place = place;
        self.watch_drafts();
    }

    /// Stops the core the window watched for the project in front, as `place` takes over, and
    /// starts on what an account host in front is watched for.
    fn leave_place(&mut self, place: &Place) {
        let left = Target::new(&self.asks, self.watched.place.clone());
        for read in Read::OF_THE_PROJECT_IN_FRONT {
            left.unwatch(read);
        }
        self.feeds.at_mut(&self.watched.place).leave();
        if self.watched.place != Place::This {
            for read in Read::OF_THE_HOST_IN_FRONT {
                left.unwatch(read);
            }
        }
        match place {
            Place::Account(host) => {
                self.feeds.set_front(Some(host.clone()));
                let target = Target::new(&self.asks, place.clone());
                target.watch_plain(Read::Trust);
                target.watch_plain(Read::Questions);
            }
            Place::This => self.feeds.set_front(None),
        }
    }

    /// Keeps the core answering the drafts on show: the commit message of the project in
    /// front, and every open chat's.
    pub(crate) fn watch_drafts(&self) {
        let keys = |elsewhere: bool| -> Vec<String> {
            let chats = (self.chats.open.iter())
                .filter(|(_, chat)| {
                    chat.composer.is_some() && crate::chat::draft::is_elsewhere(chat) == elsewhere
                })
                .map(|(agent, _)| agent.as_str());
            let start = self
                .chats
                .start
                .as_ref()
                .filter(|_| !elsewhere)
                .map(|_| START);
            chats
                .chain(start)
                .map(sikemux_view::chat::chat_draft_key)
                .collect()
        };
        let of_keys = |keys: Vec<String>| Some(DraftsParams { keys: Some(keys) });
        Target::here(&self.asks).watch::<call::Drafts>(Read::Drafts, of_keys(keys(false)));
        let there = keys(true);
        let place = self.place();
        if place != Place::This {
            let target = Target::new(&self.asks, place);
            match there.is_empty() {
                true => target.unwatch(Read::Drafts),
                false => target.watch::<call::Drafts>(Read::Drafts, of_keys(there)),
            }
        }
    }

    /// Asks for the account's picture when it is one this window has not got.
    pub(crate) fn watch_picture(&mut self) {
        let hash = self
            .this_view()
            .and_then(|view| view.account.as_ref()?.picture_hash.clone());
        self.picture.follow(hash, &self.asks);
    }
}

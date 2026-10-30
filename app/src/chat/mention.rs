//! The menu the draft opens over itself while a word starts with `@`, `+`, `/` or `&`: the
//! agents to name, the project's files and folders to hand over, the session's commands,
//! or what waits in the Stash, which goes into the draft as an attachment. A named agent, earlier chat or file goes into the draft as a token, which
//! is what the message sends: the agent or chat for the core to describe (an earlier
//! chat the core opens again first), the file as a path.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::input::InlineToken;
use gpui_kit::component::input::TextareaState;
use gpui_kit::{
    px, App, AppContext, Context, Focusable, IntoElement, ScrollHandle, SharedString, Styled, Task,
    Window,
};
use sikemux_core::server::plugins::{Category, Outcome};
use sikemux_core::server::{
    call, ClosedAgentsParams, ReleasesParams, SearchParams, ShownStashItem, TrackedParams,
};
use sikemux_plugin_api::codehost::{TrackedItem, TrackedKind};
use sikemux_wire::protocol::{ChatContext, ProjectInfo};

use super::Composing;
use crate::icons::Icon;
use crate::ui::{self, DockMenu, DockRow, DockScope};
use crate::workbench::Workbench;

const SHOWN: usize = 8;
/// How many stash items the tray over the draft shows.
const STASH_SHOWN: usize = 8;
const AGENT: &str = "agent:";
const FILE: &str = "file:";
const FOLDER: &str = "folder:";
const EARLIER: &str = "earlier:";
const TRACKED: &str = "tracked:";
const COMMAND: &str = "command:";
/// Earlier chats the core is asked for, and how long it may read transcripts for them.
const EARLIER_SHOWN: usize = 4;
const EARLIER_BUDGET_MS: u64 = 600;
/// How long the person's typing must settle before the core searches earlier chats.
const SETTLE: Duration = Duration::from_millis(150);

/// A word the caret is at the end of that starts with one of the menu's triggers.
#[derive(Clone, PartialEq)]
struct Typed {
    trigger: char,
    /// Where the word starts, in bytes.
    start: usize,
    needle: String,
}

fn typed(text: &str, caret: usize) -> Option<Typed> {
    let before = text.get(..caret)?;
    let start = before.rfind(char::is_whitespace).map_or(0, |at| at + 1);
    let mut word = before[start..].chars();
    let trigger = word
        .next()
        .filter(|first| matches!(first, '@' | '+' | '/' | '#' | '!' | '&'))?;
    Some(Typed {
        trigger,
        start,
        needle: word.as_str().to_owned(),
    })
}

/// What the menu offers.
#[derive(Clone)]
enum Choice {
    Mate {
        id: String,
        name: String,
        title: String,
        project: Option<String>,
        provider: String,
    },
    Entry {
        path: String,
        folder: bool,
    },
    Command {
        name: String,
        description: String,
        hint: Option<String>,
    },
    /// A saved chat no agent holds, which the core opens again for the message: one
    /// the person named and closed, or one found by what was said in it.
    Earlier {
        cli: String,
        session: String,
        name: Option<String>,
        title: String,
        /// When the chat last moved on, in Unix milliseconds, for one found by search.
        active_at: Option<u64>,
    },
    Tracked(TrackedItem),
    /// An item of the Stash, as the core lists it.
    Stashed(Box<ShownStashItem>),
}

/// The code host's open issues and pull requests, as `codeHostTracked` answers.
pub enum Tracked {
    Reading,
    Failed(SharedString),
    Listed(Rc<[TrackedItem]>),
}

/// What `#` lists issues and `!` pull requests by: the start of the number when the
/// needle is all digits, else every word of it in the title.
fn tracked_choices(tracked: Option<&Tracked>, trigger: char, needle: &str) -> Vec<Choice> {
    let Some(Tracked::Listed(items)) = tracked else {
        return Vec::new();
    };
    let kind = if trigger == '!' {
        TrackedKind::Pull
    } else {
        TrackedKind::Issue
    };
    let query = needle.trim().to_lowercase();
    let digits = !query.is_empty() && query.bytes().all(|byte| byte.is_ascii_digit());
    let words: Vec<&str> = query.split_whitespace().collect();
    items
        .iter()
        .filter(|item| item.kind == kind)
        .filter(|item| match digits {
            true => item.number.to_string().starts_with(&query),
            false => {
                let title = item.title.to_lowercase();
                words.iter().all(|word| title.contains(word))
            }
        })
        .take(SHOWN)
        .cloned()
        .map(Choice::Tracked)
        .collect()
}

/// What `#` or `!` says while it lists nothing, or none while the list is on its way.
fn tracked_empty(
    project: Option<&ProjectInfo>,
    tracked: Option<&Tracked>,
    trigger: char,
) -> Option<String> {
    let (things, matches) = match trigger {
        '!' => ("pull requests", "No open pull request matches"),
        _ => ("issues", "No open issue matches"),
    };
    let Some(project) = project else {
        return Some(format!("Open a project to pick its {things}"));
    };
    if project.code_host.is_none() {
        return Some("This project has no remote on a code host".into());
    }
    match tracked {
        Some(Tracked::Failed(why)) => Some(why.to_string()),
        Some(Tracked::Listed(_)) => Some(matches.into()),
        _ => None,
    }
}

/// A closed agent the person named, as the core's `closedAgents` lists it.
/// The menu on show: the word it answers, what it offers for it, and the one picked.
pub struct Menu {
    typed: Typed,
    choices: Rc<[Choice]>,
    selected: usize,
    /// What the menu says while it has no choices, where that depends on more than the trigger.
    empty: Option<SharedString>,
    /// The Stash's tray, opened by its button rather than by a word typed in the draft.
    pinned: bool,
    /// The stash items that were new when the tray opened.
    fresh: Rc<[String]>,
    /// Where the tray's rows are scrolled, so the keys keep the picked one in sight.
    scroll: ScrollHandle,
    _ranking: Option<Task<()>>,
}

/// The Stash's tray as the menu holds it: its items, the one picked, those new when it opened,
/// and what was typed after `&`.
pub struct Tray<'a> {
    pub items: Vec<&'a ShownStashItem>,
    pub selected: usize,
    pub fresh: &'a [String],
    pub needle: &'a str,
    pub scroll: ScrollHandle,
}

impl Menu {
    /// Whether the caret is still at the end of the word the menu was made for.
    pub fn answers(&self, draft: &TextareaState) -> bool {
        self.pinned || typed(&draft.value(), draft.cursor()).as_ref() == Some(&self.typed)
    }

    /// The Stash's tray, when the menu is it.
    pub fn tray(&self) -> Option<Tray<'_>> {
        (self.typed.trigger == '&').then(|| Tray {
            items: self
                .choices
                .iter()
                .filter_map(|choice| match choice {
                    Choice::Stashed(item) => Some(&**item),
                    _ => None,
                })
                .collect(),
            selected: self.selected,
            fresh: &self.fresh,
            needle: &self.typed.needle,
            scroll: self.scroll.clone(),
        })
    }
}

impl Composing {
    /// What the message sends besides its text: its attachments and the files it names,
    /// and the agents it names, for the core to describe.
    pub fn outgoing(&self, cx: &App) -> (Vec<String>, Vec<ChatContext>) {
        let mut paths: Vec<String> = self
            .attachments
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let mut context = Vec::new();
        for span in self.draft.read(cx).tokens() {
            let token = span.token();
            if let Some(item) = self.tracked_context.get(token.id().as_ref()) {
                if !context
                    .iter()
                    .any(|held: &ChatContext| held.uri == item.uri)
                {
                    context.push(item.clone());
                }
            } else if let Some(path) = token
                .id()
                .strip_prefix(FILE)
                .or_else(|| token.id().strip_prefix(FOLDER))
            {
                if !paths.iter().any(|held| held == path) {
                    paths.push(path.to_owned());
                }
            } else if let Some(uri) = token
                .id()
                .strip_prefix(AGENT)
                .map(|agent| format!("sikemux://agent/{agent}"))
                .or_else(|| {
                    let chat = token.id().strip_prefix(EARLIER)?;
                    Some(format!("sikemux://earlier/{chat}"))
                })
            {
                if !context.iter().any(|item: &ChatContext| item.uri == uri) {
                    context.push(ChatContext {
                        uri,
                        title: token.text().to_string(),
                        text: String::new(),
                    });
                }
            }
        }
        for item in &self.context {
            if !context
                .iter()
                .any(|held: &ChatContext| held.uri == item.uri)
            {
                context.push(item.clone());
            }
        }
        paths.truncate(sikemux_core::server::chat::prompt::MAX_ATTACHMENTS);
        (paths, context)
    }
}

impl Workbench {
    /// Opens, changes or closes the menu of `agent`'s draft after the person changed it.
    pub fn offer(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(composing) = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| chat.composer.as_ref())
        else {
            return;
        };
        let draft = composing.draft.read(cx);
        let typed = typed(&draft.value(), draft.cursor());
        let kept = composing.menu.as_ref().map(|menu| &menu.typed);
        let pinned = composing.menu.as_ref().is_some_and(|menu| menu.pinned);
        if typed.as_ref() == kept && !pinned {
            return;
        }
        let menu = typed.map(|typed| self.menu(agent, typed, cx));
        if let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        {
            composing.menu = menu;
        }
        cx.notify();
    }

    fn menu(&mut self, agent: &str, typed: Typed, cx: &mut Context<Self>) -> Menu {
        let needle = typed.needle.to_lowercase();
        let choices: Rc<[Choice]> = match typed.trigger {
            '@' => self.mates(agent, &needle).into(),
            '/' => self
                .chats
                .open
                .get(agent)
                .and_then(|chat| chat.state.as_ref())
                .map_or(Vec::new(), |head| {
                    head.commands
                        .iter()
                        .filter(|command| command.name.to_lowercase().contains(&needle))
                        .take(SHOWN)
                        .map(|command| Choice::Command {
                            name: command.name.clone(),
                            description: command.description.clone(),
                            hint: command.hint.clone(),
                        })
                        .collect()
                })
                .into(),
            '#' | '!' => {
                let tracked = self.tracked_of(agent);
                tracked_choices(tracked, typed.trigger, &typed.needle).into()
            }
            '&' => {
                let stash = crate::stash::Stash::of(self);
                let fresh = stash.new_items();
                let (mut new, old): (Vec<&ShownStashItem>, Vec<&ShownStashItem>) = stash
                    .items
                    .iter()
                    .filter(|shown| crate::stash::matches(&shown.item, &typed.needle))
                    .partition(|item| fresh.contains(item));
                new.extend(old);
                new.into_iter()
                    .take(STASH_SHOWN)
                    .map(|item| Choice::Stashed(Box::new(item.clone())))
                    .collect::<Vec<_>>()
                    .into()
            }
            _ => Rc::from(Vec::new()),
        };
        let ranking = match typed.trigger {
            '+' => Some(self.rank_entries(agent, typed.needle.clone(), cx)),
            '@' => self.look_back(agent, typed.needle.clone(), cx),
            '#' | '!' => self.read_tracked(agent, cx),
            _ => None,
        };
        let empty = matches!(typed.trigger, '#' | '!')
            .then(|| tracked_empty(self.project(), self.tracked_of(agent), typed.trigger))
            .flatten()
            .map(SharedString::from);
        let fresh: Rc<[String]> = match typed.trigger {
            '&' => {
                let fresh = crate::stash::Stash::of(self)
                    .new_items()
                    .iter()
                    .map(|shown| shown.item.id.clone())
                    .collect();
                self.see_stash();
                fresh
            }
            _ => Rc::from(Vec::new()),
        };
        Menu {
            typed,
            choices,
            selected: 0,
            empty,
            pinned: false,
            fresh,
            scroll: ScrollHandle::new(),
            _ranking: ranking,
        }
    }

    /// Opens the Stash's tray over the draft of `agent`'s chat, with the keys in the draft so the
    /// arrows, Return and Escape work in it, or closes it.
    pub fn toggle_stash_tray(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        composing.draft.read(cx).focus_handle(cx).focus(window, cx);
        if composing.menu.as_ref().is_some_and(|menu| menu.pinned) {
            composing.menu = None;
            return cx.notify();
        }
        let start = composing.draft.read(cx).cursor();
        let typed = Typed {
            trigger: '&',
            start,
            needle: String::new(),
        };
        let mut menu = self.menu(agent, typed, cx);
        menu.pinned = true;
        if let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        {
            composing.menu = Some(menu);
        }
        cx.notify();
    }

    fn tracked_of(&self, agent: &str) -> Option<&Tracked> {
        self.chats
            .open
            .get(agent)?
            .composer
            .as_ref()?
            .tracked
            .as_ref()
    }

    /// Asks the chat's core for its project's open issues and pull requests, the first time
    /// `#` or `!` is typed, and lists them in the menu once they are in.
    fn read_tracked(&mut self, agent: &str, cx: &mut Context<Self>) -> Option<Task<()>> {
        if self.tracked_of(agent).is_some() {
            return None;
        }
        let project = self.project()?;
        let host = project.code_host.as_ref()?.label.clone();
        let path = project.path.to_string_lossy().into_owned();
        let answer = (self.chat_target(agent))
            .request_typed::<call::CodeHostTracked>(ReleasesParams { project: path });
        let composing = self.chats.open.get_mut(agent)?.composer.as_mut()?;
        composing.tracked = Some(Tracked::Reading);
        let agent = agent.to_owned();
        Some(cx.spawn(async move |this, cx| {
            let tracked = match answer.await {
                Ok(Outcome::Failed(failed)) if failed.category == Category::Auth => {
                    Tracked::Failed(
                        format!("Sign in to {host} in the Git pane to pick from its issues").into(),
                    )
                }
                Ok(Outcome::Failed(failed)) => Tracked::Failed(failed.message.into()),
                Ok(Outcome::Value(items)) => Tracked::Listed(
                    serde_json::from_value::<Vec<TrackedItem>>(items)
                        .unwrap_or_default()
                        .into(),
                ),
                Err(error) => Tracked::Failed(error.into()),
            };
            this.update(cx, |bench, cx| {
                let project = bench.project().cloned();
                let Some(composing) = bench
                    .chats
                    .open
                    .get_mut(&agent)
                    .and_then(|chat| chat.composer.as_mut())
                else {
                    return;
                };
                if let Some(menu) = composing
                    .menu
                    .as_mut()
                    .filter(|menu| matches!(menu.typed.trigger, '#' | '!'))
                {
                    let trigger = menu.typed.trigger;
                    menu.choices =
                        tracked_choices(Some(&tracked), trigger, &menu.typed.needle).into();
                    menu.empty =
                        tracked_empty(project.as_ref(), Some(&tracked), trigger).map(Into::into);
                    menu.selected = 0;
                }
                composing.tracked = Some(tracked);
                cx.notify();
            })
            .ok();
        }))
    }

    /// The other named agents whose name starts with `needle`: the chat's own project's
    /// first, then every other project's.
    fn mates(&self, agent: &str, needle: &str) -> Vec<Choice> {
        let own = self
            .shown()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))
            .map(|saved| saved.project.clone());
        let mut projects: Vec<(&str, Option<String>)> = Vec::new();
        if let Some(own) = &own {
            projects.push((own, None));
        }
        projects.extend(
            self.projects()
                .iter()
                .filter(|project| Some(&project.id) != own.as_ref())
                .map(|project| (project.id.as_str(), Some(project.name.clone()))),
        );
        projects
            .into_iter()
            .flat_map(|(project, name)| {
                self.agents_of(project).into_iter().filter_map(move |row| {
                    let mate = row.name.clone().filter(|mate| {
                        row.id != agent && mate.to_lowercase().starts_with(needle)
                    })?;
                    Some(Choice::Mate {
                        id: row.id,
                        name: mate,
                        title: row.title,
                        project: name.clone(),
                        provider: row.provider,
                    })
                })
            })
            .take(SHOWN)
            .collect()
    }

    /// Asks the core for the closed agents the person named and the earlier chats that
    /// match `needle`, in the chat's own project, and offers them after the agents open now.
    fn look_back(
        &mut self,
        agent: &str,
        needle: String,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let view = self.shown()?;
        let own = view
            .workspace
            .agents
            .iter()
            .find(|saved| saved.id == agent)?;
        let cwd = self
            .projects()
            .iter()
            .find(|project| project.id == own.project)?
            .path
            .clone();
        let open: Vec<(String, String)> = view
            .workspace
            .agents
            .iter()
            .filter_map(|saved| Some((saved.provider.clone(), saved.resume_id.clone()?)))
            .collect();
        let offered: Vec<String> = self
            .reads
            .catalog
            .agents()
            .map(|kind| kind.kind.clone())
            .collect();
        let target = self.chat_target(agent);
        let closed = target.request_typed::<call::ClosedAgents>(ClosedAgentsParams {
            cwd: PathBuf::from(&cwd),
        });
        let wanted = needle.trim().to_lowercase();
        let search = (!wanted.is_empty()).then(|| {
            let params = SearchParams {
                query: wanted.clone(),
                projects: Some(vec![PathBuf::from(&cwd)]),
                limit: Some(EARLIER_SHOWN),
                budget_ms: Some(EARLIER_BUDGET_MS),
            };
            (target, params)
        });
        let agent = agent.to_owned();
        Some(cx.spawn(async move |this, cx| {
            let offer = |choices: Vec<Choice>, cx: &mut gpui_kit::AsyncApp| {
                this.update(cx, |bench: &mut Workbench, cx| {
                    let menu = bench
                        .chats
                        .open
                        .get_mut(&agent)
                        .and_then(|chat| chat.composer.as_mut())
                        .and_then(|composing| composing.menu.as_mut());
                    if let Some(menu) = menu.filter(|_| !choices.is_empty()) {
                        menu.choices = menu.choices.iter().cloned().chain(choices).collect();
                        cx.notify();
                    }
                })
                .ok();
            };
            let closed = closed.await.unwrap_or_default();
            let named: Vec<Choice> = closed
                .into_iter()
                .filter(|closed| closed.name.to_lowercase().starts_with(&wanted))
                .take(SHOWN)
                .map(|closed| Choice::Earlier {
                    cli: closed.provider,
                    session: closed.session_id,
                    title: closed.title.unwrap_or_default(),
                    name: Some(closed.name),
                    active_at: None,
                })
                .collect();
            let sessions: Vec<String> = named
                .iter()
                .filter_map(|choice| match choice {
                    Choice::Earlier { session, .. } => Some(session.clone()),
                    _ => None,
                })
                .collect();
            offer(named, cx);
            let Some((target, params)) = search else {
                return;
            };
            cx.background_executor().timer(SETTLE).await;
            let found =
                (target.request_typed::<call::AgentSessions>(params).await).unwrap_or_default();
            let found = found
                .into_iter()
                .filter(|found| offered.iter().any(|kind| *kind == found.cli))
                .filter(|found| !open.contains(&(found.cli.to_string(), found.session_id.clone())))
                .filter(|found| !sessions.contains(&found.session_id))
                .map(|found| Choice::Earlier {
                    cli: found.cli.into_owned(),
                    session: found.session_id,
                    name: None,
                    title: found.title,
                    active_at: Some(found.active_at),
                })
                .collect();
            offer(found, cx);
        }))
    }

    /// Ranks the project's files and their folders for `needle` off the main thread, and
    /// shows the ranking in `agent`'s menu once it is in.
    fn rank_entries(&mut self, agent: &str, needle: String, cx: &mut Context<Self>) -> Task<()> {
        let project = self
            .project()
            .map(|project| project.path.to_string_lossy().into_owned());
        if self.project_files.project != project || self.project_files.files.is_empty() {
            self.project_files
                .read(project.as_deref(), &self.chat_target(agent));
        }
        let files = self.project_files.files.clone();
        let ranking = cx.background_spawn(async move { ranked_entries(&needle, &files) });
        let agent = agent.to_owned();
        cx.spawn(async move |this, cx| {
            let entries = ranking.await;
            this.update(cx, |bench, cx| {
                if let Some(menu) = bench
                    .chats
                    .open
                    .get_mut(&agent)
                    .and_then(|chat| chat.composer.as_mut())
                    .and_then(|composing| composing.menu.as_mut())
                {
                    menu.choices = entries.into();
                    menu.selected = 0;
                    cx.notify();
                }
            })
            .ok();
        })
    }

    /// Ranks the menu of `agent`'s draft again, once the core has listed the project's files.
    pub fn files_listed(&mut self, cx: &mut Context<Self>) {
        let menus: Vec<(String, String)> = self
            .chats
            .open
            .iter()
            .filter_map(|(agent, chat)| {
                let typed = &chat.composer.as_ref()?.menu.as_ref()?.typed;
                (typed.trigger == '+').then(|| (agent.clone(), typed.needle.clone()))
            })
            .collect();
        for (agent, needle) in menus {
            let ranking = self.rank_entries(&agent, needle, cx);
            if let Some(menu) = self
                .chats
                .open
                .get_mut(&agent)
                .and_then(|chat| chat.composer.as_mut())
                .and_then(|composing| composing.menu.as_mut())
            {
                menu._ranking = Some(ranking);
            }
        }
    }

    /// Moves the menu's pick by `by`, round its ends.
    pub fn pick_choice(&mut self, agent: &str, by: isize, cx: &mut Context<Self>) {
        if let Some(menu) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
            .and_then(|composing| composing.menu.as_mut())
        {
            let count = menu.choices.len() as isize;
            if count > 0 {
                menu.selected = (menu.selected as isize + by).rem_euclid(count) as usize;
                menu.scroll.scroll_to_item(menu.selected);
                cx.notify();
            }
        }
    }

    /// Makes the word the menu answers ask for another kind of thing, as a click on its
    /// heading does, keeping what was typed after its key.
    pub fn switch_scope(
        &mut self,
        agent: &str,
        trigger: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        let Some(start) = (composing.menu.as_ref())
            .filter(|menu| menu.typed.trigger != trigger)
            .map(|menu| menu.typed.start)
        else {
            return;
        };
        composing.draft.update(cx, |draft, cx| {
            let caret = draft.cursor();
            draft.set_selected_range(start..start + 1, cx);
            draft.replace(trigger.to_string(), window, cx);
            draft.set_selected_range(caret..caret, cx);
        });
        self.offer(agent, cx);
    }

    pub fn dismiss_menu(&mut self, agent: &str, cx: &mut Context<Self>) {
        if let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        {
            composing.menu = None;
            cx.notify();
        }
    }

    /// Writes choice `index` of the menu over the word it answers.
    pub fn take_choice(
        &mut self,
        agent: &str,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project().map(|project| project.path.clone());
        let target = self.chat_target(agent);
        let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        let Some(menu) = composing.menu.take() else {
            return;
        };
        let Some(choice) = menu.choices.get(index.unwrap_or(menu.selected)).cloned() else {
            return;
        };
        let stashed = match &choice {
            Choice::Stashed(shown) => Some(shown.item.id.clone()),
            _ => None,
        };
        if let Some(id) = stashed.clone().filter(|_| menu.pinned) {
            self.add_from_stash(agent, vec![id], cx);
            return cx.notify();
        }
        if let (Choice::Tracked(item), Some(project)) = (&choice, &project) {
            let id = tracked_id(item);
            let answer = target.request_typed::<call::CodeHostTrackedContext>(TrackedParams {
                project: project.to_string_lossy().into_owned(),
                kind: item.kind,
                number: item.number,
            });
            let agent = agent.to_owned();
            cx.spawn(async move |this, cx| {
                let context = match answer.await {
                    Ok(Outcome::Value(context)) => {
                        serde_json::from_value::<ChatContext>(context).ok()
                    }
                    _ => None,
                };
                let Some(context) = context else {
                    return;
                };
                this.update(cx, |bench, cx| {
                    if let Some(composing) = bench
                        .chats
                        .open
                        .get_mut(&agent)
                        .and_then(|chat| chat.composer.as_mut())
                    {
                        composing.tracked_context.insert(id, context);
                        bench.typed_draft(&agent, cx);
                    }
                })
                .ok();
            })
            .detach();
        }
        if let Choice::Earlier {
            cli,
            session,
            name: None,
            title,
            ..
        } = &choice
        {
            let uri = format!("sikemux://earlier/{cli}/{session}");
            if !composing.context.iter().any(|held| held.uri == uri) {
                composing.context.push(ChatContext {
                    uri,
                    title: title.clone(),
                    text: String::new(),
                });
            }
        }
        composing.draft.update(cx, |draft, cx| {
            let end = draft.cursor();
            draft.set_selected_range(menu.typed.start..end, cx);
            let token = match &choice {
                Choice::Mate { id, name, .. } => {
                    Some(InlineToken::new(format!("{AGENT}{id}"), format!("@{name}")))
                }
                Choice::Entry { path, folder } => {
                    let whole = project.as_deref().map_or_else(
                        || Path::new(path).to_path_buf(),
                        |project| project.join(path),
                    );
                    let named = draft
                        .tokens()
                        .iter()
                        .any(|span| span.token().text().as_ref() == format!("+{}", leaf(path)));
                    let tag = if named {
                        format!("+{path}")
                    } else {
                        format!("+{}", leaf(path))
                    };
                    let kind = if *folder { FOLDER } else { FILE };
                    Some(InlineToken::new(
                        format!("{kind}{}", whole.to_string_lossy()),
                        tag,
                    ))
                }
                Choice::Command { name, .. } => Some(InlineToken::new(
                    format!("{COMMAND}{name}"),
                    format!("/{name}"),
                )),
                Choice::Earlier {
                    cli,
                    session,
                    name: Some(name),
                    ..
                } => Some(InlineToken::new(
                    format!("{EARLIER}{cli}/{session}"),
                    format!("@{name}"),
                )),
                Choice::Earlier { .. } | Choice::Stashed(_) => {
                    draft.replace("", window, cx);
                    None
                }
                Choice::Tracked(item) => {
                    let sign = if item.kind == TrackedKind::Pull {
                        '!'
                    } else {
                        '#'
                    };
                    Some(
                        InlineToken::new(tracked_id(item), format!("{sign}{}", item.number))
                            .with_label(format!("#{} {}", item.number, item.title)),
                    )
                }
            };
            if let Some(token) = token {
                if let Err(error) = draft.replace_with_token(token, window, cx) {
                    eprintln!("sikemux: a mention did not go into the draft: {error}");
                }
            }
            if stashed.is_none() {
                draft.replace(" ", window, cx);
            }
        });
        if let Some(id) = stashed {
            self.add_from_stash(agent, vec![id], cx);
        }
        cx.notify();
    }
}

/// The draft's token for an issue or pull request, by which its context goes with the message.
fn tracked_id(item: &TrackedItem) -> String {
    let kind = match item.kind {
        TrackedKind::Issue => "issue",
        TrackedKind::Pull => "pull",
    };
    format!("{TRACKED}{kind}:{}:{}", item.number, item.url)
}

/// `title` cut to what a menu row has room for.
fn clipped(title: &str) -> String {
    const ROOM: usize = 56;
    match title.char_indices().nth(ROOM) {
        Some((at, _)) => format!("{}…", title[..at].trim_end()),
        None => title.to_owned(),
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The best files and folders for `needle`, a file before a folder of the same name.
fn ranked_entries(needle: &str, files: &Arc<Vec<String>>) -> Vec<Choice> {
    crate::fuzzy::rank_entries(needle, files, SHOWN)
        .into_iter()
        .map(|entry| Choice::Entry {
            path: entry.path,
            folder: entry.folder,
        })
        .collect()
}

/// The kinds of thing the menu names, in the order its heading lists them.
const SCOPES: [(char, &str); 5] = [
    ('@', "Agents"),
    ('#', "Issues"),
    ('!', "Pull requests"),
    ('+', "Files"),
    ('&', "Stash"),
];

/// The menu over the draft of `agent`'s chat, while it offers anything, under a heading of the
/// kinds of thing it names; `repo` is the project's on its code host. It reads `bench`, which
/// is drawing it, and never the workbench's entity, which is busy drawing.
pub fn dock(
    bench: &Workbench,
    agent: &str,
    menu: &Menu,
    repo: Option<&str>,
    cx: &mut Context<Workbench>,
) -> Option<DockMenu> {
    let files = &bench.project_files;
    let trigger = menu.typed.trigger;
    if trigger == '&' {
        return None;
    }
    let headed = |dock: DockMenu| {
        if trigger == '/' {
            return dock.titled("Session commands", "ACP");
        }
        let scopes = SCOPES
            .iter()
            .map(|&(key, label)| {
                let (this, agent) = (cx.weak_entity(), agent.to_owned());
                DockScope {
                    key: key.to_string().into(),
                    label: label.into(),
                    on: key == trigger,
                    choose: Rc::new(move |window: &mut Window, cx: &mut App| {
                        this.update(cx, |bench, cx| bench.switch_scope(&agent, key, window, cx))
                            .ok();
                    }),
                }
            })
            .collect();
        let aside = match trigger {
            '+' => "attach",
            '#' | '!' => repo.unwrap_or_default(),
            _ => "mention",
        };
        dock.heading(scopes, aside.to_owned())
    };
    if menu.choices.is_empty() {
        let blank = menu.typed.needle.is_empty();
        let empty = match trigger {
            '@' if blank => Some(DockMenu::empty("No other agent yet")),
            '@' => Some(DockMenu::empty("No agent or earlier chat matches")),
            '+' if files.project.is_none() => {
                Some(DockMenu::empty("Open a project to pick its files"))
            }
            '+' if !files.listed() => Some(DockMenu::loading()),
            '+' => Some(DockMenu::empty("Nothing in the project matches")),
            '#' | '!' => Some(
                menu.empty
                    .clone()
                    .map_or_else(DockMenu::loading, DockMenu::empty),
            ),
            _ => menu.empty.clone().map(DockMenu::empty),
        };
        return empty.map(headed);
    }
    let this = cx.weak_entity();
    let tint = crate::theme::Tokens::get(cx).text_tertiary;
    let mut shown_project: Option<&str> = None;
    let mut shown_kind = None;
    let rows = menu.choices.iter().enumerate().map(|(index, choice)| {
        let (this, agent) = (this.clone(), agent.to_owned());
        let choose = Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| {
                bench.take_choice(&agent, Some(index), window, cx)
            })
            .ok();
        });
        match choice {
            Choice::Mate {
                id,
                name,
                title,
                project,
                provider,
            } => {
                let group = (project.as_deref() != shown_project)
                    .then(|| project.clone().map(SharedString::from))
                    .flatten();
                shown_project = project.as_deref();
                let row = bench.agent_by_id(id);
                DockRow::new(format!("@{name}"), choose)
                    .mark(
                        row.as_ref()
                            .map_or_else(|| ui::mark(provider, 13.), |agent| agent.mark(13., cx)),
                    )
                    .detail(title.clone())
                    .state(row.and_then(|agent| ui::status_mark(&agent.id, agent.status, cx)))
                    .group(group)
            }
            Choice::Entry { path, folder } => {
                let parent = path.rsplit_once('/').map_or(".", |(parent, _)| parent);
                let mark = if *folder {
                    ui::icon(Icon::Folder, 13.)
                        .text_color(tint)
                        .into_any_element()
                } else {
                    crate::file_icons::icon(leaf(path), cx).map_or_else(
                        || {
                            ui::icon(Icon::File, 13.)
                                .text_color(tint)
                                .into_any_element()
                        },
                        |icon| gpui_kit::img(icon).size(px(13.)).into_any_element(),
                    )
                };
                DockRow::new(leaf(path).to_owned(), choose)
                    .mark(mark)
                    .detail(parent.to_owned())
            }
            Choice::Command {
                name,
                description,
                hint,
            } => DockRow::new(format!("/{name}"), choose)
                .detail(description.clone())
                .aside(hint.clone().map(SharedString::from)),
            Choice::Earlier {
                cli,
                name,
                title,
                active_at,
                ..
            } => {
                let kind = if name.is_some() {
                    "Closed"
                } else {
                    "Earlier chats"
                };
                let group = (shown_kind != Some(kind)).then_some(SharedString::from(kind));
                shown_kind = Some(kind);
                let row = match name {
                    Some(name) => DockRow::new(format!("@{name}"), choose).detail(title.clone()),
                    None => DockRow::new(title.clone(), choose)
                        .aside(active_at.map(|at| crate::ui::time::age(at).into())),
                };
                row.mark(ui::mark(cli, 13.)).group(group)
            }
            Choice::Tracked(item) => {
                let (sign, icon) = match item.kind {
                    TrackedKind::Pull => ('!', Icon::PullRequest),
                    TrackedKind::Issue => ('#', Icon::Issue),
                };
                DockRow::new(format!("{sign}{}", item.number), choose)
                    .mark(ui::icon(icon, 13.).text_color(tint).into_any_element())
                    .detail(item.title.clone())
            }
            Choice::Stashed(shown) => {
                DockRow::new(clipped(&crate::stash::title(&shown.item)), choose)
                    .mark(crate::stash::mark(bench, &shown.item, 15., cx))
                    .detail(crate::stash::place(&shown.item))
            }
        }
    });
    let rows: Vec<DockRow> = rows.collect();
    Some(headed(
        DockMenu::new(rows, menu.selected).found(menu.typed.needle.clone()),
    ))
}

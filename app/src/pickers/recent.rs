//! The front project's saved chats beyond the core's recent ones: paged further back, or
//! searched by the rail's filter, for the rail and ⌘⇧N to take up again.

use gpui_kit::AppContext as _;
use gpui_kit::Window;
use gpui_kit::component::input::InputEvent;
use std::time::Duration;

use gpui_kit::component::input::InputState;
use gpui_kit::{
    App, Context, Div, Entity, FontWeight, HighlightStyle, ParentElement, Styled, StyledText,
    Subscription, div,
};
use sikemux_agents::sessions::said::FoundLine;
use sikemux_view::device::{FoundBy, FoundChat, RecentInfo};
use std::collections::HashMap;

use crate::workbench::Workbench;
use sikemux_agents::sessions::recent::RecentCursor;
use sikemux_calls::call;
use sikemux_calls::workspace::RecentChatsParams;

/// The rail's filter of recent chats: its field, and what is typed there in lower case.
pub struct Filter {
    pub input: Entity<InputState>,
    pub text: String,
    pub _changes: Subscription,
}

/// How many chats one page brings.
pub const PAGE: usize = 12;
/// How many chats ⌘⇧N brings at once, the most a page holds.
pub const PALETTE_PAGE: usize = 100;
/// How long typing in the filter settles before the core is asked.
const SETTLE: Duration = Duration::from_millis(150);

/// Saved chats of `project` the core answered, those matching `query`, where the next page
/// starts, and whether a page is on its way.
pub struct OlderChats {
    pub project: String,
    pub query: String,
    pub chats: Vec<RecentInfo>,
    pub next: Option<RecentCursor>,
    /// Where each chat found by its words said them, by the chat's id.
    pub said: HashMap<String, FoundLine>,
    /// The open agents and saved chats `query` finds, in the order the core ranks them.
    pub found: Vec<FoundChat>,
    /// `found` still holds the last query's rows, until the core answers this one.
    stale: bool,
    loading: bool,
}

impl Workbench {
    /// Asks the core for the front project's saved chats matching the rail's filter, unless
    /// it was asked already, or with `more` for the page after those here.
    pub fn look_for_older(&mut self, more: bool, limit: usize, cx: &mut Context<Self>) {
        let Some(project) = self.project().map(|project| project.id.clone()) else {
            self.older = None;
            return;
        };
        let query = self.chats_query(cx);
        let same = self
            .older
            .as_ref()
            .is_some_and(|older| older.project == project && older.query == query);
        let cursor = match (same, more, self.older.as_mut()) {
            (true, false, _) => return,
            (true, true, Some(older)) if !older.loading && older.next.is_some() => {
                older.loading = true;
                older.next.clone()
            }
            (true, _, _) => return,
            (false, _, _) => None,
        };
        if !same {
            let shown = (self.older.take())
                .filter(|older| older.project == project)
                .map(|older| older.found)
                .unwrap_or_default();
            self.older = Some(OlderChats {
                project: project.clone(),
                query: query.clone(),
                chats: Vec::new(),
                next: None,
                said: HashMap::new(),
                found: shown,
                stale: true,
                loading: true,
            });
        }
        let params = |quick| RecentChatsParams {
            project: Some(project.clone()),
            query: Some(query.clone()),
            cursor: cursor.clone(),
            limit: Some(limit),
            quick,
        };
        let quick = (!same && !query.is_empty()).then(|| params(true));
        let full = (!query.starts_with('@')).then(|| params(false));
        let target = self.target();
        let (project, query) = (project.clone(), query.clone());
        self._older = Some(cx.spawn(async move |this, cx| {
            if let Some(quick) = quick {
                let page = target.request_typed::<call::RecentChats>(quick).await.ok();
                let _ = this.update(cx, |bench, cx| {
                    bench.take_page(&project, &query, page, full.is_none(), cx)
                });
                cx.background_executor().timer(SETTLE).await;
            }
            let Some(full) = full else {
                return;
            };
            let page = target.request_typed::<call::RecentChats>(full).await.ok();
            let _ = this.update(cx, |bench, cx| {
                bench.take_page(&project, &query, page, true, cx)
            });
        }));
    }

    /// Takes in a page the core answered for `query`: its saved chats and where each said the
    /// words, and the rows it found under those already found, so none of those moves.
    fn take_page(
        &mut self,
        project: &str,
        query: &str,
        page: Option<sikemux_calls::workspace::RecentChats>,
        last: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(older) =
            (self.older.as_mut()).filter(|older| older.project == project && older.query == query)
        else {
            return;
        };
        older.loading &= !last;
        if let Some(page) = page {
            let here: std::collections::HashSet<String> =
                older.chats.iter().map(|chat| chat.id.clone()).collect();
            older.chats.extend(
                page.chats
                    .into_iter()
                    .filter(|chat| !here.contains(&chat.id)),
            );
            if last {
                older.next = page.next;
            }
            older.said.extend(page.said);
            if std::mem::take(&mut older.stale) {
                older.found.clear();
            }
            let found = page.found.into_iter().filter(|found| {
                !(older.found.iter()).any(|here| {
                    (here.agent_id.is_some() && here.agent_id == found.agent_id)
                        || (here.agent_id.is_none() && here.chat == found.chat)
                })
            });
            let found: Vec<FoundChat> = found.collect();
            older.found.extend(found);
        }
        self.refill_agent_palette(cx);
        cx.notify();
    }

    /// Whether the rail can bring older chats of the front project than it shows.
    pub fn has_older(&self) -> bool {
        self.older
            .as_ref()
            .is_some_and(|older| older.next.is_some())
    }

    /// Forgets a saved chat the person deleted.
    pub fn forget_older(&mut self, id: &str) {
        if let Some(older) = self.older.as_mut() {
            older.chats.retain(|chat| chat.id != id);
        }
    }

    /// Shows a saved chat under the title the person gave it.
    pub fn retitle_older(&mut self, id: &str, title: &str) {
        let chat = self
            .older
            .as_mut()
            .and_then(|older| older.chats.iter_mut().find(|chat| chat.id == id));
        if let Some(chat) = chat {
            chat.title = title.to_owned();
        }
    }

    /// The project's saved chats: the core's recent ones, then those paged or searched to,
    /// newest first, leaving out any an agent holds again since.
    pub fn saved_chats(&self, project: &str) -> Vec<&RecentInfo> {
        let Some(view) = self.shown() else {
            return Vec::new();
        };
        let held = |chat: &RecentInfo| {
            view.workspace.agents.iter().any(|agent| {
                agent.resume_id.as_ref().is_some_and(|session| {
                    chat.id.strip_prefix(agent.provider.as_str()) == Some(&format!(":{session}"))
                })
            })
        };
        let older = self
            .older
            .iter()
            .filter(|older| older.project == project)
            .flat_map(|older| &older.chats)
            .filter(|chat| !view.recent.iter().any(|recent| recent.id == chat.id))
            .filter(|chat| !held(chat));
        let mut chats: Vec<&RecentInfo> = view
            .recent
            .iter()
            .filter(|recent| recent.project == project)
            .chain(older)
            .collect();
        chats.sort_by_key(|chat| std::cmp::Reverse(chat.active_at));
        chats
    }

    /// What saved chats are looked for by: what ⌘⇧N holds while it is open, else the rail's filter.
    fn chats_query(&self, cx: &App) -> String {
        (self.overlays.palette.as_ref())
            .filter(|palette| palette.id() == crate::agents::palette::AGENT_PALETTE)
            .map(|palette| palette.query(cx).trim().to_lowercase())
            .unwrap_or_else(|| self.filter_text().to_owned())
    }

    /// Where the chat `id` said the words `query` holds, when it was found by them.
    pub fn said_for(&self, query: &str, id: &str) -> Option<&FoundLine> {
        (self.older.as_ref())
            .filter(|older| !query.is_empty() && older.query == query)
            .and_then(|older| older.said.get(id))
    }

    /// Where the chat `id` said the words the rail's filter holds, when it was found by them.
    pub fn said_in(&self, id: &str) -> Option<&FoundLine> {
        self.said_for(self.filter_text(), id)
    }

    /// Where the rail's filter puts the agent `agent`, or the saved chat `chat` no agent holds,
    /// in the order the core ranks what it finds; none when it does not find it.
    pub fn filter_place(
        &self,
        agent: Option<&str>,
        chat: Option<&str>,
        title: &str,
    ) -> Option<usize> {
        (self.found_place(self.filter_text(), agent, chat, title)).map(|(place, _)| place)
    }

    /// Where the core ranks the agent `agent`, or the saved chat `chat` no agent holds, among
    /// what `query` finds, and by what it found it; until the core answers `query`, a title
    /// holding it is found by that. With no query, everything is found in one place.
    pub fn found_place(
        &self,
        query: &str,
        agent: Option<&str>,
        chat: Option<&str>,
        title: &str,
    ) -> Option<(usize, FoundBy)> {
        if query.is_empty() {
            return Some((0, FoundBy::Title));
        }
        let Some(older) = self
            .older
            .as_ref()
            .filter(|older| older.query == query && !older.stale)
        else {
            return (title.to_lowercase().contains(query)).then_some((0, FoundBy::Title));
        };
        older.found.iter().enumerate().find_map(|(place, found)| {
            let this = match agent {
                Some(agent) => found.agent_id.as_deref() == Some(agent),
                None => found.agent_id.is_none() && found.chat.as_deref() == chat,
            };
            this.then_some((place, found.by))
        })
    }

    /// The words to find in the chat `id`, when the last search found it by them.
    pub fn found_words(&self, id: &str) -> Option<String> {
        let said = self.older.as_ref()?.said.get(id)?;
        said.first_found().map(str::to_owned)
    }

    /// Takes up a saved chat, and when a search found it by its words, opens its find on them.
    pub fn take_up_chat(&mut self, recent: &RecentInfo, cx: &mut Context<Self>) {
        let found = self.found_words(&recent.id);
        self.resume_chat_finding(recent, found, cx);
    }

    /// Opens the agent `id`'s chat, and when a search found it by its words, its find on them.
    pub fn open_found_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        let found = (self.chat_of(id)).and_then(|chat| self.found_words(&chat.id));
        self.open_agent(id, cx);
        if let Some(found) = found {
            self.find_once_attached(id.to_owned(), Some(found), cx);
        }
    }

    /// The saved chat `id` names, recent or paged to.
    pub fn saved_chat(&self, id: &str) -> Option<&RecentInfo> {
        let recent = self.shown()?.recent.iter();
        let older = self.older.iter().flat_map(|older| &older.chats);
        recent.chain(older).find(|chat| chat.id == id)
    }
}

impl Workbench {
    pub fn toggle_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter.take().is_some() {
            self.look_for_older(false, crate::pickers::recent::PAGE, cx);
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("filter recent chats…")));
        let changes = cx.subscribe(&input, |bench, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                if let Some(filter) = bench.filter.as_mut() {
                    filter.text = input.read(cx).value().to_lowercase();
                }
                bench.look_for_older(false, crate::pickers::recent::PAGE, cx);
                cx.notify();
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        self.filter = Some(crate::pickers::recent::Filter {
            input,
            text: String::new(),
            _changes: changes,
        });
    }

    /// What the rail's filter holds, in lower case; empty while it does not show.
    pub fn filter_text(&self) -> &str {
        self.filter
            .as_ref()
            .map_or("", |filter| filter.text.as_str())
    }
}

/// Where a chat said the words looked for: one line in the tertiary ink, the words in full ink.
pub fn said_line(said: &FoundLine, cx: &App) -> Div {
    let t = crate::theme::Tokens::get(cx);
    let words = HighlightStyle {
        color: Some(t.text_primary),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    };
    let mut text = String::new();
    let mut marks = Vec::new();
    for run in &said.runs {
        let start = text.len();
        text.push_str(&run.text);
        if run.found {
            marks.push((start..text.len(), words));
        }
    }
    div()
        .text_size(crate::theme::ui_text(crate::theme::TEXT_2XS))
        .text_color(t.text_tertiary)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(StyledText::new(text).with_highlights(marks))
}

//! The front project's saved chats beyond the core's recent ones: paged further back, or
//! searched by the rail's filter, for the rail and ⌘⇧N to take up again.

use gpui_kit::component::input::InputEvent;
use gpui_kit::AppContext as _;
use gpui_kit::Window;
use std::time::Duration;

use gpui_kit::component::input::InputState;
use gpui_kit::{Context, Entity, Subscription};
use sikemux_view::device::RecentInfo;

use crate::workbench::Workbench;
use sikemux_core::server::call;
use sikemux_core::server::workspace::RecentChatsParams;
use sikemux_core::sessions::recent::RecentCursor;

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
        let query = self.filter_text().to_owned();
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
            self.older = Some(OlderChats {
                project: project.clone(),
                query: query.clone(),
                chats: Vec::new(),
                next: None,
                loading: true,
            });
        }
        let params = RecentChatsParams {
            project: Some(project.clone()),
            query: Some(query.clone()),
            cursor,
            limit: Some(limit),
        };
        let target = self.target();
        let settle = !same && !query.is_empty();
        self._older = Some(cx.spawn(async move |this, cx| {
            if settle {
                cx.background_executor().timer(SETTLE).await;
            }
            let page = target.request_typed::<call::RecentChats>(params).await.ok();
            let _ = this.update(cx, |bench, cx| {
                let Some(older) = bench
                    .older
                    .as_mut()
                    .filter(|older| older.project == project && older.query == query)
                else {
                    return;
                };
                older.loading = false;
                older.next = None;
                if let Some(page) = page {
                    older.chats.extend(page.chats);
                    older.next = page.next;
                }
                bench.refill_agent_palette(cx);
                cx.notify();
            });
        }));
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("filter recent chats…"));
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

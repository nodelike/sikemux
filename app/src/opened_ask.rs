//! One of the person's asks, opened from its card: the chat of the agent it went to and that
//! agent's desk show only what the core filed under it, until the switch in the pane's header
//! widens both again. Leaving the agent's tab, or Back, closes it.

use gpui_kit::Context;
use sikemux_view::layout::PaneKind;
use sikemux_view::now::NowAsk;

use crate::chat::narrowed::Narrowed;
use crate::core::Ask;
use crate::tabs::shows;
use crate::toast;
use crate::workbench::Workbench;

pub(crate) struct OpenedAsk {
    pub id: u64,
    pub agent: String,
    /// As the core last sent it, kept should the core stop listing it while it is open.
    pub ask: NowAsk,
    /// Shown alone, rather than among the whole chat.
    pub narrowed: bool,
    /// Its agent's tab has come to the front since it was opened, so leaving it closes the ask.
    arrived: bool,
}

/// The ask number a `sikemux://ask/<id>` link names.
pub(crate) fn link(link: &str) -> Option<u64> {
    link.strip_prefix("sikemux://ask/")?
        .trim_end_matches('/')
        .parse()
        .ok()
}

impl Workbench {
    /// The ask numbered `id`, as this host's core last sent it.
    pub(crate) fn ask_of(&self, id: u64) -> Option<&NowAsk> {
        (self.this_view()?.now.iter())
            .flat_map(|now| &now.asks)
            .find(|ask| ask.id == id)
    }

    /// The ask open on `agent`'s pane, shown alone or not.
    pub(crate) fn opened_on(&self, agent: &str) -> Option<&OpenedAsk> {
        self.opened_ask.as_ref().filter(|ask| ask.agent == agent)
    }

    /// The ask `agent`'s chat and desk show alone, when they show one.
    pub(crate) fn narrowed_on(&self, agent: &str) -> Option<&NowAsk> {
        let opened = self.opened_on(agent).filter(|ask| ask.narrowed)?;
        Some(&opened.ask)
    }

    pub fn open_ask(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ask) = self.ask_of(id) else {
            return self.notify(toast::error(t!("Ask #{id} is no longer open", id = id)), cx);
        };
        let agent = ask.to.clone();
        self.opened_ask = Some(OpenedAsk {
            id,
            agent: agent.clone(),
            ask: ask.clone(),
            narrowed: true,
            arrived: false,
        });
        self.visits.ask_forward = None;
        let brought = self.bring_agent(&agent, cx);
        cx.spawn(async move |bench, cx| {
            brought.await;
            let _ = bench.update(cx, |bench, cx| bench.narrow_ask(true, cx));
        })
        .detach();
    }

    /// Shows the open ask alone, or among the whole chat and desk.
    pub fn narrow_ask(&mut self, narrowed: bool, cx: &mut Context<Self>) {
        let Some(opened) = self.opened_ask.as_mut() else {
            return;
        };
        opened.narrowed = narrowed;
        self.sync_chat();
        if narrowed {
            self.front_ask_page(cx);
        }
        self.desks_moved(cx);
        self.parts.redraw_stage(cx);
        cx.notify();
    }

    /// Closes the open ask, widening its chat and desk, and answers with it.
    pub(crate) fn close_ask(&mut self, cx: &mut Context<Self>) -> Option<u64> {
        let opened = self.opened_ask.take()?;
        self.narrow_chat(&opened.agent, None);
        self.desks_moved(cx);
        self.parts.redraw_stage(cx);
        cx.notify();
        Some(opened.id)
    }

    /// Brings the latest of the open ask's pages to the front of its desk, unless the page in
    /// front is one of them.
    fn front_ask_page(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.opened_ask.as_ref() else {
            return;
        };
        let Some(strip) = self.desks.read(cx).strips.get(&opened.agent).cloned() else {
            return;
        };
        let filed = |id: &str| opened.ask.pages.iter().any(|page| page.id == id);
        if strip.tabs.iter().any(|tab| tab.active && filed(&tab.id)) {
            return;
        }
        let Some(latest) = (opened.ask.pages.iter().rev())
            .find(|page| strip.tabs.iter().any(|tab| tab.id == page.id))
        else {
            return;
        };
        let (agent, page) = (opened.agent.clone(), latest.id.clone());
        self.with_engine(cx, |engine| engine.switch_tab(&agent, &page));
    }

    /// Brings the open ask's chat in step with it, once the chat is open.
    fn sync_chat(&mut self) {
        let Some(opened) = self.opened_ask.as_ref() else {
            return;
        };
        let to = opened.narrowed.then(|| Narrowed {
            id: opened.id,
            at: opened.ask.at,
            title: opened.ask.title.clone(),
        });
        let agent = opened.agent.clone();
        self.narrow_chat(&agent, to);
    }

    fn narrow_chat(&mut self, agent: &str, to: Option<Narrowed>) {
        let Some(chat) = self.chats.open.get_mut(agent) else {
            return;
        };
        if let Some(asked) = chat.narrow(agent, to) {
            chat.target.ask(Ask::Chat(asked));
        }
    }

    /// The view moved: the open ask takes what the core now says of it, and closes once the
    /// tab in front no longer shows its agent.
    pub(crate) fn keep_opened_ask(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.opened_ask.as_ref() else {
            return;
        };
        let latest = self.ask_of(opened.id).cloned();
        let front = (self.tab()).and_then(|tab| shows(tab, PaneKind::Agent));
        let here = front == Some(&opened.agent);
        let Some(opened) = self.opened_ask.as_mut() else {
            return;
        };
        opened.ask = latest.unwrap_or_else(|| opened.ask.clone());
        match (here, opened.arrived) {
            (true, _) => opened.arrived = true,
            (false, true) => {
                self.close_ask(cx);
                return;
            }
            (false, false) => {}
        }
        self.sync_chat();
    }

    /// Back closes an open ask before it leaves the tab, and Forward opens it again.
    pub(crate) fn step_ask(&mut self, by: isize, cx: &mut Context<Self>) -> bool {
        if by < 0 {
            let Some(id) = self.close_ask(cx) else {
                return false;
            };
            self.visits.ask_forward = Some(id);
            return true;
        }
        let Some(id) = self.visits.ask_forward.take() else {
            return false;
        };
        let front = (self.tab()).and_then(|tab| shows(tab, PaneKind::Agent));
        if self.ask_of(id).is_none_or(|ask| front != Some(&ask.to)) {
            return false;
        }
        self.open_ask(id, cx);
        true
    }
}

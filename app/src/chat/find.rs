//! Find in a chat: ⌘F opens a bar over the transcript, which marks every place the text
//! is found in the rows held and brings the one the reader is on into view. The core finds
//! across the whole chat, held or not: it counts the places, and stepping past the rows held
//! brings in the rows around the next one.

use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AppContext, Context, Entity, Focusable, Subscription, Window};
use gpui_transcript::Query;
use sikemux_core::server::{FindAnswer, FindAt, FindDirection, FindHit, FindParams};

use crate::core::ChatAsk;
use crate::ui::FindBar;
use crate::workbench::Workbench;

/// Places found the core is asked for at a time.
const HITS: usize = 200;
/// Counting stops here, and the bar says 999+.
const MOST_COUNTED: u64 = 1000;

/// The find bar of a chat, while it is open.
pub struct Finding {
    input: Entity<InputState>,
    match_case: bool,
    whole_word: bool,
    whole: WholeChat,
    _typed: Subscription,
}

/// What the core found across the whole chat for the query the bar holds.
#[derive(Default)]
struct WholeChat {
    /// The number of the bar's last ask, so an answer to an older query is let go.
    asked: u64,
    /// Places found, newest first, and where the core goes on from.
    hits: Vec<FindHit>,
    next: Option<FindAt>,
    /// The place among `hits` the reader is on, once they stepped.
    at: Option<usize>,
    /// Stepping older went past `hits`, and waits for more.
    stepping: bool,
    /// The ask out is only counting, so its places are not wanted.
    counting: bool,
    counted: u64,
    complete: bool,
}

static ASKED: AtomicU64 = AtomicU64::new(1);

impl WholeChat {
    /// Which of the places found in its part `hit` is, as the rows' own find counts them.
    fn nth(&self, hit: &FindHit) -> usize {
        self.hits
            .iter()
            .filter(|other| {
                other.row == hit.row && other.part == hit.part && other.range[0] < hit.range[0]
            })
            .count()
    }

    /// The place among `hits` the rows' own find is on, by its row, part and place in the part.
    fn of(&self, (row, part, nth): (&str, &str, usize)) -> Option<usize> {
        let mut in_part: Vec<usize> = (0..self.hits.len())
            .filter(|&at| self.hits[at].row == row && self.hits[at].part == part)
            .collect();
        in_part.sort_by_key(|&at| self.hits[at].range[0]);
        in_part.get(nth).copied()
    }

    /// The bar's words, `3/41…` while counting goes on, with the place the reader is on: the one
    /// stepped to, else the one the rows' own find is on, `local`.
    fn label(&self, local: Option<(&str, &str, usize)>) -> Option<String> {
        if self.counted == 0 {
            return None;
        }
        let at = self.at.or_else(|| local.and_then(|local| self.of(local)));
        if self.counted >= MOST_COUNTED {
            return Some(format!("{}/999+", at.map_or(1, |at| at + 1)));
        }
        let place = at.map_or(self.counted, |at| self.counted.saturating_sub(at as u64));
        let more = if self.complete { "" } else { "…" };
        Some(format!("{place}/{}{more}", self.counted))
    }
}

/// What `chatFind` is asked for `query` in `agent`'s chat, from its newest rows back.
fn find_params(agent: &str, feed: String, query: Query) -> FindParams {
    FindParams {
        agent_id: agent.to_owned(),
        feed,
        query: query.text,
        match_case: query.match_case,
        whole_word: query.whole_word,
        from: None,
        direction: FindDirection::Older,
        limit: Some(HITS),
        count_only: false,
    }
}

impl Workbench {
    /// Opens the find bar of `agent`'s chat, or puts the reader back in it.
    pub fn open_find(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get_mut(agent) else {
            return;
        };
        if chat.finding.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in chat"));
            let typed = {
                let agent = agent.to_owned();
                cx.subscribe(&input, move |bench, _, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        bench.search(&agent, cx);
                    }
                })
            };
            chat.finding = Some(Finding {
                input,
                match_case: false,
                whole_word: false,
                whole: WholeChat::default(),
                _typed: typed,
            });
        }
        if let Some(finding) = &chat.finding {
            finding
                .input
                .update(cx, |input, cx| input.select_all(window, cx));
            finding.input.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.parts.redraw_stage(cx);
    }

    /// Looks for what the bar holds, as its options say, in the rows held and across the chat.
    fn search(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get_mut(agent) else {
            return;
        };
        let query = chat.finding.as_ref().map(|finding| Query {
            text: finding.input.read(cx).value().to_string(),
            match_case: finding.match_case,
            whole_word: finding.whole_word,
        });
        let query = query.filter(|query| !query.text.is_empty());
        chat.unfold_for_find(query.is_some(), cx);
        chat.transcript
            .update(cx, |transcript, cx| transcript.find(query.clone(), cx));
        let feed = chat.mark.as_ref().map(|mark| mark.feed.clone());
        let Some(finding) = chat.finding.as_mut() else {
            return;
        };
        finding.whole = WholeChat::default();
        if let (Some(query), Some(feed)) = (query, feed) {
            let params = find_params(agent, feed, query);
            let asked = ASKED.fetch_add(1, Ordering::Relaxed);
            finding.whole.asked = asked;
            self.ask_chat(ChatAsk::Find {
                agent: agent.to_owned(),
                params,
                asked,
            });
        }
        self.parts.redraw_stage(cx);
    }

    /// Takes what the core found, asks it to go on counting until the count is whole or large,
    /// and goes on a step that waited for more places.
    pub fn found(&mut self, agent: &str, asked: u64, found: FindAnswer, cx: &mut Context<Self>) {
        let Some(whole) = self
            .finding_mut(agent)
            .map(|finding| &mut finding.whole)
            .filter(|whole| whole.asked == asked)
        else {
            return;
        };
        if !std::mem::take(&mut whole.counting) {
            whole.hits.extend(found.hits);
            whole.next = found.next.clone();
        }
        whole.counted = whole.counted.max(found.counted.matches);
        whole.complete = found.counted.complete;
        let count_on = !whole.complete && whole.counted < MOST_COUNTED;
        // A long chat's first slice scanned may hold no place at all, so places are asked
        // for until one comes, before the count goes on alone.
        let more = match (whole.hits.is_empty(), whole.next.clone()) {
            (true, Some(from)) => Some((from, false)),
            _ if std::mem::take(&mut whole.stepping) => {
                self.step_find(agent, false, cx);
                None
            }
            _ => found.next.filter(|_| count_on).map(|from| (from, true)),
        };
        if let (Some((from, counting)), Some(mut params)) = (more, self.find_params(agent, cx)) {
            (params.from, params.count_only) = (Some(from), counting);
            let asked = ASKED.fetch_add(1, Ordering::Relaxed);
            if let Some(finding) = self.finding_mut(agent) {
                (finding.whole.asked, finding.whole.counting) = (asked, counting);
            }
            self.ask_chat(ChatAsk::Find {
                agent: agent.to_owned(),
                params,
                asked,
            });
        }
        self.parts.redraw_stage(cx);
    }

    fn finding_mut(&mut self, agent: &str) -> Option<&mut Finding> {
        self.chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.finding.as_mut())
    }

    /// What the bar asks `chatFind` for now, without where to go on from.
    fn find_params(&self, agent: &str, cx: &Context<Self>) -> Option<FindParams> {
        let chat = self.chats.open.get(agent)?;
        let finding = chat.finding.as_ref()?;
        let feed = chat.mark.as_ref()?.feed.clone();
        let query = Query {
            text: finding.input.read(cx).value().to_string(),
            match_case: finding.match_case,
            whole_word: finding.whole_word,
        };
        Some(find_params(agent, feed, query))
    }

    fn flip_find(&mut self, agent: &str, flip: fn(&mut Finding), cx: &mut Context<Self>) {
        if let Some(finding) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.finding.as_mut())
        {
            flip(finding);
            self.search(agent, cx);
        }
    }

    /// Moves to the next place found, newer when `forward`, across the whole chat: within the
    /// rows held, or to rows the core brings in around it.
    fn step_find(&mut self, agent: &str, forward: bool, cx: &mut Context<Self>) {
        let params = self.find_params(agent, cx);
        let Some(chat) = self.chats.open.get_mut(agent) else {
            return;
        };
        let local = (chat.transcript.read(cx).found_at())
            .map(|(row, part, nth)| (row.to_owned(), part.to_owned(), nth));
        let feed = chat.mark.as_ref().map(|mark| mark.feed.clone());
        let Some(whole) = chat.finding.as_mut().map(|finding| &mut finding.whole) else {
            return;
        };
        if whole.hits.is_empty() {
            match whole.next.is_some() {
                true => whole.stepping = true,
                false => chat
                    .transcript
                    .update(cx, |transcript, cx| transcript.step(forward, cx)),
            }
            return;
        }
        let here = whole.at.or_else(|| {
            let (row, part, nth) = local.as_ref()?;
            whole.of((row, part, *nth))
        });
        let last = whole.hits.len() - 1;
        let to = match (here, forward) {
            (None, _) => Some(0),
            (Some(0), true) => whole.next.is_none().then_some(last),
            (Some(at), true) => Some(at - 1),
            (Some(at), false) if at < last => Some(at + 1),
            (Some(_), false) if whole.next.is_none() => Some(0),
            (Some(_), false) => None,
        };
        let Some(to) = to else {
            if forward {
                return;
            }
            let (Some(from), Some(mut params)) = (whole.next.clone(), params) else {
                return;
            };
            params.from = Some(from);
            let asked = ASKED.fetch_add(1, Ordering::Relaxed);
            (whole.at, whole.asked) = (here, asked);
            (whole.stepping, whole.counting) = (true, false);
            return self.ask_chat(ChatAsk::Find {
                agent: agent.to_owned(),
                params,
                asked,
            });
        };
        whole.at = Some(to);
        let hit = whole.hits[to].clone();
        let nth = whole.nth(&hit);
        let held = chat.transcript.read(cx).holds(&hit.row);
        chat.transcript.update(cx, |transcript, cx| {
            let (row, part) = (hit.row.clone().into(), hit.part.clone().into());
            transcript.go_to(row, part, nth, cx)
        });
        if let (false, Some(feed)) = (held, feed) {
            self.ask_chat(ChatAsk::Around {
                agent: agent.to_owned(),
                feed,
                around: hit.from_event,
            });
        }
        self.parts.redraw_stage(cx);
    }

    /// Closes the bar, takes the marks away, and puts the reader back in the draft.
    pub fn close_find(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get_mut(agent) else {
            return;
        };
        chat.finding = None;
        chat.unfold_for_find(false, cx);
        chat.transcript
            .update(cx, |transcript, cx| transcript.find(None, cx));
        if let Some(composing) = &chat.composer {
            composing.draft.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.parts.redraw_stage(cx);
    }
}

/// The find bar over `agent`'s transcript, while it is open.
pub fn bar(
    agent: &str,
    finding: &Finding,
    found: (usize, usize),
    local: Option<(&str, &str, usize)>,
    cx: &mut Context<Workbench>,
) -> FindBar {
    let label = match (finding.whole.label(local), found) {
        (Some(whole), _) => whole,
        (None, (current, total)) if total > 0 => format!("{}/{total}", current + 1),
        _ if finding.input.read(cx).value().is_empty() => "0/0".to_owned(),
        _ => "No results".to_owned(),
    };
    let this = cx.weak_entity();
    let on = |act: fn(&mut Workbench, &str, &mut Window, &mut Context<Workbench>)| {
        let (this, agent) = (this.clone(), agent.to_owned());
        move |window: &mut Window, cx: &mut gpui_kit::App| {
            this.update(cx, |bench, cx| act(bench, &agent, window, cx))
                .ok();
        }
    };
    let step = {
        let (this, agent) = (this.clone(), agent.to_owned());
        move |forward: bool, _: &mut Window, cx: &mut gpui_kit::App| {
            this.update(cx, |bench, cx| bench.step_find(&agent, forward, cx))
                .ok();
        }
    };
    FindBar::new(&finding.input, label)
        .match_case(
            finding.match_case,
            on(|bench, agent, _, cx| {
                bench.flip_find(agent, |finding| finding.match_case ^= true, cx)
            }),
        )
        .whole_word(
            finding.whole_word,
            on(|bench, agent, _, cx| {
                bench.flip_find(agent, |finding| finding.whole_word ^= true, cx)
            }),
        )
        .on_step(step)
        .on_close(on(|bench, agent, window, cx| {
            bench.close_find(agent, window, cx)
        }))
}

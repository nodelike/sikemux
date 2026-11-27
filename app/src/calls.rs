//! Calls: decisions put to the person, as each host's core lists them in its view. A call is
//! answered on its desk's Calls page and in the title bar's popover; the chats the core places it
//! in keep it folded to one row that opens the page.

use crate::ui::Clicks;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::base::{Button as BaseButton, Popover};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, relative, Anchor, AnyElement, App, AppContext, ClickEvent, Context, Entity,
    EventEmitter, Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, Render, SharedString, Styled, StyledImage, Subscription, WeakEntity, Window,
};
use sikemux_calls::call;
use sikemux_view::calls::{
    AnswerCallParams, CallAnswer, CallClient, CallKind, CallParams, CallResult, CallState, CallWhy,
    PersonCall, LOOK_SEEN,
};
use sikemux_view::device::DeviceView;

use crate::chat::Press;
use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::picture::Picture;
use crate::theme::{self, radius_2, radius_4, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3};
use crate::ui::{self, selectable, Button, Variant};
use crate::workbench::Workbench;

mod page;
pub(crate) mod reply;
pub(crate) use page::{install, on_desk, project_of, waiting_on_desk, Foot, Page};

/// Whether agent `agent`'s chat lists `call`, as the core places it.
pub(crate) fn listed_in(call: &PersonCall, agent: &str) -> bool {
    call.shown_in.iter().any(|chat| chat == agent)
}

/// Of `calls`, those waiting on the person that agent `agent`'s chat lists, oldest first.
fn open_in<'a>(calls: &'a [PersonCall], agent: &str) -> Vec<&'a PersonCall> {
    let mut open: Vec<&PersonCall> = (calls.iter())
        .filter(|call| call.state == CallState::Open && listed_in(call, agent))
        .collect();
    open.sort_by_key(|call| call.raised_at);
    open
}

/// The mark of a call of `kind`, as its tab shows it.
pub(crate) fn kind_mark(kind: CallKind, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (icon, color) = match kind {
        CallKind::Production => (Icon::Rocket, t.warn),
        CallKind::Taste => (Icon::Eye, t.text_tertiary),
        CallKind::Access => (Icon::Lock, t.text_tertiary),
        CallKind::Do => (Icon::Hand, t.text_tertiary),
        CallKind::Look => (Icon::Window, t.text_tertiary),
        CallKind::Money | CallKind::Unknown => (Icon::Bell, t.text_tertiary),
    };
    ui::icon(icon, 13.)
        .text_color(color)
        .flex_none()
        .into_any_element()
}

const CARD_WIDTH: f32 = 452.;
const ROW_HEIGHT: f32 = 34.;
const FACE: f32 = 20.;
const STRIP_HEIGHT: f32 = 132.;
const STRIP_LEAD: f32 = 286.;
const EVIDENCE_HEIGHT: f32 = 148.;
/// The width inside a card's picture frame: the card less its panel's and its own padding and
/// the frame's edges.
const EVIDENCE_WIDTH: f32 = CARD_WIDTH - 2. * 6. - 2. - 2. * 10. - 2.;
const PICTURE_SIZE: f32 = 800.;
const MAX_RATIOS: usize = 256;
const MAX_PAGE_PICTURES: usize = 64;
const PICTURE_RETRY_AFTER: Duration = Duration::from_secs(30);

/// What the person has done on each open call's card, so the chat and the popover show the same
/// choice and the same words while the call waits.
#[derive(Default)]
pub struct Cards {
    held: HashMap<String, Held>,
    /// The options chosen on each call the person touched; the rest show their asker's pick.
    chosen: HashMap<String, Vec<String>>,
    sending: HashSet<String>,
    /// The calls this window told their core it drew.
    seen: HashSet<String>,
    /// The call picked in the popover, shown open there and answered by its agent's chat; with
    /// none picked the popover shows the newest and a chat answers its agent's oldest.
    opened: Option<String>,
    /// Each evidence picture's width over its height, read once off the UI thread (LRU bounded).
    ratios: HashMap<String, Option<f32>>,
    ratios_order: VecDeque<String>,
    ratios_loading: HashSet<String>,
    /// The top of each page a call links, as its tab shows it, asked of the core once (LRU bounded).
    page_pictures: HashMap<String, Option<std::sync::Arc<gpui_kit::Image>>>,
    page_pictures_order: VecDeque<String>,
    page_pictures_loading: HashSet<String>,
    page_pictures_failed: HashMap<String, Instant>,
}

struct Held {
    field: Entity<InputState>,
    /// What runs on a yes shows.
    action_open: bool,
    _typed: Subscription,
}

/// Return in a card's field: answer the call with what the card holds.
pub struct GoWith(pub String);

impl EventEmitter<GoWith> for Cards {}

impl Cards {
    fn chosen(&self, call: &PersonCall) -> Vec<String> {
        match self.chosen.get(&call.id) {
            Some(chosen) => chosen.clone(),
            None => call.pick.iter().cloned().collect(),
        }
    }

    fn field_focused(&self, id: &str, window: &Window, cx: &App) -> bool {
        (self.held.get(id))
            .is_some_and(|held| held.field.read(cx).focus_handle(cx).is_focused(window))
    }

    /// A key on the card of `call`, as today's question rows take them: a number chooses its
    /// row, and the arrows move the chosen row. Says whether it took it.
    fn press(&mut self, call: &PersonCall, press: Press, cx: &mut Context<Self>) -> bool {
        let count = call.options.len();
        if count == 0 {
            return false;
        }
        let position = |id: &String| call.options.iter().position(|option| &option.id == id);
        let chosen = self.chosen(call).first().and_then(position);
        let at = match press {
            Press::Choose(ix) if ix < count => ix,
            Press::Up => chosen.map_or(count - 1, |at| (at + count - 1) % count),
            Press::Down => chosen.map_or(0, |at| (at + 1) % count),
            _ => return false,
        };
        let Some(option) = call.options.get(at) else {
            return false;
        };
        self.chosen.insert(call.id.clone(), vec![option.id.clone()]);
        cx.notify();
        true
    }

    fn action_open(&self, id: &str) -> bool {
        self.held.get(id).is_some_and(|held| held.action_open)
    }

    fn sending(&self, id: &str) -> bool {
        self.sending.contains(id)
    }

    /// Chooses an option, or clears the one chosen.
    fn choose(&mut self, call: &PersonCall, option: &str, cx: &mut Context<Self>) {
        self.toggle(call, option);
        cx.notify();
    }

    fn toggle(&mut self, call: &PersonCall, option: &str) {
        let chosen = match self.chosen(call).iter().any(|id| id == option) {
            true => Vec::new(),
            false => vec![option.to_owned()],
        };
        self.chosen.insert(call.id.clone(), chosen);
    }

    /// The card's field, made the first time the card draws.
    fn field(
        &mut self,
        call: &PersonCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(held) = self.held.get(&call.id) {
            return held.field.clone();
        }
        let field = cx.new(|cx| InputState::new(window, cx).masked(call.secret));
        let id = call.id.clone();
        // Only the button, or the card's own key, passes a step to production; Enter in its
        // note sends nothing.
        let production = call.kind == CallKind::Production;
        let typed = cx.subscribe(&field, move |_, _, event: &InputEvent, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } if !production => cx.emit(GoWith(id.clone())),
            _ => {}
        });
        self.held.insert(
            call.id.clone(),
            Held {
                field: field.clone(),
                action_open: false,
                _typed: typed,
            },
        );
        field
    }

    fn held_field(&self, id: &str) -> Option<Entity<InputState>> {
        self.held.get(id).map(|held| held.field.clone())
    }

    fn has_seen(&self, id: &str) -> bool {
        self.seen.contains(id)
    }

    fn ratio_cached(&self, path: &str) -> Option<f32> {
        self.ratios.get(path).copied().flatten()
    }

    fn needs_ratio(&self, path: &str) -> bool {
        !self.ratios.contains_key(path) && !self.ratios_loading.contains(path)
    }

    fn keep_ratio(&mut self, path: String, ratio: Option<f32>) {
        self.ratios_loading.remove(&path);
        self.ratios_order.retain(|kept| kept != &path);
        while self.ratios.len() >= MAX_RATIOS {
            let Some(oldest) = self.ratios_order.pop_front() else {
                break;
            };
            self.ratios.remove(&oldest);
        }
        self.ratios_order.push_back(path.clone());
        self.ratios.insert(path, ratio);
    }

    fn load_ratio(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.needs_ratio(path) {
            return;
        }
        let path = path.to_owned();
        self.ratios_loading.insert(path.clone());
        let reading = path.clone();
        cx.spawn(async move |cards, cx| {
            let ratio = cx
                .background_executor()
                .spawn(async move {
                    let (width, height) = image::image_dimensions(&reading).ok()?;
                    (height > 0).then(|| width as f32 / height as f32)
                })
                .await;
            let _ = cards.update(cx, |cards, cx| {
                cards.keep_ratio(path, ratio);
                if ratio.is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn page_picture_cached(&self, id: &str) -> Option<std::sync::Arc<gpui_kit::Image>> {
        self.page_pictures.get(id).cloned().flatten()
    }

    fn needs_page_picture(&self, id: &str) -> bool {
        !self.page_pictures.contains_key(id)
            && !self.page_pictures_loading.contains(id)
            && self
                .page_pictures_failed
                .get(id)
                .is_none_or(|at| at.elapsed() >= PICTURE_RETRY_AFTER)
    }

    fn keep_page_picture(&mut self, id: String, picture: Option<std::sync::Arc<gpui_kit::Image>>) {
        self.page_pictures_loading.remove(&id);
        self.page_pictures_failed.remove(&id);
        self.page_pictures_order.retain(|kept| kept != &id);
        while self.page_pictures.len() >= MAX_PAGE_PICTURES {
            let Some(oldest) = self.page_pictures_order.pop_front() else {
                break;
            };
            self.page_pictures.remove(&oldest);
        }
        self.page_pictures_order.push_back(id.clone());
        self.page_pictures.insert(id, picture);
    }

    fn load_page_picture(&mut self, id: &str, target: &Target, cx: &mut Context<Self>) {
        if !self.needs_page_picture(id) {
            return;
        }
        let id = id.to_owned();
        self.page_pictures_loading.insert(id.clone());
        let answer =
            target.request_typed::<call::ReadPage>(sikemux_calls::harness::PageParams::new(&id));
        cx.spawn(async move |cards, cx| {
            let Ok(answer) = answer.await else {
                let _ = cards.update(cx, |cards, _| {
                    cards.page_pictures_loading.remove(&id);
                    cards.page_pictures_failed.insert(id, Instant::now());
                });
                return;
            };
            let picture = match answer.picture {
                Some(encoded) => {
                    cx.background_executor()
                        .spawn(async move { crate::picture::decode("image/jpeg", &encoded) })
                        .await
                }
                None => None,
            };
            let _ = cards.update(cx, |cards, cx| {
                let redraw = picture.is_some();
                cards.keep_page_picture(id, picture);
                if redraw {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Tells the core once that this window drew the call, which a production call needs
    /// before its answer counts.
    fn saw(&mut self, call: &PersonCall, target: &Target) {
        if self.seen.insert(call.id.clone()) {
            // Unanswered on purpose: a call answered elsewhere meanwhile is no error here.
            drop(target.request_typed::<call::CallSeen>(CallParams {
                id: call.id.clone(),
            }));
        }
    }

    /// What answers the call as the card stands: the option chosen with the note, or the
    /// person's own words alone.
    fn answer(&self, call: &PersonCall, cx: &App) -> Option<AnswerCallParams> {
        let held = self.held.get(&call.id)?;
        let typed = held.field.read(cx).value().trim().to_owned();
        let typed = (!typed.is_empty()).then_some(typed);
        let id = call.id.clone();
        match self.chosen(call).first() {
            None => typed.map(|words| AnswerCallParams {
                id,
                words: Some(words),
                ..AnswerCallParams::default()
            }),
            Some(option) => Some(AnswerCallParams {
                id,
                option: Some(option.clone()),
                note: typed,
                ..AnswerCallParams::default()
            }),
        }
    }

    /// Opens call `id` on the Calls page and in the popover.
    pub(crate) fn open(&mut self, id: &str, cx: &mut Context<Self>) {
        self.opened = Some(id.to_owned());
        cx.notify();
    }

    /// Lets go of what was held for calls no longer open anywhere.
    pub(crate) fn keep(&mut self, open: &HashSet<&str>) {
        self.held.retain(|id, _| open.contains(id.as_str()));
        self.chosen.retain(|id, _| open.contains(id.as_str()));
        self.sending.retain(|id| open.contains(id.as_str()));
        self.seen.retain(|id| open.contains(id.as_str()));
        if self.opened.as_deref().is_some_and(|id| !open.contains(id)) {
            self.opened = None;
        }
    }
}

impl Workbench {
    /// Each core's view this window holds, with where it is.
    pub(crate) fn views(&self) -> impl Iterator<Item = (Place, &DeviceView)> {
        let this = (self.feeds.this.view.as_ref()).map(|view| (Place::This, view));
        let hosts = (self.feeds.hosts())
            .filter_map(|(key, feed)| Some((Place::Account(key.clone()), feed.view.as_ref()?)));
        this.into_iter().chain(hosts)
    }

    pub(crate) fn linked_host_of(&self, agent: &str) -> Option<String> {
        self.views().find_map(|(place, view)| match place {
            Place::Account(host) if view.workspace.agents.iter().any(|held| held.id == agent) => {
                Some(host)
            }
            _ => None,
        })
    }

    pub(crate) fn call_at(&self, id: &str) -> Option<(Place, &PersonCall)> {
        self.views().find_map(|(place, view)| {
            let call = view.calls.iter().find(|call| call.id == id)?;
            Some((place, call))
        })
    }

    /// The agent whose desk shows `call` at `place`: the one in front when its chat lists it, else
    /// the first whose chat does.
    pub(crate) fn desk_showing(&self, place: &Place, call: &PersonCall) -> String {
        (self.front_agent().map(|(.., agent)| agent))
            .filter(|agent| *place == self.place() && listed_in(call, agent))
            .unwrap_or_else(|| call.desk().to_owned())
    }

    /// Opens artifact `page` of call `id` on the desk that shows the call, with its choices at
    /// the foot.
    pub(crate) fn open_call_page(&mut self, id: &str, page: &str, cx: &mut Context<Self>) {
        let Some((place, call)) = self.call_at(id) else {
            return;
        };
        let agent = self.desk_showing(&place, call);
        self.open_call_page_on(&agent, id, page, place, cx);
    }

    /// Opens artifact `page` of call `id` on the call's first desk, whatever is in front, as when
    /// the person comes from outside the window.
    pub(crate) fn open_call_page_on_its_desk(
        &mut self,
        id: &str,
        page: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((place, call)) = self.call_at(id) else {
            return;
        };
        let desk = call.desk().to_owned();
        self.open_call_page_on(&desk, id, page, place, cx);
    }

    fn open_call_page_on(
        &mut self,
        agent: &str,
        id: &str,
        page: &str,
        place: Place,
        cx: &mut Context<Self>,
    ) {
        self.calls.update(cx, |cards, cx| cards.open(id, cx));
        let at = Target::new(&self.asks, place);
        self.open_page(page, Some(agent), at, cx);
    }

    /// Opens what a notification about agent `agent` of this core told of: the artifact of the
    /// latest ask its chat holds, else its chat.
    pub(crate) fn open_notified(&mut self, agent: &str, cx: &mut Context<Self>) {
        let asked = (self.open_calls(&Place::This, agent).into_iter().next_back())
            .filter(|call| call.kind != CallKind::Look)
            .and_then(|call| Some((call.id.clone(), call.page.as_ref()?.id.clone())));
        match asked {
            Some((call, page)) => self.open_call_page_on_its_desk(&call, &page, cx),
            None => self.open_agent(agent, cx),
        }
    }

    /// The calls waiting on the person on every core this window reaches, each with where it
    /// waits, newest first.
    pub(crate) fn calls_everywhere(&self) -> Vec<(Place, &PersonCall)> {
        let mut open: Vec<(Place, &PersonCall)> = (self.reached_views())
            .flat_map(|(place, view)| {
                (view.calls.iter())
                    .filter(|call| call.state == CallState::Open)
                    .map(move |call| (place.clone(), call))
            })
            .collect();
        open.sort_by_key(|(_, call)| std::cmp::Reverse(call.raised_at));
        open
    }

    /// How many calls wait on the person on every core this window reaches.
    fn count_everywhere(&self) -> usize {
        (self.reached_views())
            .flat_map(|(_, view)| &view.calls)
            .filter(|call| call.state == CallState::Open)
            .count()
    }

    pub(crate) fn reached_views(&self) -> impl Iterator<Item = (Place, &DeviceView)> {
        self.views().filter(|(place, _)| self.feeds.reaches(place))
    }

    /// Where `call` waits, as a card names it: its project, and its host when that is not this one.
    fn call_place(&self, place: &Place, call: &PersonCall) -> CallPlace {
        let project = (self.view_at(place).into_iter())
            .flat_map(|view| &view.workspace.projects)
            .find(|project| project.path.to_string_lossy() == call.project)
            .map(|project| project.name.clone())
            .unwrap_or_else(|| {
                let path = std::path::Path::new(&call.project);
                (path.file_name()).map_or_else(
                    || call.project.clone(),
                    |name| name.to_string_lossy().into_owned(),
                )
            });
        let host = match place {
            Place::This => None,
            Place::Account(host) => Some(self.host_name(host).to_string()),
        };
        CallPlace { project, host }
    }

    /// The calls on the host at `place` that wait on the person and that agent `agent`'s chat
    /// lists, oldest first.
    pub(crate) fn open_calls(&self, place: &Place, agent: &str) -> Vec<&PersonCall> {
        self.view_at(place)
            .map_or_else(Vec::new, |view| open_in(&view.calls, agent))
    }

    /// The calls agent `agent`'s chat lists, as the core places them, oldest first, and those
    /// answered since its chat's last row.
    pub(crate) fn calls_of(&self, place: &Place, agent: &str, last_row: u64) -> CallsOf {
        let Some(view) = self.view_at(place) else {
            return CallsOf::default();
        };
        let mut of = CallsOf::default();
        for call in (view.calls.iter()).filter(|call| listed_in(call, agent)) {
            match call.state {
                CallState::Open => of.open.push(call.clone()),
                _ if settled_at(call) >= last_row => of.settled.push(call.clone()),
                _ => {}
            }
        }
        of.open.sort_by_key(|call| call.raised_at);
        of.settled.sort_by_key(settled_at);
        of
    }

    /// The call agent `agent`'s chat answers when the person presses a key there, on the
    /// host at `place`: the one chosen in the title bar's list while it waits on the agent, else the
    /// oldest, drawn nearest the composer.
    pub(crate) fn waiting_call(&self, place: &Place, agent: &str, cx: &App) -> Option<&PersonCall> {
        let opened = self.calls.read(cx).opened.clone();
        let open = (self.view_at(place)?.calls.iter())
            .filter(|call| call.state == CallState::Open && waits(call, agent));
        answered_in_chat(open, opened.as_deref())
    }

    /// The calls on the host at `place` that agent `agent` waits on or its chat lists.
    pub(crate) fn calls_held<'a>(
        &'a self,
        place: &Place,
        agent: &'a str,
    ) -> impl Iterator<Item = &'a PersonCall> {
        (self.view_at(place).into_iter())
            .flat_map(|view| &view.calls)
            .filter(move |call| {
                call.state == CallState::Open && (waits(call, agent) || listed_in(call, agent))
            })
    }

    /// The call `id` on the host at `place` that agent `agent` waits on, when its own chat does
    /// not list it.
    pub(crate) fn waits_elsewhere(
        &self,
        place: &Place,
        id: &str,
        agent: &str,
    ) -> Option<&PersonCall> {
        (self.view_at(place)?.calls.iter())
            .find(|call| call.id == id && waits(call, agent) && !listed_in(call, agent))
    }

    /// Lets go of the cards and the composers of calls answered or gone, and of the composers of
    /// desks gone, once a core's view says so.
    pub(crate) fn keep_cards(&mut self, cx: &mut Context<Self>) {
        let open: HashSet<&str> = (self.views())
            .flat_map(|(_, view)| view.calls.iter())
            .filter(|call| call.state == CallState::Open)
            .map(|call| call.id.as_str())
            .collect();
        self.calls.update(cx, |cards, _| cards.keep(&open));
        let closed: Vec<String> = (self.chats.replies.keys())
            .filter(|owner| match reply::About::of(owner) {
                Some(reply::About::Call(id)) => !open.contains(id),
                Some(reply::About::Page { agent, .. }) => self.agent_by_id(agent).is_none(),
                None => true,
            })
            .cloned()
            .collect();
        for owner in closed {
            self.chats.replies.remove(&owner);
        }
    }

    /// A key on call `id`'s card. Return answers it, as Go with this does; the rest choose its
    /// rows, unless its field holds the keys. Says whether the card took it.
    pub(crate) fn press_call(
        &mut self,
        id: &str,
        press: Press,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((_, call)) = self.call_at(id) else {
            return false;
        };
        let call = call.clone();
        let cards = self.calls.clone();
        // A production call is never sent by a key, wherever the focus is: only its button sends
        // it. Its numbers and arrows still move the choice.
        if press == Press::Pick {
            if call.kind != CallKind::Production {
                self.go_with(id, cx);
            }
            return true;
        }
        if cards.read(cx).field_focused(id, window, cx) {
            return false;
        }
        cards.update(cx, |cards, cx| cards.press(&call, press, cx))
    }

    /// Whether call `id` ships to production, which no key ever sends.
    pub(crate) fn ships_to_production(&self, id: &str) -> bool {
        self.call_at(id)
            .is_some_and(|(_, call)| call.kind == CallKind::Production)
    }

    /// Answers call `id` with what its card holds.
    pub(crate) fn go_with(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((_, call)) = self.call_at(id) else {
            return;
        };
        let Some(params) = self.calls.read(cx).answer(call, cx) else {
            return;
        };
        self.send_answer(params, cx);
    }

    /// Clears each open look call that shows page `id`, now the person has opened it.
    pub(crate) fn saw_page(&mut self, id: &str, cx: &mut Context<Self>) {
        let looks: Vec<String> = (self.calls_everywhere().into_iter())
            .filter(|(_, call)| call.kind == CallKind::Look)
            .filter(|(_, call)| call.pages().any(|page| page.id == id))
            .map(|(_, call)| call.id.clone())
            .collect();
        for look in looks {
            self.seen_look(&look, cx);
        }
    }

    /// Answers look call `id` as seen.
    pub(crate) fn seen_look(&mut self, id: &str, cx: &mut Context<Self>) {
        let params = AnswerCallParams {
            id: id.to_owned(),
            option: Some(LOOK_SEEN.to_owned()),
            ..AnswerCallParams::default()
        };
        self.send_answer(params, cx);
    }

    /// Opens the page look call `id` shows, which clears it.
    pub(crate) fn open_look(&mut self, id: &str, cx: &mut Context<Self>) {
        let shown = (self.call_at(id)).and_then(|(_, call)| Some(call.page.as_ref()?.id.clone()));
        match shown {
            Some(page) => self.open_call_page(id, &page, cx),
            None => self.seen_look(id, cx),
        }
    }

    /// Sends the answer `params` holds, once, its card sending until the core replies.
    fn send_answer(&mut self, params: AnswerCallParams, cx: &mut Context<Self>) {
        let id = params.id.clone();
        let Some((place, _)) = self.call_at(&id) else {
            return;
        };
        let cards = self.calls.clone();
        if cards.read(cx).sending(&id) {
            return;
        }
        cards.update(cx, |cards, cx| {
            cards.sending.insert(id.clone());
            cx.notify();
        });
        let asked = Target::new(&self.asks, place).request_typed::<call::AnswerCall>(params);
        cx.spawn(async move |bench, cx| {
            let failed = asked.await.err();
            bench
                .update(cx, |bench, cx| {
                    bench.calls.update(cx, |cards, cx| {
                        cards.sending.remove(&id);
                        cx.notify();
                    });
                    if let Some(error) = failed {
                        bench.notify(crate::toast::error(error), cx);
                    }
                })
                .ok();
        })
        .detach();
    }
}

#[derive(Default)]
pub(crate) struct CallsOf {
    pub open: Vec<PersonCall>,
    pub settled: Vec<PersonCall>,
}

/// Whether agent `agent` asks `call`, or waits on it beside its asker.
fn waits(call: &PersonCall, agent: &str) -> bool {
    call.asker.agent_id == agent || call.joined.iter().any(|joined| joined.agent_id == agent)
}

fn settled_at(call: &PersonCall) -> u64 {
    (call.outcome.as_ref().map(|outcome| outcome.at))
        .or(call.answer.as_ref().map(|answer| answer.at))
        .unwrap_or(call.raised_at)
}

/// The asker's own face, or its agent's mark; the chief wears Sikemux's.
fn asker_mark(bench: &Workbench, call: &PersonCall, size: f32, cx: &App) -> AnyElement {
    if call.asker.chief {
        return ui::chief_mark(size);
    }
    match bench.agent_by_id(&call.asker.agent_id) {
        Some(agent) => agent.mark(size, cx),
        None => ui::mark(&call.asker.cli, size, cx),
    }
}

/// What a card answers through: the core of its call, the cards' store and the window's store.
#[derive(Clone, Copy)]
struct Through<'a> {
    target: &'a Target,
    cards: &'a Entity<Cards>,
    bench: &'a WeakEntity<Workbench>,
}

/// The pages a call names, as the rows a chat shows for pages, framed under `id`: a click opens
/// one on the desk in front and closes the popover that held the row, if `dismiss` is how.
fn linked_page(
    id: SharedString,
    call: &PersonCall,
    dismiss: Option<&crate::ui::list_picker::Dismiss>,
    through: Through,
    cx: &mut App,
) -> Option<AnyElement> {
    let Through {
        target,
        cards,
        bench,
    } = through;
    let owner = call.about.as_ref().unwrap_or(&call.asker);
    let rows: Vec<AnyElement> = (call.pages().enumerate())
        .map(|(at, page)| {
            let (picture, load) = {
                let cards = cards.read(cx);
                (
                    cards.page_picture_cached(&page.id),
                    cards.needs_page_picture(&page.id),
                )
            };
            if load {
                let (cards, id, target) = (cards.clone(), page.id.clone(), target.clone());
                cx.defer(move |cx| {
                    cards.update(cx, |cards, cx| cards.load_page_picture(&id, &target, cx));
                });
            }
            let place = t!("@{name}'s artifact", name = owner.name).into();
            let open: crate::chat::page::Opens = {
                let (bench, call, page) = (bench.clone(), call.id.clone(), page.id.clone());
                let dismiss = dismiss.cloned();
                std::rc::Rc::new(move |window, cx| {
                    if let Some(dismiss) = &dismiss {
                        dismiss(window, cx);
                    }
                    let _ = bench.update(cx, |bench, cx| bench.open_call_page(&call, &page, cx));
                })
            };
            let page = sikemux_view::chat::PageRef {
                id: page.id.clone(),
                title: page.title.clone(),
            };
            crate::chat::page::tab_row(&page, picture, place, at > 0, open, cx)
        })
        .collect();
    (!rows.is_empty()).then(|| crate::chat::page::tab_frame(id, rows, cx))
}

/// The card of `call`: who asks, the question, its context and pictures, the options with the
/// field under them, the point against, and the footer that answers it.
fn card(
    call: &PersonCall,
    dismiss: crate::ui::list_picker::Dismiss,
    through: Through,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Through {
        target,
        cards,
        bench,
    } = through;
    let t = Tokens::get(cx).clone();
    let (field, seen, chosen, sending) = {
        let cards = cards.read(cx);
        (
            cards.held_field(&call.id),
            cards.has_seen(&call.id),
            cards.chosen(call),
            cards.sending(&call.id),
        )
    };
    if !seen || field.is_none() {
        let (cards, call, target) = (cards.clone(), call.clone(), target.clone());
        window.defer(cx, move |window, cx| {
            cards.update(cx, |cards, cx| {
                cards.saw(&call, &target);
                if cards.held_field(&call.id).is_none() {
                    cards.field(&call, window, cx);
                    cx.notify();
                }
            });
        });
    }
    let typed = field
        .as_ref()
        .is_some_and(|field| !field.read(cx).value().trim().is_empty());
    let who = who(call, through, None, cx);
    let question = div()
        .mt(SPACE_2)
        .mb(px(3.))
        .text_size(theme::ui_text(15.))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(relative(1.35))
        .text_color(t.text_primary)
        .child(selectable::plain(
            SharedString::from(format!("call-question-{}", call.id)),
            call.question.clone(),
        ));
    let pages = SharedString::from(format!("call-pages-{}", call.id));
    let page = linked_page(pages, call, Some(&dismiss), through, cx)
        .map(|page| div().mt(SPACE_3_PX).child(page));
    let prose = |part: &str, text: String| {
        div()
            .text_size(theme::ui_text(12.5))
            .line_height(relative(1.5))
            .text_color(t.text_secondary)
            .child(selectable::plain(
                SharedString::from(format!("call-{part}-{}", call.id)),
                text,
            ))
    };
    let context = context_words(call).map(|words| prose("context", words));
    let evidence_lines = (call.evidence.iter())
        .enumerate()
        .filter(|(_, evidence)| evidence.picture.is_none())
        .map(|(ix, evidence)| {
            prose(&format!("evidence-{ix}"), evidence.what.clone()).text_color(t.text_tertiary)
        });
    let options_pictured =
        !call.secret && call.options.iter().any(|option| option.picture.is_some());
    let pictures = match options_pictured {
        true => Some(strip(call, &chosen, cards, bench, &t)),
        false => {
            let shown: Vec<(&str, String)> = (call.evidence.iter())
                .filter_map(|evidence| Some((evidence.picture.as_deref()?, evidence.what.clone())))
                .take(2)
                .collect();
            let width = match shown.len() > 1 {
                true => (EVIDENCE_WIDTH - 6.) / 2.,
                false => EVIDENCE_WIDTH,
            };
            (!shown.is_empty()).then(|| {
                div()
                    .w_full()
                    .h(px(EVIDENCE_HEIGHT))
                    .flex()
                    .gap(px(6.))
                    .children(shown.into_iter().enumerate().map(|(ix, (path, what))| {
                        let (ratio, load) = {
                            let cards = cards.read(cx);
                            (cards.ratio_cached(path), cards.needs_ratio(path))
                        };
                        if load {
                            let (cards, path) = (cards.clone(), path.to_owned());
                            cx.defer(move |cx| {
                                cards.update(cx, |cards, cx| cards.load_ratio(&path, cx));
                            });
                        }
                        let fill = match ratio {
                            Some(ratio) => Fill::Top(ratio, width),
                            None => Fill::Cover,
                        };
                        let label = label_words(&what).map(picture_label);
                        picture(
                            SharedString::from(format!("call-evidence-{}-{ix}", call.id)),
                            path,
                            what,
                            fill,
                            bench,
                            &t,
                        )
                        .w(px(width))
                        .h_full()
                        .children(label)
                    }))
                    .into_any_element()
            })
        }
    };
    let rows =
        (call.options.iter()).map(|option| option_row(call, option, &chosen, sending, cards, &t));
    let hint = note_hint(call, &chosen);
    let note = match &field {
        Some(field) => {
            let focused = field.read(cx).focus_handle(cx).is_focused(window);
            note_row(field, hint, focused || typed, !call.options.is_empty(), &t).into_any_element()
        }
        None => div()
            .h(px(crate::theme::dense(ROW_HEIGHT)))
            .bg(t.surface_sunken)
            .into_any_element(),
    };
    let block = answers(rows, note, &t);
    let kept = kept_line(call, &t);
    let against = against_line(call, &t);
    let action_open = cards.read(cx).action_open(&call.id);
    let runs = (call.kind == CallKind::Production)
        .then_some(call.action.as_ref())
        .flatten()
        .map(|action| what_runs(call, action, action_open, cards, &t));
    let ready = !chosen.is_empty() || typed;
    let go = go_button(
        call,
        t!("Go with this").into(),
        ready,
        sending,
        bench,
        Workbench::go_with,
    );
    let (owner, bench, agent) = (target.clone(), bench.clone(), call.desk().to_owned());
    let open_chat = {
        Button::new(
            SharedString::from(format!("call-chat-{}", call.id)),
            t!("Open chat"),
            Variant::Ghost,
        )
        .icon(Icon::Message)
        .sized(26., 9., 12.)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            dismiss(window, cx);
            bench
                .update(cx, |bench, cx| {
                    bench.open_agent_from(&agent, owner.clone(), cx)
                })
                .ok();
        })
    };
    let buttons = div()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .child(open_chat)
        .child(go);
    let foot = div()
        .mt(px(14.))
        .flex()
        .items_center()
        .justify_end()
        .child(buttons);
    div()
        .p(px(10.))
        .flex()
        .flex_col()
        .child(who)
        .child(question)
        .children(context)
        .children(page)
        .children(evidence_lines)
        .map(|card| match pictures {
            Some(pictures) => card
                .child(div().mt(SPACE_3_PX).child(pictures))
                .child(div().h(px(14.))),
            None => card.child(div().h(SPACE_3_PX)),
        })
        .child(block)
        .children(kept)
        .children(against.map(|against| div().mt(SPACE_3_PX).child(against)))
        .children(runs)
        .child(foot)
        .into_any_element()
}

const SPACE_3_PX: gpui_kit::Pixels = theme::SPACE_3;

/// Who asks `call`, how long ago, and whom it is about, with `tail` at the end of the line.
fn who(
    call: &PersonCall,
    through: Through,
    tail: Option<AnyElement>,
    cx: &mut App,
) -> gpui_kit::Div {
    let Through { target, bench, .. } = through;
    let t = Tokens::get(cx).clone();
    let asker = (bench.upgrade())
        .map(|strong| asker_mark(strong.read(cx), call, 13., cx))
        .unwrap_or_else(|| ui::mark(&call.asker.cli, 13., cx));
    div()
        .h(px(FACE))
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(theme::ui_text(12.))
        .text_color(t.text_tertiary)
        .child(crate::chat::agent_pill_of(
            &call.asker.agent_id,
            &call.asker.name,
            asker,
            12.,
            target.clone(),
            bench,
            cx,
        ))
        .child(div().text_color(t.text_disabled).child("·"))
        .child(ui::time::age(call.raised_at))
        .when_some(call.about.as_ref(), |who, about| {
            who.child(div().text_color(t.text_disabled).child("·"))
                .child(t!("about"))
                .child(crate::chat::agent_pill_of(
                    &about.agent_id,
                    &about.name,
                    ui::mark(&about.cli, 13., cx),
                    12.,
                    target.clone(),
                    bench,
                    cx,
                ))
        })
        .children(tail)
}

/// Why the call came up, in the asker's words.
fn context_words(call: &PersonCall) -> Option<String> {
    match &call.brief {
        Some(brief) => Some(format!(
            "{} {} {}",
            brief.tried, brief.shows, brief.cannot_undo
        )),
        None => call.context.clone(),
    }
}

/// One option of `call` as a row that chooses it, with the chief's pick marked.
fn option_row(
    call: &PersonCall,
    option: &sikemux_view::calls::CallOption,
    chosen: &[String],
    sending: bool,
    cards: &Entity<Cards>,
    t: &Tokens,
) -> AnyElement {
    let on = chosen.contains(&option.id);
    let key = radio(on, t);
    let pick = (call.pick.as_ref() == Some(&option.id)).then(|| pick_mark(t).ml_auto().pl(SPACE_2));
    let label = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(div().child(selectable::plain(
            SharedString::from(format!("call-label-{}-{}", call.id, option.id)),
            option.label.clone(),
        )))
        .children(option.detail.clone().map(|detail| {
            div()
                .text_size(theme::ui_text(11.5))
                .line_height(relative(1.4))
                .text_color(t.text_tertiary)
                .child(selectable::plain(
                    SharedString::from(format!("call-detail-{}-{}", call.id, option.id)),
                    detail,
                ))
        }));
    let (cards, call) = (cards.clone(), call.clone());
    let option = option.id.clone();
    let hover = t.surface_hover;
    BaseButton::new(SharedString::from(format!("call-{}-{option}", call.id)))
        .w_full()
        .min_h(px(crate::theme::dense(ROW_HEIGHT)))
        .py(px(7.))
        .px(SPACE_3_PX)
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(theme::ui_text(13.))
        .text_color(t.text_primary)
        .disabled(sending)
        .map(|row| match on {
            true => row.bg(t.surface_chosen),
            false => row.hover(move |style| style.bg(hover)),
        })
        .on_click(move |click, window, cx| {
            cx.stop_propagation();
            if selectable::selected_by(click, window, cx) {
                return;
            }
            cards.update(cx, |cards, cx| cards.choose(&call, &option, cx));
        })
        .child(key)
        .child(label)
        .children(pick)
        .into_any_element()
}

/// The options as one bordered block of rows, with the note field as its last.
fn answers(
    rows: impl Iterator<Item = AnyElement>,
    note: impl IntoElement,
    t: &Tokens,
) -> gpui_kit::Div {
    let mut block = div()
        .flex()
        .flex_col()
        .border(crate::theme::border())
        .border_color(t.line)
        .rounded(px(8.))
        .overflow_hidden();
    for (ix, row) in rows.enumerate() {
        block = block.child(
            div()
                .when(ix > 0, |row| {
                    row.border_t(crate::theme::border()).border_color(t.line)
                })
                .child(row),
        );
    }
    block.child(note)
}

/// Says the person's words on a call for a secret reach only its asker.
fn kept_line(call: &PersonCall, t: &Tokens) -> Option<gpui_kit::Div> {
    call.secret.then(|| {
        div()
            .mt(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(theme::ui_text(11.5))
            .text_color(t.text_tertiary)
            .child(ui::icon(Icon::Shield, 11.).text_color(t.text_tertiary))
            .child(t!(
                "Kept private: @{name} reads it, and it never shows in the chat.",
                name = call.asker.name
            ))
    })
}

/// The chief's pick on an option.
fn pick_mark(t: &Tokens) -> gpui_kit::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .text_size(theme::ui_text(11.))
        .text_color(t.text_tertiary)
        .child(ui::chief_mark(12.))
        .child(t!("pick"))
}

fn note_hint(call: &PersonCall, chosen: &[String]) -> &'static str {
    match (chosen.is_empty(), call.secret) {
        (_, true) => t!("Type or paste it…"),
        (false, false) => t!("Add a note…"),
        (true, false) => t!("Or answer in your own words…"),
    }
}

fn against_line(call: &PersonCall, t: &Tokens) -> Option<gpui_kit::Div> {
    call.against.clone().map(|against| {
        let lead = t!("Against it:");
        div()
            .text_size(theme::ui_text(12.5))
            .line_height(relative(1.5))
            .text_color(t.text_secondary)
            .child(selectable::selectable(
                SharedString::from(format!("call-against-{}", call.id)),
                format!("{lead} {against}"),
                [(
                    0..lead.len(),
                    gpui_kit::HighlightStyle {
                        color: Some(t.text_tertiary),
                        ..Default::default()
                    },
                )],
            ))
    })
}

/// The button that answers `call`, worded `words`: `go` sends what its card or its page holds.
fn go_button(
    call: &PersonCall,
    words: SharedString,
    ready: bool,
    sending: bool,
    bench: &WeakEntity<Workbench>,
    go: fn(&mut Workbench, &str, &mut Context<Workbench>),
) -> Button {
    let (bench, id) = (bench.clone(), call.id.clone());
    let production = call.kind == CallKind::Production;
    Button::new(
        SharedString::from(format!("call-go-{}", call.id)),
        words,
        Variant::Primary,
    )
    .sized(26., 11., 12.)
    .disabled(!ready || sending)
    // A step to production is never sent by a key: its button takes no focus, so Tab never
    // reaches it, and a click that came from a key is refused.
    .mouse_only(production)
    .on_click(move |event, _, cx| {
        cx.stop_propagation();
        if production && matches!(event, ClickEvent::Keyboard(_)) {
            return;
        }
        bench.update(cx, |bench, cx| go(bench, &id, cx)).ok();
    })
}

/// What a production call runs on a yes, folded under a row that opens it.
fn what_runs(
    call: &PersonCall,
    action: &str,
    open: bool,
    cards: &Entity<Cards>,
    t: &Tokens,
) -> impl IntoElement {
    let (cards, id) = (cards.clone(), call.id.clone());
    let hover = t.text_secondary;
    let row = BaseButton::new(SharedString::from(format!("call-runs-{}", call.id)))
        .flex()
        .justify_start()
        .items_center()
        .gap(SPACE_1H)
        .text_size(theme::ui_text(12.))
        .text_color(t.text_tertiary)
        .hover(move |style| style.text_color(hover))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cards.update(cx, |cards, cx| {
                if let Some(held) = cards.held.get_mut(&id) {
                    held.action_open = !held.action_open;
                }
                cx.notify();
            });
        })
        .child(
            ui::icon(Icon::Chevron, 10.)
                .text_color(t.text_tertiary)
                .when(open, |chevron| {
                    chevron.with_transformation(gpui_kit::Transformation::rotate(
                        gpui_kit::Radians(std::f32::consts::FRAC_PI_2),
                    ))
                }),
        )
        .child(t!("What runs"));
    let command = open.then(|| {
        div()
            .mt(SPACE_1H)
            .font_family(theme::mono_font())
            .text_size(theme::ui_text(12.))
            .line_height(relative(1.5))
            .text_color(t.text_secondary)
            .child(selectable::plain(
                SharedString::from(format!("call-action-{}", call.id)),
                action.to_owned(),
            ))
    });
    div().mt(SPACE_2).child(row).children(command)
}

/// The last row of the options: a note that goes with the option chosen, or with none chosen,
/// the person's own answer.
fn note_row(
    field: &Entity<InputState>,
    hint: &'static str,
    lit: bool,
    under_options: bool,
    t: &Tokens,
) -> impl IntoElement {
    let ring = Hsla {
        a: 0.55,
        ..t.accent
    };
    div()
        .h(px(crate::theme::dense(ROW_HEIGHT)))
        .px(SPACE_3_PX)
        .flex()
        .items_center()
        .gap(px(10.))
        .bg(t.surface_sunken)
        .when(under_options && !lit, |row| {
            row.border_t(crate::theme::border()).border_color(t.line)
        })
        .when(lit, |row| {
            row.border(crate::theme::border())
                .border_color(ring)
                .when(under_options, |row| row.rounded_b(px(7.)))
                .when(!under_options, |row| row.rounded(px(7.)))
        })
        .child(ui::icon(Icon::Pencil, 11.).text_color(match lit {
            true => t.text_tertiary,
            false => t.text_disabled,
        }))
        .child(hinted_field(field, hint, lit, t))
}

/// A call's field, with `hint` over it until it is `lit`.
fn hinted_field(
    field: &Entity<InputState>,
    hint: &'static str,
    lit: bool,
    t: &Tokens,
) -> gpui_kit::Div {
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .text_size(theme::ui_text(13.))
        .text_color(t.text_primary)
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .text_color(t.text_disabled)
                .when(lit, |hint| hint.invisible())
                .child(hint),
        )
        .child(
            // Its own key context keeps the chat's question keys off the field, so Return
            // answers this call, as Return in the composer sends.
            div().w_full().key_context("CallField").child(
                Input::new(field)
                    .appearance(false)
                    .bordered(false)
                    .p_0()
                    .text_size(theme::ui_text(13.)),
            ),
        )
}

/// The ring of an option on a call that takes one.
fn radio(on: bool, t: &Tokens) -> AnyElement {
    let ring = div().size(px(13.)).flex_none().rounded_full();
    match on {
        true => ring.border(px(4.)).border_color(t.accent).bg(t.on_accent),
        false => ring.border(px(1.5)).border_color(t.text_disabled),
    }
    .into_any_element()
}

/// How a picture fills its place on the card.
#[derive(Clone, Copy)]
enum Fill {
    /// All of it covered, cut evenly at the edges.
    Cover,
    /// All of the picture shown, with bars where its shape differs.
    Contain,
    /// From its top, as a page is read, given its width over its height and the width it
    /// fills: its full width, or wider when that would leave its place short.
    Top(f32, f32),
}

/// A picture on the card, which opens full size.
fn picture(
    id: SharedString,
    path: &str,
    title: String,
    fill: Fill,
    bench: &WeakEntity<Workbench>,
    t: &Tokens,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let shown = Picture::File(PathBuf::from(path));
    let (bench, opened, title) = (bench.clone(), shown.clone(), SharedString::from(title));
    div()
        .id(id)
        .relative()
        .overflow_hidden()
        .rounded(px(8.))
        .bg(t.surface_sunken)
        .border(crate::theme::border())
        .border_color(t.line)
        .cursor_pointer()
        .child(match fill {
            Fill::Cover => img(shown.source_at(px(PICTURE_SIZE)))
                .size_full()
                .object_fit(ObjectFit::Cover),
            Fill::Contain => img(shown.source_at(px(PICTURE_SIZE)))
                .size_full()
                .object_fit(ObjectFit::Contain),
            Fill::Top(ratio, width) => {
                let height = (width / ratio).max(EVIDENCE_HEIGHT);
                img(shown.source_at(px(PICTURE_SIZE)))
                    .w(px(height * ratio))
                    .h(px(height))
            }
            .object_fit(ObjectFit::Cover),
        })
        .clicks(move |_, window, cx| {
            cx.stop_propagation();
            bench
                .update(cx, |bench, cx| {
                    bench.show_picture_titled(opened.clone(), title.clone(), window, cx)
                })
                .ok();
        })
}

/// An evidence item's own words, when they are short enough to be a name on its picture (Now,
/// New), else none.
fn label_words(words: &str) -> Option<String> {
    let trimmed = words.trim();
    let short = trimmed.chars().count() <= 20 && trimmed.split_whitespace().count() <= 3;
    (!trimmed.is_empty() && short).then(|| trimmed.to_owned())
}

/// A picture's name on its corner, such as Now and New on two evidence pages side by side.
fn picture_label(text: String) -> gpui_kit::Div {
    div()
        .absolute()
        .left(px(6.))
        .bottom(px(6.))
        .max_w(relative(0.9))
        .px(px(6.))
        .py(px(2.))
        .rounded(px(5.))
        .bg(gpui_kit::black().opacity(0.62))
        .text_color(gpui_kit::white())
        .text_size(theme::ui_text(11.))
        .font_weight(FontWeight::MEDIUM)
        .child(div().min_w_0().truncate().child(text))
}

/// A taste call's pictures, one per option: the chosen option's leads, large, and the others
/// wait beside it, smaller and dimmed; a click on one chooses it.
fn strip(
    call: &PersonCall,
    chosen: &[String],
    cards: &Entity<Cards>,
    bench: &WeakEntity<Workbench>,
    t: &Tokens,
) -> AnyElement {
    let lead = (chosen.first())
        .or(call.pick.as_ref())
        .and_then(|id| call.options.iter().position(|option| &option.id == id))
        .unwrap_or(0);
    let shown = |ix: usize, option: &sikemux_view::calls::CallOption| {
        let id = SharedString::from(format!("call-picture-{}-{ix}", call.id));
        let fill = if ix == lead {
            Fill::Contain
        } else {
            Fill::Cover
        };
        match option.picture.as_deref() {
            Some(path) => picture(id, path, option.label.clone(), fill, bench, t),
            None => div().id(id).rounded(px(8.)).bg(t.surface_sunken),
        }
    };
    let rest = (call.options.iter().enumerate())
        .filter(|(ix, _)| *ix != lead)
        .map(|(ix, option)| {
            let (cards, call, id) = (cards.clone(), call.clone(), option.id.clone());
            shown(ix, option)
                .flex_1()
                .w_full()
                .opacity(0.72)
                .clicks(move |_, _, cx| {
                    cx.stop_propagation();
                    cards.update(cx, |cards, cx| cards.choose(&call, &id, cx));
                })
        });
    div()
        .h(px(STRIP_HEIGHT))
        .flex()
        .gap(SPACE_1H)
        .children(call.options.get(lead).map(|option| {
            shown(lead, option)
                .w(px(STRIP_LEAD))
                .flex_none()
                .h_full()
                .border_color(t.border_strong)
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .children(rest),
        )
        .into_any_element()
}

/// What came of a call, in one quiet line where its card stood: what the person said and what
/// it led to.
/// How the person answered `call`, as the line left in its place words it; never the words of a
/// call that keeps them private.
fn answer_words(call: &PersonCall, answer: &CallAnswer) -> String {
    let label = |id: &String| {
        (call.options.iter())
            .find(|option| &option.id == id)
            .map(|option| format!("“{}”", option.label))
    };
    let picked: Vec<String> = answer.option.iter().filter_map(label).collect();
    let private = call.secret && picked.is_empty();
    match (&answer.by, picked.is_empty(), private) {
        (_, _, true) => t!("You answered, kept private").to_owned(),
        (CallClient::Chat, ..) => t!("You answered in the chat").to_owned(),
        (_, false, _) => t!("You chose {options}", options = picked.join(", ")),
        (_, true, _) => t!("You answered in your own words").to_owned(),
    }
}

fn notice(call: &PersonCall, t: &Tokens) -> Option<AnyElement> {
    let answered = (call.answer.as_ref()).map(|answer| answer_words(call, answer));
    let outcome = call.outcome.as_ref().and_then(|outcome| {
        Some(match (outcome.result, outcome.healthy) {
            (CallResult::Shipped, Some(true)) => t!("shipped · healthy").to_owned(),
            (CallResult::Shipped, Some(false)) => t!("shipped · unhealthy").to_owned(),
            (CallResult::Shipped, None) => t!("shipped").to_owned(),
            (CallResult::NotShipped, _) => t!("not shipped").to_owned(),
            (CallResult::Failed, _) => t!("it failed").to_owned(),
            (CallResult::MayHaveRun, _) => t!("may have run, Sikemux restarted").to_owned(),
            (CallResult::Withdrawn, _) if outcome.why == Some(CallWhy::TakenBack) => {
                t!("@{name} took it back", name = call.asker.name)
            }
            (CallResult::Withdrawn, _) => {
                t!("@{name} went on without an answer", name = call.asker.name)
            }
            (CallResult::Heard | CallResult::HandedBack | CallResult::Unknown, _) => return None,
        })
    });
    let said = match (answered, outcome) {
        (Some(answered), Some(outcome)) => format!("{answered} · {outcome}"),
        (Some(said), None) | (None, Some(said)) => said,
        (None, None) => return None,
    };
    let lost = matches!(
        call.outcome.as_ref().map(|outcome| outcome.result),
        Some(CallResult::Failed)
    );
    Some(
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(theme::ui_text(12.))
            .text_color(t.text_tertiary)
            .child(
                ui::icon(if lost { Icon::Close } else { Icon::Check }, 11.)
                    .text_color(t.text_tertiary),
            )
            .child(div().min_w_0().truncate().child(said))
            .child(div().text_color(t.text_disabled).child("·"))
            .children(crate::chat::clock(settled_at(call)))
            .into_any_element(),
    )
}

/// The calls an agent asks, at the end of its chat: settled notices above the activity line,
/// and open calls below it.
pub struct InChat {
    bench: WeakEntity<Workbench>,
    agent: String,
    cards: Entity<Cards>,
    target: Target,
    calls: CallsOf,
    unfolded: bool,
    _cards: Subscription,
}

impl InChat {
    pub(crate) fn new(
        bench: WeakEntity<Workbench>,
        cards: Entity<Cards>,
        target: Target,
        agent: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        let observed = cx.observe(&cards, |_, _, cx| cx.notify());
        Self {
            bench,
            agent: agent.to_owned(),
            cards,
            target,
            calls: CallsOf::default(),
            unfolded: false,
            _cards: observed,
        }
    }

    pub(crate) fn show(&mut self, calls: CallsOf, cx: &mut Context<Self>) {
        if self.calls.open != calls.open || self.calls.settled != calls.settled {
            self.calls = calls;
            cx.notify();
        }
    }

    pub(crate) fn settled(&self, cx: &App) -> Option<AnyElement> {
        let t = Tokens::get(cx);
        let notices: Vec<AnyElement> = (self.calls.settled.iter())
            .filter_map(|call| notice(call, t))
            .collect();
        if notices.is_empty() {
            return None;
        }
        Some(
            div()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w_full()
                        .max_w(crate::chat::column::rows(cx))
                        .px(px(22.))
                        .pt(SPACE_2)
                        .pb(SPACE_1)
                        .flex()
                        .flex_col()
                        .gap(SPACE_2)
                        .children(notices),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn open(&self, this: WeakEntity<Self>, cx: &App) -> Option<AnyElement> {
        if self.calls.open.is_empty() {
            return None;
        }
        let through = Through {
            target: &self.target,
            cards: &self.cards,
            bench: &self.bench,
        };
        let toggle = move |cx: &mut App| {
            this.update(cx, |chat, cx| {
                chat.unfolded = !chat.unfolded;
                cx.notify();
            })
            .ok();
        };
        let row = match self.calls.open.as_slice() {
            [call] => page::chat_row(call, &self.agent, through, page::RowFrame::Own, cx),
            open => page::chat_fold(open, self.unfolded, toggle, &self.agent, through, cx),
        };
        Some(
            div()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w_full()
                        .max_w(crate::chat::column::rows(cx))
                        .px(px(22.))
                        .pt(SPACE_3)
                        .flex()
                        .flex_col()
                        .gap(SPACE_2)
                        .child(row),
                )
                .into_any_element(),
        )
    }
}

/// Of an agent's open calls, the one its chat answers: the one chosen while it is among them,
/// else the oldest.
fn answered_in_chat<'a>(
    open: impl Iterator<Item = &'a PersonCall> + Clone,
    opened: Option<&str>,
) -> Option<&'a PersonCall> {
    (open.clone())
        .find(|call| Some(call.id.as_str()) == opened)
        .or_else(|| open.min_by_key(|call| call.raised_at))
}

/// The top bar's bell over the calls waiting on the person in every project on every host this
/// window reaches, with their count, which opens them; none while nothing waits.
pub(crate) fn chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let count = bench.count_everywhere();
    if count == 0 {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let this = cx.entity().downgrade();
    let cards = bench.calls.clone();
    let words = match count {
        1 => t!("1 ask").to_owned(),
        count => t!("{count} asks", count = count),
    };
    let hover = t.surface_hover;
    let chip = Popover::new("calls-chip")
        .anchor(Anchor::TopRight)
        .offset(SPACE_1H)
        .trigger_with(move |open, _, cx| {
            let button = BaseButton::new("calls-chip-button")
                .accessibility_label(t!("Asks"))
                .h(px(22.))
                .px(px(crate::tabs::CHIP_INSET))
                .flex()
                .items_center()
                .rounded(radius_2())
                .cursor_pointer()
                .map(|chip| {
                    gpui_kit::StatefulInteractiveElement::tooltip(
                        chip,
                        crate::ui::tooltip(words.clone()),
                    )
                })
                .map(|chip| match open {
                    true => chip.bg(t.surface_chosen),
                    false => chip.hover(move |style| style.bg(hover)),
                })
                .child(crate::ui::icon(Icon::Bell, 14.).text_color(t.text_primary));
            div()
                .relative()
                .child(button)
                .child(crate::ui::count_badge(count, cx))
                .into_any_element()
        })
        .content(move |_, window, cx| {
            let Some(bench) = this.upgrade() else {
                return div().into_any_element();
            };
            let dismiss = crate::ui::list_picker::dismiss(cx.entity());
            popover(&bench, &cards, dismiss, window, cx)
        });
    Some(chip.into_any_element())
}

/// The calls waiting everywhere: the open one as its card, the rest a line each under it, each
/// answered through the core it waits on.
fn popover(
    bench: &Entity<Workbench>,
    cards: &Entity<Cards>,
    dismiss: crate::ui::list_picker::Dismiss,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let asks = bench.read(cx).asks.clone();
    let calls: Vec<(Target, CallPlace, PersonCall)> = {
        let read = bench.read(cx);
        (read.calls_everywhere().into_iter())
            .map(|(place, call)| {
                let named = read.call_place(&place, call);
                (Target::new(&asks, place), named, call.clone())
            })
            .collect()
    };
    let opened = cards.read(cx).opened.clone();
    let Some((target, name, open)) = (opened.as_ref())
        .and_then(|id| calls.iter().find(|(_, _, call)| &call.id == id))
        .or(calls.first())
        .cloned()
    else {
        return div().into_any_element();
    };
    let weak = bench.downgrade();
    let through = Through {
        target: &target,
        cards,
        bench: &weak,
    };
    let shown = card(&open, dismiss, through, window, cx);
    let rest: Vec<AnyElement> = (calls.iter())
        .filter(|(_, _, call)| call.id != open.id)
        .map(|(target, named, call)| {
            let elsewhere = (named != &name).then_some(named);
            folded(cards, call, elsewhere, target, &weak, cx)
        })
        .collect();
    let head = div()
        .px(px(10.))
        .pt(SPACE_1H)
        .pb(SPACE_1)
        .flex()
        .items_center()
        .gap(px(10.))
        .child(ui::caps(t!("Asks"), cx))
        .child(div().flex_1().h(px(1.)).bg(t.line))
        .child(name.tag(11., cx));
    call_keys(
        div().id("calls-popover").key_context("QuestionDock"),
        &open.id,
        &weak,
    )
    .w(px(CARD_WIDTH))
    .p(SPACE_1H)
    .bg(t.surface_overlay)
    .border(crate::theme::border())
    .border_color(t.line)
    .rounded(radius_4())
    .shadow(t.shadow_float())
    .child(head)
    .child(shown)
    .when(!rest.is_empty(), |pop| {
        pop.child(div().mx(px(10.)).my(SPACE_1).h(px(1.)).bg(t.line))
            .children(rest)
    })
    .into_any_element()
}

/// A call waiting behind the one open: who asks, the question, where it waits when that is not
/// where the open one does, and how long it has waited. A click opens it in its place.
fn folded(
    cards: &Entity<Cards>,
    call: &PersonCall,
    elsewhere: Option<&CallPlace>,
    target: &Target,
    bench: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let asker = (bench.upgrade())
        .map(|strong| asker_mark(strong.read(cx), call, 12., cx))
        .unwrap_or_else(|| ui::mark(&call.asker.cli, 12., cx));
    let (cards, id) = (cards.clone(), call.id.clone());
    let hover = t.surface_hover;
    BaseButton::new(SharedString::from(format!("call-row-{}", call.id)))
        .w_full()
        .h(px(30.))
        .pl(SPACE_2)
        .pr(px(10.))
        .flex()
        .items_center()
        .gap(px(9.))
        .rounded(radius_2())
        .text_size(theme::ui_text(13.))
        .text_color(t.text_primary)
        .hover(move |style| style.bg(hover))
        .on_click(move |_, _, cx| {
            cards.update(cx, |cards, cx| {
                cards.opened = Some(id.clone());
                cx.notify();
            });
        })
        .child(crate::chat::agent_pill_of(
            &call.asker.agent_id,
            &call.asker.name,
            asker,
            11.5,
            target.clone(),
            bench,
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(call.headline.clone()),
        )
        .children(elsewhere.map(|named| named.tag(11.5, cx).flex_none().max_w(px(140.))))
        .child(
            div()
                .flex_none()
                .w(px(26.))
                .flex()
                .justify_end()
                .text_size(theme::ui_text(11.5))
                .font_features(theme::tabular())
                .text_color(t.text_tertiary)
                .child(ui::time::age(call.raised_at)),
        )
        .into_any_element()
}

/// Where a call waits: its project's name, and its host's when that is not this one.
#[derive(Clone, PartialEq)]
struct CallPlace {
    project: String,
    host: Option<String>,
}

impl CallPlace {
    /// The place in `size` text, each name after the mark its rail row carries.
    fn tag(&self, size: f32, cx: &App) -> gpui_kit::Div {
        let named = |glyph: gpui_kit::Svg, name: &str| {
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(glyph)
                .child(div().min_w_0().truncate().child(name.to_owned()))
        };
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(theme::ui_text(size))
            .text_color(Tokens::get(cx).text_tertiary)
            .child(named(crate::rail::project_glyph(size, cx), &self.project))
            .children(
                (self.host.as_deref())
                    .map(|host| named(crate::rail::host_glyph(size, true, cx), host)),
            )
    }
}

/// The keys a card takes, bound as the question rows' are, for call `id`.
fn call_keys<E: InteractiveElement>(keyed: E, id: &str, bench: &WeakEntity<Workbench>) -> E {
    use crate::chat::{Choose, Confirm, NextChoice, PreviousChoice};
    fn on<A: gpui_kit::Action>(
        id: &str,
        bench: &WeakEntity<Workbench>,
        press: impl Fn(&A) -> Press + 'static,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        let (id, bench) = (id.to_owned(), bench.clone());
        move |action, window, cx| {
            let taken = bench.update(cx, |bench, cx| {
                bench.press_call(&id, press(action), window, cx)
            });
            if !matches!(taken, Ok(true)) {
                cx.propagate();
            }
        }
    }
    keyed
        .on_action(on(id, bench, |choose: &Choose| Press::Choose(choose.0)))
        .on_action(on(id, bench, |_: &PreviousChoice| Press::Up))
        .on_action(on(id, bench, |_: &NextChoice| Press::Down))
        .on_action(on(id, bench, |_: &Confirm| Press::Pick))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, asker: &str, about: &str, shown_in: &[&str]) -> PersonCall {
        let agent = |id: &str| serde_json::json!({ "agentId": id, "name": id, "cli": "claude" });
        serde_json::from_value(serde_json::json!({
            "id": id,
            "project": "/sikemux",
            "kind": "taste",
            "asker": agent(asker),
            "about": agent(about),
            "question": "?",
            "options": [],
            "canHandBack": false,
            "state": "open",
            "raisedAt": 1,
            "shownIn": shown_in,
        }))
        .unwrap()
    }

    #[test]
    fn a_chiefs_call_about_another_tree_stays_out_of_a_leads_desk() {
        let calls = [
            call("github", "hazamada", "verdia", &["hazamada", "chief"]),
            call("nexus", "chief", "rui", &["chief", "rui"]),
        ];
        let ids = |agent| {
            open_in(&calls, agent)
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("hazamada"), ["github"]);
        assert_eq!(ids("rui"), ["nexus"]);
        assert_eq!(ids("chief"), ["github", "nexus"]);
    }

    #[test]
    fn a_choice_tapped_on_the_asks_page_is_kept_without_a_card_field() {
        let gate: PersonCall = serde_json::from_value(serde_json::json!({
            "id": "gate",
            "project": "/sikemux",
            "kind": "production",
            "asker": { "agentId": "worker", "name": "worker", "cli": "claude" },
            "question": "Change Production with this work?",
            "options": [
                { "id": "allow", "label": "Change Production" },
                { "id": "refuse", "label": "Not now" },
            ],
            "action": "deploy --prod",
            "canHandBack": false,
            "state": "open",
            "raisedAt": 1,
        }))
        .unwrap();
        let mut cards = Cards::default();
        assert!(cards.chosen(&gate).is_empty());

        cards.toggle(&gate, "allow");
        assert_eq!(cards.chosen(&gate), ["allow"]);

        cards.toggle(&gate, "allow");
        assert!(cards.chosen(&gate).is_empty());
    }
}

//! The Hosts page: every host on the account at a glance, and one host's detail floating over them.

mod board;
mod chart;
mod detail;
pub mod model;

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, Subscription, Window,
};
use sikemux_core::server::call;
use sikemux_view::host_stats::{HostStats, HostStatsParams, StatsSpan};
use sikemux_view::layout::{OpenTab, PaneKind};

use crate::core::Target;
use crate::hosts::Place;
use crate::reads::Read;
use crate::theme::{self, Tokens, RADIUS_4, SPACE_3};
use crate::ui;
use crate::workbench::Workbench;

/// The narrowest and widest the chosen host's detail is drawn.
const DETAIL_MIN: f32 = 360.;
const DETAIL_MAX: f32 = 540.;

/// What this window keeps of the Hosts page while it is open: which host is chosen, how the
/// hosts are laid out and narrowed, and what is watched for it.
pub struct Page {
    pub chosen: Option<String>,
    pub table: bool,
    pub needs_you: bool,
    pub span: StatsSpan,
    pub search: Entity<InputState>,
    pub top_open: bool,
    pub top_by_memory: bool,
    pub history_open: bool,
    /// The people whose agents are folded away in the detail, by name.
    pub folded: HashSet<String>,
    /// How wide the page was last drawn.
    width: Rc<Cell<f32>>,
    /// The cores watched for the page, each with the span asked of it.
    watching: Vec<(Place, StatsSpan)>,
    _typing: Subscription,
}

impl Page {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search hosts and agents"));
        let typing = cx.subscribe(&search, |bench, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                bench.parts.redraw_stage(cx);
            }
        });
        Self {
            chosen: None,
            table: false,
            needs_you: false,
            span: StatsSpan::Hour,
            search,
            top_open: false,
            top_by_memory: false,
            history_open: false,
            folded: HashSet::new(),
            width: Rc::default(),
            watching: Vec::new(),
            _typing: typing,
        }
    }
}

pub fn pane(
    bench: &Workbench,
    _target: &str,
    _window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let listed = model::hosts(bench);
    let chosen = chosen_key(bench, &listed);
    let width = bench.hosts_page.width.clone();
    let detail_width = (width.get() * 0.42).clamp(DETAIL_MIN, DETAIL_MAX);
    let host = listed
        .iter()
        .find(|host| Some(&host.key) == chosen.as_ref());
    let detail = host.map(|host| detail::detail(bench, host, &listed, cx));
    let board = board::board(bench, &listed, chosen.as_deref(), width.get(), cx);
    let stage = bench.parts.stage.clone();
    ui::pane(cx)
        .child(
            div()
                .size_full()
                .relative()
                .on_children_prepainted(move |bounds, _, cx| {
                    let drawn = bounds
                        .first()
                        .map_or(0., |board| f32::from(board.size.width));
                    if (drawn - width.get()).abs() > 0.5 {
                        width.set(drawn);
                        stage.update(cx, |_, cx| cx.notify());
                    }
                })
                .child(
                    div()
                        .id("hosts-board")
                        .size_full()
                        .overflow_y_scroll()
                        .child(board),
                )
                .children(detail.map(|detail| {
                    div()
                        .id("hosts-detail")
                        .absolute()
                        .top(SPACE_3)
                        .right(SPACE_3)
                        .bottom(SPACE_3)
                        .w(px(detail_width))
                        .occlude()
                        .overflow_y_scroll()
                        .rounded(RADIUS_4)
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.surface_sunken)
                        .shadow(t.shadow_float())
                        .child(detail)
                })),
        )
        .into_any_element()
}

/// The host whose detail shows: the one the person chose, while it is still listed.
fn chosen_key(bench: &Workbench, listed: &[model::Listed]) -> Option<String> {
    bench
        .hosts_page
        .chosen
        .clone()
        .filter(|key| listed.iter().any(|host| &host.key == key))
}

impl Workbench {
    fn close_on_page(&mut self, cx: &mut Context<Self>) {
        self.hosts_page.chosen = None;
        self.watch_hosts_page();
        self.parts.redraw_stage(cx);
    }

    /// Opens the Hosts page in the strip in front, on `host`'s detail when one is named: this
    /// host by "this" or its own core key, another by its core key.
    pub fn open_hosts(&mut self, host: Option<String>) {
        if let Some(host) = host {
            self.hosts_page.chosen = Some(model::page_key(self, &host));
        }
        if let Some(strip) = self.strip() {
            self.open_in_front(OpenTab::new(strip.at(), PaneKind::Hosts, ""));
        }
        self.watch_hosts_page();
    }

    pub fn hosts_page_in_front(&self) -> bool {
        self.hosts_page_target().is_some()
    }

    /// The host the Hosts page in front was opened on, "" for none, while the tab in front
    /// shows the page, alone or beside other panes.
    fn hosts_page_target(&self) -> Option<String> {
        let tab = self.tab()?;
        crate::panes::views(&tab.pane)
            .find(|view| view.kind == PaneKind::Hosts)
            .map(|view| view.target.to_owned())
    }

    /// Keeps every host the page lists watched while it is in front, the chosen one over the
    /// span picked and the rest for now alone, and lets go of them all once it is not. The
    /// core samples what its processes use only while it is watched, so the watch stays this
    /// tight.
    pub fn watch_hosts_page(&mut self) {
        let wanted: Vec<(Place, StatsSpan)> = match self.hosts_page_target() {
            Some(_) => {
                let listed = model::hosts(self);
                let chosen = chosen_key(self, &listed);
                (listed.iter())
                    .filter(|host| host.online())
                    .map(|host| {
                        let span = match Some(&host.key) == chosen.as_ref() {
                            true => self.hosts_page.span,
                            false => StatsSpan::Now,
                        };
                        (host.place.clone(), span)
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let watching = std::mem::take(&mut self.hosts_page.watching);
        for (place, _) in &watching {
            if !wanted.iter().any(|(kept, _)| kept == place) {
                Target::new(&self.asks, place.clone()).unwatch(Read::HostStats);
            }
        }
        for (place, span) in &wanted {
            if !watching.contains(&(place.clone(), *span)) {
                Target::new(&self.asks, place.clone())
                    .watch::<call::HostStats>(Read::HostStats, HostStatsParams { span: *span });
            }
        }
        self.hosts_page.watching = wanted;
    }

    /// Takes in what a core said of its host for the page.
    pub(crate) fn host_stats_read(
        &mut self,
        place: Place,
        stats: HostStats,
        cx: &mut Context<Self>,
    ) {
        let feed = self.feeds.at_mut(&place);
        if feed.stats.as_ref() == Some(&stats) {
            return;
        }
        feed.stats = Some(stats);
        if self.hosts_page_in_front() {
            self.parts.redraw_stage(cx);
        }
    }

    fn choose_on_page(&mut self, key: String, cx: &mut Context<Self>) {
        let again = self.hosts_page.chosen.as_ref() == Some(&key);
        self.hosts_page.chosen = (!again).then_some(key);
        self.hosts_page.top_open = false;
        self.watch_hosts_page();
        self.parts.redraw_stage(cx);
    }
}

/// What a card or panel is filled with: a step up from the page, see-through where it is.
fn card(t: &Tokens) -> Hsla {
    t.surface_raised.opacity(0.55)
}

/// The hairline between parts of a card.
fn hair(t: &Tokens) -> Hsla {
    t.border.opacity(0.7)
}

/// The ink, faint, as a tile's ground or a meter's unlit ticks.
fn ink(t: &Tokens, share: f32) -> Hsla {
    t.text_primary.opacity(share)
}

/// `tint` washed over the card, as an alert's ground.
fn wash(tint: Hsla, t: &Tokens, share: f32) -> Hsla {
    theme::mix(tint, t.surface_raised, share).opacity(0.55)
}

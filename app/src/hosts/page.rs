//! The Devices page: every host on the account at a glance, one host's detail floating over
//! them, and the phones and browsers paired here.

mod board;
mod chart;
mod detail;
mod devices;
pub mod glance;
pub mod model;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, px, relative, AnyElement, AppContext, Context, Entity, Hsla, Image, ImageFormat,
    InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    Subscription, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::host_stats::apps::AppIconParams;
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
    /// The host picker's glance while it is open, drawn again when its host's stats change.
    pub glance: Option<gpui_kit::WeakEntity<glance::Glance>>,
    /// The people whose agents are folded away in the detail, by name.
    pub folded: HashSet<String>,
    /// Each app's icon by its key, empty while it is asked for or where it has none.
    pub app_icons: HashMap<String, Option<Arc<Image>>>,
    /// The cores watched for the page, each with the span asked of it.
    watching: Vec<(Place, StatsSpan)>,
    _typing: Subscription,
}

impl Page {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Search hosts, agents and devices"));
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
            glance: None,
            folded: HashSet::new(),
            app_icons: HashMap::new(),
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
    let host = listed
        .iter()
        .find(|host| Some(&host.key) == chosen.as_ref());
    let detail = host.map(|host| detail::detail(bench, host, &listed, cx));
    let board = board::board(bench, &listed, chosen.as_deref(), cx);
    ui::pane(cx)
        .child(
            div()
                .size_full()
                .relative()
                .flex()
                .flex_col()
                .child(board.controls)
                .child(
                    div()
                        .id("hosts-board")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(board.body),
                )
                .children(detail.map(|detail| {
                    div()
                        .id("hosts-detail")
                        .absolute()
                        .top(SPACE_3)
                        .right(SPACE_3)
                        .bottom(SPACE_3)
                        .w(relative(0.42))
                        .min_w(px(DETAIL_MIN))
                        .max_w(px(DETAIL_MAX))
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
        let glancing = self.hosts_page.glance.is_some();
        let wanted: Vec<(Place, StatsSpan)> = match self.hosts_page_target() {
            None if glancing => {
                let place = self.place();
                (model::hosts(self).iter())
                    .filter(|host| host.place == place && host.online())
                    .map(|host| (host.place.clone(), StatsSpan::Minutes15))
                    .collect()
            }
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
        let apps: Vec<String> = (stats.top.iter())
            .filter_map(|process| process.app.clone())
            .filter(|app| !self.hosts_page.app_icons.contains_key(app))
            .collect();
        feed.stats = Some(stats);
        for app in apps {
            self.ask_app_icon(&place, app, cx);
        }
        if self.hosts_page_in_front() {
            self.parts.redraw_stage(cx);
        }
        self.redraw_glance(cx);
    }

    fn redraw_glance(&self, cx: &mut Context<Self>) {
        if let Some(glance) = self
            .hosts_page
            .glance
            .as_ref()
            .and_then(|glance| glance.upgrade())
        {
            glance.update(cx, |_, cx| cx.notify());
        }
    }

    fn ask_app_icon(&mut self, place: &Place, app: String, cx: &mut Context<Self>) {
        self.hosts_page.app_icons.insert(app.clone(), None);
        let asked = Target::new(&self.asks, place.clone())
            .request_typed::<call::AppIcon>(AppIconParams { app: app.clone() });
        cx.spawn(async move |bench, cx| {
            use base64::Engine;
            let Some(png) = (asked.await.ok().flatten())
                .and_then(|png| base64::engine::general_purpose::STANDARD.decode(png).ok())
            else {
                return;
            };
            let _ = bench.update(cx, |bench, cx| {
                let icon = Arc::new(Image::from_bytes(ImageFormat::Png, png));
                bench.hosts_page.app_icons.insert(app, Some(icon));
                if bench.hosts_page_in_front() {
                    bench.parts.redraw_stage(cx);
                }
                bench.redraw_glance(cx);
            });
        })
        .detach();
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

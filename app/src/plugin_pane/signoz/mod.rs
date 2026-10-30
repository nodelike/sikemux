//! SigNoz's pane: the services with their health, a service's page, logs, traces and one
//! trace, and dashboards, all narrowed by a time window, an environment and filters.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, rgb, AnyElement, App, AppContext, Context, Div, Entity, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::SignInStatus;
use sikemux_plugin_signoz_views::{
    AuthMode, Dashboard, DashboardSummary, DashboardViewRequest, Filter, FilterOp, KeyQuery,
    LogLine, LogPage, LogSearch, Panel as Chart, PanelData, PanelRequest, Point, Scope,
    ServicePage, ServiceSort, Services, ServicesRequest, Signal, TailTick, Trace, TraceOrder,
    TracePage, TraceRequest, TraceSearch, TraceSummary, ValueQuery, VolumeBucket, VolumeQuery,
    Where,
};

use super::{after, listen, Host, Panel, PluginPane};
use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_1, RADIUS_2};
use crate::ui::{self, sign_in, Button, IconButton, Segmented, Toggle, Variant};

mod dashboards;
mod filters;
mod format;
mod logs;
mod page;
mod parts;
mod services;
mod sidebar;
mod signing_in;
mod traces;

use self::format::{clock, duration, per_minute, percent, time_label, value_in};
use self::parts::{chip, quiet};

const WINDOWS: [u32; 5] = [5, 15, 60, 360, 1440];
const SEVERITIES: [&str; 5] = ["FATAL", "ERROR", "WARN", "INFO", "DEBUG"];
const LOG_PAGE: u32 = 200;
/// How many lines a live feed keeps, dropping the oldest past it.
const KEPT_LINES: usize = 3_000;
const TRACE_PAGE: u32 = 100;
const VOLUME_BUCKETS: u32 = 90;
const OPERATIONS_SHOWN: usize = 12;
/// Below this, a pinned service's dot stays green: a few failures an hour are ordinary.
const FAILING_RATE: f64 = 0.01;
/// How long typing in the log search pauses before the logs are read again.
const TYPE_PAUSE: Duration = Duration::from_millis(350);
/// SigNoz lays dashboards out on twelve columns; a row unit is about this tall.
const DASHBOARD_ROW: f32 = 44.;
const TRACE_LOG_LOOKBACK: u32 = 24 * 60;
const MAX_INDENT: usize = 16;

/// Which SigNoz the pane is signed in to, as the plugin's `status` says.
type Status = SignInStatus<Where>;

#[derive(Clone, Copy, PartialEq, Default)]
enum Section {
    #[default]
    Services,
    Logs,
    Traces,
    Dashboards,
}

const SECTIONS: [(Section, &str, Icon); 4] = [
    (Section::Services, "Services", Icon::Server),
    (Section::Logs, "Logs", Icon::LogLines),
    (Section::Traces, "Traces", Icon::Waterfall),
    (Section::Dashboards, "Dashboards", Icon::Dashboard),
];

#[derive(Clone, Copy, PartialEq, Default)]
enum Tab {
    #[default]
    Overview,
    Logs,
    Traces,
}

const OPS: [(FilterOp, &str); 6] = [
    (FilterOp::Equals, "="),
    (FilterOp::NotEquals, "≠"),
    (FilterOp::Contains, "contains"),
    (FilterOp::NotContains, "not contains"),
    (FilterOp::Exists, "exists"),
    (FilterOp::NotExists, "missing"),
];

fn takes_value(op: FilterOp) -> bool {
    !matches!(op, FilterOp::Exists | FilterOp::NotExists)
}

/// What the pane keeps between sessions, saved with the workspace under the plugin's id.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    minutes: Option<u32>,
    environment: Option<String>,
    service_sort: ServiceSort,
    pins: Vec<String>,
    dashboard_variables: BTreeMap<String, BTreeMap<String, String>>,
}

impl Settings {
    fn minutes(&self) -> u32 {
        self.minutes
            .filter(|minutes| WINDOWS.contains(minutes))
            .unwrap_or(15)
    }
}

/// A filter being written: an attribute, how it compares, and a value, with what SigNoz
/// suggests for each.
struct Draft {
    key: Entity<InputState>,
    op: FilterOp,
    value: Entity<InputState>,
    keys: Vec<String>,
    values: Vec<String>,
}

/// Pages read past the first, kept while the search they continue stays the same.
struct More<T> {
    key: String,
    rows: Vec<T>,
    next: Option<u32>,
    reading: bool,
}

impl<T> Default for More<T> {
    fn default() -> Self {
        Self {
            key: String::new(),
            rows: Vec::new(),
            next: None,
            reading: false,
        }
    }
}

impl<T> More<T> {
    fn of(&mut self, key: &str, first: Option<u32>) -> &mut Self {
        if self.key != key {
            *self = More {
                key: key.to_owned(),
                rows: Vec::new(),
                next: first,
                reading: false,
            };
        }
        self
    }
}

/// Lines followed live, oldest first, the latest few thousand of them.
#[derive(Default)]
struct Feed {
    key: String,
    lines: VecDeque<LogLine>,
    error: Option<SharedString>,
    scroll: ScrollHandle,
    tail: Option<Task<()>>,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Method {
    #[default]
    Password,
    ApiKey,
}

struct SignIn {
    url: Entity<InputState>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    key: Entity<InputState>,
    account: Entity<InputState>,
    method: Method,
    keychain: bool,
    editing_url: bool,
    inspection: Option<Value>,
    org: String,
    busy: bool,
    error: Option<SharedString>,
}

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.signoz",
    surface: "explore",
    pane: super::boxed::<Signoz>,
    top_bar: None,
    palettes: &[
        super::PluginPalette {
            plugin: "sikemux.signoz",
            id: "signoz.services",
            title: "Open a SigNoz service",
            placeholder: "search services…",
            view: "servicesView",
            params: None,
            rows: service_rows,
            typed: Some(trace_row),
        },
        super::PluginPalette {
            plugin: "sikemux.signoz",
            id: "signoz.dashboards",
            title: "Open a SigNoz dashboard",
            placeholder: "search dashboards…",
            view: "dashboardsView",
            params: None,
            rows: dashboard_rows,
            typed: Some(trace_row),
        },
    ],
};

fn service_rows(services: &Value) -> Vec<crate::palette::Row> {
    let services = match serde_json::from_value::<Services>(services.clone()) {
        Ok(services) => services,
        Err(error) => {
            eprintln!("sikemux: SigNoz's services did not read for the palette: {error}");
            return Vec::new();
        }
    };
    services
        .rows
        .into_iter()
        .map(|row| {
            let meta = match row.errors {
                0 => "no errors".to_owned(),
                _ => format!("{} errors", percent(row.error_rate)),
            };
            crate::palette::Row::new(format!("service:{}", row.service), row.service).meta(meta)
        })
        .collect()
}

/// Opens a trace by the id typed, when the words are one: sixteen to thirty-two hex digits.
fn trace_row(typed: &str) -> Option<crate::palette::Row> {
    let id = typed.trim().to_lowercase();
    let hex = (16..=32).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit());
    hex.then(|| {
        crate::palette::Row::new(format!("trace:{id}"), format!("Open trace {id}")).meta("trace")
    })
}

fn dashboard_rows(dashboards: &Value) -> Vec<crate::palette::Row> {
    let dashboards = match serde_json::from_value::<Vec<DashboardSummary>>(dashboards.clone()) {
        Ok(dashboards) => dashboards,
        Err(error) => {
            eprintln!("sikemux: SigNoz's dashboards did not read for the palette: {error}");
            return Vec::new();
        }
    };
    dashboards
        .into_iter()
        .map(|dashboard| {
            crate::palette::Row::new(format!("dashboard:{}", dashboard.id), dashboard.title)
                .meta(dashboard.tags.join(" · "))
        })
        .collect()
}

pub struct Signoz {
    section: Section,
    service: Option<String>,
    tab: Tab,
    filters: Vec<Filter>,
    severities: Vec<String>,
    text: String,
    expression: String,
    showing_expression: bool,
    live: bool,
    fixed_end: Option<u64>,
    range: Option<(u64, u64)>,
    trace: Option<String>,
    dashboard: Option<String>,
    slowest: bool,
    errors_only: bool,
    search: Option<Entity<InputState>>,
    expression_field: Option<Entity<InputState>>,
    service_find: Option<Entity<InputState>>,
    dashboard_find: Option<Entity<InputState>>,
    draft: Option<Draft>,
    expanded: HashSet<String>,
    picked: Vec<String>,
    older: More<LogLine>,
    feed: Feed,
    more_traces: More<TraceSummary>,
    form: Option<SignIn>,
    typing: Vec<Subscription>,
    pause: Option<Task<()>>,
}

impl Default for Signoz {
    fn default() -> Self {
        Self {
            section: Section::Services,
            service: None,
            tab: Tab::Overview,
            filters: Vec::new(),
            severities: vec!["FATAL".into(), "ERROR".into()],
            text: String::new(),
            expression: String::new(),
            showing_expression: false,
            live: true,
            fixed_end: None,
            range: None,
            trace: None,
            dashboard: None,
            slowest: true,
            errors_only: false,
            search: None,
            expression_field: None,
            service_find: None,
            dashboard_find: None,
            draft: None,
            expanded: HashSet::new(),
            picked: Vec::new(),
            older: More::default(),
            feed: Feed::default(),
            more_traces: More::default(),
            form: None,
            typing: Vec::new(),
            pause: None,
        }
    }
}

fn signoz_mark() -> Hsla {
    rgb(0xf25733).into()
}

fn saved(host: &Host, cx: &App) -> Settings {
    host.read_settings(cx)
}

/// What a field does as the person types in it, presses Return or leaves it.
type Typed = fn(&mut Signoz, &mut Host, &InputEvent, &str, &mut Context<PluginPane>);

/// A one-line field, made the first time the pane draws it.
fn field(
    slot: &mut Option<Entity<InputState>>,
    typing: &mut Vec<Subscription>,
    placeholder: &'static str,
    masked: bool,
    typed: Option<Typed>,
    window: &mut Window,
    cx: &mut Context<PluginPane>,
) -> Entity<InputState> {
    if let Some(input) = slot {
        return input.clone();
    }
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(masked)
    });
    typing.push(
        cx.subscribe(&input, move |pane, input, event: &InputEvent, cx| {
            let text = input.read(cx).value().trim().to_owned();
            if let (Some(typed), Some((signoz, host))) = (typed, pane.parts::<Signoz>()) {
                typed(signoz, host, event, &text, cx);
            }
            cx.notify();
        }),
    );
    *slot = Some(input.clone());
    input
}

fn words(input: &Option<Entity<InputState>>, cx: &App) -> String {
    input
        .as_ref()
        .map(|input| input.read(cx).value().trim().to_lowercase())
        .unwrap_or_default()
}

impl Panel for Signoz {
    fn open_at(&mut self, at: &str, _: &mut Host, _: &mut Context<PluginPane>) {
        match at.split_once(':') {
            Some(("service", service)) => self.show_service(service.to_owned(), Tab::Overview),
            Some(("dashboard", id)) => {
                self.show_section(Section::Dashboards);
                self.dashboard = Some(id.to_owned());
            }
            Some(("trace", id)) => {
                self.show_section(Section::Traces);
                self.trace = Some(id.to_owned());
            }
            _ => {}
        }
    }

    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let following = self.live && self.trace.is_none() && self.signal() == Some(Signal::Logs);
        if !following && self.feed.tail.is_some() {
            self.feed = Feed::default();
        }
        let status = match host.status(cx).read::<Status>() {
            Ok(Some(status)) => status,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        if !status.ok {
            self.feed = Feed::default();
            return self.sign_in(host, &status, window, cx);
        }
        self.form = None;
        let settings = saved(host, cx);
        let services = host
            .view("servicesView", self.services_params(&settings), cx)
            .read::<Services>();
        let sidebar = self.sidebar(
            host,
            &status,
            &settings,
            services.as_ref().ok().cloned().flatten(),
            cx,
        );
        let main = self.main(host, &status, &settings, services, window, cx);
        let t = Tokens::get(cx);
        div()
            .size_full()
            .flex()
            .overflow_hidden()
            .child(
                div()
                    .w(px(200.))
                    .flex_none()
                    .h_full()
                    .border_r_1()
                    .border_color(t.border)
                    .child(sidebar),
            )
            .child(div().flex_1().min_w_0().h_full().child(main))
            .into_any_element()
    }
}

impl Signoz {
    fn show_section(&mut self, section: Section) {
        self.section = section;
        self.service = None;
        self.dashboard = None;
        self.trace = None;
    }

    fn show_service(&mut self, service: String, tab: Tab) {
        self.section = Section::Services;
        self.service = Some(service);
        self.tab = tab;
        self.trace = None;
    }

    fn set_live(&mut self, live: bool) {
        self.live = live;
        self.fixed_end = (!live).then(crate::ui::time::now_ms);
        self.range = None;
    }

    /// One filter per attribute and operator: saying it again replaces it rather than
    /// stacking a copy.
    fn add_filter(&mut self, filter: Filter) {
        self.filters
            .retain(|kept| kept.key != filter.key || kept.op != filter.op);
        self.filters.push(filter);
        self.trace = None;
    }

    /// Which signal the logs-or-traces part of the view reads, if it reads one.
    fn signal(&self) -> Option<Signal> {
        match (self.section, &self.service, self.tab) {
            (Section::Logs, _, _) => Some(Signal::Logs),
            (Section::Traces, _, _) => Some(Signal::Traces),
            (Section::Services, Some(_), Tab::Logs) => Some(Signal::Logs),
            (Section::Services, Some(_), Tab::Traces) => Some(Signal::Traces),
            _ => None,
        }
    }

    fn services_params(&self, settings: &Settings) -> ServicesRequest {
        ServicesRequest {
            scope: Scope {
                minutes: Some(settings.minutes()),
                environment: settings.environment.clone(),
                ..Scope::default()
            },
            sort: settings.service_sort,
        }
    }

    /// The minutes the view reads, the window picked or the range picked out of a chart.
    fn span_minutes(&self, settings: &Settings) -> f64 {
        match self.range {
            Some((start, end)) => (end.saturating_sub(start) as f64 / 60_000.).max(1.),
            None => f64::from(settings.minutes()),
        }
    }

    /// What every query narrows by, in the shape the plugin reads.
    fn scope(&self, settings: &Settings) -> Scope {
        let minutes = settings.minutes();
        let (start, end, minutes) = match (self.range, self.live, self.fixed_end) {
            (Some((start, end)), _, _) => (Some(start), Some(end), None),
            (None, false, Some(end)) => (
                Some(end.saturating_sub(u64::from(minutes) * 60_000)),
                Some(end),
                None,
            ),
            _ => (None, None, Some(minutes)),
        };
        let service = match (self.section, &self.service) {
            (Section::Services, Some(service)) => Some(service.clone()),
            _ => None,
        };
        let expression = self.expression.trim();
        Scope {
            service,
            environment: settings.environment.clone(),
            filters: self.filters.clone(),
            expression: (!expression.is_empty()).then(|| expression.to_owned()),
            start,
            end,
            minutes,
        }
    }

    fn log_search(&self, settings: &Settings) -> LogSearch {
        LogSearch {
            scope: self.scope(settings),
            text: (!self.text.is_empty()).then(|| self.text.clone()),
            severities: self.severities.clone(),
            ..LogSearch::default()
        }
    }

    fn trace_search(&self, settings: &Settings) -> TraceSearch {
        TraceSearch {
            scope: self.scope(settings),
            order: match self.slowest {
                true => TraceOrder::Slowest,
                false => TraceOrder::Recent,
            },
            errors_only: self.errors_only,
            ..TraceSearch::default()
        }
    }
}

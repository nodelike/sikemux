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
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_wire::signoz::{
    Dashboard, DashboardSummary, LogLine, LogPage, Panel as Chart, PanelData, Point, ServicePage,
    ServiceSort, Services, TailTick, Trace, TracePage, TraceSummary, VolumeBucket,
};

use super::{after, listen, Host, Panel, PluginPane};
use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_1, RADIUS_2};
use crate::ui::{self, Button, IconButton, Segmented, Toggle, Variant};
use crate::workbench::Workbench;

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

#[derive(Clone, Copy, PartialEq)]
enum Signal {
    Logs,
    Traces,
}

impl Signal {
    fn word(self) -> &'static str {
        match self {
            Signal::Logs => "logs",
            Signal::Traces => "traces",
        }
    }
}

const OPS: [(&str, &str); 6] = [
    ("equals", "="),
    ("not-equals", "≠"),
    ("contains", "contains"),
    ("not-contains", "not contains"),
    ("exists", "exists"),
    ("not-exists", "missing"),
];

fn takes_value(op: &str) -> bool {
    op != "exists" && op != "not-exists"
}

#[derive(Clone, PartialEq)]
struct Filter {
    key: String,
    op: &'static str,
    value: String,
}

/// What the pane keeps between sessions, saved with the workspace under the plugin's id.
#[derive(Deserialize, Default)]
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
    op: &'static str,
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
    let Ok(services) = serde_json::from_value::<Services>(services.clone()) else {
        return Vec::new();
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
    hex.then(|| crate::palette::Row::new(format!("trace:{id}"), format!("Open trace {id}")).meta("trace"))
}

fn dashboard_rows(dashboards: &Value) -> Vec<crate::palette::Row> {
    let Ok(dashboards) = serde_json::from_value::<Vec<DashboardSummary>>(dashboards.clone()) else {
        return Vec::new();
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

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn saved(host: &Host, cx: &App) -> Settings {
    serde_json::from_value(host.settings(cx)).unwrap_or_default()
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
        let status = match host.status(cx).read::<Value>() {
            Ok(Some(status)) => status,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        if status["ok"] != Value::Bool(true) {
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
        self.fixed_end = (!live).then(now_ms);
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

    fn services_params(&self, settings: &Settings) -> Value {
        json!({
            "minutes": settings.minutes(),
            "environment": settings.environment,
            "sort": settings.service_sort,
        })
    }

    /// The minutes the view reads, the window picked or the range picked out of a chart.
    fn span_minutes(&self, settings: &Settings) -> f64 {
        match self.range {
            Some((start, end)) => (end.saturating_sub(start) as f64 / 60_000.).max(1.),
            None => f64::from(settings.minutes()),
        }
    }

    /// What every query narrows by, in the shape the plugin reads.
    fn scope(&self, settings: &Settings) -> Map<String, Value> {
        let minutes = settings.minutes();
        let mut scope = Map::new();
        match (self.range, self.live, self.fixed_end) {
            (Some((start, end)), _, _) => {
                scope.insert("start".into(), json!(start));
                scope.insert("end".into(), json!(end));
            }
            (None, false, Some(end)) => {
                scope.insert(
                    "start".into(),
                    json!(end.saturating_sub(u64::from(minutes) * 60_000)),
                );
                scope.insert("end".into(), json!(end));
            }
            _ => {
                scope.insert("minutes".into(), json!(minutes));
            }
        }
        if let (Section::Services, Some(service)) = (self.section, &self.service) {
            scope.insert("service".into(), json!(service));
        }
        if let Some(environment) = &settings.environment {
            scope.insert("environment".into(), json!(environment));
        }
        let filters: Vec<Value> = self
            .filters
            .iter()
            .map(|filter| json!({ "key": filter.key, "op": filter.op, "value": filter.value }))
            .collect();
        scope.insert("filters".into(), json!(filters));
        if !self.expression.trim().is_empty() {
            scope.insert("expression".into(), json!(self.expression.trim()));
        }
        scope
    }

    fn log_search(&self, settings: &Settings) -> Map<String, Value> {
        let mut search = self.scope(settings);
        if !self.text.is_empty() {
            search.insert("text".into(), json!(self.text));
        }
        search.insert("severities".into(), json!(self.severities));
        search
    }

    fn trace_search(&self, settings: &Settings) -> Map<String, Value> {
        let mut search = self.scope(settings);
        search.insert(
            "order".into(),
            json!(if self.slowest { "slowest" } else { "recent" }),
        );
        search.insert("errorsOnly".into(), json!(self.errors_only));
        search
    }

    fn sidebar(
        &mut self,
        host: &mut Host,
        status: &Value,
        settings: &Settings,
        services: Option<Rc<Services>>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let environments = services
            .as_ref()
            .map(|services| services.environments.clone())
            .unwrap_or_default();
        let mut picks = vec![ui::Pick::new("", "All environments")];
        picks.extend(
            environments
                .iter()
                .map(|name| ui::Pick::new(name.clone(), name.clone())),
        );
        let shown = settings
            .environment
            .clone()
            .unwrap_or_else(|| "All environments".into());
        let environment = ui::select(
            "signoz-environment",
            shown,
            picks,
            Some(settings.environment.clone().unwrap_or_default().into()),
            false,
            listen::<Signoz, SharedString>(cx, |_, host, picked, _, cx| {
                let picked = picked.to_string();
                host.save_settings(
                    |kept| {
                        kept.insert(
                            "environment".into(),
                            if picked.is_empty() {
                                Value::Null
                            } else {
                                json!(picked)
                            },
                        );
                    },
                    cx,
                );
            }),
            cx,
        );
        let mut sections = div().flex().flex_col().gap(px(1.));
        for (section, label, icon) in SECTIONS {
            sections = sections.child(
                ui::Row::new(SharedString::from(format!("signoz-section-{label}")), label)
                    .chosen(
                        self.section == section
                            && self.service.is_none()
                            && self.dashboard.is_none(),
                    )
                    .mark(22., ui::icon(icon, 14.).text_color(t.text_tertiary))
                    .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                        signoz.show_section(section);
                        cx.notify();
                    })),
            );
        }
        let failing: HashSet<String> = services
            .as_ref()
            .map(|services| {
                services
                    .rows
                    .iter()
                    .filter(|row| row.error_rate >= FAILING_RATE)
                    .map(|row| row.service.clone())
                    .collect()
            })
            .unwrap_or_default();
        let pins = (!settings.pins.is_empty()).then(|| {
            let mut pinned = div().flex().flex_col().gap(px(1.)).child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(2.))
                    .child(ui::caps("Pinned", cx)),
            );
            for pin in &settings.pins {
                let Some((kind, id)) = pin.split_once(':') else {
                    continue;
                };
                let id = id.to_owned();
                let row = match kind {
                    "service" => {
                        let ink = if failing.contains(&id) {
                            t.danger
                        } else {
                            t.live
                        };
                        let chosen = self.section == Section::Services
                            && self.service.as_deref() == Some(id.as_str());
                        let service = id.clone();
                        ui::Row::new(SharedString::from(format!("signoz-pin-{pin}")), id.clone())
                            .chosen(chosen)
                            .mark(22., div().size(px(6.)).rounded_full().bg(ink))
                            .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                                signoz.show_service(service.clone(), Tab::Overview);
                                cx.notify();
                            }))
                    }
                    _ => {
                        let chosen = self.section == Section::Dashboards
                            && self.dashboard.as_deref() == Some(id.as_str());
                        let title = self.dashboard_title(host, &id, cx);
                        ui::Row::new(SharedString::from(format!("signoz-pin-{pin}")), title)
                            .chosen(chosen)
                            .mark(
                                22.,
                                ui::icon(Icon::Dashboard, 14.).text_color(t.text_tertiary),
                            )
                            .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                                signoz.section = Section::Dashboards;
                                signoz.dashboard = Some(id.clone());
                                signoz.trace = None;
                                cx.notify();
                            }))
                    }
                };
                pinned = pinned.child(row);
            }
            pinned
        });
        let url = status["url"].as_str().unwrap_or_default().to_owned();
        let host_name = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_owned();
        let account = match status["email"].as_str() {
            Some(email) if !email.is_empty() => email.to_owned(),
            _ => "API key".into(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(ui::picture(Icon::Signoz, 16., signoz_mark()))
                            .child(
                                ui::text("SigNoz", 13., FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary),
                            ),
                    )
                    .child(environment),
            )
            .child(
                div()
                    .id("signoz-side")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(sections)
                    .children(pins),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        ui::text(host_name, 12., FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .truncate(),
                    )
                    .child(ui::meta(account, cx).truncate())
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(
                                Button::new("signoz-open", "Open", Variant::Outline)
                                    .icon(Icon::Globe)
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                            .child(
                                Button::new("signoz-sign-out", "Sign out", Variant::Outline)
                                    .icon(Icon::LogOut)
                                    .on_click(listen::<Signoz, _>(cx, |_, host, _, _, cx| {
                                        let call = host.call("signOut", Value::Null);
                                        after::<Signoz, _>(
                                            call,
                                            |_, host, done, cx| {
                                                if let Err(failure) = done {
                                                    host.said(
                                                        &failure.titled("Sign out of SigNoz"),
                                                        cx,
                                                    );
                                                }
                                            },
                                            cx,
                                        );
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn dashboard_title(&self, host: &mut Host, id: &str, cx: &mut Context<PluginPane>) -> String {
        host.view("dashboardsView", Value::Null, cx)
            .read::<Vec<DashboardSummary>>()
            .ok()
            .flatten()
            .and_then(|all| {
                all.iter()
                    .find(|dashboard| dashboard.id == id)
                    .map(|dashboard| dashboard.title.clone())
            })
            .unwrap_or_else(|| "Dashboard".into())
    }
}

const TABS: [(Tab, &str); 3] = [
    (Tab::Overview, "Overview"),
    (Tab::Logs, "Logs"),
    (Tab::Traces, "Traces"),
];

fn window_label(minutes: u32) -> String {
    match minutes {
        minutes if minutes >= 1440 => format!("{}d", minutes / 1440),
        minutes if minutes >= 60 => format!("{}h", minutes / 60),
        minutes => format!("{minutes}m"),
    }
}

impl Signoz {
    fn main(
        &mut self,
        host: &mut Host,
        status: &Value,
        settings: &Settings,
        services: Result<Option<Rc<Services>>, super::Failure>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let rows = services.as_ref().ok().cloned().flatten();
        let head = div()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .border_b_1()
            .border_color(t.border)
            .child(self.title(host, settings, rows.as_deref(), cx))
            .child(div().flex_1())
            .child(self.time_controls(settings, cx));
        let tabs = (self.section == Section::Services && self.service.is_some()).then(|| {
            let chosen = TABS
                .iter()
                .position(|(tab, _)| *tab == self.tab && self.trace.is_none())
                .unwrap_or(TABS.len());
            div().px(px(16.)).pt(px(8.)).child(
                Segmented::words(
                    "signoz-tabs",
                    TABS.iter()
                        .map(|(_, label)| ((*label).into(), None))
                        .collect(),
                    chosen,
                )
                .on_select(listen::<Signoz, usize>(
                    cx,
                    |signoz, _, index, _, cx| {
                        signoz.tab = TABS[*index].0;
                        signoz.trace = None;
                        cx.notify();
                    },
                )),
            )
        });
        let body = self.body(host, status, settings, services, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .children(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    fn title(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        services: Option<&Services>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let root = SECTIONS
            .iter()
            .find(|(section, _, _)| *section == self.section)
            .map_or("Services", |(_, label, _)| *label);
        let entity = match (self.section, &self.service, &self.dashboard) {
            (Section::Services, Some(service), _) => {
                Some((service.clone(), true, format!("service:{service}")))
            }
            (Section::Dashboards, _, Some(id)) => Some((
                self.dashboard_title(host, id, cx),
                false,
                format!("dashboard:{id}"),
            )),
            _ => None,
        };
        let Some((name, mono, pin)) = entity else {
            return ui::text(root, 15., FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .into_any_element();
        };
        let section = self.section;
        let badge = match (&self.service, services) {
            (Some(service), Some(services)) => services
                .rows
                .iter()
                .find(|row| &row.service == service && row.errors > 0)
                .map(|row| chip(format!("{} errors", percent(row.error_rate)), t.danger, cx)),
            _ => None,
        };
        let pinned = settings.pins.contains(&pin);
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(
                div()
                    .id("signoz-crumb")
                    .cursor_pointer()
                    .child(ui::text(root, 13., FontWeight::MEDIUM).text_color(t.text_secondary))
                    .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                        signoz.show_section(section);
                        cx.notify();
                    })),
            )
            .child(ui::meta("/", cx))
            .child(if mono {
                ui::mono(name, 14., cx)
                    .text_color(t.text_primary)
                    .truncate()
            } else {
                ui::text(name, 15., FontWeight::SEMIBOLD)
                    .text_color(t.text_primary)
                    .truncate()
            })
            .children(badge)
            .child(
                Toggle::new("signoz-pin", if pinned { "Pinned" } else { "Pin" }, pinned).on_click(
                    listen::<Signoz, _>(cx, move |_, host, _, _, cx| {
                        let pin = pin.clone();
                        host.save_settings(
                            |kept| {
                                let mut pins: Vec<Value> = kept
                                    .get("pins")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                match pins.iter().position(|kept| kept == &json!(pin)) {
                                    Some(at) => {
                                        pins.remove(at);
                                    }
                                    None => pins.push(json!(pin)),
                                }
                                kept.insert("pins".into(), json!(pins));
                            },
                            cx,
                        );
                    }),
                ),
            )
            .into_any_element()
    }

    fn time_controls(&self, settings: &Settings, cx: &mut Context<PluginPane>) -> AnyElement {
        let minutes = settings.minutes();
        let chosen = match self.range {
            Some(_) => WINDOWS.len(),
            None => WINDOWS
                .iter()
                .position(|each| *each == minutes)
                .unwrap_or(1),
        };
        let held = self.range.or_else(|| {
            self.fixed_end
                .map(|end| (end.saturating_sub(u64::from(minutes) * 60_000), end))
        });
        let live_label = match (self.live, held) {
            (true, _) => "Live".to_owned(),
            (false, Some((start, end))) => {
                let span = end.saturating_sub(start);
                format!("{} – {}", time_label(start, span), time_label(end, span))
            }
            (false, None) => "Paused".into(),
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Segmented::words(
                    "signoz-window",
                    WINDOWS
                        .iter()
                        .map(|each| (window_label(*each).into(), None))
                        .collect(),
                    chosen,
                )
                .on_select(listen::<Signoz, usize>(
                    cx,
                    |signoz, host, index, _, cx| {
                        let minutes = WINDOWS[*index];
                        host.save_settings(
                            |kept| {
                                kept.insert("minutes".into(), json!(minutes));
                            },
                            cx,
                        );
                        if signoz.range.is_some() {
                            signoz.set_live(signoz.live);
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                Button::new("signoz-live", live_label, Variant::Outline)
                    .pressed(self.live)
                    .lead(div().size(px(6.)).rounded_full().bg(if self.live {
                        Tokens::get(cx).live
                    } else {
                        Tokens::get(cx).text_tertiary
                    }))
                    .tooltip(if self.live {
                        "Following new data. Click to hold still."
                    } else {
                        "Held still. Click to follow new data."
                    })
                    .on_click(listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                        signoz.set_live(!signoz.live);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn body(
        &mut self,
        host: &mut Host,
        status: &Value,
        settings: &Settings,
        services: Result<Option<Rc<Services>>, super::Failure>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        if let Some(trace) = self.trace.clone() {
            return self.trace_view(host, &trace, window, cx);
        }
        if let Some(signal) = self.signal() {
            let bar = self.filter_bar(host, signal, window, cx);
            let feed = match signal {
                Signal::Logs => self.logs(host, settings, cx),
                Signal::Traces => self.traces(host, settings, cx),
            };
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(bar)
                .child(div().flex_1().min_h_0().child(feed))
                .into_any_element();
        }
        let url = status["url"].as_str().unwrap_or_default().to_owned();
        match (self.section, self.service.clone(), self.dashboard.clone()) {
            (Section::Dashboards, _, Some(id)) => {
                self.dashboard_view(host, settings, &id, &url, window, cx)
            }
            (Section::Dashboards, _, None) => self.dashboards(host, window, cx),
            (_, Some(service), _) => self.overview(host, settings, &service, cx),
            _ => self.services(settings, services, window, cx),
        }
    }
}

/// A clock time, with the day when the window spans more than one.
fn time_label(ms: u64, span: u64) -> String {
    let Some(at) = chrono::DateTime::from_timestamp_millis(ms as i64) else {
        return String::new();
    };
    let at = at.with_timezone(&chrono::Local);
    if span > 24 * 3_600_000 {
        at.format("%b %-d %H:%M").to_string()
    } else {
        at.format("%H:%M").to_string()
    }
}

fn percent(rate: f64) -> String {
    match rate {
        0. => "0%".into(),
        rate if rate < 0.001 => "<0.1%".into(),
        rate if rate >= 0.1 => format!("{:.0}%", rate * 100.),
        rate => format!("{:.1}%", rate * 100.),
    }
}

fn per_minute(count: u64, minutes: f64) -> String {
    let rate = count as f64 / minutes.max(1.);
    match rate {
        rate if rate >= 10_000. => format!("{:.0}k/min", rate / 1_000.),
        rate if rate >= 1_000. => format!("{:.1}k/min", rate / 1_000.),
        rate if rate >= 10. => format!("{}/min", rate.round()),
        rate => format!("{}/min", significant(rate, 2)),
    }
}

/// `value` to `digits` significant figures, without trailing zeros.
fn significant(value: f64, digits: i32) -> String {
    if value == 0. {
        return "0".into();
    }
    let places = (digits - 1 - value.abs().log10().floor() as i32).max(0) as usize;
    let text = format!("{value:.places$}");
    match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_owned(),
        false => text,
    }
}

fn compact(value: f64) -> String {
    let magnitude = value.abs();
    match magnitude {
        magnitude if magnitude >= 1e9 => format!("{:.1}B", value / 1e9),
        magnitude if magnitude >= 1e6 => format!("{:.1}M", value / 1e6),
        magnitude if magnitude >= 1e4 => format!("{:.1}k", value / 1e3),
        magnitude if magnitude >= 100. || value.fract() == 0. => format!("{}", value.round()),
        magnitude => significant(value, if magnitude >= 1. { 3 } else { 2 }),
    }
}

fn duration(ms: f64) -> String {
    match ms {
        ms if ms >= 60_000. => format!("{:.1}m", ms / 60_000.),
        ms if ms >= 1_000. => format!("{}s", significant(ms / 1_000., 3)),
        ms if ms >= 1. => format!("{}ms", significant(ms, 3)),
        ms => format!("{}µs", significant(ms * 1_000., 3)),
    }
}

fn bytes(value: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = value;
    let mut unit = 0;
    while value.abs() >= 1024. && unit + 1 < units.len() {
        value /= 1024.;
        unit += 1;
    }
    format!("{} {}", compact(value), units[unit])
}

/// Values in the unit a SigNoz panel declares; anything unknown is a plain number.
fn value_in(value: Option<f64>, unit: &str) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return "–".into();
    };
    match unit {
        "ns" => duration(value * 1e-6),
        "µs" | "us" => duration(value * 1e-3),
        "ms" => duration(value),
        "s" => duration(value * 1_000.),
        "bytes" | "decbytes" => bytes(value),
        "percent" => format!("{}%", compact(value)),
        "percentunit" => format!("{}%", compact(value * 100.)),
        "reqps" | "rps" => format!("{}/s", compact(value)),
        _ => compact(value),
    }
}

fn chip(text: impl Into<SharedString>, ink: Hsla, cx: &App) -> AnyElement {
    let _ = cx;
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(RADIUS_1)
        .bg(ink.opacity(0.16))
        .text_size(crate::theme::ui_text(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .whitespace_nowrap()
        .child(text.into())
        .into_any_element()
}

fn severity_ink(severity: Option<&str>, cx: &App) -> Hsla {
    let t = Tokens::get(cx);
    let level = severity.unwrap_or_default().to_uppercase();
    if level.starts_with("ERR") || level.starts_with("FATAL") || level.starts_with("CRIT") {
        t.danger
    } else if level.starts_with("WARN") {
        t.warn
    } else {
        t.text_tertiary
    }
}

/// A log line's or trace's time to the millisecond.
fn clock(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp).map_or_else(
        |_| timestamp.to_owned(),
        |at| {
            at.with_timezone(&chrono::Local)
                .format("%H:%M:%S%.3f")
                .to_string()
        },
    )
}

/// A box with a small heading over what it holds.
fn card(title: &str, body: impl IntoElement, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .child(ui::caps(title, cx))
        .child(body)
}

fn quiet(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    ui::meta(text, cx).py(px(8.)).into_any_element()
}

const SORTS: [(ServiceSort, &str, bool); 4] = [
    (ServiceSort::Name, "Service", false),
    (ServiceSort::Calls, "Requests", true),
    (ServiceSort::Errors, "Error rate", true),
    (ServiceSort::P99, "p99", true),
];

/// A row of a table: its first cell takes what room is left, the rest sit at fixed widths.
fn table_row(cells: Vec<AnyElement>, widths: &[f32]) -> Div {
    let mut row = div().flex().items_center().gap(px(12.)).min_w_0();
    for (index, cell) in cells.into_iter().enumerate() {
        row = row.child(match widths.get(index) {
            Some(width) if *width > 0. => div()
                .w(px(*width))
                .flex_none()
                .flex()
                .justify_end()
                .child(cell),
            _ => div().flex_1().min_w_0().child(cell),
        });
    }
    row
}

impl Signoz {
    fn services(
        &mut self,
        settings: &Settings,
        services: Result<Option<Rc<Services>>, super::Failure>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let find = field(
            &mut self.service_find,
            &mut self.typing,
            "Find a service",
            false,
            Some(|signoz, host, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let settings = saved(host, cx);
                    let first = host
                        .view("servicesView", signoz.services_params(&settings), cx)
                        .read::<Services>()
                        .ok()
                        .flatten()
                        .and_then(|services| {
                            let wanted = words(&signoz.service_find, cx);
                            services
                                .rows
                                .iter()
                                .find(|row| row.service.to_lowercase().contains(&wanted))
                                .map(|row| row.service.clone())
                        });
                    if let Some(service) = first {
                        signoz.show_service(service, Tab::Overview);
                    }
                }
            }),
            window,
            cx,
        );
        let wanted = words(&self.service_find, cx);
        let bar = div()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(240.))
                    .child(ui::Field::new(&find).icon(Icon::Search)),
            );
        let services = match services {
            Ok(Some(services)) => services,
            Ok(None) => {
                return div()
                    .size_full()
                    .child(bar)
                    .child(super::loading(cx))
                    .into_any_element()
            }
            Err(failure) => {
                return div()
                    .size_full()
                    .child(bar)
                    .child(super::failure(&failure, cx))
                    .into_any_element()
            }
        };
        let rows: Vec<_> = services
            .rows
            .iter()
            .filter(|row| wanted.is_empty() || row.service.to_lowercase().contains(&wanted))
            .collect();
        let count = match rows.len() {
            1 => "1 service".to_owned(),
            count => format!("{count} services"),
        };
        let bar = bar.child(ui::meta(count, cx));
        if rows.is_empty() {
            let said = if wanted.is_empty() {
                "No traced services in this window."
            } else {
                "No service matches."
            };
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(bar)
                .child(super::empty(said, cx))
                .into_any_element();
        }
        let widths = [0., 110., 90., 90.];
        let sort = settings.service_sort;
        let head = table_row(
            SORTS
                .iter()
                .map(|(by, label, _)| {
                    let by = *by;
                    div()
                        .id(SharedString::from(format!("signoz-sort-{label}")))
                        .cursor_pointer()
                        .child(
                            ui::caps(label, cx)
                                .when(by == sort, |head| head.text_color(t.text_primary)),
                        )
                        .on_click(listen::<Signoz, _>(cx, move |_, host, _, _, cx| {
                            host.save_settings(
                                |kept| {
                                    kept.insert("serviceSort".into(), json!(by));
                                },
                                cx,
                            );
                        }))
                        .into_any_element()
                })
                .collect(),
            &widths,
        )
        .px(px(16.))
        .py(px(6.))
        .border_b_1()
        .border_color(t.border);
        let minutes = f64::from(settings.minutes());
        let body = rows.into_iter().enumerate().map(|(index, row)| {
            let service = row.service.clone();
            table_row(
                vec![
                    ui::mono(row.service.clone(), 12., cx)
                        .text_color(t.text_primary)
                        .truncate()
                        .into_any_element(),
                    ui::text(per_minute(row.calls, minutes), 12., FontWeight::NORMAL)
                        .text_color(t.text_secondary)
                        .into_any_element(),
                    ui::text(percent(row.error_rate), 12., FontWeight::NORMAL)
                        .text_color(if row.errors > 0 {
                            t.danger
                        } else {
                            t.text_tertiary
                        })
                        .into_any_element(),
                    ui::text(value_in(Some(row.p99_ms), "ms"), 12., FontWeight::NORMAL)
                        .text_color(t.text_secondary)
                        .into_any_element(),
                ],
                &widths,
            )
            .id(("signoz-service", index))
            .px(px(16.))
            .py(px(7.))
            .cursor_pointer()
            .hover(move |row| row.bg(t.surface_hover))
            .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                signoz.show_service(service.clone(), Tab::Overview);
                cx.notify();
            }))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(head)
            .child(
                div()
                    .id("signoz-services")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(body),
            )
            .into_any_element()
    }

    fn overview(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        service: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let read = host
            .view("serviceView", Value::Object(self.scope(settings)), cx)
            .read::<ServicePage>();
        let page = match read {
            Ok(Some(page)) => page,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let data = &page.overview;
        if data.calls == 0 {
            return super::empty(
                format!("{service} sent no traces in this window. Its logs may still be there."),
                cx,
            );
        }
        let minutes = self.span_minutes(settings);
        let tile = |label: &'static str, value: String, bad: bool| {
            div()
                .flex_1()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .child(ui::caps(label, cx))
                .child(
                    ui::text(value, 18., FontWeight::SEMIBOLD).text_color(if bad {
                        t.danger
                    } else {
                        t.text_primary
                    }),
                )
        };
        let tiles = div()
            .flex()
            .gap(px(10.))
            .child(tile("Requests", per_minute(data.calls, minutes), false))
            .child(tile(
                "Error rate",
                percent(data.error_rate),
                data.errors > 0,
            ))
            .child(tile(
                "p99 latency",
                value_in(Some(data.p99_ms), "ms"),
                false,
            ))
            .child(tile(
                "p50 latency",
                value_in(Some(data.p50_ms), "ms"),
                false,
            ));
        let failures = if data.errors == 0 {
            quiet("No errors in this window.", cx)
        } else {
            area("signoz-failures", &data.failures, t.danger, "", cx)
        };
        let charts = div()
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .child(
                card(
                    "Requests / min",
                    area("signoz-requests", &data.requests, t.accent, "", cx),
                    cx,
                )
                .flex_1()
                .min_w_0(),
            )
            .child(card("Errors / min", failures, cx).flex_1().min_w(px(220.)))
            .child(
                card(
                    "p99 latency",
                    area("signoz-p99", &data.p99, t.warn, "ms", cx),
                    cx,
                )
                .flex_1()
                .min_w_0(),
            );
        let operations = if page.operations.is_empty() {
            quiet("No entry spans in this window.", cx)
        } else {
            let widths = [0., 90., 70., 70.];
            let mut list = div().flex().flex_col().child(
                table_row(
                    ["Endpoint", "Requests", "Errors", "p99"]
                        .iter()
                        .map(|label| ui::caps(label, cx).into_any_element())
                        .collect(),
                    &widths,
                )
                .pb(px(4.)),
            );
            for (index, operation) in page.operations.iter().take(OPERATIONS_SHOWN).enumerate() {
                let name = operation.name.clone();
                list = list.child(
                    table_row(
                        vec![
                            ui::mono(operation.name.clone(), 11.5, cx)
                                .text_color(t.text_primary)
                                .truncate()
                                .into_any_element(),
                            ui::meta(per_minute(operation.calls, minutes), cx).into_any_element(),
                            ui::text(percent(operation.error_rate), 12., FontWeight::NORMAL)
                                .text_color(if operation.errors > 0 {
                                    t.danger
                                } else {
                                    t.text_tertiary
                                })
                                .into_any_element(),
                            ui::meta(value_in(Some(operation.p99_ms), "ms"), cx).into_any_element(),
                        ],
                        &widths,
                    )
                    .id(("signoz-operation", index))
                    .py(px(5.))
                    .cursor_pointer()
                    .rounded(RADIUS_1)
                    .hover(move |row| row.bg(t.surface_hover))
                    .tooltip(ui::tooltip("Show its traces"))
                    .on_click(listen::<Signoz, _>(
                        cx,
                        move |signoz, _, _, _, cx| {
                            signoz.add_filter(Filter {
                                key: "name".into(),
                                op: "equals",
                                value: name.clone(),
                            });
                            signoz.tab = Tab::Traces;
                            cx.notify();
                        },
                    )),
                );
            }
            list.into_any_element()
        };
        let errors = if page.errors.is_empty() {
            quiet("No error logs in this window.", cx)
        } else {
            let first = page
                .errors
                .iter()
                .map(|group| group.first_seen)
                .min()
                .unwrap_or_default();
            let last = page
                .errors
                .iter()
                .map(|group| group.last_seen)
                .max()
                .unwrap_or_default();
            let span = last.saturating_sub(first);
            let mut list = div().flex().flex_col();
            for (index, group) in page.errors.iter().enumerate() {
                let (service, text) = (service.to_owned(), searchable(&group.pattern));
                list = list.child(
                    div()
                        .id(("signoz-error", index))
                        .py(px(5.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .rounded(RADIUS_1)
                        .hover(move |row| row.bg(t.surface_hover))
                        .tooltip(ui::tooltip(group.sample.clone()))
                        .child(
                            ui::mono(group.pattern.clone(), 11.5, cx)
                                .text_color(t.text_primary)
                                .flex_1()
                                .min_w_0()
                                .truncate(),
                        )
                        .child(ui::meta(
                            format!("last {}", time_label(group.last_seen, span)),
                            cx,
                        ))
                        .child(
                            ui::text(group.count.to_string(), 12., FontWeight::SEMIBOLD)
                                .text_color(t.danger),
                        )
                        .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, window, cx| {
                            signoz.show_service(service.clone(), Tab::Logs);
                            signoz.severities = vec!["FATAL".into(), "ERROR".into()];
                            signoz.set_text(&text, window, cx);
                            cx.notify();
                        })),
                );
            }
            list.into_any_element()
        };
        div()
            .id("signoz-overview")
            .size_full()
            .overflow_y_scroll()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(tiles)
            .child(charts)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(card("Endpoints", operations, cx).flex_1().min_w(px(340.)))
                    .child(card("Errors", errors, cx).flex_1().min_w(px(340.))),
            )
            .into_any_element()
    }

    /// Searches the logs for `text`, in the field too.
    fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<PluginPane>) {
        self.text = text.to_owned();
        if let Some(search) = &self.search {
            search.update(cx, |search, cx| {
                search.set_value(text.to_owned(), window, cx)
            });
        }
    }
}

/// The fixed words of an error pattern, which a text search can find again.
fn searchable(pattern: &str) -> String {
    pattern
        .split("<n>")
        .flat_map(|part| part.split("<id>"))
        .map(str::trim)
        .max_by_key(|part| part.len())
        .unwrap_or_default()
        .to_owned()
}

/// A line over time with the ground under it filled.
fn area(id: &'static str, points: &[Point], ink: Hsla, unit: &'static str, cx: &App) -> AnyElement {
    let _ = cx;
    if points.is_empty() {
        return quiet("No data in this window.", cx);
    }
    let span = points.last().map(|(at, _)| *at).unwrap_or_default()
        - points.first().map(|(at, _)| *at).unwrap_or_default();
    div()
        .h(px(140.))
        .child(
            AreaChart::new(points.to_vec())
                .id(id)
                .x(move |(at, _): &Point| SharedString::from(time_label(*at, span)))
                .y(|(_, value): &Point| *value)
                .stroke(ink)
                .fill(ink.opacity(0.18))
                .tick_margin((points.len() / 4).max(1))
                .tooltip_value(move |_, _, value| value_in(Some(value), unit).into()),
        )
        .into_any_element()
}

impl Signoz {
    fn filter_bar(
        &mut self,
        host: &mut Host,
        signal: Signal,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let mut row = div().flex().flex_wrap().items_center().gap(px(6.));
        match signal {
            Signal::Logs => {
                let search = field(
                    &mut self.search,
                    &mut self.typing,
                    "Search log messages",
                    false,
                    Some(|signoz, _, event, text, cx| {
                        if !matches!(event, InputEvent::Change) {
                            return;
                        }
                        let text = text.to_owned();
                        signoz.pause = Some(cx.spawn(async move |pane, cx| {
                            cx.background_executor().timer(TYPE_PAUSE).await;
                            let _ = pane.update(cx, |pane, cx| {
                                if let Some((signoz, _)) = pane.parts::<Signoz>() {
                                    signoz.text = text;
                                    cx.notify();
                                }
                            });
                        }));
                    }),
                    window,
                    cx,
                );
                row = row.child(
                    div()
                        .w(px(240.))
                        .child(ui::Field::new(&search).icon(Icon::Search)),
                );
                for severity in SEVERITIES {
                    let on = self.severities.iter().any(|kept| kept == severity);
                    row = row.child(
                        Toggle::new(
                            SharedString::from(format!("signoz-sev-{severity}")),
                            severity,
                            on,
                        )
                        .on_click(listen::<Signoz, _>(
                            cx,
                            move |signoz, _, _, _, cx| {
                                match signoz.severities.iter().position(|kept| kept == severity) {
                                    Some(at) => {
                                        signoz.severities.remove(at);
                                    }
                                    None => signoz.severities.push(severity.to_owned()),
                                }
                                cx.notify();
                            },
                        )),
                    );
                }
            }
            Signal::Traces => {
                row = row
                    .child(
                        Segmented::words(
                            "signoz-order",
                            vec![("Slowest".into(), None), ("Recent".into(), None)],
                            usize::from(!self.slowest),
                        )
                        .on_select(listen::<Signoz, usize>(
                            cx,
                            |signoz, _, index, _, cx| {
                                signoz.slowest = *index == 0;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        Toggle::new("signoz-failed", "Failed only", self.errors_only).on_click(
                            listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                                signoz.errors_only = !signoz.errors_only;
                                cx.notify();
                            }),
                        ),
                    );
            }
        }
        for (index, filter) in self.filters.iter().enumerate() {
            let op = OPS
                .iter()
                .find(|(op, _)| *op == filter.op)
                .map_or(filter.op, |(_, label)| *label);
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(8.))
                    .pr(px(2.))
                    .h(px(22.))
                    .rounded(RADIUS_1)
                    .border_1()
                    .border_color(t.border)
                    .child(ui::mono(filter.key.clone(), 11., cx).text_color(t.text_primary))
                    .child(ui::meta(op, cx))
                    .when(takes_value(filter.op), |chip| {
                        chip.child(ui::mono(filter.value.clone(), 11., cx).text_color(t.accent))
                    })
                    .child(
                        IconButton::new(
                            ("signoz-unfilter", index),
                            Icon::Close,
                            format!("Remove the {} filter", filter.key),
                        )
                        .small()
                        .on_click(listen::<Signoz, _>(
                            cx,
                            move |signoz, _, _, _, cx| {
                                if index < signoz.filters.len() {
                                    signoz.filters.remove(index);
                                }
                                cx.notify();
                            },
                        )),
                    ),
            );
        }
        if self.draft.is_none() {
            row = row.child(
                Button::new("signoz-add-filter", "Filter", Variant::Dashed)
                    .icon(Icon::Plus)
                    .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, window, cx| {
                        signoz.open_draft(signal, window, cx);
                        cx.notify();
                    })),
            );
        }
        row = row.child(
            Toggle::new("signoz-expression-toggle", "{ }", self.showing_expression).on_click(
                listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                    signoz.showing_expression = !signoz.showing_expression;
                    cx.notify();
                }),
            ),
        );
        let draft = self.draft_editor(host, cx);
        let expression = self.showing_expression.then(|| {
            let input = field(
                &mut self.expression_field,
                &mut self.typing,
                "latency_ms > 500 AND NOT (path = '/health')",
                false,
                Some(|signoz, _, event, text, _| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        signoz.expression = text.to_owned();
                    }
                }),
                window,
                cx,
            );
            ui::Field::new(&input).mono()
        });
        div()
            .px(px(16.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .border_b_1()
            .border_color(t.border)
            .child(row)
            .children(draft)
            .children(expression)
            .into_any_element()
    }

    fn open_draft(&mut self, signal: Signal, window: &mut Window, cx: &mut Context<PluginPane>) {
        let mut key = None;
        let key = field(
            &mut key,
            &mut self.typing,
            "attribute",
            false,
            Some(|signoz, host, event, text, cx| match event {
                InputEvent::Change => signoz.suggest_keys(host, text, cx),
                InputEvent::PressEnter { .. } => signoz.add_draft(cx),
                _ => {}
            }),
            window,
            cx,
        );
        let mut value = None;
        let value = field(
            &mut value,
            &mut self.typing,
            "value",
            false,
            Some(|signoz, host, event, text, cx| match event {
                InputEvent::Change => signoz.suggest_values(host, text, cx),
                InputEvent::PressEnter { .. } => signoz.add_draft(cx),
                _ => {}
            }),
            window,
            cx,
        );
        let _ = signal;
        self.draft = Some(Draft {
            key,
            op: "equals",
            value,
            keys: Vec::new(),
            values: Vec::new(),
        });
    }

    fn draft_signal(&self) -> &'static str {
        self.signal().unwrap_or(Signal::Logs).word()
    }

    fn suggest_keys(&mut self, host: &Host, text: &str, cx: &mut Context<PluginPane>) {
        let call = host.call(
            "fieldKeys",
            json!({ "signal": self.draft_signal(), "search": text }),
        );
        after::<Signoz, _>(
            call,
            |signoz, _, found, _| {
                if let (Some(draft), Ok(found)) = (&mut signoz.draft, found) {
                    draft.keys = found
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|key| key["name"].as_str().map(str::to_owned))
                        .take(12)
                        .collect();
                }
            },
            cx,
        );
    }

    fn suggest_values(&mut self, host: &Host, text: &str, cx: &mut Context<PluginPane>) {
        let Some(draft) = &self.draft else {
            return;
        };
        let name = draft.key.read(cx).value().trim().to_owned();
        if name.is_empty() || !takes_value(draft.op) {
            return;
        }
        let params = json!({ "signal": self.draft_signal(), "name": name, "search": text });
        after::<Signoz, _>(
            host.call("fieldValues", params),
            |signoz, _, found, _| {
                if let (Some(draft), Ok(found)) = (&mut signoz.draft, found) {
                    draft.values = found
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .take(12)
                        .collect();
                }
            },
            cx,
        );
    }

    fn add_draft(&mut self, cx: &mut Context<PluginPane>) {
        let Some(draft) = &self.draft else {
            return;
        };
        let key = draft.key.read(cx).value().trim().to_owned();
        let value = draft.value.read(cx).value().trim().to_owned();
        if key.is_empty() || (takes_value(draft.op) && value.is_empty()) {
            return;
        }
        let op = draft.op;
        self.add_filter(Filter {
            key,
            op,
            value: if takes_value(op) {
                value
            } else {
                String::new()
            },
        });
        self.draft = None;
    }

    fn draft_editor(&mut self, host: &Host, cx: &mut Context<PluginPane>) -> Option<AnyElement> {
        let _ = host;
        let draft = self.draft.as_ref()?;
        let t = Tokens::get(cx).clone();
        let suggestions =
            |id: &'static str, items: &[String], key: bool, cx: &mut Context<PluginPane>| {
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(items.iter().enumerate().map(|(index, item)| {
                        let item = item.clone();
                        Button::new((id, index), item.clone(), Variant::Outline).on_click(listen::<
                            Signoz,
                            _,
                        >(
                            cx,
                            move |signoz, _, _, window, cx| {
                                if let Some(draft) = &signoz.draft {
                                    let input = if key {
                                        draft.key.clone()
                                    } else {
                                        draft.value.clone()
                                    };
                                    input.update(cx, |input, cx| {
                                        input.set_value(item.clone(), window, cx)
                                    });
                                }
                                cx.notify();
                            },
                        ))
                    }))
            };
        let ready = !draft.key.read(cx).value().trim().is_empty()
            && (!takes_value(draft.op) || !draft.value.read(cx).value().trim().is_empty());
        let op_label = OPS
            .iter()
            .find(|(op, _)| *op == draft.op)
            .map_or("=", |(_, label)| *label);
        let ops = ui::select(
            "signoz-op",
            op_label,
            OPS.iter()
                .map(|(op, label)| ui::Pick::new(*op, *label))
                .collect(),
            Some(draft.op.into()),
            false,
            listen::<Signoz, SharedString>(cx, |signoz, _, picked, _, cx| {
                if let (Some(draft), Some((op, _))) = (
                    &mut signoz.draft,
                    OPS.iter().find(|(op, _)| *op == picked.as_ref()),
                ) {
                    draft.op = op;
                }
                cx.notify();
            }),
            cx,
        );
        let (key, value, takes) = (
            draft.key.clone(),
            draft.value.clone(),
            takes_value(draft.op),
        );
        let (keys, values) = (draft.keys.clone(), draft.values.clone());
        Some(
            div()
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().w(px(200.)).child(ui::Field::new(&key).mono()))
                        .child(ops)
                        .when(takes, |row| {
                            row.child(div().w(px(200.)).child(ui::Field::new(&value).mono()))
                        })
                        .child(
                            Button::new("signoz-draft-add", "Add", Variant::Primary)
                                .disabled(!ready)
                                .on_click(listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                                    signoz.add_draft(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("signoz-draft-cancel", "Cancel", Variant::Outline)
                                .on_click(listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                                    signoz.draft = None;
                                    cx.notify();
                                })),
                        ),
                )
                .when(!keys.is_empty(), |editor| {
                    editor.child(suggestions("signoz-key-pick", &keys, true, cx))
                })
                .when(takes && !values.is_empty(), |editor| {
                    editor.child(suggestions("signoz-value-pick", &values, false, cx))
                })
                .into_any_element(),
        )
    }

    fn logs(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let search = self.log_search(settings);
        let mut volume_search = search.clone();
        volume_search.insert("buckets".into(), json!(VOLUME_BUCKETS));
        let volume = host
            .view("volumeView", Value::Object(volume_search), cx)
            .read::<Vec<VolumeBucket>>()
            .ok()
            .flatten();
        let strip = self.volume(volume.as_deref().map(Vec::as_slice), cx);
        let show_service = !search.contains_key("service");
        let mut heads = vec![("Time", 96.), ("Level", 52.)];
        if show_service {
            heads.push(("Service", 130.));
        }
        heads.push(("Message", 0.));
        let columns = div()
            .px(px(16.))
            .py(px(4.))
            .flex()
            .gap(px(10.))
            .border_b_1()
            .border_color(t.border)
            .children(heads.into_iter().map(|(label, width)| match width {
                0. => div().flex_1().child(ui::caps(label, cx)),
                width => div().w(px(width)).flex_none().child(ui::caps(label, cx)),
            }));
        let mut asked = search;
        asked.insert("limit".into(), json!(LOG_PAGE));
        let (lines, error, older) = if self.live {
            self.follow(host, asked, cx);
            let error = self.feed.error.clone();
            (
                self.feed.lines.iter().cloned().collect::<Vec<_>>(),
                error,
                None,
            )
        } else {
            self.feed = Feed::default();
            let key = Value::Object(asked.clone()).to_string();
            let page = match host
                .view("logsView", Value::Object(asked), cx)
                .read::<LogPage>()
            {
                Ok(Some(page)) => page,
                Ok(None) => {
                    return div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(strip)
                        .child(columns)
                        .child(super::loading(cx))
                        .into_any_element()
                }
                Err(failure) => {
                    return div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(strip)
                        .child(columns)
                        .child(super::failure(&failure, cx))
                        .into_any_element()
                }
            };
            let older = self.older.of(&key, page.next_offset);
            let lines = page
                .lines
                .iter()
                .chain(older.rows.iter())
                .rev()
                .cloned()
                .collect::<Vec<_>>();
            (lines, None, older.next.map(|_| older.reading))
        };
        let banner = error.map(|error| {
            div()
                .px(px(16.))
                .py(px(6.))
                .border_b_1()
                .border_color(t.border)
                .child(ui::text(error, 12., FontWeight::NORMAL).text_color(t.danger))
        });
        if lines.is_empty() {
            let errors_only = !self.severities.is_empty()
                && self
                    .severities
                    .iter()
                    .all(|severity| severity == "ERROR" || severity == "FATAL");
            let said = match (errors_only, self.live) {
                (true, _) => "No errors in this window.",
                (false, true) => "Waiting for log lines.",
                (false, false) => "No log lines in this window.",
            };
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(strip)
                .child(columns)
                .children(banner)
                .child(super::empty(said, cx))
                .into_any_element();
        }
        let bench = host.bench().clone();
        let rows: Vec<AnyElement> = lines
            .iter()
            .enumerate()
            .map(|(index, line)| self.log_row(line, index, show_service, &lines, Some(&bench), cx))
            .collect();
        let older_button = older.map(|reading| {
            div().p(px(10.)).flex().justify_center().child(
                Button::new(
                    "signoz-older",
                    if reading {
                        "Reading…"
                    } else {
                        "Load older lines"
                    },
                    Variant::Outline,
                )
                .disabled(reading)
                .on_click(listen::<Signoz, _>(cx, move |signoz, host, _, _, cx| {
                    signoz.read_older(host, cx)
                })),
            )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(strip)
            .child(columns)
            .children(banner)
            .child(
                div()
                    .id("signoz-lines")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.feed.scroll)
                    .children(older_button)
                    .children(rows),
            )
            .into_any_element()
    }

    /// Follows new lines as SigNoz takes them in, starting again when the search changes.
    fn follow(&mut self, host: &Host, asked: Map<String, Value>, cx: &mut Context<PluginPane>) {
        let key = Value::Object(asked.clone()).to_string();
        if self.feed.key == key {
            return;
        }
        self.feed = Feed {
            key: key.clone(),
            ..Feed::default()
        };
        self.feed.scroll.scroll_to_bottom();
        let tail = host.stream::<Signoz>(
            "tailLogs",
            Value::Object(asked),
            cx,
            move |signoz, _, streamed, _| {
                let feed = &mut signoz.feed;
                if feed.key != key {
                    return;
                }
                match streamed {
                    super::Streamed::Item(item) => {
                        let Ok(tick) = serde_json::from_value::<TailTick>(item) else {
                            return;
                        };
                        feed.error = tick.error.map(SharedString::from);
                        if tick.lines.is_empty() {
                            return;
                        }
                        let at_bottom =
                            -feed.scroll.offset().y >= feed.scroll.max_offset().y - px(48.);
                        feed.lines.extend(tick.lines);
                        while feed.lines.len() > KEPT_LINES {
                            feed.lines.pop_front();
                        }
                        if at_bottom {
                            feed.scroll.scroll_to_bottom();
                        }
                    }
                    super::Streamed::Ended(failure) => {
                        if let Some(failure) = failure {
                            feed.error = Some(failure.message);
                        }
                    }
                }
            },
        );
        self.feed.tail = Some(tail);
    }

    fn read_older(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(offset) = self.older.next.filter(|_| !self.older.reading) else {
            return;
        };
        let Ok(mut asked) = serde_json::from_str::<Map<String, Value>>(&self.older.key) else {
            return;
        };
        asked.insert("offset".into(), json!(offset));
        self.older.reading = true;
        let key = self.older.key.clone();
        after::<Signoz, _>(
            host.call("logsView", Value::Object(asked)),
            move |signoz, host, done, cx| {
                if signoz.older.key != key {
                    return;
                }
                signoz.older.reading = false;
                match done.and_then(|page| {
                    serde_json::from_value::<LogPage>(page)
                        .map_err(|error| super::Failure::core(error.to_string()))
                }) {
                    Ok(page) => {
                        signoz.older.rows.extend(page.lines);
                        signoz.older.next = page.next_offset;
                    }
                    Err(failure) => host.said(&failure.titled("Read older lines"), cx),
                }
            },
            cx,
        );
        cx.notify();
    }

    /// The lines a menu acts on: those picked when the line is among them, else the line.
    fn chosen_lines(&self, line: &LogLine, lines: &[LogLine]) -> Vec<LogLine> {
        if self.picked.contains(&line.id) {
            lines
                .iter()
                .filter(|each| self.picked.contains(&each.id))
                .cloned()
                .collect()
        } else {
            vec![line.clone()]
        }
    }

    fn log_row(
        &self,
        line: &LogLine,
        index: usize,
        show_service: bool,
        lines: &[LogLine],
        menu: Option<&WeakEntity<Workbench>>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let expanded = self.expanded.contains(&line.id);
        let picked = self.picked.contains(&line.id);
        let id = line.id.clone();
        let order: Vec<String> = lines.iter().map(|line| line.id.clone()).collect();
        let level: String = line
            .severity
            .clone()
            .unwrap_or_default()
            .chars()
            .take(5)
            .collect();
        let head = div()
            .id(("signoz-log", index))
            .px(px(16.))
            .py(px(3.))
            .flex()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .when(picked, |row| row.bg(t.surface_chosen))
            .hover(move |row| row.bg(t.surface_hover))
            .child(
                div()
                    .w(px(96.))
                    .flex_none()
                    .child(ui::mono(clock(&line.timestamp), 11., cx)),
            )
            .child(
                div().w(px(52.)).flex_none().child(
                    ui::mono(if level.is_empty() { "·".into() } else { level }, 10.5, cx)
                        .text_color(severity_ink(line.severity.as_deref(), cx)),
                ),
            )
            .when(show_service, |row| {
                row.child(div().w(px(130.)).flex_none().child(
                    ui::meta(line.service.clone().unwrap_or_else(|| "-".into()), cx).truncate(),
                ))
            })
            .child(
                ui::mono(
                    line.body.lines().next().unwrap_or_default().to_owned(),
                    11.5,
                    cx,
                )
                .text_color(t.text_primary)
                .flex_1()
                .min_w_0()
                .truncate(),
            )
            .on_click(listen::<Signoz, gpui_kit::ClickEvent>(
                cx,
                move |signoz, _, event, _, cx| {
                    let modifiers = event.modifiers();
                    if modifiers.platform || modifiers.control || modifiers.shift {
                        signoz.pick(&id, &order, modifiers.shift);
                    } else if !signoz.expanded.remove(&id) {
                        signoz.expanded.insert(id.clone());
                    }
                    cx.notify();
                },
            ));
        let Some(bench) = menu.cloned() else {
            return self.with_detail(head, line, index, expanded, cx);
        };
        let chosen = self.chosen_lines(line, lines);
        let some_picked = !self.picked.is_empty();
        let pane = cx.weak_entity();
        let head = head.context_menu(move |popup, window, cx| {
            let count = chosen.len();
            let text = chosen
                .iter()
                .map(|line| line.body.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let delivery = delivery(&chosen);
            let pane_copy = pane.clone();
            let mut popup = popup.item(
                PopupMenuItem::new(if count == 1 {
                    "Copy Message".to_owned()
                } else {
                    format!("Copy {count} Messages")
                })
                .icon(Icon::Copy)
                .on_click(move |_, _, cx| {
                    let _ = pane_copy.update(cx, |pane, cx| {
                        pane.host.copy(text.clone(), "the message", cx)
                    });
                }),
            );
            let send = if count == 1 {
                "Send to Agent…".to_owned()
            } else {
                format!("Send {count} Lines to Agent…")
            };
            popup = crate::send_to::submenu_with_text(popup, send, bench.clone(), delivery, window, cx);
            if some_picked {
                let pane = pane.clone();
                popup = popup.separator().item(
                    PopupMenuItem::new("Clear Selection")
                        .icon(Icon::Discard)
                        .on_click(move |_, _, cx| {
                            let _ = pane.update(cx, |pane, cx| {
                                if let Some((signoz, _)) = pane.parts::<Signoz>() {
                                    signoz.picked.clear();
                                    cx.notify();
                                }
                            });
                        }),
                );
            }
            popup
        });
        self.with_detail(head, line, index, expanded, cx)
    }

    fn with_detail(
        &self,
        head: impl IntoElement,
        line: &LogLine,
        index: usize,
        expanded: bool,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        if !expanded {
            return head.into_any_element();
        }
        div()
            .child(head)
            .child(self.log_detail(line, index, cx))
            .into_any_element()
    }

    /// ⌘ or Ctrl adds or drops one line; Shift takes every line from the last one picked.
    fn pick(&mut self, id: &str, order: &[String], range: bool) {
        let anchor = self
            .picked
            .last()
            .and_then(|last| order.iter().position(|each| each == last));
        let to = order.iter().position(|each| each == id);
        match (range, anchor, to) {
            (true, Some(from), Some(to)) => {
                let (start, end) = if from < to { (from, to) } else { (to, from) };
                for each in &order[start..=end] {
                    if !self.picked.contains(each) {
                        self.picked.push(each.clone());
                    }
                }
            }
            _ => match self.picked.iter().position(|each| each == id) {
                Some(at) => {
                    self.picked.remove(at);
                }
                None => self.picked.push(id.to_owned()),
            },
        }
    }

    fn log_detail(&self, line: &LogLine, index: usize, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let trace = line.trace_id.clone().map(|trace| {
            let shown = format!("trace {trace}");
            div()
                .id(("signoz-log-trace", index))
                .cursor_pointer()
                .child(ui::mono(shown, 11., cx).text_color(t.accent))
                .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                    signoz.trace = Some(trace.clone());
                    cx.notify();
                }))
        });
        let full = line.body.contains('\n').then(|| {
            ui::mono(line.body.clone(), 11., cx)
                .text_color(t.text_primary)
                .whitespace_normal()
        });
        let attributes: Vec<(&String, String)> = line
            .attributes
            .iter()
            .filter(|(_, value)| !value.is_null() && value.as_str() != Some(""))
            .map(|(key, value)| {
                let text = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned);
                (key, text)
            })
            .collect();
        let attributes =
            if attributes.is_empty() {
                ui::meta("no attributes", cx).into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(
                        attributes
                            .into_iter()
                            .enumerate()
                            .map(|(at, (key, value))| {
                                let (keep_key, drop_key) = (key.clone(), key.clone());
                                let (keep_value, drop_value) = (value.clone(), value.clone());
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .w(px(180.))
                                            .flex_none()
                                            .child(ui::mono(key.clone(), 11., cx).truncate()),
                                    )
                                    .child(
                                        ui::mono(value, 11., cx)
                                            .text_color(t.text_primary)
                                            .flex_1()
                                            .min_w_0()
                                            .truncate(),
                                    )
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("signoz-keep-{index}-{at}")),
                                            "=",
                                            Variant::Outline,
                                        )
                                        .tooltip(format!("Only lines where {keep_key} is this"))
                                        .on_click(
                                            listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                                                signoz.add_filter(Filter {
                                                    key: keep_key.clone(),
                                                    op: "equals",
                                                    value: keep_value.clone(),
                                                });
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("signoz-drop-{index}-{at}")),
                                            "≠",
                                            Variant::Outline,
                                        )
                                        .tooltip(format!("Hide lines where {drop_key} is this"))
                                        .on_click(
                                            listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                                                signoz.add_filter(Filter {
                                                    key: drop_key.clone(),
                                                    op: "not-equals",
                                                    value: drop_value.clone(),
                                                });
                                                cx.notify();
                                            }),
                                        ),
                                    )
                            }),
                    )
                    .into_any_element()
            };
        div()
            .px(px(16.))
            .pl(px(122.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .bg(t.surface_sunken)
            .children(trace)
            .children(full)
            .child(attributes)
            .into_any_element()
    }

    /// How many lines each moment of the window holds, by severity; a click on one holds
    /// the view still on it.
    fn volume(&self, buckets: Option<&[VolumeBucket]>, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let strip = div()
            .h(px(64.))
            .px(px(16.))
            .py(px(8.))
            .flex()
            .items_end()
            .gap(px(1.))
            .border_b_1()
            .border_color(t.border);
        let Some(buckets) = buckets.filter(|buckets| !buckets.is_empty()) else {
            return strip.into_any_element();
        };
        let most = buckets
            .iter()
            .map(|bucket| bucket.counts.values().sum::<u64>())
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let step = match buckets {
            [first, second, ..] => second.start.saturating_sub(first.start),
            _ => 60_000,
        };
        let order = ["FATAL", "ERROR", "WARN", "INFO", "DEBUG"];
        let bars = buckets.iter().enumerate().map(|(index, bucket)| {
            let total = bucket.counts.values().sum::<u64>() as f32;
            let mut stack = div().flex_1().h_full().flex().flex_col().justify_end();
            let mut levels: Vec<(&String, &u64)> = bucket.counts.iter().collect();
            levels.sort_by_key(|(level, _)| {
                order
                    .iter()
                    .position(|each| each.eq_ignore_ascii_case(level))
                    .unwrap_or(order.len())
            });
            for (level, count) in levels.into_iter().rev() {
                stack = stack.child(
                    div()
                        .w_full()
                        .h(relative(*count as f32 / most))
                        .bg(severity_ink(Some(level), cx).opacity(0.8)),
                );
            }
            let start = bucket.start;
            stack
                .id(("signoz-volume", index))
                .cursor_pointer()
                .tooltip(ui::tooltip(format!(
                    "{} · {} lines",
                    time_label(start, step * 2),
                    total
                )))
                .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                    signoz.live = false;
                    signoz.range = Some((start, start + step));
                    signoz.fixed_end = Some(start + step);
                    cx.notify();
                }))
        });
        let back =
            self.range.map(|_| {
                Button::new("signoz-back-to-live", "Back to live", Variant::Outline).on_click(
                    listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                        signoz.set_live(true);
                        cx.notify();
                    }),
                )
            });
        strip.children(bars).children(back).into_any_element()
    }
}

/// Log lines as an agent is handed them, in a fence.
fn delivery(lines: &[LogLine]) -> String {
    let heading = match lines.len() {
        1 => "A log line from SigNoz:".to_owned(),
        count => format!("{count} log lines from SigNoz:"),
    };
    let described: Vec<String> = lines
        .iter()
        .map(|line| {
            let mut head = vec![
                line.timestamp.clone(),
                line.severity.clone().unwrap_or_else(|| "-".into()),
                line.service.clone().unwrap_or_else(|| "-".into()),
            ];
            if let Some(trace) = &line.trace_id {
                head.push(format!("trace={trace}"));
            }
            format!("{}\n{}", head.join(" "), line.body)
        })
        .collect();
    let fenced = crate::send_to::code_fence(&described.join("\n\n"));
    format!("{heading}\n\n{fenced}\n")
}

impl Signoz {
    fn traces(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let mut asked = self.trace_search(settings);
        let show_service = !asked.contains_key("service");
        asked.insert("limit".into(), json!(TRACE_PAGE));
        let key = Value::Object(asked.clone()).to_string();
        let columns = div()
            .px(px(16.))
            .py(px(4.))
            .flex()
            .gap(px(10.))
            .border_b_1()
            .border_color(t.border)
            .child(div().w(px(96.)).child(ui::caps("Time", cx)))
            .child(div().w(px(52.)).child(ui::caps("Status", cx)))
            .when(show_service, |head| {
                head.child(div().w(px(130.)).child(ui::caps("Service", cx)))
            })
            .child(div().flex_1().child(ui::caps("Operation", cx)))
            .child(div().w(px(180.)).child(ui::caps("Duration", cx)));
        let page = match host
            .view("tracesView", Value::Object(asked), cx)
            .read::<TracePage>()
        {
            Ok(Some(page)) => page,
            Ok(None) => {
                return div()
                    .size_full()
                    .child(columns)
                    .child(super::loading(cx))
                    .into_any_element()
            }
            Err(failure) => {
                return div()
                    .size_full()
                    .child(columns)
                    .child(super::failure(&failure, cx))
                    .into_any_element()
            }
        };
        let more = self.more_traces.of(&key, page.next_offset);
        let traces: Vec<TraceSummary> = page
            .traces
            .iter()
            .chain(more.rows.iter())
            .cloned()
            .collect();
        let (next, reading) = (more.next, more.reading);
        if traces.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(columns)
                .child(super::empty("No traces in this window.", cx))
                .into_any_element();
        }
        let slowest = traces
            .iter()
            .map(|trace| trace.duration_ms)
            .fold(1., f64::max);
        let rows = traces.iter().enumerate().map(|(index, trace)| {
            let id = trace.trace_id.clone();
            let status = if trace.error {
                "ERROR".to_owned()
            } else {
                trace.status_code.clone().unwrap_or_default()
            };
            div()
                .id(("signoz-trace", index))
                .px(px(16.))
                .py(px(4.))
                .flex()
                .items_center()
                .gap(px(10.))
                .cursor_pointer()
                .hover(move |row| row.bg(t.surface_hover))
                .child(div().w(px(96.)).flex_none().child(ui::mono(
                    clock(&trace.timestamp),
                    11.,
                    cx,
                )))
                .child(
                    div()
                        .w(px(52.))
                        .flex_none()
                        .child(ui::mono(status, 10.5, cx).text_color(if trace.error {
                            t.danger
                        } else {
                            t.text_tertiary
                        })),
                )
                .when(show_service, |row| {
                    row.child(
                        div()
                            .w(px(130.))
                            .flex_none()
                            .child(ui::meta(trace.service.clone(), cx).truncate()),
                    )
                })
                .child(
                    ui::mono(trace.name.clone(), 11.5, cx)
                        .text_color(t.text_primary)
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                )
                .child(
                    div()
                        .w(px(180.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div().flex_1().h(px(6.)).child(
                                div()
                                    .h_full()
                                    .w(relative(((trace.duration_ms / slowest) as f32).max(0.02)))
                                    .rounded(RADIUS_1)
                                    .bg(if trace.error { t.danger } else { t.accent }.opacity(0.7)),
                            ),
                        )
                        .child(
                            div()
                                .w(px(56.))
                                .flex()
                                .justify_end()
                                .child(ui::meta(duration(trace.duration_ms), cx)),
                        ),
                )
                .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                    signoz.trace = Some(id.clone());
                    cx.notify();
                }))
        });
        let more_button = next.map(|_| {
            div().p(px(10.)).flex().justify_center().child(
                Button::new(
                    "signoz-more-traces",
                    if reading { "Reading…" } else { "More traces" },
                    Variant::Outline,
                )
                .disabled(reading)
                .on_click(listen::<Signoz, _>(cx, |signoz, host, _, _, cx| {
                    signoz.read_more_traces(host, cx)
                })),
            )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(columns)
            .child(
                div()
                    .id("signoz-traces")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .children(more_button),
            )
            .into_any_element()
    }

    fn read_more_traces(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(offset) = self.more_traces.next.filter(|_| !self.more_traces.reading) else {
            return;
        };
        let Ok(mut asked) = serde_json::from_str::<Map<String, Value>>(&self.more_traces.key)
        else {
            return;
        };
        asked.insert("offset".into(), json!(offset));
        self.more_traces.reading = true;
        let key = self.more_traces.key.clone();
        after::<Signoz, _>(
            host.call("tracesView", Value::Object(asked)),
            move |signoz, host, done, cx| {
                if signoz.more_traces.key != key {
                    return;
                }
                signoz.more_traces.reading = false;
                match done.and_then(|page| {
                    serde_json::from_value::<TracePage>(page)
                        .map_err(|error| super::Failure::core(error.to_string()))
                }) {
                    Ok(page) => {
                        signoz.more_traces.rows.extend(page.traces);
                        signoz.more_traces.next = page.next_offset;
                    }
                    Err(failure) => host.said(&failure.titled("Read more traces"), cx),
                }
            },
            cx,
        );
        cx.notify();
    }

    fn trace_view(
        &mut self,
        host: &mut Host,
        trace_id: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let _ = window;
        let t = Tokens::get(cx).clone();
        let read = host
            .view("traceView", json!({ "traceId": trace_id }), cx)
            .read::<Trace>();
        let logs = host
            .view(
                "logsView",
                json!({ "traceId": trace_id, "minutes": TRACE_LOG_LOOKBACK, "limit": 500 }),
                cx,
            )
            .read::<LogPage>();
        let mut head = div()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .border_b_1()
            .border_color(t.border)
            .child(
                IconButton::new("signoz-trace-back", Icon::Chevron, "Back to the traces")
                    .turned(std::f32::consts::PI)
                    .small()
                    .on_click(listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                        signoz.trace = None;
                        cx.notify();
                    })),
            )
            .child(ui::mono(trace_id.to_owned(), 12., cx).text_color(t.text_primary));
        let trace = match read {
            Ok(Some(trace)) => trace,
            Ok(None) => {
                return div()
                    .size_full()
                    .child(head)
                    .child(super::loading(cx))
                    .into_any_element()
            }
            Err(failure) => {
                return div()
                    .size_full()
                    .child(head)
                    .child(super::failure(&failure, cx))
                    .into_any_element()
            }
        };
        if trace.error_count > 0 {
            let errors = match trace.error_count {
                1 => "1 error".to_owned(),
                count => format!("{count} errors"),
            };
            head = head.child(chip(errors, t.danger, cx));
        }
        head = head.child(ui::meta(
            format!(
                "{} · {} spans · {}",
                duration(trace.duration_ms),
                trace.spans.len(),
                trace.services.join(", ")
            ),
            cx,
        ));
        let total = trace.duration_ms.max(0.001);
        let spans = trace.spans.iter().enumerate().map(|(index, span)| {
            let left = (span.offset_ms / total) as f32;
            let width = ((span.duration_ms / total) as f32).max(0.002);
            div()
                .id(("signoz-span", index))
                .px(px(16.))
                .py(px(3.))
                .flex()
                .items_center()
                .gap(px(10.))
                .when_some(span.status.clone(), |row, status| {
                    row.tooltip(ui::tooltip(status))
                })
                .child(
                    div()
                        .w(px(320.))
                        .flex_none()
                        .pl(px(8. * span.depth.min(MAX_INDENT) as f32))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .min_w_0()
                        .child(
                            ui::mono(span.name.clone(), 11.5, cx)
                                .text_color(if span.error { t.danger } else { t.text_primary })
                                .truncate(),
                        )
                        .child(ui::meta(span.service.clone(), cx).truncate()),
                )
                .child(
                    div().flex_1().h(px(10.)).relative().child(
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .left(relative(left.min(1.)))
                            .w(relative(width.min(1. - left.min(1.))))
                            .rounded(px(2.))
                            .bg(if span.error { t.danger } else { t.accent }),
                    ),
                )
                .child(
                    div()
                        .w(px(64.))
                        .flex()
                        .justify_end()
                        .child(ui::meta(duration(span.duration_ms), cx)),
                )
        });
        let waterfall = div()
            .flex()
            .flex_col()
            .when(trace.truncated, |list| {
                list.child(quiet(
                    "This trace has more spans than Sikemux reads at once; the latest ones are missing.",
                    cx,
                ))
            })
            .when(trace.spans.is_empty(), |list| {
                list.child(quiet(
                    "No spans for this trace. The service may send logs but not traces.",
                    cx,
                ))
            })
            .children(spans);
        let lines = match logs {
            Ok(Some(page)) if page.lines.is_empty() => {
                quiet("No log lines carry this trace id.", cx)
            }
            Ok(Some(page)) => {
                let lines: Vec<LogLine> = page.lines.iter().rev().cloned().collect();
                div()
                    .flex()
                    .flex_col()
                    .children(
                        lines
                            .iter()
                            .enumerate()
                            .map(|(index, line)| self.log_row(line, index, true, &lines, None, cx)),
                    )
                    .into_any_element()
            }
            Ok(None) => super::loading(cx),
            Err(failure) => super::failure(&failure, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("signoz-trace-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .py(px(8.))
                    .child(waterfall)
                    .child(
                        div()
                            .px(px(16.))
                            .child(ui::caps("Log lines in this trace", cx)),
                    )
                    .child(lines),
            )
            .into_any_element()
    }

    fn dashboards(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let find = field(
            &mut self.dashboard_find,
            &mut self.typing,
            "Find a dashboard",
            false,
            None,
            window,
            cx,
        );
        let wanted = words(&self.dashboard_find, cx);
        let bar = div()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(240.))
                    .child(ui::Field::new(&find).icon(Icon::Search)),
            );
        let all = match host
            .view("dashboardsView", Value::Null, cx)
            .read::<Vec<DashboardSummary>>()
        {
            Ok(Some(all)) => all,
            Ok(None) => {
                return div()
                    .size_full()
                    .child(bar)
                    .child(super::loading(cx))
                    .into_any_element()
            }
            Err(failure) => {
                return div()
                    .size_full()
                    .child(bar)
                    .child(super::failure(&failure, cx))
                    .into_any_element()
            }
        };
        let mut shown: Vec<&DashboardSummary> = all
            .iter()
            .filter(|dashboard| {
                wanted.is_empty()
                    || dashboard.title.to_lowercase().contains(&wanted)
                    || dashboard
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&wanted))
            })
            .collect();
        shown.sort_by_key(|dashboard| dashboard.title.to_lowercase());
        let bar = bar.child(ui::meta(format!("{} dashboards", shown.len()), cx));
        if shown.is_empty() {
            let said = if wanted.is_empty() {
                "No dashboards yet."
            } else {
                "No dashboard matches."
            };
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(bar)
                .child(super::empty(said, cx))
                .into_any_element();
        }
        let rows = shown.into_iter().enumerate().map(|(index, dashboard)| {
            let id = dashboard.id.clone();
            div()
                .id(("signoz-dashboard", index))
                .px(px(16.))
                .py(px(8.))
                .flex()
                .items_center()
                .gap(px(12.))
                .cursor_pointer()
                .hover(move |row| row.bg(t.surface_hover))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            ui::text(dashboard.title.clone(), 12.5, FontWeight::MEDIUM)
                                .text_color(t.text_primary)
                                .truncate(),
                        )
                        .when(!dashboard.description.is_empty(), |cell| {
                            cell.child(ui::meta(dashboard.description.clone(), cx).truncate())
                        }),
                )
                .child(
                    div().flex().gap(px(4.)).children(
                        dashboard
                            .tags
                            .iter()
                            .map(|tag| chip(tag.clone(), t.text_secondary, cx)),
                    ),
                )
                .child(
                    div()
                        .w(px(60.))
                        .flex()
                        .justify_end()
                        .child(ui::meta(format!("{} panels", dashboard.panels), cx)),
                )
                .on_click(listen::<Signoz, _>(cx, move |signoz, _, _, _, cx| {
                    signoz.dashboard = Some(id.clone());
                    cx.notify();
                }))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(
                div()
                    .id("signoz-dashboards")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    fn dashboard_view(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        id: &str,
        url: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let _ = window;
        let t = Tokens::get(cx).clone();
        let dashboard = match host
            .view("dashboardView", json!({ "id": id }), cx)
            .read::<Dashboard>()
        {
            Ok(Some(dashboard)) => dashboard,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let chosen = settings
            .dashboard_variables
            .get(id)
            .cloned()
            .unwrap_or_default();
        let variables: BTreeMap<String, String> = dashboard
            .variables
            .iter()
            .map(|variable| {
                let value = chosen
                    .get(&variable.name)
                    .cloned()
                    .unwrap_or_else(|| variable.selected.clone());
                (variable.name.clone(), value)
            })
            .collect();
        let open = format!("{}/dashboard/{id}", url.trim_end_matches('/'));
        let mut bar = div()
            .px(px(16.))
            .py(px(8.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.));
        for variable in &dashboard.variables {
            let current = variables.get(&variable.name).cloned().unwrap_or_default();
            let mut options = vec![current.clone()];
            options.extend(
                variable
                    .options
                    .iter()
                    .filter(|option| **option != current)
                    .cloned(),
            );
            let (dashboard_id, name) = (id.to_owned(), variable.name.clone());
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(ui::meta(variable.name.clone(), cx))
                    .child(ui::select(
                        SharedString::from(format!("signoz-variable-{}", variable.name)),
                        current.clone(),
                        options
                            .into_iter()
                            .map(|option| ui::Pick::new(option.clone(), option))
                            .collect(),
                        Some(current.into()),
                        false,
                        listen::<Signoz, SharedString>(cx, move |_, host, picked, _, cx| {
                            let (dashboard_id, name, picked) =
                                (dashboard_id.clone(), name.clone(), picked.to_string());
                            host.save_settings(
                                |kept| {
                                    let all = kept
                                        .entry("dashboardVariables")
                                        .or_insert_with(|| json!({}));
                                    if !all.is_object() {
                                        *all = json!({});
                                    }
                                    let mine = &mut all[&dashboard_id];
                                    if !mine.is_object() {
                                        *mine = json!({});
                                    }
                                    mine[&name] = json!(picked);
                                },
                                cx,
                            );
                        }),
                        cx,
                    )),
            );
        }
        let open_link = open.clone();
        bar = bar.child(div().flex_1()).child(
            Button::new("signoz-open-dashboard", "Open in SigNoz", Variant::Outline)
                .icon(Icon::Globe)
                .on_click(move |_, _, cx| cx.open_url(&open_link)),
        );
        let scope = self.scope(settings);
        let rows = dashboard
            .panels
            .iter()
            .map(|panel| panel.layout.y.saturating_add(panel.layout.h.max(1)))
            .filter(|end| *end < 10_000)
            .max()
            .unwrap_or(1);
        let gap = 8.;
        let mut grid = div()
            .grid()
            .grid_cols(12)
            .grid_rows(rows as u16)
            .gap(px(gap))
            .h(px(
                rows as f32 * DASHBOARD_ROW + (rows.saturating_sub(1)) as f32 * gap
            ));
        for (index, panel) in dashboard.panels.iter().enumerate() {
            let x = panel.layout.x.min(11);
            let width = panel.layout.w.clamp(1, 12 - x);
            let height = panel.layout.h.max(1);
            let place = |cell: Div| {
                cell.col_start(x as i16 + 1)
                    .col_span(width as u16)
                    .row_start(panel.layout.y.min(9_999) as i16 + 1)
                    .row_span(height as u16)
            };
            if height <= 1 {
                grid = grid.child(place(
                    div().flex().items_end().child(
                        ui::text(panel.title.clone(), 13., FontWeight::SEMIBOLD)
                            .text_color(t.text_primary),
                    ),
                ));
                continue;
            }
            let body = self.panel_body(host, panel, &scope, &variables, &open, index, cx);
            grid = grid.child(place(
                div()
                    .min_w_0()
                    .min_h_0()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .overflow_hidden()
                    .rounded(RADIUS_2)
                    .border_1()
                    .border_color(t.border)
                    .child(
                        ui::text(
                            if panel.title.is_empty() {
                                "Untitled panel".to_owned()
                            } else {
                                panel.title.clone()
                            },
                            12.,
                            FontWeight::MEDIUM,
                        )
                        .text_color(t.text_primary)
                        .truncate(),
                    )
                    .child(div().flex_1().min_h_0().child(body)),
            ));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(
                div()
                    .id("signoz-dashboard-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(16.))
                    .pb(px(16.))
                    .child(grid),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn panel_body(
        &self,
        host: &mut Host,
        panel: &Chart,
        scope: &Map<String, Value>,
        variables: &BTreeMap<String, String>,
        open: &str,
        index: usize,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if !panel.drawable {
            let open = open.to_owned();
            return div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(ui::meta(
                    format!("Sikemux does not draw {} panels yet.", panel.kind),
                    cx,
                ))
                .child(
                    Button::new(
                        ("signoz-panel-open", index),
                        "Open in SigNoz",
                        Variant::Outline,
                    )
                    .on_click(move |_, _, cx| cx.open_url(&open)),
                )
                .into_any_element();
        }
        let mut asked = scope.clone();
        asked.insert("kind".into(), json!(panel.kind));
        asked.insert("query".into(), panel.query.clone());
        asked.insert("variables".into(), json!(variables));
        let data = match host
            .view("panelView", Value::Object(asked), cx)
            .read::<PanelData>()
        {
            Ok(Some(data)) => data,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let unit = panel.unit.clone();
        match &*data {
            PanelData::Value { value } => {
                ui::text(value_in(*value, &unit), 24., FontWeight::SEMIBOLD)
                    .text_color(t.text_primary)
                    .into_any_element()
            }
            PanelData::Series { series } if series.is_empty() => {
                quiet("No data in this window.", cx)
            }
            PanelData::Series { series } => series_chart(index, series, &unit, cx),
            PanelData::Table { rows, .. } if rows.is_empty() => {
                quiet("No rows in this window.", cx)
            }
            PanelData::Table { columns, rows } if panel.kind == "pie" => {
                let value_at = columns
                    .iter()
                    .rposition(|column| column.aggregation)
                    .unwrap_or(0);
                let mut parts: Vec<(String, f64)> = rows
                    .iter()
                    .map(|row| {
                        let label = row
                            .iter()
                            .enumerate()
                            .filter(|(at, _)| {
                                !columns.get(*at).is_some_and(|column| column.aggregation)
                            })
                            .map(|(_, cell)| {
                                cell.as_str()
                                    .map_or_else(|| cell.to_string(), str::to_owned)
                            })
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let value = row
                            .get(value_at)
                            .and_then(Value::as_f64)
                            .unwrap_or_default();
                        (
                            if label.is_empty() {
                                "value".into()
                            } else {
                                label
                            },
                            value,
                        )
                    })
                    .collect();
                parts.sort_by(|left, right| right.1.total_cmp(&left.1));
                let most = parts.first().map_or(1., |(_, value)| value.max(1e-9));
                div()
                    .id(("signoz-bars", index))
                    .size_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(parts.into_iter().enumerate().map(|(at, (label, value))| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(120.))
                                    .flex_none()
                                    .child(ui::meta(label, cx).truncate()),
                            )
                            .child(
                                div().flex_1().h(px(8.)).child(
                                    div()
                                        .h_full()
                                        .w(relative((value / most) as f32))
                                        .rounded(RADIUS_1)
                                        .bg(series_ink(at, cx)),
                                ),
                            )
                            .child(
                                div()
                                    .w(px(64.))
                                    .flex()
                                    .justify_end()
                                    .child(ui::meta(value_in(Some(value), &unit), cx)),
                            )
                    }))
                    .into_any_element()
            }
            PanelData::Table { columns, rows } => div()
                .id(("signoz-table", index))
                .size_full()
                .overflow_scroll()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .pb(px(4.))
                        .children(columns.iter().map(|column| {
                            let name = if column.name.starts_with("__result") {
                                "value".to_owned()
                            } else {
                                column.name.clone()
                            };
                            div().w(px(120.)).flex_none().child(ui::caps(&name, cx))
                        })),
                )
                .children(rows.iter().map(|row| {
                    div()
                        .flex()
                        .gap(px(10.))
                        .py(px(2.))
                        .children(row.iter().enumerate().map(|(at, cell)| {
                            let aggregation =
                                columns.get(at).is_some_and(|column| column.aggregation);
                            let text = match cell.as_f64() {
                                Some(number) => {
                                    value_in(Some(number), if aggregation { &unit } else { "" })
                                }
                                None => cell
                                    .as_str()
                                    .map_or_else(|| cell.to_string(), str::to_owned),
                            };
                            div().w(px(120.)).flex_none().child(
                                ui::text(text, 11.5, FontWeight::NORMAL)
                                    .text_color(t.text_primary)
                                    .truncate(),
                            )
                        }))
                }))
                .into_any_element(),
        }
    }
}

/// The colour of the `index`th series a chart draws.
fn series_ink(index: usize, cx: &App) -> Hsla {
    let t = Tokens::get(cx);
    [
        t.accent,
        t.live,
        t.warn,
        t.danger,
        t.host,
        t.cmd,
        t.ssh,
        t.git_renamed,
    ][index % 8]
}

/// Every series of a panel over time, each in its own colour, the points lined up by moment.
fn series_chart(
    index: usize,
    series: &[sikemux_wire::signoz::Series],
    unit: &str,
    cx: &App,
) -> AnyElement {
    let mut moments: Vec<u64> = series
        .iter()
        .flat_map(|each| each.points.iter().map(|(at, _)| *at))
        .collect();
    moments.sort_unstable();
    moments.dedup();
    let rows: Vec<(u64, Vec<f64>)> = moments
        .iter()
        .map(|at| {
            let values = series
                .iter()
                .map(|each| {
                    each.points
                        .iter()
                        .find(|(when, _)| when == at)
                        .map_or(0., |(_, value)| *value)
                })
                .collect();
            (*at, values)
        })
        .collect();
    let span =
        moments.last().copied().unwrap_or_default() - moments.first().copied().unwrap_or_default();
    let unit = unit.to_owned();
    let mut chart = AreaChart::new(rows.clone())
        .id(("signoz-series", index))
        .x(move |(at, _): &(u64, Vec<f64>)| SharedString::from(time_label(*at, span)))
        .tick_margin((rows.len() / 4).max(1))
        .tooltip_value(move |_, _, value| value_in(Some(value), &unit).into());
    for (at, each) in series.iter().enumerate() {
        let ink = series_ink(at, cx);
        chart = chart
            .y(move |(_, values): &(u64, Vec<f64>)| values.get(at).copied().unwrap_or_default())
            .stroke(ink)
            .fill(ink.opacity(0.12))
            .name(each.label.clone());
    }
    div().size_full().child(chart).into_any_element()
}

impl Signoz {
    /// The address first, because it answers everything else: which version this is, and,
    /// once an email is typed, whether that person signs in with a password there at all.
    fn sign_in(
        &mut self,
        host: &mut Host,
        status: &Value,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if self.form.is_none() {
            let url_now = status["url"].as_str().unwrap_or_default().to_owned();
            let email_now = status["email"].as_str().unwrap_or_default().to_owned();
            let look: Typed = |signoz, host, event, _, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    signoz.inspect(host, true, cx);
                }
            };
            let submit: Typed = |signoz, host, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    signoz.submit(host, cx);
                }
            };
            let mut make = |placeholder: &'static str, masked: bool, typed| {
                let mut slot = None;
                field(
                    &mut slot,
                    &mut self.typing,
                    placeholder,
                    masked,
                    typed,
                    window,
                    cx,
                )
            };
            let url = make("https://signoz.example.com", false, Some(look));
            let email = make("you@example.com", false, Some(look));
            let password = make("Password", true, Some(submit));
            let key = make("API key", true, Some(submit));
            let account = make("Keychain account", false, Some(submit));
            url.update(cx, |input, cx| input.set_value(url_now.clone(), window, cx));
            email.update(cx, |input, cx| input.set_value(email_now, window, cx));
            self.form = Some(SignIn {
                url,
                email,
                password,
                key,
                account,
                method: if status["auth"] == "apiKey" {
                    Method::ApiKey
                } else {
                    Method::Password
                },
                keychain: false,
                editing_url: url_now.is_empty(),
                inspection: None,
                org: String::new(),
                busy: false,
                error: None,
            });
            if !url_now.is_empty() {
                self.inspect(host, true, cx);
            }
        }
        let form = self.form.as_ref().expect("made just now");
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let orgs: Vec<Value> = form
            .inspection
            .as_ref()
            .and_then(|found| found["orgs"].as_array().cloned())
            .unwrap_or_default();
        let password_orgs: Vec<&Value> =
            orgs.iter().filter(|org| org["password"] == true).collect();
        let sso_only = form.inspection.is_some() && !orgs.is_empty() && password_orgs.is_empty();
        let ready = !form.busy
            && !value(&form.url).is_empty()
            && match form.method {
                Method::Password => {
                    !value(&form.email).is_empty()
                        && !form.password.read(cx).value().is_empty()
                        && !sso_only
                }
                Method::ApiKey if form.keychain => !value(&form.account).is_empty(),
                Method::ApiKey => !value(&form.key).is_empty(),
            };
        let labelled = |label: &'static str, input: &Entity<InputState>, mono: bool| {
            let field = ui::Field::new(input);
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(ui::meta(label, cx))
                .child(if mono { field.mono() } else { field })
        };
        let address = if form.editing_url {
            labelled("SigNoz URL", &form.url, true).into_any_element()
        } else {
            let version = form
                .inspection
                .as_ref()
                .and_then(|found| found["version"].as_str())
                .map(|version| format!("SigNoz {version}"));
            let shown = value(&form.url)
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ui::mono(shown, 12.5, cx).text_color(t.text_primary))
                .children(version.map(|version| ui::meta(version, cx)))
                .child(
                    Button::new("signoz-change-url", "Change", Variant::Outline).on_click(
                        listen::<Signoz, _>(cx, |signoz, _, _, _, cx| {
                            if let Some(form) = &mut signoz.form {
                                form.editing_url = true;
                            }
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element()
        };
        let mut fields = div().flex().flex_col().gap(px(10.)).child(address);
        if status["keyFromEnvironment"] == true {
            fields = fields.child(ui::meta(
                "SIGNOZ_API_KEY is set in your shell, so Sikemux uses that key.",
                cx,
            ));
        }
        match form.method {
            Method::Password => {
                fields = fields.child(labelled("Email", &form.email, false));
                if form
                    .inspection
                    .as_ref()
                    .is_some_and(|found| found["accountExists"] == false)
                {
                    fields = fields.child(
                        ui::text("SigNoz does not know this email.", 12., FontWeight::NORMAL)
                            .text_color(t.warn),
                    );
                }
                if password_orgs.len() > 1 {
                    let picks = password_orgs
                        .iter()
                        .map(|org| {
                            let id = org["id"].as_str().unwrap_or_default().to_owned();
                            let name = org["name"]
                                .as_str()
                                .filter(|name| !name.is_empty())
                                .unwrap_or(&id)
                                .to_owned();
                            ui::Pick::new(id, name)
                        })
                        .collect();
                    let shown = password_orgs
                        .iter()
                        .find(|org| org["id"] == json!(form.org))
                        .and_then(|org| org["name"].as_str())
                        .unwrap_or("Choose one")
                        .to_owned();
                    fields = fields.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(ui::meta("Organisation", cx))
                            .child(ui::select(
                                "signoz-org",
                                shown,
                                picks,
                                Some(form.org.clone().into()),
                                false,
                                listen::<Signoz, SharedString>(cx, |signoz, _, picked, _, cx| {
                                    if let Some(form) = &mut signoz.form {
                                        form.org = picked.to_string();
                                    }
                                    cx.notify();
                                }),
                                cx,
                            )),
                    );
                }
                if sso_only {
                    let providers: Vec<String> = orgs
                        .iter()
                        .flat_map(|org| org["sso"].as_array().cloned().unwrap_or_default())
                        .filter_map(|sso| sso["provider"].as_str().map(str::to_owned))
                        .collect();
                    fields = fields.child(
                        ui::text(
                            format!(
                                "This organisation signs in through {}. Sikemux cannot do that yet, so use an API key.",
                                providers.join(" or ")
                            ),
                            12.,
                            FontWeight::NORMAL,
                        )
                        .text_color(t.warn),
                    );
                } else {
                    fields = fields.child(labelled("Password", &form.password, false));
                }
            }
            Method::ApiKey => {
                fields = if form.keychain {
                    fields.child(labelled("Keychain account", &form.account, true)).child(ui::meta(
                        "The account your signoz CLI saved its key under, in the signoz-api Keychain item.",
                        cx,
                    ))
                } else {
                    fields
                        .child(labelled("API key", &form.key, true))
                        .child(ui::meta(
                            "In SigNoz: Settings → Service Accounts → a service account → Keys.",
                            cx,
                        ))
                };
                fields = fields.child(
                    Toggle::new(
                        "signoz-keychain",
                        "The Keychain already has a key for this SigNoz",
                        form.keychain,
                    )
                    .on_click(listen::<Signoz, _>(
                        cx,
                        |signoz, _, _, _, cx| {
                            if let Some(form) = &mut signoz.form {
                                form.keychain = !form.keychain;
                            }
                            cx.notify();
                        },
                    )),
                );
            }
        }
        let action = match (form.busy, form.method) {
            (true, _) => "Connecting…",
            (false, Method::Password) => "Sign in",
            (false, Method::ApiKey) => "Use key",
        };
        let other = match form.method {
            Method::Password => "Use an API key instead",
            Method::ApiKey => "Sign in with email instead",
        };
        let settings_page =
            (form.method == Method::ApiKey && !form.keychain && !value(&form.url).is_empty())
                .then(|| format!("{}/settings", value(&form.url).trim_end_matches('/')));
        let foot = (form.method == Method::Password && !sso_only).then(|| {
            ui::meta("Your password goes to SigNoz once. Sikemux keeps only the session, in the macOS Keychain.", cx)
        });
        let error = form.error.clone().or_else(|| {
            status["message"]
                .as_str()
                .filter(|_| status["configured"] == true)
                .map(|message| message.to_owned().into())
        });
        div()
            .id("signoz-sign-in")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(380.))
                    .py(px(40.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(ui::picture(Icon::Signoz, 28., signoz_mark()))
                    .child(ui::text("Connect SigNoz", 15., FontWeight::SEMIBOLD).text_color(t.text_primary))
                    .child(
                        ui::text(
                            "Logs, traces and dashboards from your SigNoz, beside the code that wrote them.",
                            12.5,
                            FontWeight::NORMAL,
                        )
                        .text_color(t.text_secondary),
                    )
                    .child(fields)
                    .child(
                        Button::new("signoz-submit", action, Variant::Primary)
                            .disabled(!ready)
                            .on_click(listen::<Signoz, _>(cx, |signoz, host, _, _, cx| signoz.submit(host, cx))),
                    )
                    .children(error.map(|error| ui::text(error, 12., FontWeight::NORMAL).text_color(t.danger)))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(Button::new("signoz-method", other, Variant::Outline).on_click(listen::<Signoz, _>(
                                cx,
                                |signoz, _, _, _, cx| {
                                    if let Some(form) = &mut signoz.form {
                                        form.method = match form.method {
                                            Method::Password => Method::ApiKey,
                                            Method::ApiKey => Method::Password,
                                        };
                                    }
                                    cx.notify();
                                },
                            )))
                            .children(settings_page.map(|page| {
                                Button::new("signoz-settings", "Open SigNoz settings", Variant::Outline)
                                    .icon(Icon::Globe)
                                    .on_click(move |_, _, cx| cx.open_url(&page))
                            })),
                    )
                    .children(foot),
            )
            .into_any_element()
    }

    /// Asks the address what SigNoz runs there and, with the email, how that person signs in.
    fn inspect(&mut self, host: &Host, with_email: bool, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let url = form.url.read(cx).value().trim().to_owned();
        if url.is_empty() {
            return;
        }
        let email = form.email.read(cx).value().trim().to_owned();
        let email = (with_email && !email.is_empty()).then_some(email);
        form.error = None;
        after::<Signoz, _>(
            host.call("inspect", json!({ "url": url, "email": email })),
            |signoz, _, found, _| {
                let Some(form) = &mut signoz.form else {
                    return;
                };
                match found {
                    Ok(found) => {
                        let passwords: Vec<String> = found["orgs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|org| org["password"] == true)
                            .filter_map(|org| org["id"].as_str().map(str::to_owned))
                            .collect();
                        if let [only] = passwords.as_slice() {
                            form.org = only.clone();
                        }
                        form.editing_url = false;
                        form.inspection = Some(found);
                    }
                    Err(failure) => {
                        form.inspection = None;
                        form.editing_url = true;
                        form.error = Some(failure.message);
                    }
                }
            },
            cx,
        );
    }

    fn submit(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.busy {
            return;
        }
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let url = value(&form.url);
        let (method, params) = match form.method {
            Method::Password => (
                "signIn",
                json!({
                    "url": url,
                    "email": value(&form.email),
                    "password": form.password.read(cx).value().to_string(),
                    "orgId": (!form.org.is_empty()).then(|| form.org.clone()),
                }),
            ),
            Method::ApiKey if form.keychain => (
                "useApiKey",
                json!({ "url": url, "account": value(&form.account) }),
            ),
            Method::ApiKey => (
                "useApiKey",
                json!({ "url": url, "apiKey": value(&form.key) }),
            ),
        };
        form.busy = true;
        form.error = None;
        after::<Signoz, _>(
            host.call(method, params),
            |signoz, _, done, _| {
                let Some(form) = &mut signoz.form else {
                    return;
                };
                form.busy = false;
                form.error = match done {
                    Ok(status) if status["ok"] == true => None,
                    Ok(status) => Some(
                        status["message"]
                            .as_str()
                            .unwrap_or("SigNoz did not answer the first query")
                            .to_owned()
                            .into(),
                    ),
                    Err(failure) => Some(failure.message),
                };
            },
            cx,
        );
        cx.notify();
    }
}

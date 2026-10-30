//! What frames each page: its title, the time window and whether it is live.

use super::*;

use crate::theme::{SPACE_2, SPACE_2H, SPACE_4};

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
    pub(super) fn main(
        &mut self,
        host: &mut Host,
        status: &Status,
        settings: &Settings,
        services: Result<Option<Rc<Services>>, crate::plugin_pane::Failure>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let rows = services.as_ref().ok().cloned().flatten();
        let head = div()
            .px(SPACE_4)
            .py(SPACE_2H)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_2H)
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
            div().px(SPACE_4).pt(SPACE_2).child(
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
            .gap(SPACE_2)
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
                        host.update_settings(
                            |settings: &mut Settings| match settings
                                .pins
                                .iter()
                                .position(|kept| *kept == pin)
                            {
                                Some(at) => {
                                    settings.pins.remove(at);
                                }
                                None => settings.pins.push(pin),
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
            .gap(SPACE_2)
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
                        host.update_settings(
                            |settings: &mut Settings| settings.minutes = Some(minutes),
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
        status: &Status,
        settings: &Settings,
        services: Result<Option<Rc<Services>>, crate::plugin_pane::Failure>,
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
        let url = status.details.url.clone();
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

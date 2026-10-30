//! The services with their health, and one service's overview.

use super::*;

use crate::theme::{SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4};

/// A box with a small heading over what it holds.
fn card(title: &str, body: impl IntoElement, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .p(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .child(ui::caps(title, cx))
        .child(body)
}

const SORTS: [(ServiceSort, &str, bool); 4] = [
    (ServiceSort::Name, "Service", false),
    (ServiceSort::Calls, "Requests", true),
    (ServiceSort::Errors, "Error rate", true),
    (ServiceSort::P99, "p99", true),
];

/// A row of a table: its first cell takes what room is left, the rest sit at fixed widths.
fn table_row(cells: Vec<AnyElement>, widths: &[f32]) -> Div {
    let mut row = div().flex().items_center().gap(SPACE_3).min_w_0();
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
    pub(super) fn services(
        &mut self,
        settings: &Settings,
        services: Result<Option<Rc<Services>>, crate::plugin_pane::Failure>,
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
            .px(SPACE_4)
            .py(SPACE_2H)
            .flex()
            .items_center()
            .gap(SPACE_2H)
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
                    .child(crate::plugin_pane::loading(cx))
                    .into_any_element()
            }
            Err(failure) => {
                return div()
                    .size_full()
                    .child(bar)
                    .child(crate::plugin_pane::failure(&failure, cx))
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
                .child(crate::plugin_pane::empty(said, cx))
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
                            host.update_settings(
                                |settings: &mut Settings| settings.service_sort = by,
                                cx,
                            );
                        }))
                        .into_any_element()
                })
                .collect(),
            &widths,
        )
        .px(SPACE_4)
        .py(SPACE_1H)
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
            .px(SPACE_4)
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

    pub(super) fn overview(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        service: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let read = host
            .view("serviceView", self.scope(settings), cx)
            .read::<ServicePage>();
        let page = match read {
            Ok(Some(page)) => page,
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => return crate::plugin_pane::failure(&failure, cx),
        };
        let data = &page.overview;
        if data.calls == 0 {
            return crate::plugin_pane::empty(
                format!("{service} sent no traces in this window. Its logs may still be there."),
                cx,
            );
        }
        let minutes = self.span_minutes(settings);
        let tile = |label: &'static str, value: String, bad: bool| {
            div()
                .flex_1()
                .p(SPACE_3)
                .flex()
                .flex_col()
                .gap(SPACE_1)
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
            .gap(SPACE_2H)
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
            .gap(SPACE_2H)
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
                .pb(SPACE_1),
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
                                op: FilterOp::Equals,
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
                        .gap(SPACE_2H)
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
            .p(SPACE_4)
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .child(tiles)
            .child(charts)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(SPACE_2H)
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

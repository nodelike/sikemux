//! SigNoz's dashboards, and one drawn panel by panel.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4};

impl Signoz {
    pub(super) fn dashboards(
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
        let all = match host
            .view("dashboardsView", Value::Null, cx)
            .read::<Vec<DashboardSummary>>()
        {
            Ok(Some(all)) => all,
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
                .child(crate::plugin_pane::empty(said, cx))
                .into_any_element();
        }
        let rows = shown.into_iter().enumerate().map(|(index, dashboard)| {
            let id = dashboard.id.clone();
            div()
                .id(("signoz-dashboard", index))
                .px(SPACE_4)
                .py(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_3)
                .cursor_pointer()
                .hover(move |row| row.bg(t.surface_hover))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(SPACE_0)
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
                    div().flex().gap(SPACE_1).children(
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

    pub(super) fn dashboard_view(
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
            .view(
                "dashboardView",
                DashboardViewRequest { id: id.to_owned() },
                cx,
            )
            .read::<Dashboard>()
        {
            Ok(Some(dashboard)) => dashboard,
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => return crate::plugin_pane::failure(&failure, cx),
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
            .px(SPACE_4)
            .py(SPACE_2)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_2H);
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
                    .gap(SPACE_1H)
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
                            host.update_settings(
                                |settings: &mut Settings| {
                                    settings
                                        .dashboard_variables
                                        .entry(dashboard_id)
                                        .or_default()
                                        .insert(name, picked);
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
                    .p(SPACE_2H)
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
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
                    .px(SPACE_4)
                    .pb(SPACE_4)
                    .child(grid),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn panel_body(
        &self,
        host: &mut Host,
        panel: &Chart,
        scope: &Scope,
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
                .gap(SPACE_1H)
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
        let asked = PanelRequest {
            kind: panel.kind.clone(),
            query: panel.query.clone(),
            variables: variables.clone(),
            scope: scope.clone(),
        };
        let data = match host.view("panelView", asked, cx).read::<PanelData>() {
            Ok(Some(data)) => data,
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => return crate::plugin_pane::failure(&failure, cx),
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
                    .gap(SPACE_1)
                    .children(parts.into_iter().enumerate().map(|(at, (label, value))| {
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
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
                        .gap(SPACE_2H)
                        .pb(SPACE_1)
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
                        .gap(SPACE_2H)
                        .py(SPACE_0)
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
    series: &[sikemux_plugin_signoz_views::Series],
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

//! The traces, and one trace's spans laid out in time.

use super::*;

use crate::theme::{SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4};

impl Signoz {
    pub(super) fn traces(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let asked = TraceSearch {
            limit: Some(TRACE_PAGE),
            ..self.trace_search(settings)
        };
        let show_service = asked.scope.service.is_none();
        let key = serde_json::to_string(&asked).unwrap_or_default();
        let columns = div()
            .px(SPACE_4)
            .py(SPACE_1)
            .flex()
            .gap(SPACE_2H)
            .border_b_1()
            .border_color(t.border)
            .child(div().w(px(96.)).child(ui::caps("Time", cx)))
            .child(div().w(px(52.)).child(ui::caps("Status", cx)))
            .when(show_service, |head| {
                head.child(div().w(px(130.)).child(ui::caps("Service", cx)))
            })
            .child(div().flex_1().child(ui::caps("Operation", cx)))
            .child(div().w(px(180.)).child(ui::caps("Duration", cx)));
        let page = match host.view("tracesView", asked, cx).read::<TracePage>() {
            Ok(Some(page)) => page,
            Ok(None) => {
                return div()
                    .size_full()
                    .child(columns)
                    .child(crate::plugin_pane::loading(cx))
                    .into_any_element()
            }
            Err(failure) => {
                return div()
                    .size_full()
                    .child(columns)
                    .child(crate::plugin_pane::failure(&failure, cx))
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
                .child(crate::plugin_pane::empty("No traces in this window.", cx))
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
                .px(SPACE_4)
                .py(SPACE_1)
                .flex()
                .items_center()
                .gap(SPACE_2H)
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
                        .gap(SPACE_1H)
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
            div().p(SPACE_2H).flex().justify_center().child(
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
        let Ok(mut asked) = serde_json::from_str::<TraceSearch>(&self.more_traces.key) else {
            return;
        };
        asked.offset = Some(offset);
        self.more_traces.reading = true;
        let key = self.more_traces.key.clone();
        after::<Signoz, _>(
            host.call("tracesView", asked),
            move |signoz, host, done, cx| {
                if signoz.more_traces.key != key {
                    return;
                }
                signoz.more_traces.reading = false;
                match done.and_then(|page| {
                    serde_json::from_value::<TracePage>(page)
                        .map_err(|error| crate::plugin_pane::Failure::core(error.to_string()))
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

    pub(super) fn trace_view(
        &mut self,
        host: &mut Host,
        trace_id: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let _ = window;
        let t = Tokens::get(cx).clone();
        let read = host
            .view(
                "traceView",
                TraceRequest {
                    trace_id: trace_id.to_owned(),
                    ..TraceRequest::default()
                },
                cx,
            )
            .read::<Trace>();
        let logs = host
            .view(
                "logsView",
                LogSearch {
                    scope: Scope {
                        minutes: Some(TRACE_LOG_LOOKBACK),
                        ..Scope::default()
                    },
                    trace_id: Some(trace_id.to_owned()),
                    limit: Some(500),
                    ..LogSearch::default()
                },
                cx,
            )
            .read::<LogPage>();
        let mut head = div()
            .px(SPACE_4)
            .py(SPACE_2H)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_2H)
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
                    .child(crate::plugin_pane::loading(cx))
                    .into_any_element()
            }
            Err(failure) => {
                return div()
                    .size_full()
                    .child(head)
                    .child(crate::plugin_pane::failure(&failure, cx))
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
                .px(SPACE_4)
                .py(px(3.))
                .flex()
                .items_center()
                .gap(SPACE_2H)
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
                        .gap(SPACE_1H)
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
        let lines =
            match logs {
                Ok(Some(page)) if page.lines.is_empty() => {
                    quiet("No log lines carry this trace id.", cx)
                }
                Ok(Some(page)) => {
                    let lines: Vec<LogLine> = page.lines.iter().rev().cloned().collect();
                    div()
                        .flex()
                        .flex_col()
                        .children(lines.iter().enumerate().map(|(index, line)| {
                            self.log_row(line, index, true, &lines, false, cx)
                        }))
                        .into_any_element()
                }
                Ok(None) => crate::plugin_pane::loading(cx),
                Err(failure) => crate::plugin_pane::failure(&failure, cx),
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
                    .gap(SPACE_3)
                    .py(SPACE_2)
                    .child(waterfall)
                    .child(
                        div()
                            .px(SPACE_4)
                            .child(ui::caps("Log lines in this trace", cx)),
                    )
                    .child(lines),
            )
            .into_any_element()
    }
}

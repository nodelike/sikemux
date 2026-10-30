//! The filters that narrow logs and traces, and the one being written.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_4};

impl Signoz {
    pub(super) fn filter_bar(
        &mut self,
        host: &mut Host,
        signal: Signal,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let mut row = div().flex().flex_wrap().items_center().gap(SPACE_1H);
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
                .map_or("", |(_, label)| *label);
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1)
                    .pl(SPACE_2)
                    .pr(SPACE_0)
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
            .px(SPACE_4)
            .py(SPACE_2)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
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
            op: FilterOp::Equals,
            value,
            keys: Vec::new(),
            values: Vec::new(),
        });
    }

    fn draft_signal(&self) -> Signal {
        self.signal().unwrap_or(Signal::Logs)
    }

    fn suggest_keys(&mut self, host: &Host, text: &str, cx: &mut Context<PluginPane>) {
        let call = host.call(
            "fieldKeys",
            KeyQuery {
                signal: self.draft_signal(),
                search: text.to_owned(),
            },
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
        let params = ValueQuery {
            signal: self.draft_signal(),
            name,
            search: text.to_owned(),
        };
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
                    .gap(SPACE_1)
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
        let picked_at = OPS.iter().position(|(op, _)| *op == draft.op);
        let ops = ui::select(
            "signoz-op",
            op_label,
            OPS.iter()
                .enumerate()
                .map(|(at, (_, label))| ui::Pick::new(at.to_string(), *label))
                .collect(),
            picked_at.map(|at| at.to_string().into()),
            false,
            listen::<Signoz, SharedString>(cx, |signoz, _, picked, _, cx| {
                let op = picked.parse::<usize>().ok().and_then(|at| OPS.get(at));
                if let (Some(draft), Some((op, _))) = (&mut signoz.draft, op) {
                    draft.op = *op;
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
                .p(SPACE_2)
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
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
}

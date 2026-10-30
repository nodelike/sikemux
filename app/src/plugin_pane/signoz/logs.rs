//! The logs, read a page at a time or followed live, and one line opened.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4};

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

impl Signoz {
    pub(super) fn logs(
        &mut self,
        host: &mut Host,
        settings: &Settings,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let search = self.log_search(settings);
        let volume_search = VolumeQuery {
            search: search.clone(),
            buckets: Some(VOLUME_BUCKETS),
        };
        let volume = host
            .view("volumeView", volume_search, cx)
            .read::<Vec<VolumeBucket>>()
            .ok()
            .flatten();
        let strip = self.volume(volume.as_deref().map(Vec::as_slice), cx);
        let show_service = search.scope.service.is_none();
        let mut heads = vec![("Time", 96.), ("Level", 52.)];
        if show_service {
            heads.push(("Service", 130.));
        }
        heads.push(("Message", 0.));
        let columns = div()
            .px(SPACE_4)
            .py(SPACE_1)
            .flex()
            .gap(SPACE_2H)
            .border_b_1()
            .border_color(t.border)
            .children(heads.into_iter().map(|(label, width)| match width {
                0. => div().flex_1().child(ui::caps(label, cx)),
                width => div().w(px(width)).flex_none().child(ui::caps(label, cx)),
            }));
        let asked = LogSearch {
            limit: Some(LOG_PAGE),
            ..search
        };
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
            let key = serde_json::to_string(&asked).unwrap_or_default();
            let page = match host.view("logsView", asked, cx).read::<LogPage>() {
                Ok(Some(page)) => page,
                Ok(None) => {
                    return div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(strip)
                        .child(columns)
                        .child(crate::plugin_pane::loading(cx))
                        .into_any_element()
                }
                Err(failure) => {
                    return div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(strip)
                        .child(columns)
                        .child(crate::plugin_pane::failure(&failure, cx))
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
                .px(SPACE_4)
                .py(SPACE_1H)
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
                .child(crate::plugin_pane::empty(said, cx))
                .into_any_element();
        }
        let rows: Vec<AnyElement> = lines
            .iter()
            .enumerate()
            .map(|(index, line)| self.log_row(line, index, show_service, &lines, true, cx))
            .collect();
        let older_button = older.map(|reading| {
            div().p(SPACE_2H).flex().justify_center().child(
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
    fn follow(&mut self, host: &Host, asked: LogSearch, cx: &mut Context<PluginPane>) {
        let key = serde_json::to_string(&asked).unwrap_or_default();
        if self.feed.key == key {
            return;
        }
        self.feed = Feed {
            key: key.clone(),
            ..Feed::default()
        };
        self.feed.scroll.scroll_to_bottom();
        let tail = host.stream::<Signoz>("tailLogs", asked, cx, move |signoz, _, streamed, _| {
            let feed = &mut signoz.feed;
            if feed.key != key {
                return;
            }
            match streamed {
                crate::plugin_pane::Streamed::Item(item) => {
                    let tick = match serde_json::from_value::<TailTick>(item) {
                        Ok(tick) => tick,
                        Err(error) => {
                            eprintln!("sikemux: a SigNoz live log tick did not read: {error}");
                            return;
                        }
                    };
                    feed.error = tick.error.map(SharedString::from);
                    if tick.lines.is_empty() {
                        return;
                    }
                    let at_bottom = -feed.scroll.offset().y >= feed.scroll.max_offset().y - px(48.);
                    feed.lines.extend(tick.lines);
                    while feed.lines.len() > KEPT_LINES {
                        feed.lines.pop_front();
                    }
                    if at_bottom {
                        feed.scroll.scroll_to_bottom();
                    }
                }
                crate::plugin_pane::Streamed::Ended(failure) => {
                    if let Some(failure) = failure {
                        feed.error = Some(failure.message);
                    }
                }
            }
        });
        self.feed.tail = Some(tail);
    }

    fn read_older(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(offset) = self.older.next.filter(|_| !self.older.reading) else {
            return;
        };
        let Ok(mut asked) = serde_json::from_str::<LogSearch>(&self.older.key) else {
            return;
        };
        asked.offset = Some(offset);
        self.older.reading = true;
        let key = self.older.key.clone();
        after::<Signoz, _>(
            host.call("logsView", asked),
            move |signoz, host, done, cx| {
                if signoz.older.key != key {
                    return;
                }
                signoz.older.reading = false;
                match done.and_then(|page| {
                    serde_json::from_value::<LogPage>(page)
                        .map_err(|error| crate::plugin_pane::Failure::core(error.to_string()))
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

    pub(super) fn log_row(
        &self,
        line: &LogLine,
        index: usize,
        show_service: bool,
        lines: &[LogLine],
        menu: bool,
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
            .px(SPACE_4)
            .py(px(3.))
            .flex()
            .items_center()
            .gap(SPACE_2H)
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
        if !menu {
            return self.with_detail(head, line, index, expanded, cx);
        }
        let chosen = self.chosen_lines(line, lines);
        let some_picked = !self.picked.is_empty();
        let pane = cx.weak_entity();
        let head = head.context_menu(ui::menu(move |popup, window, cx| {
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
            popup = crate::plugin_pane::send_submenu(popup, send, &pane, delivery, window, cx);
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
        }));
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
                    .gap(SPACE_0)
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
                                    .gap(SPACE_2)
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
                                                    op: FilterOp::Equals,
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
                                                    op: FilterOp::NotEquals,
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
            .px(SPACE_4)
            .pl(px(122.))
            .py(SPACE_1H)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
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
            .px(SPACE_4)
            .py(SPACE_2)
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

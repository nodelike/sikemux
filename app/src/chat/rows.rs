//! Drawing a chat's rows: a message and its parts, the work folded under a turn, letters and
//! fired triggers, and subagents. Runs of messages between agents are in `team`.

use gpui_kit::Pixels;

use super::*;
use crate::ui::Clicks;
use sikemux_view::chat::{
    ChiefWeek, GoalMoment, PageRef, TaskNotice, ToolCall, TriggerFired, WorkerPage, REPLY_URI,
};
use sikemux_view::slice::SliceState;

use crate::theme::{
    SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_5, TEXT_2XS_H,
    TEXT_3XS_H, TEXT_XS, TEXT_XS_H,
};

/// The chat's looks with what the reader has open, for drawing its rows once.
pub(super) struct Drawn<'a> {
    pub(super) looks: &'a Looks,
    pub(super) reading: &'a Reading,
}

impl std::ops::Deref for Drawn<'_> {
    type Target = Looks;

    fn deref(&self) -> &Looks {
        self.looks
    }
}

/// A message's row: the person's in a bubble at the right, the agent's across
/// the column. A finished turn folds its work behind a line saying how long it
/// worked, which the reader opens; its reply stays out.
pub(super) fn row(
    rows: &[ChatRow],
    ix: usize,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(row) = rows.get(ix) else {
        return div().into_any_element();
    };
    let message = &row.message;
    let user = message.role == ChatRole::User;
    let team = message.team.as_ref();
    let team_open = team.is_some_and(|team| looks.reading.team_open(&team.id));
    let before = ix.checked_sub(1).and_then(|at| rows.get(at));
    let from_here = rows.get(ix..).unwrap_or_default();
    let opens_run = team.filter(|team| {
        before.is_none_or(|before| {
            (before.message.team.as_ref()).is_none_or(|before| before.id != team.id)
        })
    });
    let team_head = opens_run.and_then(|team| {
        let line = team::line_of(from_here, &team.id)?;
        Some(team::head(&team.id, line, team_open, looks, cx))
    });
    let behind = team
        .filter(|_| team_open)
        .is_some_and(|team| team::behind(from_here, &team.id, looks.reading));
    if team.is_some_and(|team| behind || (!team_open && team.shown.is_empty())) {
        return div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .children(team_head)
            .into_any_element();
    }
    // A run folds a turn's work with it, so its rows show whole once it is open.
    let fold = message.fold.as_ref().filter(|_| team.is_none());
    let folded = fold.is_some_and(|fold| !looks.reading.opened.contains(&fold.id));
    let opens = fold.filter(|fold| {
        before.is_none_or(|before| {
            (before.message.fold.as_ref()).is_none_or(|before| before.id != fold.id)
        })
    });
    let work_shown = if user || team_open {
        1.
    } else if team.is_some() {
        0.
    } else {
        motion::follow(
            SharedString::from(format!("work-shown:{}", message.id)),
            if folded { 0. } else { 1. },
            if folded {
                motion::FOLD_AWAY
            } else {
                motion::UNFOLD
            },
            window,
            cx,
        )
    };
    let mut segments: Vec<(bool, Vec<&ChatPart>)> = Vec::new();
    for (index, part) in message.parts.iter().enumerate() {
        let tucked = match team {
            Some(team) => !team.shown.contains(&(index as u64)),
            None => fold.is_some_and(|fold| !fold.shown.contains(&(index as u64))),
        };
        if tucked && work_shown <= 0. {
            continue;
        }
        match segments.last_mut() {
            Some((was, parts)) if *was == tucked => parts.push(part),
            _ => segments.push((tucked, vec![part])),
        }
    }
    let live = ix + 1 == rows.len() && looks.reading.running.get();
    let follows_own = user && before.is_some_and(plainly_sent);
    let followed_by_own = user && rows.get(ix + 1).is_some_and(plainly_sent);
    let summed = if user {
        0.
    } else {
        motion::follow(
            SharedString::from(format!("work-line:{}", message.id)),
            if opens.is_some() { 1. } else { 0. },
            motion::UNFOLD,
            window,
            cx,
        )
    };
    let summary = opens
        .filter(|_| summed > 0.)
        .map(|fold| work(fold, !folded, looks, cx))
        .map(|line| {
            let key = format!("work-line-slot:{}", message.id);
            tucked(key, summed, false, vec![line], window, cx)
        });
    let whole_row_tucked =
        summary.is_none() && !segments.is_empty() && segments.iter().all(|(tucked, _)| *tucked);
    let last_segment = segments.len().saturating_sub(1);
    let mut parts: Vec<AnyElement> = Vec::new();
    for (at_segment, (tucked_away, shown)) in segments.into_iter().enumerate() {
        let first = shown.first().map(|part| part.id().to_owned());
        let groups = runs(shown.into_iter(), fold.is_some() && !tucked_away);
        let count = groups.len();
        let live = live && at_segment == last_segment;
        let mut drawn = Vec::with_capacity(count);
        for (at, group) in groups.into_iter().enumerate() {
            drawn.push(match group {
                Shown::Part(ChatPart::Notice { notice, .. })
                    if let Some(card) = notice
                        .subagent
                        .as_deref()
                        .and_then(|session| card_of(rows, &looks.reading.row_index, session)) =>
                {
                    self::subagent(card, pen, looks, window, cx)
                }
                Shown::Part(part @ ChatPart::Text { .. }) if at > 0 && !user => div()
                    .pt(SPACE_2H)
                    .child(self::part(row, part, user, pen, looks, window, cx))
                    .into_any_element(),
                Shown::Part(part) => self::part(row, part, user, pen, looks, window, cx),
                Shown::Run(calls) => tools::run(&calls, live && at + 1 == count, looks, window, cx),
                Shown::Sent(part, call) => {
                    sent_letter(row, part.id(), call, pen, looks, window, cx)
                }
                Shown::Tabs(first, pages) => self::page::tabs(first, &pages, looks, window, cx),
                Shown::WorkerPages(first, pages) => worker_pages(first, &pages, looks, window, cx),
            });
        }
        if !tucked_away || whole_row_tucked {
            parts.extend(drawn);
            continue;
        }
        let key = format!("work-parts:{}:{}", message.id, first.unwrap_or_default());
        parts.push(tucked(key, work_shown, folded, drawn, window, cx));
    }
    let carried = !message.attachments.is_empty() || !message.context.is_empty();
    let lost = (ix == 0 && looks.reading.earlier_lost).then(|| {
        div()
            .w_full()
            .pt(SPACE_3)
            .pb(SPACE_2)
            .flex()
            .justify_center()
            .child(
                ui::text(
                    t!("Earlier messages are no longer kept."),
                    TEXT_XS,
                    FontWeight::NORMAL,
                )
                .text_color(Tokens::get(cx).text_tertiary),
            )
    });
    if parts.is_empty() && message.letters.is_empty() && summary.is_none() && !carried {
        return div().children(lost).into_any_element();
    }
    let lettered = !message.letters.is_empty();
    let triggered = lettered
        && message
            .letters
            .iter()
            .all(|letter| letter.kind == LetterKind::Trigger);
    let content = if triggered {
        fired(row, pen, looks, window, cx)
    } else if lettered {
        letters(row, pen, looks, window, cx)
    } else if user {
        let editing = looks
            .reading
            .editing
            .as_ref()
            .filter(|editing| editing.is(&message.id));
        match editing {
            Some(editing) => div().w_full().flex().justify_end().child(edit::editor(
                &looks.agent,
                editing,
                looks,
            )),
            None => {
                let group = SharedString::from(format!("sent:{}", message.id));
                let bench = looks.bench.clone();
                let open = move |path: PathBuf, window: &mut Window, cx: &mut App| {
                    bench
                        .update(cx, |bench, cx| bench.show_picture(path, window, cx))
                        .ok();
                };
                div()
                    .group(group.clone())
                    .max_w(relative(0.8))
                    .flex()
                    .flex_col()
                    .items_end()
                    .children(attach::sent(
                        message,
                        open,
                        looks.file_pressed(),
                        |id| super::page::opens_page(id, looks),
                        |id| {
                            let open = looks.bench.upgrade()?.read(cx).call_at(id).is_some_and(
                                |(_, call)| call.state == sikemux_view::calls::CallState::Open,
                            );
                            open.then(|| {
                                let (bench, id, agent) =
                                    (looks.bench.clone(), id.to_owned(), looks.agent.clone());
                                std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                                    bench
                                        .update(cx, |bench, cx| bench.open_in_asks(&id, &agent, cx))
                                        .ok();
                                }) as super::page::Opens
                            })
                        },
                        cx,
                    ))
                    .when(!parts.is_empty(), |column| {
                        column.child(bubble(looks).children(parts))
                    })
                    .when(!followed_by_own, |column| {
                        column.children(meta::line(rows, ix, &group, looks, cx))
                    })
            }
        }
    } else {
        let group = SharedString::from(format!("answer:{}", message.id));
        div()
            .group(group.clone())
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(PART_GAP))
                    .min_w_0()
                    .children(summary)
                    .children(parts)
                    .children(left_out(row, looks, cx)),
            )
            .children(meta::line(rows, ix, &group, looks, cx))
    };
    let user = user && !lettered;
    let ended = shown::ended_at(
        rows,
        ix,
        looks.reading.row_index.prev_ended_at(),
        &looks.reading.shown,
    );
    let after = (!ended.is_empty()).then(|| {
        div()
            .w_full()
            .max_w(super::column::rows(cx))
            .px(px(ROW_SIDE))
            .children(ended.iter().map(|turn| {
                let key = format!("shown:{}", turn.turn);
                shown::pictures(&turn.pages, &key, looks, cx)
            }))
    });
    let drawn = div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .children(lost)
        .children(team_head)
        .child(
            div()
                .w_full()
                .max_w(super::column::rows(cx))
                .px(px(ROW_SIDE))
                .map(|row| {
                    if user {
                        let row = match follows_own {
                            true => row.pt(SPACE_1),
                            false => row.pt(SPACE_5),
                        };
                        row.when(!followed_by_own, |row| row.pb(SPACE_2H))
                            .flex()
                            .justify_end()
                    } else {
                        row.py(px(5.))
                    }
                })
                .when(ix == 0 && !looks.reading.earlier_lost, |row| {
                    row.pt(SPACE_3)
                })
                .child(content),
        )
        .children(after);
    if !whole_row_tucked {
        return drawn.into_any_element();
    }
    let key = SharedString::from(format!("work-row:{}", message.id));
    match folded {
        true => motion::folding_away(key, work_shown, drawn, window, cx),
        false => motion::folded(key, work_shown, drawn, window, cx),
    }
}

/// A message the person sent themselves, as opposed to letters or a trigger's prompt.
fn plainly_sent(row: &ChatRow) -> bool {
    row.message.role == ChatRole::User && row.message.letters.is_empty()
}

/// The person's bubble: no border, its tail at the bottom right.
pub(super) fn bubble(looks: &Looks) -> Div {
    div()
        .max_w_full()
        .px(px(14.))
        .py(SPACE_2H)
        .rounded(BUBBLE_RADIUS)
        .rounded_br(SPACE_1H)
        .bg(looks.bubble)
}

/// How round a bubble is, but for the corner its tail sits at.
pub(super) const BUBBLE_RADIUS: gpui_kit::Pixels = px(16.);

/// How far below its place the way back to the latest message starts as it comes up.
pub(super) const LATEST_RISE: f32 = 6.;

pub(super) const PART_GAP: f32 = 10.;

/// Parts a finished turn folds away, drawn `shown` of the way open; the gap after them folds
/// with them, so nothing below moves once they are gone.
pub(super) fn tucked(
    key: String,
    shown: f32,
    closing: bool,
    parts: Vec<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let key = SharedString::from(key);
    let body = div()
        .flex()
        .flex_col()
        .gap(px(PART_GAP))
        .pb(px(PART_GAP))
        .min_w_0()
        .children(parts);
    let slot = match closing {
        true => motion::folding_away(key, shown, body, window, cx),
        false => motion::folded(key, shown, body, window, cx),
    };
    div()
        .min_w_0()
        .mb(px(-PART_GAP))
        .child(slot)
        .into_any_element()
}

/// The chat's triggers that came due, each folded to a line with the trigger's name.
pub(super) fn fired(
    row: &ChatRow,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::Div {
    let drawn: Vec<AnyElement> = row
        .message
        .letters
        .iter()
        .enumerate()
        .filter_map(|(ix, letter)| Some((ix, letter.trigger.as_ref()?)))
        .map(|(ix, trigger)| trigger_letter(row, ix, trigger, pen, looks, window, cx))
        .collect();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACE_3H)
        .children(drawn)
}

/// A trigger that came due: a line with its name, which opens onto its prompt in a bubble
/// and the webhook request that fired it.
fn trigger_letter(
    row: &ChatRow,
    ix: usize,
    trigger: &TriggerFired,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = SharedString::from(format!("{}:{}", row.message.id, letter_doc(ix)));
    let key = format!("fired:{id}");
    let open = looks.reading.opened.contains(&key);
    let says = vec![
        ui::icon(Icon::Timer, 13.)
            .text_color(t.cmd)
            .into_any_element(),
        div()
            .min_w_0()
            .truncate()
            .child(trigger.title.clone())
            .into_any_element(),
    ];
    let line = fold_toggle(&key, open, says, looks).min_w_0();
    let opened = unfolding(&key, open, window, cx, |window, cx| {
        let prompt = row.doc(&letter_doc(ix)).map(|doc| {
            let look = looks.styles.borrow().user.clone();
            sent_bubble(
                pen.markdown_with(&look, id.clone(), &doc.markdown, window, cx),
                looks,
            )
        });
        let request = trigger.request.as_ref().map(|request| {
            let body = row.doc(&request_doc(ix)).map(|doc| {
                let look = looks.styles.borrow().text.clone();
                let body_id = SharedString::from(format!("{id}:request"));
                pen.markdown_with(&look, body_id, &doc.markdown, window, cx)
            });
            fired_request(&format!("request:{id}"), request, body, looks, &t)
        });
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(SPACE_1H)
            .children(prompt)
            .children(request)
    });
    from(line, row, None, looks)
        .children(opened)
        .into_any_element()
}

/// What a line the reader opens by `key` holds, while any of it shows.
fn unfolding(
    key: &str,
    open: bool,
    window: &mut Window,
    cx: &mut App,
    body: impl FnOnce(&mut Window, &mut App) -> Div,
) -> Option<AnyElement> {
    let shown = motion::follow(
        SharedString::from(format!("{key}:shown")),
        if open { 1. } else { 0. },
        if open {
            motion::UNFOLD
        } else {
            motion::FOLD_AWAY
        },
        window,
        cx,
    );
    (shown > 0.).then(|| {
        let body = body(window, cx);
        let key = SharedString::from(key.to_owned());
        match open {
            true => motion::folded(key, shown, body, window, cx),
            false => motion::folding_away(key, shown, body, window, cx),
        }
    })
}

/// A message a turn came to, folded to a line saying who it went to, which opens onto it in a
/// bubble.
fn sent_letter(
    row: &ChatRow,
    part: &str,
    call: &ToolCall,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = SharedString::from(format!("{}:{part}", row.message.id));
    let key = format!("sent:{id}");
    let open = looks.reading.opened.contains(&key);
    let (toggle, toggled) = (team::holding(looks), key.clone());
    let to = tools::agents_reached(&call.row, TEXT_XS_H, looks, cx).unwrap_or_else(|| {
        let to = div().flex_none().child(call.row.target.clone());
        vec![to.into_any_element()]
    });
    let line = team::line(&key, open, looks)
        .clicks(move |_, _, cx| toggle(cx, &|reading| reading.toggle(&toggled)))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(looks.ink)
                .child(t!("Sent to")),
        )
        .children(to)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(looks.quiet)
                .children(call.row.detail.clone()),
        );
    let opened = unfolding(&key, open, window, cx, |window, cx| {
        let text = row.doc(part).map(|doc| {
            let look = looks.styles.borrow().user.clone();
            pen.markdown_with(&look, id.clone(), &doc.markdown, window, cx)
        });
        div()
            .flex()
            .max_w(relative(0.78))
            .pt(SPACE_1H)
            .children(text.map(|text| sent_bubble(text, looks)))
    });
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(line)
        .children(opened)
        .into_any_element()
}

/// What another agent or a trigger sent: who over its bubble at the left, and when at the
/// line's end.
fn from(who: impl IntoElement, row: &ChatRow, body: Option<AnyElement>, looks: &Looks) -> Div {
    let when = row.message.sent_at.and_then(meta::clock).map(|when| {
        div()
            .ml_auto()
            .flex_none()
            .text_size(crate::theme::ui_text(TEXT_2XS_H))
            .text_color(looks.faint)
            .child(when)
    });
    div()
        .max_w(relative(0.78))
        .flex()
        .flex_col()
        .items_start()
        .gap(SPACE_1H)
        .child(
            div()
                .w_full()
                .h(px(22.))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .text_size(crate::theme::ui_text(TEXT_XS_H))
                .text_color(looks.quiet)
                .child(who)
                .children(when),
        )
        .children(body.map(|body| sent_bubble(body, looks)))
}

/// What another agent or a trigger said, in a bubble at the left.
fn sent_bubble(body: AnyElement, looks: &Looks) -> Div {
    div()
        .max_w_full()
        .px(px(14.))
        .py(SPACE_2H)
        .rounded(BUBBLE_RADIUS)
        .rounded_bl(SPACE_1H)
        .border(crate::theme::border())
        .border_color(looks.line)
        .bg(looks.asked)
        .child(body)
}

/// The webhook request that fired a trigger: one line saying what it was, which opens onto
/// its headers and body.
pub(super) fn fired_request(
    key: &str,
    request: &FiredRequest,
    body: Option<AnyElement>,
    looks: &Drawn,
    t: &Tokens,
) -> AnyElement {
    let open = looks.reading.opened.contains(key);
    let kind = request
        .headers
        .iter()
        .find(|header| header.name == "content-type")
        .and_then(|header| header.value.split(';').next())
        .map(str::trim);
    let size = match request.body.len() {
        bytes @ 0..1024 => format!("{bytes} B"),
        bytes => format!("{:.1} KB", bytes as f64 / 1024.),
    };
    let summary = [Some(request.method.as_str()), kind, Some(size.as_str())]
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .chain((!request.query.is_empty()).then(|| format!("?{}", request.query)))
        .collect::<Vec<_>>()
        .join(" · ");
    let (reader, toggled, hover) = (
        looks.reader.clone(),
        key.to_owned(),
        theme::mix(t.text_primary, t.surface_raised, 0.05),
    );
    let line = div()
        .id(SharedString::from(key.to_owned()))
        .min_w_0()
        .py(px(3.))
        .px(px(9.))
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(theme::radius_2())
        .hover(move |line| line.bg(hover))
        .clicks(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&toggled);
                cx.notify();
            })
        })
        .child(motion::turn(
            format!("{key}:chevron"),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 9.).text_color(looks.quiet),
        ))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(theme::mono_font())
                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                .text_color(looks.faint)
                .child(summary),
        );
    let details = open.then(|| {
        let headers = request.headers.iter().map(|header| {
            div()
                .flex()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex_none()
                        .text_color(looks.faint)
                        .child(header.name.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_color(looks.ink)
                        .child(header.value.clone()),
                )
        });
        div()
            .w_full()
            .p(SPACE_2H)
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .border(crate::theme::border())
            .border_color(looks.line)
            .rounded(theme::radius_2())
            .bg(t.surface_raised)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .font_family(theme::mono_font())
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .children(headers),
            )
            .children(body)
    });
    div()
        .max_w_full()
        .flex()
        .flex_col()
        .items_start()
        .gap(SPACE_1)
        .child(line)
        .children(details)
        .into_any_element()
}

/// Messages other agents sent and the person's answers to questions, which the core hands
/// the agent as one message: each drawn as what it is rather than as the text the agent reads.
pub(super) fn letters(
    row: &ChatRow,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::Div {
    let letters = &row.message.letters;
    let mut drawn = Vec::with_capacity(letters.len());
    let mut ix = 0;
    while let Some(letter) = letters.get(ix) {
        let letter_row = match letter.kind {
            LetterKind::Answer => {
                let rest = letters.get(ix..).unwrap_or_default();
                let run = (rest.iter())
                    .take_while(|next| {
                        next.kind == LetterKind::Answer && next.agent_id == letter.agent_id
                    })
                    .count();
                let answers = rest.get(..run).unwrap_or_default();
                drawn.push(answer_thread(row, answers, ix, looks, cx));
                ix += run;
                continue;
            }
            LetterKind::Message | LetterKind::Trigger => {
                if let Some(trigger) = &letter.trigger {
                    drawn.push(trigger_letter(row, ix, trigger, pen, looks, window, cx));
                    ix += 1;
                    continue;
                }
                let id = SharedString::from(format!("{}:{}", row.message.id, letter_doc(ix)));
                let text = row.doc(&letter_doc(ix)).map(|doc| {
                    let look = looks.styles.borrow().user.clone();
                    pen.markdown_with(&look, id.clone(), &doc.markdown, window, cx)
                });
                let who = match looks.bench.upgrade() {
                    Some(bench) => div().flex().child(super::agent_pill_of(
                        &letter.agent_id,
                        &letter.name,
                        looks.mark_of(&letter.agent_id, &letter.cli, TEXT_XS + 1., cx),
                        TEXT_XS,
                        bench.read(cx).chat_target(&looks.agent),
                        &looks.bench,
                        cx,
                    )),
                    None => div(),
                };
                from(who, row, text, looks).into_any_element()
            }
        };
        drawn.push(letter_row);
        ix += 1;
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACE_3H)
        .min_w_0()
        .children(drawn)
}

/// A run of answers to one agent's questions, drawn as one thread with the files each carried.
fn answer_thread(
    row: &ChatRow,
    answers: &[AgentLetter],
    first: usize,
    looks: &Drawn,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let Some(asker) = answers.first() else {
        return div().into_any_element();
    };
    let answered = answers.iter().enumerate().map(|(at, letter)| {
        let files = letter.attachments.iter().enumerate().map(|(index, path)| {
            let id = format!("{}:{}:{index}", row.message.id, letter_doc(first + at));
            let path = PathBuf::from(path);
            let bench = looks.bench.clone();
            let open = move |path, window: &mut Window, cx: &mut App| {
                bench
                    .update(cx, |bench, cx| bench.show_picture(path, window, cx))
                    .ok();
            };
            match attach::is_image(&path) {
                true => attach::thumbnail(id, path, open, t),
                false => attach::file_pill(id, path, looks.file_pressed(), t, cx),
            }
        });
        Answered {
            question: &letter.question,
            answer: &letter.text,
            secret: letter.secret,
            files: files.collect(),
        }
    });
    answered_questions(
        looks.mark_of(&asker.agent_id, &asker.cli, questions::ASKER_MARK, cx),
        (asker.agent_id != looks.agent).then_some(asker.name.as_str()),
        answered.collect(),
        looks,
    )
    .into_any_element()
}

pub(super) enum Shown<'a> {
    Part(&'a ChatPart),
    Run(Vec<&'a ChatPart>),
    Sent(&'a ChatPart, &'a ToolCall),
    /// Pages the agent showed, from the part named on.
    Tabs(&'a str, Vec<&'a PageRef>),
    WorkerPages(&'a str, Vec<&'a WorkerPage>),
}

/// A message's parts with each run of tool calls together, and pages shown one after another,
/// the agent's or one worker's, in one frame. With `apart`, a message sent to another agent
/// stands out of its run.
fn runs<'a>(parts: impl Iterator<Item = &'a ChatPart>, apart: bool) -> Vec<Shown<'a>> {
    let mut shown = Vec::new();
    for part in parts {
        match (part, shown.last_mut()) {
            (
                ChatPart::Tool {
                    tool,
                    sent: Some(_),
                    ..
                },
                _,
            ) if apart => shown.push(Shown::Sent(part, tool)),
            (
                ChatPart::Tool {
                    page: Some(page), ..
                },
                Some(Shown::Tabs(_, pages)),
            ) => pages.push(page),
            (
                ChatPart::Tool {
                    page: Some(page), ..
                },
                _,
            ) => shown.push(Shown::Tabs(part.id(), vec![page])),
            (
                ChatPart::Tool {
                    page: None,
                    gif: None,
                    app: None,
                    ..
                },
                Some(Shown::Run(calls)),
            ) => calls.push(part),
            (
                ChatPart::Tool {
                    page: None,
                    gif: None,
                    app: None,
                    ..
                },
                _,
            ) => shown.push(Shown::Run(vec![part])),
            (
                ChatPart::AgentNotice {
                    notice:
                        AgentNotice {
                            worker_page: Some(page),
                            ..
                        },
                    ..
                },
                Some(Shown::WorkerPages(_, pages)),
            ) if pages
                .first()
                .is_some_and(|first| first.agent_id == page.agent_id) =>
            {
                pages.push(page)
            }
            (
                ChatPart::AgentNotice {
                    notice:
                        AgentNotice {
                            worker_page: Some(page),
                            ..
                        },
                    ..
                },
                _,
            ) => shown.push(Shown::WorkerPages(part.id(), vec![page])),
            _ => shown.push(Shown::Part(part)),
        }
    }
    shown
}

/// Where a goal began, passed a step or ended, as a milestone: its state in its colour, its
/// name, and at the right its steps then and, past its start, how long it had taken.
fn goal_marker(notice: &AgentNotice, looks: &Drawn, cx: &App) -> AnyElement {
    let Some(goal) = notice.goal.as_ref() else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx);
    let (state, word) = match (goal.moment, goal.state) {
        (GoalMoment::Step, _) => (SliceState::Done, t!("Step done")),
        (GoalMoment::Done, _) => (SliceState::Done, t!("Goal done")),
        (GoalMoment::Stopped, SliceState::Failed) => (SliceState::Failed, t!("Goal stopped")),
        (GoalMoment::Stopped, _) => (SliceState::Waiting, t!("Goal stopped")),
        (GoalMoment::Started, SliceState::NeedsYou) => (SliceState::NeedsYou, t!("Goal needs you")),
        (GoalMoment::Started, SliceState::Waiting) => (SliceState::Waiting, t!("Goal waiting")),
        (GoalMoment::Started, state) => (state, t!("Goal")),
    };
    let name = match goal.name.is_empty() {
        true => notice.title.clone(),
        false => goal.name.clone(),
    };
    div()
        .h(px(36.))
        .my(SPACE_1H)
        .flex()
        .items_center()
        .gap(px(9.))
        .min_w_0()
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .child(crate::ui::slice_state::state_mark(state, cx))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(crate::ui::slice_state::state_ink(state, t))
                .child(word),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(t.text_secondary)
                .child(name),
        )
        .child(
            div()
                .ml_auto()
                .flex_none()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .children(goal.took.filter(|took| *took >= 1000).map(|took| {
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .text_color(looks.faint)
                        .child(crate::ui::time::took(took))
                })),
        )
        .into_any_element()
}

/// The line a finished turn folds behind: who worked, for how long, with how many tool calls.
pub(super) fn work(fold: &WorkFold, open: bool, looks: &Drawn, cx: &App) -> AnyElement {
    let worked = match (fold.thought, fold.took) {
        (true, None) => t!("Thought").to_owned(),
        (false, None) => t!("Worked").to_owned(),
        (true, Some(took)) => t!("Thought for {time}", time = crate::ui::time::took(took)),
        (false, Some(took)) => t!("Worked for {time}", time = crate::ui::time::took(took)),
    };
    let calls = (fold.calls > 0).then(|| match fold.calls {
        1 => t!("· 1 tool call").to_owned(),
        calls => t!("· {calls} tool calls", calls = calls),
    });
    let toggle = fold_toggle(
        &fold.id,
        open,
        vec![
            looks.mark_of(&looks.agent, &looks.provider, 17., cx),
            worked.into_any_element(),
        ]
        .into_iter()
        .chain(calls.map(IntoElement::into_any_element))
        .collect(),
        looks,
    );
    div()
        .mt(px(3.))
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(div().flex().child(toggle))
        .child(div().h(px(1.)).bg(looks.rule))
        .into_any_element()
}

/// A line that folds something away beneath it, opened and closed by the reader under `key`:
/// `says` then a chevron that turns as it opens.
pub(super) fn fold_toggle(
    key: &str,
    open: bool,
    says: Vec<AnyElement>,
    looks: &Drawn,
) -> gpui::Stateful<Div> {
    let (reader, id) = (looks.reader.clone(), key.to_owned());
    let (hover, ink) = (looks.hover, looks.ink);
    div()
        .id(SharedString::from(format!("work:{key}")))
        .h(px(22.))
        .ml(px(-6.))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::radius_2())
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .font_weight(FontWeight::MEDIUM)
        .text_color(looks.quiet)
        .cursor_pointer()
        .hover(move |toggle| toggle.bg(hover).text_color(ink))
        .children(says)
        .child(motion::turn(
            format!("work-chevron:{key}"),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 10.).text_color(looks.quiet),
        ))
        .clicks(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&id);
                cx.notify();
            })
        })
}

/// Where the reader's choice to see what was left out of `row` is kept.
pub(super) fn left_out_key(row: &ChatRow) -> String {
    format!("left-out:{}", row.message.id)
}

/// The quiet line under an answer that says how many of its sentences rested on no source and
/// were left out, with Show to put them back in place, faint, and Hide to take them out again.
fn left_out(row: &ChatRow, looks: &Drawn, cx: &App) -> Option<AnyElement> {
    let count: u64 = (row.message.left_out.iter()).map(|out| out.sentences).sum();
    if count == 0 {
        return None;
    }
    let key = left_out_key(row);
    let open = looks.reading.opened.contains(&key);
    let said = match count {
        1 => t!("1 sentence with no source left out").to_owned(),
        count => t!("{count} sentences with no source left out", count = count),
    };
    let faint = Tokens::get(cx).text_tertiary;
    let (reader, ink) = (looks.reader.clone(), looks.ink);
    let need_fetch = !open && row.restored.is_empty();
    let (bench, agent, message) = (
        looks.bench.clone(),
        looks.agent.clone(),
        row.message.id.clone(),
    );
    let toggle = div()
        .id(SharedString::from(key.clone()))
        .hover(move |toggle| toggle.text_color(ink))
        .child(if open { t!("Hide") } else { t!("Show") })
        .clicks(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&key);
                cx.notify();
            });
            if need_fetch {
                let _ = bench.update(cx, |bench, _| {
                    bench.ask_chat(crate::core::ChatAsk::LeftOut {
                        agent: agent.clone(),
                        message: message.clone(),
                    });
                });
            }
        });
    Some(
        div()
            .flex()
            .items_center()
            .gap(SPACE_1)
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .text_color(faint)
            .child(said)
            .child("·")
            .child(toggle)
            .into_any_element(),
    )
}

/// The card of the subagent running in `session`, as the chat last showed it; a subagent's
/// notice shows its card rather than a notice of its own.
pub(super) fn card_of<'a>(
    rows: &'a [ChatRow],
    index: &RowIndex,
    session: &str,
) -> Option<&'a SubagentCard> {
    let (row_ix, part_ix) = index.subagent_card(session)?;
    match rows.get(row_ix)?.message.parts.get(part_ix)? {
        ChatPart::Subagent { subagent, .. } if subagent.session_id == session => Some(subagent),
        _ => None,
    }
}

/// A background task that ended, drawn as the last row of a tool-call tree: what kind of task
/// it was, its name's first line, and how it ended.
fn task_ended(notice: &TaskNotice, looks: &Drawn, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (icon, word, tint) = match notice.task_type.as_deref() {
        Some("workflow") => (Icon::Workflow, t!("workflow"), looks.tints.called),
        Some("monitor") => (Icon::Timer, t!("watch"), looks.tints.looked),
        _ => (Icon::Run, t!("bg"), looks.tints.ran),
    };
    let (mark, state, state_ink) = match notice.state {
        NoticeState::Completed => (Some((Icon::Check, t.live)), t!("done"), looks.quiet),
        NoticeState::Failed => (Some((Icon::Close, t.danger)), t!("failed"), t.danger),
        NoticeState::Stopped => (None, t!("stopped"), looks.quiet),
    };
    let name = notice
        .name
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let summary = notice
        .summary
        .as_deref()
        .and_then(|summary| summary.lines().next())
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty() && *line != name);
    div()
        .relative()
        .h(px(22.))
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .min_w_0()
        .font_family(theme::mono_font())
        .text_size(crate::theme::ui_text(crate::theme::TEXT_3XS))
        .child(super::tools::spine(true, looks))
        .child(div().w(px(9.)).flex_none())
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .flex()
                .justify_center()
                .child(ui::icon(icon, 11.).text_color(tint)),
        )
        .child(div().flex_none().text_color(tint).child(word))
        .child(div().min_w_0().truncate().text_color(looks.ink).child(name))
        .children(summary.map(|summary| {
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.quiet)
                .child(summary)
        }))
        .children(mark.map(|(icon, ink)| ui::icon(icon, 11.).flex_none().text_color(ink)))
        .child(div().flex_none().text_color(state_ink).child(state))
        .into_any_element()
}

/// The line something said beside the reply sits on: a background task that ended, a notice
/// from the agent, or a move to another account.
/// The quiet line that says a rule was kept from what the person said, with Undo until the person
/// removes it again.
fn kept_rule(notice: &AgentNotice, looks: &Drawn) -> AnyElement {
    let Some(rule) = notice.rule.clone() else {
        return div().into_any_element();
    };
    let head = match (rule.undone, &rule.org) {
        (true, _) => t!("Kept as a rule, then undone").to_owned(),
        (false, Some(org)) if rule.waiting => {
            t!("Kept as a rule, waiting for {org}'s admins", org = org)
        }
        (false, Some(org)) => t!("Kept as a rule for {org}", org = org),
        (false, None) => t!("Kept as a rule").to_owned(),
    };
    let bench = looks.bench.clone();
    notice_box(looks)
        .child(ui::icon(Icon::Book, 12.).text_color(looks.quiet))
        .child(div().flex_none().child(head))
        .when(!rule.undone, |line| {
            line.child("·").child(
                div()
                    .id(SharedString::from(format!("kept-rule-{}", rule.id)))
                    .text_color(looks.ink)
                    .child(t!("Undo"))
                    .clicks(move |_, _, cx| {
                        let rule = rule.clone();
                        let _ = bench
                            .update(cx, |bench, cx| crate::playbook::undo_rule(bench, &rule, cx));
                    }),
            )
        })
        .into_any_element()
}

/// Pages one of the agent's workers showed one after another: a line naming the worker over
/// their rows.
fn worker_pages(
    first: &str,
    pages: &[&WorkerPage],
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let name = pages.first().map_or("", |page| page.name.as_str());
    let said = match pages.len() {
        1 => t!("@{name} showed an artifact", name = name),
        count => t!(
            "@{name} showed {count} artifacts",
            name = name,
            count = count
        ),
    };
    div()
        .flex()
        .flex_col()
        .child(
            notice_box(looks)
                .child(ui::icon(Icon::Presentation, 12.).text_color(looks.quiet))
                .child(notice_name(said.to_string(), false, looks)),
        )
        .child(self::page::worker_pages(
            first,
            pages,
            looks.looks,
            window,
            cx,
        ))
        .into_any_element()
}

/// The quiet line under an agent's reply that says it recorded a decision itself: what it
/// decided, what that rests on, and Overrule. The reason gives way first on a narrow pane.
fn decided(notice: &AgentNotice, looks: &Drawn) -> AnyElement {
    let Some(decided) = notice.decided.clone() else {
        return div().into_any_element();
    };
    let quoted = t!("“{text}”", text = decided.text.clone());
    let (bench, agent) = (looks.bench.clone(), looks.agent.clone());
    let id = format!(
        "decided-{}-{}",
        decided.goal.unwrap_or_default(),
        decided.decision
    );
    notice_box(looks)
        .child(ui::icon(Icon::Check, 12.).text_color(looks.quiet))
        .child(div().flex_none().child(t!("Decided")))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.ink)
                .child(format!("· {quoted}")),
        )
        .children(decided.why.clone().map(|why| {
            div()
                .min_w_0()
                .flex_shrink(1000.)
                .truncate()
                .child(format!("· {why}"))
        }))
        .child(div().flex_none().child("·"))
        .child(
            div()
                .id(SharedString::from(id))
                .flex_none()
                .text_color(looks.ink)
                .child(t!("Overrule"))
                .clicks(move |_, _, cx| {
                    let decided = decided.clone();
                    let _ = bench.update(cx, |bench, cx| {
                        let target = bench.chat_target(&agent);
                        let decision = (decided.project, decided.goal, decided.decision);
                        crate::playbook::overrule(bench, &target, decision, cx)
                    });
                }),
        )
        .into_any_element()
}

/// The quiet line an Overrule leaves in the chief's chat, with Undo until it is undone.
fn overruled(notice: &AgentNotice, looks: &Drawn) -> AnyElement {
    let Some(overruled) = notice.overruled.as_deref().cloned() else {
        return div().into_any_element();
    };
    let (head, tail) = match (overruled.undone, overruled.rule.is_some(), &overruled.org) {
        (true, ..) if overruled.still_kept => (
            t!("Overruled, then undone"),
            Some(
                t!("it stands again, but decisions like this still come to you: remove the rule in the Playbook")
                    .to_owned(),
            ),
        ),
        (true, ..) => (
            t!("Overruled, then undone"),
            Some(t!("it stands again").to_owned()),
        ),
        (false, true, Some(org)) if overruled.waiting => (
            t!("Overruled"),
            Some(t!(
                "decisions like this come to you from now on, a rule waiting for {org}'s admins",
                org = org
            )),
        ),
        (false, true, Some(org)) => (
            t!("Overruled"),
            Some(t!(
                "decisions like this come to you from now on, a rule for {org}",
                org = org
            )),
        ),
        (false, true, None) => (
            t!("Overruled"),
            Some(t!("decisions like this come to you from now on").to_owned()),
        ),
        (false, false, _) => (t!("Overruled"), None),
    };
    let quoted =
        (!overruled.text.is_empty()).then(|| t!("“{text}”", text = overruled.text.clone()));
    let offers_undo = !overruled.undone && !overruled.stopped_auto;
    let (bench, agent) = (looks.bench.clone(), looks.agent.clone());
    let id = format!(
        "overruled-{}-{}",
        overruled.goal.unwrap_or_default(),
        overruled.decision
    );
    notice_box(looks)
        .child(ui::icon(Icon::Reply, 12.).text_color(looks.quiet))
        .child(div().flex_none().child(head))
        .children(quoted.map(|quoted| {
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.ink)
                .child(format!("· {quoted}"))
        }))
        .children(tail.map(|tail| div().flex_none().child(format!("· {tail}"))))
        .when(offers_undo, |line| {
            line.child("·").child(
                div()
                    .id(SharedString::from(id))
                    .flex_none()
                    .text_color(looks.ink)
                    .child(t!("Undo"))
                    .clicks(move |_, _, cx| {
                        let overruled = overruled.clone();
                        let _ = bench.update(cx, |bench, cx| {
                            let target = bench.chat_target(&agent);
                            crate::playbook::undo_overrule(bench, &target, overruled, cx)
                        });
                    }),
            )
        })
        .into_any_element()
}

/// The quiet line the board leaves in the chief's chat when it closed a card by itself, with Undo
/// until the person reopens it.
fn closed_card(notice: &AgentNotice, looks: &Drawn) -> AnyElement {
    let Some(closed) = notice.closed_card.as_deref().cloned() else {
        return div().into_any_element();
    };
    let head = match closed.reopened {
        true => t!("Closed by itself, then reopened"),
        false => t!("Closed by itself"),
    };
    let quoted = t!(
        "“{text}”",
        text = format!("#{} {}", closed.card, closed.title)
    );
    let passed = t!(
        "the check passed on {commit}",
        commit = closed.commit.chars().take(7).collect::<String>()
    );
    let (bench, agent) = (looks.bench.clone(), looks.agent.clone());
    notice_box(looks)
        .child(ui::icon(Icon::Check, 12.).text_color(looks.quiet))
        .child(div().flex_none().child(head))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.ink)
                .child(format!("· {quoted}")),
        )
        .child(div().flex_none().child(format!("· {passed}")))
        .when(!closed.reopened, |line| {
            line.child("·").child(
                div()
                    .id(SharedString::from(format!("closed-card-{}", closed.card)))
                    .flex_none()
                    .text_color(looks.ink)
                    .child(t!("Undo"))
                    .clicks(move |_, _, cx| {
                        let closed = closed.clone();
                        let _ = bench.update(cx, |bench, cx| {
                            let target = bench.chat_target(&agent);
                            crate::playbook::reopen_card(bench, &target, closed, cx)
                        });
                    }),
            )
        })
        .into_any_element()
}

/// The quiet line a rule leaves when a check in the repository holds it now, with Undo until the
/// person brings it back.
fn rule_change(notice: &AgentNotice, looks: &Drawn) -> AnyElement {
    let Some(line) = notice.rule_change.clone() else {
        return div().into_any_element();
    };
    let moved_to = || match line.store.is_project() {
        true => t!(
            "to {project}",
            project = crate::ui::path::file_name(std::path::Path::new(&line.project))
        ),
        false => t!("to yours").to_owned(),
    };
    let (head, tail) = match (line.restored, &line.commit, &line.was) {
        (true, _, _) if line.moved => (t!("Moved back"), None),
        (false, _, _) if line.moved => (t!("Rule moved"), Some(moved_to())),
        (true, _, _) => (
            t!("Rule back"),
            (line.back_by.clone()).map(|name| t!("by @{name}", name = name)),
        ),
        (false, Some(commit), _) => (
            t!("Now a check"),
            Some(commit.chars().take(7).collect::<String>()),
        ),
        (false, None, Some(_)) => (
            t!("Rule changed"),
            Some(t!("by @{name}", name = line.by.clone())),
        ),
        (false, None, None) => (
            t!("Rule removed"),
            Some(t!("by @{name}", name = line.by.clone())),
        ),
    };
    let quoted = t!("“{text}”", text = line.text);
    let (bench, agent) = (looks.bench.clone(), looks.agent.clone());
    notice_box(looks)
        .child(ui::icon(Icon::Check, 12.).text_color(looks.quiet))
        .child(div().flex_none().child(head))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.ink)
                .child(format!("· {quoted}")),
        )
        .children(tail.map(|tail| div().flex_none().child(format!("· {tail}"))))
        .when(!line.restored, |row| {
            row.child("·").child(
                div()
                    .id(SharedString::from(format!("rule-change-{}", line.rule)))
                    .flex_none()
                    .text_color(looks.ink)
                    .child(t!("Undo"))
                    .clicks(move |_, _, cx| {
                        let line = line.clone();
                        let _ = bench.update(cx, |bench, cx| {
                            let target = bench.chat_target(&agent);
                            crate::playbook::restore_rule(bench, &target, &line, cx)
                        });
                    }),
            )
        })
        .into_any_element()
}

/// The chief's count of last week, worded here from the core's numbers, under the time it came.
fn week_count(week: &ChiefWeek, looks: &Drawn) -> AnyElement {
    use chrono::{Datelike, TimeZone};
    let came = (chrono::Local.timestamp_millis_opt(week.to as i64).single()).map(|at| {
        let day = match at.weekday() {
            chrono::Weekday::Mon => t!("Monday"),
            chrono::Weekday::Tue => t!("Tuesday"),
            chrono::Weekday::Wed => t!("Wednesday"),
            chrono::Weekday::Thu => t!("Thursday"),
            chrono::Weekday::Fri => t!("Friday"),
            chrono::Weekday::Sat => t!("Saturday"),
            chrono::Weekday::Sun => t!("Sunday"),
        };
        t!(
            "{day} · {time}",
            day = day,
            time = crate::ui::time::time_of_day(at)
        )
    });
    let decided = week.decided;
    let first = match (decided, week.overruled) {
        (1, 0) => t!("Last week I decided 1 thing, and you overruled none.").to_owned(),
        (1, overruled) => t!(
            "Last week I decided 1 thing, and you overruled {overruled}.",
            overruled = overruled
        ),
        (_, 0) => t!(
            "Last week I decided {decided} things, and you overruled none.",
            decided = decided
        ),
        (_, overruled) => t!(
            "Last week I decided {decided} things, and you overruled {overruled}.",
            decided = decided,
            overruled = overruled
        ),
    };
    let second = match week.answered {
        0 => None,
        1 => Some(t!(
            "You answered {picked} of 1 ask as I picked.",
            picked = week.as_picked
        )),
        answered => Some(t!(
            "You answered {picked} of {answered} asks as I picked.",
            picked = week.as_picked,
            answered = answered
        )),
    };
    let words = match second {
        Some(second) => t!("{first} {second}", first = first, second = second),
        None => first,
    };
    let text = looks.styles.borrow().text.clone();
    div()
        .py(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .text_color(looks.faint)
                .children(came),
        )
        .child(
            div()
                .text_size(text.text_size)
                .line_height(gpui_kit::relative(text.line_height))
                .text_color(looks.ink)
                .child(words),
        )
        .into_any_element()
}

pub(super) fn notice_box(looks: &Drawn) -> Div {
    div()
        .min_h(px(26.))
        .pl(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .min_w_0()
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .text_color(looks.quiet)
}

/// What the notice is about, giving way before its state does.
pub(super) fn notice_name(name: String, mono: bool, looks: &Drawn) -> Div {
    div()
        .min_w_0()
        .truncate()
        .when(mono, |name| name.font_family(theme::mono_font()))
        .text_color(looks.ink)
        .child(name)
}

pub(super) fn notice_summary(summary: String) -> Div {
    div().min_w_0().truncate().child(summary)
}

/// A subagent the agent started: its name, its task and how it stands, opening onto
/// what it did. A closed card draws none of its rows, though they keep arriving.
pub(super) fn subagent(
    card: &SubagentCard,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = format!("subagent:{}", card.session_id);
    let open = looks.reading.opened.contains(&key);
    let chevron = motion::turn(
        format!("{key}:chevron"),
        motion::fold_angle(open),
        motion::DETAIL_FOLD,
        ui::icon(Icon::Chevron, 12.).text_color(t.text_tertiary),
    );
    let (reader, toggled) = (looks.reader.clone(), key.clone());
    let summary = div()
        .id(SharedString::from(key.clone()))
        .clicks(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&toggled);
                cx.notify();
            })
        })
        .child(super::live::subagent_line(
            card,
            &looks.provider,
            Some(chevron.into_any_element()),
            &t,
            cx,
        ));
    let body = open.then(|| {
        let rows = looks
            .reading
            .subagents
            .get(&card.session_id)
            .map_or(&[][..], Vec::as_slice);
        let live = card.state == SubagentState::Running;
        let mut drawn = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            let groups = runs(row.message.parts.iter(), false);
            let count = groups.len();
            for (at, group) in groups.into_iter().enumerate() {
                drawn.push(match group {
                    Shown::Part(part) => self::part(row, part, false, pen, looks, window, cx),
                    Shown::Sent(part, call) => {
                        sent_letter(row, part.id(), call, pen, looks, window, cx)
                    }
                    Shown::Tabs(first, pages) => self::page::tabs(first, &pages, looks, window, cx),
                    Shown::WorkerPages(first, pages) => {
                        worker_pages(first, &pages, looks, window, cx)
                    }
                    Shown::Run(calls) => tools::run(
                        &calls,
                        live && ix + 1 == rows.len() && at + 1 == count,
                        looks,
                        window,
                        cx,
                    ),
                });
            }
        }
        if drawn.is_empty() {
            drawn.push(
                div()
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(looks.faint)
                    .child(t!("No output yet."))
                    .into_any_element(),
            );
        }
        div()
            .id(SharedString::from(format!("{key}:body")))
            .mx(SPACE_3)
            .max_h(px(420.))
            .overflow_y_scroll()
            .pt(SPACE_1)
            .pb(SPACE_2)
            .pl(px(13.))
            .pr(SPACE_2H)
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .children(drawn)
    });
    div()
        .flex()
        .flex_col()
        .p(SPACE_1)
        .rounded(theme::radius_3())
        .bg(t.surface_raised)
        .child(summary)
        .children(body)
        .into_any_element()
}

/// `look` with a reply button beside each paragraph and list item of the text `text`, which
/// puts that part in the draft's reply.
fn with_reply(
    look: Rc<Style>,
    row: &ChatRow,
    text: &SharedString,
    pen: &Pen,
    looks: &Drawn,
    cx: &App,
) -> Rc<Style> {
    let gutter = reply_gutter(pen.width(), cx);
    let message = row.message.id.clone();
    let tagged = format!("{REPLY_URI}{message}#{text}.");
    let chosen: Rc<[usize]> = (looks.bench.upgrade())
        .and_then(|bench| {
            let composing = bench.read(cx).composing(&looks.agent)?;
            let places = (composing.draft.read(cx).tokens().iter())
                .filter_map(|span| span.token().id().strip_prefix(&tagged)?.parse().ok())
                .collect();
            Some(places)
        })
        .unwrap_or_else(|| Rc::from([]));
    let hovered = (looks.reading.quoting_button.as_ref())
        .or(looks.reading.quoting.as_ref())
        .filter(|(over, _)| over == text)
        .map(|(_, place)| *place);
    let pointed = (looks.reading.quoting_button.as_ref()).is_some_and(|(over, _)| over == text);
    let on_hover = {
        let (reader, text) = (looks.reader.clone(), text.clone());
        Rc::new(
            move |place: usize, over: bool, _: &mut Window, cx: &mut App| {
                reader.update(cx, |reading, cx| {
                    let here = Some((text.clone(), place));
                    if over {
                        if reading.quoting == here {
                            return;
                        }
                        reading.quoting = here;
                        if reading.quoting_button != reading.quoting {
                            reading.quoting_button = None;
                        }
                    } else if reading.quoting == here {
                        reading.quoting = None;
                    } else {
                        return;
                    }
                    cx.notify();
                })
            },
        )
    };
    let on_point = {
        let (reader, text) = (looks.reader.clone(), text.clone());
        Rc::new(
            move |place: usize, over: bool, _: &mut Window, cx: &mut App| {
                reader.update(cx, |reading, cx| {
                    let here = Some((text.clone(), place));
                    if over {
                        if reading.quoting_button == here {
                            return;
                        }
                        reading.quoting_button = here;
                    } else if reading.quoting_button == here {
                        reading.quoting_button = None;
                    } else {
                        return;
                    }
                    cx.notify();
                })
            },
        )
    };
    let button = {
        let (bench, agent, reader, text) = (
            looks.bench.clone(),
            looks.agent.clone(),
            looks.reader.clone(),
            text.clone(),
        );
        let accent = looks.accent;
        let ground = (gutter < px(REPLY_BUTTON)).then(|| Tokens::get(cx).surface_raised);
        Rc::new(
            move |place: usize, quote: String, chosen: bool, _: &mut Window, _: &mut App| {
                let (bench, agent, reader, message) = (
                    bench.clone(),
                    agent.clone(),
                    reader.clone(),
                    message.clone(),
                );
                let part = format!("{text}.{place}");
                let label = match chosen {
                    true => t!("Take out of the reply"),
                    false => t!("Reply to this part"),
                };
                let button = IconButton::new(
                    SharedString::from(format!("reply:{part}")),
                    Icon::Reply,
                    label,
                )
                .glyph(13.)
                .width(REPLY_BUTTON)
                .pressed(chosen)
                .when(chosen, |button| button.tint(accent))
                .on_click(move |_, window, cx| {
                    bench
                        .update(cx, |bench, cx| {
                            bench.reply_to(&agent, &message, Some(&part), &quote, window, cx)
                        })
                        .ok();
                    reader.update(cx, |_, cx| cx.notify());
                });
                match ground {
                    Some(ground) => div()
                        .rounded(theme::radius_2())
                        .bg(ground)
                        .child(button)
                        .into_any_element(),
                    None => button.into_any_element(),
                }
            },
        )
    };
    Rc::new(Style {
        reply: Some(gpui_transcript::Reply {
            hovered,
            pointed,
            chosen,
            gutter,
            on_hover,
            on_point,
            button,
        }),
        ..(*look).clone()
    })
}

/// The room right of an answer's text that holds a paragraph's reply button.
const REPLY_GUTTER: f32 = 32.;
/// The room at each side of a row.
const ROW_SIDE: f32 = 22.;
const REPLY_BUTTON: f32 = 24.;

/// The reply button's room in a pane `width` wide, short of the scrollbar at its edge. Where the
/// button doesn't fit beside the text, it lies over the line's end.
fn reply_gutter(width: Option<Pixels>, cx: &App) -> Pixels {
    let Some(width) = width else {
        return px(REPLY_GUTTER);
    };
    let beside = (width - super::column::rows(cx)).max(px(0.)) / 2. + px(ROW_SIDE);
    (beside - Scrollbar::width()).clamp(px(0.), px(REPLY_GUTTER))
}

pub(super) fn part(
    row: &ChatRow,
    part: &ChatPart,
    user: bool,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = SharedString::from(format!("{}:{}", row.message.id, part.id()));
    let line = |icon: Icon, text: String, tint: Hsla| {
        div()
            .flex()
            .items_center()
            .gap(px(7.))
            .min_w_0()
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .text_color(tint)
            .child(ui::icon(icon, 12.).text_color(tint))
            .child(div().min_w_0().truncate().child(text))
            .into_any_element()
    };
    match part {
        ChatPart::Text { .. } | ChatPart::Thought { .. } => {
            let shown = !user && looks.reading.opened.contains(&left_out_key(row));
            row.left_out_shown.set(shown);
            let restored = shown.then(|| row.restored(part.id())).flatten();
            let id = match restored {
                Some(_) => SharedString::from(format!("{id}:left-out")),
                None => id,
            };
            let Some(doc) = restored.or_else(|| row.doc(part.id())) else {
                return div().into_any_element();
            };
            let styles = looks.styles.borrow();
            let look = match (part, user) {
                (ChatPart::Thought { .. }, _) => styles.thought.clone(),
                (_, true) => styles.user.clone(),
                _ => styles.text.clone(),
            };
            drop(styles);
            let look = if user {
                attach::with_chips(
                    look,
                    &row.message,
                    &looks.agent,
                    looks.file_pressed(),
                    looks.bubble,
                    &looks.bench,
                    window,
                )
            } else {
                attach::with_why(look, &row.message, &looks.agent, &looks.bench, window, cx)
            };
            let look = match part {
                ChatPart::Text { .. } if !user => with_reply(look, row, &id, pen, looks, cx),
                _ => look,
            };
            let cut = (doc.units > LONG_TEXT && looks.reading.folds(&row.message.id, &id))
                .then(|| doc.markdown.cut(LONG_TEXT))
                .flatten();
            let Some((blocks, hidden)) = cut else {
                return pen.markdown_with(&look, id, &doc.markdown, window, cx);
            };
            let reader = looks.reader.clone();
            let label = t!(
                "Show the rest · {thousands}k more characters",
                thousands = (hidden + 500) / 1000
            );
            let more = Button::new(
                SharedString::from(format!("rest:{id}")),
                label,
                Variant::Outline,
            )
            .on_click({
                let id = id.clone();
                move |_, _, cx| {
                    reader.update(cx, |reading, cx| {
                        reading.unfolded.insert(id.clone());
                        cx.notify();
                    })
                }
            });
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .min_w_0()
                .child(pen.blocks_with(&look, id, &doc.markdown, blocks, window, cx))
                .child(div().flex().child(more))
                .into_any_element()
        }
        ChatPart::Tool {
            app: Some(app),
            tool,
            ..
        } => self::app::app(app, &tool.title, looks.looks, &looks.reading.running, cx),
        ChatPart::Tool {
            id, gif: Some(gif), ..
        } => gestures::gif(id, gif, looks, cx),
        ChatPart::Tool { tool, failure, .. } => {
            let tint = if failure.is_some() {
                Tokens::get(cx).danger
            } else {
                looks.quiet
            };
            line(Icon::Run, tool.title.clone(), tint)
        }
        ChatPart::Subagent { subagent, .. } => self::subagent(subagent, pen, looks, window, cx),
        ChatPart::Notice { notice, .. } => task_ended(notice, looks, cx),
        ChatPart::Tasks { id: part, tasks } => {
            super::ended::tasks(&row.message.id, part, tasks, looks, window, cx)
        }
        ChatPart::AgentNotice { notice, .. } if notice.goal.is_some() => {
            goal_marker(notice, looks, cx)
        }
        ChatPart::AgentNotice { notice, .. } if notice.rule.is_some() => kept_rule(notice, looks),
        ChatPart::AgentNotice { notice, .. } if notice.overruled.is_some() => {
            overruled(notice, looks)
        }
        ChatPart::AgentNotice { notice, .. } if notice.decided.is_some() => decided(notice, looks),
        ChatPart::AgentNotice { notice, .. } if notice.rule_change.is_some() => {
            rule_change(notice, looks)
        }
        ChatPart::AgentNotice {
            notice:
                AgentNotice {
                    sign_in: Some(card),
                    ..
                },
            ..
        } => super::sign_in_card::card(card, looks, cx),
        ChatPart::AgentNotice { notice, .. } if notice.closed_card.is_some() => {
            closed_card(notice, looks)
        }
        ChatPart::AgentNotice {
            notice: AgentNotice {
                week: Some(week), ..
            },
            ..
        } => week_count(week, looks),
        ChatPart::AgentNotice { notice, .. } => {
            let t = Tokens::get(cx);
            let (icon, tint, state) = match notice.severity {
                NoticeSeverity::Info => (Icon::Inbox, looks.quiet, looks.quiet),
                NoticeSeverity::Warning => (Icon::Warning, t.warn, looks.quiet),
                NoticeSeverity::Error => (Icon::Warning, t.danger, t.danger),
            };
            notice_box(looks)
                .child(ui::icon(icon, 12.).text_color(tint))
                .child(notice_summary(notice.title.clone()).text_color(state))
                .children(notice.description.clone().map(notice_summary))
                .into_any_element()
        }
        ChatPart::Account { moved, .. } => {
            let why = moved
                .from
                .as_ref()
                .filter(|_| moved.reason == MoveReason::Limit)
                .map(|from| t!("{from} reached its usage limit", from = from));
            let face = looks
                .bench
                .upgrade()
                .and_then(|bench| {
                    let bench = bench.read(cx);
                    let profile = crate::accounts::named(bench, &looks.provider, &moved.label)?;
                    let picture = bench.accounts.avatar(profile);
                    Some(
                        crate::accounts::face(picture, &looks.provider, 14., cx).into_any_element(),
                    )
                })
                .unwrap_or_else(|| {
                    ui::icon(Icon::User, 12.)
                        .text_color(looks.quiet)
                        .into_any_element()
                });
            notice_box(looks)
                .child(face)
                .child(div().flex_none().child(t!("Moved to")))
                .child(notice_name(moved.label.clone(), false, looks))
                .children(why.map(notice_summary))
                .into_any_element()
        }
        ChatPart::Content { content, .. } => content::content(id, content, looks, cx),
        ChatPart::Rotation { rotation, .. } => super::rotation::line(rotation, looks, cx),
        ChatPart::ContextHandoff { handoff, .. } => super::handoff::line(handoff, looks, cx),
    }
}

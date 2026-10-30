//! Drawing a chat's rows: a message and its parts, the work folded under a turn, letters and
//! fired triggers, and subagents.

use super::*;

use crate::theme::{
    SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, TEXT_2XS, TEXT_2XS_H,
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
    let fold = message.fold.as_ref();
    let folded = fold.is_some_and(|fold| !looks.reading.opened.contains(&fold.id));
    let opens = fold.filter(|fold| {
        ix == 0
            || rows[ix - 1]
                .message
                .fold
                .as_ref()
                .is_none_or(|before| before.id != fold.id)
    });
    let from = match fold {
        Some(fold) if fold.last != message.id => usize::MAX,
        Some(fold) => usize::try_from(fold.from).unwrap_or(0),
        None => 0,
    };
    let work_shown = if user {
        1.
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
        let reply = matches!(
            part,
            ChatPart::Text { .. } | ChatPart::Content { .. } | ChatPart::Tool { gif: Some(_), .. }
        );
        let tucked = !(from == 0 || outlives(part) || index >= from && reply);
        if tucked && work_shown <= 0. {
            continue;
        }
        match segments.last_mut() {
            Some((was, parts)) if *was == tucked => parts.push(part),
            _ => segments.push((tucked, vec![part])),
        }
    }
    let live = ix + 1 == rows.len() && looks.reading.running;
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
        let groups = runs(shown.into_iter());
        let count = groups.len();
        let live = live && at_segment == last_segment;
        let mut drawn = Vec::with_capacity(count);
        for (at, group) in groups.into_iter().enumerate() {
            drawn.push(match group {
                Shown::Part(ChatPart::Notice { notice, .. })
                    if let Some(card) = notice
                        .subagent
                        .as_deref()
                        .and_then(|session| card_of(rows, session)) =>
                {
                    self::subagent(card, pen, looks, window, cx)
                }
                Shown::Part(part) => self::part(row, part, user, pen, looks, window, cx),
                Shown::Run(calls) => tools::run(&calls, live && at + 1 == count, looks, window, cx),
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
                    "Earlier messages are no longer kept.",
                    TEXT_XS,
                    FontWeight::NORMAL,
                )
                .text_color(Tokens::get(cx).text_tertiary),
            )
    });
    let worktree = (ix == 0)
        .then_some(looks.reading.worktree.as_ref())
        .flatten()
        .and_then(|switch| crate::worktree_switch::note(switch, cx))
        .map(|note| {
            div()
                .w_full()
                .flex()
                .justify_center()
                .when(!looks.reading.earlier_lost, |row| row.pt(SPACE_3))
                .child(note)
        });
    let noted = worktree.is_some();
    if parts.is_empty() && message.letters.is_empty() && summary.is_none() && !carried {
        return div().children(lost).children(worktree).into_any_element();
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
                    .children(attach::sent(message, open, looks.file_pressed(), cx))
                    .when(!parts.is_empty(), |column| {
                        column.child(
                            div()
                                .max_w_full()
                                .px(SPACE_3)
                                .py(SPACE_2)
                                .border_1()
                                .border_color(looks.line)
                                .rounded(theme::RADIUS_3)
                                .bg(looks.bubble)
                                .children(parts),
                        )
                    })
                    .children(gestures::reactions(message, looks))
                    .children(meta::line(rows, ix, &group, looks, cx))
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
                    .children(parts),
            )
            .children(meta::line(rows, ix, &group, looks, cx))
    };
    let user = user && (!lettered || triggered);
    let drawn = div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .children(lost)
        .children(worktree)
        .child(
            div()
                .w_full()
                .max_w(px(800.))
                .px(px(22.))
                .map(|row| {
                    if user {
                        row.pt(px(17.)).pb(SPACE_2H).flex().justify_end()
                    } else {
                        row.py(px(5.))
                    }
                })
                .when(ix == 0 && !looks.reading.earlier_lost && !noted, |row| {
                    row.pt(SPACE_3)
                })
                .child(content),
        );
    if !whole_row_tucked {
        return drawn.into_any_element();
    }
    let key = SharedString::from(format!("work-row:{}", message.id));
    match folded {
        true => motion::folding_away(key, work_shown, drawn, window, cx),
        false => motion::folded(key, work_shown, drawn, window, cx),
    }
}

/// How far below its place the way back to the latest message starts as it comes up.
pub(super) const LATEST_RISE: f32 = 6.;

/// The gap between a message's parts.
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

/// The prompts of the chat's triggers that came due, drawn where the person's own messages
/// sit: a line naming the trigger, its prompt in a bubble, and the webhook request that fired
/// it folded beneath.
pub(super) fn fired(
    row: &ChatRow,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::Div {
    let t = Tokens::get(cx).clone();
    let drawn: Vec<AnyElement> = row
        .message
        .letters
        .iter()
        .enumerate()
        .filter_map(|(ix, letter)| Some((ix, letter.trigger.as_ref()?)))
        .map(|(ix, trigger)| {
            let id = SharedString::from(format!("{}:{}", row.message.id, letter_doc(ix)));
            let prompt = row.doc(&letter_doc(ix)).map(|doc| {
                let look = looks.styles.borrow().text.clone();
                pen.markdown_with(&look, id.clone(), &doc.markdown, window, cx)
            });
            let named = div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .text_color(looks.quiet)
                .child(ui::icon(Icon::Timer, 11.).text_color(t.cmd))
                .child("Trigger")
                .child("·")
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(looks.ink)
                        .child(trigger.title.clone()),
                );
            let bubble = div()
                .max_w_full()
                .px(SPACE_3)
                .py(SPACE_2)
                .border_1()
                .border_color(looks.line)
                .rounded(theme::RADIUS_3)
                .bg(looks.bubble)
                .children(prompt);
            div()
                .max_w_full()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(5.))
                .child(named)
                .child(bubble)
                .children(trigger.request.as_ref().map(|request| {
                    let body = row.doc(&request_doc(ix)).map(|doc| {
                        let look = looks.styles.borrow().text.clone();
                        let body_id = SharedString::from(format!("{id}:request"));
                        pen.markdown_with(&look, body_id, &doc.markdown, window, cx)
                    });
                    fired_request(&format!("request:{id}"), request, body, looks, &t)
                }))
                .into_any_element()
        })
        .collect();
    div()
        .max_w(relative(0.8))
        .flex()
        .flex_col()
        .items_end()
        .gap(SPACE_3H)
        .children(drawn)
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
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(move |line| line.bg(hover))
        .on_click(move |_, _, cx| {
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
                .font_family(theme::MONO)
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
            .border_1()
            .border_color(looks.line)
            .rounded(theme::RADIUS_2)
            .bg(t.surface_raised)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .children(headers),
            )
            .children(body)
    });
    div()
        .max_w_full()
        .flex()
        .flex_col()
        .items_end()
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
    let drawn = row
        .message
        .letters
        .iter()
        .enumerate()
        .map(|(ix, letter)| match letter.kind {
            LetterKind::Answer => answered_question(
                (letter.agent_id != looks.agent)
                    .then_some((letter.cli.as_str(), letter.name.as_str())),
                &letter.question,
                &letter.text,
                letter.secret,
                Tokens::get(cx),
            )
            .into_any_element(),
            LetterKind::Message | LetterKind::Trigger => {
                let (bench, agent) = (looks.bench.clone(), letter.agent_id.clone());
                let name = div()
                    .id(SharedString::from(format!(
                        "{}:{}:from",
                        row.message.id,
                        letter_doc(ix)
                    )))
                    .text_color(looks.accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|name| name.underline())
                    .child(letter.name.clone())
                    .on_click(move |_, _, cx| {
                        bench.update(cx, |bench, _| bench.open_agent(&agent)).ok();
                    });
                let from = div()
                    .mb(SPACE_2)
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .child(
                        div()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .text_size(crate::theme::ui_text(TEXT_XS_H))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(looks.quiet)
                            .child(looks.mark_of(&letter.agent_id, &letter.cli, 17., cx))
                            .child(div().flex().gap(SPACE_1).child("Message from").child(name)),
                    )
                    .child(div().h(px(1.)).bg(looks.rule));
                let id = SharedString::from(format!("{}:{}", row.message.id, letter_doc(ix)));
                let text = row.doc(&letter_doc(ix)).map(|doc| {
                    let look = looks.styles.borrow().text.clone();
                    pen.markdown_with(&look, id, &doc.markdown, window, cx)
                });
                div()
                    .flex()
                    .flex_col()
                    .child(from)
                    .children(text)
                    .into_any_element()
            }
        });
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACE_3H)
        .min_w_0()
        .children(drawn)
        .children(gestures::reactions(&row.message, looks))
}

pub(super) enum Shown<'a> {
    Part(&'a ChatPart),
    Run(Vec<&'a ChatPart>),
}

/// A message's parts with each run of tool calls together; a page the agent showed stands on its own.
pub(super) fn runs<'a>(parts: impl Iterator<Item = &'a ChatPart>) -> Vec<Shown<'a>> {
    let mut shown = Vec::new();
    for part in parts {
        match (part, shown.last_mut()) {
            (
                ChatPart::Tool {
                    page: None,
                    gif: None,
                    ..
                },
                Some(Shown::Run(calls)),
            ) => calls.push(part),
            (
                ChatPart::Tool {
                    page: None,
                    gif: None,
                    ..
                },
                _,
            ) => shown.push(Shown::Run(vec![part])),
            _ => shown.push(Shown::Part(part)),
        }
    }
    shown
}

/// A page the agent showed, a GIF it sent and a subagent still at work stay in view when the
/// turn around them folds.
pub(super) fn outlives(part: &ChatPart) -> bool {
    match part {
        ChatPart::Tool { page, gif, .. } => page.is_some() || gif.is_some(),
        ChatPart::Subagent { subagent, .. } => subagent.state == SubagentState::Running,
        _ => false,
    }
}

/// The line a finished turn folds behind: who worked, for how long, with how many tool calls.
pub(super) fn work(fold: &WorkFold, open: bool, looks: &Drawn, cx: &App) -> AnyElement {
    let verb = if fold.thought { "Thought" } else { "Worked" };
    let worked = fold.took.map_or_else(
        || verb.to_owned(),
        |took| format!("{verb} for {}", crate::ui::time::took(took)),
    );
    let calls = (fold.calls > 0).then(|| {
        format!(
            "· {} tool {}",
            fold.calls,
            if fold.calls == 1 { "call" } else { "calls" }
        )
    });
    let (reader, id) = (looks.reader.clone(), fold.id.clone());
    let (hover, ink) = (looks.hover, looks.ink);
    let toggle = div()
        .id(SharedString::from(format!("work:{}", fold.id)))
        .h(px(22.))
        .ml(px(-6.))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .font_weight(FontWeight::MEDIUM)
        .text_color(looks.quiet)
        .cursor_pointer()
        .hover(move |toggle| toggle.bg(hover).text_color(ink))
        .child(looks.mark_of(&looks.agent, &looks.provider, 17., cx))
        .child(worked)
        .children(calls)
        .child(motion::turn(
            format!("work-chevron:{}", fold.id),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 10.).text_color(looks.quiet),
        ))
        .on_click(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&id);
                cx.notify();
            })
        });
    div()
        .mt(px(3.))
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(div().flex().child(toggle))
        .child(div().h(px(1.)).bg(looks.rule))
        .into_any_element()
}

/// The card of the subagent running in `session`, as the chat last showed it; a subagent's
/// notice shows its card rather than a notice of its own.
pub(super) fn card_of<'a>(rows: &'a [ChatRow], session: &str) -> Option<&'a SubagentCard> {
    rows.iter().rev().find_map(|row| {
        row.message.parts.iter().find_map(|part| match part {
            ChatPart::Subagent { subagent, .. } if subagent.session_id == session => Some(subagent),
            _ => None,
        })
    })
}

/// The box something said beside the reply sits in: a background task that ended, a notice
/// from the agent, or a move to another account.
pub(super) fn notice_box(looks: &Drawn) -> Div {
    div()
        .my(SPACE_1)
        .px(SPACE_2H)
        .py(SPACE_1)
        .flex()
        .items_center()
        .gap(px(7.))
        .min_w_0()
        .border_1()
        .border_color(looks.tints.line_soft)
        .rounded(theme::RADIUS_2)
        .bg(looks.tints.composer)
        .text_size(crate::theme::ui_text(TEXT_3XS_H))
        .text_color(looks.quiet)
}

/// What the notice is about, giving way before its state does.
pub(super) fn notice_name(name: String, mono: bool, looks: &Drawn) -> Div {
    div()
        .min_w_0()
        .truncate()
        .when(mono, |name| name.font_family(theme::MONO))
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
    let (tint, mark, word) = match card.state {
        SubagentState::Running => (
            theme::mix(t.accent, t.text_disabled, 0.62),
            Icon::Hourglass,
            "working",
        ),
        SubagentState::Completed => (
            theme::mix(t.live, t.text_disabled, 0.58),
            Icon::Check,
            "done",
        ),
        SubagentState::Failed => (t.danger, Icon::Warning, "failed"),
        SubagentState::Disconnected => (t.danger, Icon::Plug, "lost"),
        SubagentState::Cancelled => (t.text_disabled, Icon::Close, "stopped"),
    };
    let tokens = card
        .tokens
        .map(|tokens| format!("{} tokens", crate::home::compact(tokens as f64)));
    let calls = (card.calls > 0).then(|| {
        format!(
            "{} {}",
            card.calls,
            if card.calls == 1 { "call" } else { "calls" }
        )
    });
    let cost = [tokens, calls]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let task = card
        .task
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let (reader, toggled, hover) = (
        looks.reader.clone(),
        key.clone(),
        theme::mix(t.text_primary, t.surface_raised, 0.05),
    );
    let summary = div()
        .id(SharedString::from(key.clone()))
        .min_h(px(24.))
        .py(SPACE_1)
        .pl(px(11.))
        .pr(SPACE_3)
        .flex()
        .items_center()
        .gap(px(7.))
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_2)
        .when(open, |summary| summary.rounded_b_none())
        .bg(t.surface_raised)
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .cursor_pointer()
        .hover(move |summary| summary.bg(hover))
        .on_click(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&toggled);
                cx.notify();
            })
        })
        .child(
            div()
                .flex_none()
                .opacity(if open { 1. } else { 0.5 })
                .child(motion::turn(
                    format!("{key}:chevron"),
                    motion::fold_angle(open),
                    motion::DETAIL_FOLD,
                    ui::icon(Icon::Chevron, 9.).text_color(looks.quiet),
                )),
        )
        .child(div().flex_none().child(ui::mark(&looks.provider, 18.)))
        .child(
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                .text_color(looks.ink)
                .child(card.name.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                .text_color(looks.faint)
                .map(|line| match card.state {
                    SubagentState::Running => line.child(shimmer(format!("{key}:task"), task)),
                    _ => line.child(task),
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(7.))
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(9.5))
                .text_color(Hsla {
                    a: 0.8,
                    ..looks.faint
                })
                .when(!cost.is_empty(), |end| end.child(cost))
                .child(
                    div()
                        .id(SharedString::from(format!("{key}:state")))
                        .tooltip(ui::tooltip(word))
                        .child(ui::icon(mark, 12.).text_color(tint)),
                ),
        );
    let body = open.then(|| {
        let rows = looks
            .reading
            .subagents
            .get(&card.session_id)
            .map_or(&[][..], Vec::as_slice);
        let live = card.state == SubagentState::Running;
        let mut drawn = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            let groups = runs(row.message.parts.iter());
            let count = groups.len();
            for (at, group) in groups.into_iter().enumerate() {
                drawn.push(match group {
                    Shown::Part(part) => self::part(row, part, false, pen, looks, window, cx),
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
                    .child("No output yet.")
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
            .border_1()
            .border_t_0()
            .border_color(t.border)
            .rounded_b(theme::RADIUS_2)
            .bg(Hsla {
                a: 0.45,
                ..t.surface_raised
            })
            .children(drawn)
    });
    div()
        .my(SPACE_1H)
        .flex()
        .flex_col()
        .child(summary)
        .children(body)
        .into_any_element()
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
            let Some(doc) = row.doc(part.id()) else {
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
                attach::with_chips(look, &row.message, &looks.agent, &looks.bench, window, cx)
            } else {
                look
            };
            let cut = (doc.units > LONG_TEXT && looks.reading.folds(&row.message.id, &id))
                .then(|| doc.markdown.cut(LONG_TEXT))
                .flatten();
            let Some((blocks, hidden)) = cut else {
                return pen.markdown_with(&look, id, &doc.markdown, window, cx);
            };
            let reader = looks.reader.clone();
            let label = format!("Show the rest · {}k more characters", (hidden + 500) / 1000);
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
            page: Some(page), ..
        } => self::page::page(page, looks, window, cx),
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
        ChatPart::Notice { notice, .. } => {
            let t = Tokens::get(cx);
            let icon = match notice.task_type.as_deref() {
                Some("workflow") => Icon::Workflow,
                _ => Icon::Timer,
            };
            let (state, tint) = match notice.state {
                NoticeState::Completed => ("completed", looks.quiet),
                NoticeState::Failed => ("failed", t.danger),
                NoticeState::Stopped => ("stopped", looks.quiet),
            };
            notice_box(looks)
                .child(ui::icon(icon, 12.).text_color(looks.quiet))
                .child(notice_name(notice.name.clone(), true, looks))
                .child(div().flex_none().text_color(tint).child(state))
                .children(notice.summary.clone().map(notice_summary))
                .into_any_element()
        }
        ChatPart::AgentNotice { notice, .. } => {
            let t = Tokens::get(cx);
            let (icon, tint, state) = match notice.severity {
                NoticeSeverity::Info => (Icon::Inbox, looks.quiet, looks.quiet),
                NoticeSeverity::Warning => (Icon::Warning, t.warn, looks.quiet),
                NoticeSeverity::Error => (Icon::Warning, t.danger, t.danger),
            };
            notice_box(looks)
                .child(ui::icon(icon, 12.).text_color(tint))
                .child(
                    div()
                        .flex_none()
                        .text_color(state)
                        .child(notice.title.clone()),
                )
                .children(notice.description.clone().map(notice_summary))
                .into_any_element()
        }
        ChatPart::Account { moved, .. } => {
            let why = moved
                .from
                .as_ref()
                .filter(|_| moved.reason == MoveReason::Limit)
                .map(|from| format!("{from} reached its usage limit"));
            notice_box(looks)
                .child(ui::icon(Icon::User, 12.).text_color(looks.quiet))
                .child(div().flex_none().child("Moved to"))
                .child(notice_name(moved.label.clone(), false, looks))
                .children(why.map(notice_summary))
                .into_any_element()
        }
        ChatPart::Content { content, .. } => content::content(id, content, looks, cx),
    }
}

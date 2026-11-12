//! Drawing a chat's rows: a message and its parts, the work folded under a turn, letters and
//! fired triggers, and subagents. Runs of messages between agents are in `team`.

use super::*;
use sikemux_view::chat::{GoalMoment, PageRef, TaskNotice};
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
    let opens_run = team.filter(|team| {
        ix == 0 || (rows[ix - 1].message.team.as_ref()).is_none_or(|before| before.id != team.id)
    });
    let team_head = opens_run.and_then(|team| {
        let line = team::line_of(&rows[ix..], &team.id)?;
        Some(team::head(&team.id, line, team_open, looks, cx))
    });
    let behind = team
        .filter(|_| team_open)
        .is_some_and(|team| team::behind(&rows[ix..], &team.id, looks.reading));
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
        ix == 0
            || rows[ix - 1]
                .message
                .fold
                .as_ref()
                .is_none_or(|before| before.id != fold.id)
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
    let follows_own = user && ix > 0 && plainly_sent(&rows[ix - 1]);
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
                Shown::Part(part @ ChatPart::Text { .. }) if at > 0 && !user => div()
                    .pt(SPACE_2H)
                    .child(self::part(row, part, user, pen, looks, window, cx))
                    .into_any_element(),
                Shown::Part(part) => self::part(row, part, user, pen, looks, window, cx),
                Shown::Run(calls) => tools::run(&calls, live && at + 1 == count, looks, window, cx),
                Shown::Tabs(pages) => self::page::tabs(&pages, looks, window, cx),
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
        .and_then(|switch| crate::worktrees::switch::note(switch, cx))
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
                        column.child(bubble(looks).children(parts))
                    })
                    .children(gestures::reactions(message, true, looks))
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
    let ended = shown::ended_at(rows, ix, &looks.reading.shown);
    let finished = shown::results_after(rows, ix, &looks.reading.results);
    let after = (!ended.is_empty() || !finished.is_empty()).then(|| {
        div()
            .w_full()
            .max_w(px(800.))
            .px(px(22.))
            .children(ended.into_iter().map(|turn| {
                let key = format!("shown:{}", turn.turn);
                shown::pictures(&turn.pages, &key, looks, cx)
            }))
            .children(
                finished
                    .into_iter()
                    .map(|row| shown::result_row(row, looks, cx)),
            )
    });
    let drawn = div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .children(lost)
        .children(worktree)
        .children(team_head)
        .child(
            div()
                .w_full()
                .max_w(px(800.))
                .px(px(22.))
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
                .when(ix == 0 && !looks.reading.earlier_lost && !noted, |row| {
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

/// The prompts of the chat's triggers that came due, each in a bubble at the left under the
/// trigger's name, with the webhook request that fired it folded beneath.
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
                let look = looks.styles.borrow().user.clone();
                pen.markdown_with(&look, id.clone(), &doc.markdown, window, cx)
            });
            from(trigger_named(&trigger.title, &t), row, prompt, looks)
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
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACE_3H)
        .children(drawn)
}

/// A trigger's name over its prompt.
fn trigger_named(title: &str, t: &Tokens) -> Div {
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::icon(Icon::Timer, 13.).text_color(t.cmd))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .child(title.to_owned()),
        )
}

/// What another agent or a trigger sent: who over its bubble at the left, and when at the
/// line's end.
fn from(who: Div, row: &ChatRow, body: Option<AnyElement>, looks: &Looks) -> Div {
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
        .child(
            div()
                .max_w_full()
                .px(px(14.))
                .py(SPACE_2H)
                .rounded(BUBBLE_RADIUS)
                .rounded_bl(SPACE_1H)
                .border_1()
                .border_color(looks.line)
                .bg(looks.asked)
                .children(body),
        )
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
    while ix < letters.len() {
        let letter = &letters[ix];
        if letter.kind == LetterKind::Answer {
            let run = letters[ix..]
                .iter()
                .take_while(|next| {
                    next.kind == LetterKind::Answer && next.agent_id == letter.agent_id
                })
                .count();
            drawn.push(answer_thread(row, &letters[ix..ix + run], ix, looks, cx));
            ix += run;
            continue;
        }
        drawn.push(match letter.kind {
            LetterKind::Answer => unreachable!(),
            LetterKind::Message | LetterKind::Trigger => {
                let id = SharedString::from(format!("{}:{}", row.message.id, letter_doc(ix)));
                let text = row.doc(&letter_doc(ix)).map(|doc| {
                    let look = looks.styles.borrow().user.clone();
                    pen.markdown_with(&look, id.clone(), &doc.markdown, window, cx)
                });
                let who = match &letter.trigger {
                    Some(trigger) => trigger_named(&trigger.title, Tokens::get(cx)),
                    None => {
                        let (bench, agent) = (looks.bench.clone(), letter.agent_id.clone());
                        let chat = looks.agent.clone();
                        let name = div()
                            .id(SharedString::from(format!("{id}:from")))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(looks.ink)
                            .cursor_pointer()
                            .hover(|name| name.underline())
                            .child(format!("@{}", letter.name))
                            .on_click(move |_, _, cx| {
                                bench
                                    .update(cx, |bench, cx| {
                                        let from = bench.chat_target(&chat);
                                        bench.open_agent_from(&agent, from, cx)
                                    })
                                    .ok();
                            });
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .child(looks.mark_of(&letter.agent_id, &letter.cli, 17., cx))
                            .child(name)
                    }
                };
                from(who, row, text, looks).into_any_element()
            }
        });
        ix += 1;
    }
    let answered_last = letters
        .last()
        .is_some_and(|letter| letter.kind == LetterKind::Answer);
    if let Some(chips) = gestures::reactions(&row.message, answered_last, looks) {
        let last = drawn.pop().map(|last| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .map(|column| match answered_last {
                    true => column.items_end(),
                    false => column.items_start(),
                })
                .child(last)
                .child(chips)
                .into_any_element()
        });
        drawn.extend(last);
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
    let asker = &answers[0];
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
    Tabs(Vec<&'a PageRef>),
}

/// A message's parts with each run of tool calls together, and pages shown in desk tabs one after
/// another in one frame; a page drawn in the reply stands on its own.
pub(super) fn runs<'a>(parts: impl Iterator<Item = &'a ChatPart>) -> Vec<Shown<'a>> {
    let mut shown = Vec::new();
    for part in parts {
        match (part, shown.last_mut()) {
            (
                ChatPart::Tool {
                    page: Some(page), ..
                },
                Some(Shown::Tabs(pages)),
            ) if page.tab => pages.push(page),
            (
                ChatPart::Tool {
                    page: Some(page), ..
                },
                _,
            ) if page.tab => shown.push(Shown::Tabs(vec![page])),
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

/// Where a goal began, passed a step or ended, as a milestone: its state in its colour, its
/// name, and at the right its steps then and, past its start, how long it had taken.
fn goal_marker(notice: &AgentNotice, looks: &Drawn, cx: &App) -> AnyElement {
    let Some(goal) = notice.goal.as_ref() else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx);
    let (state, word) = match (goal.moment, goal.state) {
        (GoalMoment::Step, _) => (SliceState::Done, "Step done"),
        (GoalMoment::Done, _) => (SliceState::Done, "Goal done"),
        (GoalMoment::Stopped, SliceState::Failed) => (SliceState::Failed, "Goal stopped"),
        (GoalMoment::Stopped, _) => (SliceState::Waiting, "Goal stopped"),
        (GoalMoment::Started, SliceState::NeedsYou) => (SliceState::NeedsYou, "Goal needs you"),
        (GoalMoment::Started, SliceState::Waiting) => (SliceState::Waiting, "Goal waiting"),
        (GoalMoment::Started, state) => (state, "Goal"),
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

/// Where the reader's choice to see what was left out of `row` is kept.
fn left_out_key(row: &ChatRow) -> String {
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
        1 => "1 sentence with no source left out".to_owned(),
        count => format!("{count} sentences with no source left out"),
    };
    let faint = Tokens::get(cx).text_tertiary;
    let (reader, ink) = (looks.reader.clone(), looks.ink);
    let toggle = div()
        .id(SharedString::from(key.clone()))
        .cursor_pointer()
        .hover(move |toggle| toggle.text_color(ink))
        .child(if open { "Hide" } else { "Show" })
        .on_click(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&key);
                cx.notify();
            })
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
pub(super) fn card_of<'a>(rows: &'a [ChatRow], session: &str) -> Option<&'a SubagentCard> {
    rows.iter().rev().find_map(|row| {
        row.message.parts.iter().find_map(|part| match part {
            ChatPart::Subagent { subagent, .. } if subagent.session_id == session => Some(subagent),
            _ => None,
        })
    })
}

/// A background task that ended, drawn as the last row of a tool-call tree: what kind of task
/// it was, its name's first line, and how it ended.
fn task_ended(notice: &TaskNotice, looks: &Drawn, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (icon, word, tint) = match notice.task_type.as_deref() {
        Some("workflow") => (Icon::Workflow, "workflow", looks.tints.called),
        Some("monitor") => (Icon::Timer, "watch", looks.tints.looked),
        _ => (Icon::Run, "bg", looks.tints.ran),
    };
    let (mark, state, state_ink) = match notice.state {
        NoticeState::Completed => (Some((Icon::Check, t.live)), "done", looks.quiet),
        NoticeState::Failed => (Some((Icon::Close, t.danger)), "failed", t.danger),
        NoticeState::Stopped => (None, "stopped", looks.quiet),
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
        .font_family(theme::MONO)
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
/// The quiet line that says a rule was kept from what the person said, with Undo.
fn kept_rule(notice: &AgentNotice, looks: &Drawn) -> AnyElement {
    let Some(rule) = notice.rule.clone() else {
        return div().into_any_element();
    };
    let bench = looks.bench.clone();
    notice_box(looks)
        .child(ui::icon(Icon::Book, 12.).text_color(looks.quiet))
        .child(div().flex_none().child(notice.title.clone()))
        .child("·")
        .child(
            div()
                .id(SharedString::from(format!("kept-rule-{}", rule.id)))
                .cursor_pointer()
                .text_color(looks.ink)
                .child("Undo")
                .on_click(move |_, _, cx| {
                    let rule = rule.clone();
                    let _ = bench.update(cx, |bench, cx| {
                        crate::playbook::undo_rule(bench, &rule.project, rule.id, cx)
                    });
                }),
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
        .map(|tokens| format!("{} tokens", crate::ui::numbers::compact(tokens as f64)));
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
    let doing = card.last_tool.as_ref().map(|tool| {
        let row = &tool.row;
        row.activity
            .clone()
            .unwrap_or_else(|| row.subject.clone().unwrap_or_else(|| row.target.clone()))
    });
    let state = match (card.state, doing) {
        (SubagentState::Running, Some(doing)) if !doing.is_empty() => div()
            .min_w_0()
            .truncate()
            .text_color(t.text_secondary)
            .child(shimmer(format!("{key}:doing"), doing))
            .into_any_element(),
        _ => div()
            .flex_none()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_color(
                if matches!(
                    card.state,
                    SubagentState::Failed | SubagentState::Disconnected
                ) {
                    tint
                } else {
                    looks.quiet
                },
            )
            .child(ui::icon(mark, 12.).text_color(tint))
            .child(word)
            .into_any_element(),
    };
    let (reader, toggled, hover) = (looks.reader.clone(), key.clone(), looks.hover);
    let summary = div()
        .id(SharedString::from(key.clone()))
        .h(px(30.))
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_3)
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .cursor_pointer()
        .hover(move |summary| summary.bg(hover))
        .on_click(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&toggled);
                cx.notify();
            })
        })
        .child(div().flex_none().child(ui::mark(&looks.provider, 17.)))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(looks.ink)
                .child(card.name.clone()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.quiet)
                .child(task),
        )
        .child(state)
        .child(div().flex_1())
        .when(!cost.is_empty(), |summary| {
            summary.child(div().flex_none().text_color(looks.faint).child(cost))
        })
        .child(div().flex_none().child(motion::turn(
            format!("{key}:chevron"),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 12.).text_color(looks.quiet),
        )));
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
                    Shown::Tabs(pages) => self::page::tabs(&pages, looks, window, cx),
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
            .children(drawn)
    });
    div()
        .flex()
        .flex_col()
        .rounded(theme::RADIUS_3)
        .bg(Hsla {
            a: 0.55,
            ..t.surface_raised
        })
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
            let restored = (!user && looks.reading.opened.contains(&left_out_key(row)))
                .then(|| row.restored(part.id()))
                .flatten();
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
                attach::with_chips(look, &row.message, &looks.bench, window)
            } else {
                attach::with_why(look, &row.message, &looks.bench, window)
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
        ChatPart::Notice { notice, .. } => task_ended(notice, looks, cx),
        ChatPart::AgentNotice { notice, .. } if notice.goal.is_some() => {
            goal_marker(notice, looks, cx)
        }
        ChatPart::AgentNotice { notice, .. } if notice.rule.is_some() => kept_rule(notice, looks),
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
                .child(div().flex_none().child("Moved to"))
                .child(notice_name(moved.label.clone(), false, looks))
                .children(why.map(notice_summary))
                .into_any_element()
        }
        ChatPart::Content { content, .. } => content::content(id, content, looks, cx),
        ChatPart::Rotation { .. } => div().into_any_element(),
    }
}

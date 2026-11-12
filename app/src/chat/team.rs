//! Messages between agents, which the core folds into runs: the one line a closed run shows,
//! and what it holds once opened.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, TEXT_2XS, TEXT_XS_H};

const MARK: f32 = 16.;

fn same_run<'a>(rows: &'a [ChatRow], id: &'a str) -> impl Iterator<Item = &'a ChatRow> {
    rows.iter()
        .take_while(move |row| (row.message.team.as_ref()).is_some_and(|team| team.id == id))
}

/// What run `id`'s line says, from the rows it holds from here on. Rows paged back from an
/// earlier chunk of a run carry a line of their own, so the last one counts.
pub(super) fn line_of<'a>(rows: &'a [ChatRow], id: &'a str) -> Option<&'a TeamLine> {
    same_run(rows, id)
        .filter_map(|row| row.message.team.as_ref()?.line.as_ref())
        .last()
}

/// Whether the first of `rows` waits behind "Show N earlier" while run `id` is open.
pub(super) fn behind(rows: &[ChatRow], id: &str, reading: &Reading) -> bool {
    if reading.team_all(id) {
        return false;
    }
    let Some(line) = line_of(rows, id) else {
        return false;
    };
    rows.first().is_some_and(|row| row.message.id != line.from)
        && same_run(rows, id).any(|row| row.message.id == line.from)
}

fn elapsed(took: u64) -> String {
    match took {
        0..60_000 => crate::ui::time::took(took),
        _ => crate::ui::time::total(took as f64),
    }
}

/// What a run's line says after its count, and what an agent in it needs of the person.
fn said(line: &TeamLine) -> (String, Option<TeamNeedKind>) {
    if let Some(need) = &line.need {
        let name = &need.name;
        let text = need.text.as_deref();
        let words = match (need.kind, text) {
            (TeamNeedKind::Asks, Some(text)) => {
                t!("@{name} asks: {text}", name = name, text = text)
            }
            (TeamNeedKind::Asks, None) => t!("@{name} has a question", name = name),
            (TeamNeedKind::Waits, Some(text)) => format!("@{name}: {text}"),
            (TeamNeedKind::Waits, None) => t!("@{name} waits on you", name = name),
            (TeamNeedKind::Failed, Some(text)) => {
                t!("@{name} failed: {text}", name = name, text = text)
            }
            (TeamNeedKind::Failed, None) => t!("@{name} failed", name = name),
            (TeamNeedKind::Stopped, _) => t!("@{name} stopped", name = name),
        };
        return (words, Some(need.kind));
    }
    let latest = line.latest.as_ref();
    let words = latest.map_or_else(String::new, |said| format!("@{}: {}", said.name, said.text));
    (words, None)
}

/// The one line a run folds to, and while it is open and has more than it opens on, the way to
/// the rest.
pub(super) fn head(
    id: &str,
    line: &TeamLine,
    open: bool,
    looks: &Drawn,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (words, need) = said(line);
    let tone = match need {
        Some(TeamNeedKind::Failed) => t.danger,
        Some(_) => t.accent,
        None => looks.quiet,
    };
    let marks = line
        .agents
        .iter()
        .map(|agent| looks.mark_of(&agent.agent_id, &agent.cli, MARK, cx))
        .collect::<Vec<_>>();
    let count = match line.messages {
        1 => t!("1 message").to_owned(),
        messages => t!("{messages} messages", messages = messages),
    };
    let (reader, bench, agent, hover) = (
        looks.reader.clone(),
        looks.bench.clone(),
        looks.agent.clone(),
        looks.hover,
    );
    let toggled = id.to_owned();
    let toggle = move |cx: &mut App, change: &dyn Fn(&mut Reading)| {
        bench
            .update(cx, |bench, cx| {
                if let Some(chat) = bench.chats.open.get(&agent) {
                    chat.hold_place(cx);
                }
            })
            .ok();
        reader.update(cx, |reading, cx| {
            change(reading);
            cx.notify();
        });
    };
    let row = {
        let (toggle, toggled) = (toggle.clone(), toggled.clone());
        div()
            .id(SharedString::from(format!("team:{id}")))
            .min_w_0()
            .h(px(24.))
            .ml(px(-6.))
            .mr(px(-6.))
            .px(SPACE_1H)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded(theme::RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .on_click(move |_, _, cx| {
                let id = toggled.clone();
                toggle(cx, &move |reading| {
                    if reading.teams.remove(&id).is_none() {
                        reading.teams.insert(id.clone(), false);
                    }
                });
            })
            .child(motion::turn(
                format!("team-chevron:{id}"),
                motion::fold_angle(open),
                motion::DETAIL_FOLD,
                ui::icon(Icon::Chevron, 10.).text_color(looks.quiet),
            ))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(SPACE_0)
                    .children(marks)
                    .when(line.more > 0, |marks| {
                        marks.child(
                            div()
                                .ml(SPACE_0)
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(looks.quiet)
                                .child(format!("+{}", line.more)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(looks.ink)
                    .child(count),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(tone)
                    .child(words),
            )
            .children(line.took.map(|took| {
                div()
                    .flex_none()
                    .text_color(looks.quiet)
                    .child(elapsed(took))
            }))
    };
    let earlier = (open && line.earlier > 0 && !looks.reading.team_all(id)).then(|| {
        let (toggle, toggled) = (toggle.clone(), toggled.clone());
        div()
            .id(SharedString::from(format!("team-earlier:{id}")))
            .mt(SPACE_1)
            .ml(px(-6.))
            .px(SPACE_1H)
            .h(px(22.))
            .flex()
            .items_center()
            .self_start()
            .rounded(theme::RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(looks.quiet)
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .on_click(move |_, _, cx| {
                let id = toggled.clone();
                toggle(cx, &move |reading| {
                    reading.teams.insert(id.clone(), true);
                });
            })
            .child(t!("Show {count} earlier", count = line.earlier))
    });
    div()
        .w_full()
        .max_w(super::column::rows(cx))
        .px(px(22.))
        .py(px(5.))
        .flex()
        .flex_col()
        .child(row)
        .children(earlier)
        .into_any_element()
}

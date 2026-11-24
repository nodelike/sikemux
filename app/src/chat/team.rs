//! Messages between agents, which the core folds into runs: the one line a closed run shows,
//! and what it holds once opened.

use super::*;
use crate::ui::Clicks;

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

/// What a run's line says after its count: the agent it is about, by id and name, what that
/// agent said or needs of the person, and the need itself.
fn said(line: &TeamLine) -> (Option<(&str, &str)>, String, Option<TeamNeedKind>) {
    if let Some(need) = &line.need {
        let text = need.text.as_deref();
        let words = match (need.kind, text) {
            (TeamNeedKind::Asks, Some(text)) => t!("asks: {text}", text = text),
            (TeamNeedKind::Asks, None) => t!("has a question").to_owned(),
            (TeamNeedKind::Waits, Some(text)) => text.to_owned(),
            (TeamNeedKind::Waits, None) => t!("waits on you").to_owned(),
            (TeamNeedKind::Failed, Some(text)) => t!("failed: {text}", text = text),
            (TeamNeedKind::Failed, None) => t!("failed").to_owned(),
            (TeamNeedKind::Stopped, _) => t!("stopped").to_owned(),
        };
        return (Some((&need.agent_id, &need.name)), words, Some(need.kind));
    }
    match &line.latest {
        Some(said) => (Some((&said.agent_id, &said.name)), said.text.clone(), None),
        None => (None, String::new(), None),
    }
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
    let (sender, words, need) = said(line);
    let tone = match need {
        Some(TeamNeedKind::Failed) => t.danger,
        Some(_) => t.accent,
        None => looks.quiet,
    };
    let pill = sender
        .filter(|(agent, _)| !agent.is_empty())
        .and_then(|(agent, name)| {
            let bench = looks.bench.upgrade()?;
            let cli = (line.agents.iter())
                .find(|sent| sent.agent_id == agent)
                .map_or(looks.provider.as_str(), |sent| sent.cli.as_str());
            let mark = looks.mark_of(agent, cli, TEXT_XS_H + 1., cx);
            let from = bench.read(cx).chat_target(&looks.agent);
            Some(super::agent_pill_of(
                agent,
                name,
                mark,
                TEXT_XS_H,
                from,
                &looks.bench,
                cx,
            ))
        });
    let words = match (&pill, sender) {
        (None, Some((_, name))) if need.is_none() => format!("@{name}: {words}"),
        (None, Some((_, name))) => format!("@{name} {words}"),
        _ => words,
    };
    let alone = pill.is_some() && line.agents.len() == 1 && line.more == 0;
    let marks = (line.agents.iter())
        .filter(|_| !alone)
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
            .rounded(theme::radius_2())
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .hover(move |row| row.bg(hover))
            .clicks(move |_, _, cx| {
                let id = toggled.clone();
                toggle(cx, &move |reading| reading.toggle_team(&id));
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
            .children(pill)
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
            .rounded(theme::radius_2())
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(looks.quiet)
            .hover(move |row| row.bg(hover))
            .clicks(move |_, _, cx| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sikemux_fold::{ChatEventKind, Fold};
    use sikemux_wire::protocol::PermissionMode;

    /// How a worker's report was recorded on 2026-11-24, with the report itself cut short.
    fn report(name: &str, id: &str, cli: &str, said: &str) -> serde_json::Value {
        json!({ "paths": [], "text": format!(
            "<agent-message from=\"{name}\" agent-id=\"{id}\" cli=\"{cli}\" note=\"{name} ({id}) is an agent you started, and this is how its turn ended, sent by Sikemux. Reply with agent_send to it if it needs more; once its work has landed and you need nothing more, close it with agent_close.\">\nFinished its turn. Its final answer:\n\n{said}\n</agent-message>"
        ) })
    }

    fn rows(fold: &mut Fold) -> Vec<ChatRow> {
        fold.flush();
        fold.view(false)
            .rows
            .into_iter()
            .map(ChatRow::new)
            .collect()
    }

    fn run_of<'a>(rows: &'a [ChatRow], name: &str) -> &'a str {
        let row = rows
            .iter()
            .find(|row| (row.message.letters.iter()).any(|letter| letter.name == name));
        let team = row.and_then(|row| row.message.team.as_ref());
        &team.expect("a report is in a run").id
    }

    #[test]
    fn reports_come_folded_and_a_click_folds_them_while_the_find_bar_looks() {
        let mut fold = Fold::new(PermissionMode::Bypass);
        let mut at = 1795487800000;
        let mut next = |fold: &mut Fold, kind, payload: serde_json::Value| {
            at += 1000;
            fold.apply(at, kind, &payload, at);
        };
        let ashborn = report(
            "Ashborn",
            "agent-c4cd9c7694424c30b8269801cd47984e",
            "claude",
            "Holding: I haven't edited, translated or landed anything.",
        );
        let bege = report(
            "Bege",
            "agent-712004123d574116b60ed23b9f0905d2",
            "antigravity",
            "**Recommendation:** Yes, with a small core-only rule.",
        );
        next(
            &mut fold,
            ChatEventKind::Prompt,
            json!({ "text": "What holds the rename?" }),
        );
        next(&mut fold, ChatEventKind::TurnStarted, json!({}));
        next(&mut fold, ChatEventKind::Prompt, ashborn.clone());
        next(
            &mut fold,
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "end_turn" }),
        );
        next(
            &mut fold,
            ChatEventKind::Prompt,
            json!({ "text": "And the spaces?" }),
        );
        next(&mut fold, ChatEventKind::TurnStarted, json!({}));
        next(
            &mut fold,
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "end_turn" }),
        );
        next(&mut fold, ChatEventKind::Prompt, bege);
        next(&mut fold, ChatEventKind::TurnStarted, json!({}));

        let shown = rows(&mut fold);
        let mid_turn = run_of(&shown, "Ashborn").to_owned();
        let alone = run_of(&shown, "Bege").to_owned();
        assert_ne!(mid_turn, alone);
        let letters = shown.iter().filter(|row| !row.message.letters.is_empty());
        assert!(letters
            .flat_map(|row| (0..row.docs().len()).map(move |doc| row.findable(doc)))
            .all(|found| !found));

        let mut reading = Reading {
            finding: true,
            ..Default::default()
        };
        for run in [&mid_turn, &alone] {
            assert!(!reading.team_open(run), "comes folded");
            reading.toggle_team(run);
            assert!(reading.team_open(run), "a click opens it");
            assert!(!reading.team_all(run));
            reading.toggle_team(run);
            assert!(!reading.team_open(run), "a click folds it");
        }

        reading.toggle_team(&alone);
        next(&mut fold, ChatEventKind::Prompt, ashborn);
        next(
            &mut fold,
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "end_turn" }),
        );
        let grown = rows(&mut fold);
        let last = grown.iter().rev().find_map(|row| row.message.team.as_ref());
        assert_eq!(last.map(|team| team.id.as_str()), Some(alone.as_str()));
        assert!(
            reading.team_open(&alone),
            "stays open as another message joins"
        );
    }
}

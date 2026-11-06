//! The goal page's account of itself: what happened and what was decided, as sentences the core
//! had a model write from the goal's agents' real moments. A click on a sentence shows the
//! moments behind it, and each opens what it came from. The core writes, checks and keeps the
//! sentences; this only draws them.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled,
};
use sikemux_core::server::board::BoardCard;
use sikemux_view::slice::{
    Decision, GoalSummary, GoalSummaryView, LeftOut, LeftOutWhy, SourceKind, SourceLink,
    SummaryLine, SummarySource,
};

use super::goal::heading;
use super::{At, GoalsPane};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, TEXT_2XS, TEXT_SM, TEXT_XS_H,
};
use crate::ui::{self, Button, Variant};

/// What a moment is, as its source box heads it.
fn kind_words(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Person => "You said",
        SourceKind::Answer => "Agent's answer",
        SourceKind::Ran => "Agent ran",
        SourceKind::Commit => "Commit",
        SourceKind::Evidence => "Evidence Sikemux collected",
        SourceKind::Gate => "Gate",
        SourceKind::Held => "Held action",
        SourceKind::Decision => "Decision",
        SourceKind::Assertion => "Assertion",
        SourceKind::Note => "Your note",
        SourceKind::PullRequest => "Pull request",
        SourceKind::TurnFailed => "Turn failed",
    }
}

fn open_words(link: &SourceLink) -> &'static str {
    match link {
        SourceLink::Chat { .. } => "Open chat",
        SourceLink::Commit { .. } => "Open commit",
        SourceLink::Evidence { .. } => "Open evidence",
        SourceLink::Card { .. } => "Open card",
        SourceLink::Url { .. } => "Open pull request",
    }
}

impl GoalsPane {
    /// Opens what a moment came from.
    fn open_source(&mut self, link: &SourceLink, cx: &mut Context<Self>) {
        match link {
            SourceLink::Chat {
                agent,
                feed,
                index,
                saved,
            } => self.open_chat_at(agent, feed.as_deref(), *index, saved.as_ref(), cx),
            SourceLink::Commit { sha } => self.open_commit(sha, cx),
            SourceLink::Evidence { card, evidence } => {
                self.go(At::Slice(*card), cx);
                self.picked = Some(*evidence);
                cx.notify();
            }
            SourceLink::Card { card } => match self.card(*card) {
                Some(found) if found.slice.is_some() => self.go(At::Slice(*card), cx),
                _ => self.open_on_board(*card, cx),
            },
            SourceLink::Url { url } => self.open_url(None, url.clone(), cx),
        }
    }
}

/// One moment behind a sentence: what kind it is, who and when, what it said, and a way to it.
fn source_box(id: String, source: &SummarySource, cx: &mut Context<GoalsPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let who = (source.who.clone().into_iter())
        .chain([ui::time::age(source.at).to_string()])
        .collect::<Vec<_>>()
        .join(" · ");
    let link = source.open.clone();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::caps(kind_words(source.kind), cx))
                .child(ui::meta(who, cx))
                .child(div().flex_1())
                .children(link.map(|link| {
                    Button::new(
                        SharedString::from(format!("{id}-open")),
                        open_words(&link),
                        Variant::Outline,
                    )
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        cx.stop_propagation();
                        pane.open_source(&link, cx)
                    }))
                })),
        )
        .child(
            div()
                .text_size(theme::ui_text(TEXT_XS_H))
                .text_color(t.text_secondary)
                .child(source.text.clone()),
        )
}

/// The sources of a sentence that is open, in a box under it.
fn sources_box(
    id: &str,
    sources: &[SummarySource],
    action: Option<AnyElement>,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    div()
        .mt(SPACE_0)
        .mb(SPACE_2)
        .px(SPACE_2H)
        .py(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(t.surface_raised)
        .children(
            (sources.iter().enumerate())
                .map(|(at, source)| source_box(format!("{id}-{at}"), source, cx).into_any_element())
                .collect::<Vec<_>>(),
        )
        .children(action)
        .into_any_element()
}

/// A sentence that opens its sources with a click, with a mark at its end when it has one.
fn sentence(
    pane: &GoalsPane,
    id: String,
    text: String,
    mark: Option<&'static str>,
    cx: &mut Context<GoalsPane>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let open = pane.unfolded.contains(&id);
    let toggled = id.clone();
    div()
        .id(SharedString::from(id))
        .px(SPACE_2)
        .mx(-SPACE_2)
        .py(SPACE_1H)
        .flex()
        .items_start()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .text_size(theme::ui_text(TEXT_SM))
        .text_color(t.text_primary)
        .hover(move |line| line.bg(t.surface_hover))
        .when(open, |line| line.bg(t.surface_chosen))
        .on_click(cx.listener(move |pane, _, _, cx| {
            if !pane.unfolded.remove(&toggled) {
                pane.unfolded.insert(toggled.clone());
            }
            cx.notify();
        }))
        .child(div().flex_1().min_w_0().child(text))
        .children(mark.map(|mark| ui::meta(mark, cx).mt(SPACE_0)))
}

fn left_out_words(left_out: &[LeftOut]) -> Option<String> {
    let names = |why: LeftOutWhy| {
        (left_out.iter())
            .filter(|left| left.why == why)
            .map(|left| left.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let other = names(LeftOutWhy::OtherVendor);
    let unread = (left_out.iter())
        .filter(|left| left.why == LeftOutWhy::NoSession)
        .count();
    let mut words = Vec::new();
    if !other.is_empty() {
        words.push(format!(
            "Not read: {other}, who run on another vendor's agent, so their chats aren't sent to this one."
        ));
    }
    match unread {
        0 => {}
        1 => words.push("Written without one agent's chat, which is no longer on record.".into()),
        many => words.push(format!(
            "Written without {many} agents' chats, which are no longer on record."
        )),
    }
    (!words.is_empty()).then(|| words.join(" "))
}

/// A short line under a section, in the tertiary ink and wrapping.
fn aside(text: impl Into<SharedString>, cx: &Context<GoalsPane>) -> impl IntoElement {
    div()
        .pt(SPACE_1)
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(Tokens::get(cx).text_tertiary)
        .child(text.into())
}

/// The heading's meta: how old what the summary knows is, when it was written, and whether a
/// newer one is on its way.
fn age_words(view: &GoalSummaryView) -> Option<String> {
    let summary = view.summary.as_ref()?;
    let ages = format!(
        "last moment {} · written {}",
        ui::time::age(summary.through),
        ui::time::age(summary.at)
    );
    Some(match view.writing {
        true => format!("{ages} · writing a newer one"),
        false => ages,
    })
}

/// Says the summary failed, why, and offers to try again.
fn failure(why: &str, stands: bool, cx: &mut Context<GoalsPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let said = match stands {
        true => format!("The newest summary failed, so this one is older. {why}"),
        false => {
            format!("Sikemux couldn't write this summary, and shows none rather than guess. {why}")
        }
    };
    div()
        .py(SPACE_1H)
        .flex()
        .items_start()
        .gap(SPACE_2)
        .child(ui::icon(Icon::Warning, 14.).mt(SPACE_0).text_color(t.warn))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(theme::ui_text(TEXT_XS_H))
                .text_color(t.text_secondary)
                .child(said),
        )
        .child(
            Button::new("summary-again", "Try again", Variant::Outline).on_click(cx.listener(
                |pane, _, _, cx| {
                    if let At::Goal(card) = pane.at {
                        pane.read_summary(card, true, true, cx);
                    }
                },
            )),
        )
}

/// What happened on the goal: nothing while nothing has been written or is being written.
pub(super) fn what_happened(
    pane: &GoalsPane,
    id: u64,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    let view = pane.summary_of(id)?.clone();
    let summary = view.summary.as_ref();
    let empty = summary.is_none() && !view.writing && view.failed.is_none();
    if empty {
        return None;
    }
    let lines: Vec<AnyElement> = summary.map_or(Vec::new(), |summary| {
        (summary.happened.iter().enumerate())
            .flat_map(|(at, line)| {
                line_with_sources(pane, format!("happened-{at}"), line, None, None, cx)
            })
            .collect()
    });
    let waiting = summary.is_none() && view.writing;
    Some(
        div()
            .flex()
            .flex_col()
            .child(heading("What happened", None, age_words(&view), cx))
            .when(waiting, |section| {
                section.child(aside("Writing this from what the goal's agents did…", cx))
            })
            .children(lines)
            .when_some(view.failed.clone(), |section, why| {
                section.child(failure(&why, summary.is_some(), cx))
            })
            .when_some(summary.filter(|summary| summary.unchecked), |section, _| {
                section.child(aside(
                    "Sikemux recorded no check or gate here, so this rests on what the agents said and committed.",
                    cx,
                ))
            })
            .when_some(summary.and_then(|summary| left_out_words(&summary.left_out)), |section, words| {
                section.child(aside(words, cx))
            })
            .into_any_element(),
    )
}

fn line_with_sources(
    pane: &GoalsPane,
    id: String,
    line: &SummaryLine,
    mark: Option<&'static str>,
    action: Option<AnyElement>,
    cx: &mut Context<GoalsPane>,
) -> Vec<AnyElement> {
    let open = pane.unfolded.contains(&id);
    let mut drawn =
        vec![sentence(pane, id.clone(), line.text.clone(), mark, cx).into_any_element()];
    if open {
        drawn.push(sources_box(&id, &line.sources, action, cx));
    }
    drawn
}

/// Where a recorded decision came from, as a moment.
fn recorded_source(decision: &Decision) -> SummarySource {
    let learned = &decision.learned;
    let text = match (&decision.why, &learned.name) {
        (Some(why), _) => why.clone(),
        (None, Some(name)) => format!("@{name} recorded this on the goal."),
        (None, None) => "You recorded this on the goal.".to_owned(),
    };
    let open = learned.agent.clone().map(|agent| SourceLink::Chat {
        agent,
        feed: learned.feed.clone(),
        index: learned.turn,
        saved: None,
    });
    SummarySource {
        kind: SourceKind::Decision,
        at: learned.at,
        who: learned.name.clone(),
        text,
        open,
    }
}

/// What the goal decided: what was recorded, then what the model found said and nobody recorded,
/// which stays only a suggestion until the person keeps it.
pub(super) fn decided(
    pane: &GoalsPane,
    goal: &BoardCard,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    let recorded: Vec<&Decision> = (goal.decisions.iter())
        .filter(|decision| decision.retired.is_none())
        .collect();
    let extracted: Vec<SummaryLine> = (pane.summary_of(goal.id))
        .and_then(|view| view.summary.as_ref())
        .map(|summary: &GoalSummary| summary.extracted.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|line| !recorded.iter().any(|decision| decision.text == line.text))
        .collect();
    if recorded.is_empty() && extracted.is_empty() {
        return None;
    }
    let mut lines: Vec<AnyElement> = Vec::new();
    for decision in &recorded {
        let line = SummaryLine {
            text: decision.says(),
            sources: vec![recorded_source(decision)],
        };
        let overrule = decision.holder.as_ref().map(|_| {
            let overruled = (*decision).clone();
            let goal = goal.id;
            let overrule = Button::new(
                SharedString::from(format!("recorded-{}-overrule", decision.id)),
                "Overrule",
                Variant::Outline,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                cx.stop_propagation();
                if let Some(project) = pane.here.clone() {
                    pane.overrule(project, Some(goal), &overruled, cx);
                }
            }));
            div().flex().child(overrule).into_any_element()
        });
        lines.extend(line_with_sources(
            pane,
            format!("recorded-{}", decision.id),
            &line,
            None,
            overrule,
            cx,
        ));
    }
    for (at, line) in extracted.iter().enumerate() {
        let text = line.text.clone();
        let keep = Button::new(
            SharedString::from(format!("extracted-{at}-keep")),
            "Keep as a decision",
            Variant::Outline,
        )
        .on_click(cx.listener(move |pane, _, _, cx| {
            cx.stop_propagation();
            pane.keep_decision(text.clone(), cx)
        }))
        .into_any_element();
        lines.extend(line_with_sources(
            pane,
            format!("extracted-{at}"),
            line,
            Some("Extracted"),
            Some(div().flex().child(keep).into_any_element()),
            cx,
        ));
    }
    Some(
        div()
            .flex()
            .flex_col()
            .child(heading("Decided", None, None, cx))
            .children(lines)
            .into_any_element(),
    )
}

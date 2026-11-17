//! The Automation page: what runs in the project with nobody asking, one sentence each, with
//! Pause or Resume and Remove.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_calls::call;
use sikemux_calls::triggers::HoldTriggerParams;
use sikemux_calls::triggers::TriggerParams;
use sikemux_view::chat::TriggerWhen;
use sikemux_view::playbook::{Playbook, RunsAlone};

use super::said::{self, Moment};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1H};
use crate::toast;
use crate::ui::{self, Button, IconButton, Variant};
use crate::workbench::Workbench;

const WEEKDAYS: [&str; 7] = [
    n!("Sunday"),
    n!("Monday"),
    n!("Tuesday"),
    n!("Wednesday"),
    n!("Thursday"),
    n!("Friday"),
    n!("Saturday"),
];

pub(super) fn page(
    playbook: Option<&Playbook>,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(playbook) = playbook else {
        return vec![rows::section(
            t!("Automation"),
            None,
            None,
            false,
            rows::note(t!("Reading…"), cx),
            cx,
        )];
    };
    let lines: Vec<AnyElement> = match playbook.automation.is_empty() {
        true => vec![rows::note(
            t!("Nothing yet. Ask any agent to do something on a schedule, or when CI ends, and it shows here."),
            cx,
        )],
        false => (playbook.automation.iter())
            .flat_map(|line| match line {
                RunsAlone::Auto(_) => Vec::new(),
                RunsAlone::Trigger { .. } => vec![trigger_row(line, this, cx)],
            })
            .collect(),
    };
    let count = playbook.automation.len();
    vec![rows::section(
        t!("Automation"),
        (count > 0).then(|| SharedString::from(count.to_string())),
        Some(t!("What runs with nobody asking.").into()),
        false,
        div().children(lines),
        cx,
    )]
}

fn bolt(cx: &Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx);
    ui::icon(Icon::Bolt, 14.)
        .text_color(t.text_secondary)
        .into_any_element()
}

fn trigger_row(
    line: &RunsAlone,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let RunsAlone::Trigger {
        id,
        title,
        when,
        set,
        paused,
        ..
    } = line
    else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let (words, moment) = match (paused, set) {
        (Some(at), _) => (t!("Paused by you · {day}", day = said::day(*at)), None),
        (None, Some(set)) => (
            t!(
                "{name} set this up · {day}",
                name = said::at_name(set.name.as_deref()),
                day = said::day(set.at)
            ),
            Moment::of(set),
        ),
        (None, None) => (String::new(), None),
    };
    let under = said::line(
        format!("playbook-trigger-{id}"),
        words,
        moment,
        Vec::new(),
        this,
        cx,
    );
    let controls = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(hold_button(*id, paused.is_some(), this))
        .child(remove_button(*id, this));
    rows::row_parts(
        Some(bolt(cx)),
        div()
            .when(paused.is_some(), |label| label.text_color(t.text_secondary))
            .child(sentence(when, title)),
        Some(under),
        controls,
        cx,
    )
    .into_any_element()
}

/// The trigger as one sentence with its schedule first: "Every Monday at 9:00, update
/// dependencies."
fn sentence(when: &TriggerWhen, title: &str) -> String {
    let what = {
        let mut letters = title.trim().chars();
        let first = letters.next().unwrap_or_default();
        let rest: String = letters.collect();
        let shouting = rest.chars().next().is_some_and(char::is_uppercase);
        match shouting {
            true => format!("{first}{rest}"),
            false => format!("{}{rest}", first.to_lowercase()),
        }
    };
    let ends = what.ends_with(['.', '!', '?']);
    t!(
        "{when}, {what}{stop}",
        when = schedule(when),
        what = what,
        stop = if ends { "" } else { "." }
    )
}

fn schedule(when: &TriggerWhen) -> String {
    match when {
        TriggerWhen::Once { at } => t!(
            "On {day} at {time}",
            day = said::day(*at),
            time = clock_of(*at)
        ),
        TriggerWhen::Every { seconds } => every(*seconds),
        TriggerWhen::Daily { minute, weekdays } => {
            let at = format!("{}:{:02}", minute / 60, minute % 60);
            let mut days: Vec<u8> = weekdays.clone();
            days.sort_unstable();
            days.dedup();
            match days.as_slice() {
                [] => t!("Every day at {at}", at = at),
                [1, 2, 3, 4, 5] => t!("Every weekday at {at}", at = at),
                [0, 6] => t!("Every weekend at {at}", at = at),
                days => {
                    let names: Vec<&str> = (days.iter())
                        .filter_map(|day| {
                            WEEKDAYS
                                .get(usize::from(*day))
                                .copied()
                                .map(crate::i18n::tr)
                        })
                        .collect();
                    let named = match names.split_last() {
                        Some((last, [])) => (*last).to_owned(),
                        Some((last, rest)) => {
                            t!("{days} and {last}", days = rest.join(", "), last = last)
                        }
                        None => String::new(),
                    };
                    t!("Every {days} at {at}", days = named, at = at)
                }
            }
        }
        TriggerWhen::Webhook { .. } => t!("When its address is called").to_owned(),
        TriggerWhen::Ci { on } => t!("When CI on {branch} ends", branch = on),
        TriggerWhen::Mod { on } => t!("When {collection} changes", collection = on),
    }
}

fn every(seconds: u64) -> String {
    match seconds {
        604_800 => t!("Every week").to_owned(),
        86_400 => t!("Every day").to_owned(),
        3600 => t!("Every hour").to_owned(),
        s if s % 604_800 == 0 => t!("Every {count} weeks", count = s / 604_800),
        s if s % 86_400 == 0 => t!("Every {count} days", count = s / 86_400),
        s if s % 3600 == 0 => t!("Every {count} hours", count = s / 3600),
        s => match s.div_ceil(60) {
            1 => t!("Every minute").to_owned(),
            count => t!("Every {count} minutes", count = count),
        },
    }
}

fn clock_of(at: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(at as i64)
        .single()
        .map(|at| at.format("%-H:%M").to_string())
        .unwrap_or_default()
}

/// Auto's line, and under it while unfolded its four settings as rows of their own.
fn hold_button(id: u64, paused: bool, this: &Entity<Workbench>) -> Button {
    let this = this.clone();
    Button::new(
        SharedString::from(format!("playbook-trigger-{id}-hold")),
        if paused { t!("Resume") } else { t!("Pause") },
        Variant::Ghost,
    )
    .on_click(move |_, _, cx| {
        this.update(cx, |bench, cx| {
            let asked = bench
                .target()
                .request_typed::<call::HoldTrigger>(HoldTriggerParams {
                    trigger: id,
                    paused: !paused,
                });
            bench.report(
                asked,
                t!("It was not changed"),
                move |bench, _, cx| {
                    let said = if paused { t!("Resumed") } else { t!("Paused") };
                    bench.notify(toast::success(said), cx)
                },
                cx,
            );
        })
    })
}

fn remove_button(id: u64, this: &Entity<Workbench>) -> IconButton {
    let this = this.clone();
    IconButton::new(
        SharedString::from(format!("playbook-trigger-{id}-remove")),
        Icon::Trash,
        t!("Remove"),
    )
    .on_click(move |_, _, cx| {
        this.update(cx, |bench, cx| {
            let asked = bench
                .target()
                .request_typed::<call::RemoveTrigger>(TriggerParams { trigger: id });
            bench.report(
                asked,
                t!("It was not removed"),
                |bench, _, cx| bench.notify(toast::success(t!("Removed")), cx),
                cx,
            );
        })
    })
}

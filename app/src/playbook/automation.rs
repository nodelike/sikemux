//! The Automation page: what runs in the project with nobody asking, one sentence each, with
//! Pause or Resume and Remove. Auto's line unfolds in place to the agents it starts and how many.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled,
};
use sikemux_calls::board::BoardReviewerParams;
use sikemux_calls::board::BoardSettingsParams;
use sikemux_calls::call;
use sikemux_calls::triggers::HoldTriggerParams;
use sikemux_calls::triggers::TriggerParams;
use sikemux_view::chat::TriggerWhen;
use sikemux_view::playbook::{Auto, AutoAgent, Playbook, RunsAlone};

use super::said::{self, Moment};
use crate::chat::model_picker;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1H};
use crate::toast;
use crate::ui::{self, select, Button, IconButton, Pick, Variant};
use crate::workbench::Workbench;

type Save = Rc<dyn Fn(BoardReviewerParams, &mut gpui_kit::App)>;

const SLOT_CHOICES: [u32; 14] = [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 24, 32, 48, 64];
const CEILING_CHOICES: [u32; 5] = [50, 100, 250, 500, 1000];
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
    bench: &Workbench,
    playbook: Option<&Playbook>,
    auto_open: bool,
    folder: &str,
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
                RunsAlone::Auto(auto) => auto_rows(bench, auto, auto_open, folder, this, cx),
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

fn count_words(count: u32) -> String {
    const WORDS: [&str; 10] = [
        n!("one"),
        n!("two"),
        n!("three"),
        n!("four"),
        n!("five"),
        n!("six"),
        n!("seven"),
        n!("eight"),
        n!("nine"),
        n!("ten"),
    ];
    (count.checked_sub(1))
        .and_then(|at| WORDS.get(at as usize))
        .map_or_else(
            || count.to_string(),
            |word| crate::i18n::tr(word).to_owned(),
        )
}

/// Auto's line, and under it while unfolded its four settings as rows of their own.
fn auto_rows(
    bench: &Workbench,
    auto: &Auto,
    open: bool,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let at_once = match auto.slots {
        1 => t!("one at a time").to_owned(),
        slots => t!("{count} at once", count = count_words(slots)),
    };
    let label = match auto.ceiling {
        Some(dollars) => t!(
            "Start agents on the cards nobody holds, {at_once}, stops past ${dollars}.",
            at_once = at_once,
            dollars = dollars
        ),
        None => t!(
            "Start agents on the cards nobody holds, {at_once}.",
            at_once = at_once
        ),
    };
    let under = said::line(
        if open {
            "playbook-auto-open"
        } else {
            "playbook-auto"
        },
        t!("Each works on its own branch, and another agent reviews it before it lands"),
        None,
        Vec::new(),
        this,
        cx,
    );
    let toggle = {
        let (this, folder) = (this.clone(), folder.to_owned());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
            this.update(cx, |bench, cx| {
                if let Some(settings) = bench.settings_of(&folder) {
                    settings.auto_open = !settings.auto_open;
                }
                bench.parts.redraw_stage(cx);
            })
        }
    };
    let pause = {
        let (this, folder) = (this.clone(), folder.to_owned());
        Button::new("playbook-auto-hold", t!("Pause"), Variant::Ghost).on_click(move |_, _, cx| {
            let folder = folder.clone();
            this.update(cx, |bench, cx| {
                change_auto(
                    bench,
                    folder,
                    |params| params.auto = Some(false),
                    t!("Agents no longer start on cards by themselves"),
                    cx,
                )
            })
        })
    };
    let t = Tokens::get(cx).clone();
    let head = rows::row_parts(Some(bolt(cx)), label, Some(under), pause, cx)
        .id("playbook-auto-line")
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .on_click(toggle)
        .into_any_element();
    if !open {
        return vec![head];
    }
    let slots = {
        let (this, folder) = (this.clone(), folder.to_owned());
        select(
            "playbook-auto-slots",
            agents_word(auto.slots),
            (SLOT_CHOICES.into_iter())
                .map(|slots| Pick::new(slots.to_string(), agents_word(slots)))
                .collect(),
            Some(auto.slots.to_string().into()),
            false,
            move |slots, _, cx| {
                let slots: u32 = slots.parse().unwrap_or(2);
                let folder = folder.clone();
                this.update(cx, |bench, cx| {
                    change_auto(
                        bench,
                        folder,
                        |params| params.slots = Some(slots),
                        t!("Saved"),
                        cx,
                    )
                })
            },
            cx,
        )
    };
    let ceiling = {
        let (this, folder) = (this.clone(), folder.to_owned());
        select(
            "playbook-auto-ceiling",
            ceiling_words(auto.ceiling),
            ([0].into_iter().chain(CEILING_CHOICES))
                .map(|dollars| {
                    Pick::new(
                        dollars.to_string(),
                        ceiling_words((dollars > 0).then_some(dollars)),
                    )
                })
                .collect(),
            Some(auto.ceiling.unwrap_or(0).to_string().into()),
            false,
            move |dollars, _, cx| {
                let ceiling = dollars.parse().ok().filter(|dollars: &u32| *dollars > 0);
                let folder = folder.clone();
                this.update(cx, |bench, cx| {
                    change_auto(
                        bench,
                        folder,
                        |params| params.spend_ceiling = Some(ceiling),
                        t!("Saved"),
                        cx,
                    )
                })
            },
            cx,
        )
    };
    let builder = agent_picker(bench, Role::Builder, &auto.builder, folder, this, cx);
    let reviewer = agent_picker(
        bench,
        Role::Reviewer,
        &auto.reviewer.clone().unwrap_or_default(),
        folder,
        this,
        cx,
    );
    let indent = |row: gpui_kit::Div| row.pl(gpui_kit::px(46.)).into_any_element();
    vec![
        head,
        indent(rows::row_with(
            None,
            t!("Builds"),
            Some(t!("The agent that works each card").into()),
            builder,
            cx,
        )),
        indent(rows::row_with(
            None,
            t!("Reviews"),
            Some(t!("Another agent reads each change before it lands").into()),
            reviewer,
            cx,
        )),
        indent(rows::row_with(None, t!("At once"), None, slots, cx)),
        indent(rows::row_with(
            None,
            t!("Spend ceiling"),
            Some(t!("Past it, no new card starts").into()),
            ceiling,
            cx,
        )),
    ]
}

fn agents_word(slots: u32) -> String {
    match slots {
        1 => t!("1 agent").to_owned(),
        slots => t!("{slots} agents", slots = slots),
    }
}

fn ceiling_words(ceiling: Option<u32>) -> String {
    ceiling.map_or(t!("Off").to_owned(), |dollars| format!("${dollars}"))
}

fn change_auto(
    bench: &mut Workbench,
    folder: String,
    change: impl FnOnce(&mut BoardSettingsParams),
    said: &'static str,
    cx: &mut Context<Workbench>,
) {
    let mut params = BoardSettingsParams {
        project: folder,
        ..BoardSettingsParams::default()
    };
    change(&mut params);
    let asked = bench.target().request_typed::<call::BoardSettings>(params);
    bench.report(
        asked,
        t!("It was not saved"),
        move |bench, _, cx| bench.notify(toast::success(said), cx),
        cx,
    );
}

/// Which of Auto's agents a picker chooses.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    Builder,
    Reviewer,
}

/// Auto's builder or reviewer as the composer's own picker: the agent, its account, who pays
/// and the model, in one popover.
fn agent_picker(
    bench: &Workbench,
    role: Role,
    agent: &AutoAgent,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let launchers = bench.launchers().to_vec();
    let target = bench.target();
    let Some(shown) = (agent.launcher.as_deref())
        .and_then(|id| launchers.iter().find(|known| known.id == id))
        .or(launchers.first())
        .cloned()
    else {
        return ui::meta(t!("No agents"), cx).into_any_element();
    };
    let profile_of = |id: &str| id.split_once(':').map(|(_, profile)| profile.to_owned());
    let mut models = shown.model.clone();
    if let (Some(models), Some(model)) = (models.as_mut(), agent.model.as_deref()) {
        models.current = model.to_owned();
    }
    let save: Save = {
        let (this, folder) = (this.clone(), folder.to_owned());
        Rc::new(move |chosen, cx| {
            let folder = folder.clone();
            this.update(cx, |bench, cx| {
                change_auto(
                    bench,
                    folder,
                    |params| match role {
                        Role::Builder => {
                            params.launcher = Some(chosen.launcher);
                            params.provider = Some(chosen.provider);
                            params.model = Some(chosen.model);
                        }
                        Role::Reviewer => params.reviewer = Some(Some(chosen)),
                    },
                    t!("Saved"),
                    cx,
                )
            })
        })
    };
    let set: model_picker::Set = {
        let (save, launcher, payer) = (save.clone(), shown.id.clone(), agent.provider.clone());
        Rc::new(move |id, value, _, cx| {
            if id == "model" {
                save(
                    BoardReviewerParams {
                        launcher: Some(launcher.clone()),
                        provider: payer.clone(),
                        model: Some(value.to_owned()),
                    },
                    cx,
                );
            }
        })
    };
    let moving: model_picker::Move = {
        let save = save.clone();
        Rc::new(move |to: model_picker::Moving, _, cx| {
            let launcher = launchers
                .iter()
                .find(|known| {
                    known.provider == to.provider
                        && to
                            .profile
                            .as_deref()
                            .is_none_or(|profile| profile_of(&known.id).as_deref() == Some(profile))
                })
                .map(|known| known.id.clone());
            save(
                BoardReviewerParams {
                    launcher,
                    provider: to.model_provider,
                    model: to.model,
                },
                cx,
            );
        })
    };
    let model_label = models.as_ref().and_then(|models| {
        (models.choices.iter())
            .find(|choice| choice.value == models.current)
            .map(|choice| choice.label.clone())
    });
    let label = match (&agent.launcher, model_label) {
        (None, _) if role == Role::Builder => t!("The first that works").to_owned(),
        (None, _) => t!("Any other agent").to_owned(),
        (Some(_), Some(model)) => format!("{} · {model}", shown.label),
        (Some(_), None) => shown.label.clone(),
    };
    let picked = model_picker::Picked {
        provider: shown.provider.clone(),
        profile: profile_of(&shown.id),
        model: models,
        speed: None,
        model_provider: agent.provider.clone(),
        locked: false,
        pending: None,
        set,
        moving,
        target,
    };
    let key = match role {
        Role::Builder => "playbook-auto-builder",
        Role::Reviewer => "playbook-auto-reviewer",
    };
    let mark = ui::mark(&shown.provider, 12.).into_any_element();
    let trigger = ui::select_field(key, Some(mark), label, false, cx);
    model_picker::around(trigger, key, picked, this).into_any_element()
}

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

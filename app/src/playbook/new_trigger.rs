//! The form a person sets up a trigger of their own with, over the Automation page: when it
//! fires, what it sends, and which chat it wakes.

use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled, Subscription, Window,
};
use sikemux_calls::call;
use sikemux_calls::triggers::{AddTriggerParams, TriggerOn, TriggerWakes};
use sikemux_corekit::accounts::{allows, protocol::PlanFeature};
use sikemux_view::chat::TriggerWhen;

use super::automation::{sentence, WEEKDAYS};
use super::said;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{
    radius_4, Tokens, SPACE_1H, SPACE_2, SPACE_3, SPACE_3H, SPACE_4, TEXT_2XS, TEXT_SM, TEXT_XS,
};
use crate::toast;
use crate::ui::{self, Button, Field, Segmented, Toggle, Variant};
use crate::workbench::Workbench;

#[derive(Clone, Copy, PartialEq)]
enum Repeat {
    Every,
    Daily,
    Once,
}

const REPEATS: [(Repeat, &str); 3] = [
    (Repeat::Every, n!("Every")),
    (Repeat::Daily, n!("Daily")),
    (Repeat::Once, n!("Once")),
];

const UNITS: [(u64, &str); 3] = [
    (60, n!("Minutes")),
    (3600, n!("Hours")),
    (86_400, n!("Days")),
];

/// The trigger being set up, kept while its form is open.
pub(super) struct NewTrigger {
    hook: bool,
    repeat: Repeat,
    count: Entity<InputState>,
    unit: usize,
    time: Entity<InputState>,
    /// The days it fires on, a bit each from Sunday's lowest.
    weekdays: u8,
    sends: Entity<TextareaState>,
    wakes: rows::Choice,
    /// The open chats it can wake, by id, in the choice's order; past them, a new chat.
    chats: Vec<String>,
    woken: usize,
    _typing: Vec<Subscription>,
}

/// The heading's way in: New trigger, or on a plan without it, PRO and the way to one.
pub(super) fn entry(
    bench: &Workbench,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let this = this.clone();
    let account = bench.this_view().and_then(|view| view.account.as_deref());
    if !allows(account, PlanFeature::OwnTriggers) {
        return Button::new("playbook-new-trigger", t!("New trigger"), Variant::Outline)
            .icon(Icon::Plus)
            .trail(ui::caps("Pro", cx))
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    bench.open_settings(crate::settings::Page::Account, window, cx)
                })
            })
            .into_any_element();
    }
    let folder = folder.to_owned();
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .children(bench.pro_free().map(|words| {
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(words)
        }))
        .child(
            Button::new("playbook-new-trigger", t!("New trigger"), Variant::Outline)
                .icon(Icon::Plus)
                .on_click(move |_, window, cx| {
                    this.update(cx, |bench, cx| toggle(bench, &folder, window, cx))
                }),
        )
        .into_any_element()
}

fn toggle(bench: &mut Workbench, folder: &str, window: &mut Window, cx: &mut Context<Workbench>) {
    let open = bench
        .settings_of(folder)
        .is_some_and(|settings| settings.new_trigger.is_some());
    let form = (!open).then(|| make(bench, folder, window, cx));
    if let Some(settings) = bench.settings_of(folder) {
        settings.new_trigger = form;
    }
    bench.parts.redraw_stage(cx);
}

fn make(
    bench: &Workbench,
    folder: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> NewTrigger {
    let count = cx.new(|cx| InputState::new(window, cx).default_value("1"));
    let time = cx.new(|cx| InputState::new(window, cx).default_value("9:00"));
    let sends = cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(3, 8)
            .placeholder(t!("What the chat is told when it fires"))
    });
    let project = bench.project_at(folder).map(|project| project.id.clone());
    let open: Vec<(String, String)> = bench.shown().map_or(Vec::new(), |view| {
        (view.workspace.agents.iter())
            .filter(|agent| Some(&agent.project) == project.as_ref() && agent.side_of.is_none())
            .map(|agent| {
                let name = agent.name.as_deref().map(|name| format!("@{name}"));
                let label = match name {
                    Some(name) if agent.title.is_empty() => name,
                    Some(name) => format!("{name} · {}", agent.title),
                    None => agent.title.clone(),
                };
                (agent.id.clone(), label)
            })
            .collect()
    });
    let (chats, mut labels): (Vec<String>, Vec<String>) = open.into_iter().unzip();
    labels.push(t!("A new chat in this project").to_owned());
    let picked = folder.to_owned();
    let wakes = rows::Choice::new(
        labels,
        0,
        move |bench, at, cx| {
            if let Some(form) = form_of(bench, &picked) {
                form.woken = at;
            }
            bench.parts.redraw_stage(cx);
        },
        window,
        cx,
    );
    let redraw = |bench: &mut Workbench, event: &InputEvent, cx: &mut Context<Workbench>| {
        if let InputEvent::Change = event {
            bench.parts.redraw_stage(cx);
        }
    };
    let typing = vec![
        cx.subscribe(&count, move |bench, _, event, cx| redraw(bench, event, cx)),
        cx.subscribe(&time, move |bench, _, event, cx| redraw(bench, event, cx)),
        cx.subscribe(&sends, move |bench, _, event, cx| redraw(bench, event, cx)),
    ];
    NewTrigger {
        hook: false,
        repeat: Repeat::Daily,
        count,
        unit: 1,
        time,
        weekdays: 0b0011_1110,
        sends,
        wakes,
        chats,
        woken: 0,
        _typing: typing,
    }
}

fn form_of<'a>(bench: &'a mut Workbench, folder: &str) -> Option<&'a mut NewTrigger> {
    bench.settings_of(folder)?.new_trigger.as_mut()
}

fn change(
    this: &Entity<Workbench>,
    folder: &str,
    edit: impl FnOnce(&mut NewTrigger),
    cx: &mut gpui_kit::App,
) {
    this.update(cx, |bench, cx| {
        if let Some(form) = form_of(bench, folder) {
            edit(form);
        }
        bench.parts.redraw_stage(cx);
    })
}

/// When it fires, as the core is asked for it and as the page says it; none while what is
/// typed does not make a time.
fn rule(form: &NewTrigger, cx: &gpui_kit::App) -> Option<(TriggerOn, TriggerWhen)> {
    if form.hook {
        return Some((
            TriggerOn::Webhook,
            TriggerWhen::Webhook { url: String::new() },
        ));
    }
    match form.repeat {
        Repeat::Every => {
            let count: u64 = form.count.read(cx).value().trim().parse().ok()?;
            let seconds = count.checked_mul(UNITS.get(form.unit)?.0)?;
            (count > 0).then_some((TriggerOn::Every { seconds }, TriggerWhen::Every { seconds }))
        }
        Repeat::Daily => {
            let minute = minute_of(&form.time.read(cx).value())?;
            let weekdays: Vec<u8> = (0..7u8)
                .filter(|day| form.weekdays & (1 << day) != 0)
                .collect();
            (!weekdays.is_empty()).then(|| {
                let shown = match weekdays.len() {
                    7 => Vec::new(),
                    _ => weekdays.clone(),
                };
                (
                    TriggerOn::Daily { minute, weekdays },
                    TriggerWhen::Daily {
                        minute,
                        weekdays: shown,
                    },
                )
            })
        }
        Repeat::Once => {
            let at = next_at(minute_of(&form.time.read(cx).value())?)?;
            Some((TriggerOn::Once { at }, TriggerWhen::Once { at }))
        }
    }
}

/// Minutes into the day of a time typed as 8:30 or 8.
fn minute_of(typed: &str) -> Option<u32> {
    let (hours, minutes) = typed.trim().split_once(':').unwrap_or((typed.trim(), "0"));
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// The next time the clock here reads `minute`, in Unix milliseconds.
fn next_at(minute: u32) -> Option<u64> {
    let now = chrono::Local::now();
    let time = chrono::NaiveTime::from_hms_opt(minute / 60, minute % 60, 0)?;
    let today = now.date_naive().and_time(time);
    let day = match today > now.naive_local() {
        true => today,
        false => today + chrono::Duration::days(1),
    };
    let at = day.and_local_timezone(chrono::Local).earliest()?;
    u64::try_from(at.timestamp_millis()).ok()
}

fn sends_of(form: &NewTrigger, cx: &gpui_kit::App) -> String {
    form.sends.read(cx).value().trim().to_owned()
}

/// The form, floating under the heading's button over the list.
pub(super) fn sheet(
    bench: &Workbench,
    form: &NewTrigger,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let sends = sends_of(form, cx);
    let rule = rule(form, cx);
    let summary = match (&rule, sends.lines().next()) {
        (Some((_, when)), Some(first)) => sentence(when, first),
        (None, _) => t!("Say when it runs.").to_owned(),
        (_, None) => t!("Say what it sends.").to_owned(),
    };
    let ready = rule.is_some() && !sends.is_empty();
    let head = div()
        .flex()
        .items_baseline()
        .gap(SPACE_2)
        .px(SPACE_4)
        .pt(SPACE_3H)
        .pb(SPACE_2)
        .child(
            ui::text(t!("New trigger"), TEXT_SM, FontWeight::SEMIBOLD).text_color(t.text_primary),
        )
        .children(bench.pro_free().map(|words| {
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(words)
        }));
    let foot = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .px(SPACE_4)
        .py(SPACE_3)
        .border_t(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface_sunken)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_secondary)
                .child(summary),
        )
        .child({
            let (this, folder) = (this.clone(), folder.to_owned());
            Button::new("new-trigger-cancel", t!("Cancel"), Variant::Ghost).on_click(
                move |_, _, cx| {
                    this.update(cx, |bench, cx| {
                        if let Some(settings) = bench.settings_of(&folder) {
                            settings.new_trigger = None;
                        }
                        bench.parts.redraw_stage(cx);
                    })
                },
            )
        })
        .child({
            let (this, folder) = (this.clone(), folder.to_owned());
            Button::new("new-trigger-add", t!("Add trigger"), Variant::Primary)
                .disabled(!ready)
                .on_click(move |_, _, cx| this.update(cx, |bench, cx| add(bench, &folder, cx)))
        });
    div()
        .id("new-trigger")
        .occlude()
        .absolute()
        .top(px(52.))
        .right(SPACE_3)
        .w(px(470.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(t.surface_overlay)
        .border(crate::theme::border())
        .border_color(t.border)
        .rounded(radius_4())
        .shadow(t.shadow_float())
        .child(head)
        .child(part(
            t!("When"),
            t!("On a schedule, or each time an address is called"),
            when(form, folder, this, cx),
            cx,
        ))
        .child(part(
            t!("Sends"),
            t!("What the chat is told when it fires"),
            div()
                .border(crate::theme::border())
                .border_color(t.line)
                .rounded(crate::theme::radius_2())
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(Textarea::new(&form.sends).bordered(false))
                .into_any_element(),
            cx,
        ))
        .child(part(
            t!("Wakes"),
            t!("A chat, or a new one in this project"),
            form.wakes.view().w(px(290.)).into_any_element(),
            cx,
        ))
        .child(foot)
        .into_any_element()
}

/// A row of the form: its name and what it asks on the left, the control on the right.
fn part(name: &str, about: &str, control: AnyElement, cx: &gpui_kit::App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .gap(SPACE_3)
        .px(SPACE_4)
        .py(SPACE_3)
        .border_t(crate::theme::border())
        .border_color(t.line_soft)
        .child(
            div()
                .w(px(118.))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    ui::text(name.to_owned(), TEXT_XS, FontWeight::MEDIUM)
                        .text_color(t.text_primary),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(about.to_owned()),
                ),
        )
        .child(div().flex_1().min_w_0().child(control))
        .into_any_element()
}

fn when(
    form: &NewTrigger,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &gpui_kit::App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let kind = {
        let (this, folder) = (this.clone(), folder.to_owned());
        Segmented::new(
            "new-trigger-kind",
            vec![
                (Icon::Clock, t!("Schedule").into()),
                (Icon::Link, t!("Webhook").into()),
            ],
            usize::from(form.hook),
        )
        .on_select(move |at, _, cx| change(&this, &folder, |form| form.hook = *at == 1, cx))
    };
    let repeat = {
        let (this, folder) = (this.clone(), folder.to_owned());
        let chosen = REPEATS
            .iter()
            .position(|(repeat, _)| *repeat == form.repeat)
            .unwrap_or_default();
        Segmented::words(
            "new-trigger-repeat",
            REPEATS
                .iter()
                .map(|(_, word)| (SharedString::from(crate::i18n::tr(word)), None))
                .collect(),
            chosen,
        )
        .on_select(move |at, _, cx| {
            let repeat = REPEATS.get(*at).map(|(repeat, _)| *repeat);
            change(
                &this,
                &folder,
                |form| form.repeat = repeat.unwrap_or(form.repeat),
                cx,
            )
        })
    };
    let words = |said: &str| {
        div()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .child(said.to_owned())
    };
    let timed = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(words(t!("at")))
        .child(
            div()
                .w(px(64.))
                .child(Field::new(&form.time).small().mono()),
        );
    let every = {
        let (this, folder) = (this.clone(), folder.to_owned());
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .child(
                div()
                    .w(px(52.))
                    .child(Field::new(&form.count).small().mono()),
            )
            .child(
                Segmented::words(
                    "new-trigger-unit",
                    UNITS
                        .iter()
                        .map(|(_, word)| (SharedString::from(crate::i18n::tr(word)), None))
                        .collect(),
                    form.unit,
                )
                .on_select(move |at, _, cx| change(&this, &folder, |form| form.unit = *at, cx)),
            )
    };
    let days = div().flex().gap(px(2.)).children((0..7).map(|day| {
        let (this, folder) = (this.clone(), folder.to_owned());
        let letter: String = WEEKDAYS
            .get(day)
            .map(|name| crate::i18n::tr(name).chars().take(1).collect())
            .unwrap_or_default();
        Toggle::new(
            SharedString::from(format!("new-trigger-day-{day}")),
            letter,
            form.weekdays & (1 << day) != 0,
        )
        .on_click(move |_, _, cx| change(&this, &folder, |form| form.weekdays ^= 1 << day, cx))
    }));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(kind)
        .when(!form.hook, |column| {
            column
                .child(repeat)
                .child(match form.repeat {
                    Repeat::Every => every.into_any_element(),
                    Repeat::Daily | Repeat::Once => timed.into_any_element(),
                })
                .when(form.repeat == Repeat::Daily, |column| column.child(days))
        })
        .when(form.hook, |column| {
            column.child(words(t!(
                "It gets its address once added; each request to it fires the trigger."
            )))
        })
        .into_any_element()
}

fn add(bench: &mut Workbench, folder: &str, cx: &mut Context<Workbench>) {
    let Some(form) = bench
        .project_settings
        .get(folder)
        .and_then(|settings| settings.new_trigger.as_ref())
    else {
        return;
    };
    let Some((when, _)) = rule(form, cx) else {
        return;
    };
    let wakes = match form.chats.get(form.woken) {
        Some(agent) => TriggerWakes::Chat {
            agent: agent.clone(),
        },
        None => TriggerWakes::Project {
            project: folder.to_owned(),
        },
    };
    let asked = bench
        .target()
        .request_typed::<call::AddTrigger>(AddTriggerParams {
            title: None,
            prompt: sends_of(form, cx),
            when,
            wakes,
        });
    let folder = folder.to_owned();
    bench.report(
        asked,
        t!("The trigger was not added"),
        move |bench, _, cx| {
            if let Some(settings) = bench.settings_of(&folder) {
                settings.new_trigger = None;
            }
            bench.reload_playbook(&folder, cx);
            bench.notify(toast::success(t!("Trigger added")), cx);
        },
        cx,
    );
}

pub(super) fn said_by_you(at: u64) -> String {
    t!("You set this up · {day}", day = said::day(at))
}

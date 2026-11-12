//! The Automation page: what runs in the project with nobody asking, one sentence each, with
//! Pause or Resume and Remove.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::server::board::BoardSettingsParams;
use sikemux_core::server::call;
use sikemux_core::server::triggers::{HoldTriggerParams, TriggerParams};
use sikemux_view::chat::ChatTrigger;
use sikemux_view::playbook::{Playbook, RunsAlone};

use super::said::{self, Moment};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1H};
use crate::toast;
use crate::ui::{self, Button, IconButton, Variant};
use crate::workbench::Workbench;

pub(super) fn page(
    playbook: Option<&Playbook>,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(playbook) = playbook else {
        return vec![rows::section(
            "Automation",
            None,
            None,
            false,
            rows::note("Reading…", cx),
            cx,
        )];
    };
    let lines: Vec<AnyElement> = match playbook.automation.is_empty() {
        true => vec![rows::note(
            "Nothing yet. Ask any agent to do something on a schedule, or when CI ends, and it shows here.",
            cx,
        )],
        false => (playbook.automation.iter())
            .map(|line| row(line, folder, this, cx))
            .collect(),
    };
    let count = playbook.automation.len();
    vec![rows::section(
        "Automation",
        (count > 0).then(|| SharedString::from(count.to_string())),
        Some("What runs with nobody asking.".into()),
        false,
        div().children(lines),
        cx,
    )]
}

fn row(
    line: &RunsAlone,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let folder = folder.to_owned();
    let (key, label, words, moment, paused) = match line {
        RunsAlone::Auto { slots, ceiling } => {
            let at_once = match slots {
                1 => "one at a time".to_owned(),
                slots => format!("{slots} at once"),
            };
            let label = format!("Start agents on the cards nobody holds, {at_once}.");
            let mut words =
                "Each works on its own branch, and another agent reviews it before it lands"
                    .to_owned();
            if let Some(ceiling) = ceiling {
                words.push_str(&format!(" · stops starting new ones past ${ceiling}"));
            }
            ("auto".to_owned(), label, words, None, false)
        }
        RunsAlone::Trigger {
            id,
            title,
            when,
            next_at,
            set,
            paused,
        } => {
            let fires = crate::chat::live::fires(&ChatTrigger {
                id: *id,
                title: title.clone(),
                when: when.clone(),
                next_at: *next_at,
            });
            let words = match (paused, set) {
                (Some(at), _) => format!("Paused by you · {}", said::day(*at)),
                (None, Some(set)) => format!(
                    "{fires} · {} set this up · {}",
                    said::at_name(set.name.as_deref()),
                    said::day(set.at)
                ),
                (None, None) => fires,
            };
            let moment = set.as_deref().and_then(Moment::of);
            (
                format!("trigger-{id}"),
                title.clone(),
                words,
                moment,
                paused.is_some(),
            )
        }
    };
    let line_words = said::line(
        format!("playbook-{key}"),
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
        .child(hold_button(line, &key, paused, &folder, this))
        .children(remove_button(line, &key, this));
    rows::row_parts(
        Some(
            ui::icon(Icon::Bolt, 14.)
                .text_color(t.text_secondary)
                .into_any_element(),
        ),
        div()
            .when(paused, |label| label.text_color(t.text_secondary))
            .child(label),
        Some(line_words),
        controls,
        cx,
    )
    .into_any_element()
}

fn hold_button(
    line: &RunsAlone,
    key: &str,
    paused: bool,
    folder: &str,
    this: &Entity<Workbench>,
) -> Button {
    let (this, folder) = (this.clone(), folder.to_owned());
    let line = line.clone();
    Button::new(
        SharedString::from(format!("playbook-{key}-hold")),
        if paused { "Resume" } else { "Pause" },
        Variant::Ghost,
    )
    .on_click(move |_, _, cx| {
        let (line, folder) = (line.clone(), folder.clone());
        this.update(cx, |bench, cx| match line {
            RunsAlone::Auto { .. } => {
                let asked =
                    bench
                        .target()
                        .request_typed::<call::BoardSettings>(BoardSettingsParams {
                            project: folder,
                            auto: Some(false),
                            ..BoardSettingsParams::default()
                        });
                bench.report(
                    asked,
                    "Auto was not paused",
                    |bench, _, cx| {
                        bench.notify(
                            toast::success("Agents no longer start on cards by themselves"),
                            cx,
                        )
                    },
                    cx,
                );
            }
            RunsAlone::Trigger { id, .. } => {
                let asked = bench
                    .target()
                    .request_typed::<call::HoldTrigger>(HoldTriggerParams {
                        trigger: id,
                        paused: !paused,
                    });
                bench.report(
                    asked,
                    "It was not changed",
                    move |bench, _, cx| {
                        let said = if paused { "Resumed" } else { "Paused" };
                        bench.notify(toast::success(said), cx)
                    },
                    cx,
                );
            }
        });
    })
}

fn remove_button(line: &RunsAlone, key: &str, this: &Entity<Workbench>) -> Option<IconButton> {
    let RunsAlone::Trigger { id, .. } = line else {
        return None;
    };
    let (this, id) = (this.clone(), *id);
    Some(
        IconButton::new(
            SharedString::from(format!("playbook-{key}-remove")),
            Icon::Trash,
            "Remove",
        )
        .on_click(move |_, _, cx| {
            this.update(cx, |bench, cx| {
                let asked = bench
                    .target()
                    .request_typed::<call::RemoveTrigger>(TriggerParams { trigger: id });
                bench.report(
                    asked,
                    "It was not removed",
                    |bench, _, cx| bench.notify(toast::success("Removed"), cx),
                    cx,
                );
            })
        }),
    )
}
